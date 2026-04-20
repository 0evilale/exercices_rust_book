pub mod file_output;

#[derive(Debug, thiserror::Error)]
pub enum OutputError {
    #[error("FFmpeg error: {0}")]
    Ffmpeg(#[from] ffmpeg_next::Error),
    #[error("Encode error: {0}")]
    Encode(#[from] crate::encoder::EncodeError),
    #[error("Output setup failed: {0}")]
    Setup(String),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}
