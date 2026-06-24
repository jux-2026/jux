use crate::state::{RunId, SessionId, StepId, WorkspaceId};
use crate::util::time::now_millis;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
/// Local project state container.
///
/// A workspace represents one filesystem root known to Jux. It owns the active
/// session pointer used by commands that do not explicitly select a session.
pub struct Workspace {
    pub id: WorkspaceId,
    pub root: PathBuf,
    pub active_session_id: SessionId,
    pub created_at: u128,
    pub updated_at: u128,
}

impl Workspace {
    #[must_use]
    pub fn new(root: PathBuf, id: WorkspaceId, active_session_id: SessionId) -> Self {
        let now = now_millis();

        Self {
            id,
            root,
            active_session_id,
            created_at: now,
            updated_at: now,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
/// Long-lived conversation container within a workspace.
///
/// A session groups multiple runs under one shared history. The run loop builds
/// new LLM requests from the session context plus visible steps from previous
/// runs in the same session.
pub struct Session {
    pub id: SessionId,
    pub name: Option<String>,
    pub created_at: u128,
    pub updated_at: u128,
}

impl Session {
    #[must_use]
    pub fn new(id: SessionId, name: Option<String>) -> Self {
        let now = now_millis();

        Self {
            id,
            name,
            created_at: now,
            updated_at: now,
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
/// Persisted context item shared by all runs in a session.
///
/// Session context stores stable prompt material, such as the system prompt and
/// tool definitions. It is separate from run steps so the same context can be
/// reused across multiple user requests.
pub struct SessionContextItem {
    pub session_id: SessionId,
    pub sequence: u64,
    pub kind: SessionContextKind,
    pub payload: SessionContextPayload,
    pub created_at: u128,
    pub updated_at: u128,
}

impl SessionContextItem {
    #[must_use]
    pub fn new(
        session_id: SessionId,
        sequence: u64,
        kind: SessionContextKind,
        payload: SessionContextPayload,
    ) -> Self {
        let now = now_millis();

        Self {
            session_id,
            sequence,
            kind,
            payload,
            created_at: now,
            updated_at: now,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
/// Type tag for a session context item.
pub enum SessionContextKind {
    SystemPrompt,
    ToolDefinition,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
/// Strongly typed payload for session-level context.
pub enum SessionContextPayload {
    SystemPrompt {
        content: String,
    },
    ToolDefinition {
        name: String,
        description: String,
        parameters: serde_json::Value,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
/// One execution of a user request.
///
/// A run starts in [`RunStatus::Running`], records ordered steps as the agent
/// loop progresses, and eventually becomes completed or failed. A run belongs
/// to a session through its hierarchical [`RunId`].
pub struct Run {
    pub id: RunId,
    pub request: String,
    pub status: RunStatus,
    pub created_at: u128,
    pub updated_at: u128,
}

impl Run {
    #[must_use]
    pub fn new(id: RunId, request: String) -> Self {
        let now = now_millis();

        Self {
            id,
            request,
            status: RunStatus::Running,
            created_at: now,
            updated_at: now,
        }
    }

    pub fn set_status(&mut self, status: RunStatus) {
        self.status = status;
        self.updated_at = now_millis();
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
/// Lifecycle state of a run.
///
/// The current state model is intentionally small: a run is either actively
/// executing, waiting for a human-provided tool result, completed with an
/// answer, or failed with an error step.
pub enum RunStatus {
    Running,
    WaitingForHumanInput,
    Completed,
    Failed,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
/// Ordered persisted event within a run.
///
/// Steps are the audit trail and the replay source for LLM history. Only steps
/// marked visible by [`Step::visible_to_llm`] are converted back into chat
/// messages for later LLM calls.
pub struct Step {
    pub id: StepId,
    pub kind: StepKind,
    pub payload: StepPayload,
    pub created_at: u128,
    pub updated_at: u128,
}

impl Step {
    #[must_use]
    pub fn new(id: StepId, kind: StepKind, payload: StepPayload) -> Self {
        let now = now_millis();

        Self {
            id,
            kind,
            payload,
            created_at: now,
            updated_at: now,
        }
    }

    #[must_use]
    pub fn visible_to_llm(&self) -> bool {
        matches!(
            self.kind,
            StepKind::UserMessage | StepKind::AssistantResponse | StepKind::ToolResult
        )
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
/// Type tag for a persisted run step.
pub enum StepKind {
    UserMessage,
    AssistantResponse,
    ToolResult,
    Error,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
/// Strongly typed payload for a run step.
pub enum StepPayload {
    UserMessage {
        content: String,
    },
    AssistantResponse {
        message_id: Option<String>,
        usage: LlmUsage,
        items: Vec<AssistantResponseItem>,
    },
    ToolResult {
        id: String,
        call_id: Option<String>,
        content: serde_json::Value,
    },
    Error {
        message: String,
    },
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
/// Token usage reported by an LLM completion.
pub struct LlmUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub total_tokens: u64,
    pub cached_input_tokens: u64,
    pub cache_creation_input_tokens: u64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
/// One item inside an assistant response.
///
/// A response can contain visible text, hidden reasoning that is recorded but
/// not sent back to the model, and tool calls that the run loop can execute.
pub enum AssistantResponseItem {
    Text {
        content: String,
    },
    Reasoning {
        content: String,
    },
    ToolCall {
        id: String,
        call_id: Option<String>,
        name: String,
        arguments: serde_json::Value,
    },
}
