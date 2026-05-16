//! Session handshake and capability negotiation over the control stream.
//!
//! Both peers exchange a `Handshake` message immediately after connection.
//! The host opens the control stream and sends first; the guest responds.
//!
//! After exchange:
//! - Protocol version is checked — mismatch causes immediate disconnect.
//! - Capabilities are intersected to find what both peers support.
//! - Video/audio codecs are negotiated (first mutual preference wins).
//!
//! The resulting `NegotiatedSession` is passed to the rest of the session
//! to configure encode/decode and feature availability.

use iroh::endpoint::{RecvStream, SendStream};
use pairwire_core::{Error, Result};
use pairwire_protocol::{
    AudioCodec, Capability, Handshake, PROTOCOL_VERSION, VideoCodec, control::ControlMessage,
};
use tracing::{debug, instrument, warn};

use crate::connection::{recv_message, send_message};

/// The outcome of a successful handshake — what both peers agreed on.
#[derive(Debug, Clone)]
pub struct NegotiatedSession {
    /// The remote peer's full handshake (for diagnostics / display info).
    pub remote: Handshake,
    /// Capabilities both peers support.
    pub capabilities: Vec<Capability>,
    /// Agreed video codec (None if neither peer supports video).
    pub video_codec: Option<VideoCodec>,
    /// Agreed audio codec (None if neither peer supports audio).
    pub audio_codec: Option<AudioCodec>,
}

impl NegotiatedSession {
    #[must_use]
    pub fn has_capability(&self, cap: &Capability) -> bool {
        self.capabilities.contains(cap)
    }
}

/// Build the local `Handshake` for this peer.
///
/// `display_width`/`display_height`/`display_scale` are 0 on the guest side
/// (guest doesn't share a display).
/// `session_token` is the 16-byte session ID from the invite link.
#[must_use]
pub fn local_handshake(
    capabilities: Vec<Capability>,
    display_width: u16,
    display_height: u16,
    display_scale: u16,
    session_token: [u8; 16],
) -> Handshake {
    Handshake {
        version: PROTOCOL_VERSION,
        capabilities,
        display_width,
        display_height,
        display_scale,
        video_codecs: vec![VideoCodec::H264],
        audio_codecs: vec![AudioCodec::Opus],
        session_token,
    }
}

/// Host side: open control stream, send handshake first, receive guest's handshake.
///
/// Returns the open streams (for subsequent control messages) and the negotiated session.
///
/// # Errors
///
/// Returns an error if the stream I/O fails, the remote sends an unexpected message,
/// or the protocol versions are incompatible.
#[instrument(skip(send, recv, local))]
pub async fn host_handshake(
    mut send: SendStream,
    mut recv: RecvStream,
    local: Handshake,
) -> Result<(SendStream, RecvStream, NegotiatedSession)> {
    debug!("sending host handshake");
    send_message(&mut send, &ControlMessage::Handshake(local.clone())).await?;
    // The handshake stream is one-shot. Finish the send side and wait for the
    // peer's ACK before continuing. Without `stopped().await` Quinn may not
    // flush the buffered handshake bytes before subsequent bidi streams are
    // opened, leaving the peer's `recv_message` blocked on the length prefix.
    // Bounded by a timeout so a stalled peer cannot wedge the host accept loop.
    send.finish()
        .map_err(|e| Error::Transport(format!("finish host handshake stream: {e}")))?;
    match tokio::time::timeout(std::time::Duration::from_secs(3), send.stopped()).await {
        Ok(Ok(_)) => {}
        Ok(Err(e)) => {
            return Err(Error::Transport(format!(
                "host handshake stream ack failed: {e}"
            )));
        }
        Err(_) => {
            return Err(Error::Transport("host handshake send ack timed out".into()));
        }
    }

    debug!("waiting for guest handshake");
    let msg: ControlMessage = recv_message(&mut recv).await?;
    let remote = match msg {
        ControlMessage::Handshake(hs) => hs,
        other => {
            return Err(Error::Protocol(format!(
                "expected Handshake, got {other:?}"
            )));
        }
    };

    let session = negotiate(local, remote)?;
    Ok((send, recv, session))
}

/// Guest side: accept control stream, receive host's handshake first, then respond.
///
/// Returns the open streams and the negotiated session.
///
/// # Errors
///
/// Returns an error if the stream I/O fails, the remote sends an unexpected message,
/// or the protocol versions are incompatible.
#[instrument(skip(send, recv, local))]
pub async fn guest_handshake(
    mut send: SendStream,
    mut recv: RecvStream,
    local: Handshake,
) -> Result<(SendStream, RecvStream, NegotiatedSession)> {
    debug!("waiting for host handshake");
    let msg: ControlMessage = recv_message(&mut recv).await?;
    let remote = match msg {
        ControlMessage::Handshake(hs) => hs,
        other => {
            return Err(Error::Protocol(format!(
                "expected Handshake, got {other:?}"
            )));
        }
    };

    debug!("sending guest handshake");
    send_message(&mut send, &ControlMessage::Handshake(local.clone())).await?;
    // Finish and wait for the peer's ACK before dropping this one-shot stream.
    // Without the wait, the response bytes can be lost if the SendStream is
    // dropped while Quinn still has them buffered, and the host gets stuck
    // reading the length prefix. Bounded so a stalled host cannot wedge us.
    send.finish()
        .map_err(|e| Error::Transport(format!("finish guest handshake stream: {e}")))?;
    match tokio::time::timeout(std::time::Duration::from_secs(3), send.stopped()).await {
        Ok(Ok(_)) => {}
        Ok(Err(e)) => {
            return Err(Error::Transport(format!(
                "guest handshake stream ack failed: {e}"
            )));
        }
        Err(_) => {
            return Err(Error::Transport(
                "guest handshake send ack timed out".into(),
            ));
        }
    }

    let session = negotiate(local, remote)?;
    Ok((send, recv, session))
}

/// Intersect two handshakes into a `NegotiatedSession`.
///
/// Returns `Err(VersionMismatch)` if protocol versions are incompatible.
/// Returns `Err(PeerAuthFailed)` if session tokens don't match.
#[allow(clippy::needless_pass_by_value)] // callers pass owned Handshake values
fn negotiate(local: Handshake, remote: Handshake) -> Result<NegotiatedSession> {
    // Verify session token — both peers must present the same token from the invite.
    if local.session_token != remote.session_token {
        warn!("session token mismatch — rejecting peer");
        return Err(Error::PeerAuthFailed);
    }

    if remote.version != PROTOCOL_VERSION {
        warn!(
            local_version = PROTOCOL_VERSION,
            remote_version = remote.version,
            "protocol version mismatch"
        );
        return Err(Error::VersionMismatch {
            local: PROTOCOL_VERSION,
            remote: remote.version,
        });
    }

    // Reject oversized vectors from malicious peers.
    if remote.capabilities.len() > 16
        || remote.video_codecs.len() > 8
        || remote.audio_codecs.len() > 4
    {
        return Err(Error::Protocol(format!(
            "handshake vectors too large: caps={}, video={}, audio={}",
            remote.capabilities.len(),
            remote.video_codecs.len(),
            remote.audio_codecs.len()
        )));
    }

    // Validate display dimensions (0 causes division-by-zero downstream).
    if remote.display_width > 0 && remote.display_height == 0
        || remote.display_width == 0 && remote.display_height > 0
    {
        return Err(Error::Protocol(
            "invalid display dimensions: one dimension is 0".into(),
        ));
    }

    // Capability intersection: keep only what both sides advertise.
    let capabilities: Vec<Capability> = local
        .capabilities
        .iter()
        .filter(|c| remote.capabilities.contains(c))
        .cloned()
        .collect();

    // Codec negotiation: first codec in local preference list that remote also supports.
    let video_codec = local
        .video_codecs
        .iter()
        .find(|c| remote.video_codecs.contains(c))
        .cloned();

    let audio_codec = local
        .audio_codecs
        .iter()
        .find(|c| remote.audio_codecs.contains(c))
        .cloned();

    debug!(
        ?capabilities,
        ?video_codec,
        ?audio_codec,
        "handshake negotiated"
    );

    Ok(NegotiatedSession {
        remote,
        capabilities,
        video_codec,
        audio_codec,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use pairwire_protocol::Capability;

    const TEST_TOKEN: [u8; 16] = [0xAA; 16];

    fn make_hs(caps: Vec<Capability>, video: Vec<VideoCodec>) -> Handshake {
        Handshake {
            version: PROTOCOL_VERSION,
            capabilities: caps,
            display_width: 1920,
            display_height: 1080,
            display_scale: 100,
            video_codecs: video,
            audio_codecs: vec![AudioCodec::Opus],
            session_token: TEST_TOKEN,
        }
    }

    #[tokio::test]
    async fn host_guest_handshake_completes_over_loopback() {
        let (guest_conn, host_conn) = crate::test_utils::loopback_pair().await;
        let host_local = make_hs(
            vec![
                Capability::Screen,
                Capability::Audio,
                Capability::RemoteControl,
            ],
            vec![VideoCodec::H264],
        );
        let mut guest_local = make_hs(
            vec![
                Capability::Screen,
                Capability::Audio,
                Capability::Annotations,
            ],
            vec![VideoCodec::H264],
        );
        guest_local.display_width = 0;
        guest_local.display_height = 0;
        guest_local.display_scale = 0;

        let host = async {
            let (send, recv) = host_conn.open_control_stream().await.unwrap();
            host_handshake(send, recv, host_local).await.unwrap().2
        };
        let guest = async {
            let (send, recv) = guest_conn.accept_bi().await.unwrap().unwrap();
            guest_handshake(send, recv, guest_local).await.unwrap().2
        };

        let (host_session, guest_session) =
            tokio::time::timeout(std::time::Duration::from_secs(2), async {
                tokio::join!(host, guest)
            })
            .await
            .expect("handshake timed out");

        assert_eq!(host_session.video_codec, Some(VideoCodec::H264));
        assert_eq!(guest_session.audio_codec, Some(AudioCodec::Opus));
        assert!(host_session.has_capability(&Capability::Screen));
        assert!(guest_session.has_capability(&Capability::Audio));
    }

    #[test]
    fn negotiate_matching_caps() {
        let local = make_hs(
            vec![
                Capability::Screen,
                Capability::Audio,
                Capability::RemoteControl,
            ],
            vec![VideoCodec::H264],
        );
        let remote = make_hs(
            vec![Capability::Screen, Capability::Audio],
            vec![VideoCodec::H264],
        );
        let session = negotiate(local, remote).unwrap();
        assert_eq!(
            session.capabilities,
            vec![Capability::Screen, Capability::Audio]
        );
        assert_eq!(session.video_codec, Some(VideoCodec::H264));
        assert_eq!(session.audio_codec, Some(AudioCodec::Opus));
    }

    #[test]
    fn negotiate_no_common_video_codec() {
        let local = make_hs(vec![Capability::Screen], vec![VideoCodec::H264]);
        let remote = make_hs(vec![Capability::Screen], vec![VideoCodec::Av1]);
        let session = negotiate(local, remote).unwrap();
        assert_eq!(session.video_codec, None);
    }

    #[test]
    fn negotiate_version_mismatch() {
        let mut remote = make_hs(vec![Capability::Screen], vec![VideoCodec::H264]);
        remote.version = 99;
        let local = make_hs(vec![Capability::Screen], vec![VideoCodec::H264]);
        let err = negotiate(local, remote).unwrap_err();
        assert!(matches!(err, Error::VersionMismatch { .. }));
    }

    #[test]
    fn negotiate_codec_preference_order() {
        // Local prefers H265, remote only supports H264 — should pick H264.
        let local = make_hs(
            vec![Capability::Screen],
            vec![VideoCodec::H265, VideoCodec::H264],
        );
        let remote = make_hs(vec![Capability::Screen], vec![VideoCodec::H264]);
        let session = negotiate(local, remote).unwrap();
        assert_eq!(session.video_codec, Some(VideoCodec::H264));
    }

    #[test]
    fn local_handshake_sets_version() {
        let hs = local_handshake(vec![Capability::Screen], 1920, 1080, 100, TEST_TOKEN);
        assert_eq!(hs.version, PROTOCOL_VERSION);
        assert_eq!(hs.session_token, TEST_TOKEN);
    }

    #[test]
    fn negotiate_session_token_mismatch() {
        let local = make_hs(vec![Capability::Screen], vec![VideoCodec::H264]);
        let mut remote = make_hs(vec![Capability::Screen], vec![VideoCodec::H264]);
        remote.session_token = [0xBB; 16]; // different token
        let err = negotiate(local, remote).unwrap_err();
        assert!(matches!(err, Error::PeerAuthFailed));
    }

    #[test]
    fn negotiate_matching_session_token_succeeds() {
        let local = make_hs(vec![Capability::Screen], vec![VideoCodec::H264]);
        let remote = make_hs(vec![Capability::Screen], vec![VideoCodec::H264]);
        assert!(negotiate(local, remote).is_ok());
    }

    #[test]
    fn negotiate_no_common_caps() {
        let local = make_hs(vec![Capability::Screen], vec![VideoCodec::H264]);
        let remote = make_hs(vec![Capability::Audio], vec![VideoCodec::H264]);
        let session = negotiate(local, remote).unwrap();
        assert!(session.capabilities.is_empty());
    }

    #[test]
    fn negotiate_empty_caps_succeeds() {
        let local = make_hs(vec![], vec![VideoCodec::H264]);
        let remote = make_hs(vec![], vec![VideoCodec::H264]);
        let session = negotiate(local, remote).unwrap();
        assert!(session.capabilities.is_empty());
        assert_eq!(session.video_codec, Some(VideoCodec::H264));
    }

    #[test]
    fn negotiate_rejects_oversized_caps() {
        let mut remote = make_hs(vec![Capability::Screen], vec![VideoCodec::H264]);
        remote.capabilities = vec![Capability::Screen; 17]; // > 16
        let local = make_hs(vec![Capability::Screen], vec![VideoCodec::H264]);
        assert!(negotiate(local, remote).is_err());
    }

    #[test]
    fn negotiate_rejects_oversized_video_codecs() {
        let mut remote = make_hs(vec![], vec![VideoCodec::H264]);
        remote.video_codecs = vec![VideoCodec::H264; 9]; // > 8
        let local = make_hs(vec![], vec![VideoCodec::H264]);
        assert!(negotiate(local, remote).is_err());
    }

    #[test]
    fn negotiate_rejects_inconsistent_display_dims() {
        let mut remote = make_hs(vec![Capability::Screen], vec![VideoCodec::H264]);
        remote.display_width = 1920;
        remote.display_height = 0; // inconsistent: one is 0, other isn't
        let local = make_hs(vec![Capability::Screen], vec![VideoCodec::H264]);
        assert!(negotiate(local, remote).is_err());
    }

    #[test]
    fn negotiate_allows_zero_display_dims() {
        // Guest sends 0,0 — valid (guest doesn't share a display)
        let mut remote = make_hs(vec![Capability::Screen], vec![VideoCodec::H264]);
        remote.display_width = 0;
        remote.display_height = 0;
        let local = make_hs(vec![Capability::Screen], vec![VideoCodec::H264]);
        assert!(negotiate(local, remote).is_ok());
    }

    #[test]
    fn negotiate_preserves_remote_handshake() {
        let local = make_hs(vec![Capability::Screen], vec![VideoCodec::H264]);
        let remote = make_hs(
            vec![Capability::Screen, Capability::Audio],
            vec![VideoCodec::H264],
        );
        let session = negotiate(local, remote).unwrap();
        assert_eq!(session.remote.capabilities.len(), 2);
    }

    #[test]
    fn has_capability_returns_false_for_missing() {
        let local = make_hs(vec![Capability::Screen], vec![VideoCodec::H264]);
        let remote = make_hs(vec![Capability::Screen], vec![VideoCodec::H264]);
        let session = negotiate(local, remote).unwrap();
        assert!(session.has_capability(&Capability::Screen));
        assert!(!session.has_capability(&Capability::Audio));
    }
}
