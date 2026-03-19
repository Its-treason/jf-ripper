use std::fmt;

use reqwest::blocking::Client;
use serde::Deserialize;

#[derive(Debug)]
pub enum TmdbError {
    Http(reqwest::Error),
    NoToken,
    NotFound,
}

impl fmt::Display for TmdbError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Http(e) => write!(f, "TMDB HTTP error: {}", e),
            Self::NoToken => write!(f, "TMDB token not configured"),
            Self::NotFound => write!(f, "Not found on TMDB"),
        }
    }
}

impl std::error::Error for TmdbError {}

impl From<reqwest::Error> for TmdbError {
    fn from(e: reqwest::Error) -> Self {
        Self::Http(e)
    }
}

const BASE_URL: &str = "https://api.themoviedb.org/3";

pub struct TmdbClient {
    client: Client,
    token: String,
}

#[derive(Debug, Clone)]
pub struct TmdbMovie {
    pub id: u64,
    pub title: String,
    pub year: Option<u16>,
    pub overview: String,
}

impl fmt::Display for TmdbMovie {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.year {
            Some(y) => write!(f, "{} ({})", self.title, y),
            None => write!(f, "{}", self.title),
        }
    }
}

#[derive(Debug, Clone)]
pub struct TmdbShow {
    pub id: u64,
    pub name: String,
    pub year: Option<u16>,
    pub overview: String,
    pub seasons: Vec<TmdbSeason>,
}

impl fmt::Display for TmdbShow {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.year {
            Some(y) => write!(f, "{} ({})", self.name, y),
            None => write!(f, "{}", self.name),
        }
    }
}

#[derive(Debug, Clone)]
pub struct TmdbSeason {
    pub season_number: u32,
    pub episode_count: u32,
    pub name: String,
}

impl fmt::Display for TmdbSeason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({} episodes)", self.name, self.episode_count)
    }
}

#[derive(Debug, Clone)]
pub struct TmdbEpisode {
    pub episode_number: u32,
    pub name: String,
}

/// Parse a TMDB URL and extract the type and ID.
/// Accepts: https://www.themoviedb.org/movie/550-fight-club
///          https://www.themoviedb.org/tv/1396-breaking-bad
pub fn parse_tmdb_url(input: &str) -> Option<(TmdbUrlType, u64)> {
    let input = input.trim();
    // Try movie URL
    if let Some(rest) = input
        .split("themoviedb.org/movie/")
        .nth(1)
    {
        let id_str: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
        if let Ok(id) = id_str.parse::<u64>() {
            return Some((TmdbUrlType::Movie, id));
        }
    }
    // Try TV URL
    if let Some(rest) = input
        .split("themoviedb.org/tv/")
        .nth(1)
    {
        let id_str: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
        if let Ok(id) = id_str.parse::<u64>() {
            return Some((TmdbUrlType::Tv, id));
        }
    }
    None
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TmdbUrlType {
    Movie,
    Tv,
}

fn extract_year(date: &str) -> Option<u16> {
    date.get(..4)?.parse().ok()
}

// --- API response types (serde) ---

#[derive(Deserialize)]
struct SearchResults<T> {
    results: Vec<T>,
}

#[derive(Deserialize)]
struct MovieResult {
    id: u64,
    title: String,
    release_date: Option<String>,
    overview: Option<String>,
}

#[derive(Deserialize)]
struct TvResult {
    id: u64,
    name: String,
    first_air_date: Option<String>,
    overview: Option<String>,
}

#[derive(Deserialize)]
struct TvDetailsResponse {
    id: u64,
    name: String,
    first_air_date: Option<String>,
    overview: Option<String>,
    seasons: Option<Vec<SeasonSummary>>,
}

#[derive(Deserialize)]
struct SeasonSummary {
    season_number: u32,
    episode_count: u32,
    name: Option<String>,
}

#[derive(Deserialize)]
struct SeasonResponse {
    episodes: Option<Vec<EpisodeResult>>,
}

#[derive(Deserialize)]
struct EpisodeResult {
    episode_number: u32,
    name: Option<String>,
}

impl TmdbClient {
    pub fn new(token: &str) -> Self {
        Self {
            client: Client::new(),
            token: token.to_string(),
        }
    }

    fn get(&self, path: &str) -> reqwest::blocking::RequestBuilder {
        self.client
            .get(format!("{}{}", BASE_URL, path))
            .bearer_auth(&self.token)
    }

    pub fn search_movie(&self, query: &str) -> Result<Vec<TmdbMovie>, TmdbError> {
        let resp: SearchResults<MovieResult> = self
            .get("/search/movie")
            .query(&[("query", query)])
            .send()?
            .error_for_status()?
            .json()?;

        Ok(resp
            .results
            .into_iter()
            .map(|r| TmdbMovie {
                id: r.id,
                title: r.title,
                year: r.release_date.as_deref().and_then(extract_year),
                overview: r.overview.unwrap_or_default(),
            })
            .collect())
    }

    pub fn get_movie(&self, id: u64) -> Result<TmdbMovie, TmdbError> {
        let r: MovieResult = self
            .get(&format!("/movie/{}", id))
            .send()?
            .error_for_status()?
            .json()?;

        Ok(TmdbMovie {
            id: r.id,
            title: r.title,
            year: r.release_date.as_deref().and_then(extract_year),
            overview: r.overview.unwrap_or_default(),
        })
    }

    pub fn search_tv(&self, query: &str) -> Result<Vec<TmdbShow>, TmdbError> {
        let resp: SearchResults<TvResult> = self
            .get("/search/tv")
            .query(&[("query", query)])
            .send()?
            .error_for_status()?
            .json()?;

        Ok(resp
            .results
            .into_iter()
            .map(|r| TmdbShow {
                id: r.id,
                name: r.name,
                year: r.first_air_date.as_deref().and_then(extract_year),
                overview: r.overview.unwrap_or_default(),
                seasons: Vec::new(),
            })
            .collect())
    }

    pub fn get_tv(&self, id: u64) -> Result<TmdbShow, TmdbError> {
        let r: TvDetailsResponse = self
            .get(&format!("/tv/{}", id))
            .send()?
            .error_for_status()?
            .json()?;

        Ok(TmdbShow {
            id: r.id,
            name: r.name,
            year: r.first_air_date.as_deref().and_then(extract_year),
            overview: r.overview.unwrap_or_default(),
            seasons: r
                .seasons
                .unwrap_or_default()
                .into_iter()
                .map(|s| TmdbSeason {
                    season_number: s.season_number,
                    episode_count: s.episode_count,
                    name: s.name.unwrap_or_else(|| format!("Season {}", s.season_number)),
                })
                .collect(),
        })
    }

    pub fn get_season_episodes(
        &self,
        tv_id: u64,
        season_number: u32,
    ) -> Result<Vec<TmdbEpisode>, TmdbError> {
        let r: SeasonResponse = self
            .get(&format!("/tv/{}/season/{}", tv_id, season_number))
            .send()?
            .error_for_status()?
            .json()?;

        Ok(r.episodes
            .unwrap_or_default()
            .into_iter()
            .map(|e| TmdbEpisode {
                episode_number: e.episode_number,
                name: e.name.unwrap_or_default(),
            })
            .collect())
    }
}
