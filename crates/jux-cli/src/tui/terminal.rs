use super::{
    AppAction, AppCommand, AppState, BackgroundRun, RunHandler, RunResponse, TuiRunRequest,
    TuiRuntimeInfo, TuiViewport, execute_code_change_command, execute_session_command,
    load_active_session_history, render_app, update,
};
use anyhow::Result;
use crossterm::Command;
use crossterm::event::{
    self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, Event, KeyCode,
    KeyEvent, KeyEventKind, KeyModifiers, KeyboardEnhancementFlags, MouseButton, MouseEvent,
    MouseEventKind, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use jux_core::{SkillCatalog, SqliteWorkspaceStore, StoreError};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use std::collections::VecDeque;
use std::fmt;
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

const EVENT_POLL_INTERVAL: Duration = Duration::from_millis(50);
// SGR mouse reports are drained from the PTY in one iteration below; retain a
// short grace period so an isolated Escape remains responsive.
const ESCAPE_SEQUENCE_TIMEOUT: Duration = Duration::from_millis(25);
const FRAGMENTED_MOUSE_RECOVERY_WINDOW: Duration = Duration::from_millis(500);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct DisableModifyOtherKeys;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct EnableTuiMouseCapture;

#[doc(hidden)]
#[derive(Debug, Default)]
/// Reassembles SGR mouse reports that Crossterm may expose as an Escape key followed by text.
///
/// A terminal can split one mouse report across multiple PTY reads. Crossterm 0.28 treats an
/// isolated leading Escape byte as a complete key event, so the remaining `[<...M` bytes would
/// otherwise leak into the prompt. This decoder briefly holds Escape and incrementally restores
/// the mouse event while preserving ordinary Escape input after the timeout.
pub struct TerminalEventDecoder {
    pending_escape: Option<PendingEscapeSequence>,
    fragmented_mouse_recovery_until: Option<Instant>,
    ready: VecDeque<Event>,
}

#[derive(Debug)]
struct PendingEscapeSequence {
    escape: Option<KeyEvent>,
    keys: Vec<KeyEvent>,
    text: String,
    deadline: Instant,
}

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
        EnableTuiMouseCapture
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
    let mut event_decoder = TerminalEventDecoder::default();
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
        if let Some(event) = read_terminal_event(&mut event_decoder)? {
            let action = match event {
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

fn read_terminal_event(decoder: &mut TerminalEventDecoder) -> Result<Option<Event>> {
    let now = Instant::now();
    if let Some(event) = decoder.next(now) {
        return Ok(Some(event));
    }
    if event::poll(decoder.poll_timeout(now, EVENT_POLL_INTERVAL))? {
        decoder.push(event::read()?, Instant::now());
        // Drain all events already buffered by the terminal in this iteration.
        // This keeps fragmented SGR mouse reports together instead of allowing
        // the decoder timeout to expire between individual bytes.
        while event::poll(Duration::ZERO)? {
            decoder.push(event::read()?, Instant::now());
        }
    }
    Ok(decoder.next(Instant::now()))
}

impl TerminalEventDecoder {
    #[doc(hidden)]
    pub fn push(&mut self, event: Event, now: Instant) {
        self.flush_expired(now);
        let Some(mut pending) = self.pending_escape.take() else {
            self.start_or_queue(event, now);
            return;
        };
        let Event::Key(key) = event else {
            self.flush_pending(pending);
            self.ready.push_back(event);
            return;
        };
        let KeyCode::Char(character) = key.code else {
            self.flush_pending(pending);
            self.start_or_queue(Event::Key(key), now);
            return;
        };
        pending.text.push(character);
        pending.keys.push(key);
        if pending.text.ends_with(['M', 'm']) {
            if let Some(mouse) = parse_sgr_mouse_fragment(&pending.text) {
                self.fragmented_mouse_recovery_until = None;
                self.ready.push_back(Event::Mouse(mouse));
            } else {
                self.flush_pending(pending);
            }
        } else if is_sgr_mouse_prefix(&pending.text) {
            pending.deadline = now + ESCAPE_SEQUENCE_TIMEOUT;
            self.pending_escape = Some(pending);
        } else {
            self.flush_pending(pending);
        }
    }

    #[doc(hidden)]
    pub fn next(&mut self, now: Instant) -> Option<Event> {
        self.flush_expired(now);
        self.ready.pop_front()
    }

    fn poll_timeout(&self, now: Instant, maximum: Duration) -> Duration {
        self.pending_escape.as_ref().map_or(maximum, |pending| {
            pending.deadline.saturating_duration_since(now).min(maximum)
        })
    }

    fn start_or_queue(&mut self, event: Event, now: Instant) {
        if let Event::Key(key) = &event
            && key.code == KeyCode::Esc
        {
            self.pending_escape = Some(PendingEscapeSequence {
                escape: Some(*key),
                keys: Vec::new(),
                text: String::new(),
                deadline: now + ESCAPE_SEQUENCE_TIMEOUT,
            });
        } else if let Event::Key(key) = &event
            && key.code == KeyCode::Char('[')
        {
            // Some terminals/crossterm versions expose the SGR mouse tail (`[<...M`)
            // without the leading Escape key. Start buffering on `[` unconditionally so
            // wheel reports cannot leak into the prompt as literal text. Non-mouse text
            // (for example `[abc`) is flushed after the normal short timeout.
            self.pending_escape = Some(PendingEscapeSequence {
                escape: None,
                keys: vec![*key],
                text: "[".to_owned(),
                deadline: now + ESCAPE_SEQUENCE_TIMEOUT,
            });
        } else {
            self.ready.push_back(event);
        }
    }

    fn flush_expired(&mut self, now: Instant) {
        if self
            .pending_escape
            .as_ref()
            .is_some_and(|pending| now >= pending.deadline)
            && let Some(pending) = self.pending_escape.take()
        {
            if pending.escape.is_some() && pending.text.is_empty() {
                self.fragmented_mouse_recovery_until = Some(now + FRAGMENTED_MOUSE_RECOVERY_WINDOW);
            }
            self.flush_pending(pending);
        }
    }

    fn flush_pending(&mut self, pending: PendingEscapeSequence) {
        self.ready.extend(pending.escape.map(Event::Key));
        self.ready.extend(pending.keys.into_iter().map(Event::Key));
    }
}

fn is_sgr_mouse_prefix(fragment: &str) -> bool {
    if fragment.len() > 32 {
        return false;
    }
    match fragment.len() {
        0 => true,
        1 => fragment == "[",
        _ => {
            fragment.starts_with("[<")
                && fragment[2..]
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || byte == b';')
        }
    }
}

fn parse_sgr_mouse_fragment(fragment: &str) -> Option<MouseEvent> {
    let released = fragment.ends_with('m');
    let values = fragment
        .strip_prefix("[<")?
        .strip_suffix(['M', 'm'])?
        .split(';')
        .map(str::parse::<u16>)
        .collect::<Result<Vec<_>, _>>()
        .ok()?;
    let [cb, column, row] = values.as_slice() else {
        return None;
    };
    let cb = u8::try_from(*cb).ok()?;
    let mut kind = sgr_mouse_kind(cb)?;
    if released && let MouseEventKind::Down(button) = kind {
        kind = MouseEventKind::Up(button);
    }
    let mut modifiers = KeyModifiers::empty();
    if cb & 4 != 0 {
        modifiers |= KeyModifiers::SHIFT;
    }
    if cb & 8 != 0 {
        modifiers |= KeyModifiers::ALT;
    }
    if cb & 16 != 0 {
        modifiers |= KeyModifiers::CONTROL;
    }
    Some(MouseEvent {
        kind,
        column: column.checked_sub(1)?,
        row: row.checked_sub(1)?,
        modifiers,
    })
}

fn sgr_mouse_kind(cb: u8) -> Option<MouseEventKind> {
    // SGR encodes wheel events with bit 6 set (64/65 for up/down). Check this
    // before decoding the low button bits; otherwise 65 is mistaken for a
    // middle-button press and scrolling appears to do nothing.
    if cb & 64 != 0 {
        return match cb & 3 {
            0 => Some(MouseEventKind::ScrollUp),
            1 => Some(MouseEventKind::ScrollDown),
            2 => Some(MouseEventKind::ScrollLeft),
            3 => Some(MouseEventKind::ScrollRight),
            _ => None,
        };
    }
    let button = (cb & 3) | ((cb & 192) >> 4);
    let dragging = cb & 32 != 0;
    match (button, dragging) {
        (0, false) => Some(MouseEventKind::Down(MouseButton::Left)),
        (1, false) => Some(MouseEventKind::Down(MouseButton::Middle)),
        (2, false) => Some(MouseEventKind::Down(MouseButton::Right)),
        (0, true) => Some(MouseEventKind::Drag(MouseButton::Left)),
        (1, true) => Some(MouseEventKind::Drag(MouseButton::Middle)),
        (2, true) => Some(MouseEventKind::Drag(MouseButton::Right)),
        (3, false) => Some(MouseEventKind::Up(MouseButton::Left)),
        (3..=5, true) => Some(MouseEventKind::Moved),
        (4, false) => Some(MouseEventKind::ScrollUp),
        (5, false) => Some(MouseEventKind::ScrollDown),
        (6, false) => Some(MouseEventKind::ScrollLeft),
        (7, false) => Some(MouseEventKind::ScrollRight),
        _ => None,
    }
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

impl Command for EnableTuiMouseCapture {
    fn write_ansi(&self, formatter: &mut impl fmt::Write) -> fmt::Result {
        // Button-event tracking covers clicks, drags, and wheel input. Keep SGR encoding enabled,
        // but avoid Crossterm's all-motion and legacy URXVT modes, which generate unnecessary
        // traffic and can leak incompatible mouse sequences into keyboard input on some terminals.
        formatter.write_str("\x1b[?1000h\x1b[?1002h\x1b[?1006h")
    }

    #[cfg(windows)]
    fn execute_winapi(&self) -> io::Result<()> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "SGR mouse capture is not implemented for the legacy Windows API",
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
