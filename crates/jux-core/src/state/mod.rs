//! Persistent agent state.
//!
//! This module groups the identifiers, domain models, and SQLite-backed store
//! used by the run loop. The state layer is responsible for stable IDs,
//! serializable run/session/workspace records, ordered run steps, and persisted
//! session context.
//!
//! The core hierarchy is:
//!
//! ```text
//! Workspace
//!   └── Session
//!         ├── SessionContextItem
//!         └── Run
//!               └── Step
//! ```
//!
//! A [`Workspace`] represents one local project root. It stores the root path
//! and points to the active [`Session`] used by commands that do not select a
//! session explicitly.
//!
//! A [`Session`] is the long-lived conversation container within a workspace.
//! Multiple runs can share one session history, so later runs can include
//! visible steps from earlier runs in the same session.
//!
//! A [`SessionContextItem`] is a session-level LLM context entry. It stores
//! stable prompt material such as the system prompt and tool definitions that
//! should be available to every run in the session.
//!
//! A [`Run`] represents one user request being executed by the agent loop. It
//! owns the execution status for that request and groups all persisted steps
//! produced while the request runs.
//!
//! A [`Step`] is an ordered persisted event within a run, such as the user
//! message, assistant response, tool result, or error. Visible steps are used
//! to rebuild LLM history for later model calls.
//!
//! The current implementation does not persist a separate `Turn` model. One
//! loop iteration in the run loop behaves like an implicit turn: it records one
//! assistant response and, when requested, one or more tool results before the
//! next LLM call.
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
