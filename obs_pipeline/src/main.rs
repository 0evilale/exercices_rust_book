mod encoder;
mod output;
mod pipeline;
mod preview;
mod scene;
mod source;
mod types;

use std::path::PathBuf;

use pipeline::coordinator::{Pipeline, PipelineConfig};
use preview::renderer::{create_event_loop, PreviewApp};
use tracing::info;

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    info!("obs_pipeline starting up");

    // winit event loop (main thread).
    let (event_loop, proxy) = create_event_loop();

    // Parse very minimal CLI: `cargo run -- --record out.mp4`.
    let record_path = parse_record_path();

    let config = PipelineConfig {
        canvas_width: 1280,
        canvas_height: 720,
        fps: 30,
        record_path: record_path.clone(),
        record_bitrate_kbps: Some(4_000),
    };

    let (pipeline, preview_rx) = Pipeline::start(config, proxy);

    let title = match &record_path {
        Some(p) => format!("OBS Pipeline — recording → {} — Q to stop", p.display()),
        None => "OBS Pipeline — Q to quit".to_string(),
    };
    let mut app = PreviewApp::new(preview_rx, title);

    event_loop
        .run_app(&mut app)
        .expect("Event loop error");

    info!("Shutting down pipeline…");
    pipeline.stop();
    info!("Goodbye!");
}

fn parse_record_path() -> Option<PathBuf> {
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        if a == "--record" || a == "-r" {
            return args.next().map(PathBuf::from);
        }
    }
    None
}
