//! H.264 decode thread using `FFmpeg`.
//!
//! Channel design:
//! - input:  `std::sync::mpsc::Receiver<EncodedPacket>` (OS thread → OS thread)
//! - output: `tokio::sync::mpsc::Sender<DecodedFrame>`  (OS thread → async task)
//!
//! Requires the `ffmpeg` feature.

#[cfg(feature = "ffmpeg")]
mod inner {
    use std::{
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
            mpsc,
        },
        thread,
    };

    use bytes::Bytes;
    use ffmpeg_next::{
        codec::{self, decoder},
        format::Pixel,
        frame,
        software::scaling::{self, Flags},
    };
    use pairwire_core::{Error, Result};
    use tokio::sync::mpsc as async_mpsc;
    use tracing::{debug, info, warn};

    use crate::frame::{DecodedFrame, EncodedPacket};

    const DECODE_CHANNEL_CAPACITY: usize = 8;

    pub struct DecodeThread {
        stop_flag: Arc<AtomicBool>,
        handle: Option<thread::JoinHandle<()>>,
    }

    impl DecodeThread {
        pub fn start(
            input: mpsc::Receiver<EncodedPacket>,
        ) -> Result<(Self, async_mpsc::Receiver<DecodedFrame>)> {
            let (tx, rx) = async_mpsc::channel(DECODE_CHANNEL_CAPACITY);
            let stop_flag = Arc::new(AtomicBool::new(false));
            let stop_flag_thread = Arc::clone(&stop_flag);

            let handle = thread::Builder::new()
                .name("pairwire-decode".into())
                .spawn(move || {
                    if let Err(e) = decode_loop(&input, &tx, &stop_flag_thread) {
                        warn!(error = %e, "decode thread error");
                    }
                    info!("decode thread stopped");
                })
                .map_err(|e| Error::Decode(format!("spawn decode thread: {e}")))?;

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

    impl Drop for DecodeThread {
        fn drop(&mut self) {
            self.stop_flag.store(true, Ordering::Relaxed);
        }
    }

    fn decode_loop(
        input: &mpsc::Receiver<EncodedPacket>,
        tx: &async_mpsc::Sender<DecodedFrame>,
        stop_flag: &AtomicBool,
    ) -> Result<()> {
        ffmpeg_next::init().map_err(|e| Error::Decode(format!("ffmpeg init: {e}")))?;

        let codec = decoder::find(codec::Id::H264)
            .ok_or_else(|| Error::Decode("H264 decoder not found".into()))?;

        // Allocate context with codec pointer so codec_type = AVMEDIA_TYPE_VIDEO.
        // SAFETY: codec.as_ptr() is valid; wrap takes ownership of the allocated context.
        let context = unsafe {
            use ffmpeg_next::ffi::avcodec_alloc_context3;
            let ptr = avcodec_alloc_context3(codec.as_ptr());
            if ptr.is_null() {
                return Err(Error::Decode("avcodec_alloc_context3 returned null".into()));
            }
            codec::context::Context::wrap(ptr, None)
        };
        let mut decoder = context
            .decoder()
            .open_as(codec)
            .map_err(|e| Error::Decode(format!("open decoder: {e}")))?
            .video()
            .map_err(|e| Error::Decode(format!("decoder not video: {e}")))?;

        let mut scaler: Option<scaling::Context> = None;
        let mut decoded = frame::Video::empty();
        // Discard delta frames until the first keyframe arrives so the decoder
        // always starts from a clean state.
        let mut waiting_for_keyframe = true;

        info!("decode thread started");

        loop {
            if stop_flag.load(Ordering::Relaxed) {
                break;
            }

            let pkt = match input.recv() {
                Ok(p) => p,
                Err(_) => break,
            };

            // Keepalive packets have empty data — skip them so we don't
            // feed a zero-length buffer to FFmpeg (which can crash or error).
            if pkt.is_keepalive {
                debug!("skipping keepalive packet in decoder");
                continue;
            }

            // Wait for a keyframe before feeding anything to the decoder.
            if waiting_for_keyframe {
                if !pkt.is_keyframe {
                    debug!("waiting for keyframe, dropping delta");
                    continue;
                }
                waiting_for_keyframe = false;
            }

            let mut ff_pkt = ffmpeg_next::Packet::copy(&pkt.data);
            ff_pkt.set_pts(Some(pkt.timestamp_us as i64));
            ff_pkt.set_dts(Some(pkt.timestamp_us as i64));

            if let Err(e) = decoder.send_packet(&ff_pkt) {
                // Corrupted or out-of-order packet — request a fresh keyframe.
                warn!(error = %e, "send_packet failed, waiting for next keyframe");
                waiting_for_keyframe = true;
                continue;
            }

            loop {
                match decoder.receive_frame(&mut decoded) {
                    Ok(()) => {
                        let w = decoded.width();
                        let h = decoded.height();

                        if scaler.is_none() {
                            scaler = Some(
                                scaling::Context::get(
                                    decoded.format(),
                                    w,
                                    h,
                                    Pixel::BGRA,
                                    w,
                                    h,
                                    Flags::BILINEAR,
                                )
                                .map_err(|e| Error::Decode(format!("scaler init: {e}")))?,
                            );
                            info!(width = w, height = h, "decoder: first frame");
                        }

                        let sc = scaler
                            .as_mut()
                            .ok_or_else(|| Error::Decode("scaler not initialized".into()))?;
                        let mut bgra = frame::Video::new(Pixel::BGRA, w, h);
                        sc.run(&decoded, &mut bgra)
                            .map_err(|e| Error::Decode(format!("scale: {e}")))?;

                        let stride = bgra.stride(0);
                        let row_bytes = w as usize * 4;
                        let src = bgra.data(0);
                        let mut data = Vec::with_capacity(row_bytes * h as usize);
                        for row in 0..h as usize {
                            data.extend_from_slice(&src[row * stride..row * stride + row_bytes]);
                        }

                        let ts = decoded.pts().unwrap_or(pkt.timestamp_us as i64) as u64;
                        let out = DecodedFrame {
                            data: Bytes::from(data),
                            width: w,
                            height: h,
                            timestamp_us: ts,
                        };

                        if tx.blocking_send(out).is_err() {
                            return Ok(());
                        }
                    }
                    Err(ffmpeg_next::Error::Other { errno })
                        if errno == ffmpeg_next::error::EAGAIN =>
                    {
                        break;
                    }
                    Err(e) => return Err(Error::Decode(format!("receive_frame: {e}"))),
                }
            }
        }
        Ok(())
    }
}

#[cfg(feature = "ffmpeg")]
pub use inner::DecodeThread;

#[cfg(test)]
#[cfg(feature = "ffmpeg")]
mod tests {
    use crate::{DecodeThread, EncodeThread, capture_thread::CaptureThread};
    use pairwire_core::config::CaptureConfig;
    use std::time::Duration;

    #[test]
    fn encode_decode_round_trip() {
        let config = CaptureConfig {
            fps: 10,
            width: 320,
            height: 240,
            ..Default::default()
        };

        let (capture, frame_rx) = CaptureThread::start_fake(config.clone()).unwrap();

        // encode: sync input, async output
        let (encode, pkt_rx_async) = EncodeThread::start(config.clone(), frame_rx).unwrap();

        // Bridge async→sync for decode thread input
        let (pkt_tx_sync, pkt_rx_sync) = std::sync::mpsc::sync_channel(8);
        let bridge = std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .build()
                .unwrap();
            rt.block_on(async move {
                let mut rx = pkt_rx_async;
                while let Some(pkt) = rx.recv().await {
                    if pkt_tx_sync.send(pkt).is_err() {
                        break;
                    }
                }
            });
        });

        let (decode, mut decoded_rx) = DecodeThread::start(pkt_rx_sync).unwrap();

        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_time()
            .build()
            .unwrap();

        let frames = rt.block_on(async {
            let mut frames = Vec::new();
            let deadline = tokio::time::Instant::now() + Duration::from_millis(2000);
            while frames.len() < 2 {
                tokio::select! {
                    Some(f) = decoded_rx.recv() => frames.push(f),
                    _ = tokio::time::sleep_until(deadline) => break,
                }
            }
            frames
        });

        capture.stop();
        encode.stop();
        decode.stop();
        let _ = bridge.join();

        assert!(!frames.is_empty(), "expected decoded frames, got none");
        for f in &frames {
            assert_eq!(f.width, config.width);
            assert_eq!(f.height, config.height);
            assert_eq!(f.data.len(), (config.width * config.height * 4) as usize);
        }
    }
}
