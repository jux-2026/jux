use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::path::PathBuf;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AppState {
    pub workspace_root: PathBuf,
    pub should_quit: bool,
}

impl AppState {
    #[must_use]
    pub fn new(workspace_root: impl Into<PathBuf>) -> Self {
        Self {
            workspace_root: workspace_root.into(),
            should_quit: false,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AppAction {
    Key(KeyEvent),
}

pub fn update(state: &mut AppState, action: AppAction) {
    match action {
        AppAction::Key(key)
            if key.code == KeyCode::Char('q')
                || (key.code == KeyCode::Char('c')
                    && key.modifiers.contains(KeyModifiers::CONTROL)) =>
        {
            state.should_quit = true
        }
        AppAction::Key(_) => {}
    }
}
