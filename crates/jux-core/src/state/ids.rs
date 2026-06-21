use serde::{Deserialize, Serialize};
use std::fmt::{self, Display};
use uuid::Uuid;

#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub struct WorkspaceId(String);

#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub struct SessionId(String);

#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub struct RunId(String);

#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub struct StepId(String);

impl WorkspaceId {
    #[must_use]
    pub fn new() -> Self {
        let raw = Uuid::new_v4().simple().to_string();
        Self(raw[..8].to_owned())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for WorkspaceId {
    fn default() -> Self {
        Self::new()
    }
}

impl From<String> for WorkspaceId {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl SessionId {
    #[must_use]
    pub fn new(workspace_id: &WorkspaceId, number: u64) -> Self {
        Self(format!("{}-{number:04}", workspace_id.as_str()))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    #[must_use]
    pub fn workspace_id(&self) -> WorkspaceId {
        let (workspace_id, _) = self
            .0
            .rsplit_once('-')
            .expect("session id contains workspace id");
        WorkspaceId(workspace_id.to_owned())
    }
}

impl From<String> for SessionId {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl RunId {
    #[must_use]
    pub fn new(session_id: &SessionId, number: u64) -> Self {
        Self(format!("{}-{number:06}", session_id.as_str()))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    #[must_use]
    pub fn session_id(&self) -> SessionId {
        let (session_id, _) = self.0.rsplit_once('-').expect("run id contains session id");
        SessionId(session_id.to_owned())
    }

    #[must_use]
    pub fn workspace_id(&self) -> WorkspaceId {
        self.session_id().workspace_id()
    }
}

impl From<String> for RunId {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl StepId {
    #[must_use]
    pub fn new(run_id: &RunId, number: u64) -> Self {
        Self(format!("{}-{number:06}", run_id.as_str()))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    #[must_use]
    pub fn run_id(&self) -> RunId {
        let (run_id, _) = self.0.rsplit_once('-').expect("step id contains run id");
        RunId(run_id.to_owned())
    }

    #[must_use]
    pub fn session_id(&self) -> SessionId {
        self.run_id().session_id()
    }

    #[must_use]
    pub fn workspace_id(&self) -> WorkspaceId {
        self.run_id().workspace_id()
    }
}

impl From<String> for StepId {
    fn from(value: String) -> Self {
        Self(value)
    }
}

macro_rules! display_id {
    ($name:ident) => {
        impl Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(&self.0)
            }
        }
    };
}

display_id!(WorkspaceId);
display_id!(SessionId);
display_id!(RunId);
display_id!(StepId);
