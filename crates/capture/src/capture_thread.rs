//! Screen capture thread using scap.
//!
//! Runs on a dedicated OS thread. Sends `RawFrame` via `std::sync::mpsc`
//! (OS thread → OS thread, no tokio runtime needed on the capture thread).
//!
//! The encode thread consumes frames directly from this sync channel.
//! To bridge to an async context, use `into_async_receiver()`.

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
};

use pairwire_core::{Error, Result, config::CaptureConfig};
#[cfg(feature = "capture")]
use tracing::warn;
use tracing::{debug, info};

use crate::frame::RawFrame;

const CAPTURE_CHANNEL_CAPACITY: usize = 4;

/// Handle to a running capture thread.
pub struct CaptureThread {
    stop_flag: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
}

impl CaptureThread {
    /// Start capturing from the primary display using scap.
    ///
    /// Requires the `capture` feature and display hardware with OS permissions.
    #[cfg(feature = "capture")]
    pub fn start(config: CaptureConfig) -> Result<(Self, mpsc::Receiver<RawFrame>)> {
        use scap::{
            capturer::{Capturer, Options, Resolution},
            frame::{Frame, FrameType},
        };

        if !scap::is_supported() {
            return Err(Error::Capture(
                "screen capture not supported on this platform".into(),
            ));
        }
        if !scap::has_permission() {
            // Trigger the macOS TCC permission dialog. Returns true if granted.
            // On first run this shows a system prompt; on subsequent runs it
            // returns the cached decision without prompting.
            info!("requesting screen capture permission from macOS");
            if !scap::request_permission() {
                return Err(Error::Capture(
                    "screen capture permission not granted — enable in System Settings → Privacy & Security → Screen Recording".into(),
                ));
            }
        }

        let (tx, rx) = mpsc::sync_channel(CAPTURE_CHANNEL_CAPACITY);
        let stop_flag = Arc::new(AtomicBool::new(false));
        let stop_flag_thread = Arc::clone(&stop_flag);

        let handle = thread::Builder::new()
            .name("pairwire-capture".into())
            .spawn(move || {
                let options = Options {
                    fps: config.fps,
                    show_cursor: true,
                    show_highlight: false,
                    target: None,
                    output_type: FrameType::BGRAFrame,
                    output_resolution: Resolution::Captured,
                    ..Default::default()
                };

                let mut capturer = match Capturer::build(options) {
                    Ok(c) => c,
                    Err(e) => {
                        warn!(error = %e, "failed to build capturer");
                        return;
                    }
                };

                capturer.start_capture();
                info!("capture thread started");

                while !stop_flag_thread.load(Ordering::Relaxed) {
                    match capturer.get_next_frame() {
                        Ok(Frame::Video(vf)) => {
                            if let Some(raw) = video_frame_to_raw(vf) {
                                // try_send: drop frame if encode thread is behind
                                if tx.try_send(raw).is_err() {
                                    debug!("capture channel full, dropping frame");
                                }
                            }
                        }
                        Ok(Frame::Audio(_)) => {}
                        Err(e) => {
                            warn!(error = %e, "capture error");
                            break;
                        }
                    }
                }

                capturer.stop_capture();
                info!("capture thread stopped");
            })
            .map_err(|e| Error::Capture(format!("failed to spawn capture thread: {e}")))?;

        Ok((
            Self {
                stop_flag,
                handle: Some(handle),
            },
            rx,
        ))
    }

    /// Start a fake capture thread driven by `FakeFrameSource`.
    ///
    /// Generates synthetic BGRA frames at the configured FPS.
    /// Available without the `capture` feature — use in tests and CI.
    ///
    /// # Errors
    ///
    /// Returns an error if the OS thread cannot be spawned.
    #[allow(clippy::needless_pass_by_value)] // config is moved into the thread closure
    pub fn start_fake(config: CaptureConfig) -> Result<(Self, mpsc::Receiver<RawFrame>)> {
        use pairwire_core::test_utils::FakeFrameSource;

        let (tx, rx) = mpsc::sync_channel(CAPTURE_CHANNEL_CAPACITY);
        let stop_flag = Arc::new(AtomicBool::new(false));
        let stop_flag_thread = Arc::clone(&stop_flag);
        let frame_interval = std::time::Duration::from_micros(1_000_000 / u64::from(config.fps));

        let handle = thread::Builder::new()
            .name("pairwire-fake-capture".into())
            .spawn(move || {
                let mut source = FakeFrameSource::new(config.width, config.height);
                info!(
                    width = config.width,
                    height = config.height,
                    fps = config.fps,
                    "fake capture thread started"
                );

                while !stop_flag_thread.load(Ordering::Relaxed) {
                    let (data, timestamp_us) = source.next_frame();
                    let frame = RawFrame {
                        data,
                        width: config.width,
                        height: config.height,
                        timestamp_us,
                    };
                    if tx.try_send(frame).is_err() {
                        debug!("fake capture channel full, dropping frame");
                    }
                    thread::sleep(frame_interval);
                }

                info!("fake capture thread stopped");
            })
            .map_err(|e| Error::Capture(format!("failed to spawn fake capture thread: {e}")))?;

        Ok((
            Self {
                stop_flag,
                handle: Some(handle),
            },
            rx,
        ))
    }

    pub fn stop(mut self) {
        self.stop_flag.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for CaptureThread {
    fn drop(&mut self) {
        self.stop_flag.store(true, Ordering::Relaxed);
    }
}

/// Bridge a sync `mpsc::Receiver<RawFrame>` to a `tokio::sync::mpsc` channel.
///
/// Spawns a blocking tokio task that forwards frames. Use this when you need
/// to receive frames in an async context without an encode thread in between.
#[must_use]
pub fn into_async_receiver(
    sync_rx: mpsc::Receiver<RawFrame>,
    capacity: usize,
) -> tokio::sync::mpsc::Receiver<RawFrame> {
    let (tx, rx) = tokio::sync::mpsc::channel(capacity);
    tokio::task::spawn_blocking(move || {
        while let Ok(frame) = sync_rx.recv() {
            if tx.blocking_send(frame).is_err() {
                break;
            }
        }
    });
    rx
}

#[cfg(feature = "capture")]
fn video_frame_to_raw(frame: scap::frame::VideoFrame) -> Option<RawFrame> {
    use bytes::Bytes;
    use scap::frame::VideoFrame;
    use std::time::{SystemTime, UNIX_EPOCH};

    let now_us = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros() as u64;

    match frame {
        VideoFrame::BGRA(f) => {
            if f.width == 0 || f.height == 0 || f.data.is_empty() {
                debug!("empty BGRA frame (idle status), dropping");
                return None;
            }
            Some(RawFrame {
                data: Bytes::from(f.data),
                width: f.width as u32,
                height: f.height as u32,
                timestamp_us: now_us,
            })
        }
        VideoFrame::BGR0(f) => {
            if f.width == 0 || f.height == 0 || f.data.is_empty() {
                debug!("empty BGR0 frame (idle status), dropping");
                return None;
            }
            let mut bgra = Vec::with_capacity(f.data.len());
            for chunk in f.data.chunks_exact(4) {
                bgra.extend_from_slice(&[chunk[0], chunk[1], chunk[2], 255]);
            }
            Some(RawFrame {
                data: Bytes::from(bgra),
                width: f.width as u32,
                height: f.height as u32,
                timestamp_us: now_us,
            })
        }
        other => {
            debug!(frame_type = ?std::mem::discriminant(&other), "unsupported frame type, dropping");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pairwire_core::config::CaptureConfig;
    use std::time::Duration;

    #[test]
    fn fake_capture_produces_frames() {
        let config = CaptureConfig {
            fps: 30,
            width: 320,
            height: 240,
            ..Default::default()
        };
        let (thread, rx) = CaptureThread::start_fake(config.clone()).unwrap();

        let mut frames = Vec::new();
        let deadline = std::time::Instant::now() + Duration::from_millis(200);
        while frames.len() < 3 && std::time::Instant::now() < deadline {
            if let Ok(f) = rx.recv_timeout(Duration::from_millis(50)) {
                frames.push(f);
            }
        }
        thread.stop();

        assert!(
            frames.len() >= 3,
            "expected ≥3 frames, got {}",
            frames.len()
        );
        for f in &frames {
            assert_eq!(f.width, config.width);
            assert_eq!(f.height, config.height);
            assert_eq!(
                f.data.len(),
                RawFrame::expected_len(config.width, config.height)
            );
        }
    }

    #[test]
    fn fake_capture_frames_have_increasing_timestamps() {
        let config = CaptureConfig {
            fps: 60,
            width: 64,
            height: 64,
            ..Default::default()
        };
        let (thread, rx) = CaptureThread::start_fake(config).unwrap();

        let mut timestamps = Vec::new();
        let deadline = std::time::Instant::now() + Duration::from_millis(150);
        while timestamps.len() < 5 && std::time::Instant::now() < deadline {
            if let Ok(f) = rx.recv_timeout(Duration::from_millis(20)) {
                timestamps.push(f.timestamp_us);
            }
        }
        thread.stop();

        assert!(timestamps.len() >= 2);
        for w in timestamps.windows(2) {
            assert!(w[1] >= w[0], "timestamps not monotonic");
        }
    }
}
