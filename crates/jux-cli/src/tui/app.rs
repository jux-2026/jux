use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use jux_core::{
    AgentEvent, AgentEventData, AgentEventKind, AssistantResponseItem, CALL_SKILL_TOOL_NAME,
    CodeChangeError, CodeChangeProposal, CodeChangeReview, HumanInputRequest,
    PROPOSE_CODE_CHANGE_TOOL_NAME, ReviewStatus, Run, RunStatus, Session, SessionId, SkillCatalog,
    SkillDefinition, SkillOverride, SqliteWorkspaceStore, Step, StepPayload, StoreError,
    latest_human_input_request,
};
use std::collections::HashMap;
use std::path::PathBuf;

use super::RunResponse;

#[derive(Clone, Debug, PartialEq)]
pub struct AppState {
    pub workspace_root: PathBuf,
    pub should_quit: bool,
    input: String,
    cursor: usize,
    messages: Vec<Message>,
    message_scroll: u16,
    help_visible: bool,
    run_status: TuiRunStatus,
    session_id: Option<String>,
    run_id: Option<String>,
    run_elapsed_millis: Option<u128>,
    timeline: Vec<TimelineItem>,
    selected_timeline: Option<usize>,
    steps: Vec<Step>,
    pending_human_input: Option<HumanInputRequest>,
    selected_human_option: usize,
    human_input_error: Option<String>,
    runs: Vec<Run>,
    sessions: Vec<Session>,
    session_histories: Vec<SessionHistory>,
    session_panel_visible: bool,
    code_change_review: Option<CodeChangeReview>,
    selected_changed_file: usize,
    code_change_result: Option<TuiCodeChangeResult>,
    audit_items: Vec<AuditItem>,
    audit_panel_visible: bool,
    skills: Vec<SkillDefinition>,
    skill_overrides: Vec<SkillOverride>,
    selected_skill: usize,
    selected_skill_names: Vec<String>,
    active_skill_names: Vec<String>,
    skill_panel_visible: bool,
    runtime_info: TuiRuntimeInfo,
    runtime_logs: Vec<TuiRuntimeLog>,
    log_panel_visible: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Message {
    pub role: MessageRole,
    pub content: String,
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
    pub label: String,
    pub status: TimelineStatus,
    pub detail: Option<String>,
    pub arguments: Option<String>,
    pub output: Option<String>,
    pub expanded: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TimelineStatus {
    Running,
    Output,
    Completed,
    Failed,
}

impl AppState {
    #[must_use]
    pub fn new(workspace_root: impl Into<PathBuf>) -> Self {
        Self {
            workspace_root: workspace_root.into(),
            should_quit: false,
            input: String::new(),
            cursor: 0,
            messages: Vec::new(),
            message_scroll: 0,
            help_visible: false,
            run_status: TuiRunStatus::Idle,
            session_id: None,
            run_id: None,
            run_elapsed_millis: None,
            timeline: Vec::new(),
            selected_timeline: None,
            steps: Vec::new(),
            pending_human_input: None,
            selected_human_option: 0,
            human_input_error: None,
            runs: Vec::new(),
            sessions: Vec::new(),
            session_histories: Vec::new(),
            session_panel_visible: false,
            code_change_review: None,
            selected_changed_file: 0,
            code_change_result: None,
            audit_items: Vec::new(),
            audit_panel_visible: false,
            skills: Vec::new(),
            skill_overrides: Vec::new(),
            selected_skill: 0,
            selected_skill_names: Vec::new(),
            active_skill_names: Vec::new(),
            skill_panel_visible: false,
            runtime_info: TuiRuntimeInfo::default(),
            runtime_logs: Vec::new(),
            log_panel_visible: false,
        }
    }

    #[must_use]
    pub fn input_text(&self) -> &str {
        &self.input
    }

    #[must_use]
    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    #[must_use]
    pub fn message_scroll(&self) -> u16 {
        self.message_scroll
    }

    #[must_use]
    pub fn help_visible(&self) -> bool {
        self.help_visible
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
    pub fn run_id(&self) -> Option<&str> {
        self.run_id.as_deref()
    }

    #[must_use]
    pub fn run_elapsed_millis(&self) -> Option<u128> {
        self.run_elapsed_millis
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

    #[must_use]
    pub fn session_panel_visible(&self) -> bool {
        self.session_panel_visible
    }

    #[must_use]
    pub fn code_change_review(&self) -> Option<&CodeChangeReview> {
        self.code_change_review.as_ref()
    }

    #[must_use]
    pub fn selected_changed_file(&self) -> usize {
        self.selected_changed_file
    }

    #[must_use]
    pub fn code_change_result(&self) -> Option<&TuiCodeChangeResult> {
        self.code_change_result.as_ref()
    }

    #[must_use]
    pub fn audit_items(&self) -> &[AuditItem] {
        &self.audit_items
    }

    #[must_use]
    pub fn audit_panel_visible(&self) -> bool {
        self.audit_panel_visible
    }

    pub fn set_skill_catalog(&mut self, catalog: SkillCatalog) {
        self.skills = catalog.skills;
        self.skill_overrides = catalog.overrides;
        self.selected_skill = 0;
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
    pub fn selected_skill(&self) -> usize {
        self.selected_skill
    }

    #[must_use]
    pub fn selected_skill_names(&self) -> &[String] {
        &self.selected_skill_names
    }

    #[must_use]
    pub fn active_skill_names(&self) -> &[String] {
        &self.active_skill_names
    }

    #[must_use]
    pub fn skill_panel_visible(&self) -> bool {
        self.skill_panel_visible
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
    pub fn runtime_logs(&self) -> &[TuiRuntimeLog] {
        &self.runtime_logs
    }

    #[must_use]
    pub fn log_panel_visible(&self) -> bool {
        self.log_panel_visible
    }

    #[must_use]
    pub fn selected_human_option(&self) -> usize {
        self.selected_human_option
    }

    #[must_use]
    pub fn human_input_error(&self) -> Option<&str> {
        self.human_input_error.as_deref()
    }

    fn clear_input(&mut self) {
        self.input.clear();
        self.cursor = 0;
    }

    fn insert(&mut self, character: char) {
        self.input.insert(self.cursor, character);
        self.cursor += character.len_utf8();
    }

    fn move_cursor_left(&mut self) {
        let Some((index, _)) = self.input[..self.cursor].char_indices().next_back() else {
            return;
        };
        self.cursor = index;
    }

    fn move_cursor_right(&mut self) {
        let Some(character) = self.input[self.cursor..].chars().next() else {
            return;
        };
        self.cursor += character.len_utf8();
    }

    fn move_cursor_up(&mut self) {
        let line_start = self.input[..self.cursor]
            .rfind('\n')
            .map_or(0, |index| index + 1);
        if line_start == 0 {
            return;
        }
        let column = self.input[line_start..self.cursor].chars().count();
        let previous_end = line_start - 1;
        let previous_start = self.input[..previous_end]
            .rfind('\n')
            .map_or(0, |index| index + 1);
        self.cursor = byte_at_character_column(&self.input, previous_start, previous_end, column);
    }

    fn move_cursor_down(&mut self) {
        let line_start = self.input[..self.cursor]
            .rfind('\n')
            .map_or(0, |index| index + 1);
        let column = self.input[line_start..self.cursor].chars().count();
        let Some(relative_line_end) = self.input[self.cursor..].find('\n') else {
            return;
        };
        let next_start = self.cursor + relative_line_end + 1;
        let next_end = self.input[next_start..]
            .find('\n')
            .map_or(self.input.len(), |index| next_start + index);
        self.cursor = byte_at_character_column(&self.input, next_start, next_end, column);
    }

    fn delete_before_cursor(&mut self) {
        let Some((index, _)) = self.input[..self.cursor].char_indices().next_back() else {
            return;
        };
        self.input.drain(index..self.cursor);
        self.cursor = index;
    }

    fn delete_at_cursor(&mut self) {
        let Some(character) = self.input[self.cursor..].chars().next() else {
            return;
        };
        self.input
            .drain(self.cursor..self.cursor + character.len_utf8());
    }
}

pub fn load_active_session_history(
    state: &mut AppState,
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
    state.sessions = sessions;
    state.session_histories = session_histories;
    state.messages = messages_from_steps(&steps);
    state.steps = steps;
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
    state: &mut AppState,
    store: &SqliteWorkspaceStore,
    command: &AppCommand,
) -> Result<bool, StoreError> {
    match command {
        AppCommand::CreateSession { name } => {
            let session = store.create_session(Some(name.clone()))?;
            store.set_active_session(&session.id)?;
        }
        AppCommand::RenameActiveSession { name } => {
            let session = store.load_active_session()?;
            store.rename_session(&session.id, Some(name.clone()))?;
        }
        AppCommand::SwitchSession { session_id } => {
            store.set_active_session(session_id)?;
        }
        AppCommand::StartRun { .. }
        | AppCommand::CancelRun
        | AppCommand::AcceptCodeChange
        | AppCommand::RejectCodeChange
        | AppCommand::RequestCodeChanges { .. } => return Ok(false),
    }
    load_active_session_history(state, store)?;
    state.session_panel_visible = true;
    Ok(true)
}

pub fn execute_code_change_command(
    state: &mut AppState,
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
            state.messages.push(Message {
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
            StepPayload::Error { message } => messages.push(Message {
                role: MessageRole::Error,
                content: message.clone(),
            }),
            _ => {}
        }
    }
    messages
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

fn append_code_change_result_audit(state: &mut AppState, result: &TuiCodeChangeResult) {
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

fn restore_latest_run(state: &mut AppState) {
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
pub enum AppAction {
    Key(KeyEvent),
    AssistantMessage { content: String },
    RunFinished { response: RunResponse },
    RunFailed { error: String },
    RunCanceled,
    AgentEvent(AgentEvent),
    CodeChangeProposed { proposal: CodeChangeProposal },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AppCommand {
    StartRun { request: String },
    CancelRun,
    CreateSession { name: String },
    RenameActiveSession { name: String },
    SwitchSession { session_id: SessionId },
    AcceptCodeChange,
    RejectCodeChange,
    RequestCodeChanges { feedback: String },
}

pub fn update(state: &mut AppState, action: AppAction) -> Option<AppCommand> {
    match action {
        AppAction::CodeChangeProposed { proposal } => {
            append_proposal_audit(&mut state.audit_items, &proposal);
            state.code_change_review = Some(CodeChangeReview::new(proposal));
            state.selected_changed_file = 0;
            state.code_change_result = None;
            None
        }
        AppAction::AgentEvent(event) => {
            state.runtime_logs.push(TuiRuntimeLog {
                title: format!("Agent event: {:?}", event.kind),
                detail: Some(event.id.to_string()),
            });
            if let AgentEventData::SkillsSelected { skills } = &event.data {
                state.active_skill_names = skills.clone();
            }
            if let AgentEventData::ToolOutput { name, content } = &event.data
                && name == PROPOSE_CODE_CHANGE_TOOL_NAME
                && let Ok(proposal) = serde_json::from_value::<CodeChangeProposal>(content.clone())
            {
                append_proposal_audit(&mut state.audit_items, &proposal);
                state.code_change_review = Some(CodeChangeReview::new(proposal));
                state.selected_changed_file = 0;
                state.code_change_result = None;
            }
            if let Some(mut item) = timeline_item_from_agent_event(event) {
                match state
                    .timeline
                    .iter()
                    .position(|existing| existing.id == item.id)
                {
                    Some(index) => {
                        if item.label == "Skill"
                            && state.timeline[index].label.starts_with("Skill:")
                        {
                            item.label = state.timeline[index].label.clone();
                        }
                        if item.detail.is_none() {
                            item.detail = state.timeline[index].detail.clone();
                        }
                        if item.arguments.is_none() {
                            item.arguments = state.timeline[index].arguments.clone();
                        }
                        if item.output.is_none() {
                            item.output = state.timeline[index].output.clone();
                        }
                        item.expanded = state.timeline[index].expanded;
                        state.timeline[index] = item;
                    }
                    None => state.timeline.push(item),
                }
            }
            None
        }
        AppAction::AssistantMessage { content } => {
            state.messages.push(Message {
                role: MessageRole::Assistant,
                content,
            });
            None
        }
        AppAction::RunFinished { response } => {
            let waiting_for_human_input = response.status == RunStatus::WaitingForHumanInput;
            state.run_status = match response.status {
                RunStatus::Running => TuiRunStatus::Running,
                RunStatus::WaitingForHumanInput => TuiRunStatus::WaitingForHumanInput,
                RunStatus::Completed => TuiRunStatus::Completed,
                RunStatus::Failed => TuiRunStatus::Failed,
                RunStatus::Canceled => TuiRunStatus::Canceled,
            };
            state.session_id = Some(response.session_id);
            state.run_id = Some(response.run_id);
            state.run_elapsed_millis =
                Some(response.updated_at.saturating_sub(response.created_at));
            state.steps = response.steps;
            state.steps.sort_by_key(|step| step.id.to_string());
            state.audit_items = audit_items_from_steps(&state.steps);
            state.pending_human_input = waiting_for_human_input
                .then(|| latest_human_input_request(&state.steps))
                .flatten();
            state.selected_human_option = 0;
            state.human_input_error = None;
            if let Some(content) = response.answer {
                state.messages.push(Message {
                    role: MessageRole::Assistant,
                    content,
                });
            }
            state.runtime_logs.push(TuiRuntimeLog {
                title: format!("Run finished: {:?}", state.run_status),
                detail: state.run_id.clone(),
            });
            None
        }
        AppAction::RunFailed { error } => {
            state.run_status = TuiRunStatus::Failed;
            state.runtime_logs.push(TuiRuntimeLog {
                title: "Run failed".to_owned(),
                detail: Some(error.clone()),
            });
            state.messages.push(Message {
                role: MessageRole::Error,
                content: error,
            });
            None
        }
        AppAction::RunCanceled => {
            state.run_status = TuiRunStatus::Canceled;
            state.runtime_logs.push(TuiRuntimeLog {
                title: "Run canceled".to_owned(),
                detail: state.run_id.clone(),
            });
            None
        }
        AppAction::Key(KeyEvent {
            code: KeyCode::Esc, ..
        }) if state.log_panel_visible => {
            state.log_panel_visible = false;
            None
        }
        AppAction::Key(KeyEvent {
            code: KeyCode::Esc, ..
        }) if state.skill_panel_visible => {
            state.skill_panel_visible = false;
            None
        }
        AppAction::Key(KeyEvent {
            code: KeyCode::Esc, ..
        }) if state.run_status == TuiRunStatus::Running => Some(AppCommand::CancelRun),
        AppAction::Key(key)
            if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) =>
        {
            state.should_quit = true;
            None
        }
        AppAction::Key(key)
            if key.code == KeyCode::Enter && key.modifiers.contains(KeyModifiers::SHIFT) =>
        {
            state.insert('\n');
            None
        }
        AppAction::Key(KeyEvent {
            code: KeyCode::Tab, ..
        }) => {
            if !state.timeline.is_empty() {
                let next = state
                    .selected_timeline
                    .map_or(0, |index| (index + 1) % state.timeline.len());
                state.selected_timeline = Some(next);
            }
            None
        }
        AppAction::Key(KeyEvent {
            code: KeyCode::Char(' '),
            ..
        }) if state.skill_panel_visible => {
            if let Some(skill) = state.skills.get(state.selected_skill) {
                if let Some(index) = state
                    .selected_skill_names
                    .iter()
                    .position(|name| name == &skill.name)
                {
                    state.selected_skill_names.remove(index);
                } else {
                    state.selected_skill_names.push(skill.name.clone());
                }
            }
            None
        }
        AppAction::Key(KeyEvent {
            code: KeyCode::Char(' '),
            ..
        }) if state.selected_timeline.is_some() => {
            if let Some(item) = state
                .selected_timeline
                .and_then(|index| state.timeline.get_mut(index))
            {
                item.expanded = !item.expanded;
            }
            None
        }
        AppAction::Key(KeyEvent {
            code: KeyCode::Up, ..
        }) if state.skill_panel_visible && state.input.is_empty() => {
            if !state.skills.is_empty() {
                state.selected_skill = state
                    .selected_skill
                    .checked_sub(1)
                    .unwrap_or(state.skills.len() - 1);
            }
            None
        }
        AppAction::Key(KeyEvent {
            code: KeyCode::Down,
            ..
        }) if state.skill_panel_visible && state.input.is_empty() => {
            if !state.skills.is_empty() {
                state.selected_skill = (state.selected_skill + 1) % state.skills.len();
            }
            None
        }
        AppAction::Key(KeyEvent {
            code: KeyCode::Up, ..
        }) if state.run_status == TuiRunStatus::WaitingForHumanInput && state.input.is_empty() => {
            if let Some(request) = &state.pending_human_input
                && !request.options.is_empty()
            {
                state.selected_human_option = state
                    .selected_human_option
                    .checked_sub(1)
                    .unwrap_or(request.options.len() - 1);
            }
            None
        }
        AppAction::Key(KeyEvent {
            code: KeyCode::Down,
            ..
        }) if state.run_status == TuiRunStatus::WaitingForHumanInput && state.input.is_empty() => {
            if let Some(request) = &state.pending_human_input
                && !request.options.is_empty()
            {
                state.selected_human_option =
                    (state.selected_human_option + 1) % request.options.len();
            }
            None
        }
        AppAction::Key(KeyEvent {
            code: KeyCode::Left,
            ..
        }) if state.input.is_empty() && state.code_change_review.is_some() => {
            let file_count = state
                .code_change_review
                .as_ref()
                .map_or(0, |review| review.proposal.files.len());
            if file_count > 0 {
                state.selected_changed_file = state
                    .selected_changed_file
                    .checked_sub(1)
                    .unwrap_or(file_count - 1);
            }
            None
        }
        AppAction::Key(KeyEvent {
            code: KeyCode::Right,
            ..
        }) if state.input.is_empty() && state.code_change_review.is_some() => {
            let file_count = state
                .code_change_review
                .as_ref()
                .map_or(0, |review| review.proposal.files.len());
            if file_count > 0 {
                state.selected_changed_file = (state.selected_changed_file + 1) % file_count;
            }
            None
        }
        AppAction::Key(KeyEvent {
            code: KeyCode::Enter,
            ..
        }) => {
            if state.input.trim() == "/quit" {
                state.clear_input();
                state.should_quit = true;
                return None;
            }
            if state.input.trim() == "/clear" {
                state.clear_input();
                state.messages.clear();
                state.message_scroll = 0;
                return None;
            }
            if state.input.trim() == "/help" {
                state.clear_input();
                state.help_visible = true;
                return None;
            }
            if state.input.trim() == "/sessions" {
                state.clear_input();
                state.session_panel_visible = true;
                return None;
            }
            if state.input.trim() == "/audit" {
                state.clear_input();
                state.audit_panel_visible = true;
                return None;
            }
            if state.input.trim() == "/skills" {
                state.clear_input();
                state.skill_panel_visible = true;
                return None;
            }
            if state.input.trim() == "/logs" {
                state.clear_input();
                state.log_panel_visible = true;
                return None;
            }
            if state.input.trim() == "/review accept" && state.code_change_review.is_some() {
                state.clear_input();
                return Some(AppCommand::AcceptCodeChange);
            }
            if state.input.trim() == "/review reject" && state.code_change_review.is_some() {
                state.clear_input();
                return Some(AppCommand::RejectCodeChange);
            }
            if let Some(feedback) = state.input.trim().strip_prefix("/review changes ")
                && !feedback.is_empty()
                && state.code_change_review.is_some()
            {
                let feedback = feedback.to_owned();
                state.clear_input();
                return Some(AppCommand::RequestCodeChanges { feedback });
            }
            if state.run_status != TuiRunStatus::Running
                && let Some(name) = state.input.trim().strip_prefix("/session new ")
                && !name.is_empty()
            {
                let name = name.to_owned();
                state.clear_input();
                return Some(AppCommand::CreateSession { name });
            }
            if state.run_status != TuiRunStatus::Running
                && let Some(name) = state.input.trim().strip_prefix("/session rename ")
                && !name.is_empty()
            {
                let name = name.to_owned();
                state.clear_input();
                return Some(AppCommand::RenameActiveSession { name });
            }
            if state.run_status != TuiRunStatus::Running
                && let Some(id) = state.input.trim().strip_prefix("/session switch ")
                && !id.is_empty()
            {
                let session_id = SessionId::from(id.to_owned());
                state.clear_input();
                return Some(AppCommand::SwitchSession { session_id });
            }
            if state.input.is_empty()
                && let Some(request) = &state.pending_human_input
                && let Some(option) = request.options.get(state.selected_human_option)
            {
                state.input = option.id.clone();
                state.cursor = state.input.len();
            }
            if state.input.trim().is_empty() {
                return None;
            }
            if state.run_status == TuiRunStatus::WaitingForHumanInput
                && let Some(request) = &state.pending_human_input
                && let Err(error) = request.validate(state.input.trim())
            {
                state.human_input_error = Some(error);
                return None;
            }
            if state.run_status == TuiRunStatus::Running {
                return None;
            }
            let request = std::mem::take(&mut state.input);
            state.cursor = 0;
            state.run_status = TuiRunStatus::Running;
            state.pending_human_input = None;
            state.human_input_error = None;
            state.messages.push(Message {
                role: MessageRole::User,
                content: request.clone(),
            });
            Some(AppCommand::StartRun { request })
        }
        AppAction::Key(KeyEvent {
            code: KeyCode::Char(character),
            ..
        }) => {
            state.insert(character);
            None
        }
        AppAction::Key(KeyEvent {
            code: KeyCode::Left,
            ..
        }) => {
            state.move_cursor_left();
            None
        }
        AppAction::Key(KeyEvent {
            code: KeyCode::Right,
            ..
        }) => {
            state.move_cursor_right();
            None
        }
        AppAction::Key(KeyEvent {
            code: KeyCode::Up, ..
        }) => {
            state.move_cursor_up();
            None
        }
        AppAction::Key(KeyEvent {
            code: KeyCode::Down,
            ..
        }) => {
            state.move_cursor_down();
            None
        }
        AppAction::Key(KeyEvent {
            code: KeyCode::Backspace,
            ..
        }) => {
            state.delete_before_cursor();
            None
        }
        AppAction::Key(KeyEvent {
            code: KeyCode::Delete,
            ..
        }) => {
            state.delete_at_cursor();
            None
        }
        AppAction::Key(KeyEvent {
            code: KeyCode::PageUp,
            ..
        }) => {
            state.message_scroll = state.message_scroll.saturating_sub(10);
            None
        }
        AppAction::Key(KeyEvent {
            code: KeyCode::PageDown,
            ..
        }) => {
            state.message_scroll = state.message_scroll.saturating_add(10);
            None
        }
        AppAction::Key(_) => None,
    }
}

fn byte_at_character_column(input: &str, start: usize, end: usize, column: usize) -> usize {
    input[start..end]
        .char_indices()
        .nth(column)
        .map_or(end, |(index, _)| start + index)
}

fn timeline_item_from_agent_event(event: AgentEvent) -> Option<TimelineItem> {
    let (label, detail, arguments, output) = match event.data {
        AgentEventData::LlmStarted | AgentEventData::LlmCompleted => {
            ("LLM".to_owned(), None, None, None)
        }
        AgentEventData::LlmFailed { error } => ("LLM".to_owned(), Some(error), None, None),
        AgentEventData::SkillsSelected { skills } => (
            "Active skills".to_owned(),
            None,
            None,
            Some(skills.join(", ")),
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
            (label, None, Some(format_json(&arguments)), None)
        }
        AgentEventData::ToolCompleted { name, .. } => {
            (tool_or_skill_label(&name), None, None, None)
        }
        AgentEventData::ToolOutput { name, content } => {
            let output = (name != PROPOSE_CODE_CHANGE_TOOL_NAME).then(|| format_json(&content));
            (tool_or_skill_label(&name), None, None, output)
        }
        AgentEventData::ToolFailed { name, error, .. } => {
            (tool_or_skill_label(&name), Some(error), None, None)
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
        label,
        status,
        detail,
        arguments,
        output,
        expanded: false,
    })
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
