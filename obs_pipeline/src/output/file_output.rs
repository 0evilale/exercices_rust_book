//! MP4 file recorder: H.264 video encoder + MP4 muxer.
//!
//! FFmpeg requires the muxer's `AVFormatContext` to be configured from the
//! encoder context at setup time, so we bundle both into a single struct
//! rather than splitting across two traits/threads.

use std::path::Path;

use ffmpeg_next as ff;
use ff::format::Pixel;
use ff::{codec, encoder, format, frame, software, Dictionary, Packet, Rational};
use tracing::{debug, info, warn};

use crate::types::VideoFrame;

use super::OutputError;

/// Bitrate in bits per second for 1080p@30 by default.
const DEFAULT_BITRATE_KBPS: usize = 6_000;

pub struct FileRecorder {
    octx: format::context::Output,
    encoder: encoder::Video,
    scaler: software::scaling::Context,
    stream_index: usize,
    stream_time_base: Rational,
    encoder_time_base: Rational,
    width: u32,
    height: u32,
    frame_count: i64,
    finalized: bool,
}

impl FileRecorder {
    /// Open an MP4 file for writing, configure the H.264 encoder and MP4 muxer.
    ///
    /// `path` should end in `.mp4`.
    pub fn new(
        path: &Path,
        width: u32,
        height: u32,
        fps: u32,
        bitrate_kbps: Option<usize>,
    ) -> Result<Self, OutputError> {
        ff::init()?;

        let path_str = path
            .to_str()
            .ok_or_else(|| OutputError::Setup("Path is not valid UTF-8".into()))?;

        // 1. Create output (muxer) context.
        let mut octx = format::output(&path_str)?;
        let global_header = octx
            .format()
            .flags()
            .contains(format::flag::Flags::GLOBAL_HEADER);

        // 2. Find H.264 encoder.
        let codec = encoder::find(codec::Id::H264)
            .ok_or_else(|| OutputError::Setup("libx264 not available".into()))?;

        // 3. Add a video stream to the output.
        let mut stream = octx.add_stream(codec)?;
        let stream_index = stream.index();

        // 4. Configure the encoder.
        let encoder_time_base = Rational::new(1, fps as i32);
        let mut enc = codec::context::Context::new_with_codec(codec)
            .encoder()
            .video()?;
        enc.set_width(width);
        enc.set_height(height);
        enc.set_format(Pixel::YUV420P);
        enc.set_time_base(encoder_time_base);
        enc.set_frame_rate(Some(Rational::new(fps as i32, 1)));
        enc.set_bit_rate((bitrate_kbps.unwrap_or(DEFAULT_BITRATE_KBPS)) * 1000);
        enc.set_gop(fps * 2); // keyframe every 2 seconds
        enc.set_max_b_frames(0); // no B-frames for simpler streaming later

        if global_header {
            enc.set_flags(codec::Flags::GLOBAL_HEADER);
        }

        // 5. Apply x264-specific options.
        let mut opts = Dictionary::new();
        opts.set("preset", "veryfast");
        opts.set("tune", "zerolatency");

        let opened = enc.open_with(opts)?;

        // 6. Copy encoder params onto the stream + set stream time base.
        stream.set_parameters(&opened);
        stream.set_time_base(encoder_time_base);
        let stream_time_base = stream.time_base();

        // 7. Create BGRA → YUV420P scaler.
        let scaler = software::scaling::context::Context::get(
            Pixel::BGRA,
            width,
            height,
            Pixel::YUV420P,
            width,
            height,
            software::scaling::flag::Flags::BILINEAR,
        )?;

        // 8. Write header.
        octx.write_header()?;

        info!(
            "FileRecorder initialised: {} ({}×{} @ {} fps, {} kbps)",
            path_str,
            width,
            height,
            fps,
            bitrate_kbps.unwrap_or(DEFAULT_BITRATE_KBPS)
        );

        Ok(FileRecorder {
            octx,
            encoder: opened,
            scaler,
            stream_index,
            stream_time_base,
            encoder_time_base,
            width,
            height,
            frame_count: 0,
            finalized: false,
        })
    }

    /// Encode a BGRA32 video frame and write packets to the output file.
    pub fn push_frame(&mut self, frame: &VideoFrame) -> Result<(), OutputError> {
        if frame.width != self.width || frame.height != self.height {
            return Err(OutputError::Setup(format!(
                "Frame size mismatch: expected {}×{}, got {}×{}",
                self.width, self.height, frame.width, frame.height
            )));
        }

        // Build an AVFrame holding the BGRA data.
        let mut bgra = frame::Video::new(Pixel::BGRA, self.width, self.height);
        {
            let dst = bgra.data_mut(0);
            let src = frame.data.as_ref();
            let copy_len = dst.len().min(src.len());
            dst[..copy_len].copy_from_slice(&src[..copy_len]);
        }

        // Convert BGRA → YUV420P.
        let mut yuv = frame::Video::empty();
        self.scaler.run(&bgra, &mut yuv)?;

        yuv.set_pts(Some(self.frame_count));
        self.frame_count += 1;

        self.encoder.send_frame(&yuv)?;
        self.drain_packets()?;

        debug!("pushed frame {}", self.frame_count);
        Ok(())
    }

    /// Drain any available packets from the encoder and mux them.
    fn drain_packets(&mut self) -> Result<(), OutputError> {
        let mut packet = Packet::empty();
        loop {
            match self.encoder.receive_packet(&mut packet) {
                Ok(_) => {
                    packet.set_stream(self.stream_index);
                    packet.rescale_ts(self.encoder_time_base, self.stream_time_base);
                    packet.write_interleaved(&mut self.octx)?;
                }
                Err(ff::Error::Other { errno }) if errno == ff::error::EAGAIN => break,
                Err(ff::Error::Eof) => break,
                Err(e) => return Err(e.into()),
            }
        }
        Ok(())
    }

    /// Flush the encoder and write the MP4 trailer. Consumes `self` so the
    /// recorder cannot be used again.
    pub fn finalize(mut self) -> Result<(), OutputError> {
        self.finalize_internal()
    }

    fn finalize_internal(&mut self) -> Result<(), OutputError> {
        if self.finalized {
            return Ok(());
        }
        self.finalized = true;

        self.encoder.send_eof()?;
        self.drain_packets()?;
        self.octx.write_trailer()?;
        info!("FileRecorder finalised ({} frames written)", self.frame_count);
        Ok(())
    }
}

impl Drop for FileRecorder {
    fn drop(&mut self) {
        if !self.finalized {
            if let Err(e) = self.finalize_internal() {
                warn!("FileRecorder::drop — finalize failed: {e}");
            }
        }
    }
}
