//! Host-side input receiver: reads `InputMessage` from QUIC stream, injects into OS.

use std::sync::{
    Arc,
    atomic::{AtomicU8, Ordering},
};

use iroh::endpoint::RecvStream;
use pairwire_core::{Error, Result};
use pairwire_protocol::input::{ClipboardSync, InputMessage};
use pairwire_transport::connection::recv_message;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tracing::{debug, info, instrument, warn};

use crate::InputPermission;

/// Atomic encoding of `InputPermission` for lock-free reads.
///
/// 0 = Denied, 1 = `FullControl`, 2 = `KeyboardOnly`
#[derive(Default)]
struct AtomicPermission(AtomicU8);

impl AtomicPermission {
    fn load(&self) -> InputPermission {
        match self.0.load(Ordering::Relaxed) {
            1 => InputPermission::FullControl,
            2 => InputPermission::KeyboardOnly,
            _ => InputPermission::Denied,
        }
    }

    fn store(&self, p: InputPermission) {
        let v = match p {
            InputPermission::Denied => 0,
            InputPermission::FullControl => 1,
            InputPermission::KeyboardOnly => 2,
        };
        self.0.store(v, Ordering::Relaxed);
    }
}

/// Spawned task handle for the input receiver.
pub struct InputReceiverHandle {
    permission: Arc<AtomicPermission>,
    stop_tx: oneshot::Sender<()>,
    join: JoinHandle<()>,
}

impl InputReceiverHandle {
    /// Grant or revoke input injection permission at runtime.
    pub fn set_permission(&self, p: InputPermission) {
        self.permission.store(p);
    }

    /// Signal the receiver to stop and wait for it to finish.
    pub async fn stop(self) {
        let _ = self.stop_tx.send(());
        let _ = self.join.await;
    }
}

/// Receives `InputMessage` events from a QUIC stream and injects them into the OS.
///
/// Events are silently dropped when permission is `Denied`.
/// Clipboard messages are forwarded to `clipboard_tx` if provided, bypassing injection.
pub struct InputReceiver {
    stream: RecvStream,
    /// If set, `InputMessage::Clipboard` payloads are forwarded here instead of injected.
    clipboard_tx: Option<mpsc::Sender<ClipboardSync>>,
}

impl InputReceiver {
    #[must_use]
    pub fn new(stream: RecvStream) -> Self {
        Self {
            stream,
            clipboard_tx: None,
        }
    }

    /// Attach a clipboard channel. `InputMessage::Clipboard` payloads will be
    /// forwarded to this sender rather than passed to the OS injection path.
    #[must_use]
    pub fn with_clipboard(mut self, tx: mpsc::Sender<ClipboardSync>) -> Self {
        self.clipboard_tx = Some(tx);
        self
    }

    /// Spawn the receiver task.
    ///
    /// Initial permission is `Denied`. Call `handle.set_permission()` after a
    /// `ControlHandoff` message grants control.
    #[instrument(skip(self))]
    pub fn spawn(self) -> InputReceiverHandle {
        let (stop_tx, mut stop_rx) = oneshot::channel();
        let permission = Arc::new(AtomicPermission::default());
        let perm_clone = Arc::clone(&permission);
        let mut stream = self.stream;
        let clipboard_tx = self.clipboard_tx;

        let join = tokio::spawn(async move {
            info!("input receiver started");
            loop {
                tokio::select! {
                    biased;
                    _ = &mut stop_rx => break,
                    result = recv_input(&mut stream) => {
                        match result {
                            Err(e) => {
                                // EOF or connection close — normal shutdown
                                let msg = e.to_string();
                                if msg.contains("closed") || msg.contains("reset") || msg.contains("eof") {
                                    break;
                                }
                                warn!(error = %e, "input recv error");
                                break;
                            }
                            Ok(event) => {
                                match event {
                                    // Clipboard messages are routed to the clipboard subsystem,
                                    // not injected as OS events.
                                    InputMessage::Clipboard(sync) => {
                                        if let Some(ref tx) = clipboard_tx {
                                            if tx.send(sync).await.is_err() {
                                                debug!("clipboard_tx closed, dropping clipboard message");
                                            }
                                        } else {
                                            debug!("clipboard message received but no clipboard_tx configured");
                                        }
                                    }
                                    other => {
                                        let perm = perm_clone.load();
                                        if perm == InputPermission::Denied {
                                            debug!("input event dropped — permission denied");
                                            continue;
                                        }
                                        debug!(event = ?other, ?perm, "injecting input event");
                                        if let Err(e) = inject_event(&other, perm) {
                                            warn!(error = %e, "input injection failed");
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            info!("input receiver stopped");
        });

        InputReceiverHandle {
            permission,
            stop_tx,
            join,
        }
    }
}

async fn recv_input(stream: &mut RecvStream) -> Result<InputMessage> {
    recv_message(stream)
        .await
        .map_err(|e| Error::Transport(format!("input recv: {e}")))
}

/// Dispatch to the platform-specific injection implementation.
fn inject_event(event: &InputMessage, permission: InputPermission) -> Result<()> {
    #[cfg(target_os = "macos")]
    return crate::inject_macos::inject(event, permission);

    #[cfg(not(target_os = "macos"))]
    return crate::inject_stub::inject(event, permission);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atomic_permission_round_trips() {
        let ap = AtomicPermission::default();
        assert_eq!(ap.load(), InputPermission::Denied);

        ap.store(InputPermission::FullControl);
        assert_eq!(ap.load(), InputPermission::FullControl);

        ap.store(InputPermission::KeyboardOnly);
        assert_eq!(ap.load(), InputPermission::KeyboardOnly);

        ap.store(InputPermission::Denied);
        assert_eq!(ap.load(), InputPermission::Denied);
    }
}
