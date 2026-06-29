use super::{
    AppAction, AppCommand, AppState, BackgroundRun, RunHandler, RunResponse, TuiRunRequest,
    TuiRuntimeInfo, execute_code_change_command, execute_session_command,
    load_active_session_history, render_app, update,
};
use anyhow::Result;
use crossterm::event::{self, Event};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use jux_core::{SkillCatalog, SqliteWorkspaceStore, StoreError};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

pub fn run_tui(
    workspace_root: PathBuf,
    skill_catalog: SkillCatalog,
    mut runtime_info: TuiRuntimeInfo,
    run_handler: impl RunHandler,
) -> Result<()> {
    let store = SqliteWorkspaceStore::new(&workspace_root);
    let workspace = store.init_workspace()?;
    let mut state = AppState::new(&workspace_root);
    runtime_info.workspace_id = Some(workspace.id.to_string());
    state.set_runtime_info(runtime_info);
    state.set_skill_catalog(skill_catalog);
    match load_active_session_history(&mut state, &store) {
        Ok(()) | Err(StoreError::MissingWorkspace) => {}
        Err(error) => return Err(error.into()),
    }
    let mut terminal = setup_terminal()?;
    let run_result = run_app_loop(&mut terminal, &mut state, &store, Arc::new(run_handler));
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
    store: &SqliteWorkspaceStore,
    run_handler: Arc<dyn RunHandler>,
) -> Result<()> {
    let mut active_run: Option<BackgroundRun> = None;
    while !state.should_quit {
        terminal.draw(|frame| render_app(frame, state))?;
        while let Some(event) = active_run.as_ref().and_then(BackgroundRun::try_recv_event) {
            update(state, AppAction::AgentEvent(event));
        }
        if let Some(result) = active_run.as_ref().and_then(BackgroundRun::try_recv) {
            let was_canceled = active_run
                .as_ref()
                .is_some_and(BackgroundRun::is_cancel_requested);
            active_run = None;
            if was_canceled {
                update(state, AppAction::RunCanceled);
            } else {
                apply_run_result(state, result);
            }
            continue;
        }
        if event::poll(Duration::from_millis(50))?
            && let Event::Key(key) = event::read()?
            && let Some(command) = update(state, AppAction::Key(key))
        {
            if execute_code_change_command(state, &command)? {
                continue;
            }
            if execute_session_command(state, store, &command)? {
                continue;
            }
            match command {
                AppCommand::StartRun { request } => {
                    active_run = Some(BackgroundRun::start(
                        TuiRunRequest::new(request, state.selected_skill_names().to_vec()),
                        Arc::clone(&run_handler),
                    ));
                }
                AppCommand::CancelRun => {
                    if let Some(run) = &active_run {
                        run.cancel();
                    }
                }
                AppCommand::RequestCodeChanges { feedback } => {
                    let request =
                        format!("Revise the current code change proposal.\nFeedback: {feedback}");
                    active_run = Some(BackgroundRun::start(
                        TuiRunRequest::new(request, state.selected_skill_names().to_vec()),
                        Arc::clone(&run_handler),
                    ));
                }
                AppCommand::CreateSession { .. }
                | AppCommand::RenameActiveSession { .. }
                | AppCommand::SwitchSession { .. }
                | AppCommand::AcceptCodeChange
                | AppCommand::RejectCodeChange => {}
            }
        }
    }
    Ok(())
}

fn apply_run_result(state: &mut AppState, result: Result<RunResponse, String>) {
    match result {
        Ok(response) => {
            update(state, AppAction::RunFinished { response });
        }
        Err(error) => {
            update(state, AppAction::RunFailed { error });
        }
    }
}

fn restore_terminal(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> Result<()> {
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    Ok(())
}
