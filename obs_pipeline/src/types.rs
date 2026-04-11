use std::sync::Arc;

/// Pixel format of a VideoFrame's raw bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelFormat {
    /// 4 bytes per pixel: Blue, Green, Red, Alpha (most screen-capture APIs produce this)
    BGRA32,
    /// 4 bytes per pixel: Red, Green, Blue, Alpha
    RGBA32,
    /// Planar YUV 4:2:0 — used internally by H.264 encoders
    I420,
}

impl PixelFormat {
    /// Bytes per pixel for packed formats; 0 for planar (I420 size depends on dimensions).
    pub fn bytes_per_pixel(self) -> usize {
        match self {
            PixelFormat::BGRA32 | PixelFormat::RGBA32 => 4,
            PixelFormat::I420 => 0, // planar — use VideoFrame::expected_size()
        }
    }
}

/// A monotonic presentation timestamp (nanoseconds).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Timestamp {
    pub nanos: i64,
}

impl Timestamp {
    pub fn zero() -> Self {
        Timestamp { nanos: 0 }
    }

    /// Compute the timestamp for frame number `n` at `fps` frames per second.
    pub fn from_frame(n: u64, fps: u32) -> Self {
        Timestamp {
            nanos: (n * 1_000_000_000 / fps as u64) as i64,
        }
    }
}

/// A single decoded video frame.
///
/// Pixel data is stored in an `Arc<Vec<u8>>` so it can be cheaply cloned and
/// shared across the compositor, preview renderer, and encoder threads without
/// extra heap allocations.
#[derive(Clone)]
pub struct VideoFrame {
    pub width: u32,
    pub height: u32,
    pub format: PixelFormat,
    /// Raw pixel bytes. Layout depends on `format`.
    pub data: Arc<Vec<u8>>,
    pub pts: Timestamp,
}

impl VideoFrame {
    pub fn new(width: u32, height: u32, format: PixelFormat, data: Vec<u8>, pts: Timestamp) -> Self {
        VideoFrame {
            width,
            height,
            format,
            data: Arc::new(data),
            pts,
        }
    }

    /// Allocate a black (zeroed) BGRA32 frame.
    pub fn black(width: u32, height: u32, pts: Timestamp) -> Self {
        let size = (width * height * 4) as usize;
        VideoFrame::new(width, height, PixelFormat::BGRA32, vec![0u8; size], pts)
    }

    /// Expected byte size for the current dimensions and format.
    pub fn expected_size(&self) -> usize {
        match self.format {
            PixelFormat::BGRA32 | PixelFormat::RGBA32 => (self.width * self.height * 4) as usize,
            PixelFormat::I420 => (self.width * self.height * 3 / 2) as usize,
        }
    }
}

impl std::fmt::Debug for VideoFrame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VideoFrame")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("format", &self.format)
            .field("pts_ns", &self.pts.nanos)
            .field("data_len", &self.data.len())
            .finish()
    }
}

/// A chunk of interleaved PCM audio samples (f32, normalized to [-1.0, 1.0]).
#[derive(Debug, Clone)]
pub struct AudioBuffer {
    /// Interleaved samples: [L0, R0, L1, R1, ...]
    pub samples: Vec<f32>,
    pub channels: u8,
    pub sample_rate: u32,
    pub pts: Timestamp,
}

impl AudioBuffer {
    pub fn silence(channels: u8, sample_rate: u32, frames: usize, pts: Timestamp) -> Self {
        AudioBuffer {
            samples: vec![0.0f32; frames * channels as usize],
            channels,
            sample_rate,
            pts,
        }
    }

    /// Number of audio frames (samples / channels).
    pub fn frame_count(&self) -> usize {
        if self.channels == 0 {
            return 0;
        }
        self.samples.len() / self.channels as usize
    }
}
