use std::sync::mpsc::SyncSender;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::thread;
use std::time::{Duration, Instant};

use tracing::info;

use crate::types::{PixelFormat, Timestamp, VideoFrame};

use super::{SourceError, VideoSource};

/// Generates animated SMPTE-style color bars at a fixed FPS.
///
/// Useful for testing the pipeline (compositor, encoder, preview) when no
/// real screen capture source is available (e.g. WSL2, CI, headless).
pub struct TestSource {
    width: u32,
    height: u32,
    fps: u32,
    running: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}

impl TestSource {
    pub fn new(width: u32, height: u32, fps: u32) -> Self {
        TestSource {
            width,
            height,
            fps,
            running: Arc::new(AtomicBool::new(false)),
            thread: None,
        }
    }
}

impl VideoSource for TestSource {
    fn name(&self) -> &str {
        "TestSource"
    }

    fn start(&mut self, tx: SyncSender<VideoFrame>) -> Result<(), SourceError> {
        if self.running.load(Ordering::SeqCst) {
            return Err(SourceError::AlreadyRunning);
        }
        let running = Arc::clone(&self.running);
        running.store(true, Ordering::SeqCst);
        let (w, h, fps) = (self.width, self.height, self.fps);

        let thread = thread::Builder::new()
            .name("obs-test-source".into())
            .spawn(move || test_loop(tx, running, w, h, fps))
            .map_err(|e| SourceError::Init(format!("{e}")))?;

        self.thread = Some(thread);
        info!("TestSource started ({}×{} @ {} fps)", w, h, fps);
        Ok(())
    }

    fn stop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }

    fn native_resolution(&self) -> (u32, u32) {
        (self.width, self.height)
    }
}

// ── Frame generation ──────────────────────────────────────────────────────────

/// 8 classic SMPTE color-bar colors (BGRA).
const BARS: [(u8, u8, u8); 8] = [
    (192, 192, 192), // White
    (192, 192, 0),   // Yellow
    (0, 192, 192),   // Cyan
    (0, 192, 0),     // Green
    (192, 0, 192),   // Magenta
    (192, 0, 0),     // Red
    (0, 0, 192),     // Blue
    (0, 0, 0),       // Black
];

fn test_loop(
    tx: SyncSender<VideoFrame>,
    running: Arc<AtomicBool>,
    width: u32,
    height: u32,
    fps: u32,
) {
    let frame_duration = Duration::from_nanos(1_000_000_000 / fps as u64);
    let mut frame_number: u64 = 0;
    let mut next_deadline = Instant::now() + frame_duration;

    while running.load(Ordering::SeqCst) {
        let data = make_frame(width, height, frame_number, fps);
        let pts = Timestamp::from_frame(frame_number, fps);
        frame_number += 1;

        let frame = VideoFrame::new(width, height, PixelFormat::BGRA32, data, pts);

        match tx.try_send(frame) {
            Ok(_) => {}
            Err(std::sync::mpsc::TrySendError::Full(_)) => {}
            Err(std::sync::mpsc::TrySendError::Disconnected(_)) => break,
        }

        let now = Instant::now();
        if now < next_deadline {
            thread::sleep(next_deadline - now);
        }
        next_deadline += frame_duration;
    }
}

/// Build a single BGRA32 frame: animated color bars + a moving white marker.
fn make_frame(width: u32, height: u32, frame: u64, fps: u32) -> Vec<u8> {
    let w = width as usize;
    let h = height as usize;
    let mut data = vec![0u8; w * h * 4];

    let bar_w = w / BARS.len();

    // Color bars in the top 3/4 of the frame.
    let bar_region = h * 3 / 4;
    for y in 0..bar_region {
        for x in 0..w {
            let bar = (x / bar_w).min(BARS.len() - 1);
            let (b, g, r) = BARS[bar];
            let off = (y * w + x) * 4;
            data[off] = b;
            data[off + 1] = g;
            data[off + 2] = r;
            data[off + 3] = 255;
        }
    }

    // Bottom 1/4: dark grey background with moving white marker.
    for y in bar_region..h {
        for x in 0..w {
            let off = (y * w + x) * 4;
            data[off] = 32;
            data[off + 1] = 32;
            data[off + 2] = 32;
            data[off + 3] = 255;
        }
    }

    // Animated white marker (sweeps left to right once per second).
    let total_frames = fps as u64;
    let pos = ((frame % total_frames) as usize * w) / total_frames as usize;
    let marker_w = (w / 40).max(4);
    let y_start = bar_region;
    let y_end = h;
    for y in y_start..y_end {
        for dx in 0..marker_w {
            let x = (pos + dx).min(w - 1);
            let off = (y * w + x) * 4;
            data[off] = 255;
            data[off + 1] = 255;
            data[off + 2] = 255;
            data[off + 3] = 255;
        }
    }

    // Overlay frame counter as a simple binary indicator in top-left corner.
    let indicator_size: usize = 8;
    for bit in 0..8usize {
        let is_set = (frame >> bit) & 1 == 1;
        let color = if is_set { 255u8 } else { 0u8 };
        let x_start = bit * (indicator_size + 2) + 4;
        for y in 4..4 + indicator_size {
            for x in x_start..x_start + indicator_size {
                if x < w && y < h {
                    let off = (y * w + x) * 4;
                    data[off] = color;
                    data[off + 1] = color;
                    data[off + 2] = color;
                    data[off + 3] = 255;
                }
            }
        }
    }

    data
}
