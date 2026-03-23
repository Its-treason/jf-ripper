use std::path::Path;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::transcode::{
    AudioConfig, ChapterInfo, ContainerFormat, SubtitleConfig, TranscodeJob, VideoConfig,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ContentType {
    Movie,
    Episode,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DistributedJob {
    pub id: Uuid,
    pub version: u32,
    pub media_file: String,
    pub disc_volume_id: Option<String>,
    pub content_type: ContentType,
    pub relative_output_path: String,
    pub container: ContainerFormat,
    pub video: VideoConfig,
    pub audio_tracks: Vec<AudioConfig>,
    pub subtitle_tracks: Vec<SubtitleConfig>,
    pub chapters: Vec<ChapterInfo>,
    pub metadata: Vec<(String, String)>,
    pub attempt: u32,
    pub max_retries: u32,
    pub created_at: DateTime<Utc>,
    pub error_message: Option<String>,
}

impl DistributedJob {
    pub fn to_transcode_job(&self, shared_dir: &Path, output_base: &str) -> TranscodeJob {
        let input_path = shared_dir.join("media").join(&self.media_file);
        let output_path = Path::new(output_base).join(&self.relative_output_path);

        let mut job = TranscodeJob::new(
            input_path.to_string_lossy(),
            output_path.to_string_lossy(),
        )
        .container(self.container.clone())
        .video(self.video.clone());

        for audio in &self.audio_tracks {
            job = job.add_audio(audio.clone());
        }
        for sub in &self.subtitle_tracks {
            job = job.add_subtitle(sub.clone());
        }
        if !self.chapters.is_empty() {
            job = job.chapters(self.chapters.clone());
        }
        for (key, value) in &self.metadata {
            job = job.metadata(key, value);
        }

        job
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Heartbeat {
    pub worker_name: String,
    pub timestamp: DateTime<Utc>,
    pub pid: u32,
}
