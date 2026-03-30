//! Guest-side input sender: reads `InputMessage` from a channel, writes to QUIC stream.

use iroh::endpoint::SendStream;
use pairwire_core::{Error, Result};
use pairwire_protocol::input::InputMessage;
use pairwire_transport::connection::send_message;
use tokio::{
    sync::{mpsc, oneshot},
    task::JoinHandle,
};
use tracing::{debug, info, instrument, warn};

/// Spawned task handle for the input sender.
pub struct InputSenderHandle {
    stop_tx: oneshot::Sender<()>,
    join: JoinHandle<()>,
}

impl InputSenderHandle {
    /// Signal the sender to stop and wait for it to finish.
    pub async fn stop(self) {
        let _ = self.stop_tx.send(());
        let _ = self.join.await;
    }
}

/// Sends `InputMessage` events over a QUIC unidirectional stream.
///
/// Spawns a tokio task that drains the input channel and writes each message
/// as a length-prefixed postcard frame.
pub struct InputSender {
    stream: SendStream,
}

impl InputSender {
    #[must_use]
    pub fn new(stream: SendStream) -> Self {
        Self { stream }
    }

    /// Spawn the sender task.
    ///
    /// `input_rx` receives `InputMessage` events from the guest UI/input capture.
    /// The task exits when the channel is closed or the stop signal fires.
    #[instrument(skip(self, input_rx))]
    pub fn spawn(self, mut input_rx: mpsc::Receiver<InputMessage>) -> InputSenderHandle {
        let (stop_tx, mut stop_rx) = oneshot::channel();
        let mut stream = self.stream;

        let join = tokio::spawn(async move {
            info!("input sender started");
            loop {
                tokio::select! {
                    biased;
                    _ = &mut stop_rx => break,
                    msg = input_rx.recv() => {
                        match msg {
                            None => break,
                            Some(event) => {
                                if let Err(e) = send_input(&mut stream, &event).await {
                                    warn!(error = %e, "input send error");
                                    break;
                                }
                                debug!(event = ?event, "input event sent");
                            }
                        }
                    }
                }
            }
            // Finish the stream cleanly so the receiver sees EOF.
            let _ = stream.finish();
            info!("input sender stopped");
        });

        InputSenderHandle { stop_tx, join }
    }
}

async fn send_input(stream: &mut SendStream, msg: &InputMessage) -> Result<()> {
    send_message(stream, msg)
        .await
        .map_err(|e| Error::Transport(format!("input send: {e}")))
}

#[cfg(test)]
mod tests {
    use pairwire_protocol::input::{InputMessage, KeyEvent};
    use postcard::to_allocvec;

    #[test]
    fn input_message_serializes() {
        let msg = InputMessage::Key(KeyEvent {
            key_code: 65,
            modifiers: 0,
            pressed: true,
            timestamp_ms: 1000,
        });
        let bytes = to_allocvec(&msg).unwrap();
        assert!(!bytes.is_empty());
        let decoded: InputMessage = postcard::from_bytes(&bytes).unwrap();
        assert_eq!(msg, decoded);
    }
}
