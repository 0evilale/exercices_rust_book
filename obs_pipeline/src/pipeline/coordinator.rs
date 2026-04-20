use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::sync::Arc;
use std::thread;

use tracing::{error, info, warn};
use winit::event_loop::EventLoopProxy;

use crate::output::file_output::FileRecorder;
use crate::preview::renderer::PreviewEvent;
use crate::scene::compositor::Compositor;
use crate::scene::{Scene, SceneItem, SourceId, Transform};
use crate::source::screen::ScreenCaptureSource;
use crate::source::VideoSource;
use crate::types::VideoFrame;

const CHANNEL_DEPTH: usize = 4;

/// Top-level pipeline configuration.
#[derive(Debug, Clone)]
pub struct PipelineConfig {
    pub canvas_width: u32,
    pub canvas_height: u32,
    pub fps: u32,
    /// Optional MP4 recording output.
    pub record_path: Option<PathBuf>,
    /// Bitrate for the recording encoder, in kbps.
    pub record_bitrate_kbps: Option<usize>,
}

impl Default for PipelineConfig {
    fn default() -> Self {
        PipelineConfig {
            canvas_width: 1920,
            canvas_height: 1080,
            fps: 30,
            record_path: None,
            record_bitrate_kbps: None,
        }
    }
}

/// Owns the running pipeline.
///
/// Thread layout with recording enabled:
///
/// ```text
/// [ScreenCapture] --VideoFrame--> [Compositor] ──┬── Arc<VideoFrame> ──> [Preview (main)]
///                                                │
///                                                └── VideoFrame ────────> [Recorder]
/// ```
pub struct Pipeline {
    compositor_thread: Option<thread::JoinHandle<()>>,
    recorder_thread: Option<thread::JoinHandle<()>>,
    capture_tx: SyncSender<VideoFrame>,
    record_tx: Option<SyncSender<VideoFrame>>,
}

impl Pipeline {
    pub fn start(
        config: PipelineConfig,
        proxy: EventLoopProxy<PreviewEvent>,
    ) -> (Self, Receiver<Arc<VideoFrame>>) {
        // ── Channel: screen capture → compositor ──────────────────────────────
        let (capture_tx, capture_rx) = mpsc::sync_channel::<VideoFrame>(CHANNEL_DEPTH);

        // ── Channel: compositor → preview renderer ────────────────────────────
        let (preview_tx, preview_rx) = mpsc::sync_channel::<Arc<VideoFrame>>(CHANNEL_DEPTH);

        // ── Optional channel: compositor → recorder ───────────────────────────
        let (record_tx_opt, recorder_thread) = match config.record_path.clone() {
            Some(path) => {
                let (record_tx, record_rx) = mpsc::sync_channel::<VideoFrame>(CHANNEL_DEPTH);
                let width = config.canvas_width;
                let height = config.canvas_height;
                let fps = config.fps;
                let bitrate = config.record_bitrate_kbps;

                let handle = thread::Builder::new()
                    .name("obs-recorder".into())
                    .spawn(move || {
                        recorder_loop(record_rx, path, width, height, fps, bitrate);
                    })
                    .expect("Failed to spawn recorder thread");

                (Some(record_tx), Some(handle))
            }
            None => (None, None),
        };

        // ── Build scene with a single full-screen source ──────────────────────
        let source_id = SourceId(0);
        let mut scene = Scene::new("Main", config.canvas_width, config.canvas_height);
        scene.add_item(SceneItem {
            source_id,
            transform: Transform::fullscreen(config.canvas_width, config.canvas_height),
            visible: true,
            z_order: 0,
        });
        let compositor = Compositor::new(scene);
        let fps = config.fps;

        // ── Compositor thread ─────────────────────────────────────────────────
        let record_tx_for_thread = record_tx_opt.clone();
        let compositor_thread = thread::Builder::new()
            .name("obs-compositor".into())
            .spawn(move || {
                compositor_loop(
                    capture_rx,
                    preview_tx,
                    record_tx_for_thread,
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

        // Keep the source alive for the lifetime of the process. We detach it
        // here; dropping the capture_tx from `stop()` will unblock its loop.
        thread::Builder::new()
            .name("obs-screen-src-guard".into())
            .spawn(move || drop(screen_src))
            .expect("Failed to spawn source guard");

        info!(
            "Pipeline started ({}×{} @ {} fps, recording: {})",
            config.canvas_width,
            config.canvas_height,
            fps,
            config.record_path.is_some()
        );

        let pipeline = Pipeline {
            compositor_thread: Some(compositor_thread),
            recorder_thread,
            capture_tx,
            record_tx: record_tx_opt,
        };
        (pipeline, preview_rx)
    }

    pub fn stop(mut self) {
        // Drop the capture tx so the compositor loop exits once its channel drains.
        drop(self.capture_tx);
        if let Some(t) = self.compositor_thread.take() {
            let _ = t.join();
        }
        // Drop the record tx so the recorder loop finalizes the MP4.
        drop(self.record_tx);
        if let Some(t) = self.recorder_thread.take() {
            let _ = t.join();
        }
        info!("Pipeline stopped");
    }
}

// ── Compositor loop ───────────────────────────────────────────────────────────

fn compositor_loop(
    capture_rx: Receiver<VideoFrame>,
    preview_tx: SyncSender<Arc<VideoFrame>>,
    record_tx: Option<SyncSender<VideoFrame>>,
    proxy: EventLoopProxy<PreviewEvent>,
    mut compositor: Compositor,
    source_id: SourceId,
    fps: u32,
) {
    let mut frame_number: u64 = 0;

    for raw_frame in &capture_rx {
        compositor.update_source(source_id, raw_frame);

        let pts = crate::types::Timestamp::from_frame(frame_number, fps);
        frame_number += 1;

        let composed = compositor.composite(pts);
        let composed_arc = Arc::new(composed.clone());

        // Forward to preview (drop if behind)
        match preview_tx.try_send(Arc::clone(&composed_arc)) {
            Ok(_) => {}
            Err(mpsc::TrySendError::Full(_)) => {}
            Err(mpsc::TrySendError::Disconnected(_)) => {
                info!("Preview channel disconnected — stopping compositor");
                break;
            }
        }

        // Forward to recorder (block to preserve every frame)
        if let Some(tx) = &record_tx {
            if let Err(e) = tx.send(composed) {
                warn!("Record channel send failed: {e} — stopping compositor");
                break;
            }
        }

        // Wake the winit event loop.
        if proxy.send_event(PreviewEvent::NewFrame).is_err() {
            info!("Event loop proxy disconnected — stopping compositor");
            break;
        }
    }

    info!("Compositor loop finished");
}

// ── Recorder loop ─────────────────────────────────────────────────────────────

fn recorder_loop(
    rx: Receiver<VideoFrame>,
    path: PathBuf,
    width: u32,
    height: u32,
    fps: u32,
    bitrate_kbps: Option<usize>,
) {
    let mut recorder = match FileRecorder::new(&path, width, height, fps, bitrate_kbps) {
        Ok(r) => r,
        Err(e) => {
            error!("Failed to create FileRecorder: {e}");
            // Drain the channel so the compositor doesn't block forever.
            for _ in rx.iter() {}
            return;
        }
    };

    for frame in &rx {
        if let Err(e) = recorder.push_frame(&frame) {
            warn!("push_frame failed: {e}");
        }
    }

    if let Err(e) = recorder.finalize() {
        error!("FileRecorder finalise failed: {e}");
    }
    info!("Recorder loop finished");
}
