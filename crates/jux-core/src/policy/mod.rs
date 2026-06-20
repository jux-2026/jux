mod native;
mod wasm;

use std::path::PathBuf;

pub use self::native::{NativeCommandPolicy, NativeCommandRule};
pub use self::wasm::{
    WasmEnvironmentPolicy, WasmFilesystemPolicy, WasmHttpDecision, WasmHttpMatchKind,
    WasmHttpMethod, WasmHttpRule, WasmHttpRuleEffect, WasmNetworkPolicy, WasmPackageRule,
    WasmPackageSource, WasmSandboxPolicy,
};
use crate::tools::wasm::WasmerRuntimeCapabilities;

#[derive(Clone, Debug, Eq, PartialEq)]
/// Internal runtime policy used by Jux execution backends.
///
/// This type is not a user configuration format. Future user-facing sandbox
/// configuration should be normalized into this policy before tool execution.
pub struct RuntimePolicy {
    pub workspace_root: PathBuf,
    pub wasm: WasmSandboxPolicy,
    pub native: NativeCommandPolicy,
}

impl RuntimePolicy {
    #[must_use]
    pub fn workspace_default(workspace_root: impl Into<PathBuf>) -> Self {
        let workspace_root = workspace_root.into();
        Self {
            workspace_root,
            wasm: WasmSandboxPolicy::workspace_default(),
            native: NativeCommandPolicy::disabled(),
        }
    }

    #[must_use]
    pub fn wasm_capabilities(&self) -> WasmerRuntimeCapabilities {
        let policy = &self.wasm;
        WasmerRuntimeCapabilities::from(policy)
    }
}
