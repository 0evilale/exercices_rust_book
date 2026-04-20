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
use source::{screen::ScreenCaptureSource, test::TestSource, VideoSource};
use tracing::info;

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    info!("obs_pipeline starting up");

    let args = parse_args();

    let (event_loop, proxy) = create_event_loop();

    let config = PipelineConfig {
        canvas_width: 1280,
        canvas_height: 720,
        fps: 30,
        record_path: args.record_path.clone(),
        record_bitrate_kbps: Some(4_000),
    };

    // Select the video source based on --source flag.
    let source: Box<dyn VideoSource> = match args.source.as_deref() {
        Some("test") | Some("t") => {
            info!("Using TestSource (synthetic color bars)");
            Box::new(TestSource::new(config.canvas_width, config.canvas_height, config.fps))
        }
        _ => {
            info!("Using ScreenCaptureSource (falls back to test pattern on WSL2)");
            Box::new(ScreenCaptureSource::new(0, config.fps))
        }
    };

    let (pipeline, preview_rx) = Pipeline::start(config, source, proxy);

    let title = match &args.record_path {
        Some(p) => format!("OBS Pipeline — recording → {} — Q to stop", p.display()),
        None => "OBS Pipeline — Q to quit".to_string(),
    };
    let mut app = PreviewApp::new(preview_rx, title);

    event_loop.run_app(&mut app).expect("Event loop error");

    info!("Shutting down pipeline…");
    pipeline.stop();
    info!("Goodbye!");
}

// ── Argument parsing ──────────────────────────────────────────────────────────

struct Args {
    /// "screen" | "test"
    source: Option<String>,
    record_path: Option<PathBuf>,
}

fn parse_args() -> Args {
    let mut source = None;
    let mut record_path = None;
    let mut iter = std::env::args().skip(1);

    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--source" | "-s" => source = iter.next(),
            "--record" | "-r" => record_path = iter.next().map(PathBuf::from),
            _ => {}
        }
    }

    Args { source, record_path }
}
