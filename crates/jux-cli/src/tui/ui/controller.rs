use super::*;
use crate::tui::ui::state::*;
use crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use std::ops::{Deref, DerefMut};
use std::time::{Duration, Instant};

const DEFAULT_CONVERSATION_WIDTH_PERCENT: u16 = 60;
const MIN_PANEL_WIDTH: u16 = 20;
const DIVIDER_WIDTH: u16 = 1;
const ESCAPE_CONFIRMATION_WINDOW: Duration = Duration::from_secs(1);
const NOTIFICATION_DURATION: Duration = Duration::from_secs(2);

enum SlashCommandExecution {
    NotSelected,
    Executed(Option<AppCommand>),
}

pub(crate) struct UiStateRefs<'a> {
    pub prompt: &'a mut PromptState,
    pub conversation: &'a mut ConversationUiState,
    pub sidebar: &'a mut SidebarUiState,
    pub overlay: &'a mut OverlayUiState,
    pub root: &'a mut RootUiState,
}

struct UiContext<'a> {
    model: &'a mut AppModel,
    prompt: &'a mut PromptState,
    conversation_ui: &'a mut ConversationUiState,
    sidebar_ui: &'a mut SidebarUiState,
    overlay_ui: &'a mut OverlayUiState,
    root_ui: &'a mut RootUiState,
}

impl<'a> UiContext<'a> {
    fn new(model: &'a mut AppModel, ui: UiStateRefs<'a>) -> Self {
        Self {
            model,
            prompt: ui.prompt,
            conversation_ui: ui.conversation,
            sidebar_ui: ui.sidebar,
            overlay_ui: ui.overlay,
            root_ui: ui.root,
        }
    }

    fn clear_input(&mut self) {
        self.prompt.input.clear();
        self.prompt.cursor = 0;
        self.prompt.selected_slash_command = 0;
        self.prompt.selected_inline_skill = 0;
        self.prompt.selected_file_reference = 0;
        self.prompt.slash_commands_dismissed = false;
        self.prompt.pending_escape_action = None;
        self.prompt.input_history_index = None;
        self.prompt.input_history_draft.clear();
    }

    fn insert(&mut self, character: char) {
        self.remember_undo_state();
        self.prompt.input.insert(self.prompt.cursor, character);
        self.prompt.cursor += character.len_utf8();
        self.prompt.selected_slash_command = 0;
        self.prompt.selected_inline_skill = 0;
        self.prompt.selected_file_reference = 0;
        self.prompt.slash_commands_dismissed = false;
    }

    fn move_cursor_left(&mut self) {
        if let Some((index, _)) = self.prompt.input[..self.prompt.cursor]
            .char_indices()
            .next_back()
        {
            self.prompt.cursor = index;
        }
    }

    fn move_cursor_right(&mut self) {
        if let Some(character) = self.prompt.input[self.prompt.cursor..].chars().next() {
            self.prompt.cursor += character.len_utf8();
        }
    }

    fn move_cursor_up(&mut self) {
        let line_start = self.prompt.input[..self.prompt.cursor]
            .rfind('\n')
            .map_or(0, |index| index + 1);
        if line_start == 0 {
            return;
        }
        let column = self.prompt.input[line_start..self.prompt.cursor]
            .chars()
            .count();
        let previous_end = line_start - 1;
        let previous_start = self.prompt.input[..previous_end]
            .rfind('\n')
            .map_or(0, |index| index + 1);
        self.prompt.cursor =
            byte_at_character_column(&self.prompt.input, previous_start, previous_end, column);
    }

    fn move_cursor_down(&mut self) {
        let line_start = self.prompt.input[..self.prompt.cursor]
            .rfind('\n')
            .map_or(0, |index| index + 1);
        let column = self.prompt.input[line_start..self.prompt.cursor]
            .chars()
            .count();
        let Some(relative_line_end) = self.prompt.input[self.prompt.cursor..].find('\n') else {
            return;
        };
        let next_start = self.prompt.cursor + relative_line_end + 1;
        let next_end = self.prompt.input[next_start..]
            .find('\n')
            .map_or(self.prompt.input.len(), |index| next_start + index);
        self.prompt.cursor =
            byte_at_character_column(&self.prompt.input, next_start, next_end, column);
    }

    fn delete_before_cursor(&mut self) {
        if let Some((start, end)) = self.file_reference_range_at(self.prompt.cursor, true) {
            self.remember_undo_state();
            self.prompt.input.drain(start..end);
            self.prompt.cursor = start;
            self.prompt.selected_file_reference = 0;
            return;
        }
        let Some((index, _)) = self.prompt.input[..self.prompt.cursor]
            .char_indices()
            .next_back()
        else {
            return;
        };
        self.remember_undo_state();
        self.prompt.input.drain(index..self.prompt.cursor);
        self.prompt.cursor = index;
        self.prompt.selected_slash_command = 0;
        self.prompt.slash_commands_dismissed = false;
    }

    fn delete_at_cursor(&mut self) {
        if let Some((start, end)) = self.file_reference_range_at(self.prompt.cursor, false) {
            self.remember_undo_state();
            self.prompt.input.drain(start..end);
            self.prompt.cursor = start;
            self.prompt.selected_file_reference = 0;
            return;
        }
        let Some(character) = self.prompt.input[self.prompt.cursor..].chars().next() else {
            return;
        };
        self.remember_undo_state();
        self.prompt
            .input
            .drain(self.prompt.cursor..self.prompt.cursor + character.len_utf8());
        self.prompt.selected_slash_command = 0;
        self.prompt.slash_commands_dismissed = false;
    }

    fn undo_edit(&mut self) {
        if let Some((input, cursor)) = self.prompt.undo_input.take() {
            self.prompt.input = input;
            self.prompt.cursor = cursor;
        }
    }

    fn move_cursor_word_left(&mut self) {
        let before = &self.prompt.input[..self.prompt.cursor];
        let trimmed = before.trim_end_matches(char::is_whitespace);
        self.prompt.cursor = trimmed.rfind(char::is_whitespace).map_or(0, |index| {
            index + trimmed[index..].chars().next().map_or(0, char::len_utf8)
        });
    }

    fn move_cursor_word_right(&mut self) {
        let after = &self.prompt.input[self.prompt.cursor..];
        let word_end = after.find(char::is_whitespace).unwrap_or(after.len());
        let rest = &after[word_end..];
        let whitespace_end = rest
            .find(|character: char| !character.is_whitespace())
            .unwrap_or(rest.len());
        self.prompt.cursor += word_end + whitespace_end;
    }

    fn move_cursor_to_line_start(&mut self) {
        self.prompt.cursor = self.prompt.input[..self.prompt.cursor]
            .rfind('\n')
            .map_or(0, |index| index + 1);
    }

    fn move_cursor_to_line_end(&mut self) {
        self.prompt.cursor = self.prompt.input[self.prompt.cursor..]
            .find('\n')
            .map_or(self.prompt.input.len(), |index| self.prompt.cursor + index);
    }

    fn delete_current_line(&mut self) {
        if self.prompt.input.is_empty() {
            return;
        }
        self.remember_undo_state();
        let start = self.prompt.input[..self.prompt.cursor]
            .rfind('\n')
            .map_or(0, |index| index + 1);
        let end = self.prompt.input[self.prompt.cursor..]
            .find('\n')
            .map_or(self.prompt.input.len(), |index| {
                self.prompt.cursor + index + 1
            });
        self.prompt.input.drain(start..end);
        self.prompt.cursor = start.min(self.prompt.input.len());
    }

    fn browse_input_history(&mut self, previous: bool) {
        if self.prompt.input_history.is_empty() {
            return;
        }
        let index = match (self.prompt.input_history_index, previous) {
            (None, true) => {
                self.prompt.input_history_draft = self.prompt.input.clone();
                self.prompt.input_history.len() - 1
            }
            (Some(index), true) => index.saturating_sub(1),
            (Some(index), false) if index + 1 < self.prompt.input_history.len() => index + 1,
            (Some(_), false) => {
                self.prompt.input_history_index = None;
                self.prompt.input = std::mem::take(&mut self.prompt.input_history_draft);
                self.prompt.cursor = self.prompt.input.len();
                return;
            }
            (None, false) => return,
        };
        self.prompt.input_history_index = Some(index);
        self.prompt
            .input
            .clone_from(&self.prompt.input_history[index]);
        self.prompt.cursor = self.prompt.input.len();
    }

    fn complete_file_reference(&mut self, finish_token: bool) {
        let Some(path) = self
            .file_reference_suggestion(self.prompt.selected_file_reference)
            .map(str::to_owned)
        else {
            return;
        };
        let token_start = self.prompt.input[..self.prompt.cursor]
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
        self.prompt
            .input
            .replace_range(token_start..self.prompt.cursor, &reference);
        self.prompt.cursor = token_start + reference.len();
        self.prompt.selected_file_reference = 0;
    }

    fn complete_inline_skill(&mut self) {
        let Some(name) = self
            .inline_skill_suggestions()
            .get(self.prompt.selected_inline_skill)
            .map(|skill| skill.name.clone())
        else {
            return;
        };
        let token_start = self.prompt.input[..self.prompt.cursor]
            .rfind(char::is_whitespace)
            .map_or(0, |index| index + 1);
        self.remember_undo_state();
        self.prompt
            .input
            .replace_range(token_start..self.prompt.cursor, &format!("${name}"));
        self.prompt.cursor = token_start + name.len() + 1;
        if !self.model.selected_skill_names.contains(&name) {
            self.model.selected_skill_names.push(name);
        }
    }

    fn slash_command_suggestions(&self) -> Vec<SlashCommandDefinition> {
        if self.prompt.slash_commands_dismissed {
            return Vec::new();
        }
        let Some(query) = self.prompt.input.strip_prefix('/') else {
            return Vec::new();
        };
        if query.chars().any(char::is_whitespace) {
            return Vec::new();
        }
        SLASH_COMMANDS
            .iter()
            .copied()
            .filter(|definition| match definition.command {
                SlashCommand::Retry => self.model.run_status == TuiRunStatus::Failed,
                SlashCommand::Continue => self.model.run_status == TuiRunStatus::Canceled,
                _ => true,
            })
            .filter(|definition| definition.name.trim_start_matches('/').starts_with(query))
            .collect()
    }

    fn inline_skill_suggestions(&self) -> Vec<&SkillDefinition> {
        let token_start = self.prompt.input[..self.prompt.cursor]
            .rfind(char::is_whitespace)
            .map_or(0, |index| index + 1);
        let token = &self.prompt.input[token_start..self.prompt.cursor];
        let Some(query) = token.strip_prefix('$') else {
            return Vec::new();
        };
        let query = query.to_lowercase();
        self.model
            .skills
            .iter()
            .filter(|skill| skill.name.to_lowercase().contains(&query))
            .collect()
    }

    fn file_reference_suggestion_count(&self) -> usize {
        match &self.prompt.file_reference_cache.matches {
            FileReferenceMatches::Disabled => 0,
            FileReferenceMatches::AllFiles => self.model.indexed_files.len(),
            FileReferenceMatches::Filtered(matches) => matches.len(),
        }
    }

    fn file_reference_suggestion(&self, index: usize) -> Option<&str> {
        let file_index = match &self.prompt.file_reference_cache.matches {
            FileReferenceMatches::Disabled => return None,
            FileReferenceMatches::AllFiles => index,
            FileReferenceMatches::Filtered(matches) => *matches.get(index)?,
        };
        self.model.indexed_files.get(file_index).map(String::as_str)
    }

    fn file_reference_range_at(&self, cursor: usize, backwards: bool) -> Option<(usize, usize)> {
        let probe = if backwards {
            cursor.checked_sub(1)?
        } else {
            cursor
        };
        reference_ranges(&self.prompt.input)
            .into_iter()
            .find(|(start, end)| {
                *start <= probe
                    && probe < *end
                    && self.reference_path_exists(&self.prompt.input[*start..*end])
            })
    }

    fn reference_path_exists(&self, reference: &str) -> bool {
        let path = reference
            .strip_prefix("@{")
            .and_then(|path| path.strip_suffix('}'))
            .or_else(|| reference.strip_prefix('@'));
        path.is_some_and(|path| self.model.indexed_files.iter().any(|item| item == path))
    }

    fn remember_undo_state(&mut self) {
        self.prompt.undo_input = Some((self.prompt.input.clone(), self.prompt.cursor));
        self.prompt.input_history_index = None;
    }

    fn refresh_file_reference_cache(&mut self) {
        let token_start = self.prompt.input[..self.prompt.cursor]
            .rfind(char::is_whitespace)
            .map_or(0, |index| index + 1);
        let token = &self.prompt.input[token_start..self.prompt.cursor];
        let query = token
            .strip_prefix('@')
            .filter(|query| !query.starts_with('{'))
            .map(str::to_owned);
        if self.prompt.file_reference_cache.index_revision == self.model.file_index_revision
            && self.prompt.file_reference_cache.query == query
        {
            return;
        }
        let matches = match query.as_deref() {
            None => FileReferenceMatches::Disabled,
            Some("") => FileReferenceMatches::AllFiles,
            Some(query) => {
                let query = query.to_lowercase();
                FileReferenceMatches::Filtered(
                    self.model
                        .indexed_files
                        .iter()
                        .enumerate()
                        .filter_map(|(index, path)| fuzzy_path_match(path, &query).then_some(index))
                        .collect(),
                )
            }
        };
        self.prompt.file_reference_cache = FileReferenceCache {
            index_revision: self.model.file_index_revision,
            query,
            matches,
        };
        let count = self.file_reference_suggestion_count();
        self.prompt.selected_file_reference = if count == 0 {
            0
        } else {
            self.prompt.selected_file_reference % count
        };
    }

    fn notify(&mut self, message: impl Into<String>) {
        self.root_ui.notification = Some((message.into(), Instant::now() + NOTIFICATION_DURATION));
    }

    fn set_file_index(&mut self, snapshot: FileIndexSnapshot) {
        self.model.indexed_files = snapshot.files;
        self.model.file_index_revision = self.model.file_index_revision.wrapping_add(1);
        self.prompt.selected_file_reference = 0;
    }

    fn begin_new_session(&mut self) {
        self.model.pending_new_session = true;
        self.model.session_id = None;
        self.model.run_id = None;
        self.model.run_elapsed_millis = None;
        self.model.clear_messages();
        self.model.timeline.clear();
        self.model.steps.clear();
        self.prompt.input_history.clear();
        self.prompt.input_history_index = None;
        self.prompt.input_history_draft.clear();
        self.model.run_status = TuiRunStatus::Idle;
        self.sidebar_ui.session_panel_visible = false;
        self.notify("New session");
    }

    fn filtered_sessions(&self) -> Vec<&Session> {
        let query = self.sidebar_ui.session_search.to_lowercase();
        let mut sessions = self
            .model
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
        self.filtered_sessions()
            .get(self.sidebar_ui.selected_session)
            .copied()
    }

    fn conversation_panel_width(&self, viewport_width: u16) -> u16 {
        if viewport_width < 60 {
            return viewport_width;
        }
        if !self.sidebar_ui.sidebar_visible {
            return viewport_width.saturating_sub(DIVIDER_WIDTH);
        }
        let maximum = viewport_width
            .saturating_sub(DIVIDER_WIDTH)
            .saturating_sub(MIN_PANEL_WIDTH);
        viewport_width
            .saturating_mul(self.sidebar_ui.conversation_width_percent)
            .checked_div(100)
            .unwrap_or_default()
            .clamp(MIN_PANEL_WIDTH, maximum)
    }

    fn selected_message(&self) -> Option<usize> {
        self.conversation_ui.selected_message
    }

    fn help_visible(&self) -> bool {
        self.sidebar_ui.help_visible
    }

    fn panel_text_lines(&self, panel: SelectionPanel) -> Vec<String> {
        match panel {
            SelectionPanel::Conversation => conversation_text_lines(self),
            SelectionPanel::Sidebar => sidebar_text_lines(self),
        }
    }

    fn filtered_audit_items(&self) -> Vec<&AuditItem> {
        self.model
            .audit_items
            .iter()
            .filter(|item| match self.sidebar_ui.audit_filter {
                AuditFilter::All => true,
                AuditFilter::Files => item.title.starts_with("File"),
                AuditFilter::Commands => {
                    item.title.starts_with("Tool") || item.title.contains("command")
                }
                AuditFilter::Policy => item.title.starts_with("Policy"),
            })
            .collect()
    }
}

impl Deref for UiContext<'_> {
    type Target = AppModel;

    fn deref(&self) -> &Self::Target {
        self.model
    }
}

impl DerefMut for UiContext<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.model
    }
}

pub fn update(state: &mut AppModel, action: AppMsg) -> Option<AppCommand> {
    let prompt = &mut PromptState::default();
    let conversation = &mut ConversationUiState::default();
    let sidebar = &mut SidebarUiState::default();
    let overlay = &mut OverlayUiState::default();
    let root = &mut RootUiState::default();
    update_with_ui(
        state,
        action,
        UiStateRefs {
            prompt,
            conversation,
            sidebar,
            overlay,
            root,
        },
    )
}

pub(crate) fn update_with_ui(
    state: &mut AppModel,
    action: AppMsg,
    ui: UiStateRefs<'_>,
) -> Option<AppCommand> {
    let mut context = UiContext::new(state, ui);
    apply_app_msg(&mut context, action);
    context.refresh_file_reference_cache();
    None
}

fn apply_app_msg(state: &mut UiContext<'_>, message: AppMsg) {
    match message {
        AppMsg::UpdateAvailable {
            notice,
            show_startup_message,
        } => {
            if show_startup_message {
                let mut content = format!(
                    "Jux {} is available (current {}). {}",
                    notice.latest_version, notice.current_version, notice.recommendation.guidance
                );
                if let Some(command) = &notice.recommendation.command {
                    content.push_str(&format!("\nRun: {}", command.display()));
                }
                state.push_message(Message {
                    role: MessageRole::Assistant,
                    content,
                });
            }
            state.update_notice = Some(notice);
        }
        AppMsg::FileIndexUpdated(snapshot) => state.set_file_index(snapshot),
        AppMsg::CodeChangeProposed { proposal } => {
            append_proposal_audit(&mut state.audit_items, &proposal);
            state.code_change_review = Some(CodeChangeReview::new(proposal));
            state.overlay_ui.selected_changed_file = 0;
            state.code_change_result = None;
        }
        AppMsg::AgentEvent(event) => apply_agent_event(state, event),
        AppMsg::AssistantMessage { content } => {
            state.estimated_output_tokens = state
                .estimated_output_tokens
                .saturating_add(estimate_tokens(&content));
            state.push_message(Message {
                role: MessageRole::Assistant,
                content,
            });
        }
        AppMsg::RunFinished { response } => apply_run_finished(state, response),
        AppMsg::RunFailed { error } => {
            state.prompt.pending_escape_action = None;
            state.run_status = TuiRunStatus::Failed;
            state.runtime_logs.push(TuiRuntimeLog {
                title: "Run failed".to_owned(),
                detail: Some(error.clone()),
            });
            state.push_message(Message {
                role: MessageRole::Error,
                content: error,
            });
        }
        AppMsg::RunCanceled => {
            state.prompt.pending_escape_action = None;
            state.run_status = TuiRunStatus::Canceled;
            let run_id = state.run_id.clone();
            state.runtime_logs.push(TuiRuntimeLog {
                title: "Run canceled".to_owned(),
                detail: run_id,
            });
        }
    }
}

fn apply_agent_event(state: &mut UiContext<'_>, event: AgentEvent) {
    if event.sequence > 0 && matches!(&event.data, AgentEventData::RunStarted { .. }) {
        state.last_agent_event_sequence = 0;
    }
    if event.sequence > 0 && event.sequence <= state.last_agent_event_sequence {
        return;
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
            Some(index) => state.append_message_content(index, content),
            None => {
                state.push_message(Message {
                    role: MessageRole::Assistant,
                    content: content.clone(),
                });
                state.streaming_assistant_message = Some(state.messages.len() - 1);
            }
        }
        return;
    }
    if matches!(&event.data, AgentEventData::LlmCompleted) {
        let completed_id = event.id.to_string();
        state.timeline.retain(|item| item.id != completed_id);
        state.conversation_ui.selected_timeline = None;
        return;
    }
    if let AgentEventData::ToolOutput { name, content } = &event.data
        && name == PROPOSE_CODE_CHANGE_TOOL_NAME
        && let Ok(proposal) = serde_json::from_value::<CodeChangeProposal>(content.clone())
    {
        append_proposal_audit(&mut state.audit_items, &proposal);
        state.code_change_review = Some(CodeChangeReview::new(proposal));
        state.overlay_ui.selected_changed_file = 0;
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
                if item.label == "Skill" && state.timeline[index].label.starts_with("Skill:") {
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
                item.command =
                    merge_command_execution(state.timeline[index].command.clone(), item.command);
                state.timeline[index] = item;
            }
            None => state.timeline.push(item),
        }
    }
}

fn apply_run_finished(state: &mut UiContext<'_>, response: RunResponse) {
    state.prompt.pending_escape_action = None;
    if let Some(index) = state.streaming_assistant_message.take()
        && index < state.messages.len()
    {
        state.remove_message(index);
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
    state.run_elapsed_millis = Some(response.updated_at.saturating_sub(response.created_at));
    state.steps = response.steps;
    state.steps.sort_by_key(|step| step.id.to_string());
    let run_steps = state.steps.clone();
    let message_base = merge_run_messages(state, &run_steps, response.answer.as_deref());
    state.timeline = command_timeline_from_steps(&state.steps);
    for item in &mut state.timeline {
        item.message_count = item.message_count.saturating_add(message_base);
    }
    state.conversation_ui.selected_timeline = None;
    state.audit_items = audit_items_from_steps(&state.steps);
    state.pending_human_input = waiting_for_human_input
        .then(|| latest_human_input_request(&state.steps))
        .flatten();
    state.overlay_ui.selected_human_option = 0;
    state.overlay_ui.human_input_error = None;
    let status = state.run_status;
    let run_id = state.run_id.clone();
    state.runtime_logs.push(TuiRuntimeLog {
        title: format!("Run finished: {status:?}"),
        detail: run_id,
    });
}

pub(crate) fn update_ui(
    state: &mut AppModel,
    event: crate::tui::UiEvent,
    viewport: TuiViewport,
    ui: UiStateRefs<'_>,
) -> Option<AppCommand> {
    let mut context = UiContext::new(state, ui);
    let command = handle_ui_event(&mut context, event, viewport);
    context.refresh_file_reference_cache();
    command
}

fn handle_ui_event(
    state: &mut UiContext<'_>,
    event: crate::tui::UiEvent,
    viewport: TuiViewport,
) -> Option<AppCommand> {
    if matches!(
        &event,
        crate::tui::UiEvent::Key(key)
            if key.kind != KeyEventKind::Release && key.code != KeyCode::Esc
    ) || matches!(&event, crate::tui::UiEvent::Mouse(_))
    {
        state.prompt.pending_escape_action = None;
    }
    match event {
        crate::tui::UiEvent::Paste(content) => {
            for character in content.chars() {
                state.insert(character);
            }
            None
        }
        crate::tui::UiEvent::Mouse(event) => handle_mouse_event(state, event, viewport),
        crate::tui::UiEvent::Key(KeyEvent {
            kind: KeyEventKind::Release,
            ..
        }) => None,
        crate::tui::UiEvent::Key(KeyEvent {
            code: KeyCode::Esc, ..
        }) if !state.slash_command_suggestions().is_empty() => {
            state.prompt.pending_escape_action = None;
            state.prompt.slash_commands_dismissed = true;
            None
        }
        crate::tui::UiEvent::Key(KeyEvent {
            code: KeyCode::Esc, ..
        }) if state.sidebar_ui.log_panel_visible => {
            state.prompt.pending_escape_action = None;
            state.sidebar_ui.log_panel_visible = false;
            None
        }
        crate::tui::UiEvent::Key(KeyEvent {
            code: KeyCode::Esc, ..
        }) if state.sidebar_ui.skill_panel_visible => {
            state.prompt.pending_escape_action = None;
            state.sidebar_ui.skill_panel_visible = false;
            None
        }
        crate::tui::UiEvent::Key(KeyEvent {
            code: KeyCode::Esc, ..
        }) if state.sidebar_ui.session_rename.is_some() => {
            state.sidebar_ui.session_rename = None;
            None
        }
        crate::tui::UiEvent::Key(KeyEvent {
            code: KeyCode::Esc, ..
        }) if state.sidebar_ui.session_panel_visible => {
            state.prompt.pending_escape_action = None;
            state.sidebar_ui.session_panel_visible = false;
            state.sidebar_ui.session_search.clear();
            state.sidebar_ui.session_rename = None;
            state.sidebar_ui.selected_session = 0;
            None
        }
        crate::tui::UiEvent::Key(KeyEvent {
            code: KeyCode::Char('d'),
            modifiers: KeyModifiers::CONTROL,
            ..
        }) if state.sidebar_ui.session_panel_visible => {
            state
                .selected_filtered_session()
                .map(|session| AppCommand::DeleteSession {
                    session_id: session.id.clone(),
                })
        }
        crate::tui::UiEvent::Key(KeyEvent {
            code: KeyCode::Esc, ..
        }) => handle_escape(state, Instant::now()),
        crate::tui::UiEvent::Key(KeyEvent {
            code: KeyCode::Char('f'),
            ..
        }) if state.sidebar_ui.audit_panel_visible => {
            state.sidebar_ui.audit_filter = match state.sidebar_ui.audit_filter {
                AuditFilter::All => AuditFilter::Files,
                AuditFilter::Files => AuditFilter::Commands,
                AuditFilter::Commands => AuditFilter::Policy,
                AuditFilter::Policy => AuditFilter::All,
            };
            state.sidebar_ui.selected_audit_item = 0;
            None
        }
        crate::tui::UiEvent::Key(KeyEvent {
            code: KeyCode::Up, ..
        }) if state.sidebar_ui.audit_panel_visible => {
            state.sidebar_ui.selected_audit_item =
                state.sidebar_ui.selected_audit_item.saturating_sub(1);
            None
        }
        crate::tui::UiEvent::Key(KeyEvent {
            code: KeyCode::Down,
            ..
        }) if state.sidebar_ui.audit_panel_visible => {
            let maximum = state.filtered_audit_items().len().saturating_sub(1);
            state.sidebar_ui.selected_audit_item =
                (state.sidebar_ui.selected_audit_item + 1).min(maximum);
            None
        }
        crate::tui::UiEvent::Key(key) if is_quit_shortcut(state, key) => {
            state.should_quit = true;
            None
        }
        crate::tui::UiEvent::Key(key)
            if key.code == KeyCode::Up && key.modifiers.contains(KeyModifiers::ALT) =>
        {
            if !state.messages.is_empty() {
                state.conversation_ui.selected_message = Some(
                    state
                        .conversation_ui
                        .selected_message
                        .unwrap_or(state.messages.len())
                        .saturating_sub(1),
                );
            }
            None
        }
        crate::tui::UiEvent::Key(key)
            if key.code == KeyCode::Down && key.modifiers.contains(KeyModifiers::ALT) =>
        {
            if !state.messages.is_empty() {
                state.conversation_ui.selected_message = Some(
                    state
                        .conversation_ui
                        .selected_message
                        .map_or(0, |index| (index + 1).min(state.messages.len() - 1)),
                );
            }
            None
        }
        crate::tui::UiEvent::Key(key) if is_copy_message_shortcut(state, key) => state
            .conversation_ui
            .selected_message
            .and_then(|index| state.messages.get(index))
            .map(|message| message.content.clone())
            .map(|content| {
                state.notify("Copied to clipboard");
                AppCommand::CopyText { content }
            }),
        crate::tui::UiEvent::Key(key)
            if key.code == KeyCode::Char('e') && key.modifiers.contains(KeyModifiers::CONTROL) =>
        {
            if let Some(message) = state
                .conversation_ui
                .selected_message
                .and_then(|index| state.messages.get(index))
                .filter(|message| message.role == MessageRole::User)
                .cloned()
            {
                state.prompt.input = message.content;
                state.prompt.cursor = state.prompt.input.len();
                state.notify("Message loaded for editing");
            }
            None
        }
        crate::tui::UiEvent::Key(key)
            if key.code == KeyCode::Enter && key.modifiers.contains(KeyModifiers::SHIFT) =>
        {
            state.insert('\n');
            None
        }
        crate::tui::UiEvent::Key(KeyEvent {
            code: KeyCode::Tab, ..
        }) if !state.slash_command_suggestions().is_empty() => {
            if let Some(command) = state
                .slash_command_suggestions()
                .get(state.prompt.selected_slash_command)
            {
                state.prompt.input = command.name.to_owned();
                state.prompt.cursor = state.prompt.input.len();
            }
            None
        }
        crate::tui::UiEvent::Key(KeyEvent {
            code: KeyCode::Tab, ..
        }) if !state.inline_skill_suggestions().is_empty() => {
            state.complete_inline_skill();
            None
        }
        crate::tui::UiEvent::Key(KeyEvent {
            code: KeyCode::Tab, ..
        }) if state.file_reference_suggestion_count() > 0 => {
            state.complete_file_reference(false);
            None
        }
        crate::tui::UiEvent::Key(KeyEvent {
            code: KeyCode::Char(' '),
            ..
        }) if state.sidebar_ui.skill_panel_visible => {
            if let Some(name) = state
                .skills
                .get(state.sidebar_ui.selected_skill)
                .map(|skill| skill.name.clone())
            {
                if let Some(index) = state
                    .selected_skill_names
                    .iter()
                    .position(|selected| selected == &name)
                {
                    state.selected_skill_names.remove(index);
                } else {
                    state.selected_skill_names.push(name);
                }
            }
            None
        }
        crate::tui::UiEvent::Key(KeyEvent {
            code: KeyCode::Char('l'),
            modifiers: KeyModifiers::CONTROL,
            ..
        }) if state.sidebar_ui.session_panel_visible => {
            state
                .selected_filtered_session()
                .map(|session| AppCommand::ToggleSessionLiked {
                    session_id: session.id.clone(),
                })
        }
        crate::tui::UiEvent::Key(KeyEvent {
            code: KeyCode::Char('n'),
            modifiers: KeyModifiers::CONTROL,
            ..
        }) if state.sidebar_ui.session_panel_visible
            && state.run_status != TuiRunStatus::Running =>
        {
            Some(AppCommand::CreateSession { name: None })
        }
        crate::tui::UiEvent::Key(KeyEvent {
            code: KeyCode::Char('a'),
            modifiers: KeyModifiers::CONTROL,
            ..
        }) if state.sidebar_ui.session_panel_visible => {
            state
                .selected_filtered_session()
                .map(|session| AppCommand::ArchiveSession {
                    session_id: session.id.clone(),
                })
        }
        crate::tui::UiEvent::Key(KeyEvent {
            code: KeyCode::Char('g'),
            modifiers: KeyModifiers::CONTROL,
            ..
        }) if state.sidebar_ui.session_panel_visible => {
            state.selected_filtered_session().and_then(|session| {
                state.session_history(&session.id).and_then(|history| {
                    history.runs.first().map(|run| AppCommand::RenameSession {
                        session_id: session.id.clone(),
                        name: generated_session_title(&run.request),
                    })
                })
            })
        }
        crate::tui::UiEvent::Key(KeyEvent {
            code: KeyCode::Char('r'),
            modifiers: KeyModifiers::CONTROL,
            ..
        }) if state.sidebar_ui.session_panel_visible => {
            state.sidebar_ui.session_rename = state
                .selected_filtered_session()
                .map(|session| session.name.clone().unwrap_or_default());
            None
        }
        crate::tui::UiEvent::Key(KeyEvent {
            code: KeyCode::Up, ..
        }) if state.sidebar_ui.session_panel_visible => {
            let session_count = state.filtered_sessions().len();
            if session_count > 0 {
                state.sidebar_ui.selected_session = state
                    .sidebar_ui
                    .selected_session
                    .checked_sub(1)
                    .unwrap_or(session_count - 1);
            }
            None
        }
        crate::tui::UiEvent::Key(KeyEvent {
            code: KeyCode::Down,
            ..
        }) if state.sidebar_ui.session_panel_visible => {
            let session_count = state.filtered_sessions().len();
            if session_count > 0 {
                state.sidebar_ui.selected_session =
                    (state.sidebar_ui.selected_session + 1) % session_count;
            }
            None
        }
        crate::tui::UiEvent::Key(KeyEvent {
            code: KeyCode::Up, ..
        }) if state.sidebar_ui.skill_panel_visible && state.prompt.input.is_empty() => {
            if !state.skills.is_empty() {
                state.sidebar_ui.selected_skill = state
                    .sidebar_ui
                    .selected_skill
                    .checked_sub(1)
                    .unwrap_or(state.skills.len() - 1);
            }
            None
        }
        crate::tui::UiEvent::Key(KeyEvent {
            code: KeyCode::Down,
            ..
        }) if state.sidebar_ui.skill_panel_visible && state.prompt.input.is_empty() => {
            if !state.skills.is_empty() {
                state.sidebar_ui.selected_skill =
                    (state.sidebar_ui.selected_skill + 1) % state.skills.len();
            }
            None
        }
        crate::tui::UiEvent::Key(KeyEvent {
            code: KeyCode::Up, ..
        }) if state.run_status == TuiRunStatus::WaitingForHumanInput
            && state.prompt.input.is_empty() =>
        {
            if let Some(request) = &state.pending_human_input
                && !request.options.is_empty()
            {
                state.overlay_ui.selected_human_option = state
                    .overlay_ui
                    .selected_human_option
                    .checked_sub(1)
                    .unwrap_or(request.options.len() - 1);
            }
            None
        }
        crate::tui::UiEvent::Key(KeyEvent {
            code: KeyCode::Down,
            ..
        }) if state.run_status == TuiRunStatus::WaitingForHumanInput
            && state.prompt.input.is_empty() =>
        {
            if let Some(request) = &state.pending_human_input
                && !request.options.is_empty()
            {
                state.overlay_ui.selected_human_option =
                    (state.overlay_ui.selected_human_option + 1) % request.options.len();
            }
            None
        }
        crate::tui::UiEvent::Key(KeyEvent {
            code: KeyCode::Up, ..
        }) if !state.slash_command_suggestions().is_empty() => {
            let command_count = state.slash_command_suggestions().len();
            state.prompt.selected_slash_command = state
                .prompt
                .selected_slash_command
                .checked_sub(1)
                .unwrap_or(command_count - 1);
            None
        }
        crate::tui::UiEvent::Key(KeyEvent {
            code: KeyCode::Down,
            ..
        }) if !state.slash_command_suggestions().is_empty() => {
            let command_count = state.slash_command_suggestions().len();
            state.prompt.selected_slash_command =
                (state.prompt.selected_slash_command + 1) % command_count;
            None
        }
        crate::tui::UiEvent::Key(KeyEvent {
            code: KeyCode::Up, ..
        }) if !state.inline_skill_suggestions().is_empty() => {
            let count = state.inline_skill_suggestions().len();
            state.prompt.selected_inline_skill = state
                .prompt
                .selected_inline_skill
                .checked_sub(1)
                .unwrap_or(count - 1);
            None
        }
        crate::tui::UiEvent::Key(KeyEvent {
            code: KeyCode::Down,
            ..
        }) if !state.inline_skill_suggestions().is_empty() => {
            let count = state.inline_skill_suggestions().len();
            state.prompt.selected_inline_skill = (state.prompt.selected_inline_skill + 1) % count;
            None
        }
        crate::tui::UiEvent::Key(KeyEvent {
            code: KeyCode::Up, ..
        }) if state.file_reference_suggestion_count() > 0 => {
            let count = state.file_reference_suggestion_count();
            state.prompt.selected_file_reference = (state.prompt.selected_file_reference % count)
                .checked_sub(1)
                .unwrap_or(count - 1);
            None
        }
        crate::tui::UiEvent::Key(KeyEvent {
            code: KeyCode::Down,
            ..
        }) if state.file_reference_suggestion_count() > 0 => {
            let count = state.file_reference_suggestion_count();
            state.prompt.selected_file_reference =
                (state.prompt.selected_file_reference + 1) % count;
            None
        }
        crate::tui::UiEvent::Key(KeyEvent {
            code: KeyCode::Left,
            ..
        }) if state.prompt.input.is_empty() && state.code_change_review.is_some() => {
            let file_count = state
                .code_change_review
                .as_ref()
                .map_or(0, |review| review.proposal.files.len());
            if file_count > 0 {
                state.overlay_ui.selected_changed_file = state
                    .overlay_ui
                    .selected_changed_file
                    .checked_sub(1)
                    .unwrap_or(file_count - 1);
            }
            None
        }
        crate::tui::UiEvent::Key(KeyEvent {
            code: KeyCode::Right,
            ..
        }) if state.prompt.input.is_empty() && state.code_change_review.is_some() => {
            let file_count = state
                .code_change_review
                .as_ref()
                .map_or(0, |review| review.proposal.files.len());
            if file_count > 0 {
                state.overlay_ui.selected_changed_file =
                    (state.overlay_ui.selected_changed_file + 1) % file_count;
            }
            None
        }
        crate::tui::UiEvent::Key(KeyEvent {
            code: KeyCode::Left,
            ..
        }) if state.prompt.input.is_empty() => None,
        crate::tui::UiEvent::Key(KeyEvent {
            code: KeyCode::Right,
            ..
        }) if state.prompt.input.is_empty() => {
            state.sidebar_ui.sidebar_visible = true;
            None
        }
        crate::tui::UiEvent::Key(KeyEvent {
            code: KeyCode::Enter,
            ..
        }) if state.sidebar_ui.session_panel_visible
            && state.sidebar_ui.session_rename.is_some() =>
        {
            let name = state.sidebar_ui.session_rename.take().unwrap_or_default();
            state
                .selected_filtered_session()
                .map(|session| AppCommand::RenameSession {
                    session_id: session.id.clone(),
                    name,
                })
        }
        crate::tui::UiEvent::Key(KeyEvent {
            code: KeyCode::Enter,
            ..
        }) if state.sidebar_ui.session_panel_visible
            && state.run_status != TuiRunStatus::Running =>
        {
            state
                .filtered_sessions()
                .get(state.sidebar_ui.selected_session)
                .map(|session| AppCommand::SwitchSession {
                    session_id: session.id.clone(),
                })
        }
        crate::tui::UiEvent::Key(KeyEvent {
            code: KeyCode::Enter,
            ..
        }) if state.file_reference_suggestion_count() > 0 => {
            state.complete_file_reference(true);
            None
        }
        crate::tui::UiEvent::Key(KeyEvent {
            code: KeyCode::Enter,
            ..
        }) => {
            let execution = execute_selected_slash_command(state);
            if let SlashCommandExecution::Executed(command) = execution {
                return command;
            }
            if state.prompt.input.trim() == "/quit" {
                state.clear_input();
                state.should_quit = true;
                return None;
            }
            if state.prompt.input.trim() == "/clear" {
                state.clear_input();
                state.clear_messages();
                state.conversation_ui.conversation_scroll_from_bottom = 0;
                return None;
            }
            if state.prompt.input.trim() == "/help" {
                state.clear_input();
                state.sidebar_ui.help_visible = true;
                state.sidebar_ui.sidebar_visible = true;
                return None;
            }
            if state.prompt.input.trim() == "/sessions" {
                state.clear_input();
                state.sidebar_ui.session_panel_visible = true;
                state.sidebar_ui.session_search.clear();
                state.sidebar_ui.selected_session = 0;
                return None;
            }
            if state.prompt.input.trim() == "/audit" {
                state.clear_input();
                state.sidebar_ui.audit_panel_visible = true;
                state.sidebar_ui.sidebar_visible = true;
                return None;
            }
            if state.prompt.input.trim() == "/skills" {
                state.clear_input();
                state.sidebar_ui.skill_panel_visible = true;
                state.sidebar_ui.sidebar_visible = true;
                return None;
            }
            if state.prompt.input.trim() == "/logs" {
                state.clear_input();
                state.sidebar_ui.log_panel_visible = true;
                state.sidebar_ui.sidebar_visible = true;
                return None;
            }
            if let Some(query) = state.prompt.input.trim().strip_prefix("/search ")
                && !query.is_empty()
            {
                let query = query.to_lowercase();
                let start = if state.conversation_ui.conversation_search.as_deref()
                    == Some(query.as_str())
                {
                    state
                        .conversation_ui
                        .selected_message
                        .map_or(0, |index| index + 1)
                } else {
                    0
                };
                state.conversation_ui.selected_message = state
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
                state.conversation_ui.conversation_search = Some(query);
                state.conversation_ui.conversation_scroll_from_bottom = u16::MAX;
                if state.conversation_ui.selected_message.is_none() {
                    state.notify("No conversation matches");
                }
                state.clear_input();
                return None;
            }
            if state.prompt.input.trim() == "/review accept" && state.code_change_review.is_some() {
                state.clear_input();
                return Some(AppCommand::AcceptCodeChange);
            }
            if state.prompt.input.trim() == "/review reject" && state.code_change_review.is_some() {
                state.clear_input();
                return Some(AppCommand::RejectCodeChange);
            }
            if let Some(feedback) = state.prompt.input.trim().strip_prefix("/review changes ")
                && !feedback.is_empty()
                && state.code_change_review.is_some()
            {
                let feedback = feedback.to_owned();
                state.clear_input();
                return Some(AppCommand::RequestCodeChanges { feedback });
            }
            if state.run_status != TuiRunStatus::Running
                && let Some(name) = state.prompt.input.trim().strip_prefix("/session new ")
                && !name.is_empty()
            {
                let name = name.to_owned();
                state.clear_input();
                return Some(AppCommand::CreateSession { name: Some(name) });
            }
            if state.run_status != TuiRunStatus::Running
                && let Some(name) = state.prompt.input.trim().strip_prefix("/session rename ")
                && !name.is_empty()
            {
                let name = name.to_owned();
                state.clear_input();
                return Some(AppCommand::RenameActiveSession { name });
            }
            if state.run_status != TuiRunStatus::Running
                && let Some(id) = state.prompt.input.trim().strip_prefix("/session switch ")
                && !id.is_empty()
            {
                let session_id = SessionId::from(id.to_owned());
                state.clear_input();
                return Some(AppCommand::SwitchSession { session_id });
            }
            if state.prompt.input.is_empty()
                && let Some(request) = &state.pending_human_input
                && let Some(option) = request.options.get(state.overlay_ui.selected_human_option)
            {
                state.prompt.input = option.id.clone();
                state.prompt.cursor = state.prompt.input.len();
            }
            if state.prompt.input.trim().is_empty() {
                return None;
            }
            if state.run_status == TuiRunStatus::WaitingForHumanInput
                && let Some(request) = &state.pending_human_input
                && let Err(error) = request.validate(state.prompt.input.trim())
            {
                state.overlay_ui.human_input_error = Some(error);
                return None;
            }
            if state.run_status == TuiRunStatus::Running {
                return None;
            }
            if state.prompt.input.trim_start().starts_with('/') {
                state.notify("Unknown or invalid command");
                state.clear_input();
                return None;
            }
            let inline_skill_names = state
                .prompt
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
            let request = std::mem::take(&mut state.prompt.input);
            state.begin_token_estimate(&request);
            if state.prompt.input_history.last() != Some(&request) {
                state.prompt.input_history.push(request.clone());
            }
            state.prompt.input_history_index = None;
            state.prompt.input_history_draft.clear();
            state.prompt.cursor = 0;
            state.run_status = TuiRunStatus::Running;
            state.pending_human_input = None;
            state.overlay_ui.human_input_error = None;
            state.push_message(Message {
                role: MessageRole::User,
                content: request.clone(),
            });
            Some(AppCommand::StartRun { request })
        }
        crate::tui::UiEvent::Key(KeyEvent {
            code: KeyCode::Char('/'),
            ..
        }) if state.sidebar_ui.session_panel_visible => {
            // Preserve the legacy typed session subcommands: beginning a slash
            // command leaves the picker and returns input to the conversation.
            state.sidebar_ui.session_panel_visible = false;
            state.sidebar_ui.session_search.clear();
            state.sidebar_ui.selected_session = 0;
            state.insert('/');
            None
        }
        crate::tui::UiEvent::Key(KeyEvent {
            code: KeyCode::Left,
            modifiers: KeyModifiers::CONTROL,
            ..
        }) => {
            state.move_cursor_word_left();
            None
        }
        crate::tui::UiEvent::Key(KeyEvent {
            code: KeyCode::Right,
            modifiers: KeyModifiers::CONTROL,
            ..
        }) => {
            state.move_cursor_word_right();
            None
        }
        crate::tui::UiEvent::Key(KeyEvent {
            code: KeyCode::Home,
            ..
        }) if !state.prompt.input.is_empty() => {
            state.move_cursor_to_line_start();
            None
        }
        crate::tui::UiEvent::Key(KeyEvent {
            code: KeyCode::End, ..
        }) if !state.prompt.input.is_empty() => {
            state.move_cursor_to_line_end();
            None
        }
        crate::tui::UiEvent::Key(KeyEvent {
            code: KeyCode::Char('u'),
            modifiers: KeyModifiers::CONTROL,
            ..
        }) => {
            state.delete_current_line();
            None
        }
        crate::tui::UiEvent::Key(KeyEvent {
            code: KeyCode::Char('z'),
            modifiers: KeyModifiers::CONTROL,
            ..
        }) => {
            state.undo_edit();
            None
        }
        crate::tui::UiEvent::Key(KeyEvent {
            code: KeyCode::Up, ..
        }) if state.prompt.input_history_index.is_some() || !state.prompt.input.contains('\n') => {
            state.browse_input_history(true);
            None
        }
        crate::tui::UiEvent::Key(KeyEvent {
            code: KeyCode::Down,
            ..
        }) if state.prompt.input_history_index.is_some() || !state.prompt.input.contains('\n') => {
            state.browse_input_history(false);
            None
        }
        crate::tui::UiEvent::Key(KeyEvent {
            code: KeyCode::Char(character),
            ..
        }) if state.sidebar_ui.session_panel_visible
            && state.sidebar_ui.session_rename.is_some() =>
        {
            if let Some(rename) = &mut state.sidebar_ui.session_rename {
                rename.push(character);
            }
            None
        }
        crate::tui::UiEvent::Key(KeyEvent {
            code: KeyCode::Backspace,
            ..
        }) if state.sidebar_ui.session_panel_visible
            && state.sidebar_ui.session_rename.is_some() =>
        {
            if let Some(rename) = &mut state.sidebar_ui.session_rename {
                rename.pop();
            }
            None
        }
        crate::tui::UiEvent::Key(KeyEvent {
            code: KeyCode::Char(character),
            ..
        }) if state.sidebar_ui.session_panel_visible => {
            state.sidebar_ui.session_search.push(character);
            state.sidebar_ui.selected_session = 0;
            None
        }
        crate::tui::UiEvent::Key(KeyEvent {
            code: KeyCode::Backspace,
            ..
        }) if state.sidebar_ui.session_panel_visible => {
            state.sidebar_ui.session_search.pop();
            state.sidebar_ui.selected_session = 0;
            None
        }
        crate::tui::UiEvent::Key(KeyEvent {
            code: KeyCode::Char(character),
            ..
        }) => {
            state.insert(character);
            None
        }
        crate::tui::UiEvent::Key(KeyEvent {
            code: KeyCode::Left,
            ..
        }) => {
            state.move_cursor_left();
            None
        }
        crate::tui::UiEvent::Key(KeyEvent {
            code: KeyCode::Right,
            ..
        }) => {
            state.move_cursor_right();
            None
        }
        crate::tui::UiEvent::Key(KeyEvent {
            code: KeyCode::Up, ..
        }) => {
            state.move_cursor_up();
            None
        }
        crate::tui::UiEvent::Key(KeyEvent {
            code: KeyCode::Down,
            ..
        }) => {
            state.move_cursor_down();
            None
        }
        crate::tui::UiEvent::Key(KeyEvent {
            code: KeyCode::Backspace,
            ..
        }) => {
            state.delete_before_cursor();
            None
        }
        crate::tui::UiEvent::Key(KeyEvent {
            code: KeyCode::Delete,
            ..
        }) => {
            state.delete_at_cursor();
            None
        }
        crate::tui::UiEvent::Key(KeyEvent {
            code: KeyCode::PageUp,
            ..
        }) => {
            state.conversation_ui.conversation_scroll_from_bottom = state
                .conversation_ui
                .conversation_scroll_from_bottom
                .saturating_add(10);
            None
        }
        crate::tui::UiEvent::Key(KeyEvent {
            code: KeyCode::PageDown,
            ..
        }) => {
            state.conversation_ui.conversation_scroll_from_bottom = state
                .conversation_ui
                .conversation_scroll_from_bottom
                .saturating_sub(10);
            None
        }
        crate::tui::UiEvent::Key(KeyEvent {
            code: KeyCode::Home,
            ..
        }) => {
            state.conversation_ui.conversation_scroll_from_bottom = u16::MAX;
            None
        }
        crate::tui::UiEvent::Key(KeyEvent {
            code: KeyCode::End, ..
        }) => {
            state.conversation_ui.conversation_scroll_from_bottom = 0;
            None
        }
        crate::tui::UiEvent::Key(_) => None,
        crate::tui::UiEvent::Resize(_, _)
        | crate::tui::UiEvent::Scroll(_)
        | crate::tui::UiEvent::Tick => None,
    }
}

fn merge_run_messages(state: &mut AppModel, steps: &[Step], answer: Option<&str>) -> usize {
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
    state.message_render_keys.truncate(message_base);
    for message in run_messages {
        state.push_message(message);
    }
    message_base
}

fn is_quit_shortcut(state: &AppModel, key: KeyEvent) -> bool {
    let character = match state.runtime_info.shortcuts.quit {
        QuitShortcut::CtrlC => 'c',
        QuitShortcut::CtrlQ => 'q',
    };
    key.code == KeyCode::Char(character) && key.modifiers.contains(KeyModifiers::CONTROL)
}

fn is_copy_message_shortcut(state: &AppModel, key: KeyEvent) -> bool {
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

fn handle_escape(state: &mut UiContext<'_>, now: Instant) -> Option<AppCommand> {
    let action = if state.run_status == TuiRunStatus::Running {
        EscapeAction::InterruptRun
    } else {
        EscapeAction::ClearInput
    };
    let confirmed = state
        .prompt
        .pending_escape_action
        .is_some_and(|pending| pending.action == action && pending.expires_at >= now);
    if !confirmed {
        state.prompt.pending_escape_action = Some(PendingEscapeAction {
            action,
            expires_at: now + ESCAPE_CONFIRMATION_WINDOW,
        });
        return None;
    }

    state.prompt.pending_escape_action = None;
    match action {
        EscapeAction::ClearInput => {
            state.clear_input();
            None
        }
        EscapeAction::InterruptRun => Some(AppCommand::CancelRun),
    }
}

fn execute_selected_slash_command(state: &mut UiContext<'_>) -> SlashCommandExecution {
    let Some(definition) = state
        .slash_command_suggestions()
        .get(state.prompt.selected_slash_command)
        .copied()
    else {
        return SlashCommandExecution::NotSelected;
    };
    state.clear_input();
    match definition.command {
        SlashCommand::NewSession if state.run_status == TuiRunStatus::Running => {
            state.push_message(Message {
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
            state.sidebar_ui.session_panel_visible = true;
            state.sidebar_ui.session_search.clear();
            state.sidebar_ui.selected_session = state
                .filtered_sessions()
                .iter()
                .position(|session| Some(session.id.to_string()) == state.session_id)
                .unwrap_or_default();
            SlashCommandExecution::Executed(None)
        }
        SlashCommand::Version => {
            state.push_message(Message {
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
                state.push_message(Message {
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
                state.push_message(Message {
                    role: MessageRole::User,
                    content: request.clone(),
                });
                SlashCommandExecution::Executed(Some(AppCommand::StartRun { request }))
            }),
        SlashCommand::Retry | SlashCommand::Continue => {
            state.push_message(Message {
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

pub(super) fn generated_session_title(request: &str) -> String {
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
    state: &mut UiContext<'_>,
    event: MouseEvent,
    viewport: TuiViewport,
) -> Option<AppCommand> {
    handle_mouse_event_after_command_hit_test(state, event, viewport, None)
}

fn handle_mouse_event_after_command_hit_test(
    state: &mut UiContext<'_>,
    event: MouseEvent,
    viewport: TuiViewport,
    command_toggle: Option<usize>,
) -> Option<AppCommand> {
    if conversation_area_contains(state, viewport, event.column, event.row) {
        match event.kind {
            MouseEventKind::ScrollUp => {
                state.conversation_ui.conversation_scroll_from_bottom = state
                    .conversation_ui
                    .conversation_scroll_from_bottom
                    .saturating_add(state.runtime_info.scroll_lines);
                return None;
            }
            MouseEventKind::ScrollDown => {
                state.conversation_ui.conversation_scroll_from_bottom = state
                    .conversation_ui
                    .conversation_scroll_from_bottom
                    .saturating_sub(state.runtime_info.scroll_lines);
                return None;
            }
            _ => {}
        }
    }
    match event.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            debug_assert!(command_toggle.is_none());
            if state.sidebar_ui.session_panel_visible && event.row >= 3 {
                let index = usize::from(event.row.saturating_sub(3));
                if let Some(session_id) = state
                    .filtered_sessions()
                    .get(index)
                    .map(|session| session.id.clone())
                {
                    state.sidebar_ui.selected_session = index;
                    return Some(AppCommand::SwitchSession { session_id });
                }
            }
            if state.sidebar_ui.skill_panel_visible
                && event.column > state.conversation_panel_width(viewport.width)
                && event.row >= 4
            {
                let index = usize::from(event.row.saturating_sub(4));
                if index < state.skills.len() {
                    state.sidebar_ui.selected_skill = index;
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
                let input_lines = state.prompt.input.lines().count().max(1) as u16;
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
                        state.prompt.selected_slash_command = index;
                        return match execute_selected_slash_command(state) {
                            SlashCommandExecution::Executed(command) => command,
                            SlashCommandExecution::NotSelected => None,
                        };
                    }
                }
            }
            if divider_column(state, viewport) == Some(event.column) {
                state.conversation_ui.text_selection = None;
                state.conversation_ui.text_selection_drag = None;
                if event.row == divider_arrow_row(viewport) {
                    state.sidebar_ui.sidebar_visible = !state.sidebar_ui.sidebar_visible;
                    state.sidebar_ui.divider_dragging = false;
                } else {
                    state.sidebar_ui.divider_dragging = state.sidebar_ui.sidebar_visible;
                }
                return None;
            }
            state.sidebar_ui.divider_dragging = false;
            if let Some((panel, point)) = selection_point_for_event(state, event, viewport) {
                state.conversation_ui.text_selection = None;
                state.conversation_ui.text_selection_drag = Some(TextSelection {
                    panel,
                    anchor: point,
                    focus: point,
                });
            } else {
                state.conversation_ui.text_selection = None;
                state.conversation_ui.text_selection_drag = None;
            }
            None
        }
        MouseEventKind::Drag(MouseButton::Left) => {
            if state.sidebar_ui.divider_dragging {
                resize_panels(state, event.column, viewport);
                return None;
            }
            let selection = state.conversation_ui.text_selection_drag?;
            let point = selection_point_for_panel(state, event, viewport, selection.panel);
            let selection = TextSelection {
                focus: point,
                ..selection
            };
            state.conversation_ui.text_selection_drag = Some(selection);
            state.conversation_ui.text_selection = Some(selection);
            None
        }
        MouseEventKind::Up(MouseButton::Left) => {
            if state.sidebar_ui.divider_dragging {
                resize_panels(state, event.column, viewport);
                state.sidebar_ui.divider_dragging = false;
                return None;
            }
            let selection = state.conversation_ui.text_selection_drag.take()?;
            let point = selection_point_for_panel(state, event, viewport, selection.panel);
            let selection = TextSelection {
                focus: point,
                ..selection
            };
            let content = selected_text(state, selection);
            if content.is_empty() {
                state.conversation_ui.text_selection = None;
                None
            } else {
                state.conversation_ui.text_selection = Some(selection);
                state.notify("Copied to clipboard");
                Some(AppCommand::CopyText { content })
            }
        }
        _ => None,
    }
}

fn conversation_area_contains(
    state: &UiContext<'_>,
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

fn divider_column(state: &UiContext<'_>, viewport: TuiViewport) -> Option<u16> {
    (viewport.width >= 60).then(|| state.conversation_panel_width(viewport.width))
}

fn divider_arrow_row(viewport: TuiViewport) -> u16 {
    viewport.height / 2
}

fn resize_panels(state: &mut UiContext<'_>, column: u16, viewport: TuiViewport) {
    let maximum = viewport
        .width
        .saturating_sub(DIVIDER_WIDTH)
        .saturating_sub(MIN_PANEL_WIDTH);
    let width = column.clamp(MIN_PANEL_WIDTH, maximum);
    state.sidebar_ui.conversation_width_percent = u16::try_from(
        u32::from(width)
            .saturating_mul(100)
            .checked_div(u32::from(viewport.width))
            .unwrap_or_default(),
    )
    .unwrap_or(DEFAULT_CONVERSATION_WIDTH_PERCENT);
}

fn selection_point_for_event(
    state: &UiContext<'_>,
    event: MouseEvent,
    viewport: TuiViewport,
) -> Option<(SelectionPanel, TextSelectionPoint)> {
    if state
        .conversation_ui
        .selection_snapshot
        .contains(event.column, event.row)
    {
        return Some((
            SelectionPanel::Conversation,
            state
                .conversation_ui
                .selection_snapshot
                .point(event.column, event.row),
        ));
    }
    let geometry = panel_geometries(state, viewport)
        .into_iter()
        .filter(|geometry| geometry.panel != SelectionPanel::Conversation)
        .find(|geometry| geometry.contains(event.column, event.row))?;
    Some((
        geometry.panel,
        selection_point_from_geometry(state, event.column, event.row, geometry),
    ))
}

fn selection_point_for_panel(
    state: &UiContext<'_>,
    event: MouseEvent,
    viewport: TuiViewport,
    panel: SelectionPanel,
) -> TextSelectionPoint {
    if panel == SelectionPanel::Conversation {
        return state
            .conversation_ui
            .selection_snapshot
            .point(event.column, event.row);
    }
    let geometry = panel_geometries(state, viewport)
        .into_iter()
        .find(|geometry| geometry.panel == panel)
        .expect("selection panel geometry exists");
    selection_point_from_geometry(state, event.column, event.row, geometry)
}

fn selection_point_from_geometry(
    state: &UiContext<'_>,
    column: u16,
    row: u16,
    geometry: PanelGeometry,
) -> TextSelectionPoint {
    let total_started = Instant::now();
    let lines_started = Instant::now();
    let lines = state.panel_text_lines(geometry.panel);
    let lines_elapsed = lines_started.elapsed();
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
    line = line.min(line_count.saturating_sub(1));
    let column = usize::from(clamped_column.saturating_sub(geometry.x))
        .min(lines.get(line).map_or(0, |line| line.chars().count()));
    tracing::debug!(
        target: "jux::selection_perf",
        panel = ?geometry.panel,
        line_count,
        lines_us = %lines_elapsed.as_micros(),
        total_us = %total_started.elapsed().as_micros(),
        "[DEBUG-selection-perf] selection point calculated"
    );
    TextSelectionPoint { line, column }
}

fn panel_geometries(state: &UiContext<'_>, viewport: TuiViewport) -> Vec<PanelGeometry> {
    let conversation = conversation_geometry(state, viewport);
    if viewport.width < 60 {
        return vec![conversation];
    }
    let left_width = state.conversation_panel_width(viewport.width);
    let mut geometries = vec![conversation];
    if state.sidebar_ui.sidebar_visible {
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

fn conversation_geometry(state: &UiContext<'_>, viewport: TuiViewport) -> PanelGeometry {
    let width = if viewport.width < 60 {
        viewport.width
    } else {
        state.conversation_panel_width(viewport.width)
    };
    content_geometry(SelectionPanel::Conversation, 0, 0, width, viewport.height)
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

fn selected_text(state: &UiContext<'_>, selection: TextSelection) -> String {
    if selection.panel == SelectionPanel::Conversation {
        return state
            .conversation_ui
            .selection_snapshot
            .selected_text(selection);
    }
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

fn conversation_text_lines(state: &UiContext<'_>) -> Vec<String> {
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

fn append_timeline_text_lines(
    state: &UiContext<'_>,
    message_count: usize,
    lines: &mut Vec<String>,
) {
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

fn sidebar_text_lines(state: &UiContext<'_>) -> Vec<String> {
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
    let mut lines = vec![
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
    ];
    if let Some(notice) = state.update_notice() {
        lines.extend([
            String::new(),
            format!("Update: {} available", notice.latest_version),
            notice.recommendation.guidance.clone(),
        ]);
    }
    lines
}
