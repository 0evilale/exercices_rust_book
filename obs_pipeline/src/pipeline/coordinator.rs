use std::sync::mpsc::{self, Receiver, SyncSender};
use std::sync::Arc;
use std::thread;

use tracing::{error, info};
use winit::event_loop::EventLoopProxy;

use crate::preview::renderer::PreviewEvent;
use crate::scene::compositor::Compositor;
use crate::scene::{Scene, SceneItem, SourceId, Transform};
use crate::source::screen::ScreenCaptureSource;
use crate::source::VideoSource;
use crate::types::VideoFrame;

/// Buffer depth for the inter-thread channels.
/// A depth of 4 means: hold at most 4 frames in flight before backpressure kicks in.
const CHANNEL_DEPTH: usize = 4;

/// Top-level pipeline configuration.
#[derive(Debug, Clone)]
pub struct PipelineConfig {
    /// Output canvas width in pixels.
    pub canvas_width: u32,
    /// Output canvas height in pixels.
    pub canvas_height: u32,
    /// Target frames per second.
    pub fps: u32,
}

impl Default for PipelineConfig {
    fn default() -> Self {
        PipelineConfig {
            canvas_width: 1920,
            canvas_height: 1080,
            fps: 30,
        }
    }
}

/// Owns and coordinates all pipeline threads.
///
/// Thread layout (Phase 1 — MVP):
///
/// ```text
/// [ScreenCaptureThread] --VideoFrame--> [CompositorThread]
///                                              |
///                                    Arc<VideoFrame>
///                                              |
///                                       [PreviewThread ← winit event loop on main thread]
/// ```
pub struct Pipeline {
    config: PipelineConfig,
    /// Handle to the compositor thread.
    compositor_thread: Option<thread::JoinHandle<()>>,
    /// Sender used to push raw frames from the screen capture source.
    capture_tx: SyncSender<VideoFrame>,
}

impl Pipeline {
    /// Build and start the pipeline.
    ///
    /// Returns the `Pipeline` handle together with:
    /// * `preview_rx` — the receiver the winit event loop reads composed frames from.
    /// * `proxy`      — used to send `PreviewEvent::NewFrame` to wake the event loop.
    pub fn start(
        config: PipelineConfig,
        proxy: EventLoopProxy<PreviewEvent>,
    ) -> (Self, Receiver<Arc<VideoFrame>>) {
        // ── Channel: screen capture → compositor ──────────────────────────────
        let (capture_tx, capture_rx) = mpsc::sync_channel::<VideoFrame>(CHANNEL_DEPTH);

        // ── Channel: compositor → preview renderer ────────────────────────────
        let (preview_tx, preview_rx) = mpsc::sync_channel::<Arc<VideoFrame>>(CHANNEL_DEPTH);

        // ── Build scene with a single full-screen source ───────────────────────
        let source_id = SourceId(0);
        let mut scene = Scene::new("Main", config.canvas_width, config.canvas_height);
        scene.add_item(SceneItem {
            source_id,
            transform: Transform::fullscreen(config.canvas_width, config.canvas_height),
            visible: true,
            z_order: 0,
        });

        let mut compositor = Compositor::new(scene);
        let fps = config.fps;

        // ── Compositor thread ─────────────────────────────────────────────────
        let compositor_thread = thread::Builder::new()
            .name("obs-compositor".into())
            .spawn(move || {
                compositor_loop(
                    capture_rx,
                    preview_tx,
                    proxy,
                    compositor,
                    source_id,
                    fps,
                )
            })
            .expect("Failed to spawn compositor thread");

        // ── Start screen capture source ───────────────────────────────────────
        let capture_tx_clone = capture_tx.clone();
        let mut screen_src = ScreenCaptureSource::new(0, fps);
        if let Err(e) = screen_src.start(capture_tx_clone) {
            error!("ScreenCaptureSource failed to start: {e}");
        }

        // Move source into a thread so its `stop()` can be called on shutdown.
        thread::Builder::new()
            .name("obs-screen-src-guard".into())
            .spawn(move || {
                // This thread just holds the source alive until drop.
                // The capture loop is internal to ScreenCaptureSource.
                drop(screen_src);
            })
            .expect("Failed to spawn source guard thread");

        info!("Pipeline started ({}×{} @ {} fps)", config.canvas_width, config.canvas_height, fps);

        let pipeline = Pipeline {
            config,
            compositor_thread: Some(compositor_thread),
            capture_tx,
        };

        (pipeline, preview_rx)
    }

    /// Stop the pipeline gracefully.
    pub fn stop(mut self) {
        // Dropping capture_tx signals the compositor thread to exit when the
        // channel becomes empty and the sender is gone.
        drop(self.capture_tx);

        if let Some(t) = self.compositor_thread.take() {
            let _ = t.join();
        }
        info!("Pipeline stopped");
    }
}

// ── Compositor loop ───────────────────────────────────────────────────────────

fn compositor_loop(
    capture_rx: Receiver<VideoFrame>,
    preview_tx: SyncSender<Arc<VideoFrame>>,
    proxy: EventLoopProxy<PreviewEvent>,
    mut compositor: Compositor,
    source_id: SourceId,
    fps: u32,
) {
    let mut frame_number: u64 = 0;

    for raw_frame in &capture_rx {
        // Update cache for the single source.
        compositor.update_source(source_id, raw_frame);

        let pts = crate::types::Timestamp::from_frame(frame_number, fps);
        frame_number += 1;

        // Composite all scene items into one frame.
        let composed = compositor.composite(pts);
        let composed = Arc::new(composed);

        // Forward to the preview renderer; drop if the buffer is full (frame skip).
        match preview_tx.try_send(Arc::clone(&composed)) {
            Ok(_) => {}
            Err(mpsc::TrySendError::Full(_)) => {} // preview is behind — skip
            Err(mpsc::TrySendError::Disconnected(_)) => {
                info!("Preview channel disconnected — stopping compositor");
                break;
            }
        }

        // Wake the winit event loop so it issues a RedrawRequested.
        if proxy.send_event(PreviewEvent::NewFrame).is_err() {
            info!("Event loop proxy disconnected — stopping compositor");
            break;
        }
    }

    info!("Compositor loop finished");
}
