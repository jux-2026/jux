use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use jux_cli::tui::{AppAction, AppState, render_app, update};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;

#[test]
fn tui_quits_when_q_is_pressed() {
    let mut state = AppState::new("/workspace");

    update(
        &mut state,
        AppAction::Key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE)),
    );

    assert!(state.should_quit);
}

#[test]
fn tui_quits_when_ctrl_c_is_pressed() {
    let mut state = AppState::new("/workspace");

    update(
        &mut state,
        AppAction::Key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
    );

    assert!(state.should_quit);
}

#[test]
fn tui_shell_renders_workspace_and_idle_status() {
    let state = AppState::new("/workspace");

    let buffer = render_to_buffer(&state, 80, 24);

    assert_buffer_contains(&buffer, "Jux");
    assert_buffer_contains(&buffer, "What should Jux work on?");
    assert_buffer_contains(&buffer, "Workspace: /workspace");
    assert_buffer_contains(&buffer, "Status: Idle");
    assert_buffer_contains(&buffer, "q quit");
}

fn render_to_buffer(state: &AppState, width: u16, height: u16) -> Buffer {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("test terminal is created");
    terminal
        .draw(|frame| render_app(frame, state))
        .expect("app renders");
    terminal.backend().buffer().clone()
}

fn assert_buffer_contains(buffer: &Buffer, expected: &str) {
    let content = buffer
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect::<String>();
    assert!(
        content.contains(expected),
        "buffer does not contain {expected:?}:\n{content}"
    );
}
