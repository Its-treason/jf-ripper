use clap::{Parser, Subcommand};

use crate::bluray::{disc_info, list_titles, read_title};
use crate::transcode::{
    AudioAction, AudioConfig, ContainerFormat, SubtitleConfig, TranscodeJob, VideoCodec,
    VideoConfig,
};

#[derive(Parser)]
#[command(version, about, long_about = None)]
pub struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Print information about a Blu-Ray disc
    DiscInfo {
        #[arg(long, default_value = "/dev/sr0")]
        bd_path: String,
    },
    /// List titles available on a Blu-Ray disc
    ListTitles {
        #[arg(long, default_value = "/dev/sr0")]
        bd_path: String,
    },
    /// Read a raw Blu-Ray title to a file
    ReadTitle {
        #[arg(long)]
        title: u32,
        #[arg(long)]
        out_path: String,
        #[arg(long, default_value = "/dev/sr0")]
        bd_path: String,
    },
    /// Transcode an input file to MKV or MP4
    Transcode {
        /// Input file (e.g. output.m2ts from read-title)
        #[arg(long)]
        input: String,
        /// Output file (.mkv or .mp4)
        #[arg(long)]
        output: String,
        /// Video codec: copy, h264, h265, av1
        #[arg(long, default_value = "copy")]
        video_codec: String,
        /// CRF value for video encoding (e.g. 20)
        #[arg(long)]
        crf: Option<u32>,
        /// Encoder preset (e.g. slow, medium, fast)
        #[arg(long)]
        preset: Option<String>,
        /// Audio stream indices to copy (e.g. --audio-copy 1 --audio-copy 2)
        #[arg(long = "audio-copy")]
        audio_copy: Vec<usize>,
        /// Audio stream indices to encode as AAC (e.g. --audio-aac 1)
        #[arg(long = "audio-aac")]
        audio_aac: Vec<usize>,
        /// Subtitle stream indices to copy
        #[arg(long = "subtitle")]
        subtitles: Vec<usize>,
    },
}

pub fn execute_cli() {
    let cli = Cli::parse();

    let result: Result<(), Box<dyn std::error::Error>> = match &cli.command {
        Commands::DiscInfo { bd_path } => disc_info(bd_path).map_err(Into::into),
        Commands::ListTitles { bd_path } => list_titles(bd_path).map_err(Into::into),
        Commands::ReadTitle { title, out_path, bd_path } => read_title(*title, out_path, bd_path)
            .map(|bytes| println!("Read {} bytes", bytes))
            .map_err(Into::into),
        Commands::Transcode {
            input,
            output,
            video_codec,
            crf,
            preset,
            audio_copy,
            audio_aac,
            subtitles,
        } => {
            let codec = match video_codec.as_str() {
                "h264" => VideoCodec::H264,
                "h265" => VideoCodec::H265,
                "av1" => VideoCodec::Av1,
                _ => VideoCodec::Copy,
            };

            let container = if output.ends_with(".mp4") {
                ContainerFormat::Mp4
            } else {
                ContainerFormat::Mkv
            };

            let mut job = TranscodeJob::new(input, output)
                .container(container)
                .video(VideoConfig {
                    codec,
                    crf: *crf,
                    preset: preset.clone(),
                    ..Default::default()
                });

            for &src in audio_copy {
                job = job.add_audio(AudioConfig {
                    source_stream: src,
                    language: None,
                    name: None,
                    action: AudioAction::Copy,
                });
            }
            for &src in audio_aac {
                job = job.add_audio(AudioConfig {
                    source_stream: src,
                    language: None,
                    name: None,
                    action: AudioAction::Encode {
                        codec_name: "aac".into(),
                        bitrate: None,
                        extra_options: Vec::new(),
                    },
                });
            }
            for &src in subtitles {
                job = job.add_subtitle(SubtitleConfig {
                    source_stream: src,
                    language: None,
                    name: None,
                });
            }

            job.run().map_err(Into::into)
        }
    };

    if let Err(e) = result {
        eprintln!("Error: {}", e);
        std::process::exit(1);
    }
}
