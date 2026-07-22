use self::components::conversation::{
    render_conversation_history, render_conversation_status, render_prompt_input,
};
use self::components::divider;
use self::components::sessions;
use self::components::sidebar::{audit_panel, help_panel, log_panel, run_panel, skill_panel};
use self::layout::WorkspaceLayout;
use self::state::{
    ConversationUiState, EscapeAction, FileReferenceMatches, OverlayUiState, PromptState,
    RootUiState, SLASH_COMMANDS, SidebarUiState, SlashCommand, SlashCommandDefinition,
};
use super::app::{AuditFilter, UiStateRefs};
use super::{AppCommand, AppModel, AppMsg, FocusedPanel, TuiRunStatus, TuiViewport};
use crossterm::event::{KeyCode, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind};
use jux_core::{HumanInputKind, SkillDefinition};
use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Style};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Widget, Wrap};
use std::ops::Deref;

pub(crate) struct RenderState<'a> {
    model: &'a AppModel,
    prompt: &'a PromptState,
    conversation: &'a ConversationUiState,
    sidebar: &'a SidebarUiState,
    overlay: &'a OverlayUiState,
    root: &'a RootUiState,
}

impl<'a> RenderState<'a> {
    #[cfg(test)]
    pub(crate) fn for_test(model: &'a AppModel) -> Self {
        use std::sync::LazyLock;
        static PROMPT: LazyLock<PromptState> = LazyLock::new(PromptState::default);
        static CONVERSATION: LazyLock<ConversationUiState> =
            LazyLock::new(ConversationUiState::default);
        static SIDEBAR: LazyLock<SidebarUiState> = LazyLock::new(SidebarUiState::default);
        static OVERLAY: LazyLock<OverlayUiState> = LazyLock::new(OverlayUiState::default);
        static ROOT: LazyLock<RootUiState> = LazyLock::new(RootUiState::default);
        Self::from_parts(model, &PROMPT, &CONVERSATION, &SIDEBAR, &OVERLAY, &ROOT)
    }

    fn from_parts(
        model: &'a AppModel,
        prompt: &'a PromptState,
        conversation: &'a ConversationUiState,
        sidebar: &'a SidebarUiState,
        overlay: &'a OverlayUiState,
        root: &'a RootUiState,
    ) -> Self {
        Self {
            model,
            prompt,
            conversation,
            sidebar,
            overlay,
            root,
        }
    }

    pub(crate) fn input_text(&self) -> &str {
        &self.prompt.input
    }

    pub(crate) fn input_cursor_line_column(&self) -> (u16, u16) {
        let before = &self.prompt.input[..self.prompt.cursor];
        let line = before.bytes().filter(|byte| *byte == b'\n').count();
        let current = before.rsplit_once('\n').map_or(before, |(_, line)| line);
        (
            u16::try_from(line).unwrap_or(u16::MAX),
            u16::try_from(unicode_width::UnicodeWidthStr::width(current)).unwrap_or(u16::MAX),
        )
    }

    pub(crate) fn selected_message(&self) -> Option<usize> {
        self.conversation.selected_message
    }

    pub(crate) fn conversation_search(&self) -> Option<&str> {
        self.conversation.conversation_search.as_deref()
    }

    pub(crate) fn conversation_scroll_from_bottom(&self) -> u16 {
        self.conversation.conversation_scroll_from_bottom
    }

    pub(crate) fn help_visible(&self) -> bool {
        self.sidebar.help_visible
    }

    pub(crate) fn selected_session(&self) -> usize {
        self.sidebar.selected_session
    }

    pub(crate) fn session_search(&self) -> &str {
        &self.sidebar.session_search
    }

    pub(crate) fn session_rename(&self) -> Option<&str> {
        self.sidebar.session_rename.as_deref()
    }

    pub(crate) fn filtered_sessions(&self) -> Vec<&jux_core::Session> {
        let query = self.sidebar.session_search.to_lowercase();
        let mut sessions = self
            .model
            .sessions()
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

    pub(crate) fn session_panel_visible(&self) -> bool {
        self.sidebar.session_panel_visible
    }

    pub(crate) fn selected_changed_file(&self) -> usize {
        self.overlay.selected_changed_file
    }

    pub(crate) fn audit_panel_visible(&self) -> bool {
        self.sidebar.audit_panel_visible
    }

    pub(crate) fn audit_filter(&self) -> AuditFilter {
        self.sidebar.audit_filter
    }

    pub(crate) fn selected_audit_item(&self) -> usize {
        self.sidebar.selected_audit_item
    }

    pub(crate) fn filtered_audit_items(&self) -> Vec<&super::AuditItem> {
        self.model
            .audit_items()
            .iter()
            .filter(|item| match self.sidebar.audit_filter {
                AuditFilter::All => true,
                AuditFilter::Files => item.title.starts_with("File"),
                AuditFilter::Commands => {
                    item.title.starts_with("Tool") || item.title.contains("command")
                }
                AuditFilter::Policy => item.title.starts_with("Policy"),
            })
            .collect()
    }

    pub(crate) fn selected_skill(&self) -> usize {
        self.sidebar.selected_skill
    }

    pub(crate) fn skill_panel_visible(&self) -> bool {
        self.sidebar.skill_panel_visible
    }

    pub(crate) fn log_panel_visible(&self) -> bool {
        self.sidebar.log_panel_visible
    }

    pub(crate) fn text_selection(&self) -> Option<super::TextSelection> {
        self.conversation.text_selection
    }

    pub(crate) fn sidebar_visible(&self) -> bool {
        self.sidebar.sidebar_visible
    }

    pub(crate) fn selected_slash_command(&self) -> usize {
        self.prompt.selected_slash_command
    }

    pub(crate) fn slash_command_suggestions(&self) -> Vec<SlashCommandDefinition> {
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
                SlashCommand::Retry => self.run_status() == TuiRunStatus::Failed,
                SlashCommand::Continue => self.run_status() == TuiRunStatus::Canceled,
                _ => true,
            })
            .filter(|definition| definition.name.trim_start_matches('/').starts_with(query))
            .collect()
    }

    pub(crate) fn inline_skill_suggestions(&self) -> Vec<&SkillDefinition> {
        let token_start = self.prompt.input[..self.prompt.cursor]
            .rfind(char::is_whitespace)
            .map_or(0, |index| index + 1);
        let token = &self.prompt.input[token_start..self.prompt.cursor];
        let Some(query) = token.strip_prefix('$') else {
            return Vec::new();
        };
        let query = query.to_lowercase();
        self.skills()
            .iter()
            .filter(|skill| skill.name.to_lowercase().contains(&query))
            .collect()
    }

    pub(crate) fn selected_inline_skill(&self) -> usize {
        self.prompt.selected_inline_skill
    }

    pub(crate) fn file_reference_suggestion_count(&self) -> usize {
        match &self.prompt.file_reference_cache.matches {
            FileReferenceMatches::Disabled => 0,
            FileReferenceMatches::AllFiles => self.indexed_files().len(),
            FileReferenceMatches::Filtered(matches) => matches.len(),
        }
    }

    pub(crate) fn file_reference_suggestion(&self, index: usize) -> Option<&str> {
        let file_index = match &self.prompt.file_reference_cache.matches {
            FileReferenceMatches::Disabled => return None,
            FileReferenceMatches::AllFiles => index,
            FileReferenceMatches::Filtered(matches) => *matches.get(index)?,
        };
        self.indexed_files().get(file_index).map(String::as_str)
    }

    pub(crate) fn selected_file_reference(&self) -> usize {
        self.prompt.selected_file_reference
    }

    pub(crate) fn notification(&self) -> Option<&str> {
        self.root
            .notification
            .as_ref()
            .filter(|(_, expires_at)| *expires_at > std::time::Instant::now())
            .map(|(message, _)| message.as_str())
    }

    pub(crate) fn escape_confirmation_hint(&self) -> Option<&'static str> {
        self.prompt
            .pending_escape_action
            .filter(|pending| pending.expires_at >= std::time::Instant::now())
            .map(|pending| match pending.action {
                EscapeAction::ClearInput => "Press Esc again to clear the input",
                EscapeAction::InterruptRun => "Press Esc again to interrupt the current run",
            })
    }

    pub(crate) fn completed_file_reference_ranges(&self) -> Vec<(usize, usize)> {
        reference_ranges(&self.prompt.input)
            .into_iter()
            .filter(|(start, end)| {
                let reference = &self.prompt.input[*start..*end];
                let path = reference
                    .strip_prefix("@{")
                    .and_then(|path| path.strip_suffix('}'))
                    .or_else(|| reference.strip_prefix('@'));
                path.is_some_and(|path| self.indexed_files().iter().any(|item| item == path))
            })
            .collect()
    }

    pub(crate) fn conversation_panel_width(&self, viewport_width: u16) -> u16 {
        if viewport_width < 60 {
            return viewport_width;
        }
        if !self.sidebar.sidebar_visible {
            return viewport_width.saturating_sub(1);
        }
        let maximum = viewport_width.saturating_sub(1).saturating_sub(20);
        viewport_width
            .saturating_mul(self.sidebar.conversation_width_percent)
            .checked_div(100)
            .unwrap_or_default()
            .clamp(20, maximum)
    }

    pub(crate) fn selected_human_option(&self) -> usize {
        self.overlay.selected_human_option
    }

    pub(crate) fn human_input_error(&self) -> Option<&str> {
        self.overlay.human_input_error.as_deref()
    }
}

fn reference_ranges(input: &str) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    let mut offset = 0;
    while let Some(relative) = input[offset..].find('@') {
        let start = offset + relative;
        let remainder = &input[start..];
        let end = if let Some(braced) = remainder.strip_prefix("@{") {
            braced.find('}').map(|index| start + index + 3)
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

fn conversation_panel_width(state: &SidebarUiState, viewport_width: u16) -> u16 {
    if viewport_width < 60 {
        return viewport_width;
    }
    if !state.sidebar_visible {
        return viewport_width.saturating_sub(1);
    }
    let maximum = viewport_width.saturating_sub(1).saturating_sub(20);
    viewport_width
        .saturating_mul(state.conversation_width_percent)
        .checked_div(100)
        .unwrap_or_default()
        .clamp(20, maximum)
}

impl Deref for RenderState<'_> {
    type Target = AppModel;

    fn deref(&self) -> &Self::Target {
        self.model
    }
}

mod component;
mod components;
mod event;
mod focus;
mod layout;
pub(crate) mod state;
mod text;
mod theme;
mod virtual_list;

pub use self::component::{Component, EventResult};
pub use self::event::UiEvent;
pub use self::focus::{FocusId, FocusManager};
pub use self::virtual_list::{VirtualItemRenderer, VirtualListState, VisibleItem};

pub(crate) use self::components::conversation::ConversationPanel;

#[derive(Default)]
pub struct AppComponent {
    conversation: ConversationPanel,
    prompt_input: PromptInputComponent,
    status_bar: StatusBarComponent,
    sidebar: SidebarComponent,
    overlay: OverlayComponent,
    focus: FocusManager,
    root_state: RootUiState,
    last_area: Rect,
}

#[derive(Default)]
struct PromptInputComponent {
    state: PromptState,
    focused: bool,
    cursor: Option<Position>,
}

struct StatusBarComponent {
    scroll_position: &'static str,
}

impl Default for StatusBarComponent {
    fn default() -> Self {
        Self {
            scroll_position: "Bottom",
        }
    }
}

#[derive(Default)]
struct SidebarComponent {
    state: SidebarUiState,
}

#[derive(Default)]
struct OverlayComponent {
    state: OverlayUiState,
    visible: bool,
}

impl AppComponent {
    pub(crate) fn update_model(
        &mut self,
        model: &mut AppModel,
        message: AppMsg,
    ) -> Option<AppCommand> {
        super::app::update_with_ui(model, message, self.state_refs())
    }

    pub(crate) fn update_ui(
        &mut self,
        model: &mut AppModel,
        event: UiEvent,
        viewport: TuiViewport,
    ) -> Option<AppCommand> {
        super::app::update_ui(model, event, viewport, self.state_refs())
    }

    pub(crate) fn restore_session_ui(&mut self, input_history: Vec<String>) {
        self.prompt_input.state.input_history = input_history;
        self.prompt_input.state.input_history_index = None;
        self.prompt_input.state.input_history_draft.clear();
        self.conversation.ui_state.selected_timeline = None;
    }

    pub(crate) fn finish_session_command(&mut self, switched: bool) {
        self.sidebar.state.session_panel_visible = !switched;
        if switched {
            self.sidebar.state.session_search.clear();
            self.sidebar.state.session_rename = None;
            self.sidebar.state.selected_session = 0;
            self.root_state.notification = Some((
                "Session switched".to_owned(),
                std::time::Instant::now() + std::time::Duration::from_secs(2),
            ));
        }
    }

    pub(crate) fn remember_submitted_input(&mut self, request: &str) {
        self.prompt_input
            .state
            .input_history
            .push(request.to_owned());
    }

    fn state_refs(&mut self) -> UiStateRefs<'_> {
        UiStateRefs {
            prompt: &mut self.prompt_input.state,
            conversation: &mut self.conversation.ui_state,
            sidebar: &mut self.sidebar.state,
            overlay: &mut self.overlay.state,
            root: &mut self.root_state,
        }
    }

    pub(crate) fn prompt_text(&self) -> &str {
        self.prompt_input.state.input_text()
    }

    pub(crate) fn conversation_scroll_from_bottom(&self) -> u16 {
        self.conversation.scroll_from_bottom()
    }

    pub(crate) fn focused_panel(&self) -> FocusedPanel {
        match self.focus.current() {
            Some(FocusId::Sidebar) => FocusedPanel::Sidebar,
            _ => FocusedPanel::Conversation,
        }
    }

    pub(crate) fn text_selection(&self) -> Option<super::TextSelection> {
        self.conversation.ui_state.text_selection
    }

    pub(crate) fn sidebar_visible(&self) -> bool {
        self.sidebar.state.sidebar_visible
    }

    pub fn render(&mut self, frame: &mut Frame<'_>, state: &AppModel) {
        let area = frame.area();
        self.last_area = area;
        let cursor = render_workspace(frame.buffer_mut(), state, self, area);
        if let Some(cursor) = cursor {
            frame.set_cursor_position(cursor);
        }
    }

    pub fn handle_event(
        &mut self,
        state: &AppModel,
        event: UiEvent,
        viewport: TuiViewport,
    ) -> EventResult<UiEvent> {
        self.sync_focus(state);
        match event {
            UiEvent::Key(key) if key.kind == KeyEventKind::Release => EventResult::ignored(),
            UiEvent::Key(key)
                if key.code == KeyCode::Left
                    && self.prompt_input.state.input.is_empty()
                    && !self.focus.modal_active() =>
            {
                self.focus.focus(FocusId::PromptInput);
                EventResult::consumed(None)
            }
            UiEvent::Key(key)
                if key.code == KeyCode::Right
                    && self.prompt_input.state.input.is_empty()
                    && self.sidebar.state.sidebar_visible
                    && !self.focus.modal_active() =>
            {
                self.focus.focus(FocusId::Sidebar);
                EventResult::consumed(None)
            }
            UiEvent::Key(key) if key.code == KeyCode::PageUp => {
                self.conversation.scroll_by(10);
                EventResult::consumed(None)
            }
            UiEvent::Key(key) if key.code == KeyCode::PageDown => {
                self.conversation.scroll_by(-10);
                EventResult::consumed(None)
            }
            UiEvent::Key(key)
                if key.code == KeyCode::Home && self.prompt_input.state.input.is_empty() =>
            {
                self.conversation.scroll_to_top();
                EventResult::consumed(None)
            }
            UiEvent::Key(key)
                if key.code == KeyCode::End && self.prompt_input.state.input.is_empty() =>
            {
                self.conversation.scroll_to_bottom();
                EventResult::consumed(None)
            }
            UiEvent::Key(key)
                if key.code == KeyCode::BackTab
                    || (key.code == KeyCode::Tab
                        && key.modifiers.contains(KeyModifiers::SHIFT)) =>
            {
                self.focus.focus_next(true);
                EventResult::consumed(None)
            }
            UiEvent::Key(key)
                if key.code == KeyCode::Tab && self.prompt_input.state.input.is_empty() =>
            {
                self.focus.focus_next(false);
                EventResult::consumed(None)
            }
            UiEvent::Mouse(event) => {
                if matches!(event.kind, MouseEventKind::Down(MouseButton::Left))
                    && !self.focus.modal_active()
                {
                    let target = if event.column
                        < conversation_panel_width(&self.sidebar.state, viewport.width)
                    {
                        FocusId::PromptInput
                    } else {
                        FocusId::Sidebar
                    };
                    self.focus.focus(target);
                }
                if matches!(
                    event.kind,
                    MouseEventKind::ScrollUp | MouseEventKind::ScrollDown
                ) && event.column < conversation_panel_width(&self.sidebar.state, viewport.width)
                    && event.row < viewport.height
                {
                    let delta = i32::from(state.scroll_lines());
                    self.conversation.scroll_by(
                        if matches!(event.kind, MouseEventKind::ScrollUp) {
                            delta
                        } else {
                            -delta
                        },
                    );
                    return EventResult::consumed(None);
                }
                let command_toggle = matches!(event.kind, MouseEventKind::Down(MouseButton::Left))
                    .then(|| {
                        let prompt = self.prompt_input.state.clone();
                        let conversation = self.conversation.ui_state.clone();
                        let sidebar = self.sidebar.state.clone();
                        let overlay = self.overlay.state.clone();
                        let root = self.root_state.clone();
                        let view = RenderState::from_parts(
                            state,
                            &prompt,
                            &conversation,
                            &sidebar,
                            &overlay,
                            &root,
                        );
                        self.conversation.command_toggle_at(
                            &view,
                            viewport,
                            event.column,
                            event.row,
                        )
                    })
                    .flatten();
                if let Some(index) = command_toggle
                    && let Some(item) = state.timeline().get(index)
                {
                    self.conversation.toggle_timeline_item(&item.id);
                    return EventResult::consumed(None);
                }
                EventResult::consumed(Some(UiEvent::Mouse(event)))
            }
            UiEvent::Scroll(delta) => {
                self.conversation.scroll_by(delta);
                EventResult::consumed(None)
            }
            UiEvent::Resize(_, _) => EventResult::consumed(None),
            UiEvent::Tick => EventResult::ignored(),
            event @ (UiEvent::Key(_) | UiEvent::Paste(_)) => {
                self.dispatch_focused_event(state, &event)
            }
        }
    }

    #[doc(hidden)]
    pub fn conversation_scroll_position(&self) -> &'static str {
        self.conversation.scroll_position_label()
    }

    fn sync_focus(&mut self, state: &AppModel) {
        let order = if self.sidebar.state.sidebar_visible {
            vec![FocusId::PromptInput, FocusId::Sidebar]
        } else {
            vec![FocusId::PromptInput]
        };
        self.focus.set_order(order);
        let overlay_visible =
            state.pending_human_input().is_some() || state.code_change_review().is_some();
        if overlay_visible && !self.focus.modal_active() {
            self.focus.open_modal(FocusId::Overlay);
        } else if !overlay_visible && self.focus.modal_active() {
            self.focus.close_modal();
        }
    }

    fn dispatch_focused_event(
        &mut self,
        state: &AppModel,
        event: &UiEvent,
    ) -> EventResult<UiEvent> {
        match self.focus.current() {
            Some(FocusId::PromptInput | FocusId::Conversation) => {
                Component::handle_event(&mut self.prompt_input, state, event)
            }
            Some(FocusId::Sidebar) => Component::handle_event(&mut self.sidebar, state, event),
            Some(FocusId::Overlay) => Component::handle_event(&mut self.overlay, state, event),
            None => EventResult::ignored(),
        }
    }
}

impl Component<UiEvent> for AppComponent {
    type Model = AppModel;

    fn render(&mut self, model: &Self::Model, area: Rect, buffer: &mut Buffer) {
        self.last_area = area;
        let _ = render_workspace(buffer, model, self, area);
    }

    fn handle_event(&mut self, model: &Self::Model, event: &UiEvent) -> EventResult<UiEvent> {
        AppComponent::handle_event(
            self,
            model,
            event.clone(),
            TuiViewport {
                width: self.last_area.width,
                height: self.last_area.height,
            },
        )
    }
}

impl Component<UiEvent> for ConversationPanel {
    type Model = AppModel;

    fn render(&mut self, model: &Self::Model, area: Rect, buffer: &mut Buffer) {
        let prompt = PromptState::default();
        let conversation = self.ui_state.clone();
        let sidebar = SidebarUiState::default();
        let overlay = OverlayUiState::default();
        let root = RootUiState::default();
        let state =
            RenderState::from_parts(model, &prompt, &conversation, &sidebar, &overlay, &root);
        render_conversation_history(self, buffer, &state, area);
    }
}

impl Component<UiEvent> for PromptInputComponent {
    type Model = AppModel;

    fn render(&mut self, model: &Self::Model, area: Rect, buffer: &mut Buffer) {
        let conversation = ConversationUiState::default();
        let sidebar = SidebarUiState::default();
        let overlay = OverlayUiState::default();
        let root = RootUiState::default();
        let state =
            RenderState::from_parts(model, &self.state, &conversation, &sidebar, &overlay, &root);
        self.cursor = render_prompt_input(buffer, &state, area, self.focused);
    }

    fn focusable(&self) -> bool {
        true
    }

    fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
    }

    fn handle_event(&mut self, _model: &Self::Model, event: &UiEvent) -> EventResult<UiEvent> {
        match event {
            UiEvent::Key(key) => EventResult::consumed(Some(UiEvent::Key(*key))),
            UiEvent::Paste(content) => EventResult::consumed(Some(UiEvent::Paste(content.clone()))),
            _ => EventResult::ignored(),
        }
    }
}

impl Component<UiEvent> for StatusBarComponent {
    type Model = AppModel;

    fn render(&mut self, model: &Self::Model, area: Rect, buffer: &mut Buffer) {
        let prompt = PromptState::default();
        let conversation = ConversationUiState::default();
        let sidebar = SidebarUiState::default();
        let overlay = OverlayUiState::default();
        let root = RootUiState::default();
        let state =
            RenderState::from_parts(model, &prompt, &conversation, &sidebar, &overlay, &root);
        render_conversation_status(buffer, &state, area, self.scroll_position);
    }
}

impl Component<UiEvent> for SidebarComponent {
    type Model = AppModel;

    fn render(&mut self, model: &Self::Model, area: Rect, buffer: &mut Buffer) {
        let prompt = PromptState::default();
        let conversation = ConversationUiState::default();
        let overlay = OverlayUiState::default();
        let root = RootUiState::default();
        let state =
            RenderState::from_parts(model, &prompt, &conversation, &self.state, &overlay, &root);
        if state.help_visible() {
            help_panel(&state).render(area, buffer);
        } else if state.log_panel_visible() {
            log_panel(&state).render(area, buffer);
        } else if state.skill_panel_visible() {
            skill_panel(&state).render(area, buffer);
        } else if state.audit_panel_visible() {
            audit_panel(&state).render(area, buffer);
        } else {
            run_panel(&state, area).render(area, buffer);
        }
    }

    fn focusable(&self) -> bool {
        true
    }

    fn handle_event(&mut self, _model: &Self::Model, event: &UiEvent) -> EventResult<UiEvent> {
        match event {
            UiEvent::Key(key) => EventResult::consumed(Some(UiEvent::Key(*key))),
            _ => EventResult::ignored(),
        }
    }
}

impl Component<UiEvent> for OverlayComponent {
    type Model = AppModel;

    fn render(&mut self, model: &Self::Model, area: Rect, buffer: &mut Buffer) {
        let prompt = PromptState::default();
        let conversation = ConversationUiState::default();
        let sidebar = SidebarUiState::default();
        let root = RootUiState::default();
        let state =
            RenderState::from_parts(model, &prompt, &conversation, &sidebar, &self.state, &root);
        self.visible = render_confirmation_overlay(buffer, &state, area);
    }

    fn handle_event(&mut self, _model: &Self::Model, event: &UiEvent) -> EventResult<UiEvent> {
        match event {
            UiEvent::Key(key) => EventResult::consumed(Some(UiEvent::Key(*key))),
            _ => EventResult::consumed(None),
        }
    }
}

fn render_workspace(
    buffer: &mut Buffer,
    state: &AppModel,
    app: &mut AppComponent,
    area: Rect,
) -> Option<Position> {
    if area.width < 20 || area.height < 6 {
        Clear.render(area, buffer);
        Paragraph::new("Terminal too small\nResize to at least 40x10")
            .style(Style::default().fg(Color::Yellow))
            .block(Block::default().borders(Borders::ALL).title("Jux"))
            .wrap(Wrap { trim: true })
            .render(area, buffer);
        return None;
    }
    // Business data stays borrowed from AppModel. Only the conversation's
    // small interaction state is snapshotted because its renderer mutates
    // measurement caches while reading that state.
    let conversation_focused = app.focus.current() != Some(FocusId::Sidebar);
    app.prompt_input.focused = conversation_focused;
    let conversation_state = app.conversation.ui_state.clone();
    let prompt_state = &app.prompt_input.state;
    let sidebar_state = &app.sidebar.state;
    let overlay_state = &app.overlay.state;
    let root_state = &app.root_state;
    let view = RenderState::from_parts(
        state,
        prompt_state,
        &conversation_state,
        sidebar_state,
        overlay_state,
        root_state,
    );
    let layout = WorkspaceLayout::calculate(&view, area);
    let mut cursor = if view.session_panel_visible() {
        Some(sessions::render(buffer, &view, layout.conversation))
    } else {
        render_conversation_history(&mut app.conversation, buffer, &view, layout.conversation);
        app.status_bar.scroll_position = app.conversation.scroll_position_label();
        app.prompt_input.cursor =
            render_prompt_input(buffer, &view, layout.conversation, app.prompt_input.focused);
        render_conversation_status(
            buffer,
            &view,
            layout.conversation,
            app.status_bar.scroll_position,
        );
        app.prompt_input.cursor
    };
    if let Some(divider) = layout.divider {
        divider::render(buffer, divider, view.sidebar_visible(), view.theme());
    }
    if let Some(sidebar_area) = layout.sidebar {
        if view.help_visible() {
            help_panel(&view).render(sidebar_area, buffer);
        } else if view.log_panel_visible() {
            log_panel(&view).render(sidebar_area, buffer);
        } else if view.skill_panel_visible() {
            skill_panel(&view).render(sidebar_area, buffer);
        } else if view.audit_panel_visible() {
            audit_panel(&view).render(sidebar_area, buffer);
        } else {
            run_panel(&view, sidebar_area).render(sidebar_area, buffer);
        }
    }
    app.overlay.visible = render_confirmation_overlay(buffer, &view, area);
    if app.overlay.visible {
        cursor = None;
    }
    cursor
}

fn render_confirmation_overlay(buffer: &mut Buffer, state: &RenderState<'_>, area: Rect) -> bool {
    let (title, body) = if let Some(request) = state
        .pending_human_input()
        .filter(|request| request.kind == HumanInputKind::Confirmation)
    {
        (
            "Permission confirmation",
            format!(
                "{}\n\n{}\n\nUse Up/Down and Enter to confirm or reject.",
                request.prompt,
                request.reason.as_deref().unwrap_or("")
            ),
        )
    } else if let Some(review) = state.code_change_review() {
        (
            "File change confirmation",
            format!(
                "{}\n{} file(s), policy {:?}\n\n/review accept | /review reject | /review changes <feedback>",
                review.proposal.plan.summary,
                review.proposal.files.len(),
                review.proposal.policy
            ),
        )
    } else {
        return false;
    };
    let width = area.width.saturating_sub(8).min(72);
    let height = area.height.saturating_sub(4).min(11);
    let popup = Rect::new(
        area.x.saturating_add(area.width.saturating_sub(width) / 2),
        area.y
            .saturating_add(area.height.saturating_sub(height) / 2),
        width,
        height,
    );
    Clear.render(popup, buffer);
    Paragraph::new(body)
        .block(Block::default().borders(Borders::ALL).title(title))
        .style(Style::default().fg(Color::Yellow))
        .wrap(Wrap { trim: true })
        .render(popup, buffer);
    true
}
