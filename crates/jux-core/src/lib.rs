//! Core library for the Jux agent runtime.

mod ids;
mod main_loop;
mod model;
mod policy;
mod store;
mod time;
mod tools;

pub use ids::{RunId, SessionId, StepId, WorkspaceId};
pub use main_loop::{RunLoop, RunLoopError, RunLoopOutput, SYSTEM_PROMPT};
pub use model::{
    AssistantResponseItem, LlmUsage, Run, RunStatus, Session, SessionContextItem,
    SessionContextKind, SessionContextPayload, Step, StepKind, StepPayload, Workspace,
};
pub use policy::{
    NativeCommandPolicy, NativeCommandRule, RuntimePolicy, WasmEnvironmentPolicy,
    WasmFilesystemPolicy, WasmHttpDecision, WasmHttpMatchKind, WasmHttpMethod, WasmHttpRule,
    WasmHttpRuleEffect, WasmNetworkPolicy, WasmPackageRule, WasmPackageSource, WasmSandboxPolicy,
};
pub use store::{SqliteWorkspaceStore, StoreError};
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
