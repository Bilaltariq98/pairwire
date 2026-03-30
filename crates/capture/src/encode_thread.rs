//! H.264 encode thread using FFmpeg.
//!
//! Channel design (per CONVENTIONS.md):
//! - input:  `std::sync::mpsc::Receiver<RawFrame>`  (OS thread → OS thread)
//! - output: `tokio::sync::mpsc::Sender<EncodedPacket>` (OS thread → async task)
//!
//! `blocking_send` on a `tokio::sync::mpsc::Sender` is safe from OS threads
//! and does not require a tokio runtime on the calling thread.
//!
//! Requires the `ffmpeg` feature.

#[cfg(feature = "ffmpeg")]
mod inner {
    use std::{
        collections::hash_map::DefaultHasher,
        hash::{Hash, Hasher},
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
            mpsc,
        },
        thread,
    };

    use bytes::Bytes;
    use ffmpeg_next::{
        Dictionary, Rational,
        codec::{self, encoder},
        format::Pixel,
        frame, picture,
        software::scaling::{self, Flags},
    };
    use pairwire_core::{Error, Result, config::CaptureConfig};
    use tokio::sync::mpsc as async_mpsc;
    use tracing::{debug, info, warn};

    use crate::frame::{EncodedPacket, RawFrame};

    const ENCODE_CHANNEL_CAPACITY: usize = 4;

    pub struct EncodeThread {
        stop_flag: Arc<AtomicBool>,
        handle: Option<thread::JoinHandle<()>>,
    }

    impl EncodeThread {
        pub fn start(
            config: CaptureConfig,
            input: mpsc::Receiver<RawFrame>,
        ) -> Result<(Self, async_mpsc::Receiver<EncodedPacket>)> {
            let (tx, rx) = async_mpsc::channel(ENCODE_CHANNEL_CAPACITY);
            let stop_flag = Arc::new(AtomicBool::new(false));
            let stop_flag_thread = Arc::clone(&stop_flag);

            let handle = thread::Builder::new()
                .name("pairwire-encode".into())
                .spawn(move || {
                    if let Err(e) = encode_loop(&config, &input, &tx, &stop_flag_thread) {
                        warn!(error = %e, "encode thread error");
                    }
                    info!("encode thread stopped");
                })
                .map_err(|e| Error::Encode(format!("spawn encode thread: {e}")))?;

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

    impl Drop for EncodeThread {
        fn drop(&mut self) {
            self.stop_flag.store(true, Ordering::Relaxed);
        }
    }

    fn encode_loop(
        config: &CaptureConfig,
        input: &mpsc::Receiver<RawFrame>,
        tx: &async_mpsc::Sender<EncodedPacket>,
        stop_flag: &AtomicBool,
    ) -> Result<()> {
        ffmpeg_next::init().map_err(|e| Error::Encode(format!("ffmpeg init: {e}")))?;

        let mut encoder = open_encoder(config)?;
        let mut scaler = scaling::Context::get(
            Pixel::BGRA,
            config.width,
            config.height,
            Pixel::YUV420P,
            config.width,
            config.height,
            Flags::BILINEAR,
        )
        .map_err(|e| Error::Encode(format!("scaler init: {e}")))?;

        let keyframe_interval = config.keyframe_interval_secs * config.fps;
        let keepalive_after = config.keepalive_after_frames;
        let mut frame_index: u64 = 0;
        let mut last_hash: Option<u64> = None;
        let mut static_frame_count: u32 = 0;

        info!(
            width = config.width,
            height = config.height,
            fps = config.fps,
            "encode thread started"
        );

        loop {
            if stop_flag.load(Ordering::Relaxed) {
                break;
            }

            let raw = match input.recv() {
                Ok(f) => f,
                Err(_) => break,
            };

            let hash = hash_frame(&raw.data);
            if last_hash == Some(hash) {
                static_frame_count += 1;
                // Send a keepalive after N consecutive static frames so the
                // receiver knows the stream is alive even with no new content.
                if keepalive_after > 0 && static_frame_count % keepalive_after == 0 {
                    debug!(static_frame_count, "sending keepalive");
                    let keepalive = EncodedPacket {
                        data: Bytes::new(),
                        timestamp_us: raw.timestamp_us,
                        is_keyframe: false,
                        is_keepalive: true,
                    };
                    if tx.blocking_send(keepalive).is_err() {
                        return Ok(());
                    }
                } else {
                    debug!("static frame, skipping");
                }
                continue;
            }
            last_hash = Some(hash);
            static_frame_count = 0;

            let mut src = frame::Video::new(Pixel::BGRA, config.width, config.height);
            fill_bgra_frame(&mut src, &raw.data, config.width, config.height);

            let mut yuv = frame::Video::new(Pixel::YUV420P, config.width, config.height);
            scaler
                .run(&src, &mut yuv)
                .map_err(|e| Error::Encode(format!("scale: {e}")))?;

            yuv.set_pts(Some(frame_index as i64));
            frame_index += 1;

            if frame_index % keyframe_interval as u64 == 1 {
                yuv.set_kind(picture::Type::I);
            }

            encoder
                .send_frame(&yuv)
                .map_err(|e| Error::Encode(format!("send_frame: {e}")))?;

            let mut pkt = ffmpeg_next::Packet::empty();
            loop {
                match encoder.receive_packet(&mut pkt) {
                    Ok(()) => {
                        let ep = EncodedPacket {
                            data: Bytes::copy_from_slice(pkt.data().unwrap_or(&[])),
                            timestamp_us: raw.timestamp_us,
                            is_keyframe: pkt.is_key(),
                            is_keepalive: false,
                        };
                        // blocking_send is safe from OS threads (no runtime needed)
                        if tx.blocking_send(ep).is_err() {
                            return Ok(());
                        }
                    }
                    Err(ffmpeg_next::Error::Other { errno })
                        if errno == ffmpeg_next::error::EAGAIN =>
                    {
                        break;
                    }
                    Err(e) => return Err(Error::Encode(format!("receive_packet: {e}"))),
                }
            }
        }
        Ok(())
    }

    fn open_encoder(config: &CaptureConfig) -> Result<encoder::video::Encoder> {
        use pairwire_core::config::VideoEncoder;

        let codec_name = match config.preferred_encoder {
            VideoEncoder::H264VideoToolbox => {
                if encoder::find_by_name("h264_videotoolbox").is_some() {
                    "h264_videotoolbox"
                } else {
                    warn!("h264_videotoolbox unavailable, using libx264");
                    "libx264"
                }
            }
            VideoEncoder::H264Software => "libx264",
        };

        let codec = encoder::find_by_name(codec_name)
            .ok_or_else(|| Error::Encode(format!("encoder '{codec_name}' not found")))?;

        // Allocate context with the codec pointer so AVCodecContext.codec_type is set.
        // SAFETY: codec.as_ptr() is valid for the lifetime of this call; wrap takes ownership.
        let context = unsafe {
            use ffmpeg_next::ffi::avcodec_alloc_context3;
            let ptr = avcodec_alloc_context3(codec.as_ptr());
            if ptr.is_null() {
                return Err(Error::Encode("avcodec_alloc_context3 returned null".into()));
            }
            codec::context::Context::wrap(ptr, None)
        };
        let mut video = context
            .encoder()
            .video()
            .map_err(|e| Error::Encode(format!("encoder context: {e}")))?;

        video.set_width(config.width);
        video.set_height(config.height);
        video.set_format(Pixel::YUV420P);
        video.set_time_base(Rational::new(1, config.fps as i32));
        video.set_gop(config.keyframe_interval_secs * config.fps);
        video.set_max_b_frames(0);

        let mut opts = Dictionary::new();
        if codec_name == "libx264" {
            opts.set("preset", "ultrafast");
            opts.set("tune", "zerolatency");
        }

        let enc = video
            .open_as_with(codec, opts)
            .map_err(|e| Error::Encode(format!("open encoder: {e}")))?;

        info!(codec = codec_name, "encoder opened");
        Ok(enc)
    }

    fn fill_bgra_frame(frame: &mut frame::Video, data: &[u8], width: u32, height: u32) {
        let stride = frame.stride(0);
        let dst = frame.data_mut(0);
        let row_bytes = width as usize * 4;
        for row in 0..height as usize {
            dst[row * stride..row * stride + row_bytes]
                .copy_from_slice(&data[row * row_bytes..(row + 1) * row_bytes]);
        }
    }

    fn hash_frame(data: &[u8]) -> u64 {
        let mut h = DefaultHasher::new();
        for chunk in data.chunks(64) {
            chunk[0].hash(&mut h);
        }
        h.finish()
    }
}

#[cfg(feature = "ffmpeg")]
pub use inner::EncodeThread;

#[cfg(test)]
#[cfg(feature = "ffmpeg")]
mod tests {
    use crate::{EncodeThread, capture_thread::CaptureThread};
    use pairwire_core::config::CaptureConfig;
    use std::time::Duration;

    /// Directly test FFmpeg encode without threading, to isolate the encode path.
    #[test]
    fn ffmpeg_encode_direct() {
        use ffmpeg_next::{
            Dictionary, Rational,
            codec::{self, encoder},
            format::Pixel,
            frame, picture,
            software::scaling::{self, Flags},
        };

        ffmpeg_next::init().unwrap();

        let codec = encoder::find_by_name("libx264").expect("libx264 not found");
        let context = unsafe {
            use ffmpeg_next::ffi::avcodec_alloc_context3;
            let ptr = avcodec_alloc_context3(codec.as_ptr());
            assert!(!ptr.is_null());
            codec::context::Context::wrap(ptr, None)
        };
        let mut video = context.encoder().video().unwrap();
        video.set_width(64);
        video.set_height(64);
        video.set_format(Pixel::YUV420P);
        video.set_time_base(Rational::new(1, 10));
        video.set_gop(20);
        video.set_max_b_frames(0);

        let mut opts = Dictionary::new();
        opts.set("preset", "ultrafast");
        opts.set("tune", "zerolatency");

        let mut enc = video
            .open_as_with(codec, opts)
            .expect("open encoder failed");

        let mut scaler =
            scaling::Context::get(Pixel::BGRA, 64, 64, Pixel::YUV420P, 64, 64, Flags::BILINEAR)
                .unwrap();

        let mut total_packets = 0usize;
        for i in 0u64..10 {
            let mut src = frame::Video::new(Pixel::BGRA, 64, 64);
            let stride = src.stride(0);
            let data = src.data_mut(0);
            for j in 0..64 * 64 * 4 {
                data[j] = ((i * 37 + j as u64) & 0xff) as u8;
            }
            let _ = stride; // used above

            let mut yuv = frame::Video::new(Pixel::YUV420P, 64, 64);
            scaler.run(&src, &mut yuv).unwrap();
            yuv.set_pts(Some(i as i64));
            if i == 0 {
                yuv.set_kind(picture::Type::I);
            }

            enc.send_frame(&yuv).unwrap();

            let mut pkt = ffmpeg_next::Packet::empty();
            loop {
                match enc.receive_packet(&mut pkt) {
                    Ok(()) => {
                        total_packets += 1;
                    }
                    Err(ffmpeg_next::Error::Other { errno })
                        if errno == ffmpeg_next::error::EAGAIN =>
                    {
                        break;
                    }
                    Err(e) => panic!("receive_packet error: {e}"),
                }
            }
        }

        assert!(
            total_packets > 0,
            "direct FFmpeg encode produced no packets"
        );
    }

    #[test]
    fn encode_fake_frames_produces_packets() {
        let config = CaptureConfig {
            fps: 10,
            width: 320,
            height: 240,
            ..Default::default()
        };

        let (capture, frame_rx) = CaptureThread::start_fake(config.clone()).unwrap();
        let (encode, mut packet_rx) = EncodeThread::start(config, frame_rx).unwrap();

        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_time()
            .build()
            .unwrap();

        let packets = rt.block_on(async {
            let mut pkts = Vec::new();
            let deadline = tokio::time::Instant::now() + Duration::from_millis(1500);
            while pkts.len() < 2 {
                tokio::select! {
                    Some(p) = packet_rx.recv() => pkts.push(p),
                    _ = tokio::time::sleep_until(deadline) => break,
                }
            }
            pkts
        });

        capture.stop();
        encode.stop();

        assert!(!packets.is_empty(), "expected encoded packets, got none");
        assert!(packets[0].is_keyframe, "first packet should be a keyframe");
    }

    #[test]
    fn static_frames_are_skipped() {
        use crate::frame::RawFrame;
        use bytes::Bytes;
        use std::sync::mpsc;
        use std::time::Duration;

        let config = CaptureConfig {
            fps: 30,
            width: 64,
            height: 64,
            ..Default::default()
        };

        let (tx, frame_rx) = mpsc::sync_channel(32);
        let (encode, mut packet_rx) = EncodeThread::start(config.clone(), frame_rx).unwrap();

        // Send 21 identical frames synchronously
        let data = Bytes::from(vec![0x80u8; 64 * 64 * 4]);
        for i in 0..21u64 {
            let _ = tx.send(RawFrame {
                data: data.clone(),
                width: 64,
                height: 64,
                timestamp_us: i * 33_333,
            });
        }
        drop(tx); // signal encode thread to stop

        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_time()
            .build()
            .unwrap();

        let packets = rt.block_on(async {
            let mut pkts = Vec::new();
            let deadline = tokio::time::Instant::now() + Duration::from_millis(500);
            loop {
                tokio::select! {
                    Some(p) = packet_rx.recv() => pkts.push(p),
                    _ = tokio::time::sleep_until(deadline) => break,
                }
            }
            pkts
        });

        encode.stop();

        assert!(
            packets.len() < 5,
            "expected < 5 packets for 21 identical frames, got {}",
            packets.len()
        );
    }
}
