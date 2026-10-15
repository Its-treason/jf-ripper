pub mod config;
pub mod error;

mod audio;
mod pipeline;
mod video;

pub use config::{
    AudioAction, AudioConfig, ChapterInfo, ContainerFormat, SubtitleConfig, VideoCodec,
    VideoConfig,
};
pub use error::TranscodeError;

/// Demuxer options for opening a raw title. Job creation and the transcoder
/// must use the same ones: DVD program streams have no header, so stream
/// indices depend on the order streams are found while probing, and the job's
/// `source_stream` indices are only valid under identical probing.
pub fn input_options() -> ffmpeg_next::Dictionary<'static> {
    let mut opts = ffmpeg_next::Dictionary::new();
    // DVD subpicture streams can start minutes into the program stream, so
    // probe far deeper than the defaults. Harmless for m2ts input.
    opts.set("probesize", "200000000");
    opts.set("analyzeduration", "120000000");
    // DVD MPEG-PS leaves PTS unset on many packets; have the demuxer generate
    // them from DTS (same as ffmpeg's -fflags +genpts). No effect on m2ts.
    opts.set("fflags", "+genpts");
    opts
}

pub struct TranscodeJob {
    input_path: String,
    output_path: String,
    container: ContainerFormat,
    video: VideoConfig,
    audio_tracks: Vec<AudioConfig>,
    subtitle_tracks: Vec<SubtitleConfig>,
    chapters: Vec<ChapterInfo>,
    metadata: Vec<(String, String)>,
}

impl TranscodeJob {
    pub fn new(input: impl Into<String>, output: impl Into<String>) -> Self {
        Self {
            input_path: input.into(),
            output_path: output.into(),
            container: ContainerFormat::Mkv,
            video: VideoConfig::default(),
            audio_tracks: Vec::new(),
            subtitle_tracks: Vec::new(),
            chapters: Vec::new(),
            metadata: Vec::new(),
        }
    }

    pub fn container(mut self, fmt: ContainerFormat) -> Self {
        self.container = fmt;
        self
    }

    pub fn video(mut self, cfg: VideoConfig) -> Self {
        self.video = cfg;
        self
    }

    pub fn add_audio(mut self, cfg: AudioConfig) -> Self {
        self.audio_tracks.push(cfg);
        self
    }

    pub fn add_subtitle(mut self, cfg: SubtitleConfig) -> Self {
        self.subtitle_tracks.push(cfg);
        self
    }

    pub fn chapters(mut self, chapters: Vec<ChapterInfo>) -> Self {
        self.chapters = chapters;
        self
    }

    pub fn metadata(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.metadata.push((key.into(), value.into()));
        self
    }

    pub fn run(self) -> Result<(), TranscodeError> {
        pipeline::run(
            &self.input_path,
            &self.output_path,
            self.container.format_name(),
            self.video,
            &self.audio_tracks,
            &self.subtitle_tracks,
            &self.chapters,
            &self.metadata,
        )
    }
}
