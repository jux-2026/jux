//! Fixed-size distribution metadata embedded in every Jux executable.
//!
//! Release jobs patch only this 1 KiB slot, then sign and package the resulting channel artifact.
//! The data selects user-facing update guidance; it is intentionally not a trust boundary.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::fs;
use std::io::{Seek, SeekFrom, Write};
use std::path::Path;

pub const DISTRIBUTION_METADATA_SLOT_SIZE: usize = 1024;
const MARKER_SIZE: usize = 64;
const MAGIC: &[u8; 16] = b"JUXDISTMETA\0\x01\0\0\0";
const SCHEMA_OFFSET: usize = 64;
const CHANNEL_OFFSET: usize = 66;
const INSTALLER_OFFSET: usize = 67;
const VERSION_OFFSET: usize = 68;
const VERSION_SIZE: usize = 32;
const PACKAGE_OFFSET: usize = 100;
const PACKAGE_SIZE: usize = 128;
const COMMIT_OFFSET: usize = 228;
const COMMIT_SIZE: usize = 40;
const SCHEMA_VERSION: u16 = 1;

const fn blank_slot() -> [u8; DISTRIBUTION_METADATA_SLOT_SIZE] {
    let mut slot = [0_u8; DISTRIBUTION_METADATA_SLOT_SIZE];
    let mut index = 0;
    while index < MAGIC.len() {
        slot[index] = MAGIC[index];
        index += 1;
    }
    while index < MARKER_SIZE {
        slot[index] = 0xa5;
        index += 1;
    }
    slot
}

// This fixed-size section is patched after compilation and before signing or packaging. Keeping
// its layout stable lets one expensive platform build produce several channel-specific artifacts.
#[used]
#[cfg_attr(target_os = "macos", unsafe(link_section = "__DATA,__juxmeta"))]
#[cfg_attr(target_os = "linux", unsafe(link_section = ".juxmeta"))]
#[cfg_attr(target_os = "windows", unsafe(link_section = ".juxmeta"))]
static DISTRIBUTION_METADATA_SLOT: [u8; DISTRIBUTION_METADATA_SLOT_SIZE] = blank_slot();

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
#[repr(u8)]
pub enum DistributionChannel {
    Unknown = 0,
    GithubRelease = 1,
    Homebrew = 2,
    Winget = 3,
}

impl DistributionChannel {
    pub fn package_id(self) -> &'static str {
        match self {
            Self::Unknown => "",
            Self::GithubRelease => "jux-2026/jux",
            Self::Homebrew => "jux-2026/tap/jux",
            Self::Winget => "Jux.Jux",
        }
    }
}

impl TryFrom<u8> for DistributionChannel {
    type Error = DistributionMetadataError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::Unknown),
            1 => Ok(Self::GithubRelease),
            2 => Ok(Self::Homebrew),
            3 => Ok(Self::Winget),
            _ => Err(DistributionMetadataError::InvalidChannel(value)),
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
#[repr(u8)]
pub enum InstallerKind {
    Unknown = 0,
    Bash = 1,
    PowerShell = 2,
    Homebrew = 3,
    Winget = 4,
    Manual = 6,
}

impl TryFrom<u8> for InstallerKind {
    type Error = DistributionMetadataError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::Unknown),
            1 => Ok(Self::Bash),
            2 => Ok(Self::PowerShell),
            3 => Ok(Self::Homebrew),
            4 => Ok(Self::Winget),
            6 => Ok(Self::Manual),
            _ => Err(DistributionMetadataError::InvalidInstaller(value)),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DistributionMetadata {
    pub schema_version: u16,
    pub channel: DistributionChannel,
    pub installer: InstallerKind,
    pub application_version: String,
    pub package_id: String,
    pub source_commit: String,
}

impl DistributionMetadata {
    pub fn new(
        channel: DistributionChannel,
        installer: InstallerKind,
        application_version: impl Into<String>,
        source_commit: impl Into<String>,
    ) -> Result<Self, DistributionMetadataError> {
        validate_pair(channel, installer)?;
        let application_version = application_version.into();
        semver::Version::parse(&application_version)
            .map_err(DistributionMetadataError::InvalidVersion)?;
        let source_commit = source_commit.into();
        validate_source_commit(&source_commit)?;
        let metadata = Self {
            schema_version: SCHEMA_VERSION,
            channel,
            installer,
            application_version,
            package_id: channel.package_id().to_owned(),
            source_commit,
        };
        metadata.to_slot()?;
        Ok(metadata)
    }

    pub fn unbranded() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            channel: DistributionChannel::Unknown,
            installer: InstallerKind::Unknown,
            application_version: env!("CARGO_PKG_VERSION").to_owned(),
            package_id: String::new(),
            source_commit: String::new(),
        }
    }

    pub fn from_slot(slot: &[u8]) -> Result<Self, DistributionMetadataError> {
        validate_slot(slot)?;
        let schema_version = u16::from_le_bytes([slot[SCHEMA_OFFSET], slot[SCHEMA_OFFSET + 1]]);
        if schema_version == 0 {
            return Ok(Self::unbranded());
        }
        if schema_version != SCHEMA_VERSION {
            return Err(DistributionMetadataError::UnsupportedSchema(schema_version));
        }
        let channel = DistributionChannel::try_from(slot[CHANNEL_OFFSET])?;
        let installer = InstallerKind::try_from(slot[INSTALLER_OFFSET])?;
        validate_pair(channel, installer)?;
        let package_id = read_text(slot, PACKAGE_OFFSET, PACKAGE_SIZE)?;
        if package_id != channel.package_id() {
            return Err(DistributionMetadataError::InvalidPackageId(package_id));
        }
        let application_version = read_text(slot, VERSION_OFFSET, VERSION_SIZE)?;
        semver::Version::parse(&application_version)
            .map_err(DistributionMetadataError::InvalidVersion)?;
        let source_commit = read_text(slot, COMMIT_OFFSET, COMMIT_SIZE)?;
        validate_source_commit(&source_commit)?;
        Ok(Self {
            schema_version,
            channel,
            installer,
            application_version,
            package_id,
            source_commit,
        })
    }

    pub fn to_slot(
        &self,
    ) -> Result<[u8; DISTRIBUTION_METADATA_SLOT_SIZE], DistributionMetadataError> {
        validate_pair(self.channel, self.installer)?;
        if self.schema_version != SCHEMA_VERSION {
            return Err(DistributionMetadataError::UnsupportedSchema(
                self.schema_version,
            ));
        }
        if self.package_id != self.channel.package_id() {
            return Err(DistributionMetadataError::InvalidPackageId(
                self.package_id.clone(),
            ));
        }
        let mut slot = blank_slot();
        slot[SCHEMA_OFFSET..SCHEMA_OFFSET + 2].copy_from_slice(&SCHEMA_VERSION.to_le_bytes());
        slot[CHANNEL_OFFSET] = self.channel as u8;
        slot[INSTALLER_OFFSET] = self.installer as u8;
        write_text(
            &mut slot,
            VERSION_OFFSET,
            VERSION_SIZE,
            &self.application_version,
        )?;
        write_text(&mut slot, PACKAGE_OFFSET, PACKAGE_SIZE, &self.package_id)?;
        write_text(&mut slot, COMMIT_OFFSET, COMMIT_SIZE, &self.source_commit)?;
        Ok(slot)
    }
}

pub fn embedded_distribution_metadata() -> Result<DistributionMetadata, DistributionMetadataError> {
    DistributionMetadata::from_slot(&DISTRIBUTION_METADATA_SLOT)
}

pub fn inject_distribution_metadata(
    input: &Path,
    output: &Path,
    metadata: &DistributionMetadata,
) -> Result<(), DistributionMetadataError> {
    let executable = fs::read(input).map_err(DistributionMetadataError::ReadExecutable)?;
    let offset = find_slot(&executable)?;
    let slot = metadata.to_slot()?;
    fs::copy(input, output).map_err(DistributionMetadataError::WriteExecutable)?;
    let mut output = fs::OpenOptions::new()
        .write(true)
        .open(output)
        .map_err(DistributionMetadataError::WriteExecutable)?;
    output
        .seek(SeekFrom::Start(offset as u64))
        .and_then(|_| output.write_all(&slot))
        .map_err(DistributionMetadataError::WriteExecutable)
}

fn find_slot(executable: &[u8]) -> Result<usize, DistributionMetadataError> {
    let mut matches = executable
        .windows(DISTRIBUTION_METADATA_SLOT_SIZE)
        .enumerate()
        .filter(|(_, slot)| marker_matches(slot));
    let offset = matches
        .next()
        .map(|(offset, _)| offset)
        .ok_or(DistributionMetadataError::SlotNotFound)?;
    if matches.next().is_some() {
        return Err(DistributionMetadataError::MultipleSlots);
    }
    Ok(offset)
}

fn validate_slot(slot: &[u8]) -> Result<(), DistributionMetadataError> {
    if slot.len() != DISTRIBUTION_METADATA_SLOT_SIZE {
        return Err(DistributionMetadataError::InvalidSlotSize(slot.len()));
    }
    if !marker_matches(slot) {
        return Err(DistributionMetadataError::InvalidMagic);
    }
    Ok(())
}

fn marker_matches(slot: &[u8]) -> bool {
    slot.len() >= MARKER_SIZE
        && &slot[..MAGIC.len()] == MAGIC
        && slot[MAGIC.len()..MARKER_SIZE]
            .iter()
            .all(|byte| *byte == 0xa5)
}

fn validate_pair(
    channel: DistributionChannel,
    installer: InstallerKind,
) -> Result<(), DistributionMetadataError> {
    let valid = matches!(
        (channel, installer),
        (DistributionChannel::Unknown, InstallerKind::Unknown)
            | (DistributionChannel::GithubRelease, InstallerKind::Bash)
            | (
                DistributionChannel::GithubRelease,
                InstallerKind::PowerShell
            )
            | (DistributionChannel::GithubRelease, InstallerKind::Manual)
            | (DistributionChannel::Homebrew, InstallerKind::Homebrew)
            | (DistributionChannel::Winget, InstallerKind::Winget)
    );
    if valid {
        Ok(())
    } else {
        Err(DistributionMetadataError::InvalidChannelInstallerPair)
    }
}

fn validate_source_commit(value: &str) -> Result<(), DistributionMetadataError> {
    if value.len() <= COMMIT_SIZE && value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err(DistributionMetadataError::InvalidSourceCommit(
            value.to_owned(),
        ))
    }
}

fn write_text(
    slot: &mut [u8],
    offset: usize,
    size: usize,
    value: &str,
) -> Result<(), DistributionMetadataError> {
    if value.as_bytes().contains(&0) || value.len() > size {
        return Err(DistributionMetadataError::InvalidText(value.to_owned()));
    }
    slot[offset..offset + value.len()].copy_from_slice(value.as_bytes());
    Ok(())
}

fn read_text(slot: &[u8], offset: usize, size: usize) -> Result<String, DistributionMetadataError> {
    let bytes = &slot[offset..offset + size];
    let length = bytes.iter().position(|byte| *byte == 0).unwrap_or(size);
    std::str::from_utf8(&bytes[..length])
        .map(str::to_owned)
        .map_err(DistributionMetadataError::InvalidUtf8)
}

#[derive(Debug)]
pub enum DistributionMetadataError {
    InvalidSlotSize(usize),
    InvalidMagic,
    UnsupportedSchema(u16),
    InvalidChannel(u8),
    InvalidInstaller(u8),
    InvalidChannelInstallerPair,
    InvalidPackageId(String),
    InvalidVersion(semver::Error),
    InvalidSourceCommit(String),
    InvalidText(String),
    InvalidUtf8(std::str::Utf8Error),
    SlotNotFound,
    MultipleSlots,
    ReadExecutable(std::io::Error),
    WriteExecutable(std::io::Error),
}

impl fmt::Display for DistributionMetadataError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidSlotSize(size) => write!(
                formatter,
                "distribution metadata slot must be 1024 bytes, got {size}"
            ),
            Self::InvalidMagic => write!(formatter, "distribution metadata magic is invalid"),
            Self::UnsupportedSchema(version) => write!(
                formatter,
                "unsupported distribution metadata schema {version}"
            ),
            Self::InvalidChannel(value) => {
                write!(formatter, "invalid distribution channel value {value}")
            }
            Self::InvalidInstaller(value) => {
                write!(formatter, "invalid installer kind value {value}")
            }
            Self::InvalidChannelInstallerPair => write!(
                formatter,
                "distribution channel and installer kind are incompatible"
            ),
            Self::InvalidPackageId(value) => {
                write!(formatter, "invalid package identifier {value:?}")
            }
            Self::InvalidVersion(error) => {
                write!(formatter, "invalid application version: {error}")
            }
            Self::InvalidSourceCommit(value) => {
                write!(formatter, "invalid source commit {value:?}")
            }
            Self::InvalidText(value) => write!(
                formatter,
                "distribution metadata text is invalid or too long: {value:?}"
            ),
            Self::InvalidUtf8(error) => write!(
                formatter,
                "distribution metadata contains invalid UTF-8: {error}"
            ),
            Self::SlotNotFound => write!(formatter, "distribution metadata slot was not found"),
            Self::MultipleSlots => {
                write!(formatter, "multiple distribution metadata slots were found")
            }
            Self::ReadExecutable(error) => write!(formatter, "failed to read executable: {error}"),
            Self::WriteExecutable(error) => {
                write!(formatter, "failed to write executable: {error}")
            }
        }
    }
}

impl std::error::Error for DistributionMetadataError {}
