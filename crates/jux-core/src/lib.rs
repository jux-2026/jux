//! Core library for the Jux agent runtime.

mod ids;
mod model;
mod orchestrator;
mod store;
mod time;
mod wasm;

pub use ids::{RunId, SessionId, StepId, WorkspaceId};
pub use model::{
    AssistantResponseItem, LlmUsage, Run, RunStatus, Session, SessionContextItem,
    SessionContextKind, SessionContextPayload, Step, StepKind, StepPayload, Workspace,
};
pub use orchestrator::{RunLoop, RunLoopError, RunLoopOutput, SYSTEM_PROMPT};
pub use store::{SqliteWorkspaceStore, StoreError};
pub use wasm::{
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
