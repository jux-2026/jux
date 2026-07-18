//! Non-blocking update discovery and channel-specific upgrade guidance.
//!
//! Successful results are cached for 24 hours. The service never executes the displayed command;
//! package-manager validation and user execution remain outside this read-only check.

use crate::{DistributionChannel, DistributionMetadata, InstallerKind};
use reqwest::blocking::Client;
use semver::Version;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const UPDATE_CHECK_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);
const CACHE_SCHEMA_VERSION: u16 = 1;
const LATEST_RELEASE_ENDPOINT: &str = "https://api.github.com/repos/jux-2026/jux/releases/latest";

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct UpdateCache {
    pub schema_version: u16,
    pub last_checked_at: u64,
    pub current_version: Version,
    pub latest_version: Version,
    pub release_url: String,
    pub startup_notified_version: Option<Version>,
}

impl UpdateCache {
    pub fn update_available(&self) -> bool {
        self.latest_version > self.current_version
    }

    pub fn should_check(&self, now: u64) -> bool {
        now.saturating_sub(self.last_checked_at) >= UPDATE_CHECK_INTERVAL.as_secs()
    }

    pub fn needs_startup_notification(&self) -> bool {
        self.update_available()
            && self.startup_notified_version.as_ref() != Some(&self.latest_version)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct UpdateNotice {
    pub current_version: Version,
    pub latest_version: Version,
    pub release_url: String,
    pub recommendation: UpdateRecommendation,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct UpdateRecommendation {
    pub channel: DistributionChannel,
    pub installer: InstallerKind,
    pub command: Option<UpdateCommand>,
    pub guidance: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct UpdateCommand {
    pub program: String,
    pub arguments: Vec<String>,
}

impl UpdateRecommendation {
    pub fn for_distribution(metadata: &DistributionMetadata) -> Self {
        let (command, guidance) = match metadata.channel {
            DistributionChannel::Homebrew => (
                Some(UpdateCommand::new("brew", ["upgrade", "jux-2026/tap/jux"])),
                "Upgrade with Homebrew.".to_owned(),
            ),
            DistributionChannel::Winget => (
                Some(UpdateCommand::new(
                    "winget",
                    ["upgrade", "--id", "Jux.Jux", "--exact"],
                )),
                "Upgrade with WinGet.".to_owned(),
            ),
            DistributionChannel::GithubRelease => (
                None,
                match metadata.installer {
                    InstallerKind::Bash => "Run the official Bash installer again.",
                    InstallerKind::PowerShell => "Run the official PowerShell installer again.",
                    _ => "Download the latest archive from GitHub Releases.",
                }
                .to_owned(),
            ),
            DistributionChannel::Unknown => (
                None,
                "Choose a supported installation channel from GitHub Releases.".to_owned(),
            ),
        };
        Self {
            channel: metadata.channel,
            installer: metadata.installer,
            command,
            guidance,
        }
    }
}

impl UpdateCommand {
    fn new<const N: usize>(program: &str, arguments: [&str; N]) -> Self {
        Self {
            program: program.to_owned(),
            arguments: arguments.map(str::to_owned).into(),
        }
    }

    pub fn display(&self) -> String {
        std::iter::once(self.program.as_str())
            .chain(self.arguments.iter().map(String::as_str))
            .collect::<Vec<_>>()
            .join(" ")
    }
}

pub struct UpdateChecker {
    cache_path: PathBuf,
    endpoint: String,
}

impl UpdateChecker {
    pub fn new(cache_path: impl Into<PathBuf>) -> Self {
        Self {
            cache_path: cache_path.into(),
            endpoint: LATEST_RELEASE_ENDPOINT.to_owned(),
        }
    }

    pub fn load_cache(&self) -> Result<Option<UpdateCache>, UpdateError> {
        if !self.cache_path.exists() {
            return Ok(None);
        }
        let content = fs::read(&self.cache_path).map_err(UpdateError::ReadCache)?;
        let cache: UpdateCache =
            serde_json::from_slice(&content).map_err(UpdateError::ParseCache)?;
        if cache.schema_version != CACHE_SCHEMA_VERSION {
            return Err(UpdateError::UnsupportedCacheSchema(cache.schema_version));
        }
        validate_release_url(&cache.release_url)?;
        Ok(Some(cache))
    }

    pub fn check_if_due(
        &self,
        current_version: &Version,
    ) -> Result<Option<UpdateCache>, UpdateError> {
        let now = unix_timestamp()?;
        if let Some(cache) = self.load_cache().ok().flatten()
            && cache.current_version == *current_version
            && !cache.should_check(now)
        {
            return Ok(Some(cache));
        }
        self.check_now_at(current_version, now).map(Some)
    }

    pub fn check_now(&self, current_version: &Version) -> Result<UpdateCache, UpdateError> {
        self.check_now_at(current_version, unix_timestamp()?)
    }

    fn check_now_at(
        &self,
        current_version: &Version,
        now: u64,
    ) -> Result<UpdateCache, UpdateError> {
        let client = Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .map_err(UpdateError::BuildClient)?;
        let response = client
            .get(&self.endpoint)
            .header("User-Agent", "jux-update-check")
            .send()
            .and_then(reqwest::blocking::Response::error_for_status)
            .map_err(UpdateError::Request)?;
        let release: GithubRelease = response.json().map_err(UpdateError::Request)?;
        let latest_version = parse_tag_version(&release.tag_name)?;
        validate_release_url(&release.html_url)?;
        let previous_notification = self
            .load_cache()
            .ok()
            .flatten()
            .and_then(|cache| cache.startup_notified_version);
        let cache = UpdateCache {
            schema_version: CACHE_SCHEMA_VERSION,
            last_checked_at: now,
            current_version: current_version.clone(),
            latest_version,
            release_url: release.html_url,
            startup_notified_version: previous_notification,
        };
        self.save_cache(&cache)?;
        Ok(cache)
    }

    pub fn mark_startup_notified(&self, version: &Version) -> Result<(), UpdateError> {
        let Some(mut cache) = self.load_cache()? else {
            return Ok(());
        };
        cache.startup_notified_version = Some(version.clone());
        self.save_cache(&cache)
    }

    fn save_cache(&self, cache: &UpdateCache) -> Result<(), UpdateError> {
        let parent = self.cache_path.parent().unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(parent).map_err(UpdateError::CreateCacheDirectory)?;
        let temporary = self.cache_path.with_extension("json.tmp");
        let content = serde_json::to_vec_pretty(cache).map_err(UpdateError::SerializeCache)?;
        fs::write(&temporary, content).map_err(UpdateError::WriteCache)?;
        match fs::rename(&temporary, &self.cache_path) {
            Ok(()) => Ok(()),
            Err(_) if self.cache_path.exists() => {
                fs::remove_file(&self.cache_path).map_err(UpdateError::WriteCache)?;
                fs::rename(temporary, &self.cache_path).map_err(UpdateError::WriteCache)
            }
            Err(error) => Err(UpdateError::WriteCache(error)),
        }
    }
}

#[derive(Deserialize)]
struct GithubRelease {
    tag_name: String,
    html_url: String,
}

pub fn update_cache_path() -> PathBuf {
    if let Some(path) = std::env::var_os("XDG_CACHE_HOME") {
        return PathBuf::from(path).join("jux/update-cache.json");
    }
    if cfg!(windows)
        && let Some(path) = std::env::var_os("LOCALAPPDATA")
    {
        return PathBuf::from(path).join("jux/update-cache.json");
    }
    std::env::var_os("HOME").map_or_else(
        || PathBuf::from(".jux/update-cache.json"),
        |home| PathBuf::from(home).join(".cache/jux/update-cache.json"),
    )
}

fn parse_tag_version(tag: &str) -> Result<Version, UpdateError> {
    Version::parse(tag.strip_prefix('v').unwrap_or(tag)).map_err(UpdateError::InvalidVersion)
}

fn validate_release_url(value: &str) -> Result<(), UpdateError> {
    let url = reqwest::Url::parse(value)
        .map_err(|error| UpdateError::InvalidReleaseUrl(error.to_string()))?;
    let expected_prefix = "/jux-2026/jux/releases/tag/";
    if url.scheme() == "https"
        && url.host_str() == Some("github.com")
        && url.path().starts_with(expected_prefix)
    {
        Ok(())
    } else {
        Err(UpdateError::UntrustedReleaseUrl(value.to_owned()))
    }
}

fn unix_timestamp() -> Result<u64, UpdateError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(UpdateError::Clock)
}

#[derive(Debug)]
pub enum UpdateError {
    ReadCache(std::io::Error),
    ParseCache(serde_json::Error),
    UnsupportedCacheSchema(u16),
    CreateCacheDirectory(std::io::Error),
    SerializeCache(serde_json::Error),
    WriteCache(std::io::Error),
    BuildClient(reqwest::Error),
    Request(reqwest::Error),
    InvalidVersion(semver::Error),
    InvalidReleaseUrl(String),
    UntrustedReleaseUrl(String),
    Clock(std::time::SystemTimeError),
}

impl fmt::Display for UpdateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ReadCache(error) => write!(formatter, "failed to read update cache: {error}"),
            Self::ParseCache(error) => write!(formatter, "failed to parse update cache: {error}"),
            Self::UnsupportedCacheSchema(version) => {
                write!(formatter, "unsupported update cache schema {version}")
            }
            Self::CreateCacheDirectory(error) => write!(
                formatter,
                "failed to create update cache directory: {error}"
            ),
            Self::SerializeCache(error) => {
                write!(formatter, "failed to serialize update cache: {error}")
            }
            Self::WriteCache(error) => write!(formatter, "failed to write update cache: {error}"),
            Self::BuildClient(error) => {
                write!(formatter, "failed to initialize update client: {error}")
            }
            Self::Request(error) => write!(formatter, "update check failed: {error}"),
            Self::InvalidVersion(error) => write!(formatter, "release version is invalid: {error}"),
            Self::InvalidReleaseUrl(error) => write!(formatter, "release URL is invalid: {error}"),
            Self::UntrustedReleaseUrl(value) => {
                write!(formatter, "release URL is not an official Jux URL: {value}")
            }
            Self::Clock(error) => write!(formatter, "system clock is invalid: {error}"),
        }
    }
}

impl std::error::Error for UpdateError {}
