use dialoguer::{FuzzySelect, Input, MultiSelect, Select};

use crate::config::Config;
use crate::tmdb::{self, TmdbClient, TmdbMovie, TmdbShow, TmdbUrlType};

use super::analysis::{AudioStreamInfo, DiscType, SubtitleStreamInfo, TitleAnalysis};

pub enum MediaChoice {
    Movie(MovieChoice),
    Show(ShowChoice),
}

pub struct MovieChoice {
    pub title_idx: u32,
    pub tmdb: TmdbMovie,
    pub audio_selections: Vec<AudioSelection>,
    pub subtitle_indices: Vec<usize>,
}

pub struct ShowChoice {
    pub title_indices: Vec<u32>,
    pub tmdb: TmdbShow,
    pub season: u32,
    pub start_episode: u32,
    pub audio_selections: Vec<AudioSelection>,
    pub subtitle_indices: Vec<usize>,
}

#[derive(Clone)]
pub struct AudioSelection {
    pub stream_index_in_clip: usize,
    pub action: AudioSelectionAction,
}

#[derive(Clone)]
pub enum AudioSelectionAction {
    Copy,
    EncodeAac,
}

fn format_duration(secs: u64) -> String {
    format!("{:02}:{:02}:{:02}", secs / 3600, (secs % 3600) / 60, secs % 60)
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

pub fn run_tui(
    analysis: &TitleAnalysis,
    config: &Config,
) -> Result<MediaChoice, Box<dyn std::error::Error>> {
    println!("\n=== Blu-ray Disc Analysis ===\n");
    println!("Main title: {}", analysis.main_title_idx);
    println!("Found {} titles:\n", analysis.titles.len());

    for t in &analysis.titles {
        let main_marker = if t.index == analysis.main_title_idx {
            " [MAIN]"
        } else {
            ""
        };
        println!(
            "  Title {:>3}  {}  chapters={}  audio={}  subs={}  score={:+}{}",
            t.index,
            format_duration(t.duration_secs),
            t.chapter_count,
            t.audio_streams.len(),
            t.subtitle_streams.len(),
            t.score,
            main_marker,
        );
    }

    // Show detected type
    let is_show = match &analysis.detected_type {
        DiscType::Movie { title_idx } => {
            println!("\nDetected: Movie (title {})", title_idx);
            false
        }
        DiscType::Show { episode_indices } => {
            println!(
                "\nDetected: TV Show ({} episodes: {:?})",
                episode_indices.len(),
                episode_indices
            );
            true
        }
    };

    // Let user confirm or override
    let type_options = ["Movie", "TV Show"];
    let default_idx = if is_show { 1 } else { 0 };
    let type_choice = Select::new()
        .with_prompt("Content type")
        .items(&type_options)
        .default(default_idx)
        .interact()?;

    let tmdb_client = match &config.tmdb_token {
        Some(token) => TmdbClient::new(token),
        None => return Err("TMDB token not configured. Run `config init` first.".into()),
    };

    if type_choice == 0 {
        run_movie_flow(analysis, &tmdb_client, config)
    } else {
        run_show_flow(analysis, &tmdb_client, config)
    }
}

fn run_movie_flow(
    analysis: &TitleAnalysis,
    tmdb: &TmdbClient,
    config: &Config,
) -> Result<MediaChoice, Box<dyn std::error::Error>> {
    // Select title
    let title_labels: Vec<String> = analysis
        .titles
        .iter()
        .map(|t| {
            format!(
                "Title {:>3}  {}  ch={}  audio={}  subs={}  score={:+}",
                t.index,
                format_duration(t.duration_secs),
                t.chapter_count,
                t.audio_streams.len(),
                t.subtitle_streams.len(),
                t.score,
            )
        })
        .collect();

    let default_title = match &analysis.detected_type {
        DiscType::Movie { title_idx } => analysis
            .titles
            .iter()
            .position(|t| t.index == *title_idx)
            .unwrap_or(0),
        _ => 0,
    };

    let title_choice = Select::new()
        .with_prompt("Select title to rip")
        .items(&title_labels)
        .default(default_title)
        .interact()?;

    let selected_title = &analysis.titles[title_choice];

    // TMDB lookup
    let movie = tmdb_movie_lookup(tmdb)?;
    println!("Selected: {}", movie);

    // Audio/subtitle selection
    let audio_selections = select_audio_streams(&selected_title.audio_streams, config)?;
    let subtitle_indices = select_subtitle_streams(&selected_title.subtitle_streams, config)?;

    Ok(MediaChoice::Movie(MovieChoice {
        title_idx: selected_title.index,
        tmdb: movie,
        audio_selections,
        subtitle_indices,
    }))
}

fn run_show_flow(
    analysis: &TitleAnalysis,
    tmdb: &TmdbClient,
    config: &Config,
) -> Result<MediaChoice, Box<dyn std::error::Error>> {
    // Select episode titles
    let title_labels: Vec<String> = analysis
        .titles
        .iter()
        .map(|t| {
            format!(
                "Title {:>3}  {}  ch={}  audio={}  subs={}",
                t.index,
                format_duration(t.duration_secs),
                t.chapter_count,
                t.audio_streams.len(),
                t.subtitle_streams.len(),
            )
        })
        .collect();

    let defaults: Vec<bool> = analysis
        .titles
        .iter()
        .map(|t| match &analysis.detected_type {
            DiscType::Show { episode_indices } => episode_indices.contains(&t.index),
            _ => false,
        })
        .collect();

    let selected = MultiSelect::new()
        .with_prompt("Select episode titles")
        .items(&title_labels)
        .defaults(&defaults)
        .interact()?;

    if selected.is_empty() {
        return Err("No titles selected".into());
    }

    let selected_indices: Vec<u32> = selected
        .iter()
        .map(|&i| analysis.titles[i].index)
        .collect();

    // TMDB lookup
    let show = tmdb_show_lookup(tmdb)?;
    println!("Selected: {}", show);

    // Season selection
    if show.seasons.is_empty() {
        return Err("No seasons found for this show".into());
    }
    let season_labels: Vec<String> = show.seasons.iter().map(|s| s.to_string()).collect();
    let season_choice = Select::new()
        .with_prompt("Select season")
        .items(&season_labels)
        .default(0)
        .interact()?;
    let season = show.seasons[season_choice].season_number;

    let guessed_start = config.show_dir.as_deref().map(|dir| {
        let season_path = super::jellyfin::season_dir(
            dir,
            &show.name,
            show.year,
            show.id,
            season,
        );
        super::jellyfin::guess_next_episode(&season_path)
    }).unwrap_or(1);

    let start_episode: u32 = Input::new()
        .with_prompt("Starting episode number")
        .default(guessed_start)
        .interact_text()?;

    // Audio/subtitle selection from first selected title
    let first_title = analysis
        .titles
        .iter()
        .find(|t| t.index == selected_indices[0])
        .unwrap();

    let audio_selections = select_audio_streams(&first_title.audio_streams, config)?;
    let subtitle_indices = select_subtitle_streams(&first_title.subtitle_streams, config)?;

    Ok(MediaChoice::Show(ShowChoice {
        title_indices: selected_indices,
        tmdb: show,
        season,
        start_episode,
        audio_selections,
        subtitle_indices,
    }))
}

fn tmdb_movie_lookup(
    tmdb: &TmdbClient,
) -> Result<TmdbMovie, Box<dyn std::error::Error>> {
    let input: String = Input::new()
        .with_prompt("TMDB search query or URL")
        .interact_text()?;

    if let Some((TmdbUrlType::Movie, id)) = tmdb::parse_tmdb_url(&input) {
        return Ok(tmdb.get_movie(id)?);
    }

    let results = tmdb.search_movie(&input)?;
    if results.is_empty() {
        return Err("No results found".into());
    }

    let labels: Vec<String> = results.iter().map(|m| m.to_string()).collect();
    let choice = FuzzySelect::new()
        .with_prompt("Select movie")
        .items(&labels)
        .interact()?;

    Ok(results[choice].clone())
}

fn tmdb_show_lookup(
    tmdb: &TmdbClient,
) -> Result<TmdbShow, Box<dyn std::error::Error>> {
    let input: String = Input::new()
        .with_prompt("TMDB search query or URL")
        .interact_text()?;

    if let Some((TmdbUrlType::Tv, id)) = tmdb::parse_tmdb_url(&input) {
        return Ok(tmdb.get_tv(id)?);
    }

    let results = tmdb.search_tv(&input)?;
    if results.is_empty() {
        return Err("No results found".into());
    }

    let labels: Vec<String> = results.iter().map(|s| s.to_string()).collect();
    let choice = FuzzySelect::new()
        .with_prompt("Select show")
        .items(&labels)
        .interact()?;

    // Fetch full details (includes seasons)
    let show = tmdb.get_tv(results[choice].id)?;
    Ok(show)
}

fn select_audio_streams(
    streams: &[AudioStreamInfo],
    config: &Config,
) -> Result<Vec<AudioSelection>, Box<dyn std::error::Error>> {
    if streams.is_empty() {
        return Ok(Vec::new());
    }

    let labels: Vec<String> = streams
        .iter()
        .enumerate()
        .map(|(i, s)| {
            format!(
                "Audio {}  {}  {}",
                i,
                s.language,
                format_coding_type(s.coding_type),
            )
        })
        .collect();

    // Pre-select streams matching preferred languages
    let defaults: Vec<bool> = streams
        .iter()
        .map(|s| {
            if config.languages.audio.is_empty() {
                true // select all if no preference
            } else {
                config.languages.audio.contains(&s.language)
            }
        })
        .collect();

    let selected = MultiSelect::new()
        .with_prompt("Select audio tracks")
        .items(&labels)
        .defaults(&defaults)
        .interact()?;

    let mut selections = Vec::new();
    for &idx in &selected {
        let action_labels = ["Encode to AAC", "Copy (lossless)"];
        let action_choice = Select::new()
            .with_prompt(format!("Action for audio {} ({})", idx, streams[idx].language))
            .items(&action_labels)
            .default(0)
            .interact()?;

        selections.push(AudioSelection {
            stream_index_in_clip: streams[idx].index_in_clip,
            action: if action_choice == 0 {
                AudioSelectionAction::EncodeAac
            } else {
                AudioSelectionAction::Copy
            },
        });
    }

    Ok(selections)
}

fn select_subtitle_streams(
    streams: &[SubtitleStreamInfo],
    config: &Config,
) -> Result<Vec<usize>, Box<dyn std::error::Error>> {
    if streams.is_empty() {
        return Ok(Vec::new());
    }

    let labels: Vec<String> = streams
        .iter()
        .enumerate()
        .map(|(i, s)| format!("Subtitle {}  {}", i, s.language))
        .collect();

    let defaults: Vec<bool> = streams
        .iter()
        .map(|s| {
            if config.languages.subtitle.is_empty() {
                true
            } else {
                config.languages.subtitle.contains(&s.language)
            }
        })
        .collect();

    let selected = MultiSelect::new()
        .with_prompt("Select subtitle tracks")
        .items(&labels)
        .defaults(&defaults)
        .interact()?;

    Ok(selected
        .iter()
        .map(|&i| streams[i].index_in_clip)
        .collect())
}
