use crate::ids::{RunId, SessionId, StepId, WorkspaceId};
use crate::time::now_millis;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
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
pub enum SessionContextKind {
    SystemPrompt,
    ToolDefinition,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
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
pub enum RunStatus {
    Running,
    Completed,
    Failed,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
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
pub enum StepKind {
    UserMessage,
    AssistantResponse,
    ToolResult,
    Error,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
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
        content: String,
    },
    Error {
        message: String,
    },
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct LlmUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub total_tokens: u64,
    pub cached_input_tokens: u64,
    pub cache_creation_input_tokens: u64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
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
