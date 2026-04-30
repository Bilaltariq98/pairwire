//! Persistent bidi control-message stream.
//!
//! Opened after the one-shot `Handshake` exchange. Carries `ControlMessage`
//! variants — `ControlRequest`, `ControlGrant`, `ControlRevoke`,
//! `ControlHandoff`, `Goodbye`, `SetSharing` — for the lifetime of the session.

use iroh::endpoint::{RecvStream, SendStream};
use pairwire_core::{Error, Result};
use pairwire_protocol::ControlMessage;
use tokio::sync::{broadcast, mpsc};
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};

use crate::connection::{recv_message, send_message};

const INBOUND_BUFFER: usize = 32;

/// Persistent bidirectional control-message stream.
///
/// Opened once per session, after the one-shot handshake stream closes.
/// Both peers keep this stream open for the duration of the session.
pub struct ControlChannel;

/// Handle to the running `ControlChannel` tasks.
///
/// Use [`ControlChannelHandle::subscribe`] to receive inbound messages.
/// Use [`ControlChannelHandle::stop`] to abort both tasks and release resources.
pub struct ControlChannelHandle {
    inbound_tx: broadcast::Sender<ControlMessage>,
    send_task: JoinHandle<()>,
    recv_task: JoinHandle<()>,
}

impl ControlChannel {
    /// Spawn send + recv tasks for the persistent control stream.
    ///
    /// `outbound_rx` — messages this peer wants to send to the other side.
    /// Use [`ControlChannelHandle::subscribe`] to read inbound messages.
    pub fn spawn(
        mut send: SendStream,
        mut recv: RecvStream,
        mut outbound_rx: mpsc::Receiver<ControlMessage>,
    ) -> ControlChannelHandle {
        let (inbound_tx, _) = broadcast::channel(INBOUND_BUFFER);
        let inbound_tx_for_recv = inbound_tx.clone();

        let send_task = tokio::spawn(async move {
            info!("control channel send task started");
            while let Some(msg) = outbound_rx.recv().await {
                if let Err(e) = send_message(&mut send, &msg).await {
                    warn!(error = %e, "control channel send failed");
                    break;
                }
                debug!(?msg, "control message sent");
            }
            info!("control channel send task stopped");
        });

        let recv_task = tokio::spawn(async move {
            info!("control channel recv task started");
            loop {
                let result: Result<ControlMessage> = recv_message(&mut recv)
                    .await
                    .map_err(|e| Error::Transport(format!("control recv: {e}")));
                match result {
                    Ok(m) => {
                        debug!(msg = ?m, "control message received");
                        let _ = inbound_tx_for_recv.send(m);
                    }
                    Err(e) => {
                        let s = e.to_string();
                        if s.contains("closed")
                            || s.contains("eof")
                            || s.contains("reset")
                            || s.contains("finish")
                        {
                            info!("control channel recv stream closed cleanly");
                        } else {
                            warn!(error = %e, "control channel recv error");
                        }
                        break;
                    }
                }
            }
            info!("control channel recv task stopped");
        });

        ControlChannelHandle {
            inbound_tx,
            send_task,
            recv_task,
        }
    }
}

impl ControlChannelHandle {
    /// Subscribe to inbound messages. Each subscriber receives every message
    /// produced after the call to `subscribe`.
    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<ControlMessage> {
        self.inbound_tx.subscribe()
    }

    /// Abort both tasks and wait for them to finish.
    pub async fn stop(self) {
        self.send_task.abort();
        self.recv_task.abort();
        let _ = self.send_task.await;
        let _ = self.recv_task.await;
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use pairwire_protocol::{ControlMessage, ControlRequest};
    use tokio::sync::mpsc;

    use super::ControlChannel;

    #[tokio::test]
    async fn control_channel_round_trips_request() {
        let (peer_a, peer_b) = crate::test_utils::loopback_pair().await;

        // Pre-queue A's outbound message before opening the stream so that A's
        // send task can write to the wire immediately. This is required because QUIC
        // streams are only announced to the remote when the first STREAM frame is
        // written; accept_bi() on B blocks until A's send task writes that frame.
        let (a_outbound_tx, a_outbound_rx) = mpsc::channel(8);
        a_outbound_tx
            .send(ControlMessage::ControlRequest(ControlRequest {
                peer_id: 1,
            }))
            .await
            .unwrap();

        // Open A's stream and spawn A's channel; concurrently accept on B.
        let (a_send, a_recv) = peer_a.open_control_stream().await.unwrap();
        let a_handle = ControlChannel::spawn(a_send, a_recv, a_outbound_rx);

        // Now B can accept — A's send task will write the queued message which
        // triggers the STREAM frame that makes accept_bi() return.
        let (b_send, b_recv) = tokio::time::timeout(Duration::from_secs(2), async {
            peer_b
                .accept_bi()
                .await
                .unwrap()
                .expect("connection closed before stream opened")
        })
        .await
        .expect("accept_bi timed out");

        let (_b_outbound_tx, b_outbound_rx) = mpsc::channel(8);
        let b_handle = ControlChannel::spawn(b_send, b_recv, b_outbound_rx);

        let mut b_inbound = b_handle.subscribe();
        let msg = tokio::time::timeout(Duration::from_secs(2), b_inbound.recv())
            .await
            .unwrap()
            .unwrap();

        assert!(matches!(
            msg,
            ControlMessage::ControlRequest(ControlRequest { peer_id: 1 })
        ));

        a_handle.stop().await;
        b_handle.stop().await;
    }

    #[tokio::test]
    async fn control_channel_stops_cleanly() {
        let (peer_a, peer_b) = crate::test_utils::loopback_pair().await;

        // Pre-queue a message from A so A's send task immediately writes a STREAM
        // frame, allowing B's accept_bi to return.
        let (a_tx, a_rx) = mpsc::channel(8);
        a_tx.send(ControlMessage::ControlRequest(ControlRequest {
            peer_id: 0,
        }))
        .await
        .unwrap();

        let (a_send, a_recv) = peer_a.open_control_stream().await.unwrap();
        let a = ControlChannel::spawn(a_send, a_recv, a_rx);

        let (b_send, b_recv) = tokio::time::timeout(Duration::from_secs(2), async {
            peer_b
                .accept_bi()
                .await
                .unwrap()
                .expect("connection closed before stream opened")
        })
        .await
        .expect("accept_bi timed out");

        let (_b_tx, b_rx) = mpsc::channel(8);
        let b = ControlChannel::spawn(b_send, b_recv, b_rx);

        tokio::time::timeout(Duration::from_secs(2), async {
            a.stop().await;
            b.stop().await;
        })
        .await
        .unwrap();
    }
}
