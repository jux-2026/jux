//! Wasmer capability mapping.
//!
//! This module is the single place that converts Jux's internal WASM capability
//! model into concrete Wasmer/WASIX runtime and runner configuration. It should
//! stay below the user-facing permission model: callers describe what they want
//! through permissions, then this layer decides which Wasmer knobs are enabled.

use super::runtime::WasmRuntimeError;
use std::path::PathBuf;
use std::sync::Arc;
use wasmer_wasix::runners::MappedDirectory;
use wasmer_wasix::runners::wasi::WasiRunner;
use wasmer_wasix::runtime::package_loader::BuiltinPackageLoader;
use wasmer_wasix::runtime::resolver::InMemorySource;
use wasmer_wasix::{PluggableRuntime, UnsupportedVirtualNetworking};

#[derive(Clone, Debug, Eq, PartialEq)]
/// Concrete capabilities that Jux provides to the Wasmer runtime.
///
/// These values are close to Wasmer/WASIX concepts and should be produced by a
/// higher-level permission or configuration layer.
pub struct WasmerRuntimeCapabilities {
    pub filesystem: WasmFilesystemCapability,
    pub environment: WasmEnvironmentCapability,
    pub stdio: WasmStdioCapability,
    pub network: WasmNetworkCapability,
    pub package_loading: WasmPackageLoadingCapability,
}

impl Default for WasmerRuntimeCapabilities {
    fn default() -> Self {
        Self {
            filesystem: WasmFilesystemCapability::MappedHostDirectory,
            environment: WasmEnvironmentCapability::Isolated,
            stdio: WasmStdioCapability::Buffered,
            network: WasmNetworkCapability::HttpClient,
            package_loading: WasmPackageLoadingCapability::BuiltinWithHttpClient,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// Filesystem capability exposed to the WASI/WASIX program.
pub enum WasmFilesystemCapability {
    Disabled,
    MappedHostDirectory,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// Environment variable capability exposed to the WASI/WASIX program.
pub enum WasmEnvironmentCapability {
    Isolated,
    ForwardHost,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// Standard IO capability exposed to the WASI/WASIX program.
pub enum WasmStdioCapability {
    Buffered,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// Network-related capability exposed through the Wasmer runtime.
pub enum WasmNetworkCapability {
    Disabled,
    HttpClient,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// Package loading capability used by the Wasmer runtime.
pub enum WasmPackageLoadingCapability {
    Disabled,
    Builtin,
    BuiltinWithHttpClient,
}

pub(super) fn apply_runtime_capabilities(
    runtime: &mut PluggableRuntime,
    capabilities: &WasmerRuntimeCapabilities,
) -> Result<(), WasmRuntimeError> {
    let http_client = match capabilities.network {
        WasmNetworkCapability::Disabled => {
            runtime.http_client = None;
            runtime
                .set_networking_implementation(UnsupportedVirtualNetworking::default())
                .set_source(InMemorySource::new());
            None
        }
        WasmNetworkCapability::HttpClient => Some(default_http_client()?),
    };

    match capabilities.package_loading {
        WasmPackageLoadingCapability::Disabled => {}
        WasmPackageLoadingCapability::Builtin => {
            runtime.set_package_loader(BuiltinPackageLoader::new());
        }
        WasmPackageLoadingCapability::BuiltinWithHttpClient => {
            let Some(http_client) = http_client.clone() else {
                return Err(WasmRuntimeError::Run(
                    "wasm package loading requires an enabled http client".to_owned(),
                ));
            };
            runtime.set_package_loader(
                BuiltinPackageLoader::new().with_shared_http_client(http_client),
            );
        }
    }

    if let Some(http_client) = http_client {
        runtime.set_http_client(http_client);
    }

    Ok(())
}

pub(super) fn apply_runner_capabilities(
    runner: &mut WasiRunner,
    capabilities: &WasmerRuntimeCapabilities,
    host_directory: PathBuf,
) {
    runner.with_forward_host_env(matches!(
        capabilities.environment,
        WasmEnvironmentCapability::ForwardHost
    ));

    match capabilities.filesystem {
        WasmFilesystemCapability::Disabled => {}
        WasmFilesystemCapability::MappedHostDirectory => {
            runner.with_mapped_directories([MappedDirectory {
                host: host_directory,
                guest: "/".to_owned(),
            }]);
        }
    }
}

fn default_http_client()
-> Result<Arc<dyn wasmer_wasix::http::HttpClient + Send + Sync>, WasmRuntimeError> {
    Ok(Arc::new(
        wasmer_wasix::http::default_http_client()
            .ok_or_else(|| WasmRuntimeError::Run("wasm http client is unavailable".to_owned()))?,
    ))
}
