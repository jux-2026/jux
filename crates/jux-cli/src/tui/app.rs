use crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use jux_core::{
    AgentEvent, AgentEventData, AgentEventKind, AssistantResponseItem, CALL_SKILL_TOOL_NAME,
    CodeChangeError, CodeChangeProposal, CodeChangeReview, CopyMessageShortcut, HumanInputRequest,
    PROPOSE_CODE_CHANGE_TOOL_NAME, QuitShortcut, ReviewStatus, Run, RunStatus, Session, SessionId,
    SkillCatalog, SkillDefinition, SkillOverride, SqliteWorkspaceStore, Step, StepPayload,
    StoreError, ToolOutputStream, TuiShortcutConfig, TuiTheme, latest_human_input_request,
};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use unicode_width::UnicodeWidthStr;

use super::FileIndexSnapshot;
use super::RunResponse;

const DEFAULT_CONVERSATION_WIDTH_PERCENT: u16 = 60;
const MIN_PANEL_WIDTH: u16 = 20;
const DIVIDER_WIDTH: u16 = 1;
const ESCAPE_CONFIRMATION_WINDOW: Duration = Duration::from_secs(1);
const NOTIFICATION_DURATION: Duration = Duration::from_secs(2);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EscapeAction {
    ClearInput,
    InterruptRun,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PendingEscapeAction {
    action: EscapeAction,
    expires_at: Instant,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SlashCommand {
    NewSession,
    Session,
    Version,
    Retry,
    Continue,
}

enum SlashCommandExecution {
    NotSelected,
    Executed(Option<AppCommand>),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct SlashCommandDefinition {
    command: SlashCommand,
    pub name: &'static str,
    pub description: &'static str,
    pub usage: &'static str,
}

const SLASH_COMMANDS: [SlashCommandDefinition; 5] = [
    SlashCommandDefinition {
        command: SlashCommand::NewSession,
        name: "/new",
        description: "Start a new session",
        usage: "/new",
    },
    SlashCommandDefinition {
        command: SlashCommand::Session,
        name: "/session",
        description: "Switch active session",
        usage: "/session [new|rename|switch]",
    },
    SlashCommandDefinition {
        command: SlashCommand::Version,
        name: "/version",
        description: "Show the Jux version",
        usage: "/version",
    },
    SlashCommandDefinition {
        command: SlashCommand::Retry,
        name: "/retry",
        description: "Retry the failed request",
        usage: "/retry",
    },
    SlashCommandDefinition {
        command: SlashCommand::Continue,
        name: "/continue",
        description: "Continue the canceled request",
        usage: "/continue",
    },
];

#[derive(Clone, Debug, PartialEq)]
pub struct AppState {
    pub workspace_root: PathBuf,
    pub should_quit: bool,
    input: String,
    cursor: usize,
    input_history: Vec<String>,
    input_history_index: Option<usize>,
    input_history_draft: String,
    undo_input: Option<(String, usize)>,
    messages: Vec<Message>,
    streaming_assistant_message: Option<usize>,
    last_agent_event_sequence: u64,
    selected_message: Option<usize>,
    conversation_search: Option<String>,
    conversation_scroll_from_bottom: u16,
    conversation_max_scroll: u16,
    help_visible: bool,
    run_status: TuiRunStatus,
    session_id: Option<String>,
    run_id: Option<String>,
    run_elapsed_millis: Option<u128>,
    estimated_input_tokens: u64,
    estimated_output_tokens: u64,
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
    pending_new_session: bool,
    session_search: String,
    session_rename: Option<String>,
    selected_session: usize,
    code_change_review: Option<CodeChangeReview>,
    selected_changed_file: usize,
    code_change_result: Option<TuiCodeChangeResult>,
    audit_items: Vec<AuditItem>,
    audit_panel_visible: bool,
    audit_filter: AuditFilter,
    selected_audit_item: usize,
    skills: Vec<SkillDefinition>,
    skill_overrides: Vec<SkillOverride>,
    selected_skill: usize,
    selected_skill_names: Vec<String>,
    active_skill_names: Vec<String>,
    skill_panel_visible: bool,
    runtime_info: TuiRuntimeInfo,
    runtime_logs: Vec<TuiRuntimeLog>,
    log_panel_visible: bool,
    focused_panel: FocusedPanel,
    text_selection: Option<TextSelection>,
    text_selection_drag: Option<TextSelection>,
    sidebar_visible: bool,
    divider_dragging: bool,
    conversation_width_percent: u16,
    selected_slash_command: usize,
    selected_inline_skill: usize,
    indexed_files: Vec<String>,
    file_index_revision: u64,
    file_reference_cache: FileReferenceCache,
    selected_file_reference: usize,
    slash_commands_dismissed: bool,
    pending_escape_action: Option<PendingEscapeAction>,
    notification: Option<(String, Instant)>,
}

#[derive(Clone, Debug, Default, PartialEq)]
struct FileReferenceCache {
    index_revision: u64,
    query: Option<String>,
    matches: FileReferenceMatches,
}

#[derive(Clone, Debug, Default, PartialEq)]
enum FileReferenceMatches {
    #[default]
    Disabled,
    AllFiles,
    Filtered(Vec<usize>),
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
    pub expanded: bool,
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

impl AppState {
    #[must_use]
    pub fn new(workspace_root: impl Into<PathBuf>) -> Self {
        Self {
            workspace_root: workspace_root.into(),
            should_quit: false,
            input: String::new(),
            cursor: 0,
            input_history: Vec::new(),
            input_history_index: None,
            input_history_draft: String::new(),
            undo_input: None,
            messages: Vec::new(),
            streaming_assistant_message: None,
            last_agent_event_sequence: 0,
            selected_message: None,
            conversation_search: None,
            conversation_scroll_from_bottom: 0,
            conversation_max_scroll: 0,
            help_visible: false,
            run_status: TuiRunStatus::Idle,
            session_id: None,
            run_id: None,
            run_elapsed_millis: None,
            estimated_input_tokens: 0,
            estimated_output_tokens: 0,
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
            pending_new_session: false,
            session_search: String::new(),
            session_rename: None,
            selected_session: 0,
            code_change_review: None,
            selected_changed_file: 0,
            code_change_result: None,
            audit_items: Vec::new(),
            audit_panel_visible: false,
            audit_filter: AuditFilter::All,
            selected_audit_item: 0,
            skills: Vec::new(),
            skill_overrides: Vec::new(),
            selected_skill: 0,
            selected_skill_names: Vec::new(),
            active_skill_names: Vec::new(),
            skill_panel_visible: false,
            runtime_info: TuiRuntimeInfo::default(),
            runtime_logs: Vec::new(),
            log_panel_visible: false,
            focused_panel: FocusedPanel::Conversation,
            text_selection: None,
            text_selection_drag: None,
            sidebar_visible: true,
            divider_dragging: false,
            conversation_width_percent: DEFAULT_CONVERSATION_WIDTH_PERCENT,
            selected_slash_command: 0,
            selected_inline_skill: 0,
            indexed_files: Vec::new(),
            file_index_revision: 0,
            file_reference_cache: FileReferenceCache::default(),
            selected_file_reference: 0,
            slash_commands_dismissed: false,
            pending_escape_action: None,
            notification: None,
        }
    }

    #[must_use]
    pub fn input_text(&self) -> &str {
        &self.input
    }

    #[must_use]
    pub(super) fn input_cursor_line_column(&self) -> (u16, u16) {
        let input_before_cursor = &self.input[..self.cursor];
        let line = input_before_cursor
            .bytes()
            .filter(|byte| *byte == b'\n')
            .count();
        let current_line = input_before_cursor
            .rsplit_once('\n')
            .map_or(input_before_cursor, |(_, line)| line);
        (
            u16::try_from(line).unwrap_or(u16::MAX),
            u16::try_from(UnicodeWidthStr::width(current_line)).unwrap_or(u16::MAX),
        )
    }

    #[must_use]
    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    #[must_use]
    pub(super) fn selected_message(&self) -> Option<usize> {
        self.selected_message
    }

    #[must_use]
    pub(super) fn conversation_search(&self) -> Option<&str> {
        self.conversation_search.as_deref()
    }

    #[must_use]
    pub fn conversation_scroll_from_bottom(&self) -> u16 {
        self.conversation_scroll_from_bottom
    }

    pub(crate) fn apply_scroll_delta(&mut self, delta: i32) {
        if delta >= 0 {
            self.conversation_scroll_from_bottom = self
                .conversation_scroll_from_bottom
                .saturating_add(delta as u16);
        } else {
            self.conversation_scroll_from_bottom = self
                .conversation_scroll_from_bottom
                .saturating_sub(delta.unsigned_abs() as u16);
        }
    }

    pub(crate) fn clamp_conversation_scroll_to(&mut self, maximum: u16) {
        self.conversation_max_scroll = maximum;
        self.conversation_scroll_from_bottom = self.conversation_scroll_from_bottom.min(maximum);
    }

    #[must_use]
    pub(super) fn scroll_position_label(&self) -> &'static str {
        if self.conversation_scroll_from_bottom == 0 {
            "Bottom"
        } else if self.conversation_scroll_from_bottom >= self.conversation_max_scroll {
            "Top"
        } else {
            "History"
        }
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
    pub fn selected_timeline(&self) -> Option<usize> {
        self.selected_timeline
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
    pub fn selected_session(&self) -> usize {
        self.selected_session
    }

    #[must_use]
    pub fn session_search(&self) -> &str {
        &self.session_search
    }

    #[must_use]
    pub fn session_rename(&self) -> Option<&str> {
        self.session_rename.as_deref()
    }

    #[must_use]
    pub fn filtered_sessions(&self) -> Vec<&Session> {
        let query = self.session_search.to_lowercase();
        let mut sessions = self
            .sessions
            .iter()
            .filter(|session| !session.archived)
            .filter(|session| {
                query.is_empty()
                    || session
                        .name
                        .as_deref()
                        .unwrap_or("(unnamed)")
                        .to_lowercase()
                        .contains(&query)
            })
            .collect::<Vec<_>>();
        sessions.sort_by(|left, right| {
            right
                .liked
                .cmp(&left.liked)
                .then_with(|| right.updated_at.cmp(&left.updated_at))
        });
        sessions
    }

    fn selected_filtered_session(&self) -> Option<&Session> {
        self.filtered_sessions().get(self.selected_session).copied()
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

    pub fn pending_new_session(&self) -> bool {
        self.pending_new_session
    }

    fn begin_new_session(&mut self) {
        self.pending_new_session = true;
        self.session_id = None;
        self.run_id = None;
        self.run_elapsed_millis = None;
        self.messages.clear();
        self.timeline.clear();
        self.steps.clear();
        self.input_history.clear();
        self.input_history_index = None;
        self.input_history_draft.clear();
        self.run_status = TuiRunStatus::Idle;
        self.session_panel_visible = false;
        self.focused_panel = FocusedPanel::Conversation;
        self.notify("New session");
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

    #[must_use]
    pub fn audit_filter(&self) -> AuditFilter {
        self.audit_filter
    }

    #[must_use]
    pub fn selected_audit_item(&self) -> usize {
        self.selected_audit_item
    }

    #[must_use]
    pub fn filtered_audit_items(&self) -> Vec<&AuditItem> {
        self.audit_items
            .iter()
            .filter(|item| match self.audit_filter {
                AuditFilter::All => true,
                AuditFilter::Files => item.title.starts_with("File"),
                AuditFilter::Commands => {
                    item.title.starts_with("Tool") || item.title.contains("command")
                }
                AuditFilter::Policy => item.title.starts_with("Policy"),
            })
            .collect()
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
    pub fn log_panel_visible(&self) -> bool {
        self.log_panel_visible
    }

    #[must_use]
    pub fn focused_panel(&self) -> FocusedPanel {
        self.focused_panel
    }

    #[must_use]
    pub fn text_selection(&self) -> Option<TextSelection> {
        self.text_selection
    }

    #[must_use]
    pub fn sidebar_visible(&self) -> bool {
        self.sidebar_visible
    }

    #[must_use]
    pub fn selected_slash_command(&self) -> usize {
        self.selected_slash_command
    }

    #[must_use]
    pub(super) fn slash_command_suggestions(&self) -> Vec<SlashCommandDefinition> {
        if self.slash_commands_dismissed {
            return Vec::new();
        }
        let Some(query) = self.input.strip_prefix('/') else {
            return Vec::new();
        };
        if query.chars().any(char::is_whitespace) {
            return Vec::new();
        }
        SLASH_COMMANDS
            .iter()
            .copied()
            .filter(|definition| match definition.command {
                SlashCommand::Retry => self.run_status == TuiRunStatus::Failed,
                SlashCommand::Continue => self.run_status == TuiRunStatus::Canceled,
                _ => true,
            })
            .filter(|definition| definition.name.trim_start_matches('/').starts_with(query))
            .collect()
    }

    #[must_use]
    pub(super) fn inline_skill_suggestions(&self) -> Vec<&SkillDefinition> {
        let token_start = self.input[..self.cursor]
            .rfind(char::is_whitespace)
            .map_or(0, |index| index + 1);
        let token = &self.input[token_start..self.cursor];
        let Some(query) = token.strip_prefix('$') else {
            return Vec::new();
        };
        let query = query.to_lowercase();
        self.skills
            .iter()
            .filter(|skill| skill.name.to_lowercase().contains(&query))
            .collect()
    }

    #[must_use]
    pub(super) fn selected_inline_skill(&self) -> usize {
        self.selected_inline_skill
    }

    pub(crate) fn set_file_index(&mut self, snapshot: FileIndexSnapshot) {
        self.indexed_files = snapshot.files;
        self.file_index_revision = self.file_index_revision.wrapping_add(1);
        self.selected_file_reference = 0;
    }

    #[must_use]
    pub(super) fn file_reference_suggestion_count(&self) -> usize {
        match &self.file_reference_cache.matches {
            FileReferenceMatches::Disabled => 0,
            FileReferenceMatches::AllFiles => self.indexed_files.len(),
            FileReferenceMatches::Filtered(matches) => matches.len(),
        }
    }

    #[must_use]
    pub(super) fn file_reference_suggestion(&self, index: usize) -> Option<&str> {
        let file_index = match &self.file_reference_cache.matches {
            FileReferenceMatches::Disabled => return None,
            // An empty `@` query uses the file index directly. Materializing
            // every file into a second candidate list caused input latency in
            // large workspaces.
            FileReferenceMatches::AllFiles => index,
            FileReferenceMatches::Filtered(matches) => *matches.get(index)?,
        };
        self.indexed_files.get(file_index).map(String::as_str)
    }

    fn current_file_reference_query(&self) -> Option<&str> {
        let token_start = self.input[..self.cursor]
            .rfind(char::is_whitespace)
            .map_or(0, |index| index + 1);
        let token = &self.input[token_start..self.cursor];
        let query = token.strip_prefix('@')?;
        if query.starts_with('{') {
            return None;
        }
        Some(query)
    }

    fn refresh_file_reference_cache(&mut self) {
        let query = self.current_file_reference_query().map(str::to_owned);
        if self.file_reference_cache.index_revision == self.file_index_revision
            && self.file_reference_cache.query == query
        {
            return;
        }
        let matches = match query.as_deref() {
            None => FileReferenceMatches::Disabled,
            Some("") => FileReferenceMatches::AllFiles,
            Some(query) => {
                let query = query.to_lowercase();
                FileReferenceMatches::Filtered(
                    self.indexed_files
                        .iter()
                        .enumerate()
                        .filter_map(|(index, path)| fuzzy_path_match(path, &query).then_some(index))
                        .collect(),
                )
            }
        };
        self.file_reference_cache = FileReferenceCache {
            index_revision: self.file_index_revision,
            query,
            matches,
        };
        let count = self.file_reference_suggestion_count();
        self.selected_file_reference = if count == 0 {
            0
        } else {
            self.selected_file_reference % count
        };
    }

    #[must_use]
    pub(super) fn selected_file_reference(&self) -> usize {
        self.selected_file_reference
    }

    pub(super) fn completed_file_reference_ranges(&self) -> Vec<(usize, usize)> {
        reference_ranges(&self.input)
            .into_iter()
            .filter(|(start, end)| self.reference_path_exists(&self.input[*start..*end]))
            .collect()
    }

    fn complete_file_reference(&mut self, finish_token: bool) {
        let Some(path) = self
            .file_reference_suggestion(self.selected_file_reference)
            .map(str::to_owned)
        else {
            return;
        };
        let token_start = self.input[..self.cursor]
            .rfind(char::is_whitespace)
            .map_or(0, |index| index + 1);
        let mut reference = if path.chars().any(char::is_whitespace) {
            format!("@{{{path}}}")
        } else {
            format!("@{path}")
        };
        if finish_token {
            reference.push(' ');
        }
        self.remember_undo_state();
        self.input
            .replace_range(token_start..self.cursor, &reference);
        self.cursor = token_start + reference.len();
        self.selected_file_reference = 0;
    }

    fn complete_inline_skill(&mut self) {
        let Some(name) = self
            .inline_skill_suggestions()
            .get(self.selected_inline_skill)
            .map(|skill| skill.name.clone())
        else {
            return;
        };
        let token_start = self.input[..self.cursor]
            .rfind(char::is_whitespace)
            .map_or(0, |index| index + 1);
        self.remember_undo_state();
        self.input
            .replace_range(token_start..self.cursor, &format!("${name}"));
        self.cursor = token_start + name.len() + 1;
        if !self.selected_skill_names.contains(&name) {
            self.selected_skill_names.push(name);
        }
    }

    #[must_use]
    pub(super) fn escape_confirmation_hint(&self) -> Option<&'static str> {
        self.pending_escape_action
            .filter(|pending| pending.expires_at >= Instant::now())
            .map(|pending| match pending.action {
                EscapeAction::ClearInput => "Press Esc again to clear the input",
                EscapeAction::InterruptRun => "Press Esc again to interrupt the current run",
            })
    }

    #[must_use]
    pub(super) fn notification(&self) -> Option<&str> {
        self.notification
            .as_ref()
            .filter(|(_, expires_at)| *expires_at >= Instant::now())
            .map(|(message, _)| message.as_str())
    }

    fn notify(&mut self, message: impl Into<String>) {
        self.notification = Some((message.into(), Instant::now() + NOTIFICATION_DURATION));
    }

    #[must_use]
    pub(super) fn conversation_panel_width(&self, viewport_width: u16) -> u16 {
        if viewport_width < 60 {
            return viewport_width;
        }
        if !self.sidebar_visible {
            return viewport_width.saturating_sub(DIVIDER_WIDTH);
        }
        let maximum = viewport_width
            .saturating_sub(DIVIDER_WIDTH)
            .saturating_sub(MIN_PANEL_WIDTH);
        viewport_width
            .saturating_mul(self.conversation_width_percent)
            .checked_div(100)
            .unwrap_or_default()
            .clamp(MIN_PANEL_WIDTH, maximum)
    }

    #[must_use]
    pub fn conversation_text_lines(&self) -> Vec<String> {
        conversation_text_lines(self)
    }

    #[must_use]
    pub fn sidebar_text_lines(&self) -> Vec<String> {
        sidebar_text_lines(self)
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
        self.selected_slash_command = 0;
        self.selected_inline_skill = 0;
        self.selected_file_reference = 0;
        self.slash_commands_dismissed = false;
        self.pending_escape_action = None;
        self.input_history_index = None;
        self.input_history_draft.clear();
    }

    fn insert(&mut self, character: char) {
        self.remember_undo_state();
        self.input.insert(self.cursor, character);
        self.cursor += character.len_utf8();
        self.selected_slash_command = 0;
        self.selected_inline_skill = 0;
        self.selected_file_reference = 0;
        self.slash_commands_dismissed = false;
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
        if let Some((start, end)) = self.file_reference_range_at(self.cursor, true) {
            self.remember_undo_state();
            self.input.drain(start..end);
            self.cursor = start;
            self.selected_file_reference = 0;
            return;
        }
        let Some((index, _)) = self.input[..self.cursor].char_indices().next_back() else {
            return;
        };
        self.remember_undo_state();
        self.input.drain(index..self.cursor);
        self.cursor = index;
        self.selected_slash_command = 0;
        self.slash_commands_dismissed = false;
    }

    fn delete_at_cursor(&mut self) {
        if let Some((start, end)) = self.file_reference_range_at(self.cursor, false) {
            self.remember_undo_state();
            self.input.drain(start..end);
            self.cursor = start;
            self.selected_file_reference = 0;
            return;
        }
        let Some(character) = self.input[self.cursor..].chars().next() else {
            return;
        };
        self.remember_undo_state();
        self.input
            .drain(self.cursor..self.cursor + character.len_utf8());
        self.selected_slash_command = 0;
        self.slash_commands_dismissed = false;
    }

    fn file_reference_range_at(&self, cursor: usize, backwards: bool) -> Option<(usize, usize)> {
        let probe = if backwards {
            cursor.checked_sub(1)?
        } else {
            cursor
        };
        reference_ranges(&self.input)
            .into_iter()
            .find(|(start, end)| {
                let contains = *start <= probe && probe < *end;
                contains && self.reference_path_exists(&self.input[*start..*end])
            })
    }

    fn reference_path_exists(&self, reference: &str) -> bool {
        let path = reference
            .strip_prefix("@{")
            .and_then(|path| path.strip_suffix('}'))
            .or_else(|| reference.strip_prefix('@'));
        path.is_some_and(|path| self.indexed_files.iter().any(|indexed| indexed == path))
    }

    fn remember_undo_state(&mut self) {
        self.undo_input = Some((self.input.clone(), self.cursor));
        self.input_history_index = None;
    }

    fn undo_edit(&mut self) {
        if let Some((input, cursor)) = self.undo_input.take() {
            self.input = input;
            self.cursor = cursor;
        }
    }

    fn move_cursor_word_left(&mut self) {
        let before = &self.input[..self.cursor];
        let trimmed = before.trim_end_matches(char::is_whitespace);
        self.cursor = trimmed.rfind(char::is_whitespace).map_or(0, |index| {
            index + trimmed[index..].chars().next().map_or(0, char::len_utf8)
        });
    }

    fn move_cursor_word_right(&mut self) {
        let after = &self.input[self.cursor..];
        let word_end = after.find(char::is_whitespace).unwrap_or(after.len());
        let rest = &after[word_end..];
        let whitespace_end = rest
            .find(|character: char| !character.is_whitespace())
            .unwrap_or(rest.len());
        self.cursor += word_end + whitespace_end;
    }

    fn move_cursor_to_line_start(&mut self) {
        self.cursor = self.input[..self.cursor]
            .rfind('\n')
            .map_or(0, |index| index + 1);
    }

    fn move_cursor_to_line_end(&mut self) {
        self.cursor = self.input[self.cursor..]
            .find('\n')
            .map_or(self.input.len(), |index| self.cursor + index);
    }

    fn delete_current_line(&mut self) {
        if self.input.is_empty() {
            return;
        }
        self.remember_undo_state();
        let start = self.input[..self.cursor]
            .rfind('\n')
            .map_or(0, |index| index + 1);
        let end = self.input[self.cursor..]
            .find('\n')
            .map_or(self.input.len(), |index| self.cursor + index + 1);
        self.input.drain(start..end);
        self.cursor = start.min(self.input.len());
    }

    fn browse_input_history(&mut self, previous: bool) {
        if self.input_history.is_empty() {
            return;
        }
        let index = match (self.input_history_index, previous) {
            (None, true) => {
                self.input_history_draft = self.input.clone();
                self.input_history.len() - 1
            }
            (Some(index), true) => index.saturating_sub(1),
            (Some(index), false) if index + 1 < self.input_history.len() => index + 1,
            (Some(_), false) => {
                self.input_history_index = None;
                self.input = std::mem::take(&mut self.input_history_draft);
                self.cursor = self.input.len();
                return;
            }
            (None, false) => return,
        };
        self.input_history_index = Some(index);
        self.input.clone_from(&self.input_history[index]);
        self.cursor = self.input.len();
    }

    fn panel_text_lines(&self, panel: SelectionPanel) -> Vec<String> {
        match panel {
            SelectionPanel::Conversation => self.conversation_text_lines(),
            SelectionPanel::Sidebar => self.sidebar_text_lines(),
        }
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
    state.pending_new_session = false;
    state.sessions = sessions;
    state.session_histories = session_histories;
    state.messages = messages_from_steps(&steps);
    state.input_history = input_history_from_runs(&runs);
    if state.input_history.is_empty() {
        state.input_history = input_history_from_steps(&steps);
    }
    state.input_history_index = None;
    state.input_history_draft.clear();
    state.steps = steps;
    state.timeline = command_timeline_from_steps(&state.steps);
    state.selected_timeline = None;
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
    let close_picker = matches!(command, AppCommand::SwitchSession { .. });
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
    state.session_panel_visible = !close_picker;
    if close_picker {
        state.session_search.clear();
        state.session_rename = None;
        state.selected_session = 0;
        state.focused_panel = FocusedPanel::Conversation;
        state.notify("Session switched");
    }
    Ok(true)
}

pub fn materialize_pending_new_session(
    state: &mut AppState,
    store: &SqliteWorkspaceStore,
    request: &str,
) -> Result<(), StoreError> {
    if !state.pending_new_session {
        return Ok(());
    }
    let session = store.create_session(None)?;
    store.set_active_session(&session.id)?;
    load_active_session_history(state, store)?;
    state.input_history.push(request.to_owned());
    state.messages.push(Message {
        role: MessageRole::User,
        content: request.to_owned(),
    });
    state.run_status = TuiRunStatus::Running;
    Ok(())
}

pub fn assign_default_session_title(
    state: &mut AppState,
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
    let title = generated_session_title(request);
    *session = store.rename_session(&session.id, Some(title))?;
    Ok(())
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
                        expanded: false,
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
    FileIndexUpdated(FileIndexSnapshot),
    Key(KeyEvent),
    Mouse {
        event: MouseEvent,
        viewport: TuiViewport,
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

pub fn update(state: &mut AppState, action: AppAction) -> Option<AppCommand> {
    let command = update_inner(state, action);
    state.refresh_file_reference_cache();
    command
}

fn update_inner(state: &mut AppState, action: AppAction) -> Option<AppCommand> {
    if matches!(
        &action,
        AppAction::Key(key) if key.kind != KeyEventKind::Release && key.code != KeyCode::Esc
    ) || matches!(&action, AppAction::Mouse { .. })
    {
        state.pending_escape_action = None;
    }
    match action {
        AppAction::FileIndexUpdated(snapshot) => {
            state.set_file_index(snapshot);
            None
        }
        AppAction::Mouse { event, viewport } => handle_mouse_event(state, event, viewport),
        AppAction::Key(KeyEvent {
            kind: KeyEventKind::Release,
            ..
        }) => None,
        AppAction::CodeChangeProposed { proposal } => {
            append_proposal_audit(&mut state.audit_items, &proposal);
            state.code_change_review = Some(CodeChangeReview::new(proposal));
            state.selected_changed_file = 0;
            state.code_change_result = None;
            None
        }
        AppAction::AgentEvent(event) => {
            if event.sequence > 0 && matches!(&event.data, AgentEventData::RunStarted { .. }) {
                state.last_agent_event_sequence = 0;
            }
            if event.sequence > 0 && event.sequence <= state.last_agent_event_sequence {
                return None;
            }
            state.last_agent_event_sequence = state.last_agent_event_sequence.max(event.sequence);
            state.runtime_logs.push(TuiRuntimeLog {
                title: format!("Agent event: {:?}", event.kind),
                detail: Some(event.id.to_string()),
            });
            if let AgentEventData::SkillsSelected { skills } = &event.data {
                state.active_skill_names = skills.clone();
            }
            if let AgentEventData::AssistantTextDelta { content } = &event.data {
                state.estimated_output_tokens = state
                    .estimated_output_tokens
                    .saturating_add(estimate_tokens(content));
                match state.streaming_assistant_message {
                    Some(index) => state.messages[index].content.push_str(content),
                    None => {
                        state.messages.push(Message {
                            role: MessageRole::Assistant,
                            content: content.clone(),
                        });
                        state.streaming_assistant_message = Some(state.messages.len() - 1);
                    }
                }
                return None;
            }
            if matches!(&event.data, AgentEventData::LlmCompleted) {
                let completed_id = event.id.to_string();
                state.timeline.retain(|item| item.id != completed_id);
                state.selected_timeline = None;
                return None;
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
                item.message_count = state.messages.len();
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
                        item.command = merge_command_execution(
                            state.timeline[index].command.clone(),
                            item.command,
                        );
                        item.expanded = state.timeline[index].expanded;
                        state.timeline[index] = item;
                    }
                    None => state.timeline.push(item),
                }
            }
            None
        }
        AppAction::AssistantMessage { content } => {
            state.estimated_output_tokens = state
                .estimated_output_tokens
                .saturating_add(estimate_tokens(&content));
            state.messages.push(Message {
                role: MessageRole::Assistant,
                content,
            });
            None
        }
        AppAction::RunFinished { response } => {
            state.pending_escape_action = None;
            if let Some(index) = state.streaming_assistant_message.take()
                && index < state.messages.len()
            {
                state.messages.remove(index);
            }
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
            let run_steps = state.steps.clone();
            let message_base = merge_run_messages(state, &run_steps, response.answer.as_deref());
            state.timeline = command_timeline_from_steps(&state.steps);
            for item in &mut state.timeline {
                item.message_count = item.message_count.saturating_add(message_base);
            }
            state.selected_timeline = None;
            state.audit_items = audit_items_from_steps(&state.steps);
            state.pending_human_input = waiting_for_human_input
                .then(|| latest_human_input_request(&state.steps))
                .flatten();
            state.selected_human_option = 0;
            state.human_input_error = None;
            state.runtime_logs.push(TuiRuntimeLog {
                title: format!("Run finished: {:?}", state.run_status),
                detail: state.run_id.clone(),
            });
            None
        }
        AppAction::RunFailed { error } => {
            state.pending_escape_action = None;
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
            state.pending_escape_action = None;
            state.run_status = TuiRunStatus::Canceled;
            state.runtime_logs.push(TuiRuntimeLog {
                title: "Run canceled".to_owned(),
                detail: state.run_id.clone(),
            });
            None
        }
        AppAction::Key(KeyEvent {
            code: KeyCode::Esc, ..
        }) if !state.slash_command_suggestions().is_empty() => {
            state.pending_escape_action = None;
            state.slash_commands_dismissed = true;
            None
        }
        AppAction::Key(KeyEvent {
            code: KeyCode::Esc, ..
        }) if state.log_panel_visible => {
            state.pending_escape_action = None;
            state.log_panel_visible = false;
            state.focused_panel = FocusedPanel::Conversation;
            None
        }
        AppAction::Key(KeyEvent {
            code: KeyCode::Esc, ..
        }) if state.skill_panel_visible => {
            state.pending_escape_action = None;
            state.skill_panel_visible = false;
            state.focused_panel = FocusedPanel::Conversation;
            None
        }
        AppAction::Key(KeyEvent {
            code: KeyCode::Esc, ..
        }) if state.session_rename.is_some() => {
            state.session_rename = None;
            None
        }
        AppAction::Key(KeyEvent {
            code: KeyCode::Esc, ..
        }) if state.session_panel_visible => {
            state.pending_escape_action = None;
            state.session_panel_visible = false;
            state.session_search.clear();
            state.session_rename = None;
            state.selected_session = 0;
            state.focused_panel = FocusedPanel::Conversation;
            None
        }
        AppAction::Key(KeyEvent {
            code: KeyCode::Char('d'),
            modifiers: KeyModifiers::CONTROL,
            ..
        }) if state.session_panel_visible => {
            state
                .selected_filtered_session()
                .map(|session| AppCommand::DeleteSession {
                    session_id: session.id.clone(),
                })
        }
        AppAction::Key(KeyEvent {
            code: KeyCode::Esc, ..
        }) => handle_escape(state, Instant::now()),
        AppAction::Key(KeyEvent {
            code: KeyCode::Char('f'),
            ..
        }) if state.audit_panel_visible => {
            state.audit_filter = match state.audit_filter {
                AuditFilter::All => AuditFilter::Files,
                AuditFilter::Files => AuditFilter::Commands,
                AuditFilter::Commands => AuditFilter::Policy,
                AuditFilter::Policy => AuditFilter::All,
            };
            state.selected_audit_item = 0;
            None
        }
        AppAction::Key(KeyEvent {
            code: KeyCode::Up, ..
        }) if state.audit_panel_visible => {
            state.selected_audit_item = state.selected_audit_item.saturating_sub(1);
            None
        }
        AppAction::Key(KeyEvent {
            code: KeyCode::Down,
            ..
        }) if state.audit_panel_visible => {
            let maximum = state.filtered_audit_items().len().saturating_sub(1);
            state.selected_audit_item = (state.selected_audit_item + 1).min(maximum);
            None
        }
        AppAction::Key(key) if is_quit_shortcut(state, key) => {
            state.should_quit = true;
            None
        }
        AppAction::Key(key)
            if key.code == KeyCode::Up && key.modifiers.contains(KeyModifiers::ALT) =>
        {
            if !state.messages.is_empty() {
                state.selected_message = Some(
                    state
                        .selected_message
                        .unwrap_or(state.messages.len())
                        .saturating_sub(1),
                );
            }
            None
        }
        AppAction::Key(key)
            if key.code == KeyCode::Down && key.modifiers.contains(KeyModifiers::ALT) =>
        {
            if !state.messages.is_empty() {
                state.selected_message = Some(
                    state
                        .selected_message
                        .map_or(0, |index| (index + 1).min(state.messages.len() - 1)),
                );
            }
            None
        }
        AppAction::Key(key) if is_copy_message_shortcut(state, key) => state
            .selected_message
            .and_then(|index| state.messages.get(index))
            .map(|message| message.content.clone())
            .map(|content| {
                state.notify("Copied to clipboard");
                AppCommand::CopyText { content }
            }),
        AppAction::Key(key)
            if key.code == KeyCode::Char('e') && key.modifiers.contains(KeyModifiers::CONTROL) =>
        {
            if let Some(message) = state
                .selected_message
                .and_then(|index| state.messages.get(index))
                .filter(|message| message.role == MessageRole::User)
                .cloned()
            {
                state.input = message.content;
                state.cursor = state.input.len();
                state.notify("Message loaded for editing");
            }
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
        }) if !state.slash_command_suggestions().is_empty() => {
            if let Some(command) = state
                .slash_command_suggestions()
                .get(state.selected_slash_command)
            {
                state.input = command.name.to_owned();
                state.cursor = state.input.len();
            }
            None
        }
        AppAction::Key(KeyEvent {
            code: KeyCode::Tab, ..
        }) if !state.inline_skill_suggestions().is_empty() => {
            state.complete_inline_skill();
            None
        }
        AppAction::Key(KeyEvent {
            code: KeyCode::Tab, ..
        }) if state.file_reference_suggestion_count() > 0 => {
            state.complete_file_reference(false);
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
            code: KeyCode::Char('l'),
            modifiers: KeyModifiers::CONTROL,
            ..
        }) if state.session_panel_visible => {
            state
                .selected_filtered_session()
                .map(|session| AppCommand::ToggleSessionLiked {
                    session_id: session.id.clone(),
                })
        }
        AppAction::Key(KeyEvent {
            code: KeyCode::Char('n'),
            modifiers: KeyModifiers::CONTROL,
            ..
        }) if state.session_panel_visible && state.run_status != TuiRunStatus::Running => {
            Some(AppCommand::CreateSession { name: None })
        }
        AppAction::Key(KeyEvent {
            code: KeyCode::Char('a'),
            modifiers: KeyModifiers::CONTROL,
            ..
        }) if state.session_panel_visible => {
            state
                .selected_filtered_session()
                .map(|session| AppCommand::ArchiveSession {
                    session_id: session.id.clone(),
                })
        }
        AppAction::Key(KeyEvent {
            code: KeyCode::Char('g'),
            modifiers: KeyModifiers::CONTROL,
            ..
        }) if state.session_panel_visible => {
            state.selected_filtered_session().and_then(|session| {
                state.session_history(&session.id).and_then(|history| {
                    history.runs.first().map(|run| AppCommand::RenameSession {
                        session_id: session.id.clone(),
                        name: generated_session_title(&run.request),
                    })
                })
            })
        }
        AppAction::Key(KeyEvent {
            code: KeyCode::Char('r'),
            modifiers: KeyModifiers::CONTROL,
            ..
        }) if state.session_panel_visible => {
            state.session_rename = state
                .selected_filtered_session()
                .map(|session| session.name.clone().unwrap_or_default());
            None
        }
        AppAction::Key(KeyEvent {
            code: KeyCode::Up, ..
        }) if state.session_panel_visible => {
            let session_count = state.filtered_sessions().len();
            if session_count > 0 {
                state.selected_session = state
                    .selected_session
                    .checked_sub(1)
                    .unwrap_or(session_count - 1);
            }
            None
        }
        AppAction::Key(KeyEvent {
            code: KeyCode::Down,
            ..
        }) if state.session_panel_visible => {
            let session_count = state.filtered_sessions().len();
            if session_count > 0 {
                state.selected_session = (state.selected_session + 1) % session_count;
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
            code: KeyCode::Up, ..
        }) if !state.slash_command_suggestions().is_empty() => {
            let command_count = state.slash_command_suggestions().len();
            state.selected_slash_command = state
                .selected_slash_command
                .checked_sub(1)
                .unwrap_or(command_count - 1);
            None
        }
        AppAction::Key(KeyEvent {
            code: KeyCode::Down,
            ..
        }) if !state.slash_command_suggestions().is_empty() => {
            let command_count = state.slash_command_suggestions().len();
            state.selected_slash_command = (state.selected_slash_command + 1) % command_count;
            None
        }
        AppAction::Key(KeyEvent {
            code: KeyCode::Up, ..
        }) if !state.inline_skill_suggestions().is_empty() => {
            let count = state.inline_skill_suggestions().len();
            state.selected_inline_skill = state
                .selected_inline_skill
                .checked_sub(1)
                .unwrap_or(count - 1);
            None
        }
        AppAction::Key(KeyEvent {
            code: KeyCode::Down,
            ..
        }) if !state.inline_skill_suggestions().is_empty() => {
            let count = state.inline_skill_suggestions().len();
            state.selected_inline_skill = (state.selected_inline_skill + 1) % count;
            None
        }
        AppAction::Key(KeyEvent {
            code: KeyCode::Up, ..
        }) if state.file_reference_suggestion_count() > 0 => {
            let count = state.file_reference_suggestion_count();
            state.selected_file_reference = (state.selected_file_reference % count)
                .checked_sub(1)
                .unwrap_or(count - 1);
            None
        }
        AppAction::Key(KeyEvent {
            code: KeyCode::Down,
            ..
        }) if state.file_reference_suggestion_count() > 0 => {
            let count = state.file_reference_suggestion_count();
            state.selected_file_reference = (state.selected_file_reference + 1) % count;
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
            code: KeyCode::Left,
            ..
        }) if state.input.is_empty() => {
            state.focused_panel = FocusedPanel::Conversation;
            None
        }
        AppAction::Key(KeyEvent {
            code: KeyCode::Right,
            ..
        }) if state.input.is_empty() => {
            state.sidebar_visible = true;
            state.focused_panel = FocusedPanel::Sidebar;
            None
        }
        AppAction::Key(KeyEvent {
            code: KeyCode::Enter,
            ..
        }) if state.session_panel_visible && state.session_rename.is_some() => {
            let name = state.session_rename.take().unwrap_or_default();
            state
                .selected_filtered_session()
                .map(|session| AppCommand::RenameSession {
                    session_id: session.id.clone(),
                    name,
                })
        }
        AppAction::Key(KeyEvent {
            code: KeyCode::Enter,
            ..
        }) if state.session_panel_visible && state.run_status != TuiRunStatus::Running => state
            .filtered_sessions()
            .get(state.selected_session)
            .map(|session| AppCommand::SwitchSession {
                session_id: session.id.clone(),
            }),
        AppAction::Key(KeyEvent {
            code: KeyCode::Enter,
            ..
        }) if state.file_reference_suggestion_count() > 0 => {
            state.complete_file_reference(true);
            None
        }
        AppAction::Key(KeyEvent {
            code: KeyCode::Enter,
            ..
        }) => {
            if let SlashCommandExecution::Executed(command) = execute_selected_slash_command(state)
            {
                return command;
            }
            if state.input.trim() == "/quit" {
                state.clear_input();
                state.should_quit = true;
                return None;
            }
            if state.input.trim() == "/clear" {
                state.clear_input();
                state.messages.clear();
                state.conversation_scroll_from_bottom = 0;
                return None;
            }
            if state.input.trim() == "/help" {
                state.clear_input();
                state.help_visible = true;
                state.sidebar_visible = true;
                state.focused_panel = FocusedPanel::Sidebar;
                return None;
            }
            if state.input.trim() == "/sessions" {
                state.clear_input();
                state.session_panel_visible = true;
                state.session_search.clear();
                state.selected_session = 0;
                state.focused_panel = FocusedPanel::Conversation;
                return None;
            }
            if state.input.trim() == "/audit" {
                state.clear_input();
                state.audit_panel_visible = true;
                state.sidebar_visible = true;
                state.focused_panel = FocusedPanel::Sidebar;
                return None;
            }
            if state.input.trim() == "/skills" {
                state.clear_input();
                state.skill_panel_visible = true;
                state.sidebar_visible = true;
                state.focused_panel = FocusedPanel::Sidebar;
                return None;
            }
            if state.input.trim() == "/logs" {
                state.clear_input();
                state.log_panel_visible = true;
                state.sidebar_visible = true;
                state.focused_panel = FocusedPanel::Sidebar;
                return None;
            }
            if let Some(query) = state.input.trim().strip_prefix("/search ")
                && !query.is_empty()
            {
                let query = query.to_lowercase();
                let start = if state.conversation_search.as_deref() == Some(query.as_str()) {
                    state.selected_message.map_or(0, |index| index + 1)
                } else {
                    0
                };
                state.selected_message = state
                    .messages
                    .iter()
                    .enumerate()
                    .skip(start)
                    .find(|(_, message)| message.content.to_lowercase().contains(&query))
                    .map(|(index, _)| index)
                    .or_else(|| {
                        state.messages.iter().enumerate().take(start).find_map(
                            |(index, message)| {
                                message
                                    .content
                                    .to_lowercase()
                                    .contains(&query)
                                    .then_some(index)
                            },
                        )
                    });
                state.conversation_search = Some(query);
                state.conversation_scroll_from_bottom = u16::MAX;
                if state.selected_message.is_none() {
                    state.notify("No conversation matches");
                }
                state.clear_input();
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
                return Some(AppCommand::CreateSession { name: Some(name) });
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
            if state.input.trim_start().starts_with('/') {
                state.notify("Unknown or invalid command");
                state.clear_input();
                return None;
            }
            let inline_skill_names = state
                .input
                .split_whitespace()
                .filter_map(|token| token.strip_prefix('$'))
                .filter(|name| state.skills.iter().any(|skill| skill.name == *name))
                .map(str::to_owned)
                .collect::<Vec<_>>();
            for name in inline_skill_names {
                if !state.selected_skill_names.contains(&name) {
                    state.selected_skill_names.push(name);
                }
            }
            let request = std::mem::take(&mut state.input);
            state.begin_token_estimate(&request);
            if state.input_history.last() != Some(&request) {
                state.input_history.push(request.clone());
            }
            state.input_history_index = None;
            state.input_history_draft.clear();
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
            code: KeyCode::Char('/'),
            ..
        }) if state.session_panel_visible => {
            // Preserve the legacy typed session subcommands: beginning a slash
            // command leaves the picker and returns input to the conversation.
            state.session_panel_visible = false;
            state.session_search.clear();
            state.selected_session = 0;
            state.insert('/');
            None
        }
        AppAction::Key(KeyEvent {
            code: KeyCode::Left,
            modifiers: KeyModifiers::CONTROL,
            ..
        }) => {
            state.move_cursor_word_left();
            None
        }
        AppAction::Key(KeyEvent {
            code: KeyCode::Right,
            modifiers: KeyModifiers::CONTROL,
            ..
        }) => {
            state.move_cursor_word_right();
            None
        }
        AppAction::Key(KeyEvent {
            code: KeyCode::Home,
            ..
        }) if !state.input.is_empty() => {
            state.move_cursor_to_line_start();
            None
        }
        AppAction::Key(KeyEvent {
            code: KeyCode::End, ..
        }) if !state.input.is_empty() => {
            state.move_cursor_to_line_end();
            None
        }
        AppAction::Key(KeyEvent {
            code: KeyCode::Char('u'),
            modifiers: KeyModifiers::CONTROL,
            ..
        }) => {
            state.delete_current_line();
            None
        }
        AppAction::Key(KeyEvent {
            code: KeyCode::Char('z'),
            modifiers: KeyModifiers::CONTROL,
            ..
        }) => {
            state.undo_edit();
            None
        }
        AppAction::Key(KeyEvent {
            code: KeyCode::Up, ..
        }) if state.input_history_index.is_some() || !state.input.contains('\n') => {
            state.browse_input_history(true);
            None
        }
        AppAction::Key(KeyEvent {
            code: KeyCode::Down,
            ..
        }) if state.input_history_index.is_some() || !state.input.contains('\n') => {
            state.browse_input_history(false);
            None
        }
        AppAction::Key(KeyEvent {
            code: KeyCode::Char(character),
            ..
        }) if state.session_panel_visible && state.session_rename.is_some() => {
            if let Some(rename) = &mut state.session_rename {
                rename.push(character);
            }
            None
        }
        AppAction::Key(KeyEvent {
            code: KeyCode::Backspace,
            ..
        }) if state.session_panel_visible && state.session_rename.is_some() => {
            if let Some(rename) = &mut state.session_rename {
                rename.pop();
            }
            None
        }
        AppAction::Key(KeyEvent {
            code: KeyCode::Char(character),
            ..
        }) if state.session_panel_visible => {
            state.session_search.push(character);
            state.selected_session = 0;
            None
        }
        AppAction::Key(KeyEvent {
            code: KeyCode::Backspace,
            ..
        }) if state.session_panel_visible => {
            state.session_search.pop();
            state.selected_session = 0;
            None
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
            state.conversation_scroll_from_bottom =
                state.conversation_scroll_from_bottom.saturating_add(10);
            None
        }
        AppAction::Key(KeyEvent {
            code: KeyCode::PageDown,
            ..
        }) => {
            state.conversation_scroll_from_bottom =
                state.conversation_scroll_from_bottom.saturating_sub(10);
            None
        }
        AppAction::Key(KeyEvent {
            code: KeyCode::Home,
            ..
        }) => {
            state.conversation_scroll_from_bottom = u16::MAX;
            None
        }
        AppAction::Key(KeyEvent {
            code: KeyCode::End, ..
        }) => {
            state.conversation_scroll_from_bottom = 0;
            None
        }
        AppAction::Key(_) => None,
    }
}

fn merge_run_messages(state: &mut AppState, steps: &[Step], answer: Option<&str>) -> usize {
    let mut run_messages = messages_from_steps(steps);
    if let Some(answer) = answer
        && run_messages.last().is_none_or(|message| {
            message.role != MessageRole::Assistant || message.content != answer
        })
    {
        run_messages.push(Message {
            role: MessageRole::Assistant,
            content: answer.to_owned(),
        });
    }
    let message_base = run_messages
        .iter()
        .find(|message| message.role == MessageRole::User)
        .and_then(|request| {
            state
                .messages
                .iter()
                .rposition(|message| message == request)
        })
        .unwrap_or(state.messages.len());
    state.messages.truncate(message_base);
    state.messages.extend(run_messages);
    message_base
}

fn is_quit_shortcut(state: &AppState, key: KeyEvent) -> bool {
    let character = match state.runtime_info.shortcuts.quit {
        QuitShortcut::CtrlC => 'c',
        QuitShortcut::CtrlQ => 'q',
    };
    key.code == KeyCode::Char(character) && key.modifiers.contains(KeyModifiers::CONTROL)
}

fn is_copy_message_shortcut(state: &AppState, key: KeyEvent) -> bool {
    match state.runtime_info.shortcuts.copy_message {
        CopyMessageShortcut::CtrlY => {
            key.code == KeyCode::Char('y') && key.modifiers.contains(KeyModifiers::CONTROL)
        }
        CopyMessageShortcut::CtrlShiftC => {
            key.code == KeyCode::Char('c')
                && key
                    .modifiers
                    .contains(KeyModifiers::CONTROL | KeyModifiers::SHIFT)
        }
    }
}

fn handle_escape(state: &mut AppState, now: Instant) -> Option<AppCommand> {
    let action = if state.run_status == TuiRunStatus::Running {
        EscapeAction::InterruptRun
    } else {
        EscapeAction::ClearInput
    };
    let confirmed = state
        .pending_escape_action
        .is_some_and(|pending| pending.action == action && pending.expires_at >= now);
    if !confirmed {
        state.pending_escape_action = Some(PendingEscapeAction {
            action,
            expires_at: now + ESCAPE_CONFIRMATION_WINDOW,
        });
        return None;
    }

    state.pending_escape_action = None;
    match action {
        EscapeAction::ClearInput => {
            state.clear_input();
            None
        }
        EscapeAction::InterruptRun => Some(AppCommand::CancelRun),
    }
}

fn execute_selected_slash_command(state: &mut AppState) -> SlashCommandExecution {
    let Some(definition) = state
        .slash_command_suggestions()
        .get(state.selected_slash_command)
        .copied()
    else {
        return SlashCommandExecution::NotSelected;
    };
    state.clear_input();
    match definition.command {
        SlashCommand::NewSession if state.run_status == TuiRunStatus::Running => {
            state.messages.push(Message {
                role: MessageRole::Error,
                content: "Cannot create a session while a run is active.".to_owned(),
            });
            SlashCommandExecution::Executed(None)
        }
        SlashCommand::NewSession => {
            state.begin_new_session();
            SlashCommandExecution::Executed(None)
        }
        SlashCommand::Session => {
            state.session_panel_visible = true;
            state.session_search.clear();
            state.focused_panel = FocusedPanel::Conversation;
            state.selected_session = state
                .filtered_sessions()
                .iter()
                .position(|session| Some(session.id.to_string()) == state.session_id)
                .unwrap_or_default();
            SlashCommandExecution::Executed(None)
        }
        SlashCommand::Version => {
            state.messages.push(Message {
                role: MessageRole::Assistant,
                content: format!("Jux {}", jux_core::version()),
            });
            SlashCommandExecution::Executed(None)
        }
        SlashCommand::Retry if state.run_status == TuiRunStatus::Failed => state
            .runs
            .last()
            .map(|run| run.request.clone())
            .or_else(|| {
                state
                    .messages
                    .iter()
                    .rev()
                    .find(|message| message.role == MessageRole::User)
                    .map(|message| message.content.clone())
            })
            .map_or(SlashCommandExecution::Executed(None), |request| {
                state.begin_token_estimate(&request);
                state.run_status = TuiRunStatus::Running;
                state.messages.push(Message {
                    role: MessageRole::User,
                    content: request.clone(),
                });
                SlashCommandExecution::Executed(Some(AppCommand::StartRun { request }))
            }),
        SlashCommand::Continue if state.run_status == TuiRunStatus::Canceled => state
            .runs
            .last()
            .map(|run| run.request.clone())
            .or_else(|| {
                state
                    .messages
                    .iter()
                    .rev()
                    .find(|message| message.role == MessageRole::User)
                    .map(|message| message.content.clone())
            })
            .map_or(SlashCommandExecution::Executed(None), |request| {
                state.begin_token_estimate(&request);
                state.run_status = TuiRunStatus::Running;
                state.messages.push(Message {
                    role: MessageRole::User,
                    content: request.clone(),
                });
                SlashCommandExecution::Executed(Some(AppCommand::StartRun { request }))
            }),
        SlashCommand::Retry | SlashCommand::Continue => {
            state.messages.push(Message {
                role: MessageRole::Error,
                content: "The current run cannot be restarted in this state.".to_owned(),
            });
            SlashCommandExecution::Executed(None)
        }
    }
}

fn byte_at_character_column(input: &str, start: usize, end: usize, column: usize) -> usize {
    input[start..end]
        .char_indices()
        .nth(column)
        .map_or(end, |(index, _)| start + index)
}

fn generated_session_title(request: &str) -> String {
    let title = request
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("New session")
        .trim();
    let mut shortened = title.chars().take(48).collect::<String>();
    if title.chars().count() > 48 {
        shortened.push('…');
    }
    shortened
}

fn handle_mouse_event(
    state: &mut AppState,
    event: MouseEvent,
    viewport: TuiViewport,
) -> Option<AppCommand> {
    if conversation_area_contains(state, viewport, event.column, event.row) {
        match event.kind {
            MouseEventKind::ScrollUp => {
                state.conversation_scroll_from_bottom = state
                    .conversation_scroll_from_bottom
                    .saturating_add(state.runtime_info.scroll_lines);
                return None;
            }
            MouseEventKind::ScrollDown => {
                state.conversation_scroll_from_bottom = state
                    .conversation_scroll_from_bottom
                    .saturating_sub(state.runtime_info.scroll_lines);
                return None;
            }
            _ => {}
        }
    }
    match event.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            if let Some(index) =
                super::ui::command_toggle_at(state, viewport, event.column, event.row)
            {
                if let Some(item) = state.timeline.get_mut(index) {
                    item.expanded = !item.expanded;
                }
                return None;
            }
            if state.session_panel_visible && event.row >= 3 {
                let index = usize::from(event.row.saturating_sub(3));
                if let Some(session_id) = state
                    .filtered_sessions()
                    .get(index)
                    .map(|session| session.id.clone())
                {
                    state.selected_session = index;
                    return Some(AppCommand::SwitchSession { session_id });
                }
            }
            if state.skill_panel_visible
                && event.column > state.conversation_panel_width(viewport.width)
                && event.row >= 4
            {
                let index = usize::from(event.row.saturating_sub(4));
                if index < state.skills.len() {
                    state.selected_skill = index;
                    let name = state.skills[index].name.clone();
                    if let Some(selected) = state
                        .selected_skill_names
                        .iter()
                        .position(|selected| selected == &name)
                    {
                        state.selected_skill_names.remove(selected);
                    } else {
                        state.selected_skill_names.push(name);
                    }
                    return None;
                }
            }
            let slash_suggestions = state.slash_command_suggestions();
            if !slash_suggestions.is_empty() {
                let input_lines = state.input.lines().count().max(1) as u16;
                let input_top = viewport
                    .height
                    .saturating_sub(1)
                    .saturating_sub(input_lines.saturating_add(2));
                let popup_height = u16::try_from(slash_suggestions.len())
                    .unwrap_or(input_top)
                    .saturating_add(2)
                    .min(input_top.saturating_sub(1));
                let first_row = input_top.saturating_sub(popup_height).saturating_add(1);
                if event.row >= first_row {
                    let index = usize::from(event.row.saturating_sub(first_row));
                    if index < slash_suggestions.len() {
                        state.selected_slash_command = index;
                        return match execute_selected_slash_command(state) {
                            SlashCommandExecution::Executed(command) => command,
                            SlashCommandExecution::NotSelected => None,
                        };
                    }
                }
            }
            if divider_column(state, viewport) == Some(event.column) {
                state.text_selection = None;
                state.text_selection_drag = None;
                if event.row == divider_arrow_row(viewport) {
                    state.sidebar_visible = !state.sidebar_visible;
                    state.divider_dragging = false;
                    if !state.sidebar_visible {
                        state.focused_panel = FocusedPanel::Conversation;
                    }
                } else {
                    state.divider_dragging = state.sidebar_visible;
                }
                return None;
            }
            state.divider_dragging = false;
            if let Some((panel, point)) = selection_point_for_event(state, event, viewport) {
                state.focused_panel = match panel {
                    SelectionPanel::Conversation => FocusedPanel::Conversation,
                    SelectionPanel::Sidebar => FocusedPanel::Sidebar,
                };
                state.text_selection = None;
                state.text_selection_drag = Some(TextSelection {
                    panel,
                    anchor: point,
                    focus: point,
                });
            } else {
                state.text_selection = None;
                state.text_selection_drag = None;
            }
            None
        }
        MouseEventKind::Drag(MouseButton::Left) => {
            if state.divider_dragging {
                resize_panels(state, event.column, viewport);
                return None;
            }
            let selection = state.text_selection_drag?;
            let point = selection_point_for_panel(state, event, viewport, selection.panel);
            let selection = TextSelection {
                focus: point,
                ..selection
            };
            state.text_selection_drag = Some(selection);
            state.text_selection = Some(selection);
            None
        }
        MouseEventKind::Up(MouseButton::Left) => {
            if state.divider_dragging {
                resize_panels(state, event.column, viewport);
                state.divider_dragging = false;
                return None;
            }
            let selection = state.text_selection_drag.take()?;
            let point = selection_point_for_panel(state, event, viewport, selection.panel);
            let selection = TextSelection {
                focus: point,
                ..selection
            };
            let content = selected_text(state, selection);
            if content.is_empty() {
                state.text_selection = None;
                None
            } else {
                state.text_selection = Some(selection);
                state.notify("Copied to clipboard");
                Some(AppCommand::CopyText { content })
            }
        }
        _ => None,
    }
}

fn conversation_area_contains(
    state: &AppState,
    viewport: TuiViewport,
    column: u16,
    row: u16,
) -> bool {
    let width = if viewport.width < 60 {
        viewport.width
    } else {
        state.conversation_panel_width(viewport.width)
    };
    column < width && row < viewport.height
}

fn divider_column(state: &AppState, viewport: TuiViewport) -> Option<u16> {
    (viewport.width >= 60).then(|| state.conversation_panel_width(viewport.width))
}

fn divider_arrow_row(viewport: TuiViewport) -> u16 {
    viewport.height / 2
}

fn resize_panels(state: &mut AppState, column: u16, viewport: TuiViewport) {
    let maximum = viewport
        .width
        .saturating_sub(DIVIDER_WIDTH)
        .saturating_sub(MIN_PANEL_WIDTH);
    let width = column.clamp(MIN_PANEL_WIDTH, maximum);
    state.conversation_width_percent = u16::try_from(
        u32::from(width)
            .saturating_mul(100)
            .checked_div(u32::from(viewport.width))
            .unwrap_or_default(),
    )
    .unwrap_or(DEFAULT_CONVERSATION_WIDTH_PERCENT);
}

fn selection_point_for_event(
    state: &AppState,
    event: MouseEvent,
    viewport: TuiViewport,
) -> Option<(SelectionPanel, TextSelectionPoint)> {
    let geometry = panel_geometries(state, viewport)
        .into_iter()
        .find(|geometry| geometry.contains(event.column, event.row))?;
    Some((
        geometry.panel,
        selection_point_from_geometry(state, event.column, event.row, geometry),
    ))
}

fn selection_point_for_panel(
    state: &AppState,
    event: MouseEvent,
    viewport: TuiViewport,
    panel: SelectionPanel,
) -> TextSelectionPoint {
    let geometry = panel_geometries(state, viewport)
        .into_iter()
        .find(|geometry| geometry.panel == panel)
        .expect("selection panel geometry exists");
    selection_point_from_geometry(state, event.column, event.row, geometry)
}

fn selection_point_from_geometry(
    state: &AppState,
    column: u16,
    row: u16,
    geometry: PanelGeometry,
) -> TextSelectionPoint {
    let lines = state.panel_text_lines(geometry.panel);
    let line_count = lines.len().max(1);
    let clamped_row = row.clamp(
        geometry.y,
        geometry.y.saturating_add(geometry.height.saturating_sub(1)),
    );
    let clamped_column = column.clamp(
        geometry.x,
        geometry.x.saturating_add(geometry.width.saturating_sub(1)),
    );
    let mut line = usize::from(clamped_row.saturating_sub(geometry.y));
    if geometry.panel == SelectionPanel::Conversation {
        line = line.saturating_add(conversation_scroll_offset(state, geometry));
    }
    line = line.min(line_count.saturating_sub(1));
    let column = usize::from(clamped_column.saturating_sub(geometry.x))
        .min(lines.get(line).map_or(0, |line| line.chars().count()));
    TextSelectionPoint { line, column }
}

fn panel_geometries(state: &AppState, viewport: TuiViewport) -> Vec<PanelGeometry> {
    let conversation = conversation_geometry(state, viewport);
    if viewport.width < 60 {
        return vec![conversation];
    }
    let left_width = state.conversation_panel_width(viewport.width);
    let mut geometries = vec![conversation];
    if state.sidebar_visible {
        geometries.push(content_geometry(
            SelectionPanel::Sidebar,
            left_width.saturating_add(DIVIDER_WIDTH),
            0,
            viewport
                .width
                .saturating_sub(left_width)
                .saturating_sub(DIVIDER_WIDTH),
            viewport.height,
        ));
    }
    geometries
}

fn conversation_geometry(state: &AppState, viewport: TuiViewport) -> PanelGeometry {
    let width = if viewport.width < 60 {
        viewport.width
    } else {
        state.conversation_panel_width(viewport.width)
    };
    content_geometry(SelectionPanel::Conversation, 0, 0, width, viewport.height)
}

fn conversation_scroll_offset(state: &AppState, geometry: PanelGeometry) -> usize {
    let width = usize::from(geometry.width.max(1));
    let total_rows = state
        .conversation_text_lines()
        .iter()
        .map(|line| UnicodeWidthStr::width(line.as_str()).max(1).div_ceil(width))
        .sum::<usize>();
    let maximum = total_rows.saturating_sub(usize::from(geometry.height));
    maximum.saturating_sub(usize::from(state.conversation_scroll_from_bottom).min(maximum))
}

fn content_geometry(
    panel: SelectionPanel,
    x: u16,
    y: u16,
    width: u16,
    height: u16,
) -> PanelGeometry {
    let padding = match panel {
        SelectionPanel::Conversation => 1,
        SelectionPanel::Sidebar => 2,
    };
    PanelGeometry {
        panel,
        x: x.saturating_add(padding),
        y: y.saturating_add(padding),
        width: width.saturating_sub(padding.saturating_mul(2)),
        height: height.saturating_sub(padding.saturating_mul(2)),
    }
}

impl PanelGeometry {
    fn contains(self, column: u16, row: u16) -> bool {
        column >= self.x
            && column < self.x.saturating_add(self.width)
            && row >= self.y
            && row < self.y.saturating_add(self.height)
    }
}

fn selected_text(state: &AppState, selection: TextSelection) -> String {
    let lines = state.panel_text_lines(selection.panel);
    let (start, end) = ordered_points(selection.anchor, selection.focus);
    lines
        .iter()
        .enumerate()
        .filter_map(|(index, line)| {
            if index < start.line || index > end.line {
                return None;
            }
            let start_column = if index == start.line { start.column } else { 0 };
            let end_column = if index == end.line {
                end.column
            } else {
                line.chars().count()
            };
            Some(slice_chars(line, start_column, end_column))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn ordered_points(
    first: TextSelectionPoint,
    second: TextSelectionPoint,
) -> (TextSelectionPoint, TextSelectionPoint) {
    if (first.line, first.column) <= (second.line, second.column) {
        (first, second)
    } else {
        (second, first)
    }
}

fn slice_chars(line: &str, start: usize, end: usize) -> String {
    line.chars()
        .skip(start)
        .take(end.saturating_sub(start))
        .collect()
}

fn conversation_text_lines(state: &AppState) -> Vec<String> {
    let mut lines = Vec::new();
    append_timeline_text_lines(state, 0, &mut lines);
    for (message_index, message) in state.messages().iter().enumerate() {
        match message.role {
            MessageRole::User => {
                lines.push(String::new());
                let marker = if state.selected_message() == Some(message_index) {
                    "▶"
                } else {
                    ">"
                };
                lines.extend(
                    message
                        .content
                        .split('\n')
                        .map(|line| format!("\u{00a0}{marker} {line}")),
                );
                lines.push(String::new());
            }
            MessageRole::Assistant => {
                lines.push(String::new());
                lines.extend(
                    message.content.lines().map(|line| {
                        format!("\u{00a0}\u{00a0}\u{00a0}{line}\u{00a0}\u{00a0}\u{00a0}")
                    }),
                );
                lines.push(String::new());
            }
            MessageRole::Error => {
                lines.push("Error".to_owned());
                lines.extend(message.content.lines().map(str::to_owned));
                lines.push(String::new());
            }
        }
        append_timeline_text_lines(state, message_index.saturating_add(1), &mut lines);
    }
    if !state.timeline().is_empty() {
        lines.push(String::new());
    }
    lines
}

fn append_timeline_text_lines(state: &AppState, message_count: usize, lines: &mut Vec<String>) {
    for item in state
        .timeline()
        .iter()
        .filter(|item| item.message_count == message_count)
    {
        let status = match item.status {
            TimelineStatus::Running => "Running",
            TimelineStatus::Output => "Output",
            TimelineStatus::Completed => "Completed",
            TimelineStatus::Failed => "Failed",
        };
        lines.push(format!("{}  {status}", item.label));
        if let Some(detail) = &item.detail {
            lines.push(detail.clone());
        }
        if let Some(output) = &item.output {
            let summary = output.split_whitespace().collect::<Vec<_>>().join(" ");
            lines.push(truncate_timeline_detail_text(&summary));
        }
    }
}

fn sidebar_text_lines(state: &AppState) -> Vec<String> {
    if state.help_visible() {
        return vec![
            "Commands".to_owned(),
            "/help  Show help".to_owned(),
            "/clear Clear messages".to_owned(),
            "/quit  Quit Jux".to_owned(),
            "/new   Start a new session".to_owned(),
            "/version Show the Jux version".to_owned(),
            "/skills Browse and select skills".to_owned(),
            "/logs   Show runtime logs".to_owned(),
        ];
    }
    let status = match state.run_status() {
        TuiRunStatus::Idle => "Idle",
        TuiRunStatus::Running => "Running",
        TuiRunStatus::WaitingForHumanInput => "Waiting",
        TuiRunStatus::Completed => "Completed",
        TuiRunStatus::Failed => "Failed",
        TuiRunStatus::Canceled => "Canceled",
    };
    vec![
        "Jux".to_owned(),
        String::new(),
        format!("Session: {}", state.session_id().unwrap_or("-")),
        format!("Run: {}", state.run_id().unwrap_or("-")),
        format!(
            "Model: {}/{}",
            state.runtime_info().model_provider,
            state.runtime_info().model_name
        ),
        "Focus: Left/Right".to_owned(),
        "Quit: Ctrl+C".to_owned(),
        String::new(),
        format!("Status: {status}"),
        match state.run_elapsed_millis() {
            Some(millis) => format!("Elapsed: {millis} ms"),
            None => "Elapsed: -".to_owned(),
        },
        String::new(),
        format!("Workspace: {}", state.workspace_root.display()),
        format!(
            "Workspace ID: {}",
            state.runtime_info().workspace_id.as_deref().unwrap_or("-")
        ),
    ]
}

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
        expanded: false,
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
