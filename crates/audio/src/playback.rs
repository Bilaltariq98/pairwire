//! Audio playback thread using cpal.
//!
//! Reads decoded `AudioFrame`s from a sync channel and plays them through
//! the default output device. A simple ring-buffer jitter buffer smooths
//! delivery jitter.
//!
//! Without the `capture` feature, playback is a no-op stub that drains
//! the channel and discards frames (useful for headless/CI builds).

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::Duration,
};

use pairwire_core::{Error, Result, config::AudioConfig};
use tracing::{info, warn};

use crate::frame::AudioFrame;

/// Jitter buffer target depth in frames (each frame = 20ms → 40ms buffer).
#[cfg(feature = "capture")]
const JITTER_BUFFER_FRAMES: usize = 2;

/// Handle to a running audio playback thread.
pub struct AudioPlaybackThread {
    stop_flag: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
}

impl AudioPlaybackThread {
    /// Start the playback thread.
    ///
    /// `frame_rx`: decoded PCM frames from the decode thread.
    ///
    /// # Errors
    ///
    /// Returns an error if the OS thread cannot be spawned.
    pub fn start(config: AudioConfig, frame_rx: mpsc::Receiver<AudioFrame>) -> Result<Self> {
        let stop_flag = Arc::new(AtomicBool::new(false));
        let stop_flag_thread = Arc::clone(&stop_flag);

        let handle = thread::Builder::new()
            .name("pairwire-audio-playback".into())
            .spawn(move || {
                if let Err(e) = playback_loop(config, frame_rx, stop_flag_thread) {
                    warn!(error = %e, "audio playback thread error");
                }
                info!("audio playback thread stopped");
            })
            .map_err(|e| Error::Audio(format!("spawn playback thread: {e}")))?;

        Ok(Self {
            stop_flag,
            handle: Some(handle),
        })
    }

    pub fn stop(mut self) {
        self.stop_flag.store(true, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

#[cfg(feature = "capture")]
#[allow(clippy::needless_pass_by_value)] // args are consumed by the dedicated OS thread
fn playback_loop(
    config: AudioConfig,
    frame_rx: mpsc::Receiver<AudioFrame>,
    stop_flag: Arc<AtomicBool>,
) -> Result<()> {
    use cpal::{
        StreamConfig,
        traits::{DeviceTrait, HostTrait, StreamTrait},
    };
    use std::sync::Mutex;

    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or_else(|| Error::Audio("no output device found".into()))?;

    info!(device = %device.description().map(|d| d.name).unwrap_or_default(), "audio playback device selected");

    #[allow(clippy::cast_possible_truncation)] // channels is always 1 or 2
    let stream_config = StreamConfig {
        channels: config.channels as u16,
        sample_rate: config.sample_rate,
        buffer_size: cpal::BufferSize::Default,
    };

    // Jitter buffer: shared ring buffer between the fill thread and cpal callback.
    let jitter: Arc<Mutex<Vec<f32>>> = Arc::new(Mutex::new(Vec::new()));
    let jitter_write = Arc::clone(&jitter);
    let jitter_read = Arc::clone(&jitter);

    let stream = device
        .build_output_stream(
            &stream_config,
            move |output: &mut [f32], _| {
                let mut buf = jitter_read.lock().unwrap();
                let available = buf.len().min(output.len());
                output[..available].copy_from_slice(&buf[..available]);
                buf.drain(..available);
                // Fill remainder with silence if buffer underruns.
                for s in &mut output[available..] {
                    *s = 0.0;
                }
            },
            |e| warn!(error = %e, "audio output stream error"),
            None,
        )
        .map_err(|e| Error::Audio(format!("build output stream: {e}")))?;

    stream
        .play()
        .map_err(|e| Error::Audio(format!("stream play: {e}")))?;

    let frame_samples = crate::OPUS_FRAME_SAMPLES * config.channels as usize;
    let target_buf_samples = frame_samples * JITTER_BUFFER_FRAMES;

    info!("audio playback thread started");

    loop {
        if stop_flag.load(Ordering::Relaxed) {
            break;
        }

        match frame_rx.recv_timeout(Duration::from_millis(100)) {
            Ok(frame) => {
                let mut buf = jitter_write.lock().unwrap();
                // Drop oldest samples if buffer grows too large (> 4 frames).
                if buf.len() > target_buf_samples * 2 {
                    let excess = buf.len() - target_buf_samples;
                    buf.drain(..excess);
                }
                buf.extend_from_slice(&frame.samples);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {} // keep looping
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }

    drop(stream);
    Ok(())
}

#[cfg(not(feature = "capture"))]
#[allow(clippy::needless_pass_by_value, clippy::unnecessary_wraps)]
// Args are consumed by the dedicated OS thread; Result<()> matches the real impl signature.
fn playback_loop(
    _config: AudioConfig,
    frame_rx: mpsc::Receiver<AudioFrame>,
    stop_flag: Arc<AtomicBool>,
) -> Result<()> {
    info!("audio playback stub started (capture feature not enabled)");
    loop {
        if stop_flag.load(Ordering::Relaxed) {
            break;
        }
        match frame_rx.recv_timeout(Duration::from_millis(100)) {
            Ok(_) | Err(mpsc::RecvTimeoutError::Timeout) => {} // discard or keep looping
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    Ok(())
}
