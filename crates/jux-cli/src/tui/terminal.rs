use super::{
    AppAction, AppCommand, AppState, BackgroundRun, RunHandler, RunResponse, TuiRunRequest,
    TuiRuntimeInfo, TuiViewport, execute_code_change_command, execute_session_command,
    load_active_session_history, render_app, update,
};
use anyhow::Result;
use crossterm::Command;
use crossterm::event::{
    self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
    Event, KeyEventKind, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags,
    PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use jux_core::{SkillCatalog, SqliteWorkspaceStore, StoreError};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use std::fmt;
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct DisableModifyOtherKeys;

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
    let (mut terminal, keyboard_enhancement_enabled) = setup_terminal()?;
    let run_result = run_app_loop(&mut terminal, &mut state, &store, Arc::new(run_handler));
    restore_terminal(&mut terminal, keyboard_enhancement_enabled)?;
    run_result
}

fn setup_terminal() -> Result<(Terminal<CrosstermBackend<io::Stdout>>, bool)> {
    let mut stdout = io::stdout();
    enable_raw_mode()?;
    execute!(
        stdout,
        EnterAlternateScreen,
        EnableBracketedPaste,
        EnableMouseCapture
    )?;
    // Enter the alternate screen before enabling enhanced keyboard reporting. Ghostty keeps
    // keyboard mode state per screen, so entering it afterwards resets the requested flags and
    // encodes Shift+Enter as xterm modifyOtherKeys (`ESC[27;2;13~`), which Crossterm 0.28 cannot
    // parse. Pushing the flags here makes Ghostty emit the supported CSI-u form (`ESC[13;2u`).
    let keyboard_enhancement_enabled = {
        let flags = KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
            | KeyboardEnhancementFlags::REPORT_EVENT_TYPES
            | KeyboardEnhancementFlags::REPORT_ALTERNATE_KEYS;
        let result = execute!(
            stdout,
            DisableModifyOtherKeys,
            PushKeyboardEnhancementFlags(flags)
        );
        result.is_ok()
    };
    let backend = CrosstermBackend::new(stdout);
    Ok((Terminal::new(backend)?, keyboard_enhancement_enabled))
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
        if event::poll(Duration::from_millis(50))? {
            let action = match event::read()? {
                Event::Key(key) if key.kind != KeyEventKind::Release => Some(AppAction::Key(key)),
                Event::Mouse(event) => {
                    let size = terminal.size()?;
                    Some(AppAction::Mouse {
                        event,
                        viewport: TuiViewport {
                            width: size.width,
                            height: size.height,
                        },
                    })
                }
                _ => None,
            };
            let Some(action) = action else {
                continue;
            };
            if let Some(command) = update(state, action) {
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
                        let request = format!(
                            "Revise the current code change proposal.\nFeedback: {feedback}"
                        );
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
                    AppCommand::CopyText { content } => copy_text_to_clipboard(&content)?,
                }
            }
        }
    }
    Ok(())
}

impl Command for DisableModifyOtherKeys {
    fn write_ansi(&self, formatter: &mut impl fmt::Write) -> fmt::Result {
        formatter.write_str("\x1b[>4;0m")
    }

    #[cfg(windows)]
    fn execute_winapi(&self) -> io::Result<()> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "modifyOtherKeys reset is not implemented for the legacy Windows API",
        ))
    }

    #[cfg(windows)]
    fn is_ansi_code_supported(&self) -> bool {
        false
    }
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

fn restore_terminal(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    keyboard_enhancement_enabled: bool,
) -> Result<()> {
    let keyboard_restore = if keyboard_enhancement_enabled {
        execute!(
            terminal.backend_mut(),
            PopKeyboardEnhancementFlags,
            DisableModifyOtherKeys
        )
    } else {
        Ok(())
    };
    disable_raw_mode()?;
    keyboard_restore?;
    execute!(
        terminal.backend_mut(),
        DisableBracketedPaste,
        DisableMouseCapture,
        LeaveAlternateScreen
    )?;
    terminal.show_cursor()?;
    Ok(())
}

fn copy_text_to_clipboard(content: &str) -> Result<()> {
    let encoded = base64_encode(content.as_bytes());
    let mut stdout = io::stdout();
    write!(stdout, "\x1b]52;c;{encoded}\x07")?;
    stdout.flush()?;
    Ok(())
}

fn base64_encode(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut output = String::new();
    for chunk in bytes.chunks(3) {
        let first = chunk[0];
        let second = *chunk.get(1).unwrap_or(&0);
        let third = *chunk.get(2).unwrap_or(&0);
        output.push(TABLE[(first >> 2) as usize] as char);
        output.push(TABLE[(((first & 0b0000_0011) << 4) | (second >> 4)) as usize] as char);
        if chunk.len() > 1 {
            output.push(TABLE[(((second & 0b0000_1111) << 2) | (third >> 6)) as usize] as char);
        } else {
            output.push('=');
        }
        if chunk.len() > 2 {
            output.push(TABLE[(third & 0b0011_1111) as usize] as char);
        } else {
            output.push('=');
        }
    }
    output
}
