use std::fmt;

#[derive(Debug)]
pub enum TranscodeError {
    Ffmpeg(ffmpeg_next::Error),
    EncoderNotFound(String),
    DecoderNotFound(String),
    InvalidConfig(String),
    Io(std::io::Error),
}

impl fmt::Display for TranscodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Ffmpeg(e) => write!(f, "FFmpeg error: {}", e),
            Self::EncoderNotFound(s) => write!(f, "Encoder not found: {}", s),
            Self::DecoderNotFound(s) => write!(f, "Decoder not found: {}", s),
            Self::InvalidConfig(s) => write!(f, "Invalid config: {}", s),
            Self::Io(e) => write!(f, "IO error: {}", e),
        }
    }
}

impl std::error::Error for TranscodeError {}

impl From<ffmpeg_next::Error> for TranscodeError {
    fn from(e: ffmpeg_next::Error) -> Self {
        Self::Ffmpeg(e)
    }
}

impl From<std::io::Error> for TranscodeError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}
