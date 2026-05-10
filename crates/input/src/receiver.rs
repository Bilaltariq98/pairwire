//! Host-side input receiver: reads `InputMessage` from QUIC stream, injects into OS.

use std::sync::{
    Arc,
    atomic::{AtomicU8, AtomicU64, Ordering},
};
use std::time::{SystemTime, UNIX_EPOCH};

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
///
/// Made public so the host control-state task can share the same
/// `Arc<AtomicPermission>` that drives injection without holding
/// an `InputReceiverHandle` (which consumes `self` on stop).
#[derive(Default)]
pub struct AtomicPermission(AtomicU8);

impl AtomicPermission {
    fn load(&self) -> InputPermission {
        match self.0.load(Ordering::Relaxed) {
            1 => InputPermission::FullControl,
            2 => InputPermission::KeyboardOnly,
            _ => InputPermission::Denied,
        }
    }

    /// Public alias for `load` — used by external consumers such as the
    /// host control-state task that holds an `Arc<AtomicPermission>`.
    #[must_use]
    pub fn load_permission(&self) -> InputPermission {
        self.load()
    }

    fn store(&self, p: InputPermission) {
        let v = match p {
            InputPermission::Denied => 0,
            InputPermission::FullControl => 1,
            InputPermission::KeyboardOnly => 2,
        };
        self.0.store(v, Ordering::Relaxed);
    }

    /// Public alias for `store` — sets the permission atomically.
    ///
    /// Used by the host control-state task to flip input gating without
    /// going through `InputReceiverHandle` (which consumes `self` on stop).
    pub fn set(&self, p: InputPermission) {
        self.store(p);
    }
}

/// Records the wall-clock millis of the most recent successful input injection.
///
/// Used by the host's auto-revoke watcher to distinguish guest-driven cursor
/// motion (which is closely preceded by an injection) from host-driven motion
/// (no recent injection). Shareable via `Arc`; reads are lock-free.
#[derive(Default)]
pub struct InjectProbe {
    last_ms: AtomicU64,
}

impl InjectProbe {
    /// Stamp the probe with the current wall-clock millis.
    pub fn mark_now(&self) {
        let ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX));
        self.last_ms.store(ms, Ordering::Relaxed);
    }

    /// Wall-clock millis of the most recent injection (0 if never marked).
    #[must_use]
    pub fn last_ms(&self) -> u64 {
        self.last_ms.load(Ordering::Relaxed)
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

    /// Return a cloneable handle to the permission atomic so that an external
    /// task (e.g. the host control-state machine) can flip gating independently
    /// without consuming the receiver handle.
    #[must_use]
    pub fn permission_handle(&self) -> Arc<AtomicPermission> {
        Arc::clone(&self.permission)
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
    /// If set, stamped on every successful injection so external watchers can
    /// distinguish guest-driven activity from host-driven activity.
    inject_probe: Option<Arc<InjectProbe>>,
}

impl InputReceiver {
    #[must_use]
    pub fn new(stream: RecvStream) -> Self {
        Self {
            stream,
            clipboard_tx: None,
            inject_probe: None,
        }
    }

    /// Attach a clipboard channel. `InputMessage::Clipboard` payloads will be
    /// forwarded to this sender rather than passed to the OS injection path.
    #[must_use]
    pub fn with_clipboard(mut self, tx: mpsc::Sender<ClipboardSync>) -> Self {
        self.clipboard_tx = Some(tx);
        self
    }

    /// Attach an `InjectProbe` that gets stamped after every successful injection.
    #[must_use]
    pub fn with_inject_probe(mut self, probe: Arc<InjectProbe>) -> Self {
        self.inject_probe = Some(probe);
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
        let inject_probe = self.inject_probe;

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
                                if msg.contains("closed")
                                    || msg.contains("reset")
                                    || msg.contains("eof")
                                    || msg.contains("connection lost")
                                    || msg.contains("timed out")
                                {
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
                                        match inject_event(&other, perm) {
                                            Ok(()) => {
                                                if let Some(ref probe) = inject_probe {
                                                    probe.mark_now();
                                                }
                                            }
                                            Err(e) => warn!(error = %e, "input injection failed"),
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

    #[test]
    fn atomic_permission_public_set_and_load() {
        let ap = AtomicPermission::default();
        assert_eq!(ap.load_permission(), InputPermission::Denied);

        ap.set(InputPermission::FullControl);
        assert_eq!(ap.load_permission(), InputPermission::FullControl);

        ap.set(InputPermission::KeyboardOnly);
        assert_eq!(ap.load_permission(), InputPermission::KeyboardOnly);
    }
}
