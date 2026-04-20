pub mod screen;

use std::sync::mpsc::SyncSender;

use crate::types::{AudioBuffer, VideoFrame};

// ── Error type ────────────────────────────────────────────────────────────────

#[derive(Debug, thiserror::Error)]
pub enum SourceError {
    #[error("Failed to initialise capture: {0}")]
    Init(String),
    #[error("Capture frame error: {0}")]
    Capture(String),
    #[error("Source already running")]
    AlreadyRunning,
}

// ── Traits ────────────────────────────────────────────────────────────────────

/// A source that produces decoded video frames on a background thread.
///
/// Implementors spawn an OS thread in `start()` and push `VideoFrame`s via the
/// provided `SyncSender`.  The bounded channel naturally applies backpressure:
/// if the compositor falls behind the capture source will block until there is
/// room in the channel buffer.
pub trait VideoSource: Send + 'static {
    fn name(&self) -> &str;

    /// Start capturing and send frames to `tx`. Returns immediately; the actual
    /// capture loop runs on a newly spawned thread.
    fn start(&mut self, tx: SyncSender<VideoFrame>) -> Result<(), SourceError>;

    /// Signal the capture thread to stop.  May block briefly.
    fn stop(&mut self);

    /// Resolution that this source natively captures at.
    fn native_resolution(&self) -> (u32, u32);
}

/// A source that produces PCM audio buffers on a background thread.
pub trait AudioSource: Send + 'static {
    fn name(&self) -> &str;
    fn start(&mut self, tx: SyncSender<AudioBuffer>) -> Result<(), SourceError>;
    fn stop(&mut self);
    fn sample_rate(&self) -> u32;
    fn channels(&self) -> u8;
}
