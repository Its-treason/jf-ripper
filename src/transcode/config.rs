use serde::{Deserialize, Serialize};

/// Video codec to use when re-encoding
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum VideoCodec {
    H264,   // libx264
    H265,   // libx265
    Av1,    // libsvtav1
    Copy,
}

impl VideoCodec {
    pub fn encoder_name(&self) -> Option<&'static str> {
        match self {
            Self::H264 => Some("libx264"),
            Self::H265 => Some("libx265"),
            Self::Av1 => Some("libsvtav1"),
            Self::Copy => None,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VideoConfig {
    pub codec: VideoCodec,
    /// Constant Rate Factor (lower = better quality, larger file)
    pub crf: Option<u32>,
    /// Encoder preset ("ultrafast", "slow", "veryslow", etc.)
    pub preset: Option<String>,
    /// Additional codec-private AVOption key/value pairs
    pub extra_options: Vec<(String, String)>,
    /// Override source stream index; None = auto-detect best video stream
    pub source_stream: Option<usize>,
}

impl Default for VideoConfig {
    fn default() -> Self {
        Self {
            codec: VideoCodec::Copy,
            crf: None,
            preset: None,
            extra_options: Vec::new(),
            source_stream: None,
        }
    }
}

impl VideoConfig {
    pub fn from_transcode_config(tc: &crate::config::TranscodeConfig) -> Self {
        let codec = match tc.video_codec.as_str() {
            "h264" => VideoCodec::H264,
            "h265" => VideoCodec::H265,
            "av1" => VideoCodec::Av1,
            _ => VideoCodec::Copy,
        };
        Self {
            codec,
            crf: tc.crf,
            preset: tc.preset.clone(),
            extra_options: Vec::new(),
            source_stream: None,
        }
    }
}

/// What to do with an audio track
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum AudioAction {
    Copy,
    Encode {
        /// Codec name passed to ffmpeg encoder ("aac", "libopus", "flac", etc.)
        codec_name: String,
        /// Bitrate in bits/sec; None = encoder default
        bitrate: Option<u64>,
        /// Additional codec-private AVOption key/value pairs
        extra_options: Vec<(String, String)>,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AudioConfig {
    /// Input stream index
    pub source_stream: usize,
    /// ISO 639-2 language tag written to output stream metadata
    pub language: Option<String>,
    /// Human-readable track name (e.g. "Director's Commentary")
    pub name: Option<String>,
    pub action: AudioAction,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SubtitleConfig {
    /// Input stream index
    pub source_stream: usize,
    /// ISO 639-2 language tag written to output stream metadata
    pub language: Option<String>,
    /// Human-readable track name (e.g. "Forced", "Full")
    pub name: Option<String>,
    /// Whether this subtitle stream has the forced disposition
    pub forced: bool,
}

/// A chapter mark to write into the output container
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChapterInfo {
    /// Unique numeric ID (1-based)
    pub id: i64,
    /// Human-readable title
    pub title: String,
    /// Start time in milliseconds
    pub start_ms: i64,
    /// End time in milliseconds
    pub end_ms: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ContainerFormat {
    Mkv,
    Mp4,
}

impl ContainerFormat {
    pub fn format_name(&self) -> &'static str {
        match self {
            Self::Mkv => "matroska",
            Self::Mp4 => "mp4",
        }
    }
}
