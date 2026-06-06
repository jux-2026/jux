use crate::ids::{RunId, SessionId, StepId, WorkspaceId};
use crate::model::{Run, RunStatus, Session, Step, StepKind, StepPayload, Workspace};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use std::error::Error;
use std::fmt::{self, Display};
use std::fs;
use std::path::{Path, PathBuf};

const STATE_DIR_NAME: &str = ".jux";
const DATABASE_FILE_NAME: &str = "state.db";
const DEFAULT_SESSION_NAME: &str = "default";

#[derive(Clone, Debug)]
pub struct SqliteWorkspaceStore {
    root: PathBuf,
}

impl SqliteWorkspaceStore {
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    #[must_use]
    pub fn database_path(&self) -> PathBuf {
        self.root.join(STATE_DIR_NAME).join(DATABASE_FILE_NAME)
    }

    pub fn init_workspace(&self) -> Result<Workspace, StoreError> {
        let mut connection = self.connect()?;
        let existing = self.load_workspace_from(&connection)?;
        if let Some(workspace) = existing {
            return Ok(workspace);
        }

        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let workspace_id = WorkspaceId::new();
        let session_id = SessionId::new(&workspace_id, 1);
        let workspace = Workspace::new(self.root.clone(), workspace_id.clone(), session_id.clone());
        let session = Session::new(session_id.clone(), Some(DEFAULT_SESSION_NAME.to_owned()));

        transaction.execute(
            "insert into workspaces (id, root_path, active_session_id, created_at, updated_at)
             values (?1, ?2, ?3, ?4, ?5)",
            params![
                workspace.id.as_str(),
                workspace.root.to_string_lossy(),
                workspace.active_session_id.as_str(),
                workspace.created_at.to_string(),
                workspace.updated_at.to_string(),
            ],
        )?;
        insert_session(&transaction, &session)?;
        transaction.commit()?;

        tracing::info!(
            workspace_id = %workspace.id,
            session_id = %session.id,
            database = %self.database_path().display(),
            "initialized workspace state"
        );

        Ok(workspace)
    }

    pub fn load_workspace(&self) -> Result<Workspace, StoreError> {
        let connection = self.connect()?;
        self.load_workspace_from(&connection)?
            .ok_or(StoreError::MissingWorkspace)
    }

    pub fn load_active_session(&self) -> Result<Session, StoreError> {
        let workspace = self.load_workspace()?;
        self.load_session(&workspace.active_session_id)
    }

    pub fn load_session(&self, session_id: &SessionId) -> Result<Session, StoreError> {
        let connection = self.connect()?;
        connection
            .query_row(
                "select id, name, created_at, updated_at from sessions where id = ?1",
                params![session_id.as_str()],
                row_to_session,
            )
            .optional()?
            .ok_or_else(|| StoreError::MissingSession(session_id.to_string()))
    }

    pub fn create_run(&self, request: String) -> Result<Run, StoreError> {
        let workspace = self.init_workspace()?;
        let mut connection = self.connect()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let run_number = next_run_number(&transaction, &workspace.active_session_id)?;
        let run_id = RunId::new(&workspace.active_session_id, run_number);
        let run = Run::new(run_id.clone(), request);

        transaction.execute(
            "insert into runs (id, request, status, created_at, updated_at)
             values (?1, ?2, ?3, ?4, ?5)",
            params![
                run.id.as_str(),
                run.request,
                encode_run_status(&run.status),
                run.created_at.to_string(),
                run.updated_at.to_string(),
            ],
        )?;
        transaction.commit()?;

        tracing::info!(run_id = %run.id, "persisted run");

        Ok(run)
    }

    pub fn append_step(
        &self,
        run_id: &RunId,
        kind: StepKind,
        payload: StepPayload,
    ) -> Result<Step, StoreError> {
        let mut connection = self.connect()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let step_number = next_step_number(&transaction, run_id)?;
        let step_id = StepId::new(run_id, step_number);
        let step = Step::new(step_id, kind, payload);

        insert_step(&transaction, &step)?;
        transaction.commit()?;

        tracing::debug!(
            step_id = %step.id,
            kind = ?step.kind,
            "persisted step"
        );

        Ok(step)
    }

    pub fn load_run(&self, run_id: &RunId) -> Result<Run, StoreError> {
        let connection = self.connect()?;
        connection
            .query_row(
                "select id, request, status, created_at, updated_at from runs where id = ?1",
                params![run_id.as_str()],
                row_to_run,
            )
            .optional()?
            .ok_or_else(|| StoreError::MissingRun(run_id.to_string()))
    }

    pub fn update_run_status(&self, run_id: &RunId, status: RunStatus) -> Result<Run, StoreError> {
        let mut run = self.load_run(run_id)?;
        run.set_status(status);
        let connection = self.connect()?;
        connection.execute(
            "update runs set status = ?1, updated_at = ?2 where id = ?3",
            params![
                encode_run_status(&run.status),
                run.updated_at.to_string(),
                run.id.as_str(),
            ],
        )?;
        Ok(run)
    }

    pub fn load_session_runs(&self, session_id: &SessionId) -> Result<Vec<Run>, StoreError> {
        let connection = self.connect()?;
        let prefix = format!("{}-%", session_id.as_str());
        let mut statement = connection.prepare(
            "select id, request, status, created_at, updated_at
             from runs
             where id like ?1
             order by id",
        )?;
        let rows = statement.query_map(params![prefix], row_to_run)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(StoreError::from)
    }

    pub fn load_session_steps(&self, session_id: &SessionId) -> Result<Vec<Step>, StoreError> {
        let connection = self.connect()?;
        let prefix = format!("{}-%", session_id.as_str());
        let mut statement = connection.prepare(
            "select id, kind, payload_json, created_at, updated_at
             from steps
             where id like ?1
             order by id",
        )?;
        let rows = statement.query_map(params![prefix], row_to_step)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(StoreError::from)
    }

    pub fn load_run_steps(&self, run_id: &RunId) -> Result<Vec<Step>, StoreError> {
        let connection = self.connect()?;
        let prefix = format!("{}-%", run_id.as_str());
        let mut statement = connection.prepare(
            "select id, kind, payload_json, created_at, updated_at
             from steps
             where id like ?1
             order by id",
        )?;
        let rows = statement.query_map(params![prefix], row_to_step)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(StoreError::from)
    }

    fn connect(&self) -> Result<Connection, StoreError> {
        fs::create_dir_all(self.root.join(STATE_DIR_NAME))?;
        let connection = Connection::open(self.database_path())?;
        init_schema(&connection)?;
        Ok(connection)
    }

    fn load_workspace_from(
        &self,
        connection: &Connection,
    ) -> Result<Option<Workspace>, StoreError> {
        connection
            .query_row(
                "select id, root_path, active_session_id, created_at, updated_at
                 from workspaces
                 limit 1",
                [],
                row_to_workspace,
            )
            .optional()
            .map_err(StoreError::from)
    }
}

#[derive(Debug)]
pub enum StoreError {
    Io(std::io::Error),
    Sqlite(rusqlite::Error),
    Json(serde_json::Error),
    MissingWorkspace,
    MissingSession(String),
    MissingRun(String),
    InvalidStatus(String),
    InvalidStepKind(String),
    InvalidId(String),
}

impl Display for StoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "workspace store IO error: {error}"),
            Self::Sqlite(error) => write!(formatter, "workspace store SQLite error: {error}"),
            Self::Json(error) => write!(formatter, "workspace store JSON error: {error}"),
            Self::MissingWorkspace => formatter.write_str("workspace state is not initialized"),
            Self::MissingSession(id) => write!(formatter, "session does not exist: {id}"),
            Self::MissingRun(id) => write!(formatter, "run does not exist: {id}"),
            Self::InvalidStatus(status) => write!(formatter, "invalid stored status: {status}"),
            Self::InvalidStepKind(kind) => write!(formatter, "invalid stored step kind: {kind}"),
            Self::InvalidId(id) => write!(formatter, "invalid stored id: {id}"),
        }
    }
}

impl Error for StoreError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Sqlite(error) => Some(error),
            Self::Json(error) => Some(error),
            Self::MissingWorkspace
            | Self::MissingSession(_)
            | Self::MissingRun(_)
            | Self::InvalidStatus(_)
            | Self::InvalidStepKind(_)
            | Self::InvalidId(_) => None,
        }
    }
}

impl From<std::io::Error> for StoreError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<rusqlite::Error> for StoreError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Sqlite(error)
    }
}

impl From<serde_json::Error> for StoreError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

fn init_schema(connection: &Connection) -> Result<(), StoreError> {
    connection.execute_batch(
        "
        create table if not exists workspaces (
            id text primary key,
            root_path text not null,
            active_session_id text not null,
            created_at text not null,
            updated_at text not null
        );

        create table if not exists sessions (
            id text primary key,
            name text,
            created_at text not null,
            updated_at text not null
        );

        create table if not exists runs (
            id text primary key,
            request text not null,
            status text not null,
            created_at text not null,
            updated_at text not null
        );

        create table if not exists steps (
            id text primary key,
            kind text not null,
            payload_json text not null,
            created_at text not null,
            updated_at text not null
        );

        ",
    )?;
    Ok(())
}

fn next_run_number(
    transaction: &Transaction<'_>,
    session_id: &SessionId,
) -> Result<u64, StoreError> {
    let prefix = format!("{}-%", session_id.as_str());
    let latest_id = latest_id_with_prefix(transaction, "runs", &prefix)?;
    next_number_after(latest_id.as_deref())
}

fn next_step_number(transaction: &Transaction<'_>, run_id: &RunId) -> Result<u64, StoreError> {
    let prefix = format!("{}-%", run_id.as_str());
    let latest_id = latest_id_with_prefix(transaction, "steps", &prefix)?;
    next_number_after(latest_id.as_deref())
}

fn latest_id_with_prefix(
    transaction: &Transaction<'_>,
    table_name: &str,
    prefix: &str,
) -> Result<Option<String>, StoreError> {
    let sql = format!("select id from {table_name} where id like ?1 order by id desc limit 1");
    transaction
        .query_row(&sql, params![prefix], |row| row.get(0))
        .optional()
        .map_err(StoreError::from)
}

fn next_number_after(latest_id: Option<&str>) -> Result<u64, StoreError> {
    let Some(latest_id) = latest_id else {
        return Ok(1);
    };
    let (_, number) = latest_id
        .rsplit_once('-')
        .ok_or_else(|| StoreError::InvalidId(latest_id.to_owned()))?;
    let number = number
        .parse::<u64>()
        .map_err(|_| StoreError::InvalidId(latest_id.to_owned()))?;
    Ok(number + 1)
}

fn insert_session(transaction: &Transaction<'_>, session: &Session) -> Result<(), StoreError> {
    transaction.execute(
        "insert into sessions (id, name, created_at, updated_at)
         values (?1, ?2, ?3, ?4)",
        params![
            session.id.as_str(),
            session.name,
            session.created_at.to_string(),
            session.updated_at.to_string(),
        ],
    )?;
    Ok(())
}

fn insert_step(transaction: &Transaction<'_>, step: &Step) -> Result<(), StoreError> {
    transaction.execute(
        "insert into steps (id, kind, payload_json, created_at, updated_at)
         values (?1, ?2, ?3, ?4, ?5)",
        params![
            step.id.as_str(),
            encode_step_kind(&step.kind),
            serde_json::to_string(&step.payload)?,
            step.created_at.to_string(),
            step.updated_at.to_string(),
        ],
    )?;
    Ok(())
}

fn row_to_workspace(row: &rusqlite::Row<'_>) -> rusqlite::Result<Workspace> {
    Ok(Workspace {
        id: WorkspaceId::from(row.get::<_, String>(0)?),
        root: PathBuf::from(row.get::<_, String>(1)?),
        active_session_id: SessionId::from(row.get::<_, String>(2)?),
        created_at: parse_timestamp(row.get::<_, String>(3)?),
        updated_at: parse_timestamp(row.get::<_, String>(4)?),
    })
}

fn row_to_session(row: &rusqlite::Row<'_>) -> rusqlite::Result<Session> {
    Ok(Session {
        id: SessionId::from(row.get::<_, String>(0)?),
        name: row.get(1)?,
        created_at: parse_timestamp(row.get::<_, String>(2)?),
        updated_at: parse_timestamp(row.get::<_, String>(3)?),
    })
}

fn row_to_run(row: &rusqlite::Row<'_>) -> rusqlite::Result<Run> {
    let status = decode_run_status(row.get::<_, String>(2)?).map_err(to_sql_error)?;
    Ok(Run {
        id: RunId::from(row.get::<_, String>(0)?),
        request: row.get(1)?,
        status,
        created_at: parse_timestamp(row.get::<_, String>(3)?),
        updated_at: parse_timestamp(row.get::<_, String>(4)?),
    })
}

fn row_to_step(row: &rusqlite::Row<'_>) -> rusqlite::Result<Step> {
    let kind = decode_step_kind(row.get::<_, String>(1)?).map_err(to_sql_error)?;
    let payload = serde_json::from_str(&row.get::<_, String>(2)?)
        .map_err(|error| to_sql_error(error.into()))?;

    Ok(Step {
        id: StepId::from(row.get::<_, String>(0)?),
        kind,
        payload,
        created_at: parse_timestamp(row.get::<_, String>(3)?),
        updated_at: parse_timestamp(row.get::<_, String>(4)?),
    })
}

fn to_sql_error(error: StoreError) -> rusqlite::Error {
    rusqlite::Error::ToSqlConversionFailure(Box::new(error))
}

fn parse_timestamp(value: String) -> u128 {
    value
        .parse()
        .expect("stored timestamp is written by jux runtime")
}

fn encode_run_status(status: &RunStatus) -> &'static str {
    match status {
        RunStatus::Running => "running",
        RunStatus::Completed => "completed",
        RunStatus::Failed => "failed",
    }
}

fn decode_run_status(status: String) -> Result<RunStatus, StoreError> {
    match status.as_str() {
        "running" => Ok(RunStatus::Running),
        "completed" => Ok(RunStatus::Completed),
        "failed" => Ok(RunStatus::Failed),
        _ => Err(StoreError::InvalidStatus(status)),
    }
}

fn encode_step_kind(kind: &StepKind) -> &'static str {
    match kind {
        StepKind::UserRequest => "user_request",
        StepKind::LlmCall => "llm_call",
        StepKind::AssistantToolCall => "assistant_tool_call",
        StepKind::ToolResult => "tool_result",
        StepKind::AssistantMessage => "assistant_message",
        StepKind::Error => "error",
    }
}

fn decode_step_kind(kind: String) -> Result<StepKind, StoreError> {
    match kind.as_str() {
        "user_request" => Ok(StepKind::UserRequest),
        "llm_call" => Ok(StepKind::LlmCall),
        "assistant_tool_call" => Ok(StepKind::AssistantToolCall),
        "tool_result" => Ok(StepKind::ToolResult),
        "assistant_message" => Ok(StepKind::AssistantMessage),
        "error" => Ok(StepKind::Error),
        _ => Err(StoreError::InvalidStepKind(kind)),
    }
}
