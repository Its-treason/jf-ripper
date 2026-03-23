use chrono::Utc;
use uuid::Uuid;

use crate::config::Config;
use crate::rip::analysis::analyse_disc;
use crate::rip::jellyfin;
use crate::rip::tui::{self, MediaChoice, MovieChoice, ShowChoice};
use crate::rip::{read_title_with_progress, resolve_stream_configs};
use crate::transcode::{ContainerFormat, VideoConfig};

use super::fs_ops::{self, SharedDirs};
use super::job::{ContentType, DistributedJob};

pub fn run_create_jobs(config: &Config) -> Result<(), Box<dyn std::error::Error>> {
    let shared_dir = config
        .distributed
        .shared_dir
        .as_deref()
        .ok_or("distributed.shared_dir not configured")?;
    let dirs = SharedDirs::new(shared_dir);
    dirs.ensure_dirs()?;

    let analysis = analyse_disc(&config.bd_path, &config.languages.player_language)?;
    let choice = tui::run_tui(&analysis, config)?;

    match choice {
        MediaChoice::Movie(m) => create_movie_job(&analysis, &m, config, &dirs),
        MediaChoice::Show(s) => create_show_jobs(&analysis, &s, config, &dirs),
    }
}

fn create_movie_job(
    analysis: &crate::rip::analysis::TitleAnalysis,
    choice: &MovieChoice,
    config: &Config,
    dirs: &SharedDirs,
) -> Result<(), Box<dyn std::error::Error>> {
    let title = analysis
        .titles
        .iter()
        .find(|t| t.index == choice.title_idx)
        .ok_or("Selected title not found")?;

    let id = Uuid::new_v4();
    let media_file = format!("{}.m2ts", id);
    let media_path = dirs.media.join(&media_file);

    println!("Reading title {} to shared media...", choice.title_idx);
    read_title_with_progress(
        choice.title_idx,
        &media_path.to_string_lossy(),
        &config.bd_path,
    )?;

    let video = VideoConfig::from_transcode_config(&config.transcode);
    let (audio_tracks, subtitle_tracks, chapters) = resolve_stream_configs(
        title,
        &media_path.to_string_lossy(),
        &choice.audio_selections,
        &choice.subtitle_indices,
        config,
    )?;

    // Build relative output path using Jellyfin naming
    let movie_dir = config
        .movie_dir
        .as_deref()
        .ok_or("movie_dir not configured")?;
    let full_path =
        jellyfin::movie_path(movie_dir, &choice.tmdb.title, choice.tmdb.year, choice.tmdb.id);
    let relative_path = full_path
        .strip_prefix(movie_dir)
        .unwrap_or(&full_path)
        .to_string_lossy()
        .into_owned();

    let job = DistributedJob {
        id,
        version: 1,
        media_file,
        disc_volume_id: None,
        content_type: ContentType::Movie,
        relative_output_path: relative_path,
        container: ContainerFormat::Mkv,
        video,
        audio_tracks,
        subtitle_tracks,
        chapters,
        metadata: vec![("title".to_string(), choice.tmdb.title.clone())],
        attempt: 0,
        max_retries: config.distributed.max_retries,
        created_at: Utc::now(),
        error_message: None,
    };

    fs_ops::write_job_atomic(&dirs.pending, &job)?;
    println!("Created job {} for '{}'", job.id, choice.tmdb.title);

    if config.eject_on_complete {
        let _ = std::process::Command::new("eject")
            .arg(&config.bd_path)
            .status();
    }

    Ok(())
}

fn create_show_jobs(
    analysis: &crate::rip::analysis::TitleAnalysis,
    choice: &ShowChoice,
    config: &Config,
    dirs: &SharedDirs,
) -> Result<(), Box<dyn std::error::Error>> {
    let show_dir = config
        .show_dir
        .as_deref()
        .ok_or("show_dir not configured")?;

    // Read ALL titles first, then create jobs (enables parallel worker pickup)
    let mut title_media: Vec<(u32, Uuid, String)> = Vec::new();

    for &title_idx in &choice.title_indices {
        let id = Uuid::new_v4();
        let media_file = format!("{}.m2ts", id);
        let media_path = dirs.media.join(&media_file);

        println!("Reading title {}...", title_idx);
        read_title_with_progress(title_idx, &media_path.to_string_lossy(), &config.bd_path)?;
        title_media.push((title_idx, id, media_file));
    }

    // Create job files
    for (i, (title_idx, id, media_file)) in title_media.iter().enumerate() {
        let episode_num = choice.start_episode + i as u32;

        let title = analysis
            .titles
            .iter()
            .find(|t| t.index == *title_idx)
            .ok_or_else(|| format!("Title {} not found", title_idx))?;

        let media_path = dirs.media.join(media_file);
        let video = VideoConfig::from_transcode_config(&config.transcode);
        let (audio_tracks, subtitle_tracks, chapters) = resolve_stream_configs(
            title,
            &media_path.to_string_lossy(),
            &choice.audio_selections,
            &choice.subtitle_indices,
            config,
        )?;

        let full_path = jellyfin::episode_path(
            show_dir,
            &choice.tmdb.name,
            choice.tmdb.year,
            choice.tmdb.id,
            choice.season,
            episode_num,
        );
        let relative_path = full_path
            .strip_prefix(show_dir)
            .unwrap_or(&full_path)
            .to_string_lossy()
            .into_owned();

        let job = DistributedJob {
            id: *id,
            version: 1,
            media_file: media_file.clone(),
            disc_volume_id: None,
            content_type: ContentType::Episode,
            relative_output_path: relative_path,
            container: ContainerFormat::Mkv,
            video,
            audio_tracks,
            subtitle_tracks,
            chapters,
            metadata: vec![
                ("show".to_string(), choice.tmdb.name.clone()),
                ("season".to_string(), choice.season.to_string()),
                ("episode".to_string(), episode_num.to_string()),
            ],
            attempt: 0,
            max_retries: config.distributed.max_retries,
            created_at: Utc::now(),
            error_message: None,
        };

        fs_ops::write_job_atomic(&dirs.pending, &job)?;
        println!(
            "Created job {} for S{:02}E{:02}",
            job.id, choice.season, episode_num
        );
    }

    if config.eject_on_complete {
        let _ = std::process::Command::new("eject")
            .arg(&config.bd_path)
            .status();
    }

    Ok(())
}
