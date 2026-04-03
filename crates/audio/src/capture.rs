//! Audio capture thread using cpal (real) or a fake sine-wave source (test).
//!
//! Runs on a dedicated OS thread. Sends `AudioFrame` via `std::sync::mpsc`.
//! The encode thread consumes frames directly from this sync channel.

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
use tracing::info;
#[cfg(feature = "capture")]
use tracing::warn;

use crate::{OPUS_FRAME_SAMPLES, frame::AudioFrame};

const CAPTURE_CHANNEL_CAPACITY: usize = 8;

/// Handle to a running audio capture thread.
pub struct AudioCaptureThread {
    stop_flag: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
}

impl AudioCaptureThread {
    /// Start capturing from the default microphone using cpal.
    ///
    /// Requires the `capture` feature.
    #[cfg(feature = "capture")]
    pub fn start(config: AudioConfig) -> Result<(Self, mpsc::Receiver<AudioFrame>)> {
        use cpal::{
            SampleFormat, StreamConfig,
            traits::{DeviceTrait, HostTrait, StreamTrait},
        };

        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .ok_or_else(|| Error::Audio("no input device found".into()))?;

        info!(device = %device.description().map(|d| d.name().to_owned()).unwrap_or_default(), "audio capture device selected");

        let supported = device
            .default_input_config()
            .map_err(|e| Error::Audio(format!("input config: {e}")))?;

        let stream_config = StreamConfig {
            channels: config.channels as u16,
            sample_rate: config.sample_rate,
            buffer_size: cpal::BufferSize::Default,
        };

        let (tx, rx) = mpsc::sync_channel(CAPTURE_CHANNEL_CAPACITY);
        let stop_flag = Arc::new(AtomicBool::new(false));
        let stop_flag_stream = Arc::clone(&stop_flag);
        let stop_flag_thread = Arc::clone(&stop_flag);
        let sample_rate = config.sample_rate;
        let channels = config.channels;

        let stream = match supported.sample_format() {
            SampleFormat::F32 => {
                let tx = tx.clone();
                device.build_input_stream(
                    &stream_config,
                    move |data: &[f32], _| {
                        if stop_flag_stream.load(Ordering::Relaxed) {
                            return;
                        }
                        let frame = AudioFrame {
                            samples: data.to_vec(),
                            sample_rate,
                            channels,
                            timestamp_us: 0,
                        };
                        let _ = tx.try_send(frame);
                    },
                    |e| warn!(error = %e, "audio input stream error"),
                    None,
                )
            }
            fmt => {
                return Err(Error::Audio(format!(
                    "unsupported sample format {fmt:?} — need F32"
                )));
            }
        }
        .map_err(|e| Error::Audio(format!("build input stream: {e}")))?;

        stream
            .play()
            .map_err(|e| Error::Audio(format!("stream play: {e}")))?;

        let handle = thread::Builder::new()
            .name("pairwire-audio-capture".into())
            .spawn(move || {
                info!("audio capture thread started");
                // Keep stream alive until stop is signalled.
                while !stop_flag_thread.load(Ordering::Relaxed) {
                    thread::sleep(Duration::from_millis(10));
                }
                drop(stream);
                info!("audio capture thread stopped");
            })
            .map_err(|e| Error::Audio(format!("spawn: {e}")))?;

        Ok((
            Self {
                stop_flag,
                handle: Some(handle),
            },
            rx,
        ))
    }

    /// Start a fake audio source that generates a 440Hz sine wave.
    ///
    /// Always available — no `capture` feature required.
    ///
    /// # Errors
    ///
    /// Returns an error if the OS thread cannot be spawned.
    #[allow(clippy::needless_pass_by_value)] // config is moved into the thread closure
    pub fn start_fake(config: AudioConfig) -> Result<(Self, mpsc::Receiver<AudioFrame>)> {
        let (tx, rx) = mpsc::sync_channel(CAPTURE_CHANNEL_CAPACITY);
        let stop_flag = Arc::new(AtomicBool::new(false));
        let stop_flag_thread = Arc::clone(&stop_flag);

        let handle = thread::Builder::new()
            .name("pairwire-audio-fake".into())
            .spawn(move || {
                info!("fake audio capture thread started");
                let mut phase: f32 = 0.0;
                let freq = 440.0_f32;
                #[allow(clippy::cast_precision_loss)]
                // sample_rate is ≤192000; fits in f32 mantissa
                let sample_rate = config.sample_rate as f32;
                let frame_duration = Duration::from_micros(
                    OPUS_FRAME_SAMPLES as u64 * 1_000_000 / u64::from(config.sample_rate),
                );
                let mut timestamp_us: u64 = 0;

                while !stop_flag_thread.load(Ordering::Relaxed) {
                    let mut samples =
                        Vec::with_capacity(OPUS_FRAME_SAMPLES * config.channels as usize);
                    for _ in 0..OPUS_FRAME_SAMPLES {
                        let sample = (phase * 2.0 * std::f32::consts::PI).sin() * 0.1;
                        for _ in 0..config.channels {
                            samples.push(sample);
                        }
                        phase = (phase + freq / sample_rate).fract();
                    }

                    let frame = AudioFrame {
                        samples,
                        sample_rate: config.sample_rate,
                        channels: config.channels,
                        timestamp_us,
                    };
                    #[allow(clippy::cast_possible_truncation)]
                    // Duration::as_micros() returns u128; audio timestamps fit in u64 (>500k years)
                    {
                        timestamp_us += frame_duration.as_micros() as u64;
                    }

                    if tx.send(frame).is_err() {
                        break;
                    }
                    thread::sleep(frame_duration);
                }
                info!("fake audio capture thread stopped");
            })
            .map_err(|e| Error::Audio(format!("spawn: {e}")))?;

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
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

impl Drop for AudioCaptureThread {
    fn drop(&mut self) {
        self.stop_flag.store(true, Ordering::Relaxed);
    }
}

/// Marker type for the fake audio source (used in tests).
pub struct FakeAudioSource;
