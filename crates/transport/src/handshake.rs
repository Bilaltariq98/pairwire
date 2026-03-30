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
    pub fn has_capability(&self, cap: &Capability) -> bool {
        self.capabilities.contains(cap)
    }
}

/// Build the local `Handshake` for this peer.
///
/// `display_width`/`display_height`/`display_scale` are 0 on the guest side
/// (guest doesn't share a display).
pub fn local_handshake(
    capabilities: Vec<Capability>,
    display_width: u16,
    display_height: u16,
    display_scale: u16,
) -> Handshake {
    Handshake {
        version: PROTOCOL_VERSION,
        capabilities,
        display_width,
        display_height,
        display_scale,
        video_codecs: vec![VideoCodec::H264],
        audio_codecs: vec![AudioCodec::Opus],
    }
}

/// Host side: open control stream, send handshake first, receive guest's handshake.
///
/// Returns the open streams (for subsequent control messages) and the negotiated session.
#[instrument(skip(send, recv, local))]
pub async fn host_handshake(
    mut send: SendStream,
    mut recv: RecvStream,
    local: Handshake,
) -> Result<(SendStream, RecvStream, NegotiatedSession)> {
    debug!("sending host handshake");
    send_message(&mut send, &ControlMessage::Handshake(local.clone())).await?;

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

    let session = negotiate(local, remote)?;
    Ok((send, recv, session))
}

/// Intersect two handshakes into a `NegotiatedSession`.
///
/// Returns `Err(VersionMismatch)` if protocol versions are incompatible.
fn negotiate(local: Handshake, remote: Handshake) -> Result<NegotiatedSession> {
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

    fn make_hs(caps: Vec<Capability>, video: Vec<VideoCodec>) -> Handshake {
        Handshake {
            version: PROTOCOL_VERSION,
            capabilities: caps,
            display_width: 1920,
            display_height: 1080,
            display_scale: 100,
            video_codecs: video,
            audio_codecs: vec![AudioCodec::Opus],
        }
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
        let hs = local_handshake(vec![Capability::Screen], 1920, 1080, 100);
        assert_eq!(hs.version, PROTOCOL_VERSION);
    }
}
