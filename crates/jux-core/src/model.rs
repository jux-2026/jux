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

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
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
        matches!(self.kind, StepKind::LlmMessage)
    }

    #[must_use]
    pub fn to_llm_line(&self) -> Option<String> {
        match &self.payload {
            StepPayload::LlmMessage { role, content } => Some(format!("{role:?}: {content}")),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum StepKind {
    LlmMessage,
    Error,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum LlmMessageRole {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum StepPayload {
    LlmMessage {
        role: LlmMessageRole,
        content: String,
    },
    Error {
        message: String,
    },
}
