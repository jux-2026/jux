use jux_core::{
    AgentEvent, AgentEventData, AgentEventKind, AssistantResponseItem, CALL_SKILL_TOOL_NAME,
    CodeChangeError, CodeChangeProposal, CodeChangeReview, CopyMessageShortcut, HumanInputRequest,
    PROPOSE_CODE_CHANGE_TOOL_NAME, QuitShortcut, ReviewStatus, Run, RunStatus, Session, SessionId,
    SkillCatalog, SkillDefinition, SkillOverride, SqliteWorkspaceStore, Step, StepPayload,
    StoreError, ToolOutputStream, TuiShortcutConfig, TuiTheme, UpdateNotice,
    latest_human_input_request,
};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::PathBuf;

use super::FileIndexSnapshot;
use super::RunResponse;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AppCommand {
    StartRun { request: String },
    CancelRun,
    CreateSession { name: Option<String> },
    RenameActiveSession { name: String },
    RenameSession { session_id: SessionId, name: String },
    ToggleSessionLiked { session_id: SessionId },
    ArchiveSession { session_id: SessionId },
    DeleteSession { session_id: SessionId },
    SwitchSession { session_id: SessionId },
    AcceptCodeChange,
    RejectCodeChange,
    RequestCodeChanges { feedback: String },
    CopyText { content: String },
}

#[path = "ui/controller.rs"]
mod controller;

pub use self::controller::update;
pub(crate) use self::controller::{UiStateRefs, update_ui, update_with_ui};

fn truncate_timeline_detail_text(content: &str) -> String {
    if content.chars().count() <= 80 {
        return content.to_owned();
    }
    let mut truncated = content.chars().take(80).collect::<String>();
    truncated.push_str("… [truncated]");
    truncated
}

fn timeline_item_from_agent_event(event: AgentEvent) -> Option<TimelineItem> {
    let (label, detail, arguments, output, command) = match event.data {
        AgentEventData::LlmStarted => ("LLM".to_owned(), None, None, None, None),
        AgentEventData::LlmCompleted => return None,
        AgentEventData::LlmFailed { error } => ("LLM".to_owned(), Some(error), None, None, None),
        AgentEventData::SkillsSelected { skills } => (
            "Active skills".to_owned(),
            None,
            None,
            Some(skills.join(", ")),
            None,
        ),
        AgentEventData::ToolStarted {
            name, arguments, ..
        } => {
            let label = if name == CALL_SKILL_TOOL_NAME {
                arguments
                    .get("name")
                    .and_then(serde_json::Value::as_str)
                    .map_or_else(|| "Skill".to_owned(), |name| format!("Skill: {name}"))
            } else {
                format!("Tool: {name}")
            };
            let command = (name == "exec")
                .then(|| command_from_arguments(&arguments))
                .flatten();
            (label, None, Some(format_json(&arguments)), None, command)
        }
        AgentEventData::ToolCompleted { name, .. } => {
            (tool_or_skill_label(&name), None, None, None, None)
        }
        AgentEventData::ToolOutput { name, content } => {
            let output = (name != PROPOSE_CODE_CHANGE_TOOL_NAME).then(|| format_json(&content));
            let command = (name == "exec")
                .then(|| command_from_output(&content))
                .flatten();
            (tool_or_skill_label(&name), None, None, output, command)
        }
        AgentEventData::ToolOutputChunk {
            name,
            stream,
            content,
        } => {
            let command = (name == "exec").then(|| command_from_output_chunk(stream, content));
            (tool_or_skill_label(&name), None, None, None, command)
        }
        AgentEventData::ToolFailed { name, error, .. } => {
            (tool_or_skill_label(&name), Some(error), None, None, None)
        }
        _ => return None,
    };
    let status = match event.kind {
        AgentEventKind::Started => TimelineStatus::Running,
        AgentEventKind::Output => TimelineStatus::Output,
        AgentEventKind::Completed => TimelineStatus::Completed,
        AgentEventKind::Failed => TimelineStatus::Failed,
    };
    Some(TimelineItem {
        id: event.id.to_string(),
        message_count: 0,
        label,
        status,
        detail,
        arguments,
        output,
        command,
    })
}

fn command_from_arguments(value: &serde_json::Value) -> Option<TuiCommandExecution> {
    let arguments = serde_json::from_value::<TuiCommandArguments>(value.clone()).ok()?;
    Some(TuiCommandExecution {
        program: arguments.program,
        args: arguments.args,
        success: None,
        exit_code: None,
        stdout: String::new(),
        stderr: String::new(),
    })
}

fn command_from_output(value: &serde_json::Value) -> Option<TuiCommandExecution> {
    let output = serde_json::from_value::<TuiCommandOutput>(value.clone()).ok()?;
    Some(TuiCommandExecution {
        program: String::new(),
        args: Vec::new(),
        success: Some(output.success),
        exit_code: output.exit_code,
        stdout: output.stdout,
        stderr: output.stderr,
    })
}

fn command_from_output_chunk(stream: ToolOutputStream, content: String) -> TuiCommandExecution {
    let (stdout, stderr) = match stream {
        ToolOutputStream::Stdout => (content, String::new()),
        ToolOutputStream::Stderr => (String::new(), content),
    };
    TuiCommandExecution {
        program: String::new(),
        args: Vec::new(),
        success: None,
        exit_code: None,
        stdout,
        stderr,
    }
}

fn merge_command_execution(
    existing: Option<TuiCommandExecution>,
    incoming: Option<TuiCommandExecution>,
) -> Option<TuiCommandExecution> {
    match (existing, incoming) {
        (Some(mut existing), Some(incoming)) => {
            if !incoming.program.is_empty() {
                existing.program = incoming.program;
                existing.args = incoming.args;
            }
            if incoming.success.is_some() {
                existing.success = incoming.success;
                existing.exit_code = incoming.exit_code;
                existing.stdout = incoming.stdout;
                existing.stderr = incoming.stderr;
            } else {
                existing.stdout.push_str(&incoming.stdout);
                existing.stderr.push_str(&incoming.stderr);
            }
            Some(existing)
        }
        (existing, None) => existing,
        (None, incoming) => incoming,
    }
}

fn format_json(value: &serde_json::Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
}

fn tool_or_skill_label(name: &str) -> String {
    if name == CALL_SKILL_TOOL_NAME {
        "Skill".to_owned()
    } else {
        format!("Tool: {name}")
    }
}

fn apply_code_change_review(
    review: &mut CodeChangeReview,
    workspace_root: &std::path::Path,
) -> Result<TuiCodeChangeResult, CodeChangeError> {
    match review.approve() {
        Ok(()) => {}
        Err(CodeChangeError::PolicyDenied) => return Ok(TuiCodeChangeResult::Denied),
        Err(error) => return Err(error),
    }
    let file_count = review.proposal.files.len();
    match review.apply(workspace_root) {
        Ok(()) => Ok(TuiCodeChangeResult::Applied { file_count }),
        Err(CodeChangeError::Conflict(paths)) => {
            debug_assert!(matches!(review.status, ReviewStatus::Conflict { .. }));
            Ok(TuiCodeChangeResult::Conflict { paths })
        }
        Err(error) => Err(error),
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct AppModel {
    pub workspace_root: PathBuf,
    pub should_quit: bool,
    messages: Vec<Message>,
    message_render_keys: Vec<MessageRenderKey>,
    next_message_id: u64,
    streaming_assistant_message: Option<usize>,
    last_agent_event_sequence: u64,
    run_status: TuiRunStatus,
    session_id: Option<String>,
    run_id: Option<String>,
    run_elapsed_millis: Option<u128>,
    estimated_input_tokens: u64,
    estimated_output_tokens: u64,
    timeline: Vec<TimelineItem>,
    steps: Vec<Step>,
    pending_human_input: Option<HumanInputRequest>,
    runs: Vec<Run>,
    sessions: Vec<Session>,
    session_histories: Vec<SessionHistory>,
    pending_new_session: bool,
    code_change_review: Option<CodeChangeReview>,
    code_change_result: Option<TuiCodeChangeResult>,
    audit_items: Vec<AuditItem>,
    skills: Vec<SkillDefinition>,
    skill_overrides: Vec<SkillOverride>,
    selected_skill_names: Vec<String>,
    active_skill_names: Vec<String>,
    runtime_info: TuiRuntimeInfo,
    runtime_logs: Vec<TuiRuntimeLog>,
    indexed_files: Vec<String>,
    file_index_revision: u64,
    update_notice: Option<UpdateNotice>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Message {
    pub role: MessageRole,
    pub content: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct MessageRenderKey {
    pub id: u64,
    pub revision: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionHistory {
    pub session_id: SessionId,
    pub runs: Vec<Run>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TuiCodeChangeResult {
    Applied { file_count: usize },
    Rejected,
    ChangesRequested,
    Conflict { paths: Vec<String> },
    Denied,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuditItem {
    pub title: String,
    pub detail: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AuditFilter {
    All,
    Files,
    Commands,
    Policy,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TuiSandboxSummary {
    pub filesystem: String,
    pub network: String,
    pub native_commands: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TuiRuntimeInfo {
    pub workspace_id: Option<String>,
    pub model_provider: String,
    pub model_name: String,
    pub sandbox: TuiSandboxSummary,
    pub config_error: Option<String>,
    pub theme: TuiTheme,
    pub scroll_lines: u16,
    pub shortcuts: TuiShortcutConfig,
}

impl Default for TuiRuntimeInfo {
    fn default() -> Self {
        Self {
            workspace_id: None,
            model_provider: "-".to_owned(),
            model_name: "-".to_owned(),
            sandbox: TuiSandboxSummary {
                filesystem: "-".to_owned(),
                network: "-".to_owned(),
                native_commands: "-".to_owned(),
            },
            config_error: None,
            theme: TuiTheme::Dark,
            scroll_lines: 5,
            shortcuts: TuiShortcutConfig::default(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TuiRuntimeLog {
    pub title: String,
    pub detail: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MessageRole {
    User,
    Assistant,
    Error,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TuiRunStatus {
    Idle,
    Running,
    WaitingForHumanInput,
    Completed,
    Failed,
    Canceled,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TimelineItem {
    pub id: String,
    pub message_count: usize,
    pub label: String,
    pub status: TimelineStatus,
    pub detail: Option<String>,
    pub arguments: Option<String>,
    pub output: Option<String>,
    pub command: Option<TuiCommandExecution>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TuiCommandExecution {
    pub program: String,
    pub args: Vec<String>,
    pub success: Option<bool>,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

#[derive(Deserialize)]
struct TuiCommandArguments {
    program: String,
    args: Vec<String>,
}

#[derive(Deserialize)]
struct TuiCommandOutput {
    success: bool,
    exit_code: Option<i32>,
    stdout: String,
    stderr: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TimelineStatus {
    Running,
    Output,
    Completed,
    Failed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FocusedPanel {
    Conversation,
    Sidebar,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SelectionPanel {
    Conversation,
    Sidebar,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TextSelectionPoint {
    pub line: usize,
    pub column: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TextSelection {
    pub panel: SelectionPanel,
    pub anchor: TextSelectionPoint,
    pub focus: TextSelectionPoint,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TuiViewport {
    pub width: u16,
    pub height: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PanelGeometry {
    panel: SelectionPanel,
    x: u16,
    y: u16,
    width: u16,
    height: u16,
}

impl AppModel {
    #[must_use]
    pub fn new(workspace_root: impl Into<PathBuf>) -> Self {
        Self {
            workspace_root: workspace_root.into(),
            should_quit: false,
            messages: Vec::new(),
            message_render_keys: Vec::new(),
            next_message_id: 1,
            streaming_assistant_message: None,
            last_agent_event_sequence: 0,
            run_status: TuiRunStatus::Idle,
            session_id: None,
            run_id: None,
            run_elapsed_millis: None,
            estimated_input_tokens: 0,
            estimated_output_tokens: 0,
            timeline: Vec::new(),
            steps: Vec::new(),
            pending_human_input: None,
            runs: Vec::new(),
            sessions: Vec::new(),
            session_histories: Vec::new(),
            pending_new_session: false,
            code_change_review: None,
            code_change_result: None,
            audit_items: Vec::new(),
            skills: Vec::new(),
            skill_overrides: Vec::new(),
            selected_skill_names: Vec::new(),
            active_skill_names: Vec::new(),
            runtime_info: TuiRuntimeInfo::default(),
            runtime_logs: Vec::new(),
            indexed_files: Vec::new(),
            file_index_revision: 0,
            update_notice: None,
        }
    }

    #[must_use]
    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    pub fn message_revision(&self, index: usize) -> Option<u64> {
        self.message_render_keys.get(index).map(|key| key.revision)
    }

    pub(crate) fn message_render_key(&self, index: usize) -> MessageRenderKey {
        self.message_render_keys[index]
    }

    fn push_message(&mut self, message: Message) {
        self.messages.push(message);
        self.message_render_keys.push(MessageRenderKey {
            id: self.next_message_id,
            revision: 0,
        });
        self.next_message_id = self.next_message_id.saturating_add(1);
    }

    fn replace_messages(&mut self, messages: Vec<Message>) {
        self.clear_messages();
        for message in messages {
            self.push_message(message);
        }
    }

    fn clear_messages(&mut self) {
        self.messages.clear();
        self.message_render_keys.clear();
    }

    fn remove_message(&mut self, index: usize) {
        self.messages.remove(index);
        self.message_render_keys.remove(index);
    }

    fn append_message_content(&mut self, index: usize, content: &str) {
        self.messages[index].content.push_str(content);
        self.message_render_keys[index].revision =
            self.message_render_keys[index].revision.saturating_add(1);
    }

    pub(crate) fn scroll_lines(&self) -> u16 {
        self.runtime_info.scroll_lines
    }

    pub(crate) fn indexed_files(&self) -> &[String] {
        &self.indexed_files
    }

    #[must_use]
    pub fn run_status(&self) -> TuiRunStatus {
        self.run_status
    }

    #[must_use]
    pub fn session_id(&self) -> Option<&str> {
        self.session_id.as_deref()
    }

    #[must_use]
    pub fn session_name(&self) -> Option<&str> {
        if self.pending_new_session {
            return Some("New session");
        }
        let active_id = self.session_id.as_deref()?;
        self.sessions
            .iter()
            .find(|session| session.id.as_str() == active_id)
            .and_then(|session| session.name.as_deref())
    }

    #[must_use]
    pub fn run_id(&self) -> Option<&str> {
        self.run_id.as_deref()
    }

    #[must_use]
    pub fn run_elapsed_millis(&self) -> Option<u128> {
        self.run_elapsed_millis
    }

    pub fn estimated_token_usage(&self) -> (u64, u64) {
        (self.estimated_input_tokens, self.estimated_output_tokens)
    }

    fn begin_token_estimate(&mut self, request: &str) {
        self.estimated_input_tokens = estimate_tokens(request);
        self.estimated_output_tokens = 0;
        self.last_agent_event_sequence = 0;
        self.streaming_assistant_message = None;
    }

    #[must_use]
    pub fn timeline(&self) -> &[TimelineItem] {
        &self.timeline
    }

    #[must_use]
    pub fn steps(&self) -> &[Step] {
        &self.steps
    }

    #[must_use]
    pub fn pending_human_input(&self) -> Option<&HumanInputRequest> {
        self.pending_human_input.as_ref()
    }

    #[must_use]
    pub fn runs(&self) -> &[Run] {
        &self.runs
    }

    #[must_use]
    pub fn sessions(&self) -> &[Session] {
        &self.sessions
    }

    #[must_use]
    pub fn session_history(&self, session_id: &SessionId) -> Option<&SessionHistory> {
        self.session_histories
            .iter()
            .find(|history| &history.session_id == session_id)
    }

    pub fn pending_new_session(&self) -> bool {
        self.pending_new_session
    }

    #[must_use]
    pub fn code_change_review(&self) -> Option<&CodeChangeReview> {
        self.code_change_review.as_ref()
    }

    #[must_use]
    pub fn code_change_result(&self) -> Option<&TuiCodeChangeResult> {
        self.code_change_result.as_ref()
    }

    #[must_use]
    pub fn audit_items(&self) -> &[AuditItem] {
        &self.audit_items
    }

    pub fn set_skill_catalog(&mut self, catalog: SkillCatalog) {
        self.skills = catalog.skills;
        self.skill_overrides = catalog.overrides;
        self.selected_skill_names
            .retain(|name| self.skills.iter().any(|skill| &skill.name == name));
    }

    #[must_use]
    pub fn skills(&self) -> &[SkillDefinition] {
        &self.skills
    }

    #[must_use]
    pub fn skill_overrides(&self) -> &[SkillOverride] {
        &self.skill_overrides
    }

    #[must_use]
    pub fn selected_skill_names(&self) -> &[String] {
        &self.selected_skill_names
    }

    #[must_use]
    pub fn active_skill_names(&self) -> &[String] {
        &self.active_skill_names
    }

    pub fn set_runtime_info(&mut self, runtime_info: TuiRuntimeInfo) {
        self.runtime_logs.push(TuiRuntimeLog {
            title: "TUI initialized".to_owned(),
            detail: runtime_info
                .workspace_id
                .as_ref()
                .map(|id| format!("Workspace {id}")),
        });
        if let Some(error) = &runtime_info.config_error {
            self.runtime_logs.push(TuiRuntimeLog {
                title: "Configuration error".to_owned(),
                detail: Some(error.clone()),
            });
        }
        self.runtime_info = runtime_info;
    }

    #[must_use]
    pub fn runtime_info(&self) -> &TuiRuntimeInfo {
        &self.runtime_info
    }

    #[must_use]
    pub(super) fn theme(&self) -> TuiTheme {
        self.runtime_info.theme
    }

    #[must_use]
    pub(super) fn quit_shortcut_label(&self) -> &'static str {
        match self.runtime_info.shortcuts.quit {
            QuitShortcut::CtrlC => "Ctrl+C",
            QuitShortcut::CtrlQ => "Ctrl+Q",
        }
    }

    #[must_use]
    pub(super) fn copy_shortcut_label(&self) -> &'static str {
        match self.runtime_info.shortcuts.copy_message {
            CopyMessageShortcut::CtrlY => "Ctrl+Y",
            CopyMessageShortcut::CtrlShiftC => "Ctrl+Shift+C",
        }
    }

    #[must_use]
    pub fn runtime_logs(&self) -> &[TuiRuntimeLog] {
        &self.runtime_logs
    }

    #[must_use]
    pub fn update_notice(&self) -> Option<&UpdateNotice> {
        self.update_notice.as_ref()
    }
}

pub fn load_active_session_history(
    state: &mut AppModel,
    store: &SqliteWorkspaceStore,
) -> Result<(), StoreError> {
    let session = store.load_active_session()?;
    let sessions = store.load_sessions()?;
    let session_histories = sessions
        .iter()
        .map(|session| {
            Ok(SessionHistory {
                session_id: session.id.clone(),
                runs: store.load_session_runs(&session.id)?,
            })
        })
        .collect::<Result<Vec<_>, StoreError>>()?;
    let mut runs = store.load_session_runs(&session.id)?;
    let mut steps = store.load_session_steps(&session.id)?;
    runs.sort_by_key(|run| run.id.to_string());
    steps.sort_by_key(|step| step.id.to_string());

    state.session_id = Some(session.id.to_string());
    state.pending_new_session = false;
    state.sessions = sessions;
    state.session_histories = session_histories;
    state.replace_messages(messages_from_steps(&steps));
    state.steps = steps;
    state.timeline = command_timeline_from_steps(&state.steps);
    state.audit_items = audit_items_from_steps(&state.steps);
    state.runs = runs;
    state.run_status = TuiRunStatus::Idle;
    state.run_id = None;
    state.run_elapsed_millis = None;
    state.pending_human_input = None;
    restore_latest_run(state);
    Ok(())
}

pub fn execute_session_command(
    state: &mut AppModel,
    store: &SqliteWorkspaceStore,
    command: &AppCommand,
) -> Result<bool, StoreError> {
    match command {
        AppCommand::CreateSession { name } => {
            let session = store.create_session(name.clone())?;
            store.set_active_session(&session.id)?;
        }
        AppCommand::RenameActiveSession { name } => {
            let session = store.load_active_session()?;
            store.rename_session(&session.id, Some(name.clone()))?;
        }
        AppCommand::RenameSession { session_id, name } => {
            store.rename_session(session_id, Some(name.clone()))?;
        }
        AppCommand::ToggleSessionLiked { session_id } => {
            store.toggle_session_liked(session_id)?;
        }
        AppCommand::ArchiveSession { session_id } => {
            store.set_session_archived(session_id, true)?;
            if state.session_id.as_deref() == Some(session_id.as_str()) {
                let replacement = store.create_session(None)?;
                store.set_active_session(&replacement.id)?;
            }
        }
        AppCommand::DeleteSession { session_id } => {
            if state.session_id.as_deref() == Some(session_id.as_str()) {
                let replacement = store.create_session(None)?;
                store.set_active_session(&replacement.id)?;
            }
            store.delete_session(session_id)?;
        }
        AppCommand::SwitchSession { session_id } => {
            store.set_active_session(session_id)?;
        }
        AppCommand::StartRun { .. }
        | AppCommand::CancelRun
        | AppCommand::AcceptCodeChange
        | AppCommand::RejectCodeChange
        | AppCommand::RequestCodeChanges { .. }
        | AppCommand::CopyText { .. } => return Ok(false),
    }
    load_active_session_history(state, store)?;
    Ok(true)
}

pub fn materialize_pending_new_session(
    state: &mut AppModel,
    store: &SqliteWorkspaceStore,
    request: &str,
) -> Result<(), StoreError> {
    if !state.pending_new_session {
        return Ok(());
    }
    let session = store.create_session(None)?;
    store.set_active_session(&session.id)?;
    load_active_session_history(state, store)?;
    state.push_message(Message {
        role: MessageRole::User,
        content: request.to_owned(),
    });
    state.run_status = TuiRunStatus::Running;
    Ok(())
}

pub fn assign_default_session_title(
    state: &mut AppModel,
    store: &SqliteWorkspaceStore,
    request: &str,
) -> Result<(), StoreError> {
    let Some(session_id) = state.session_id.clone() else {
        return Ok(());
    };
    let Some(session) = state
        .sessions
        .iter_mut()
        .find(|session| session.id.as_str() == session_id)
    else {
        return Ok(());
    };
    if session
        .name
        .as_deref()
        .is_some_and(|name| name != "default")
    {
        return Ok(());
    }
    let title = controller::generated_session_title(request);
    *session = store.rename_session(&session.id, Some(title))?;
    Ok(())
}

pub fn execute_code_change_command(
    state: &mut AppModel,
    command: &AppCommand,
) -> Result<bool, CodeChangeError> {
    match command {
        AppCommand::AcceptCodeChange => {
            let workspace_root = state.workspace_root.clone();
            let result = {
                let review = state
                    .code_change_review
                    .as_mut()
                    .ok_or(CodeChangeError::InvalidReviewState)?;
                apply_code_change_review(review, &workspace_root)?
            };
            append_code_change_result_audit(state, &result);
            state.code_change_result = Some(result);
            Ok(true)
        }
        AppCommand::RejectCodeChange => {
            let review = state
                .code_change_review
                .as_mut()
                .ok_or(CodeChangeError::InvalidReviewState)?;
            review.reject()?;
            state.code_change_result = Some(TuiCodeChangeResult::Rejected);
            state.audit_items.push(AuditItem {
                title: "Review: Rejected".to_owned(),
                detail: None,
            });
            Ok(true)
        }
        AppCommand::RequestCodeChanges { feedback } => {
            let review = state
                .code_change_review
                .as_mut()
                .ok_or(CodeChangeError::InvalidReviewState)?;
            review.request_changes(feedback.clone())?;
            state.code_change_result = Some(TuiCodeChangeResult::ChangesRequested);
            state.audit_items.push(AuditItem {
                title: "Review: Changes requested".to_owned(),
                detail: Some(feedback.clone()),
            });
            state.run_status = TuiRunStatus::Running;
            state.push_message(Message {
                role: MessageRole::User,
                content: feedback.clone(),
            });
            Ok(false)
        }
        _ => Ok(false),
    }
}

fn messages_from_steps(steps: &[Step]) -> Vec<Message> {
    let mut messages = Vec::new();
    for step in steps {
        match &step.payload {
            StepPayload::UserMessage { content } => messages.push(Message {
                role: MessageRole::User,
                content: content.clone(),
            }),
            StepPayload::AssistantResponse { items, .. } => {
                let content = items
                    .iter()
                    .filter_map(|item| match item {
                        AssistantResponseItem::Text { content } => Some(content.as_str()),
                        _ => None,
                    })
                    .collect::<String>();
                if !content.is_empty() {
                    messages.push(Message {
                        role: MessageRole::Assistant,
                        content,
                    });
                }
            }
            StepPayload::AssistantOutputCheckpoint { content } if !content.is_empty() => {
                messages.push(Message {
                    role: MessageRole::Assistant,
                    content: content.clone(),
                });
            }
            StepPayload::Error { message } => messages.push(Message {
                role: MessageRole::Error,
                content: message.clone(),
            }),
            _ => {}
        }
    }
    messages
}

fn input_history_from_steps(steps: &[Step]) -> Vec<String> {
    let mut history = Vec::new();
    for step in steps {
        let StepPayload::UserMessage { content } = &step.payload else {
            continue;
        };
        if history.last() != Some(content) {
            history.push(content.clone());
        }
    }
    history
}

fn input_history_from_runs(runs: &[Run]) -> Vec<String> {
    let mut history = Vec::new();
    for run in runs {
        if history.last() != Some(&run.request) {
            history.push(run.request.clone());
        }
    }
    history
}

pub(crate) fn restored_input_history(state: &AppModel) -> Vec<String> {
    let mut history = input_history_from_runs(&state.runs);
    if history.is_empty() {
        history = input_history_from_steps(&state.steps);
    }
    history
}

fn estimate_tokens(content: &str) -> u64 {
    let units = content
        .chars()
        .map(|character| if character.is_ascii() { 1 } else { 2 })
        .sum::<u64>();
    units.div_ceil(4)
}

fn fuzzy_path_match(path: &str, query: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    let path = path.to_lowercase();
    if path.contains(query) {
        return true;
    }
    let mut characters = path.chars();
    query.chars().all(|query_character| {
        characters
            .by_ref()
            .any(|character| character == query_character)
    })
}

fn reference_ranges(input: &str) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    let mut offset = 0;
    while let Some(relative) = input[offset..].find('@') {
        let start = offset + relative;
        let remainder = &input[start..];
        let end = if let Some(braced) = remainder.strip_prefix("@{") {
            braced.find('}').map(|index| start + 2 + index + 1)
        } else {
            Some(
                remainder
                    .char_indices()
                    .skip(1)
                    .find(|(_, character)| character.is_whitespace())
                    .map_or(input.len(), |(index, _)| start + index),
            )
        };
        let Some(end) = end else {
            break;
        };
        ranges.push((start, end));
        offset = end.max(start + 1);
    }
    ranges
}

fn command_timeline_from_steps(steps: &[Step]) -> Vec<TimelineItem> {
    let mut timeline = Vec::new();
    let mut command_indexes = HashMap::new();
    let mut message_count = 0;
    for step in steps {
        match &step.payload {
            StepPayload::UserMessage { .. } | StepPayload::Error { .. } => {
                message_count += 1;
            }
            StepPayload::AssistantResponse { items, .. } => {
                if !assistant_response_text(items).is_empty() {
                    message_count += 1;
                }
                for item in items {
                    let AssistantResponseItem::ToolCall {
                        id,
                        name,
                        arguments,
                        ..
                    } = item
                    else {
                        continue;
                    };
                    if name != "exec" {
                        continue;
                    }
                    let Some(command) = command_from_arguments(arguments) else {
                        continue;
                    };
                    command_indexes.insert(id.clone(), timeline.len());
                    timeline.push(TimelineItem {
                        id: format!("persisted:{id}"),
                        message_count,
                        label: "Tool: exec".to_owned(),
                        status: TimelineStatus::Running,
                        detail: None,
                        arguments: None,
                        output: None,
                        command: Some(command),
                    });
                }
            }
            StepPayload::AssistantOutputCheckpoint { content } => {
                if !content.is_empty() {
                    message_count += 1;
                }
            }
            StepPayload::ToolResult { id, content, .. } => {
                let Some(index) = command_indexes.get(id).copied() else {
                    continue;
                };
                let item = &mut timeline[index];
                if let Some(output) = command_from_output(content) {
                    item.command = merge_command_execution(item.command.take(), Some(output));
                    item.status = TimelineStatus::Completed;
                } else {
                    item.status = TimelineStatus::Failed;
                    item.detail = Some(format_json(content));
                }
            }
            _ => {}
        }
    }
    timeline
}

fn assistant_response_text(items: &[AssistantResponseItem]) -> String {
    items
        .iter()
        .filter_map(|item| match item {
            AssistantResponseItem::Text { content } => Some(content.as_str()),
            _ => None,
        })
        .collect()
}

fn audit_items_from_steps(steps: &[Step]) -> Vec<AuditItem> {
    let mut items = Vec::new();
    let mut tool_names = HashMap::new();
    for step in steps {
        match &step.payload {
            StepPayload::UserMessage { content } => items.push(AuditItem {
                title: "User request".to_owned(),
                detail: Some(content.clone()),
            }),
            StepPayload::AssistantResponse {
                items: response_items,
                ..
            } => {
                for item in response_items {
                    let AssistantResponseItem::ToolCall {
                        id,
                        name,
                        arguments,
                        ..
                    } = item
                    else {
                        continue;
                    };
                    tool_names.insert(id.clone(), name.clone());
                    items.push(audit_tool_call(name, arguments));
                }
            }
            StepPayload::ToolResult { id, content, .. } => {
                let name = tool_names.get(id).map(String::as_str).unwrap_or("unknown");
                items.push(AuditItem {
                    title: format!("Tool result: {name}"),
                    detail: Some("Result recorded".to_owned()),
                });
                if name == PROPOSE_CODE_CHANGE_TOOL_NAME
                    && let Ok(proposal) =
                        serde_json::from_value::<CodeChangeProposal>(content.clone())
                {
                    append_proposal_audit(&mut items, &proposal);
                }
            }
            StepPayload::Error { message } => items.push(AuditItem {
                title: format!("Error: {message}"),
                detail: None,
            }),
            _ => {}
        }
    }
    items
}

fn audit_tool_call(name: &str, arguments: &serde_json::Value) -> AuditItem {
    if name == "exec"
        && let Some(program) = arguments.get("program").and_then(serde_json::Value::as_str)
    {
        let action = match program {
            "cat" => "File read",
            "rg" | "find" => "File search",
            _ => "Tool call",
        };
        let detail = arguments
            .get("args")
            .and_then(serde_json::Value::as_array)
            .map(|args| {
                args.iter()
                    .filter_map(serde_json::Value::as_str)
                    .collect::<Vec<_>>()
                    .join(" ")
            });
        return AuditItem {
            title: action.to_owned(),
            detail,
        };
    }
    AuditItem {
        title: format!("Tool call: {name}"),
        detail: None,
    }
}

fn append_proposal_audit(items: &mut Vec<AuditItem>, proposal: &CodeChangeProposal) {
    items.push(AuditItem {
        title: format!("Policy: {:?}", proposal.policy),
        detail: None,
    });
    for file in &proposal.files {
        items.push(AuditItem {
            title: format!("File proposed: {}", file.path.as_str()),
            detail: None,
        });
    }
}

fn append_code_change_result_audit(state: &mut AppModel, result: &TuiCodeChangeResult) {
    match result {
        TuiCodeChangeResult::Applied { .. } => {
            let paths = state
                .code_change_review
                .iter()
                .flat_map(|review| &review.proposal.files)
                .map(|file| file.path.as_str().to_owned())
                .collect::<Vec<_>>();
            state
                .audit_items
                .extend(paths.into_iter().map(|path| AuditItem {
                    title: format!("File write: {path}"),
                    detail: Some("Applied".to_owned()),
                }));
        }
        TuiCodeChangeResult::Conflict { paths } => {
            state.audit_items.extend(paths.iter().map(|path| AuditItem {
                title: format!("File conflict: {path}"),
                detail: None,
            }));
        }
        TuiCodeChangeResult::Denied => state.audit_items.push(AuditItem {
            title: "Policy: Deny".to_owned(),
            detail: Some("Change not applied".to_owned()),
        }),
        TuiCodeChangeResult::Rejected | TuiCodeChangeResult::ChangesRequested => {}
    }
}

fn restore_latest_run(state: &mut AppModel) {
    let Some(run) = state.runs.last() else {
        return;
    };
    state.run_status = tui_run_status(&run.status);
    state.run_id = Some(run.id.to_string());
    state.run_elapsed_millis = Some(run.updated_at.saturating_sub(run.created_at));
    state.pending_human_input = (run.status == RunStatus::WaitingForHumanInput)
        .then(|| latest_human_input_request(&state.steps))
        .flatten();
}

fn tui_run_status(status: &RunStatus) -> TuiRunStatus {
    match status {
        RunStatus::Running => TuiRunStatus::Running,
        RunStatus::WaitingForHumanInput => TuiRunStatus::WaitingForHumanInput,
        RunStatus::Completed => TuiRunStatus::Completed,
        RunStatus::Failed => TuiRunStatus::Failed,
        RunStatus::Canceled => TuiRunStatus::Canceled,
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum AppMsg {
    FileIndexUpdated(FileIndexSnapshot),
    UpdateAvailable {
        notice: UpdateNotice,
        show_startup_message: bool,
    },
    AssistantMessage {
        content: String,
    },
    RunFinished {
        response: RunResponse,
    },
    RunFailed {
        error: String,
    },
    RunCanceled,
    AgentEvent(AgentEvent),
    CodeChangeProposed {
        proposal: CodeChangeProposal,
    },
}
