use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::{fmt, fs, io};

#[derive(Debug)]
pub enum ConfigError {
    Io(io::Error),
    Parse(toml::de::Error),
    Serialize(toml::ser::Error),
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "Config IO error: {}", e),
            Self::Parse(e) => write!(f, "Config parse error: {}", e),
            Self::Serialize(e) => write!(f, "Config serialize error: {}", e),
        }
    }
}

impl std::error::Error for ConfigError {}

impl From<io::Error> for ConfigError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub movie_dir: Option<String>,
    pub show_dir: Option<String>,
    pub tmdb_token: Option<String>,
    #[serde(default = "default_bd_path")]
    pub bd_path: String,
    #[serde(default = "default_temp_dir")]
    pub temp_dir: String,
    #[serde(default)]
    pub eject_on_complete: bool,
    #[serde(default)]
    pub transcode: TranscodeConfig,
    #[serde(default)]
    pub languages: LanguageConfig,
}

fn default_bd_path() -> String {
    "/dev/sr0".into()
}

fn default_temp_dir() -> String {
    // Avoid /tmp — it's often tmpfs with limited space, too small for Blu-ray rips.
    // /var/tmp persists across reboots and is typically on the real filesystem.
    "/var/tmp".into()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscodeConfig {
    #[serde(default = "default_video_codec")]
    pub video_codec: String,
    pub crf: Option<u32>,
    pub preset: Option<String>,
    pub audio_bitrate: Option<u64>,
}

impl Default for TranscodeConfig {
    fn default() -> Self {
        Self {
            video_codec: default_video_codec(),
            crf: None,
            preset: None,
            audio_bitrate: None,
        }
    }
}

fn default_video_codec() -> String {
    "copy".into()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LanguageConfig {
    #[serde(default)]
    pub audio: Vec<String>,
    #[serde(default)]
    pub subtitle: Vec<String>,
    #[serde(default = "default_player_language")]
    pub player_language: String,
}

fn default_player_language() -> String {
    "eng".into()
}

impl Default for LanguageConfig {
    fn default() -> Self {
        Self {
            audio: Vec::new(),
            subtitle: Vec::new(),
            player_language: default_player_language(),
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            movie_dir: None,
            show_dir: None,
            tmdb_token: None,
            bd_path: default_bd_path(),
            temp_dir: default_temp_dir(),
            eject_on_complete: false,
            transcode: TranscodeConfig::default(),
            languages: LanguageConfig::default(),
        }
    }
}

impl Config {
    pub fn default_path() -> Option<PathBuf> {
        dirs::config_dir().map(|d| d.join("bluray-rip").join("config.toml"))
    }

    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let content = fs::read_to_string(path)?;
        toml::from_str(&content).map_err(ConfigError::Parse)
    }

    pub fn load_or_default(path: Option<&Path>) -> Self {
        let path = path
            .map(PathBuf::from)
            .or_else(Self::default_path);

        match path {
            Some(p) if p.exists() => Self::load(&p).unwrap_or_else(|e| {
                eprintln!("Warning: failed to load config from {}: {}", p.display(), e);
                Self::default()
            }),
            _ => Self::default(),
        }
    }

    pub fn save(&self, path: &Path) -> Result<(), ConfigError> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let content = toml::to_string_pretty(self).map_err(ConfigError::Serialize)?;
        fs::write(path, content)?;
        Ok(())
    }
}
