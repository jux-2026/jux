//! Core library for the Jux agent runtime.

mod ids;
mod model;
mod orchestrator;
mod store;
mod time;
mod wasm_assets;
mod wasm_runtime;

pub use ids::{RunId, SessionId, StepId, WorkspaceId};
pub use model::{
    AssistantResponseItem, LlmUsage, Run, RunStatus, Session, SessionContextItem,
    SessionContextKind, SessionContextPayload, Step, StepKind, StepPayload, Workspace,
};
pub use orchestrator::{RunLoop, RunLoopError, RunLoopOutput, SYSTEM_PROMPT};
pub use store::{SqliteWorkspaceStore, StoreError};
pub use wasm_runtime::{WasmCommandOutput, WasmCommandRequest, WasmRuntimeError, WasmerRuntime};

/// Returns the current workspace package version.
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[cfg(test)]
mod tests;
