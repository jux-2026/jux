//! Runtime policy model.
//!
//! This module defines Jux's internal execution policy. The policy is not a
//! user-facing configuration format; configuration should be normalized into
//! these types before tools or execution backends use it.
//!
//! `RuntimePolicy` is the top-level policy object. It groups workspace scope,
//! WASM sandbox policy, and native command policy so each backend can derive its
//! own narrower execution context and capabilities.

mod match_pattern;
mod native;
mod wasm;

use std::path::PathBuf;

pub use self::match_pattern::{MatchPattern, MatchPatternKind};
pub use self::native::{NativeCommandPolicy, NativeCommandRule};
pub use self::wasm::{
    WasmEnvironmentPolicy, WasmFilesystemAccess, WasmFilesystemDecision, WasmFilesystemPermissions,
    WasmFilesystemPolicy, WasmFilesystemRule, WasmFilesystemRuleBase, WasmHttpDecision,
    WasmHttpMatchKind, WasmHttpMethod, WasmHttpRule, WasmHttpRuleEffect, WasmNetworkPolicy,
    WasmPackageRule, WasmPackageSource, WasmSandboxPolicy,
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
        let mut wasm = WasmSandboxPolicy::workspace_default();
        wasm.filesystem = WasmFilesystemPolicy::read_write_workdir(workspace_root.clone());
        Self {
            workspace_root,
            wasm,
            native: NativeCommandPolicy::disabled(),
        }
    }

    #[must_use]
    pub fn wasm_capabilities(&self) -> WasmerRuntimeCapabilities {
        let policy = &self.wasm;
        WasmerRuntimeCapabilities::from(policy)
    }
}
