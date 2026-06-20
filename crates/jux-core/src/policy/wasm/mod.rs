mod network;

use crate::tools::wasm::{
    WasmEnvironmentCapability, WasmFilesystemCapability, WasmNetworkCapability,
    WasmPackageLoadingCapability, WasmStdioCapability, WasmerRuntimeCapabilities,
};

pub use self::network::{
    WasmHttpDecision, WasmHttpMatchKind, WasmHttpMethod, WasmHttpRule, WasmHttpRuleEffect,
    WasmNetworkPolicy,
};

#[derive(Clone, Debug, Eq, PartialEq)]
/// Policy for WASM-backed command execution.
pub struct WasmSandboxPolicy {
    pub filesystem: WasmFilesystemPolicy,
    pub environment: WasmEnvironmentPolicy,
    pub network: WasmNetworkPolicy,
    pub packages: Vec<WasmPackageRule>,
}

impl WasmSandboxPolicy {
    #[must_use]
    pub fn workspace_default() -> Self {
        Self {
            filesystem: WasmFilesystemPolicy::ReadWriteWorkspace,
            environment: WasmEnvironmentPolicy::Isolated,
            network: WasmNetworkPolicy {
                http_rules: Vec::new(),
            },
            packages: vec![WasmPackageRule {
                package: "wasmer/coreutils".to_owned(),
                version: Some("1.0.25".to_owned()),
                source: WasmPackageSource::Builtin,
            }],
        }
    }

    #[must_use]
    pub fn allows_package(&self, package: &str, version: Option<&str>) -> bool {
        self.packages
            .iter()
            .any(|rule| rule.matches(package, version))
    }

    #[must_use]
    pub fn requires_http_package_loading(&self) -> bool {
        self.packages
            .iter()
            .any(|rule| rule.source == WasmPackageSource::ConfiguredHttp)
    }
}

impl From<&WasmSandboxPolicy> for WasmerRuntimeCapabilities {
    fn from(policy: &WasmSandboxPolicy) -> Self {
        Self {
            filesystem: match policy.filesystem {
                WasmFilesystemPolicy::Disabled => WasmFilesystemCapability::Disabled,
                WasmFilesystemPolicy::ReadWriteWorkspace => {
                    WasmFilesystemCapability::MappedHostDirectory
                }
            },
            environment: match policy.environment {
                WasmEnvironmentPolicy::Isolated => WasmEnvironmentCapability::Isolated,
                WasmEnvironmentPolicy::AllowList(_) => WasmEnvironmentCapability::Isolated,
            },
            stdio: WasmStdioCapability::Buffered,
            network: if policy.network.http_rules.is_empty() {
                WasmNetworkCapability::Disabled
            } else {
                WasmNetworkCapability::HttpClient
            },
            package_loading: if policy.requires_http_package_loading() {
                WasmPackageLoadingCapability::BuiltinWithHttpClient
            } else if !policy.packages.is_empty() {
                WasmPackageLoadingCapability::Builtin
            } else {
                WasmPackageLoadingCapability::Disabled
            },
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// Filesystem policy for WASM execution.
pub enum WasmFilesystemPolicy {
    Disabled,
    ReadWriteWorkspace,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// Environment variable policy for WASM execution.
pub enum WasmEnvironmentPolicy {
    Isolated,
    AllowList(Vec<String>),
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// Allow-list entry for a WASM package.
pub struct WasmPackageRule {
    pub package: String,
    pub version: Option<String>,
    pub source: WasmPackageSource,
}

impl WasmPackageRule {
    #[must_use]
    pub fn matches(&self, package: &str, version: Option<&str>) -> bool {
        self.package == package
            && self
                .version
                .as_deref()
                .is_none_or(|allowed| Some(allowed) == version)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// Source of an allowed WASM package.
pub enum WasmPackageSource {
    Builtin,
    ConfiguredLocal,
    ConfiguredHttp,
}
