use crate::model::{Run, Session, Turn, Workspace};
use serde::{Deserialize, Serialize};
use std::error::Error;
use std::fmt::{self, Display};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

const STATE_DIR_NAME: &str = ".jux";
const WORKSPACE_FILE_NAME: &str = "workspace.json";

pub trait WorkspaceStore {
    fn init_workspace(&self) -> Result<Workspace, StoreError>;
    fn load_workspace(&self) -> Result<Workspace, StoreError>;
    fn load_active_session(&self) -> Result<Session, StoreError>;
    fn create_run_in_active_session(&self, request: String) -> Result<Run, StoreError>;
}

#[derive(Clone, Debug)]
pub struct FileWorkspaceStore {
    root: PathBuf,
}

impl FileWorkspaceStore {
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    fn state_dir(&self) -> PathBuf {
        self.root.join(STATE_DIR_NAME)
    }

    fn workspace_path(&self) -> PathBuf {
        self.state_dir().join(WORKSPACE_FILE_NAME)
    }

    fn sessions_dir(&self) -> PathBuf {
        self.state_dir().join("sessions")
    }

    fn runs_dir(&self) -> PathBuf {
        self.state_dir().join("runs")
    }

    fn turns_dir(&self) -> PathBuf {
        self.state_dir().join("turns")
    }

    fn session_path(&self, session: &Session) -> PathBuf {
        self.sessions_dir()
            .join(format!("{}.json", session.id.as_str()))
    }

    fn run_path(&self, run: &Run) -> PathBuf {
        self.runs_dir().join(format!("{}.json", run.id.as_str()))
    }

    fn turn_path(&self, turn: &Turn) -> PathBuf {
        self.turns_dir().join(format!("{}.json", turn.id.as_str()))
    }

    fn ensure_dirs(&self) -> Result<(), StoreError> {
        fs::create_dir_all(self.sessions_dir())?;
        fs::create_dir_all(self.runs_dir())?;
        fs::create_dir_all(self.turns_dir())?;
        Ok(())
    }

    fn write_json<T: Serialize>(&self, path: &Path, value: &T) -> Result<(), StoreError> {
        let json = serde_json::to_string_pretty(value)?;
        fs::write(path, json)?;
        Ok(())
    }

    fn read_json<T: for<'de> Deserialize<'de>>(&self, path: &Path) -> Result<T, StoreError> {
        let json = fs::read_to_string(path)?;
        Ok(serde_json::from_str(&json)?)
    }
}

impl WorkspaceStore for FileWorkspaceStore {
    fn init_workspace(&self) -> Result<Workspace, StoreError> {
        self.ensure_dirs()?;

        if self.workspace_path().exists() {
            return self.load_workspace();
        }

        let mut session = Session::new(crate::WorkspaceId::new(), Some("default".to_owned()));
        let workspace = Workspace::new(self.root.clone(), session.id.clone());
        session.workspace_id = workspace.id.clone();

        self.write_json(&self.workspace_path(), &workspace)?;
        self.write_json(&self.session_path(&session), &session)?;

        Ok(workspace)
    }

    fn load_workspace(&self) -> Result<Workspace, StoreError> {
        self.read_json(&self.workspace_path())
    }

    fn load_active_session(&self) -> Result<Session, StoreError> {
        let workspace = self.load_workspace()?;
        let path = self
            .sessions_dir()
            .join(format!("{}.json", workspace.active_session_id.as_str()));

        self.read_json(&path)
    }

    fn create_run_in_active_session(&self, request: String) -> Result<Run, StoreError> {
        let _workspace = self.init_workspace()?;
        let mut session = self.load_active_session()?;
        let mut run = Run::new(session.id.clone(), request.clone());
        let turn = Turn::user_request(run.id.clone(), request);

        run.add_turn(turn.id.clone());
        session.add_run(run.id.clone());

        self.write_json(&self.session_path(&session), &session)?;
        self.write_json(&self.run_path(&run), &run)?;
        self.write_json(&self.turn_path(&turn), &turn)?;

        Ok(run)
    }
}

#[derive(Debug)]
pub enum StoreError {
    Io(io::Error),
    Json(serde_json::Error),
}

impl Display for StoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "workspace store IO error: {error}"),
            Self::Json(error) => write!(formatter, "workspace store JSON error: {error}"),
        }
    }
}

impl Error for StoreError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Json(error) => Some(error),
        }
    }
}

impl From<io::Error> for StoreError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<serde_json::Error> for StoreError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn initializes_workspace_with_active_session() {
        let store = FileWorkspaceStore::new(temp_workspace_root());

        let workspace = store.init_workspace().expect("workspace initializes");
        let active_session = store
            .load_active_session()
            .expect("active session can be loaded");

        assert_eq!(workspace.active_session_id, active_session.id);
        assert_eq!(workspace.id, active_session.workspace_id);
    }

    #[test]
    fn creates_run_in_active_session() {
        let store = FileWorkspaceStore::new(temp_workspace_root());

        let run = store
            .create_run_in_active_session("Update docs".to_owned())
            .expect("run is created");
        let active_session = store
            .load_active_session()
            .expect("active session can be loaded");

        assert_eq!(run.session_id, active_session.id);
        assert_eq!(run.status, crate::RunStatus::Created);
        assert_eq!(run.turn_ids.len(), 1);
        assert!(active_session.run_ids.contains(&run.id));
    }

    fn temp_workspace_root() -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock is after unix epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("jux-core-test-{unique}"));
        fs::create_dir_all(&path).expect("temp workspace root is created");
        path
    }
}
