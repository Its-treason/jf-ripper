pub mod analysis;
pub mod jellyfin;
pub mod tui;

use std::fs;
use std::path::Path;

use indicatif::{ProgressBar, ProgressStyle};

use crate::bluray::read_title;
use crate::config::Config;
use crate::transcode::{
    AudioAction, AudioConfig, ChapterInfo, SubtitleConfig, TranscodeJob, VideoCodec, VideoConfig,
};

use self::analysis::{AnalysedTitle, TitleAnalysis};
use self::tui::{AudioSelection, AudioSelectionAction, MediaChoice, MovieChoice, ShowChoice};

/// Probe an m2ts file to find the actual ffmpeg stream indices for each type.
/// Stores the MPEG-TS PID (stream id) so we can reliably match ffmpeg streams
/// to libbluray streams — sequential ordering breaks when ffmpeg splits
/// TrueHD+AC3-core into two separate streams.
struct ProbedStreams {
    audio: Vec<ProbedAudioStream>,
    subtitle: Vec<ProbedSubtitleStream>,
}

struct ProbedAudioStream {
    ffmpeg_index: usize,
    /// MPEG-TS PID (from ffmpeg stream id)
    pid: u16,
}

struct ProbedSubtitleStream {
    ffmpeg_index: usize,
    pid: u16,
    forced: bool,
}

fn probe_input_streams(input_path: &str) -> Result<ProbedStreams, Box<dyn std::error::Error>> {
    ffmpeg_next::init()?;
    let ictx = ffmpeg_next::format::input(&input_path)?;

    let mut audio = Vec::new();
    let mut subtitle = Vec::new();

    for stream in ictx.streams() {
        let pid = unsafe { (*stream.as_ptr()).id as u16 };
        match stream.parameters().medium() {
            ffmpeg_next::media::Type::Audio => {
                audio.push(ProbedAudioStream {
                    ffmpeg_index: stream.index(),
                    pid,
                });
            }
            ffmpeg_next::media::Type::Subtitle => {
                let disposition = unsafe { (*stream.as_ptr()).disposition };
                let forced = (disposition & ffmpeg_next::ffi::AV_DISPOSITION_FORCED as i32) != 0;
                subtitle.push(ProbedSubtitleStream {
                    ffmpeg_index: stream.index(),
                    pid,
                    forced,
                });
            }
            _ => {}
        }
    }

    Ok(ProbedStreams { audio, subtitle })
}

pub fn run_rip(config: &Config) -> Result<(), Box<dyn std::error::Error>> {
    let analysis = analysis::analyse_disc(&config.bd_path, &config.languages.player_language)?;
    let choice = tui::run_tui(&analysis, config)?;

    match choice {
        MediaChoice::Movie(m) => rip_movie(&analysis, &m, config),
        MediaChoice::Show(s) => rip_show(&analysis, &s, config),
    }
}

fn rip_movie(
    analysis: &TitleAnalysis,
    choice: &MovieChoice,
    config: &Config,
) -> Result<(), Box<dyn std::error::Error>> {
    let movie_dir = config
        .movie_dir
        .as_deref()
        .ok_or("movie_dir not configured")?;

    let output_path =
        jellyfin::movie_path(movie_dir, &choice.tmdb.title, choice.tmdb.year, choice.tmdb.id);

    let title = analysis
        .titles
        .iter()
        .find(|t| t.index == choice.title_idx)
        .ok_or("Selected title not found")?;

    println!("\nRipping: {} -> {}", choice.tmdb, output_path.display());

    let temp_m2ts = format!("{}/rip_{}.m2ts", config.temp_dir, choice.title_idx);
    read_title_with_progress(choice.title_idx, &temp_m2ts, &config.bd_path)?;

    transcode_title(title, &temp_m2ts, &output_path, choice, config)?;

    // Cleanup temp file
    let _ = fs::remove_file(&temp_m2ts);

    println!("Done: {}", output_path.display());

    if config.eject_on_complete {
        let _ = std::process::Command::new("eject")
            .arg(&config.bd_path)
            .status();
    }

    Ok(())
}

fn rip_show(
    analysis: &TitleAnalysis,
    choice: &ShowChoice,
    config: &Config,
) -> Result<(), Box<dyn std::error::Error>> {
    let show_dir = config
        .show_dir
        .as_deref()
        .ok_or("show_dir not configured")?;

    for (i, &title_idx) in choice.title_indices.iter().enumerate() {
        let episode_num = choice.start_episode + i as u32;
        let output_path = jellyfin::episode_path(
            show_dir,
            &choice.tmdb.name,
            choice.tmdb.year,
            choice.tmdb.id,
            choice.season,
            episode_num,
        );

        let title = analysis
            .titles
            .iter()
            .find(|t| t.index == title_idx)
            .ok_or_else(|| format!("Title {} not found", title_idx))?;

        println!(
            "\nRipping episode {} (title {}) -> {}",
            episode_num,
            title_idx,
            output_path.display()
        );

        let temp_m2ts = format!("{}/rip_{}.m2ts", config.temp_dir, title_idx);
        read_title_with_progress(title_idx, &temp_m2ts, &config.bd_path)?;

        transcode_title_show(title, &temp_m2ts, &output_path, choice, config)?;

        let _ = fs::remove_file(&temp_m2ts);

        println!("Done: {}", output_path.display());
    }

    if config.eject_on_complete {
        let _ = std::process::Command::new("eject")
            .arg(&config.bd_path)
            .status();
    }

    Ok(())
}

fn read_title_with_progress(
    title_idx: u32,
    out_path: &str,
    bd_path: &str,
) -> Result<u64, Box<dyn std::error::Error>> {
    let pb = ProgressBar::new_spinner();
    pb.set_style(
        ProgressStyle::with_template("{spinner:.green} Reading title... {msg}")
            .unwrap(),
    );
    pb.set_message("starting");

    let bytes = read_title(title_idx, out_path, bd_path)?;

    pb.finish_with_message(format!(
        "read {} MB",
        bytes / 1_000_000
    ));

    Ok(bytes)
}

fn format_coding_type(ct: u8) -> &'static str {
    match ct {
        0x01 => "MPEG-1",
        0x02 => "MPEG-2",
        0x03 => "MPEG-2 Audio",
        0x80 => "LPCM",
        0x81 => "AC-3",
        0x82 => "DTS",
        0x83 => "TrueHD",
        0x84 => "EAC-3",
        0x85 => "DTS-HD",
        0x86 => "DTS-HD MA",
        0xA1 => "EAC-3 2nd",
        0xA2 => "DTS-HD 2nd",
        _ => "Unknown",
    }
}

/// Convert ISO 639-2/B language code to its English name.
fn iso639_to_name(code: &str) -> &str {
    match code {
        "eng" => "English",
        "deu" | "ger" => "German",
        "fra" | "fre" => "French",
        "spa" => "Spanish",
        "ita" => "Italian",
        "por" => "Portuguese",
        "rus" => "Russian",
        "jpn" => "Japanese",
        "kor" => "Korean",
        "zho" | "chi" => "Chinese",
        "ara" => "Arabic",
        "hin" => "Hindi",
        "nld" | "dut" => "Dutch",
        "pol" => "Polish",
        "swe" => "Swedish",
        "nor" => "Norwegian",
        "dan" => "Danish",
        "fin" => "Finnish",
        "ces" | "cze" => "Czech",
        "hun" => "Hungarian",
        "tur" => "Turkish",
        "tha" => "Thai",
        "vie" => "Vietnamese",
        "ron" | "rum" => "Romanian",
        "ell" | "gre" => "Greek",
        "heb" => "Hebrew",
        "cat" => "Catalan",
        "bul" => "Bulgarian",
        "hrv" => "Croatian",
        "slk" | "slo" => "Slovak",
        "slv" => "Slovenian",
        "srp" => "Serbian",
        "ukr" => "Ukrainian",
        "ind" => "Indonesian",
        "msa" | "may" => "Malay",
        "und" => "Unknown",
        other => other,
    }
}

fn build_transcode_job(
    title: &AnalysedTitle,
    input_path: &str,
    output_path: &Path,
    audio_selections: &[AudioSelection],
    subtitle_indices: &[usize],
    config: &Config,
) -> Result<TranscodeJob, Box<dyn std::error::Error>> {
    // Ensure output directory exists
    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent)?;
    }

    // Probe the actual m2ts file to get correct stream indices and languages
    let probed = probe_input_streams(input_path)?;

    let video_codec = match config.transcode.video_codec.as_str() {
        "h264" => VideoCodec::H264,
        "h265" => VideoCodec::H265,
        "av1" => VideoCodec::Av1,
        _ => VideoCodec::Copy,
    };

    let mut job = TranscodeJob::new(input_path, output_path.to_string_lossy())
        .video(VideoConfig {
            codec: video_codec,
            crf: config.transcode.crf,
            preset: config.transcode.preset.clone(),
            ..Default::default()
        });

    // Match libbluray streams to ffmpeg streams by PID.
    // Sequential index matching breaks when ffmpeg splits TrueHD+AC3-core
    // into two streams while libbluray counts them as one.
    for sel in audio_selections {
        let bd_stream = title
            .audio_streams
            .iter()
            .find(|a| a.index_in_clip == sel.stream_index_in_clip)
            .ok_or_else(|| {
                format!("Audio stream {} not in libbluray analysis", sel.stream_index_in_clip)
            })?;

        // Find ffmpeg stream by PID; take the first match (primary codec, not AC3 core)
        let probed_stream = probed
            .audio
            .iter()
            .find(|p| p.pid == bd_stream.pid)
            .ok_or_else(|| {
                format!(
                    "Audio PID 0x{:04x} (libbluray stream {}) not found in ffmpeg probe",
                    bd_stream.pid, sel.stream_index_in_clip
                )
            })?;

        let lang = &bd_stream.language;
        let codec_label = match sel.action {
            AudioSelectionAction::Copy => format_coding_type(bd_stream.coding_type).to_string(),
            AudioSelectionAction::EncodeAac => "AAC".to_string(),
        };
        let display_name = format!("{} ({})", iso639_to_name(lang), codec_label);

        let action = match sel.action {
            AudioSelectionAction::Copy => AudioAction::Copy,
            AudioSelectionAction::EncodeAac => AudioAction::Encode {
                codec_name: "aac".into(),
                bitrate: config.transcode.audio_bitrate,
                extra_options: Vec::new(),
            },
        };

        job = job.add_audio(AudioConfig {
            source_stream: probed_stream.ffmpeg_index,
            language: Some(lang.clone()),
            name: Some(display_name),
            action,
        });
    }

    // Subtitle streams: match by PID, use libbluray language
    for &sub_clip_idx in subtitle_indices {
        let bd_stream = title
            .subtitle_streams
            .iter()
            .find(|s| s.index_in_clip == sub_clip_idx)
            .ok_or_else(|| {
                format!("Subtitle stream {} not in libbluray analysis", sub_clip_idx)
            })?;

        let probed_sub = probed
            .subtitle
            .iter()
            .find(|p| p.pid == bd_stream.pid)
            .ok_or_else(|| {
                format!(
                    "Subtitle PID 0x{:04x} (libbluray stream {}) not found in ffmpeg probe",
                    bd_stream.pid, sub_clip_idx
                )
            })?;

        job = job.add_subtitle(SubtitleConfig {
            source_stream: probed_sub.ffmpeg_index,
            language: Some(bd_stream.language.clone()),
            name: Some(iso639_to_name(&bd_stream.language).to_string()),
            forced: probed_sub.forced,
        });
    }

    // Chapters
    let chapters: Vec<ChapterInfo> = title
        .chapters
        .iter()
        .map(|ch| {
            let start_ms = (ch.start_ticks / 90) as i64;
            let end_ms = start_ms + (ch.duration_ticks / 90) as i64;
            ChapterInfo {
                id: ch.index as i64 + 1,
                title: format!("Chapter {}", ch.index + 1),
                start_ms,
                end_ms,
            }
        })
        .collect();

    if !chapters.is_empty() {
        job = job.chapters(chapters);
    }

    Ok(job)
}

fn transcode_title(
    title: &AnalysedTitle,
    input_path: &str,
    output_path: &Path,
    choice: &MovieChoice,
    config: &Config,
) -> Result<(), Box<dyn std::error::Error>> {
    println!("Transcoding...");
    let job = build_transcode_job(
        title,
        input_path,
        output_path,
        &choice.audio_selections,
        &choice.subtitle_indices,
        config,
    )?;
    job.run()?;
    Ok(())
}

fn transcode_title_show(
    title: &AnalysedTitle,
    input_path: &str,
    output_path: &Path,
    choice: &ShowChoice,
    config: &Config,
) -> Result<(), Box<dyn std::error::Error>> {
    println!("Transcoding...");
    let job = build_transcode_job(
        title,
        input_path,
        output_path,
        &choice.audio_selections,
        &choice.subtitle_indices,
        config,
    )?;
    job.run()?;
    Ok(())
}
