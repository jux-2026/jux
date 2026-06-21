//! Core library for the Jux agent runtime.

mod main_loop;
mod policy;
mod state;
mod tools;
mod util;

pub use main_loop::{RunLoop, RunLoopContext, RunLoopError, RunLoopOutput, SYSTEM_PROMPT};
pub use policy::{
    MatchPattern, MatchPatternKind, NativeCommandPolicy, NativeCommandRule, RuntimePolicy,
    WasmEnvironmentPolicy, WasmFilesystemPolicy, WasmHttpDecision, WasmHttpMatchKind,
    WasmHttpMethod, WasmHttpRule, WasmHttpRuleEffect, WasmNetworkPolicy, WasmPackageRule,
    WasmPackageSource, WasmSandboxPolicy,
};
pub use state::{
    AssistantResponseItem, LlmUsage, Run, RunId, RunStatus, Session, SessionContextItem,
    SessionContextKind, SessionContextPayload, SessionId, SqliteWorkspaceStore, Step, StepId,
    StepKind, StepPayload, StoreError, Workspace, WorkspaceId,
};
pub use tools::wasm::{
    WasmCommandDefinition, WasmCommandOutput, WasmCommandRequest, WasmEnvironmentCapability,
    WasmEnvironmentPermission, WasmFilesystemCapability, WasmFilesystemPermission,
    WasmNetworkCapability, WasmNetworkPermission, WasmPackageLoadingCapability, WasmPermissions,
    WasmRuntimeError, WasmStdioCapability, WasmerRuntime, WasmerRuntimeCapabilities,
    available_wasm_command_names, available_wasm_commands,
};

/// Returns the current workspace package version.
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
