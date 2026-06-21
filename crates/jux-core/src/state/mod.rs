//! Persistent agent state.
//!
//! This module groups the identifiers, domain models, and SQLite-backed store
//! used by the run loop. The state layer is responsible for stable IDs,
//! serializable run/session/workspace records, ordered run steps, and persisted
//! session context.
//!
//! Other modules should use the exported model and store types instead of
//! reaching into individual submodules directly.

pub(crate) mod ids;
pub(crate) mod model;
pub(crate) mod store;

pub use self::ids::{RunId, SessionId, StepId, WorkspaceId};
pub use self::model::{
    AssistantResponseItem, LlmUsage, Run, RunStatus, Session, SessionContextItem,
    SessionContextKind, SessionContextPayload, Step, StepKind, StepPayload, Workspace,
};
pub use self::store::{SqliteWorkspaceStore, StoreError};
