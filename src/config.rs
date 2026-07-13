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
    /// Disc device path (Blu-ray or DVD); also accepts ISOs and directories.
    #[serde(default = "default_bd_path", alias = "disc_path")]
    pub bd_path: String,
    #[serde(default = "default_temp_dir")]
    pub temp_dir: String,
    #[serde(default)]
    pub eject_on_complete: bool,
    #[serde(default)]
    pub transcode: TranscodeConfig,
    #[serde(default)]
    pub languages: LanguageConfig,
    #[serde(default)]
    pub distributed: DistributedConfig,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DistributedConfig {
    pub shared_dir: Option<String>,
    #[serde(default = "default_worker_name")]
    pub worker_name: String,
    #[serde(default = "default_poll_interval")]
    pub poll_interval_secs: u64,
    #[serde(default = "default_max_concurrent")]
    pub max_concurrent_jobs: u32,
    #[serde(default = "default_stale_timeout")]
    pub stale_lock_timeout_secs: u64,
    #[serde(default = "default_max_retries")]
    pub max_retries: u32,
    #[serde(default = "default_cleanup_raw")]
    pub cleanup_raw_media: bool,
}

fn default_worker_name() -> String {
    let mut buf = [0u8; 256];
    let ret = unsafe { libc::gethostname(buf.as_mut_ptr() as *mut libc::c_char, buf.len()) };
    if ret == 0 {
        let len = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
        String::from_utf8_lossy(&buf[..len]).into_owned()
    } else {
        "unknown".into()
    }
}

fn default_poll_interval() -> u64 {
    30
}
fn default_max_concurrent() -> u32 {
    1
}
fn default_stale_timeout() -> u64 {
    3600
}
fn default_max_retries() -> u32 {
    3
}
fn default_cleanup_raw() -> bool {
    true
}

impl Default for DistributedConfig {
    fn default() -> Self {
        Self {
            shared_dir: None,
            worker_name: default_worker_name(),
            poll_interval_secs: default_poll_interval(),
            max_concurrent_jobs: default_max_concurrent(),
            stale_lock_timeout_secs: default_stale_timeout(),
            max_retries: default_max_retries(),
            cleanup_raw_media: default_cleanup_raw(),
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
            distributed: DistributedConfig::default(),
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
