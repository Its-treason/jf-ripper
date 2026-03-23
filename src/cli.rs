use clap::{Parser, Subcommand};
use std::path::PathBuf;

use crate::bluray::{disc_info, list_titles, read_title};
use crate::config::Config;
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
    /// Manage configuration
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },
    /// Rip a Blu-ray disc interactively
    Rip {
        /// Config file path
        #[arg(long)]
        config: Option<PathBuf>,
        /// Override Blu-ray device path
        #[arg(long)]
        bd_path: Option<String>,
        /// Override video codec
        #[arg(long)]
        video_codec: Option<String>,
        /// Override CRF value
        #[arg(long)]
        crf: Option<u32>,
        /// Override encoder preset
        #[arg(long)]
        preset: Option<String>,
        /// Don't eject disc after completion
        #[arg(long)]
        no_eject: bool,
    },
    /// Create distributed transcoding jobs from a Blu-ray disc
    CreateJobs {
        /// Config file path
        #[arg(long)]
        config: Option<PathBuf>,
        /// Override Blu-ray device path
        #[arg(long)]
        bd_path: Option<String>,
    },
    /// Run as a distributed transcoding worker
    Worker {
        /// Config file path
        #[arg(long)]
        config: Option<PathBuf>,
    },
    /// Manage distributed transcoding jobs
    Jobs {
        #[command(subcommand)]
        action: JobsAction,
    },
}

#[derive(Subcommand)]
enum ConfigAction {
    /// Interactively create a config file
    Init {
        #[arg(long)]
        config: Option<PathBuf>,
    },
    /// Show the current configuration
    Show {
        #[arg(long)]
        config: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
enum JobsAction {
    /// List all jobs and their status
    List {
        #[arg(long)]
        config: Option<PathBuf>,
    },
    /// Retry all failed jobs
    Retry {
        #[arg(long)]
        config: Option<PathBuf>,
    },
    /// Clean up completed jobs
    Clean {
        #[arg(long)]
        config: Option<PathBuf>,
    },
    /// Recover jobs with stale heartbeats
    RecoverStale {
        #[arg(long)]
        config: Option<PathBuf>,
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
                    forced: false,
                });
            }

            job.run().map_err(Into::into)
        }
        Commands::Rip {
            config: config_path,
            bd_path,
            video_codec,
            crf,
            preset,
            no_eject,
        } => {
            let mut cfg = Config::load_or_default(config_path.as_deref());
            if let Some(p) = bd_path {
                cfg.bd_path = p.clone();
            }
            if let Some(vc) = video_codec {
                cfg.transcode.video_codec = vc.clone();
            }
            if let Some(c) = crf {
                cfg.transcode.crf = Some(*c);
            }
            if let Some(p) = preset {
                cfg.transcode.preset = Some(p.clone());
            }
            if *no_eject {
                cfg.eject_on_complete = false;
            }
            crate::rip::run_rip(&cfg).map_err(Into::into)
        }
        Commands::CreateJobs {
            config: config_path,
            bd_path,
        } => {
            let mut cfg = Config::load_or_default(config_path.as_deref());
            if let Some(p) = bd_path {
                cfg.bd_path = p.clone();
            }
            crate::distributed::reader::run_create_jobs(&cfg).map_err(Into::into)
        }
        Commands::Worker { config: config_path } => {
            let cfg = Config::load_or_default(config_path.as_deref());
            crate::distributed::worker::run_worker(&cfg).map_err(Into::into)
        }
        Commands::Jobs { action } => match action {
            JobsAction::List { config: config_path } => {
                let cfg = Config::load_or_default(config_path.as_deref());
                crate::distributed::manager::list_jobs(&cfg)
            }
            JobsAction::Retry { config: config_path } => {
                let cfg = Config::load_or_default(config_path.as_deref());
                crate::distributed::manager::retry_jobs(&cfg)
            }
            JobsAction::Clean { config: config_path } => {
                let cfg = Config::load_or_default(config_path.as_deref());
                crate::distributed::manager::clean_jobs(&cfg)
            }
            JobsAction::RecoverStale { config: config_path } => {
                let cfg = Config::load_or_default(config_path.as_deref());
                crate::distributed::manager::recover_stale(&cfg)
            }
        },
        Commands::Config { action } => match action {
            ConfigAction::Init { config: config_path } => {
                config_init(config_path.as_deref()).map_err(Into::into)
            }
            ConfigAction::Show { config: config_path } => {
                config_show(config_path.as_deref());
                Ok(())
            }
        },
    };

    if let Err(e) = result {
        eprintln!("Error: {}", e);
        std::process::exit(1);
    }
}

fn config_init(config_path: Option<&std::path::Path>) -> Result<(), Box<dyn std::error::Error>> {
    use dialoguer::{Confirm, Input};

    let path = config_path
        .map(PathBuf::from)
        .or_else(Config::default_path)
        .ok_or("Could not determine config path")?;

    if path.exists() {
        let overwrite = Confirm::new()
            .with_prompt(format!("Config already exists at {}. Overwrite?", path.display()))
            .default(false)
            .interact()?;
        if !overwrite {
            println!("Aborted.");
            return Ok(());
        }
    }

    let movie_dir: String = Input::new()
        .with_prompt("Movie directory (Jellyfin)")
        .allow_empty(true)
        .interact_text()?;

    let show_dir: String = Input::new()
        .with_prompt("Show directory (Jellyfin)")
        .allow_empty(true)
        .interact_text()?;

    let tmdb_token: String = Input::new()
        .with_prompt("TMDB bearer token")
        .allow_empty(true)
        .interact_text()?;

    let bd_path: String = Input::new()
        .with_prompt("Blu-ray device path")
        .default("/dev/sr0".into())
        .interact_text()?;

    let video_codec: String = Input::new()
        .with_prompt("Default video codec (copy/h264/h265/av1)")
        .default("h265".into())
        .interact_text()?;

    let crf_str: String = Input::new()
        .with_prompt("Default CRF (empty for encoder default)")
        .allow_empty(true)
        .interact_text()?;
    let crf = crf_str.parse::<u32>().ok();

    let preset: String = Input::new()
        .with_prompt("Default preset (e.g. slow, medium, fast)")
        .default("slow".into())
        .interact_text()?;

    let audio_langs: String = Input::new()
        .with_prompt("Preferred audio languages (comma-separated, e.g. eng,deu)")
        .allow_empty(true)
        .interact_text()?;

    let subtitle_langs: String = Input::new()
        .with_prompt("Preferred subtitle languages (comma-separated, e.g. eng,deu)")
        .allow_empty(true)
        .interact_text()?;

    let player_language: String = Input::new()
        .with_prompt("Player language for disc menus/defaults (ISO 639-2, e.g. eng)")
        .default("eng".into())
        .interact_text()?;

    let config = Config {
        movie_dir: if movie_dir.is_empty() { None } else { Some(movie_dir) },
        show_dir: if show_dir.is_empty() { None } else { Some(show_dir) },
        tmdb_token: if tmdb_token.is_empty() { None } else { Some(tmdb_token) },
        bd_path,
        temp_dir: "/var/tmp".into(),
        eject_on_complete: false,
        transcode: crate::config::TranscodeConfig {
            video_codec,
            crf,
            preset: Some(preset),
            audio_bitrate: None,
        },
        languages: crate::config::LanguageConfig {
            audio: parse_comma_list(&audio_langs),
            subtitle: parse_comma_list(&subtitle_langs),
            player_language,
        },
        distributed: crate::config::DistributedConfig::default(),
    };

    config.save(&path)?;
    println!("Config written to {}", path.display());
    Ok(())
}

fn config_show(config_path: Option<&std::path::Path>) {
    let config = Config::load_or_default(config_path);
    match toml::to_string_pretty(&config) {
        Ok(s) => println!("{}", s),
        Err(e) => eprintln!("Error serializing config: {}", e),
    }
}

fn parse_comma_list(s: &str) -> Vec<String> {
    s.split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}
