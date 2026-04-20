use std::sync::mpsc::SyncSender;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::thread;
use std::time::{Duration, Instant};

use scap::{
    capturer::{Capturer, Options, Resolution},
    frame::Frame,
};
use tracing::{debug, error, info, warn};

use crate::types::{PixelFormat, Timestamp, VideoFrame};

use super::{SourceError, VideoSource};

/// Captures the primary display and produces BGRA32 `VideoFrame`s at a fixed FPS.
pub struct ScreenCaptureSource {
    display_index: usize,
    fps: u32,
    running: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}

impl ScreenCaptureSource {
    /// Create a new screen capture source.
    ///
    /// * `display_index` — which display to capture (0 = primary)
    /// * `fps`           — target capture frame rate (e.g. 30)
    pub fn new(display_index: usize, fps: u32) -> Self {
        ScreenCaptureSource {
            display_index,
            fps,
            running: Arc::new(AtomicBool::new(false)),
            thread: None,
        }
    }
}

impl VideoSource for ScreenCaptureSource {
    fn name(&self) -> &str {
        "ScreenCapture"
    }

    fn start(&mut self, tx: SyncSender<VideoFrame>) -> Result<(), SourceError> {
        if self.running.load(Ordering::SeqCst) {
            return Err(SourceError::AlreadyRunning);
        }

        // Check if screen capture is supported on this platform/session.
        if !scap::is_supported() {
            return Err(SourceError::Init(
                "Screen capture is not supported on this platform".into(),
            ));
        }

        // On macOS this may trigger a permission prompt.
        if !scap::has_permission() {
            return Err(SourceError::Init(
                "Screen recording permission has not been granted".into(),
            ));
        }

        let fps = self.fps;
        let running = Arc::clone(&self.running);
        running.store(true, Ordering::SeqCst);

        let thread = thread::Builder::new()
            .name("obs-screen-capture".into())
            .spawn(move || {
                capture_loop(tx, running, fps);
            })
            .map_err(|e| SourceError::Init(format!("Failed to spawn thread: {e}")))?;

        self.thread = Some(thread);
        info!("ScreenCaptureSource started ({} fps)", fps);
        Ok(())
    }

    fn stop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        info!("ScreenCaptureSource stopped");
    }

    fn native_resolution(&self) -> (u32, u32) {
        (0, 0)
    }
}

// ── Capture loop (runs on its own thread) ────────────────────────────────────

fn capture_loop(tx: SyncSender<VideoFrame>, running: Arc<AtomicBool>, fps: u32) {
    // scap can panic internally on unsupported platforms (e.g. WSL2 without
    // a proper XDG Desktop Portal). Wrap in catch_unwind so the thread exits
    // cleanly instead of aborting the process.
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        capture_loop_inner(tx, running, fps)
    }));
    if let Err(payload) = result {
        let msg = payload
            .downcast_ref::<String>()
            .map(|s| s.as_str())
            .or_else(|| payload.downcast_ref::<&str>().copied())
            .unwrap_or("unknown panic");
        error!("Screen capture thread panicked: {msg}");
        error!("Hint: screen capture is not supported in WSL2 without XDG Desktop Portal.");
        error!("Try running with --source test for a synthetic test pattern.");
    }
}

fn capture_loop_inner(tx: SyncSender<VideoFrame>, running: Arc<AtomicBool>, fps: u32) {
    let options = Options {
        fps,
        output_resolution: Resolution::Captured,
        show_cursor: true,
        show_highlight: false,
        ..Default::default()
    };

    let mut capturer = match Capturer::build(options) {
        Ok(c) => c,
        Err(e) => {
            error!("Failed to create Capturer: {:?}", e);
            return;
        }
    };

    capturer.start_capture();

    let frame_duration = Duration::from_nanos(1_000_000_000 / fps as u64);
    let mut frame_number: u64 = 0;
    let mut next_deadline = Instant::now() + frame_duration;

    while running.load(Ordering::SeqCst) {
        match capturer.get_next_frame() {
            Ok(frame) => {
                let pts = Timestamp::from_frame(frame_number, fps);
                frame_number += 1;

                if let Some(video_frame) = convert_frame(frame, pts) {
                    match tx.try_send(video_frame) {
                        Ok(_) => debug!("sent frame {}", frame_number),
                        Err(std::sync::mpsc::TrySendError::Full(_)) => {
                            warn!("frame channel full, dropping frame {}", frame_number);
                        }
                        Err(std::sync::mpsc::TrySendError::Disconnected(_)) => {
                            info!("frame channel disconnected — stopping capture");
                            break;
                        }
                    }
                }
            }
            Err(e) => {
                error!("get_next_frame error: {e}");
                break;
            }
        }

        // Pace the loop to the target FPS.
        let now = Instant::now();
        if now < next_deadline {
            thread::sleep(next_deadline - now);
        }
        next_deadline += frame_duration;
    }

    capturer.stop_capture();
}

/// Convert a `scap::frame::Frame` into a BGRA32 `VideoFrame`.
fn convert_frame(frame: Frame, pts: Timestamp) -> Option<VideoFrame> {
    match frame {
        // ── Already BGRA ──────────────────────────────────────────────────────
        Frame::BGRA(f) => {
            let width = f.width as u32;
            let height = f.height as u32;
            Some(VideoFrame::new(width, height, PixelFormat::BGRA32, f.data, pts))
        }

        // ── BGR0 / BGRx: BGR + 1 padding byte → set A=255 ─────────────────────
        Frame::BGR0(f) => {
            let width = f.width as u32;
            let height = f.height as u32;
            let mut bgra = Vec::with_capacity(f.data.len());
            // BGR0 can be 4 bytes (BGRX) or 3 bytes (BGR) depending on platform.
            // Detect by comparing data length to 4-byte and 3-byte sizes.
            let pixels = width as usize * height as usize;
            if f.data.len() == pixels * 4 {
                for chunk in f.data.chunks_exact(4) {
                    bgra.push(chunk[0]); // B
                    bgra.push(chunk[1]); // G
                    bgra.push(chunk[2]); // R
                    bgra.push(255);      // A
                }
            } else {
                for chunk in f.data.chunks_exact(3) {
                    bgra.push(chunk[0]); // B
                    bgra.push(chunk[1]); // G
                    bgra.push(chunk[2]); // R
                    bgra.push(255);      // A
                }
            }
            Some(VideoFrame::new(width, height, PixelFormat::BGRA32, bgra, pts))
        }

        Frame::BGRx(f) => {
            let width = f.width as u32;
            let height = f.height as u32;
            let mut bgra = Vec::with_capacity(f.data.len());
            for chunk in f.data.chunks_exact(4) {
                bgra.push(chunk[0]); // B
                bgra.push(chunk[1]); // G
                bgra.push(chunk[2]); // R
                bgra.push(255);      // A
            }
            Some(VideoFrame::new(width, height, PixelFormat::BGRA32, bgra, pts))
        }

        // ── RGB / RGBx: swap R and B ──────────────────────────────────────────
        Frame::RGB(f) => {
            let width = f.width as u32;
            let height = f.height as u32;
            let mut bgra = Vec::with_capacity(width as usize * height as usize * 4);
            for chunk in f.data.chunks_exact(3) {
                bgra.push(chunk[2]); // B (from R)
                bgra.push(chunk[1]); // G
                bgra.push(chunk[0]); // R (from B)
                bgra.push(255);      // A
            }
            Some(VideoFrame::new(width, height, PixelFormat::BGRA32, bgra, pts))
        }

        Frame::RGBx(f) => {
            let width = f.width as u32;
            let height = f.height as u32;
            let mut bgra = Vec::with_capacity(f.data.len());
            for chunk in f.data.chunks_exact(4) {
                bgra.push(chunk[2]); // B (from R)
                bgra.push(chunk[1]); // G
                bgra.push(chunk[0]); // R (from B)
                bgra.push(255);      // A
            }
            Some(VideoFrame::new(width, height, PixelFormat::BGRA32, bgra, pts))
        }

        // ── XBGR: padding, B, G, R ────────────────────────────────────────────
        Frame::XBGR(f) => {
            let width = f.width as u32;
            let height = f.height as u32;
            let mut bgra = Vec::with_capacity(f.data.len());
            for chunk in f.data.chunks_exact(4) {
                bgra.push(chunk[1]); // B
                bgra.push(chunk[2]); // G
                bgra.push(chunk[3]); // R
                bgra.push(255);      // A
            }
            Some(VideoFrame::new(width, height, PixelFormat::BGRA32, bgra, pts))
        }

        Frame::YUVFrame(f) => {
            // YUV frames are not handled in Phase 1 — the compositor and preview
            // both expect BGRA32.  Log a warning and skip.
            warn!(
                "YUVFrame captured ({}×{}) — skipping (not yet supported)",
                f.width, f.height
            );
            None
        }
    }
}
