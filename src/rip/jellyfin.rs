use std::path::{Path, PathBuf};

fn sanitize_filename(name: &str) -> String {
    name.chars()
        .filter(|c| !matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|'))
        .collect()
}

pub fn movie_path(base_dir: &str, title: &str, year: Option<u16>, tmdb_id: u64) -> PathBuf {
    let title = sanitize_filename(title);
    let folder = match year {
        Some(y) => format!("{} ({}) [tmdbid-{}]", title, y, tmdb_id),
        None => format!("{} [tmdbid-{}]", title, tmdb_id),
    };
    let file = format!("{}.mkv", folder);
    PathBuf::from(base_dir).join(&folder).join(file)
}

pub fn episode_path(
    base_dir: &str,
    show_name: &str,
    year: Option<u16>,
    tmdb_id: u64,
    season: u32,
    episode: u32,
) -> PathBuf {
    let show_name = sanitize_filename(show_name);
    let show_folder = match year {
        Some(y) => format!("{} ({}) [tmdbid-{}]", show_name, y, tmdb_id),
        None => format!("{} [tmdbid-{}]", show_name, tmdb_id),
    };
    let season_folder = format!("Season {:02}", season);
    let file = format!("{} S{:02}E{:02}.mkv", show_name, season, episode);
    PathBuf::from(base_dir)
        .join(show_folder)
        .join(season_folder)
        .join(file)
}

/// Build the season directory path for a show.
pub fn season_dir(
    base_dir: &str,
    show_name: &str,
    year: Option<u16>,
    tmdb_id: u64,
    season: u32,
) -> PathBuf {
    let show_name = sanitize_filename(show_name);
    let show_folder = match year {
        Some(y) => format!("{} ({}) [tmdbid-{}]", show_name, y, tmdb_id),
        None => format!("{} [tmdbid-{}]", show_name, tmdb_id),
    };
    let season_folder = format!("Season {:02}", season);
    PathBuf::from(base_dir).join(show_folder).join(season_folder)
}

/// Scan an existing season directory for episode files and return the next
/// episode number (i.e. highest existing episode + 1). Returns 1 if the
/// directory doesn't exist or contains no recognisable episodes.
pub fn guess_next_episode(season_path: &Path) -> u32 {
    let entries = match std::fs::read_dir(season_path) {
        Ok(e) => e,
        Err(_) => return 1,
    };

    let mut max_episode: u32 = 0;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        // Look for S##E## pattern anywhere in the filename
        if let Some(pos) = name.find('E') {
            // Walk backwards to find 'S'
            let before = &name[..pos];
            if let Some(s_pos) = before.rfind('S') {
                let season_part = &before[s_pos + 1..];
                // Verify the season part is numeric (so we don't match random S/E)
                if season_part.chars().all(|c| c.is_ascii_digit()) && !season_part.is_empty() {
                    let ep_str: String = name[pos + 1..]
                        .chars()
                        .take_while(|c| c.is_ascii_digit())
                        .collect();
                    if let Ok(ep) = ep_str.parse::<u32>() {
                        max_episode = max_episode.max(ep);
                    }
                }
            }
        }
    }

    if max_episode > 0 {
        max_episode + 1
    } else {
        1
    }
}
