//! Clipboard synchronization between host and guest.
//!
//! `ClipboardSender`: monitors the local clipboard for changes and sends
//! `InputMessage::Clipboard` events on the input stream.
//!
//! `ClipboardReceiver`: receives `InputMessage::Clipboard` events and writes
//! the content to the local clipboard.
//!
//! Size limit: payloads > 1MB are rejected on both send and receive.
//! Auto-sync is opt-in; manual sync is triggered via `ClipboardSenderHandle::sync_now()`.

use std::time::Duration;

use arboard::Clipboard;
use pairwire_core::{Error, Result};
use pairwire_protocol::input::{ClipboardFormat, ClipboardSync, InputMessage};
use tokio::{
    sync::{mpsc, oneshot},
    task::JoinHandle,
    time,
};
use tracing::{debug, info, instrument, warn};

/// Maximum clipboard payload size (1 MB).
pub const MAX_CLIPBOARD_BYTES: usize = 1024 * 1024;

/// Poll interval for clipboard change detection.
const POLL_INTERVAL: Duration = Duration::from_millis(500);

/// Handle for the clipboard sender task.
pub struct ClipboardSenderHandle {
    /// Send on this channel to trigger an immediate sync regardless of change detection.
    pub sync_tx: mpsc::Sender<()>,
    stop_tx: oneshot::Sender<()>,
    join: JoinHandle<()>,
}

impl ClipboardSenderHandle {
    /// Trigger an immediate clipboard sync.
    pub async fn sync_now(&self) {
        let _ = self.sync_tx.send(()).await;
    }

    pub async fn stop(self) {
        let _ = self.stop_tx.send(());
        let _ = self.join.await;
    }
}

/// Monitors the local clipboard and sends changes as `InputMessage::Clipboard`.
///
/// Polls every 500ms. Only sends when content has changed since last send.
/// Payloads > 1MB are skipped with a warning.
pub struct ClipboardSender {
    /// Channel to push clipboard messages onto (shared with `InputSender` or direct stream).
    event_tx: mpsc::Sender<InputMessage>,
    /// Whether to automatically send on every detected change.
    auto_sync: bool,
}

impl ClipboardSender {
    #[must_use]
    pub fn new(event_tx: mpsc::Sender<InputMessage>, auto_sync: bool) -> Self {
        Self {
            event_tx,
            auto_sync,
        }
    }

    #[instrument(skip(self))]
    pub fn spawn(self) -> ClipboardSenderHandle {
        let (stop_tx, mut stop_rx) = oneshot::channel();
        let (sync_tx, mut sync_rx) = mpsc::channel::<()>(4);
        let event_tx = self.event_tx;
        let auto_sync = self.auto_sync;

        let join = tokio::spawn(async move {
            info!(auto_sync, "clipboard sender started");

            // Clipboard access must happen on a thread that can block.
            // We use spawn_blocking for each poll to avoid blocking the tokio runtime.
            let mut last_text: Option<String> = None;
            let mut interval = time::interval(POLL_INTERVAL);

            loop {
                tokio::select! {
                    biased;
                    _ = &mut stop_rx => break,
                    _ = sync_rx.recv() => {
                        // Manual sync requested — send regardless of change.
                        if let Some(msg) = read_clipboard_text().await {
                            send_clipboard(&event_tx, msg).await;
                        }
                    }
                    _ = interval.tick() => {
                        if !auto_sync {
                            continue;
                        }
                        if let Some(text) = read_clipboard_text().await
                            && last_text.as_deref() != Some(text.content.as_str())
                        {
                            last_text = Some(text.content.clone());
                            debug!(bytes = text.content.len(), "clipboard changed");
                            send_clipboard(&event_tx, text).await;
                        }
                    }
                }
            }
            info!("clipboard sender stopped");
        });

        ClipboardSenderHandle {
            sync_tx,
            stop_tx,
            join,
        }
    }
}

struct ClipboardText {
    content: String,
}

async fn read_clipboard_text() -> Option<ClipboardText> {
    tokio::task::spawn_blocking(|| {
        let mut cb = Clipboard::new().ok()?;
        let text = cb.get_text().ok()?;
        if text.is_empty() {
            return None;
        }
        if text.len() > MAX_CLIPBOARD_BYTES {
            warn!(
                bytes = text.len(),
                "clipboard content exceeds 1MB limit, skipping"
            );
            return None;
        }
        Some(ClipboardText { content: text })
    })
    .await
    .ok()
    .flatten()
}

async fn send_clipboard(tx: &mpsc::Sender<InputMessage>, text: ClipboardText) {
    let msg = InputMessage::Clipboard(ClipboardSync {
        format: ClipboardFormat::PlainText,
        data: text.content.into_bytes(),
    });
    if tx.send(msg).await.is_err() {
        debug!("clipboard event_tx closed");
    }
}

/// Handle for the clipboard receiver task.
pub struct ClipboardReceiverHandle {
    stop_tx: oneshot::Sender<()>,
    join: JoinHandle<()>,
}

impl ClipboardReceiverHandle {
    pub async fn stop(self) {
        let _ = self.stop_tx.send(());
        let _ = self.join.await;
    }
}

/// Receives `ClipboardSync` messages and writes them to the local clipboard.
///
/// Payloads > 1MB are rejected. Only `PlainText` is supported in Phase 2.
pub struct ClipboardReceiver {
    /// Receives `InputMessage::Clipboard` events (filtered from the input stream).
    event_rx: mpsc::Receiver<ClipboardSync>,
}

impl ClipboardReceiver {
    #[must_use]
    pub fn new(event_rx: mpsc::Receiver<ClipboardSync>) -> Self {
        Self { event_rx }
    }

    #[instrument(skip(self))]
    pub fn spawn(self) -> ClipboardReceiverHandle {
        let (stop_tx, mut stop_rx) = oneshot::channel();
        let mut event_rx = self.event_rx;

        let join = tokio::spawn(async move {
            info!("clipboard receiver started");
            loop {
                tokio::select! {
                    biased;
                    _ = &mut stop_rx => break,
                    msg = event_rx.recv() => {
                        match msg {
                            None => break,
                            Some(sync) => {
                                if let Err(e) = apply_clipboard(sync).await {
                                    warn!(error = %e, "clipboard apply failed");
                                }
                            }
                        }
                    }
                }
            }
            info!("clipboard receiver stopped");
        });

        ClipboardReceiverHandle { stop_tx, join }
    }
}

async fn apply_clipboard(sync: ClipboardSync) -> Result<()> {
    if sync.data.len() > MAX_CLIPBOARD_BYTES {
        return Err(Error::Protocol(format!(
            "clipboard payload {} bytes exceeds 1MB limit",
            sync.data.len()
        )));
    }

    match sync.format {
        ClipboardFormat::PlainText => {
            let text = String::from_utf8(sync.data)
                .map_err(|e| Error::Protocol(format!("clipboard UTF-8 error: {e}")))?;
            tokio::task::spawn_blocking(move || {
                let mut cb = Clipboard::new()
                    .map_err(|e| Error::Protocol(format!("clipboard open: {e}")))?;
                cb.set_text(&text)
                    .map_err(|e| Error::Protocol(format!("clipboard set: {e}")))?;
                debug!(bytes = text.len(), "clipboard updated");
                Ok::<(), Error>(())
            })
            .await
            .map_err(|e| Error::Protocol(format!("spawn_blocking: {e}")))??;
        }
        other => {
            warn!(format = ?other, "clipboard format not supported in Phase 2, skipping");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use pairwire_protocol::input::ClipboardFormat;

    #[test]
    fn clipboard_sync_size_limit() {
        // Verify the constant is what we expect
        assert_eq!(MAX_CLIPBOARD_BYTES, 1024 * 1024);
    }

    #[test]
    fn clipboard_format_plain_text_round_trip() {
        use postcard::{from_bytes, to_allocvec};
        let sync = ClipboardSync {
            format: ClipboardFormat::PlainText,
            data: b"hello from clipboard".to_vec(),
        };
        let bytes = to_allocvec(&sync).unwrap();
        let decoded: ClipboardSync = from_bytes(&bytes).unwrap();
        assert_eq!(sync, decoded);
    }

    #[tokio::test]
    async fn apply_clipboard_rejects_oversized() {
        let sync = ClipboardSync {
            format: ClipboardFormat::PlainText,
            data: vec![b'x'; MAX_CLIPBOARD_BYTES + 1],
        };
        let result = apply_clipboard(sync).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("1MB"));
    }
}
