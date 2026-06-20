//! User-facing WASM permission model.
//!
//! This module describes the permissions a future configuration layer can
//! expose to users. It deliberately stays above Wasmer-specific runtime details:
//! permissions are stable product concepts, while capabilities are the concrete
//! Wasmer/WASIX switches used to enforce them.

use super::capability::{
    WasmEnvironmentCapability, WasmFilesystemCapability, WasmNetworkCapability,
    WasmPackageLoadingCapability, WasmStdioCapability, WasmerRuntimeCapabilities,
};

#[derive(Clone, Debug, Eq, PartialEq)]
/// User-configurable permissions for a WASM execution environment.
///
/// These permissions are converted into `WasmerRuntimeCapabilities` before a
/// command is executed.
pub struct WasmPermissions {
    pub filesystem: WasmFilesystemPermission,
    pub environment: WasmEnvironmentPermission,
    pub network: WasmNetworkPermission,
}

impl Default for WasmPermissions {
    fn default() -> Self {
        Self {
            filesystem: WasmFilesystemPermission::Disabled,
            environment: WasmEnvironmentPermission::Isolated,
            network: WasmNetworkPermission::Disabled,
        }
    }
}

impl From<WasmPermissions> for WasmerRuntimeCapabilities {
    fn from(permissions: WasmPermissions) -> Self {
        Self {
            filesystem: match permissions.filesystem {
                WasmFilesystemPermission::Disabled => WasmFilesystemCapability::Disabled,
                WasmFilesystemPermission::HostDirectoryMapping => {
                    WasmFilesystemCapability::MappedHostDirectory
                }
            },
            environment: match permissions.environment {
                WasmEnvironmentPermission::Isolated => WasmEnvironmentCapability::Isolated,
                WasmEnvironmentPermission::ForwardHost => WasmEnvironmentCapability::ForwardHost,
            },
            stdio: WasmStdioCapability::Buffered,
            network: match permissions.network {
                WasmNetworkPermission::Disabled => WasmNetworkCapability::Disabled,
                WasmNetworkPermission::HttpClient => WasmNetworkCapability::HttpClient,
            },
            http_policy: None,
            package_loading: match permissions.network {
                WasmNetworkPermission::Disabled => WasmPackageLoadingCapability::Builtin,
                WasmNetworkPermission::HttpClient => {
                    WasmPackageLoadingCapability::BuiltinWithHttpClient
                }
            },
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// User-facing filesystem permission for WASM execution.
pub enum WasmFilesystemPermission {
    Disabled,
    HostDirectoryMapping,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// User-facing environment variable permission for WASM execution.
pub enum WasmEnvironmentPermission {
    Isolated,
    ForwardHost,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// User-facing network permission for WASM execution.
pub enum WasmNetworkPermission {
    Disabled,
    HttpClient,
}
