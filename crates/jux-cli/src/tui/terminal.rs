use super::{AppAction, AppState, render_app, update};
use anyhow::Result;
use crossterm::event::{self, Event};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use std::io;
use std::path::PathBuf;

pub fn run_tui(workspace_root: PathBuf) -> Result<()> {
    let mut terminal = setup_terminal()?;
    let mut state = AppState::new(workspace_root);
    let run_result = run_app_loop(&mut terminal, &mut state);
    restore_terminal(&mut terminal)?;
    run_result
}

fn setup_terminal() -> Result<Terminal<CrosstermBackend<io::Stdout>>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    Ok(Terminal::new(backend)?)
}

fn run_app_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    state: &mut AppState,
) -> Result<()> {
    while !state.should_quit {
        terminal.draw(|frame| render_app(frame, state))?;
        if let Event::Key(key) = event::read()? {
            update(state, AppAction::Key(key));
        }
    }
    Ok(())
}

fn restore_terminal(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> Result<()> {
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    Ok(())
}
