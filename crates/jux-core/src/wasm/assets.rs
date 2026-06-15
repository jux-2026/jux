//! Local asset management for WASM runtimes.
//!
//! This module tracks external WASM/WEBC packages that Jux needs at runtime,
//! ensures they exist under the crate-local `assets` directory, and downloads
//! missing files before execution. It does not decide sandbox permissions; those
//! are defined by the WASM capability layer.

use std::error::Error;
use std::fmt::{self, Display};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

const ASSET_ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/assets");

#[derive(Clone, Copy, Debug)]
pub struct WasmAsset {
    pub package: &'static str,
    pub version: &'static str,
    pub filename: &'static str,
    pub download_url: &'static str,
    pub relative_dir: &'static str,
}

impl WasmAsset {
    pub fn ensure_local_file(&self) -> Result<PathBuf, WasmAssetError> {
        let path = self.local_path();
        if path.exists() {
            return Ok(path);
        }
        tracing::info!(
            package = self.package,
            version = self.version,
            url = self.download_url,
            "downloading wasm asset"
        );

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|source| WasmAssetError::CreateDirectory {
                path: parent.to_path_buf(),
                source: source.to_string(),
            })?;
        }

        let bytes = download(self.download_url)?;
        let temporary_path = path.with_extension("download");
        let mut file =
            fs::File::create(&temporary_path).map_err(|source| WasmAssetError::Write {
                path: temporary_path.clone(),
                source: source.to_string(),
            })?;
        file.write_all(&bytes)
            .map_err(|source| WasmAssetError::Write {
                path: temporary_path.clone(),
                source: source.to_string(),
            })?;
        file.sync_all().map_err(|source| WasmAssetError::Write {
            path: temporary_path.clone(),
            source: source.to_string(),
        })?;

        fs::rename(&temporary_path, &path).map_err(|source| WasmAssetError::Write {
            path: path.clone(),
            source: source.to_string(),
        })?;

        Ok(path)
    }

    fn local_path(&self) -> PathBuf {
        Path::new(ASSET_ROOT)
            .join(self.relative_dir)
            .join(self.filename)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WasmAssetError {
    CreateDirectory { path: PathBuf, source: String },
    Download { url: String, source: String },
    HttpStatus { url: String, status: String },
    Write { path: PathBuf, source: String },
}

impl Display for WasmAssetError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CreateDirectory { path, source } => {
                write!(
                    formatter,
                    "wasm asset directory creation failed at {}: {source}",
                    path.display()
                )
            }
            Self::Download { url, source } => {
                write!(formatter, "wasm asset download failed from {url}: {source}")
            }
            Self::HttpStatus { url, status } => {
                write!(
                    formatter,
                    "wasm asset download failed from {url}: HTTP {status}"
                )
            }
            Self::Write { path, source } => {
                write!(
                    formatter,
                    "wasm asset write failed at {}: {source}",
                    path.display()
                )
            }
        }
    }
}

impl Error for WasmAssetError {}

fn download(url: &str) -> Result<Vec<u8>, WasmAssetError> {
    let response = reqwest::blocking::get(url).map_err(|source| WasmAssetError::Download {
        url: url.to_owned(),
        source: source.to_string(),
    })?;
    let status = response.status();
    if !status.is_success() {
        return Err(WasmAssetError::HttpStatus {
            url: url.to_owned(),
            status: status.to_string(),
        });
    }

    response
        .bytes()
        .map(|bytes| bytes.to_vec())
        .map_err(|source| WasmAssetError::Download {
            url: url.to_owned(),
            source: source.to_string(),
        })
}
