//! Single-reader datagram dispatcher with tag-byte routing.

use bytes::Bytes;
use iroh::endpoint::Connection;
use tokio::sync::mpsc;
use tracing::{debug, info};

/// Tag byte for cursor datagrams.
pub const TAG_CURSOR: u8 = 0x43;
/// Tag byte for guest-to-host audio datagrams.
pub const TAG_GUEST_AUDIO: u8 = 0x47;
/// Tag byte for drawing point datagrams (real-time preview).
pub const TAG_DRAW_POINT: u8 = 0x44;
/// Tag byte for reaction datagrams.
pub const TAG_REACTION: u8 = 0x52;

/// Channels that receive routed datagram payloads.
pub struct DatagramChannels {
    pub cursor_tx: mpsc::Sender<Bytes>,
    pub guest_audio_tx: Option<mpsc::Sender<Bytes>>,
    pub draw_point_tx: Option<mpsc::Sender<Bytes>>,
    pub reaction_tx: Option<mpsc::Sender<Bytes>>,
}

/// Handle to the running datagram router task.
pub struct DatagramRouterHandle {
    join: tokio::task::JoinHandle<()>,
}

impl DatagramRouterHandle {
    pub async fn stop(self) {
        self.join.abort();
        let _ = self.join.await;
    }
}

/// Start the single datagram reader for a connection.
#[must_use]
pub fn start(conn: Connection, channels: DatagramChannels) -> DatagramRouterHandle {
    let join = tokio::spawn(async move {
        info!("datagram router started");
        loop {
            let data = match conn.read_datagram().await {
                Ok(data) => data,
                Err(error) => {
                    info!(%error, "datagram router stopped");
                    break;
                }
            };

            if data.is_empty() {
                debug!("empty datagram dropped");
                continue;
            }

            let tag = data[0];
            let payload = data.slice(1..);

            match tag {
                TAG_CURSOR => {
                    let _ = channels.cursor_tx.try_send(payload);
                }
                TAG_GUEST_AUDIO => {
                    if let Some(tx) = channels.guest_audio_tx.as_ref() {
                        let _ = tx.try_send(payload);
                    }
                }
                TAG_DRAW_POINT => {
                    if let Some(tx) = channels.draw_point_tx.as_ref() {
                        let _ = tx.try_send(payload);
                    }
                }
                TAG_REACTION => {
                    if let Some(tx) = channels.reaction_tx.as_ref() {
                        let _ = tx.try_send(payload);
                    }
                }
                _ => {
                    debug!(tag, len = data.len(), "unknown datagram tag dropped");
                }
            }
        }
    });

    DatagramRouterHandle { join }
}

/// Prepend a tag byte and send the payload as a QUIC datagram.
pub fn send_tagged(conn: &Connection, tag: u8, payload: &[u8]) -> Result<(), String> {
    let buf = build_tagged(tag, payload);
    conn.send_datagram(buf).map_err(|error| error.to_string())
}

fn build_tagged(tag: u8, payload: &[u8]) -> Bytes {
    let mut buf = Vec::with_capacity(payload.len() + 1);
    buf.push(tag);
    buf.extend_from_slice(payload);
    Bytes::from(buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_tagged_prefixes_tag() {
        let tagged = build_tagged(TAG_CURSOR, &[1, 2, 3]);
        assert_eq!(tagged.as_ref(), &[TAG_CURSOR, 1, 2, 3]);
    }

    #[test]
    fn build_tagged_allows_empty_payload() {
        let tagged = build_tagged(TAG_GUEST_AUDIO, &[]);
        assert_eq!(tagged.as_ref(), &[TAG_GUEST_AUDIO]);
    }
}
