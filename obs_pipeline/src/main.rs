mod pipeline;
mod preview;
mod scene;
mod source;
mod types;

use pipeline::coordinator::{Pipeline, PipelineConfig};
use preview::renderer::{PreviewApp, create_event_loop};
use tracing::info;


fn main() {
    // ── Logging ───────────────────────────────────────────────────────────────
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    info!("obs_pipeline starting up");

    // ── winit event loop (must be created on the main thread) ─────────────────
    let (event_loop, proxy) = create_event_loop();

    // ── Pipeline config ───────────────────────────────────────────────────────
    let config = PipelineConfig {
        canvas_width: 1920,
        canvas_height: 1080,
        fps: 30,
    };

    // ── Start pipeline threads ────────────────────────────────────────────────
    let (pipeline, preview_rx) = Pipeline::start(config, proxy);

    // ── Preview application ───────────────────────────────────────────────────
    let mut app = PreviewApp::new(preview_rx, "OBS Pipeline (Rust) — Press Q to quit");

    // ── Run event loop on main thread (blocks until window is closed) ─────────
    event_loop
        .run_app(&mut app)
        .expect("Event loop error");

    // ── Shutdown ──────────────────────────────────────────────────────────────
    info!("Shutting down pipeline…");
    pipeline.stop();
    info!("Goodbye!");
}
