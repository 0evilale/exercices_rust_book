//! Encoder abstractions.
//!
//! In this repository the H.264 encoder is tightly integrated with the MP4
//! muxer (see `output::file_output::FileRecorder`) because `ffmpeg-next` requires
//! the muxer's `AVFormatContext` to be configured from the encoder context at
//! setup time. The traits below document the intended abstraction and will be
//! useful when we add a second encoder (e.g. NVENC) or a second output
//! (e.g. RTMP) in later phases.

use crate::types::{Timestamp, VideoFrame};

// ── Error type ────────────────────────────────────────────────────────────────

#[derive(Debug, thiserror::Error)]
pub enum EncodeError {
    #[error("FFmpeg error: {0}")]
    Ffmpeg(#[from] ffmpeg_next::Error),
    #[error("Encoder init failed: {0}")]
    Init(String),
}

// ── Packet type ───────────────────────────────────────────────────────────────

/// A compressed packet emitted by the encoder.
#[derive(Debug, Clone)]
pub struct EncodedPacket {
    pub data: Vec<u8>,
    pub pts: Timestamp,
    pub is_keyframe: bool,
    pub stream: StreamType,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamType {
    Video,
    Audio,
}

// ── Traits ────────────────────────────────────────────────────────────────────

pub trait VideoEncoder: Send {
    fn push_frame(&mut self, frame: &VideoFrame) -> Result<(), EncodeError>;
    fn flush(&mut self) -> Result<(), EncodeError>;
}
