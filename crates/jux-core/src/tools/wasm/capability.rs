//! Wasmer capability mapping.
//!
//! This module is the single place that converts Jux's internal WASM capability
//! model into concrete Wasmer/WASIX runtime and runner configuration. It should
//! stay below the user-facing permission model: callers describe what they want
//! through permissions, then this layer decides which Wasmer knobs are enabled.

use super::runtime::WasmRuntimeError;
use crate::{WasmHttpDecision, WasmHttpMethod, WasmNetworkPolicy};
use futures::future::BoxFuture;
use std::path::PathBuf;
use std::sync::Arc;
use wasmer_wasix::http::{HttpClient, HttpRequest, HttpResponse};
use wasmer_wasix::runners::MappedDirectory;
use wasmer_wasix::runners::wasi::WasiRunner;
use wasmer_wasix::runtime::package_loader::BuiltinPackageLoader;
use wasmer_wasix::runtime::resolver::InMemorySource;
use wasmer_wasix::{PluggableRuntime, UnsupportedVirtualNetworking};

pub(super) const GUEST_WORKSPACE_DIRECTORY: &str = "/workspace";

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
    pub http_policy: Option<WasmNetworkPolicy>,
    pub package_loading: WasmPackageLoadingCapability,
}

impl Default for WasmerRuntimeCapabilities {
    fn default() -> Self {
        Self {
            filesystem: WasmFilesystemCapability::MappedHostDirectory,
            environment: WasmEnvironmentCapability::Isolated,
            stdio: WasmStdioCapability::Buffered,
            network: WasmNetworkCapability::HttpClient,
            http_policy: None,
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
        WasmNetworkCapability::HttpClient => Some(configured_http_client(capabilities)?),
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
                guest: GUEST_WORKSPACE_DIRECTORY.to_owned(),
            }]);
        }
    }
}

fn default_http_client() -> Result<Arc<dyn HttpClient + Send + Sync>, WasmRuntimeError> {
    Ok(Arc::new(
        wasmer_wasix::http::default_http_client()
            .ok_or_else(|| WasmRuntimeError::Run("wasm http client is unavailable".to_owned()))?,
    ))
}

fn configured_http_client(
    capabilities: &WasmerRuntimeCapabilities,
) -> Result<Arc<dyn HttpClient + Send + Sync>, WasmRuntimeError> {
    let inner = default_http_client()?;
    let Some(policy) = capabilities.http_policy.clone() else {
        return Ok(inner);
    };
    Ok(Arc::new(PolicyHttpClient { inner, policy }))
}

#[derive(Debug)]
struct PolicyHttpClient {
    inner: Arc<dyn HttpClient + Send + Sync>,
    policy: WasmNetworkPolicy,
}

impl HttpClient for PolicyHttpClient {
    fn request(&self, request: HttpRequest) -> BoxFuture<'_, Result<HttpResponse, anyhow::Error>> {
        let method = request.method.as_str().to_owned();
        let url = request.url.to_string();
        let Some(method) = WasmHttpMethod::from_http_method(&method) else {
            return Box::pin(async move {
                Err(anyhow::anyhow!(
                    "wasm HTTP request denied by policy: unsupported method {method} {url}"
                ))
            });
        };

        match self.policy.decide_http_request(method, &url) {
            Ok(WasmHttpDecision::Allow) => self.inner.request(request),
            Ok(WasmHttpDecision::Deny) => Box::pin(async move {
                Err(anyhow::anyhow!(
                    "wasm HTTP request denied by policy: {method:?} {url}"
                ))
            }),
            Err(error) => Box::pin(async move {
                Err(anyhow::anyhow!(
                    "wasm HTTP policy evaluation failed: {error}"
                ))
            }),
        }
    }
}
