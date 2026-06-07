//! Core library for the Jux agent runtime.

mod ids;
mod model;
mod orchestrator;
mod store;
mod time;

pub use ids::{RunId, SessionId, StepId, WorkspaceId};
pub use model::{LlmMessageRole, Run, RunStatus, Session, Step, StepKind, StepPayload, Workspace};
pub use orchestrator::{RunLoop, RunLoopError, RunLoopOutput, SYSTEM_PROMPT};
pub use store::{SqliteWorkspaceStore, StoreError};

/// Returns the current workspace package version.
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[cfg(test)]
mod tests;
