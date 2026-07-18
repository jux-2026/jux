use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use super::app::restored_input_history;
use super::ui::{AppComponent, Component, UiEvent};
use super::{
    AppCommand, AppModel, AppMsg, FocusedPanel, TextSelection, TuiViewport,
    execute_code_change_command, execute_session_command, load_active_session_history,
    materialize_pending_new_session,
};
use jux_core::{CodeChangeError, SqliteWorkspaceStore, StoreError};

#[derive(Debug, Eq, PartialEq)]
pub struct DispatchResult {
    pub command: Option<AppCommand>,
    pub consumed: bool,
}

pub struct TuiApp {
    model: AppModel,
    root: AppComponent,
}

impl TuiApp {
    pub fn new(model: AppModel) -> Self {
        Self {
            model,
            root: AppComponent::default(),
        }
    }

    pub fn model(&self) -> &AppModel {
        &self.model
    }

    pub fn model_mut(&mut self) -> &mut AppModel {
        &mut self.model
    }

    pub fn input_text(&self) -> &str {
        self.root.prompt_text()
    }

    #[doc(hidden)]
    pub fn conversation_scroll_from_bottom(&self) -> u16 {
        self.root.conversation_scroll_from_bottom()
    }

    #[doc(hidden)]
    pub fn focused_panel(&self) -> FocusedPanel {
        self.root.focused_panel()
    }

    #[doc(hidden)]
    pub fn text_selection(&self) -> Option<TextSelection> {
        self.root.text_selection()
    }

    #[doc(hidden)]
    pub fn sidebar_visible(&self) -> bool {
        self.root.sidebar_visible()
    }

    pub fn render(&mut self, frame: &mut Frame<'_>) {
        self.root.render(frame, &self.model);
    }

    #[doc(hidden)]
    pub fn render_buffer(&mut self, area: Rect, buffer: &mut Buffer) {
        Component::render(&mut self.root, &self.model, area, buffer);
    }

    #[doc(hidden)]
    pub fn conversation_scroll_position(&self) -> &'static str {
        self.root.conversation_scroll_position()
    }

    pub fn update(&mut self, message: AppMsg) -> Option<AppCommand> {
        Self::update_parts(&mut self.model, &mut self.root, message)
    }

    pub fn dispatch(&mut self, event: UiEvent, viewport: TuiViewport) -> DispatchResult {
        Self::dispatch_parts(&mut self.model, &mut self.root, event, viewport)
    }

    pub fn load_active_session_history(
        &mut self,
        store: &SqliteWorkspaceStore,
    ) -> Result<(), StoreError> {
        load_active_session_history(&mut self.model, store)?;
        let history = restored_input_history(&self.model);
        self.root.restore_session_ui(history);
        Ok(())
    }

    pub(crate) fn restore_session_ui(&mut self) {
        let history = restored_input_history(&self.model);
        self.root.restore_session_ui(history);
    }

    pub fn execute_session_command(
        &mut self,
        store: &SqliteWorkspaceStore,
        command: &AppCommand,
    ) -> Result<bool, StoreError> {
        let handled = execute_session_command(&mut self.model, store, command)?;
        if handled {
            let history = restored_input_history(&self.model);
            let switched = matches!(command, AppCommand::SwitchSession { .. });
            let root = &mut self.root;
            root.restore_session_ui(history);
            root.finish_session_command(switched);
        }
        Ok(handled)
    }

    pub fn materialize_pending_new_session(
        &mut self,
        store: &SqliteWorkspaceStore,
        request: &str,
    ) -> Result<(), StoreError> {
        let pending = self.model.pending_new_session();
        materialize_pending_new_session(&mut self.model, store, request)?;
        if pending {
            let history = restored_input_history(&self.model);
            let root = &mut self.root;
            root.restore_session_ui(history);
            root.remember_submitted_input(request);
        }
        Ok(())
    }

    pub fn execute_code_change_command(
        &mut self,
        command: &AppCommand,
    ) -> Result<bool, CodeChangeError> {
        execute_code_change_command(&mut self.model, command)
    }

    pub(crate) fn update_parts(
        model: &mut AppModel,
        root: &mut AppComponent,
        message: AppMsg,
    ) -> Option<AppCommand> {
        root.update_model(model, message)
    }

    pub(crate) fn dispatch_parts(
        model: &mut AppModel,
        root: &mut AppComponent,
        event: UiEvent,
        viewport: TuiViewport,
    ) -> DispatchResult {
        let event_result = root.handle_event(model, event, viewport);
        let command = event_result
            .message
            .and_then(|message| root.update_ui(model, message, viewport));
        DispatchResult {
            command,
            consumed: event_result.consumed,
        }
    }
}
