use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use jux_cli::tui::{
    AgentEventSender, AppCommand, AppModel, AppMsg, BackgroundRun, FileIndexKind, FileIndexService,
    FileIndexSnapshot, FocusedPanel, Message, MessageRole, RunResponse, SelectionPanel,
    TerminalEventDecoder, TuiApp, TuiRunRequest, TuiRunStatus, TuiRuntimeInfo, TuiSandboxSummary,
    TuiViewport, UiEvent,
};
use jux_core::{
    AgentEvent, AgentEventData, AgentEventId, AgentEventKind, AssistantResponseItem,
    CodeChangePlan, CodeChangeProposal, DistributionMetadata, LlmUsage, ProposedFileContent,
    ReviewStatus, RunId, RunStatus as CoreRunStatus, SkillCatalog, SkillDefinition, SkillOverride,
    SkillScope, SqliteWorkspaceStore, Step, StepId, StepKind, StepPayload, TuiShortcutConfig,
    TuiTheme, UpdateNotice, UpdateRecommendation,
};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::layout::Position;
use ratatui::style::Color;
use semver::Version;
use std::fs;
use std::ops::{Deref, DerefMut};
use std::path::PathBuf;
use std::process::Command as ProcessCommand;
use std::sync::{Arc, Barrier};
use std::time::{Duration, Instant};

enum TestInput {
    Model(AppMsg),
    Key(KeyEvent),
    Mouse {
        event: MouseEvent,
        viewport: TuiViewport,
    },
}

struct TestState {
    app: TuiApp,
}

impl TestState {
    fn new(workspace_root: impl Into<PathBuf>) -> Self {
        Self {
            app: TuiApp::new(AppModel::new(workspace_root)),
        }
    }

    fn input_text(&self) -> &str {
        self.app.input_text()
    }

    fn conversation_scroll_from_bottom(&self) -> u16 {
        self.app.conversation_scroll_from_bottom()
    }

    fn focused_panel(&self) -> FocusedPanel {
        self.app.focused_panel()
    }

    fn text_selection(&self) -> Option<jux_cli::tui::TextSelection> {
        self.app.text_selection()
    }

    fn sidebar_visible(&self) -> bool {
        self.app.sidebar_visible()
    }
}

impl Deref for TestState {
    type Target = AppModel;

    fn deref(&self) -> &Self::Target {
        self.app.model()
    }
}

impl DerefMut for TestState {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.app.model_mut()
    }
}

impl From<AppMsg> for TestInput {
    fn from(message: AppMsg) -> Self {
        Self::Model(message)
    }
}

fn test_key(key: KeyEvent) -> TestInput {
    TestInput::Key(key)
}

fn update(state: &mut TestState, input: impl Into<TestInput>) -> Option<AppCommand> {
    match input.into() {
        TestInput::Model(message) => state.app.update(message),
        TestInput::Key(key) => dispatch_test_event(state, UiEvent::Key(key), test_viewport()),
        TestInput::Mouse { event, viewport } => {
            dispatch_test_event(state, UiEvent::Mouse(event), viewport)
        }
    }
}

fn dispatch_test_event(
    state: &mut TestState,
    event: UiEvent,
    viewport: TuiViewport,
) -> Option<AppCommand> {
    let area = ratatui::layout::Rect::new(0, 0, viewport.width, viewport.height);
    let mut buffer = Buffer::empty(area);
    state.app.render_buffer(area, &mut buffer);
    state.app.dispatch(event, viewport).command
}

fn test_viewport() -> TuiViewport {
    TuiViewport {
        width: 80,
        height: 24,
    }
}

#[test]
fn tui_recovers_a_fragmented_sgr_mouse_report() {
    let mut decoder = TerminalEventDecoder::default();
    let started_at = Instant::now();
    decoder.push(
        Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
        started_at,
    );
    for (index, character) in "[<65;113;52M".chars().enumerate() {
        decoder.push(
            Event::Key(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE)),
            started_at + Duration::from_millis(index as u64 + 1),
        );
    }

    assert_eq!(
        decoder.next(started_at + Duration::from_millis(15)),
        Some(Event::Mouse(MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column: 112,
            row: 51,
            modifiers: KeyModifiers::NONE,
        }))
    );
    assert_eq!(decoder.next(started_at + Duration::from_millis(15)), None);
}

#[test]
fn tui_preserves_an_isolated_escape_key_after_the_mouse_sequence_timeout() {
    let mut decoder = TerminalEventDecoder::default();
    let started_at = Instant::now();
    let escape = Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    decoder.push(escape.clone(), started_at);

    assert_eq!(decoder.next(started_at + Duration::from_millis(24)), None);
    assert_eq!(
        decoder.next(started_at + Duration::from_millis(25)),
        Some(escape)
    );
}

#[test]
fn tui_recovers_a_mouse_tail_after_crossterm_already_emitted_escape() {
    let mut decoder = TerminalEventDecoder::default();
    let started_at = Instant::now();
    let escape = Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    decoder.push(escape.clone(), started_at);
    assert_eq!(
        decoder.next(started_at + Duration::from_millis(25)),
        Some(escape)
    );

    for (index, character) in "[<64;113;52M".chars().enumerate() {
        decoder.push(
            Event::Key(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE)),
            started_at + Duration::from_millis(index as u64 + 250),
        );
    }

    assert_eq!(
        decoder.next(started_at + Duration::from_millis(265)),
        Some(Event::Mouse(MouseEvent {
            kind: MouseEventKind::ScrollUp,
            column: 112,
            row: 51,
            modifiers: KeyModifiers::NONE,
        }))
    );
}

#[test]
fn tui_preserves_text_after_escape_when_it_is_not_a_mouse_report() {
    let mut decoder = TerminalEventDecoder::default();
    let started_at = Instant::now();
    let escape = Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    let bracket = Event::Key(KeyEvent::new(KeyCode::Char('['), KeyModifiers::NONE));
    let letter = Event::Key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));
    decoder.push(escape.clone(), started_at);
    decoder.push(bracket.clone(), started_at + Duration::from_millis(1));
    decoder.push(letter.clone(), started_at + Duration::from_millis(2));

    assert_eq!(
        decoder.next(started_at + Duration::from_millis(2)),
        Some(escape)
    );
    assert_eq!(
        decoder.next(started_at + Duration::from_millis(2)),
        Some(bracket)
    );
    assert_eq!(
        decoder.next(started_at + Duration::from_millis(2)),
        Some(letter)
    );
}

#[test]
fn tui_accepts_q_as_text_input() {
    let mut state = TestState::new("/workspace");

    update(
        &mut state,
        test_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE)),
    );

    assert_eq!(state.input_text(), "q");
    assert!(!state.should_quit);
}

#[test]
fn tui_quits_when_ctrl_c_is_pressed() {
    let mut state = TestState::new("/workspace");

    update(
        &mut state,
        test_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
    );

    assert!(state.should_quit);
}

#[test]
fn tui_accepts_multiline_text_input() {
    let mut state = TestState::new("/workspace");

    type_text(&mut state, "Fix");
    update(
        &mut state,
        test_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT)),
    );
    let newline_buffer = render_to_buffer(&mut state, 80, 24);
    let newline_cursor = render_cursor_position(&mut state, 80, 24);
    let (first_line_row, _) =
        find_fragment_position(&newline_buffer, "Fix").expect("first input line is rendered");
    assert_row_has_background(
        &newline_buffer,
        first_line_row.saturating_sub(1),
        0,
        47,
        input_background(),
    );
    assert_row_has_background(
        &newline_buffer,
        newline_cursor.y.saturating_add(1),
        0,
        47,
        input_background(),
    );
    type_text(&mut state, "query");

    assert_eq!(state.input_text(), "Fix\nquery");
    let buffer = render_to_buffer(&mut state, 80, 24);
    assert_buffer_contains(&buffer, "Fix");
    assert_buffer_contains(&buffer, "query");
    let (_, first_line_column) =
        find_fragment_position(&buffer, "Fix").expect("first input line is rendered");
    let (_, continuation_column) =
        find_fragment_position(&buffer, "query").expect("continued input line is rendered");
    assert_eq!(continuation_column, first_line_column);
    assert_buffer_does_not_contain(&buffer, "> query");
}

#[test]
fn tui_ignores_shift_enter_key_release_events() {
    let mut state = TestState::new("/workspace");
    type_text(&mut state, "first");

    update(
        &mut state,
        test_key(KeyEvent::new_with_kind(
            KeyCode::Enter,
            KeyModifiers::SHIFT,
            KeyEventKind::Release,
        )),
    );

    assert_eq!(state.input_text(), "first");
}

#[test]
fn tui_inserts_text_at_the_cursor() {
    let mut state = TestState::new("/workspace");

    type_text(&mut state, "ac");
    update(
        &mut state,
        test_key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE)),
    );
    type_text(&mut state, "b");

    assert_eq!(state.input_text(), "abc");
}

#[test]
fn tui_deletes_text_around_the_cursor() {
    let mut state = TestState::new("/workspace");

    type_text(&mut state, "ab中c");
    update(
        &mut state,
        test_key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE)),
    );
    update(
        &mut state,
        test_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE)),
    );
    update(
        &mut state,
        test_key(KeyEvent::new(KeyCode::Delete, KeyModifiers::NONE)),
    );

    assert_eq!(state.input_text(), "ab");
}

#[test]
fn tui_completes_file_references_and_renders_suggestions() {
    let mut state = TestState::new("/workspace");
    update(
        &mut state,
        AppMsg::FileIndexUpdated(FileIndexSnapshot {
            kind: FileIndexKind::Filesystem,
            files: vec!["README.md".to_owned(), "src/main.rs".to_owned()],
        }),
    );
    type_text(&mut state, "Please inspect @mai");

    let buffer = render_to_buffer(&mut state, 80, 24);
    assert_buffer_contains(&buffer, "@src/main.rs");
    update(
        &mut state,
        test_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
    );

    assert_eq!(state.input_text(), "Please inspect @src/main.rs ");
    let completed = render_to_buffer(&mut state, 80, 24);
    assert_buffer_fragment_has_foreground(&completed, "@src/main.rs", Color::Cyan);
    assert_eq!(find_fragment_position(&completed, "@README.md"), None);
    let command = update(
        &mut state,
        test_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
    );
    assert_eq!(
        command,
        Some(AppCommand::StartRun {
            request: "Please inspect @src/main.rs ".to_owned(),
        })
    );
    assert_eq!(
        state
            .messages()
            .last()
            .map(|message| message.content.as_str()),
        Some("Please inspect @src/main.rs ")
    );
}

#[test]
fn tui_file_reference_popup_clears_the_conversation_beneath_it() {
    let mut state = TestState::new("/workspace");
    update(
        &mut state,
        AppMsg::AssistantMessage {
            content: (0..14)
                .map(|index| format!("UNDERLYING CONVERSATION TEXT {index}"))
                .collect::<Vec<_>>()
                .join("\n"),
        },
    );
    update(
        &mut state,
        AppMsg::FileIndexUpdated(FileIndexSnapshot {
            kind: FileIndexKind::Filesystem,
            files: (0..8).map(|index| format!("src/file-{index}.rs")).collect(),
        }),
    );
    type_text(&mut state, "@");

    let buffer = render_to_buffer(&mut state, 80, 24);

    assert_buffer_contains(&buffer, "@src/file-0.rs");
    let (row, column) = find_fragment_position(&buffer, "@src/file-0.rs")
        .expect("first file suggestion is rendered");
    let content_end = column + "@src/file-0.rs".chars().count() as u16;
    assert!(
        (content_end..content_end + 12).all(|x| buffer
            .cell((x, row))
            .is_some_and(|cell| cell.symbol() == " ")),
        "file popup leaves conversation text behind"
    );
}

#[test]
fn tui_file_reference_selection_scrolls_through_all_matches() {
    let mut state = TestState::new("/workspace");
    update(
        &mut state,
        AppMsg::FileIndexUpdated(FileIndexSnapshot {
            kind: FileIndexKind::Filesystem,
            files: (0..12)
                .map(|index| format!("src/file-{index:02}.rs"))
                .collect(),
        }),
    );
    type_text(&mut state, "@");
    for _ in 0..8 {
        update(
            &mut state,
            test_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE)),
        );
    }

    let buffer = render_to_buffer(&mut state, 80, 24);
    assert_buffer_contains(&buffer, "@src/file-08.rs");
    assert_buffer_does_not_contain(&buffer, "@src/file-00.rs");
    update(
        &mut state,
        test_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
    );
    assert_eq!(state.input_text(), "@src/file-08.rs ");
}

#[test]
fn tui_deletes_a_file_reference_as_one_input_unit() {
    let snapshot = FileIndexSnapshot {
        kind: FileIndexKind::Filesystem,
        files: vec!["docs/My File.md".to_owned(), "src/main.rs".to_owned()],
    };
    let mut backspace_state = TestState::new("/workspace");
    update(
        &mut backspace_state,
        AppMsg::FileIndexUpdated(snapshot.clone()),
    );
    type_text(&mut backspace_state, "@src/main.rs");
    update(
        &mut backspace_state,
        test_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE)),
    );
    assert_eq!(backspace_state.input_text(), "");

    let mut delete_state = TestState::new("/workspace");
    update(&mut delete_state, AppMsg::FileIndexUpdated(snapshot));
    type_text(&mut delete_state, "@My");
    update(
        &mut delete_state,
        test_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
    );
    assert_eq!(delete_state.input_text(), "@{docs/My File.md}");
    update(
        &mut delete_state,
        test_key(KeyEvent::new(KeyCode::Home, KeyModifiers::NONE)),
    );
    update(
        &mut delete_state,
        test_key(KeyEvent::new(KeyCode::Delete, KeyModifiers::NONE)),
    );
    assert_eq!(delete_state.input_text(), "");
}

#[test]
fn tui_submits_braced_file_references_as_workspace_relative_paths() {
    let mut state = TestState::new("/workspace");
    update(
        &mut state,
        AppMsg::FileIndexUpdated(FileIndexSnapshot {
            kind: FileIndexKind::Filesystem,
            files: vec!["docs/My File.md".to_owned()],
        }),
    );
    type_text(&mut state, "Read @My");
    update(
        &mut state,
        test_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
    );

    let command = update(
        &mut state,
        test_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
    );

    assert_eq!(
        command,
        Some(AppCommand::StartRun {
            request: "Read @{docs/My File.md} ".to_owned(),
        })
    );
}

#[test]
fn file_index_service_indexes_and_refreshes_non_git_workspaces() {
    let workspace = assert_fs::TempDir::new().expect("temp workspace exists");
    fs::create_dir(workspace.path().join("src")).expect("source directory is created");
    fs::write(workspace.path().join("src/main.rs"), "fn main() {}")
        .expect("source file is written");
    fs::create_dir(workspace.path().join(".git")).expect("git metadata directory is created");
    fs::write(workspace.path().join(".git/config"), "not a repository")
        .expect("git metadata file is written");

    let service = FileIndexService::start(workspace.path().to_path_buf());
    let initial = service
        .recv_timeout(Duration::from_secs(3))
        .expect("initial file index arrives");
    assert_eq!(initial.kind, FileIndexKind::Filesystem);
    assert_eq!(initial.files, vec!["src/main.rs"]);

    fs::write(workspace.path().join("README.md"), "# Workspace").expect("new file is written");
    let refreshed = wait_for_file(&service, "README.md");
    assert!(refreshed.files.contains(&"src/main.rs".to_owned()));
}

#[test]
fn file_index_service_uses_git_tracked_files_and_gitignore_rules() {
    let workspace = assert_fs::TempDir::new().expect("temp workspace exists");
    fs::write(workspace.path().join("tracked.rs"), "fn tracked() {}")
        .expect("tracked file is written");
    fs::write(workspace.path().join("ignored.log"), "ignored").expect("ignored file is written");
    fs::write(workspace.path().join(".gitignore"), "*.log\n").expect("gitignore is written");
    assert!(
        ProcessCommand::new("git")
            .args(["init", "-q"])
            .current_dir(workspace.path())
            .status()
            .expect("git init runs")
            .success()
    );
    assert!(
        ProcessCommand::new("git")
            .args(["add", "tracked.rs", ".gitignore"])
            .current_dir(workspace.path())
            .status()
            .expect("git add runs")
            .success()
    );

    let service = FileIndexService::start(workspace.path().to_path_buf());
    let snapshot = service
        .recv_timeout(Duration::from_secs(3))
        .expect("git file index arrives");

    assert_eq!(snapshot.kind, FileIndexKind::Git);
    assert_eq!(snapshot.files, vec![".gitignore", "tracked.rs"]);

    fs::write(workspace.path().join("new.rs"), "fn new_file() {}")
        .expect("new tracked file is written");
    assert!(
        ProcessCommand::new("git")
            .args(["add", "new.rs"])
            .current_dir(workspace.path())
            .status()
            .expect("second git add runs")
            .success()
    );
    let refreshed = wait_for_file(&service, "new.rs");
    assert!(!refreshed.files.contains(&"ignored.log".to_owned()));
}

#[test]
fn tui_moves_the_cursor_across_lines() {
    let mut state = TestState::new("/workspace");

    type_text(&mut state, "abcd");
    update(
        &mut state,
        test_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT)),
    );
    type_text(&mut state, "xy");
    update(
        &mut state,
        test_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE)),
    );
    type_text(&mut state, "Z");
    update(
        &mut state,
        test_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE)),
    );
    update(
        &mut state,
        test_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE)),
    );
    type_text(&mut state, "Q");
    update(
        &mut state,
        test_key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE)),
    );
    update(
        &mut state,
        test_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE)),
    );
    type_text(&mut state, "!");

    assert_eq!(state.input_text(), "abZcd\nxyQ!");
}

#[test]
fn tui_displays_the_input_cursor_at_the_editing_position() {
    let mut state = TestState::new("/workspace");
    type_text(&mut state, "你");

    let single_line = render_to_buffer(&mut state, 80, 24);
    assert_eq!(find_fragment_position(&single_line, "> 你"), Some((21, 1)));
    assert_eq!(
        render_cursor_position(&mut state, 80, 24),
        Position::new(5, 21)
    );

    update(
        &mut state,
        test_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT)),
    );
    type_text(&mut state, "a");

    assert_eq!(
        render_cursor_position(&mut state, 80, 24),
        Position::new(4, 21)
    );
}

#[test]
fn tui_submits_input_as_a_new_run_request() {
    let mut state = TestState::new("/workspace");
    type_text(&mut state, "Fix the failing test");

    let command = update(
        &mut state,
        test_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
    );

    assert_eq!(
        command,
        Some(AppCommand::StartRun {
            request: "Fix the failing test".to_owned(),
        })
    );
    assert_eq!(state.input_text(), "");
}

#[test]
fn tui_does_not_submit_blank_input() {
    let mut state = TestState::new("/workspace");
    type_text(&mut state, " \n ");

    let command = update(
        &mut state,
        test_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
    );

    assert_eq!(command, None);
    assert_eq!(state.input_text(), " \n ");
}

#[test]
fn tui_displays_the_submitted_user_request() {
    let mut state = TestState::new("/workspace");
    type_text(&mut state, "Explain this workspace");
    update(
        &mut state,
        test_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
    );

    let buffer = render_to_buffer(&mut state, 80, 24);

    assert_buffer_contains(&buffer, "> Explain this workspace");
}

#[test]
fn tui_displays_the_agent_response() {
    let mut state = TestState::new("/workspace");

    type_text(&mut state, "User request");
    update(
        &mut state,
        test_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
    );

    update(
        &mut state,
        AppMsg::AssistantMessage {
            content: "The workspace contains two crates.".to_owned(),
        },
    );
    update(
        &mut state,
        AppMsg::AssistantMessage {
            content: "The second response is separate.".to_owned(),
        },
    );

    let buffer = render_to_buffer(&mut state, 80, 40);
    assert_buffer_contains(&buffer, "The workspace contains two crates.");
    let (user_row, _) =
        find_fragment_position(&buffer, "User request").expect("user message is rendered");
    let (first_response_row, _) =
        find_fragment_position(&buffer, "The workspace contains two crates.")
            .expect("first assistant response is rendered");
    let (second_response_row, _) =
        find_fragment_position(&buffer, "The second response is separate.")
            .expect("second assistant response is rendered");
    assert_eq!(
        find_fragment_position(&buffer, "User request"),
        Some((2, 4))
    );
    assert_eq!(
        buffer.cell((1, user_row)).map(|cell| cell.symbol()),
        Some("\u{00a0}")
    );
    assert_eq!(
        buffer.cell((46, user_row)).map(|cell| cell.symbol()),
        Some("\u{00a0}")
    );
    for row in user_row.saturating_sub(1)..=user_row.saturating_add(1) {
        assert_row_has_background(&buffer, row, 1, 47, input_active_background());
    }
    assert_row_has_background(
        &buffer,
        user_row.saturating_add(2),
        1,
        47,
        conversation_background(),
    );
    assert_row_has_background(
        &buffer,
        first_response_row,
        1,
        47,
        conversation_background(),
    );
    assert_row_has_background(
        &buffer,
        second_response_row,
        1,
        47,
        conversation_background(),
    );
}

#[test]
fn tui_keeps_commands_in_their_conversation_order() {
    let mut state = TestState::new("/workspace");
    update(
        &mut state,
        AppMsg::AssistantMessage {
            content: "First response".to_owned(),
        },
    );
    update(
        &mut state,
        AppMsg::AgentEvent(AgentEvent::new(
            AgentEventId::tool(1, "exec", 1),
            AgentEventKind::Started,
            AgentEventData::ToolStarted {
                name: "exec".to_owned(),
                call_id: Some("call-1".to_owned()),
                arguments: serde_json::json!({ "program": "first", "args": [] }),
            },
        )),
    );
    update(
        &mut state,
        AppMsg::AssistantMessage {
            content: "Second response".to_owned(),
        },
    );
    update(
        &mut state,
        AppMsg::AgentEvent(AgentEvent::new(
            AgentEventId::tool(1, "exec", 2),
            AgentEventKind::Started,
            AgentEventData::ToolStarted {
                name: "exec".to_owned(),
                call_id: Some("call-2".to_owned()),
                arguments: serde_json::json!({ "program": "second", "args": [] }),
            },
        )),
    );

    let buffer = render_to_buffer(&mut state, 100, 40);
    let first_response =
        find_fragment_position(&buffer, "First response").expect("first response is rendered");
    let first_command =
        find_fragment_position(&buffer, "▶ $ first").expect("first command is rendered");
    let second_response =
        find_fragment_position(&buffer, "Second response").expect("second response is rendered");
    let second_command =
        find_fragment_position(&buffer, "▶ $ second").expect("second command is rendered");
    assert!(
        first_response.0 < first_command.0
            && first_command.0 < second_response.0
            && second_response.0 < second_command.0
    );
}

#[test]
fn tui_appends_streamed_assistant_text_deltas() {
    let mut state = TestState::new("/workspace");

    for content in ["Hello", " world"] {
        update(
            &mut state,
            AppMsg::AgentEvent(AgentEvent::new(
                AgentEventId::llm(1, 1),
                AgentEventKind::Output,
                AgentEventData::AssistantTextDelta {
                    content: content.to_owned(),
                },
            )),
        );
    }

    assert_eq!(state.messages().len(), 1);
    assert_eq!(state.messages()[0].content, "Hello world");
    assert_eq!(state.message_revision(0), Some(1));
}

#[test]
fn tui_restores_canceled_partial_assistant_output() {
    let mut state = TestState::new("/workspace");
    let run_id = RunId::from("workspace-0001-000001".to_owned());
    let checkpoint = Step::new(
        StepId::new(&run_id, 1),
        StepKind::AssistantOutputCheckpoint,
        StepPayload::AssistantOutputCheckpoint {
            content: "partial response".to_owned(),
        },
    );

    update(
        &mut state,
        AppMsg::RunFinished {
            response: RunResponse {
                session_id: "workspace-0001".to_owned(),
                run_id: run_id.to_string(),
                status: CoreRunStatus::Canceled,
                created_at: 1_000,
                updated_at: 1_100,
                answer: None,
                steps: vec![checkpoint],
            },
        },
    );

    assert_eq!(state.messages()[0].content, "partial response");
}

#[test]
fn tui_ignores_duplicate_sequenced_text_deltas() {
    let mut state = TestState::new("/workspace");
    let mut event = AgentEvent::new(
        AgentEventId::llm(1, 1),
        AgentEventKind::Output,
        AgentEventData::AssistantTextDelta {
            content: "once".to_owned(),
        },
    );
    event.sequence = 2;

    update(&mut state, AppMsg::AgentEvent(event.clone()));
    update(&mut state, AppMsg::AgentEvent(event));

    assert_eq!(state.messages()[0].content, "once");
}

#[test]
fn tui_resets_event_sequence_for_a_new_background_run() {
    let mut state = TestState::new("/workspace");
    let mut first = AgentEvent::new(
        AgentEventId::llm(1, 1),
        AgentEventKind::Output,
        AgentEventData::AssistantTextDelta {
            content: "first".to_owned(),
        },
    );
    first.sequence = 4;
    update(&mut state, AppMsg::AgentEvent(first));

    type_text(&mut state, "next run");
    assert!(matches!(
        update(
            &mut state,
            test_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        ),
        Some(AppCommand::StartRun { .. })
    ));
    let mut second = AgentEvent::new(
        AgentEventId::llm(1, 1),
        AgentEventKind::Output,
        AgentEventData::AssistantTextDelta {
            content: "second".to_owned(),
        },
    );
    second.sequence = 1;
    update(&mut state, AppMsg::AgentEvent(second));

    assert!(
        state
            .messages()
            .iter()
            .any(|message| message.content == "second")
    );
}

#[test]
fn tui_keeps_persisted_commands_visible_when_a_run_finishes() {
    let mut state = TestState::new("/workspace");
    type_text(&mut state, "Inspect the workspace");
    update(
        &mut state,
        test_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
    );
    let run_id = RunId::from("workspace-0001-000001".to_owned());
    let steps = vec![
        Step::new(
            StepId::new(&run_id, 1),
            StepKind::UserMessage,
            StepPayload::UserMessage {
                content: "Inspect the workspace".to_owned(),
            },
        ),
        assistant_step(&run_id, 2, "I will inspect the workspace."),
        assistant_step(&run_id, 3, "I need one command."),
        Step::new(
            StepId::new(&run_id, 4),
            StepKind::AssistantResponse,
            StepPayload::AssistantResponse {
                message_id: None,
                usage: LlmUsage::default(),
                items: vec![AssistantResponseItem::ToolCall {
                    id: "tool-1".to_owned(),
                    call_id: Some("call-1".to_owned()),
                    name: "exec".to_owned(),
                    arguments: serde_json::json!({ "program": "ls", "args": [] }),
                }],
            },
        ),
        Step::new(
            StepId::new(&run_id, 5),
            StepKind::ToolResult,
            StepPayload::ToolResult {
                id: "tool-1".to_owned(),
                call_id: Some("call-1".to_owned()),
                content: serde_json::json!({
                    "success": true,
                    "exit_code": 0,
                    "stdout": "Cargo.toml",
                    "stderr": ""
                }),
            },
        ),
        assistant_step(&run_id, 6, "Inspection complete."),
    ];

    update(
        &mut state,
        AppMsg::RunFinished {
            response: RunResponse {
                session_id: "workspace-0001".to_owned(),
                run_id: run_id.to_string(),
                status: CoreRunStatus::Completed,
                created_at: 1_000,
                updated_at: 1_500,
                answer: Some("Inspection complete.".to_owned()),
                steps,
            },
        },
    );

    let buffer = render_to_buffer(&mut state, 100, 40);
    assert_buffer_contains(&buffer, "▶ $ ls");
}

#[test]
fn tui_scrolls_through_messages_longer_than_the_viewport() {
    let mut state = TestState::new("/workspace");
    for index in 0..12 {
        update(
            &mut state,
            AppMsg::AssistantMessage {
                content: format!("response-{index}"),
            },
        );
    }

    let initial = render_to_buffer(&mut state, 80, 24);
    assert_buffer_contains(&initial, "response-11");
    assert_buffer_does_not_contain(&initial, "response-0");
    let initial_thumb_row = scrollbar_thumb_start(&initial, 47);

    update(
        &mut state,
        TestInput::Mouse {
            event: mouse_event(MouseEventKind::ScrollUp, 10, 10),
            viewport: TuiViewport {
                width: 80,
                height: 24,
            },
        },
    );
    update(
        &mut state,
        TestInput::Mouse {
            event: mouse_event(MouseEventKind::ScrollUp, 10, 10),
            viewport: TuiViewport {
                width: 80,
                height: 24,
            },
        },
    );
    let scrolled = render_to_buffer(&mut state, 80, 24);
    assert_buffer_contains(&scrolled, "response-8");
    assert_buffer_does_not_contain(&scrolled, "response-11");
    assert!(scrollbar_thumb_start(&scrolled, 47) < initial_thumb_row);
    update(
        &mut state,
        AppMsg::AssistantMessage {
            content: "response-12".to_owned(),
        },
    );
    assert_eq!(state.conversation_scroll_from_bottom(), 10);
    assert_buffer_does_not_contain(&render_to_buffer(&mut state, 80, 24), "response-12");

    for _ in 0..2 {
        update(
            &mut state,
            TestInput::Mouse {
                event: mouse_event(MouseEventKind::ScrollDown, 10, 10),
                viewport: TuiViewport {
                    width: 80,
                    height: 24,
                },
            },
        );
    }
    let returned_to_bottom = render_to_buffer(&mut state, 80, 24);
    assert_buffer_contains(&returned_to_bottom, "response-12");
    assert_eq!(state.conversation_scroll_from_bottom(), 0);

    update(
        &mut state,
        TestInput::Mouse {
            event: mouse_event(MouseEventKind::ScrollUp, 70, 10),
            viewport: TuiViewport {
                width: 80,
                height: 24,
            },
        },
    );
    assert_eq!(state.conversation_scroll_from_bottom(), 0);

    update(
        &mut state,
        TestInput::Mouse {
            event: mouse_event(MouseEventKind::ScrollUp, 0, 23),
            viewport: TuiViewport {
                width: 80,
                height: 24,
            },
        },
    );
    assert_eq!(state.conversation_scroll_from_bottom(), 5);
    update(
        &mut state,
        TestInput::Mouse {
            event: mouse_event(MouseEventKind::ScrollDown, 47, 23),
            viewport: TuiViewport {
                width: 80,
                height: 24,
            },
        },
    );
    assert_eq!(state.conversation_scroll_from_bottom(), 0);
}

#[test]
fn tui_keeps_input_and_status_fixed_beside_the_full_height_scrollbar() {
    let mut state = TestState::new("/workspace");
    for index in 0..12 {
        update(
            &mut state,
            AppMsg::AssistantMessage {
                content: format!("response-{index}"),
            },
        );
    }
    type_text(&mut state, "draft");
    update(
        &mut state,
        test_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT)),
    );

    let initial = render_to_buffer(&mut state, 80, 24);
    let input_position =
        find_fragment_position(&initial, "> draft").expect("input is rendered above status");
    let status_position =
        find_fragment_position(&initial, "Shift+Enter newline").expect("status bar is rendered");
    assert_eq!(status_position.0, 23);
    assert_eq!(input_position.1, 1);
    assert_row_has_background(&initial, 22, 0, 47, input_background());
    assert_row_has_background(&initial, 23, 0, 47, status_bar_background());
    assert!((1..18).all(|row| {
        initial
            .cell((47, row))
            .is_some_and(|cell| matches!(cell.symbol(), "█" | "│"))
    }));
    assert!((18..24).all(|row| {
        initial
            .cell((47, row))
            .is_some_and(|cell| !matches!(cell.symbol(), "█" | "│"))
    }));

    update(
        &mut state,
        TestInput::Mouse {
            event: mouse_event(MouseEventKind::ScrollUp, 10, 10),
            viewport: TuiViewport {
                width: 80,
                height: 24,
            },
        },
    );
    let scrolled = render_to_buffer(&mut state, 80, 24);
    assert_eq!(
        find_fragment_position(&scrolled, "> draft"),
        Some(input_position)
    );
    assert_eq!(
        find_fragment_position(&scrolled, "Shift+Enter newline"),
        Some(status_position)
    );
    assert!((1..18).all(|row| {
        scrolled
            .cell((47, row))
            .is_some_and(|cell| matches!(cell.symbol(), "█" | "│"))
    }));
}

#[test]
fn tui_run_executes_in_the_background() {
    let release = Arc::new(Barrier::new(2));
    let runner_release = Arc::clone(&release);
    let run = BackgroundRun::start(
        "inspect workspace".to_owned(),
        Arc::new(move |request: TuiRunRequest, _cancellation, _events| {
            runner_release.wait();
            Ok(RunResponse {
                session_id: "workspace-0001".to_owned(),
                run_id: "workspace-0001-000001".to_owned(),
                status: CoreRunStatus::Completed,
                created_at: 1_000,
                updated_at: 1_250,
                answer: Some(format!("completed: {}", request.request)),
                steps: Vec::new(),
            })
        }),
    );
    let mut state = TestState::new("/workspace");

    assert_eq!(run.try_recv(), None);
    type_text(&mut state, "input remains responsive");
    assert_eq!(state.input_text(), "input remains responsive");

    release.wait();
    assert_eq!(
        run.recv_timeout(Duration::from_secs(1)),
        Some(Ok(RunResponse {
            session_id: "workspace-0001".to_owned(),
            run_id: "workspace-0001-000001".to_owned(),
            status: CoreRunStatus::Completed,
            created_at: 1_000,
            updated_at: 1_250,
            answer: Some("completed: inspect workspace".to_owned()),
            steps: Vec::new(),
        }))
    );
}

#[test]
fn tui_background_run_forwards_agent_events_before_completion() {
    let release = Arc::new(Barrier::new(2));
    let runner_release = Arc::clone(&release);
    let run = BackgroundRun::start(
        "inspect workspace".to_owned(),
        Arc::new(
            move |_request: TuiRunRequest, _cancellation, events: AgentEventSender| {
                events.send(AgentEvent::new(
                    AgentEventId::llm(1, 1),
                    AgentEventKind::Started,
                    AgentEventData::LlmStarted,
                ));
                runner_release.wait();
                Ok(RunResponse {
                    session_id: "workspace-0001".to_owned(),
                    run_id: "workspace-0001-000001".to_owned(),
                    status: CoreRunStatus::Completed,
                    created_at: 1_000,
                    updated_at: 1_250,
                    answer: None,
                    steps: Vec::new(),
                })
            },
        ),
    );

    let event = run
        .recv_event_timeout(Duration::from_secs(1))
        .expect("LLM event arrives while run is active");
    assert_eq!(event.data, AgentEventData::LlmStarted);
    assert_eq!(run.try_recv(), None);

    release.wait();
    assert!(run.recv_timeout(Duration::from_secs(1)).is_some());
}

#[test]
fn tui_background_run_forwards_explicit_skill_selection() {
    let run = BackgroundRun::start(
        TuiRunRequest::new(
            "review workspace",
            vec!["review".to_owned(), "security".to_owned()],
        ),
        Arc::new(
            move |request: TuiRunRequest, _cancellation, _events: AgentEventSender| {
                Ok(RunResponse {
                    session_id: "workspace-0001".to_owned(),
                    run_id: "workspace-0001-000001".to_owned(),
                    status: CoreRunStatus::Completed,
                    created_at: 1_000,
                    updated_at: 1_250,
                    answer: Some(format!(
                        "{}: {}",
                        request.explicit_skills.join(","),
                        request.request
                    )),
                    steps: Vec::new(),
                })
            },
        ),
    );

    let response = run
        .recv_timeout(Duration::from_secs(1))
        .expect("background run completes")
        .expect("background run succeeds");

    assert_eq!(
        response.answer.as_deref(),
        Some("review,security: review workspace")
    );
}

#[test]
fn tui_updates_llm_lifecycle_in_the_timeline() {
    let mut state = TestState::new("/workspace");
    let event_id = AgentEventId::llm(1, 1);

    update(
        &mut state,
        AppMsg::AgentEvent(AgentEvent::new(
            event_id.clone(),
            AgentEventKind::Started,
            AgentEventData::LlmStarted,
        )),
    );
    let running = render_to_buffer(&mut state, 80, 24);
    assert_buffer_contains(&running, "LLM");
    assert_buffer_contains(&running, "Running");

    update(
        &mut state,
        AppMsg::AgentEvent(AgentEvent::new(
            event_id,
            AgentEventKind::Completed,
            AgentEventData::LlmCompleted,
        )),
    );
    assert!(state.timeline().is_empty());
    let completed = render_to_buffer(&mut state, 80, 24);
    assert_buffer_does_not_contain(&completed, "LLM");
    assert_buffer_does_not_contain(&completed, "Completed");
}

#[test]
fn tui_displays_llm_failure_details_in_the_timeline() {
    let mut state = TestState::new("/workspace");

    update(
        &mut state,
        AppMsg::AgentEvent(AgentEvent::new(
            AgentEventId::llm(1, 1),
            AgentEventKind::Failed,
            AgentEventData::LlmFailed {
                error: "model unavailable".to_owned(),
            },
        )),
    );

    let buffer = render_to_buffer(&mut state, 80, 24);
    assert_buffer_contains(&buffer, "LLM");
    assert_buffer_contains(&buffer, "Failed");
    assert_buffer_contains(&buffer, "model unavailable");
}

#[test]
fn tui_updates_tool_lifecycle_and_keeps_its_output() {
    let mut state = TestState::new("/workspace");
    let event_id = AgentEventId::tool(1, "exec", 1);

    update(
        &mut state,
        AppMsg::AgentEvent(AgentEvent::new(
            event_id.clone(),
            AgentEventKind::Started,
            AgentEventData::ToolStarted {
                name: "exec".to_owned(),
                call_id: Some("call-1".to_owned()),
                arguments: serde_json::json!({ "program": "cargo", "args": ["test"] }),
            },
        )),
    );
    let running = render_to_buffer(&mut state, 80, 24);
    let (running_row, running_column) =
        find_fragment_position(&running, "▶ $ cargo test").expect("running command is rendered");
    let running_color = running
        .cell((running_column, running_row))
        .map(|cell| cell.fg)
        .expect("running command icon has a color");
    assert!(
        [
            Color::Rgb(70, 130, 180),
            Color::Rgb(64, 170, 190),
            Color::Rgb(80, 200, 170),
            Color::Rgb(190, 210, 90),
            Color::Rgb(230, 180, 70),
            Color::Rgb(170, 110, 190),
        ]
        .contains(&running_color)
    );
    update(
        &mut state,
        AppMsg::AgentEvent(AgentEvent::new(
            event_id.clone(),
            AgentEventKind::Output,
            AgentEventData::ToolOutput {
                name: "exec".to_owned(),
                content: serde_json::json!({
                    "success": true,
                    "exit_code": 0,
                    "stdout": "tests passed",
                    "stderr": ""
                }),
            },
        )),
    );
    update(
        &mut state,
        AppMsg::AgentEvent(AgentEvent::new(
            event_id,
            AgentEventKind::Completed,
            AgentEventData::ToolCompleted {
                name: "exec".to_owned(),
                call_id: Some("call-1".to_owned()),
            },
        )),
    );

    assert_eq!(state.timeline().len(), 1);
    let buffer = render_to_buffer(&mut state, 80, 24);
    assert_buffer_contains(&buffer, "$ cargo test");
    assert_buffer_does_not_contain(&buffer, "Succeeded");
    assert_buffer_does_not_contain(&buffer, "exit 0");
    assert_buffer_does_not_contain(&buffer, "stdout 1 line(s)");
    assert_buffer_does_not_contain(&buffer, "tests passed");
    assert_buffer_does_not_contain(&buffer, "╭");
    assert_buffer_does_not_contain(&buffer, "╮");
    assert_buffer_fragment_has_fg_bg(&buffer, "cargo", Color::Yellow, Color::Rgb(20, 28, 36));
    assert_buffer_fragment_has_fg_bg(&buffer, "▶", Color::Green, Color::Rgb(20, 28, 36));
}

#[test]
fn tui_appends_tool_output_chunks_before_completion() {
    let mut state = TestState::new("/workspace");
    let event_id = AgentEventId::tool(1, "exec", 1);
    update(
        &mut state,
        AppMsg::AgentEvent(AgentEvent::new(
            event_id.clone(),
            AgentEventKind::Started,
            AgentEventData::ToolStarted {
                name: "exec".to_owned(),
                call_id: Some("call-1".to_owned()),
                arguments: serde_json::json!({ "program": "cargo", "args": ["test"] }),
            },
        )),
    );
    for content in ["first\n", "second\n"] {
        update(
            &mut state,
            AppMsg::AgentEvent(AgentEvent::new(
                event_id.clone(),
                AgentEventKind::Output,
                AgentEventData::ToolOutputChunk {
                    name: "exec".to_owned(),
                    stream: jux_core::ToolOutputStream::Stdout,
                    content: content.to_owned(),
                },
            )),
        );
    }

    assert_eq!(
        state.timeline()[0]
            .command
            .as_ref()
            .map(|command| command.stdout.as_str()),
        Some("first\nsecond\n")
    );
}

#[test]
fn tui_expands_tool_arguments_and_output() {
    let mut state = TestState::new("/workspace");
    let event_id = AgentEventId::tool(1, "exec", 1);
    update(
        &mut state,
        AppMsg::AgentEvent(AgentEvent::new(
            event_id.clone(),
            AgentEventKind::Started,
            AgentEventData::ToolStarted {
                name: "exec".to_owned(),
                call_id: Some("call-1".to_owned()),
                arguments: serde_json::json!({ "program": "cargo", "args": ["test"] }),
            },
        )),
    );
    update(
        &mut state,
        AppMsg::AgentEvent(AgentEvent::new(
            event_id,
            AgentEventKind::Output,
            AgentEventData::ToolOutput {
                name: "exec".to_owned(),
                content: serde_json::json!({
                    "success": true,
                    "stdout": "all tests passed",
                    "stderr": "",
                    "exit_code": 0
                }),
            },
        )),
    );

    let collapsed = render_to_buffer(&mut state, 80, 24);
    assert_buffer_contains(&collapsed, "▶ $ cargo test");
    assert_buffer_does_not_contain(&collapsed, "all tests passed");
    update(
        &mut state,
        test_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE)),
    );
    assert_eq!(state.input_text(), " ");
    let collapsed = render_to_buffer(&mut state, 80, 24);
    let (toggle_row, command_column) =
        find_fragment_position(&collapsed, "cargo").expect("command is rendered");

    update(
        &mut state,
        TestInput::Mouse {
            event: mouse_event(
                MouseEventKind::Down(MouseButton::Left),
                command_column,
                toggle_row,
            ),
            viewport: TuiViewport {
                width: 80,
                height: 24,
            },
        },
    );

    let expanded = render_to_buffer(&mut state, 80, 24);
    assert_buffer_contains(&expanded, "▼ $ cargo test");
    assert_buffer_contains(&expanded, "stdout");
    assert_buffer_contains(&expanded, "stderr");
    assert_buffer_contains(&expanded, "all tests passed");
    assert_buffer_contains(&expanded, "No standard error");
    let (expanded_row, _) =
        find_fragment_position(&expanded, "▼ $ cargo test").expect("expanded command is rendered");
    let (_, divider_column) =
        find_fragment_position(&expanded, "▶").expect("conversation divider is rendered");
    assert_row_has_background(
        &expanded,
        expanded_row,
        1,
        divider_column.saturating_sub(1),
        Color::Rgb(24, 38, 48),
    );
}

#[test]
fn tui_renders_long_tool_output_without_truncating_it() {
    let mut state = TestState::new("/workspace");
    let event_id = AgentEventId::tool(1, "exec", 1);
    update(
        &mut state,
        AppMsg::AgentEvent(AgentEvent::new(
            event_id.clone(),
            AgentEventKind::Started,
            AgentEventData::ToolStarted {
                name: "exec".to_owned(),
                call_id: Some("call-1".to_owned()),
                arguments: serde_json::json!({ "program": "cat", "args": ["large.txt"] }),
            },
        )),
    );
    update(
        &mut state,
        AppMsg::AgentEvent(AgentEvent::new(
            event_id,
            AgentEventKind::Output,
            AgentEventData::ToolOutput {
                name: "exec".to_owned(),
                content: serde_json::json!({
                    "success": true,
                    "exit_code": 0,
                    "stdout": format!("{}\nEND OF OUTPUT", "中".repeat(500)),
                    "stderr": ""
                }),
            },
        )),
    );
    let collapsed = render_to_buffer(&mut state, 120, 30);
    let (toggle_row, toggle_column) =
        find_fragment_position(&collapsed, "▶").expect("fold button is rendered");
    update(
        &mut state,
        TestInput::Mouse {
            event: mouse_event(
                MouseEventKind::Down(MouseButton::Left),
                toggle_column,
                toggle_row,
            ),
            viewport: TuiViewport {
                width: 120,
                height: 30,
            },
        },
    );

    let buffer = render_to_buffer(&mut state, 120, 30);
    assert_buffer_contains(&buffer, "END OF OUTPUT");
    assert_buffer_does_not_contain(&buffer, "[truncated]");
}

#[test]
fn tui_renders_failed_command_status_and_stderr() {
    let mut state = TestState::new("/workspace");
    let event_id = AgentEventId::tool(1, "exec", 1);
    update(
        &mut state,
        AppMsg::AgentEvent(AgentEvent::new(
            event_id.clone(),
            AgentEventKind::Started,
            AgentEventData::ToolStarted {
                name: "exec".to_owned(),
                call_id: Some("call-1".to_owned()),
                arguments: serde_json::json!({
                    "program": "cat",
                    "args": ["missing file.txt"]
                }),
            },
        )),
    );
    update(
        &mut state,
        AppMsg::AgentEvent(AgentEvent::new(
            event_id,
            AgentEventKind::Output,
            AgentEventData::ToolOutput {
                name: "exec".to_owned(),
                content: serde_json::json!({
                    "success": false,
                    "exit_code": 1,
                    "stdout": "",
                    "stderr": "cat: missing file.txt: No such file"
                }),
            },
        )),
    );

    let collapsed = render_to_buffer(&mut state, 100, 30);
    assert_buffer_contains(&collapsed, "'missing file.txt'");
    assert_buffer_does_not_contain(&collapsed, "Failed");
    assert_buffer_does_not_contain(&collapsed, "exit 1");
    assert_buffer_does_not_contain(&collapsed, "stderr 1 line(s)");
    assert_buffer_contains(&collapsed, "cat: missing file.txt: No such file");
    assert_buffer_fragment_has_fg_bg(&collapsed, "▶", Color::Red, Color::Rgb(20, 28, 36));
    let (toggle_row, toggle_column) =
        find_fragment_position(&collapsed, "▶").expect("fold button is rendered");

    update(
        &mut state,
        TestInput::Mouse {
            event: mouse_event(
                MouseEventKind::Down(MouseButton::Left),
                toggle_column,
                toggle_row,
            ),
            viewport: TuiViewport {
                width: 100,
                height: 30,
            },
        },
    );
    let expanded = render_to_buffer(&mut state, 100, 30);
    assert_buffer_contains(&expanded, "cat: missing file.txt: No such file");
}

#[test]
fn tui_displays_active_and_running_skills() {
    let mut state = TestState::new("/workspace");
    update(
        &mut state,
        AppMsg::AgentEvent(AgentEvent::new(
            AgentEventId::skills(),
            AgentEventKind::Output,
            AgentEventData::SkillsSelected {
                skills: vec!["review".to_owned()],
            },
        )),
    );
    let event_id = AgentEventId::tool(1, "call_skill", 1);
    update(
        &mut state,
        AppMsg::AgentEvent(AgentEvent::new(
            event_id.clone(),
            AgentEventKind::Started,
            AgentEventData::ToolStarted {
                name: "call_skill".to_owned(),
                call_id: Some("call-1".to_owned()),
                arguments: serde_json::json!({
                    "name": "review",
                    "task": "Review changes"
                }),
            },
        )),
    );
    update(
        &mut state,
        AppMsg::AgentEvent(AgentEvent::new(
            event_id,
            AgentEventKind::Completed,
            AgentEventData::ToolCompleted {
                name: "call_skill".to_owned(),
                call_id: Some("call-1".to_owned()),
            },
        )),
    );

    let buffer = render_to_buffer(&mut state, 80, 24);
    assert_buffer_contains(&buffer, "Active skills");
    assert_buffer_contains(&buffer, "review");
    assert_buffer_contains(&buffer, "Skill: review");
    assert_buffer_contains(&buffer, "Completed");
}

#[test]
fn tui_lists_skill_details_and_project_overrides() {
    let user_skill = skill_definition("review", SkillScope::User, "/home/user/.jux/skills/review");
    let project_skill = skill_definition(
        "review",
        SkillScope::Project,
        "/workspace/.jux/skills/review",
    );
    let mut state = TestState::new("/workspace");
    state.set_skill_catalog(SkillCatalog {
        skills: vec![project_skill.clone()],
        overrides: vec![SkillOverride {
            name: "review".to_owned(),
            overridden: user_skill,
            active: project_skill,
        }],
    });
    type_text(&mut state, "/skills");
    update(
        &mut state,
        test_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
    );

    let buffer = render_to_buffer(&mut state, 120, 30);
    assert_buffer_contains(&buffer, "Skills");
    assert_buffer_contains(&buffer, "review");
    assert_buffer_contains(&buffer, "Project");
    assert_buffer_contains(&buffer, "overrides User");
    assert_buffer_contains(&buffer, "Description for review");
    assert_buffer_contains(&buffer, "/workspace/.jux/skills/review/SKILL.md");
}

#[test]
fn tui_renders_the_skill_panel_with_the_sidebar_background() {
    let mut state = TestState::new("/workspace");
    state.set_skill_catalog(SkillCatalog {
        skills: vec![skill_definition(
            "review",
            SkillScope::Project,
            "/workspace/.jux/skills/review",
        )],
        overrides: Vec::new(),
    });
    type_text(&mut state, "/skills");
    update(
        &mut state,
        test_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
    );

    let buffer = render_to_buffer(&mut state, 120, 30);

    assert_buffer_fragment_has_background(&buffer, "Skills", sidebar_background());
}

#[test]
fn tui_selects_explicit_skills_for_the_next_run() {
    let mut state = TestState::new("/workspace");
    state.set_skill_catalog(SkillCatalog {
        skills: vec![skill_definition(
            "review",
            SkillScope::Project,
            "/workspace/.jux/skills/review",
        )],
        overrides: Vec::new(),
    });
    type_text(&mut state, "/skills");
    update(
        &mut state,
        test_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
    );
    update(
        &mut state,
        test_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE)),
    );
    update(
        &mut state,
        test_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
    );
    type_text(&mut state, "Review this change");

    let command = update(
        &mut state,
        test_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
    );

    assert_eq!(
        command,
        Some(AppCommand::StartRun {
            request: "Review this change".to_owned(),
        })
    );
    assert_eq!(state.selected_skill_names(), &["review"]);
}

#[test]
fn tui_quit_command_exits_without_starting_a_run() {
    let mut state = TestState::new("/workspace");
    type_text(&mut state, "/quit");

    let command = update(
        &mut state,
        test_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
    );

    assert_eq!(command, None);
    assert!(state.should_quit);
}

#[test]
fn tui_clear_command_removes_messages_without_starting_a_run() {
    let mut state = TestState::new("/workspace");
    update(
        &mut state,
        AppMsg::AssistantMessage {
            content: "Temporary output".to_owned(),
        },
    );
    type_text(&mut state, "/clear");

    let command = update(
        &mut state,
        test_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
    );

    assert_eq!(command, None);
    assert!(state.messages().is_empty());
    assert_eq!(state.input_text(), "");
}

#[test]
fn tui_help_command_displays_commands_and_shortcuts() {
    let mut state = TestState::new("/workspace");
    type_text(&mut state, "/help");

    let command = update(
        &mut state,
        test_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
    );

    assert_eq!(command, None);
    let buffer = render_to_buffer(&mut state, 80, 24);
    assert_buffer_contains(&buffer, "/clear");
    assert_buffer_contains(&buffer, "/quit");
    assert_buffer_contains(&buffer, "/new");
    assert_buffer_contains(&buffer, "/version");
    assert_buffer_contains(&buffer, "PageUp");
    assert_buffer_contains(&buffer, "Shift+Enter");
}

#[test]
fn tui_shows_filters_and_dismisses_slash_command_suggestions() {
    let mut state = TestState::new("/workspace");

    type_text(&mut state, "/");
    let initial = render_to_buffer(&mut state, 80, 24);
    assert_buffer_contains(&initial, "/new");
    assert_buffer_contains(&initial, "Start a new session");
    assert_buffer_contains(&initial, "/version");
    assert_eq!(find_fragment_position(&initial, "/new"), Some((16, 1)));
    assert_buffer_fragment_has_fg_bg(&initial, "/new", Color::Black, Color::Cyan);
    let (selected_row, selected_start) =
        find_fragment_position(&initial, "/new").expect("selected command is rendered");
    let (_, divider_column) = find_fragment_position(&initial, "▶").expect("divider is rendered");
    assert_row_has_background(
        &initial,
        selected_row,
        selected_start,
        divider_column.saturating_sub(2),
        Color::Cyan,
    );

    type_text(&mut state, "ver");
    let filtered = render_to_buffer(&mut state, 80, 24);
    assert_buffer_does_not_contain(&filtered, "Start a new session");
    assert_buffer_contains(&filtered, "Show the Jux version");

    update(
        &mut state,
        test_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
    );
    let dismissed = render_to_buffer(&mut state, 80, 24);
    assert_buffer_does_not_contain(&dismissed, "Show the Jux version");
    assert_buffer_does_not_contain(&dismissed, "Press Esc again");
    assert_eq!(state.input_text(), "/ver");
}

#[test]
fn tui_selects_and_executes_the_version_slash_command() {
    let mut state = TestState::new("/workspace");
    type_text(&mut state, "/");

    update(
        &mut state,
        test_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE)),
    );
    update(
        &mut state,
        test_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE)),
    );
    let selected = render_to_buffer(&mut state, 80, 24);
    assert_eq!(find_fragment_position(&selected, "/version"), Some((18, 1)));
    assert_buffer_fragment_has_fg_bg(&selected, "/version", Color::Black, Color::Cyan);
    let command = update(
        &mut state,
        test_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
    );

    assert_eq!(command, None);
    assert_eq!(state.input_text(), "");
    assert_eq!(
        state.messages().last(),
        Some(&Message {
            role: MessageRole::Assistant,
            content: format!("Jux {}", jux_core::version()),
        })
    );
}

#[test]
fn tui_new_slash_command_defers_creating_an_unnamed_session_until_submission() {
    let workspace = assert_fs::TempDir::new().expect("temp workspace exists");
    let store = SqliteWorkspaceStore::new(workspace.path());
    let original_session = store
        .init_workspace()
        .expect("workspace initializes")
        .active_session_id;
    let mut state = TestState::new(workspace.path());
    state
        .app
        .load_active_session_history(&store)
        .expect("history loads");
    type_text(&mut state, "/new");

    let command = update(
        &mut state,
        test_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
    );
    assert_eq!(command, None);
    assert!(state.pending_new_session());
    assert_eq!(
        store
            .load_active_session()
            .expect("active session loads")
            .id,
        original_session
    );

    type_text(&mut state, "new request");
    let command = update(
        &mut state,
        test_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
    );
    assert_eq!(
        command,
        Some(AppCommand::StartRun {
            request: "new request".to_owned()
        })
    );
    state
        .app
        .materialize_pending_new_session(&store, "new request")
        .expect("new session materializes");

    let active_session = store.load_active_session().expect("active session loads");
    assert_ne!(active_session.id, original_session);
    assert_eq!(active_session.name, None);
}

#[test]
fn tui_keeps_the_input_visible_in_a_small_terminal() {
    let mut state = TestState::new("/workspace");
    type_text(&mut state, "compact request");

    let buffer = render_to_buffer(&mut state, 30, 8);

    assert_buffer_contains(&buffer, "compact request");
}

#[test]
fn tui_does_not_start_a_second_run_while_running() {
    let mut state = TestState::new("/workspace");
    type_text(&mut state, "first request");
    let first = update(
        &mut state,
        test_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
    );
    type_text(&mut state, "second request");

    let second = update(
        &mut state,
        test_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
    );

    assert_eq!(
        first,
        Some(AppCommand::StartRun {
            request: "first request".to_owned(),
        })
    );
    assert_eq!(state.run_status(), TuiRunStatus::Running);
    assert_eq!(second, None);
    assert_eq!(state.input_text(), "second request");
    assert_eq!(state.messages().len(), 1);
    let buffer = render_to_buffer(&mut state, 80, 24);
    assert_buffer_contains(&buffer, "Running");
}

#[test]
fn tui_cancels_the_running_run_with_escape() {
    let mut state = TestState::new("/workspace");
    type_text(&mut state, "long-running request");
    update(
        &mut state,
        test_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
    );

    let first_command = update(
        &mut state,
        test_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
    );
    assert_eq!(first_command, None);
    assert_eq!(state.run_status(), TuiRunStatus::Running);
    assert_buffer_contains(
        &render_to_buffer(&mut state, 80, 24),
        "Press Esc again to interrupt the current run",
    );

    let command = update(
        &mut state,
        test_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
    );
    update(&mut state, AppMsg::RunCanceled);

    assert_eq!(command, Some(AppCommand::CancelRun));
    assert_eq!(state.run_status(), TuiRunStatus::Canceled);
    let buffer = render_to_buffer(&mut state, 80, 24);
    assert_buffer_contains(&buffer, "Canceled");
}

#[test]
fn tui_clears_input_with_double_escape_while_not_running() {
    let mut state = TestState::new("/workspace");
    type_text(&mut state, "draft request");

    let first_command = update(
        &mut state,
        test_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
    );

    assert_eq!(first_command, None);
    assert_eq!(state.input_text(), "draft request");
    assert_buffer_contains(
        &render_to_buffer(&mut state, 80, 24),
        "Press Esc again to clear the input",
    );

    let second_command = update(
        &mut state,
        test_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
    );

    assert_eq!(second_command, None);
    assert_eq!(state.input_text(), "");
    assert_buffer_does_not_contain(&render_to_buffer(&mut state, 80, 24), "Press Esc again");
}

#[test]
fn tui_displays_waiting_run_status_and_metadata() {
    let mut state = TestState::new("/workspace");

    update(
        &mut state,
        AppMsg::RunFinished {
            response: RunResponse {
                session_id: "workspace-0001".to_owned(),
                run_id: "workspace-0001-000001".to_owned(),
                status: CoreRunStatus::WaitingForHumanInput,
                created_at: 1_000,
                updated_at: 1_250,
                answer: None,
                steps: Vec::new(),
            },
        },
    );

    assert_eq!(state.run_status(), TuiRunStatus::WaitingForHumanInput);
    let buffer = render_to_buffer(&mut state, 80, 24);
    assert_buffer_contains(&buffer, "Waiting · 250ms");
}

#[test]
fn tui_displays_the_pending_human_input_question() {
    let mut state = TestState::new("/workspace");
    let run_id = RunId::from("workspace-0001-000001".to_owned());
    let steps = vec![Step::new(
        StepId::new(&run_id, 1),
        StepKind::AssistantResponse,
        StepPayload::AssistantResponse {
            message_id: None,
            usage: LlmUsage::default(),
            items: vec![AssistantResponseItem::ToolCall {
                id: "human-1".to_owned(),
                call_id: Some("call-1".to_owned()),
                name: "human_input".to_owned(),
                arguments: serde_json::json!({
                    "prompt": "Which implementation should Jux use?",
                    "options": [
                        { "id": "safe", "label": "Safer implementation" },
                        { "id": "fast", "label": "Faster implementation" }
                    ],
                    "allow_free_text": false
                }),
            }],
        },
    )];

    update(
        &mut state,
        AppMsg::RunFinished {
            response: RunResponse {
                session_id: "workspace-0001".to_owned(),
                run_id: run_id.to_string(),
                status: CoreRunStatus::WaitingForHumanInput,
                created_at: 1_000,
                updated_at: 1_250,
                answer: None,
                steps,
            },
        },
    );

    let buffer = render_to_buffer(&mut state, 80, 24);
    assert_buffer_contains(&buffer, "Which implementation should Jux use?");
    assert_buffer_contains(&buffer, "safe  Safer implementation");
    assert_buffer_contains(&buffer, "fast  Faster implementation");
    assert_buffer_fragment_has_background(
        &buffer,
        "Which implementation should Jux use?",
        conversation_background(),
    );
}

#[test]
fn tui_loads_active_session_history_from_the_workspace_store() {
    let workspace = assert_fs::TempDir::new().expect("temp workspace exists");
    let store = SqliteWorkspaceStore::new(workspace.path());
    let run = store
        .create_run("Historical request".to_owned())
        .expect("run is created");
    store
        .append_step(
            &run.id,
            StepKind::UserMessage,
            StepPayload::UserMessage {
                content: "Historical request".to_owned(),
            },
        )
        .expect("user step is stored");
    store
        .append_step(
            &run.id,
            StepKind::AssistantResponse,
            StepPayload::AssistantResponse {
                message_id: None,
                usage: LlmUsage::default(),
                items: vec![AssistantResponseItem::Text {
                    content: "Historical answer".to_owned(),
                }],
            },
        )
        .expect("assistant step is stored");
    store
        .update_run_status(&run.id, CoreRunStatus::Completed)
        .expect("run is completed");
    let mut state = TestState::new(workspace.path());

    state
        .app
        .load_active_session_history(&store)
        .expect("history loads");

    assert_eq!(state.run_status(), TuiRunStatus::Completed);
    let buffer = render_to_buffer(&mut state, 80, 24);
    assert_buffer_contains(&buffer, "Historical request");
    assert_buffer_contains(&buffer, "Historical answer");
    assert_buffer_contains(&buffer, "1 runs");
}

#[test]
fn tui_restores_persisted_command_output_when_loading_a_session() {
    let workspace = assert_fs::TempDir::new().expect("temp workspace exists");
    let store = SqliteWorkspaceStore::new(workspace.path());
    let run = store
        .create_run("Inspect files".to_owned())
        .expect("run is created");
    store
        .append_step(
            &run.id,
            StepKind::AssistantResponse,
            StepPayload::AssistantResponse {
                message_id: Some("message-1".to_owned()),
                usage: LlmUsage::default(),
                items: vec![AssistantResponseItem::ToolCall {
                    id: "tool-1".to_owned(),
                    call_id: Some("call-1".to_owned()),
                    name: "exec".to_owned(),
                    arguments: serde_json::json!({
                        "program": "ls",
                        "args": ["-la"]
                    }),
                }],
            },
        )
        .expect("tool call is persisted");
    store
        .append_step(
            &run.id,
            StepKind::ToolResult,
            StepPayload::ToolResult {
                id: "tool-1".to_owned(),
                call_id: Some("call-1".to_owned()),
                content: serde_json::json!({
                    "success": true,
                    "exit_code": 0,
                    "stdout": "README.md\nsrc",
                    "stderr": ""
                }),
            },
        )
        .expect("tool result is persisted");
    store
        .update_run_status(&run.id, CoreRunStatus::Completed)
        .expect("run completes");

    let mut restored = TestState::new(workspace.path());
    restored
        .app
        .load_active_session_history(&store)
        .expect("history loads");

    let command = restored.timeline()[0]
        .command
        .as_ref()
        .expect("command is restored");
    assert_eq!(command.program, "ls");
    assert_eq!(command.args, ["-la"]);
    assert_eq!(command.exit_code, Some(0));
    assert_eq!(command.stdout, "README.md\nsrc");
    let collapsed = render_to_buffer(&mut restored, 100, 30);
    assert_buffer_contains(&collapsed, "▶ $ ls -la");
    assert_buffer_does_not_contain(&collapsed, "stdout 2 line(s)");
    assert_buffer_does_not_contain(&collapsed, "README.md");
    let (toggle_row, toggle_column) =
        find_fragment_position(&collapsed, "▶").expect("fold button is rendered");

    update(
        &mut restored,
        TestInput::Mouse {
            event: mouse_event(
                MouseEventKind::Down(MouseButton::Left),
                toggle_column,
                toggle_row,
            ),
            viewport: TuiViewport {
                width: 100,
                height: 30,
            },
        },
    );
    let expanded = render_to_buffer(&mut restored, 100, 30);
    assert_buffer_contains(&expanded, "▼ $ ls -la");
    assert_buffer_contains(&expanded, "README.md");
}

#[test]
fn tui_restores_a_failed_run_and_its_error() {
    let workspace = assert_fs::TempDir::new().expect("temp workspace exists");
    let store = SqliteWorkspaceStore::new(workspace.path());
    let run = store
        .create_run("Failing request".to_owned())
        .expect("run is created");
    store
        .append_step(
            &run.id,
            StepKind::Error,
            StepPayload::Error {
                message: "Persisted failure".to_owned(),
            },
        )
        .expect("error step is stored");
    store
        .update_run_status(&run.id, CoreRunStatus::Failed)
        .expect("run is failed");
    let mut state = TestState::new(workspace.path());

    state
        .app
        .load_active_session_history(&store)
        .expect("history loads");

    assert_eq!(state.run_status(), TuiRunStatus::Failed);
    let buffer = render_to_buffer(&mut state, 80, 24);
    assert_buffer_contains(&buffer, "Persisted failure");
}

#[test]
fn tui_restores_a_waiting_run_and_can_resume_it() {
    let workspace = assert_fs::TempDir::new().expect("temp workspace exists");
    let store = SqliteWorkspaceStore::new(workspace.path());
    let run = store
        .create_run("Choose implementation".to_owned())
        .expect("run is created");
    store
        .append_step(
            &run.id,
            StepKind::AssistantResponse,
            StepPayload::AssistantResponse {
                message_id: None,
                usage: LlmUsage::default(),
                items: vec![AssistantResponseItem::ToolCall {
                    id: "human-1".to_owned(),
                    call_id: Some("call-1".to_owned()),
                    name: "human_input".to_owned(),
                    arguments: serde_json::json!({
                        "prompt": "Choose an implementation",
                        "options": [
                            { "id": "safe", "label": "Safer implementation" }
                        ],
                        "allow_free_text": false
                    }),
                }],
            },
        )
        .expect("human input step is stored");
    store
        .update_run_status(&run.id, CoreRunStatus::WaitingForHumanInput)
        .expect("run waits for input");
    let mut state = TestState::new(workspace.path());

    state
        .app
        .load_active_session_history(&store)
        .expect("history loads");
    let command = update(
        &mut state,
        test_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
    );

    assert_eq!(
        command,
        Some(AppCommand::StartRun {
            request: "safe".to_owned(),
        })
    );
    assert_eq!(state.run_id(), Some(run.id.as_str()));
    assert_eq!(state.run_status(), TuiRunStatus::Running);
}

#[test]
fn tui_lists_workspace_sessions_and_marks_the_active_session() {
    let workspace = assert_fs::TempDir::new().expect("temp workspace exists");
    let store = SqliteWorkspaceStore::new(workspace.path());
    store.init_workspace().expect("workspace initializes");
    store
        .create_session(Some("feature-a".to_owned()))
        .expect("session is created");
    let mut state = TestState::new(workspace.path());
    state
        .app
        .load_active_session_history(&store)
        .expect("history loads");
    type_text(&mut state, "/sessions");

    let command = update(
        &mut state,
        test_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
    );

    assert_eq!(command, None);
    let buffer = render_to_buffer(&mut state, 80, 24);
    assert_buffer_contains(&buffer, "Search:");
    assert_buffer_contains(&buffer, "default");
    assert_buffer_contains(&buffer, "feature-a");
}

#[test]
fn tui_creates_renames_and_switches_sessions() {
    let workspace = assert_fs::TempDir::new().expect("temp workspace exists");
    let store = SqliteWorkspaceStore::new(workspace.path());
    let default_session = store
        .init_workspace()
        .expect("workspace initializes")
        .active_session_id;
    let mut state = TestState::new(workspace.path());
    state
        .app
        .load_active_session_history(&store)
        .expect("history loads");

    type_text(&mut state, "/session new feature-a");
    let create = update(
        &mut state,
        test_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
    )
    .expect("create command is emitted");
    assert!(
        state
            .app
            .execute_session_command(&store, &create)
            .expect("session is created")
    );
    assert_eq!(
        store
            .load_active_session()
            .expect("active session loads")
            .name
            .as_deref(),
        Some("feature-a")
    );

    type_text(&mut state, "/session rename feature-renamed");
    let rename = update(
        &mut state,
        test_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
    )
    .expect("rename command is emitted");
    assert!(
        state
            .app
            .execute_session_command(&store, &rename)
            .expect("session is renamed")
    );
    assert_eq!(
        store
            .load_active_session()
            .expect("active session loads")
            .name
            .as_deref(),
        Some("feature-renamed")
    );

    type_text(&mut state, &format!("/session switch {default_session}"));
    let switch = update(
        &mut state,
        test_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
    )
    .expect("switch command is emitted");
    assert!(
        state
            .app
            .execute_session_command(&store, &switch)
            .expect("session is switched")
    );
    assert_eq!(state.session_id(), Some(default_session.as_str()));
}

#[test]
fn tui_session_slash_command_selects_and_switches_sessions() {
    let workspace = assert_fs::TempDir::new().expect("temp workspace exists");
    let store = SqliteWorkspaceStore::new(workspace.path());
    store.init_workspace().expect("workspace initializes");
    let target = store
        .create_session(Some("feature-a".to_owned()))
        .expect("session is created");
    let mut state = TestState::new(workspace.path());
    state
        .app
        .load_active_session_history(&store)
        .expect("history loads");

    type_text(&mut state, "/session");
    assert_eq!(
        update(
            &mut state,
            test_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        ),
        None
    );
    update(
        &mut state,
        test_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE)),
    );
    let command = update(
        &mut state,
        test_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
    )
    .expect("switch command is emitted");
    assert_eq!(
        command,
        AppCommand::SwitchSession {
            session_id: target.id.clone(),
        }
    );
    assert!(
        state
            .app
            .execute_session_command(&store, &command)
            .expect("session switches")
    );
    assert_eq!(
        store.load_active_session().expect("session loads").id,
        target.id
    );
    let switched = render_to_buffer(&mut state, 80, 24);
    assert_buffer_does_not_contain(&switched, "Search:");
    assert_buffer_contains(&switched, "feature-a");
}

#[test]
fn tui_session_picker_searches_likes_renames_and_closes() {
    let workspace = assert_fs::TempDir::new().expect("temp workspace exists");
    let store = SqliteWorkspaceStore::new(workspace.path());
    store.init_workspace().expect("workspace initializes");
    let target = store
        .create_session(Some("feature-a".to_owned()))
        .expect("session is created");
    let mut state = TestState::new(workspace.path());
    state
        .app
        .load_active_session_history(&store)
        .expect("history loads");
    type_text(&mut state, "/session");
    update(
        &mut state,
        test_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
    );
    type_text(&mut state, "feature");
    let filtered = render_to_buffer(&mut state, 80, 24);
    assert_buffer_contains(&filtered, "feature-a");

    let like = update(
        &mut state,
        test_key(KeyEvent::new(KeyCode::Char('l'), KeyModifiers::CONTROL)),
    )
    .expect("like command is emitted");
    assert!(
        state
            .app
            .execute_session_command(&store, &like)
            .expect("like toggles")
    );
    assert!(store.load_session(&target.id).expect("session loads").liked);

    update(
        &mut state,
        test_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL)),
    );
    for _ in 0.."feature-a".chars().count() {
        update(
            &mut state,
            test_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE)),
        );
    }
    type_text(&mut state, "renamed");
    let rename = update(
        &mut state,
        test_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
    )
    .expect("rename command is emitted");
    assert!(
        state
            .app
            .execute_session_command(&store, &rename)
            .expect("session renames")
    );
    assert_eq!(
        store
            .load_session(&target.id)
            .expect("session loads")
            .name
            .as_deref(),
        Some("renamed")
    );

    update(
        &mut state,
        test_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
    );
    let closed = render_to_buffer(&mut state, 80, 24);
    assert_buffer_does_not_contain(&closed, "Search:");
}

#[test]
fn tui_lists_run_history_under_each_session() {
    let workspace = assert_fs::TempDir::new().expect("temp workspace exists");
    let store = SqliteWorkspaceStore::new(workspace.path());
    store
        .create_run("Default session run".to_owned())
        .expect("default run is created");
    let feature_session = store
        .create_session(Some("feature-a".to_owned()))
        .expect("feature session is created");
    store
        .set_active_session(&feature_session.id)
        .expect("feature session is active");
    store
        .create_run("Feature session run".to_owned())
        .expect("feature run is created");
    let mut state = TestState::new(workspace.path());
    state
        .app
        .load_active_session_history(&store)
        .expect("history loads");
    type_text(&mut state, "/sessions");
    update(
        &mut state,
        test_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
    );

    let buffer = render_to_buffer(&mut state, 120, 30);
    assert_buffer_contains(&buffer, "default");
    assert_buffer_contains(&buffer, "feature-a");
    assert_buffer_does_not_contain(&buffer, "Default session run");
    assert_buffer_does_not_contain(&buffer, "Feature session run");
}

#[test]
fn tui_displays_plan_files_and_switchable_diffs() {
    let workspace = assert_fs::TempDir::new().expect("temp workspace exists");
    std::fs::create_dir_all(workspace.path().join("src")).expect("source directory exists");
    std::fs::write(workspace.path().join("src/a.rs"), "fn old_a() {}\n")
        .expect("first source file exists");
    std::fs::write(workspace.path().join("src/b.rs"), "fn old_b() {}\n")
        .expect("second source file exists");
    let proposal = CodeChangeProposal::prepare(
        workspace.path(),
        CodeChangePlan::new(
            "Rename both functions",
            vec!["Update a.rs".to_owned(), "Update b.rs".to_owned()],
        ),
        vec![
            ProposedFileContent::new("src/a.rs", "fn new_a() {}\n"),
            ProposedFileContent::new("src/b.rs", "fn new_b() {}\n"),
        ],
    )
    .expect("proposal is prepared");
    let mut state = TestState::new(workspace.path());

    update(
        &mut state,
        AppMsg::AgentEvent(AgentEvent::new(
            AgentEventId::tool(1, "propose_code_change", 1),
            AgentEventKind::Output,
            AgentEventData::ToolOutput {
                name: "propose_code_change".to_owned(),
                content: serde_json::to_value(proposal).expect("proposal serializes"),
            },
        )),
    );
    let first = render_to_buffer(&mut state, 120, 40);
    assert_buffer_contains(&first, "Plan: Rename both functions");
    assert_buffer_contains(&first, "src/a.rs");
    assert_buffer_contains(&first, "src/b.rs");
    assert_buffer_fragment_has_background(
        &first,
        "Plan: Rename both functions",
        conversation_background(),
    );
    assert_buffer_contains(&first, "Policy: Confirm");
    assert_buffer_contains(&first, "-fn old_a() {}");
    assert_buffer_contains(&first, "+fn new_a() {}");

    update(
        &mut state,
        test_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE)),
    );
    let second = render_to_buffer(&mut state, 120, 40);
    assert_buffer_fragment_has_background(
        &second,
        "Plan: Rename both functions",
        conversation_background(),
    );
    assert_buffer_contains(&second, "-fn old_b() {}");
    assert_buffer_contains(&second, "+fn new_b() {}");
}

#[test]
fn tui_accepts_and_applies_a_code_change() {
    let workspace = assert_fs::TempDir::new().expect("temp workspace exists");
    std::fs::write(workspace.path().join("README.md"), "old\n").expect("source file exists");
    let proposal = CodeChangeProposal::prepare(
        workspace.path(),
        CodeChangePlan::new("Update README", vec!["Replace content".to_owned()]),
        vec![ProposedFileContent::new("README.md", "new\n")],
    )
    .expect("proposal is prepared");
    let mut state = TestState::new(workspace.path());
    update(&mut state, AppMsg::CodeChangeProposed { proposal });
    type_text(&mut state, "/review accept");
    let command = update(
        &mut state,
        test_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
    )
    .expect("accept command is emitted");

    state
        .app
        .execute_code_change_command(&command)
        .expect("change is applied");

    assert_eq!(
        state.code_change_review().expect("review exists").status,
        ReviewStatus::Applied
    );
    assert_eq!(
        std::fs::read_to_string(workspace.path().join("README.md")).expect("source file loads"),
        "new\n"
    );
    type_text(&mut state, "/audit");
    update(
        &mut state,
        test_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
    );
    let buffer = render_to_buffer(&mut state, 120, 30);
    assert_buffer_contains(&buffer, "Applied 1 file");
    assert_buffer_contains(&buffer, "File write: README.md");
}

#[test]
fn tui_rejects_a_code_change_without_writing_files() {
    let workspace = assert_fs::TempDir::new().expect("temp workspace exists");
    std::fs::write(workspace.path().join("README.md"), "old\n").expect("source file exists");
    let proposal = CodeChangeProposal::prepare(
        workspace.path(),
        CodeChangePlan::new("Update README", vec!["Replace content".to_owned()]),
        vec![ProposedFileContent::new("README.md", "new\n")],
    )
    .expect("proposal is prepared");
    let mut state = TestState::new(workspace.path());
    update(&mut state, AppMsg::CodeChangeProposed { proposal });
    type_text(&mut state, "/review reject");
    let command = update(
        &mut state,
        test_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
    )
    .expect("reject command is emitted");

    state
        .app
        .execute_code_change_command(&command)
        .expect("change is rejected");

    assert_eq!(
        state.code_change_review().expect("review exists").status,
        ReviewStatus::Rejected
    );
    assert_eq!(
        std::fs::read_to_string(workspace.path().join("README.md")).expect("source file loads"),
        "old\n"
    );
}

#[test]
fn tui_requests_agent_adjustments_for_a_code_change() {
    let workspace = assert_fs::TempDir::new().expect("temp workspace exists");
    let proposal = CodeChangeProposal::prepare(
        workspace.path(),
        CodeChangePlan::new("Create README", vec!["Write content".to_owned()]),
        vec![ProposedFileContent::new("README.md", "content\n")],
    )
    .expect("proposal is prepared");
    let mut state = TestState::new(workspace.path());
    update(&mut state, AppMsg::CodeChangeProposed { proposal });
    type_text(&mut state, "/review changes Add an example");
    let command = update(
        &mut state,
        test_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
    )
    .expect("changes command is emitted");

    let handled = state
        .app
        .execute_code_change_command(&command)
        .expect("changes are requested");

    assert!(!handled);
    assert_eq!(
        command,
        AppCommand::RequestCodeChanges {
            feedback: "Add an example".to_owned(),
        }
    );
    assert_eq!(
        state.code_change_review().expect("review exists").status,
        ReviewStatus::ChangesRequested {
            feedback: "Add an example".to_owned(),
        }
    );
    assert_eq!(state.run_status(), TuiRunStatus::Running);
}

#[test]
fn tui_reports_patch_conflicts_without_overwriting_files() {
    let workspace = assert_fs::TempDir::new().expect("temp workspace exists");
    std::fs::write(workspace.path().join("README.md"), "original\n").expect("source file exists");
    let proposal = CodeChangeProposal::prepare(
        workspace.path(),
        CodeChangePlan::new("Update README", vec!["Replace content".to_owned()]),
        vec![ProposedFileContent::new("README.md", "proposed\n")],
    )
    .expect("proposal is prepared");
    let mut state = TestState::new(workspace.path());
    update(&mut state, AppMsg::CodeChangeProposed { proposal });
    std::fs::write(workspace.path().join("README.md"), "external\n").expect("source file changes");
    type_text(&mut state, "/review accept");
    let command = update(
        &mut state,
        test_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
    )
    .expect("accept command is emitted");

    state
        .app
        .execute_code_change_command(&command)
        .expect("conflict is displayed");

    assert_eq!(
        state.code_change_review().expect("review exists").status,
        ReviewStatus::Conflict {
            paths: vec!["README.md".to_owned()],
        }
    );
    let buffer = render_to_buffer(&mut state, 120, 30);
    assert_buffer_contains(&buffer, "Conflict");
    assert_buffer_contains(&buffer, "README.md");
    assert_eq!(
        std::fs::read_to_string(workspace.path().join("README.md")).expect("source file loads"),
        "external\n"
    );
}

#[test]
fn tui_displays_and_enforces_sensitive_path_policy_denial() {
    let workspace = assert_fs::TempDir::new().expect("temp workspace exists");
    std::fs::write(workspace.path().join(".env"), "TOKEN=old\n").expect("env file exists");
    let proposal = CodeChangeProposal::prepare(
        workspace.path(),
        CodeChangePlan::new("Update token", vec!["Modify .env".to_owned()]),
        vec![ProposedFileContent::new(".env", "TOKEN=new\n")],
    )
    .expect("proposal is prepared");
    let mut state = TestState::new(workspace.path());
    update(&mut state, AppMsg::CodeChangeProposed { proposal });
    let proposal_view = render_to_buffer(&mut state, 120, 30);
    assert_buffer_contains(&proposal_view, "Policy: Deny");
    assert_buffer_contains(&proposal_view, "Risk [High] .env");
    type_text(&mut state, "/review accept");
    let command = update(
        &mut state,
        test_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
    )
    .expect("accept command is emitted");

    state
        .app
        .execute_code_change_command(&command)
        .expect("denial is displayed");

    let denied = render_to_buffer(&mut state, 120, 30);
    assert_buffer_contains(&denied, "Denied by policy");
    assert_eq!(
        std::fs::read_to_string(workspace.path().join(".env")).expect("env file loads"),
        "TOKEN=old\n"
    );
}

#[test]
fn tui_displays_run_tool_file_policy_and_error_audit_events() {
    let workspace = assert_fs::TempDir::new().expect("temp workspace exists");
    std::fs::write(workspace.path().join("README.md"), "old\n").expect("source file exists");
    let proposal = CodeChangeProposal::prepare(
        workspace.path(),
        CodeChangePlan::new("Update README", vec!["Replace content".to_owned()]),
        vec![ProposedFileContent::new("README.md", "new\n")],
    )
    .expect("proposal is prepared");
    let run_id = RunId::from("workspace-0001-000001".to_owned());
    let steps = vec![
        Step::new(
            StepId::new(&run_id, 1),
            StepKind::UserMessage,
            StepPayload::UserMessage {
                content: "Inspect and update README".to_owned(),
            },
        ),
        Step::new(
            StepId::new(&run_id, 2),
            StepKind::AssistantResponse,
            StepPayload::AssistantResponse {
                message_id: None,
                usage: LlmUsage::default(),
                items: vec![
                    AssistantResponseItem::ToolCall {
                        id: "read-1".to_owned(),
                        call_id: Some("call-read".to_owned()),
                        name: "exec".to_owned(),
                        arguments: serde_json::json!({
                            "program": "cat",
                            "args": ["README.md"]
                        }),
                    },
                    AssistantResponseItem::ToolCall {
                        id: "proposal-1".to_owned(),
                        call_id: Some("call-proposal".to_owned()),
                        name: "propose_code_change".to_owned(),
                        arguments: serde_json::json!({}),
                    },
                ],
            },
        ),
        Step::new(
            StepId::new(&run_id, 3),
            StepKind::ToolResult,
            StepPayload::ToolResult {
                id: "read-1".to_owned(),
                call_id: Some("call-read".to_owned()),
                content: serde_json::json!({ "stdout": "old" }),
            },
        ),
        Step::new(
            StepId::new(&run_id, 4),
            StepKind::ToolResult,
            StepPayload::ToolResult {
                id: "proposal-1".to_owned(),
                call_id: Some("call-proposal".to_owned()),
                content: serde_json::to_value(proposal).expect("proposal serializes"),
            },
        ),
        Step::new(
            StepId::new(&run_id, 5),
            StepKind::Error,
            StepPayload::Error {
                message: "Example failure".to_owned(),
            },
        ),
    ];
    let mut state = TestState::new(workspace.path());
    update(
        &mut state,
        AppMsg::RunFinished {
            response: RunResponse {
                session_id: "workspace-0001".to_owned(),
                run_id: run_id.to_string(),
                status: CoreRunStatus::Failed,
                created_at: 1_000,
                updated_at: 1_500,
                answer: None,
                steps,
            },
        },
    );
    type_text(&mut state, "/audit");
    update(
        &mut state,
        test_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
    );

    let buffer = render_to_buffer(&mut state, 120, 40);
    assert_buffer_contains(&buffer, "Audit");
    assert_buffer_contains(&buffer, "User request");
    assert_buffer_contains(&buffer, "File read");
    assert_buffer_contains(&buffer, "Tool result");
    assert_buffer_contains(&buffer, "Policy: Confirm");
    assert_buffer_contains(&buffer, "File proposed: README.md");
    assert_buffer_contains(&buffer, "Error: Example failure");
}

#[test]
fn tui_submits_the_selected_human_input_option() {
    let mut state = waiting_human_input_state(false);

    update(
        &mut state,
        test_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE)),
    );
    let command = update(
        &mut state,
        test_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
    );

    assert_eq!(
        command,
        Some(AppCommand::StartRun {
            request: "fast".to_owned(),
        })
    );
    assert_eq!(state.run_status(), TuiRunStatus::Running);
}

#[test]
fn tui_submits_free_form_human_input_when_allowed() {
    let mut state = waiting_human_input_state(true);
    type_text(&mut state, "Use a hybrid implementation");

    let command = update(
        &mut state,
        test_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
    );

    assert_eq!(
        command,
        Some(AppCommand::StartRun {
            request: "Use a hybrid implementation".to_owned(),
        })
    );
}

#[test]
fn tui_rejects_human_input_that_does_not_match_an_option() {
    let mut state = waiting_human_input_state(false);
    type_text(&mut state, "invalid");

    let command = update(
        &mut state,
        test_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
    );

    assert_eq!(command, None);
    assert_eq!(state.run_status(), TuiRunStatus::WaitingForHumanInput);
    assert_eq!(state.input_text(), "invalid");
    let buffer = render_to_buffer(&mut state, 120, 24);
    assert_buffer_contains(&buffer, "must match one of the option ids");
    assert_buffer_contains(&buffer, "safe, fast");
}

#[test]
fn tui_distinguishes_operation_confirmation_from_clarification() {
    let mut state = waiting_human_input_state_with_kind(false, Some("confirmation"));

    let buffer = render_to_buffer(&mut state, 80, 24);

    assert_buffer_contains(&buffer, "Confirmation required");
    assert_buffer_does_not_contain(&buffer, "Input required");
}

#[test]
fn tui_displays_the_completed_run_answer() {
    let mut state = TestState::new("/workspace");
    let run_id = RunId::from("workspace-0001-000001".to_owned());
    let response_step = Step::new(
        StepId::new(&run_id, 1),
        StepKind::AssistantResponse,
        StepPayload::AssistantResponse {
            message_id: Some("message-1".to_owned()),
            usage: LlmUsage {
                input_tokens: 120,
                output_tokens: 30,
                total_tokens: 150,
                cached_input_tokens: 0,
                cache_creation_input_tokens: 0,
            },
            items: vec![AssistantResponseItem::Text {
                content: "The requested change is complete.".to_owned(),
            }],
        },
    );

    update(
        &mut state,
        AppMsg::RunFinished {
            response: RunResponse {
                session_id: "workspace-0001".to_owned(),
                run_id: "workspace-0001-000001".to_owned(),
                status: CoreRunStatus::Completed,
                created_at: 1_000,
                updated_at: 1_500,
                answer: Some("The requested change is complete.".to_owned()),
                steps: vec![response_step],
            },
        },
    );
    let command_event_id = AgentEventId::tool(1, "exec", 1);
    update(
        &mut state,
        AppMsg::AgentEvent(AgentEvent::new(
            command_event_id.clone(),
            AgentEventKind::Started,
            AgentEventData::ToolStarted {
                name: "exec".to_owned(),
                call_id: Some("call-1".to_owned()),
                arguments: serde_json::json!({ "program": "ls", "args": ["/etc"] }),
            },
        )),
    );
    update(
        &mut state,
        AppMsg::AgentEvent(AgentEvent::new(
            command_event_id,
            AgentEventKind::Output,
            AgentEventData::ToolOutput {
                name: "exec".to_owned(),
                content: serde_json::json!({
                    "success": true,
                    "exit_code": 0,
                    "stdout": "hosts",
                    "stderr": ""
                }),
            },
        )),
    );

    assert_eq!(state.run_status(), TuiRunStatus::Completed);
    let buffer = render_to_buffer(&mut state, 80, 24);
    assert_buffer_contains(&buffer, "Completed · 500ms");
    assert_buffer_contains(&buffer, "The requested change is complete.");
    assert_buffer_contains(&buffer, "150 tokens (120 in / 30 out) · 500 ms");
    let (command_row, _) =
        find_fragment_position(&buffer, "▶ $ ls /etc").expect("command is rendered");
    let (summary_row, _) = find_fragment_position(&buffer, "150 tokens")
        .expect("response summary is rendered after the command");
    assert!(command_row < summary_row);
    assert_eq!(
        find_fragment_position(&buffer, "The requested change is complete.")
            .map(|(_, column)| column),
        Some(4)
    );
}

#[test]
fn tui_displays_persisted_steps_in_id_order() {
    let mut state = TestState::new("/workspace");
    let run_id = RunId::from("workspace-0001-000001".to_owned());
    let steps = vec![
        Step::new(
            StepId::new(&run_id, 1),
            StepKind::UserMessage,
            StepPayload::UserMessage {
                content: "Inspect workspace".to_owned(),
            },
        ),
        Step::new(
            StepId::new(&run_id, 2),
            StepKind::ToolResult,
            StepPayload::ToolResult {
                id: "tool-1".to_owned(),
                call_id: Some("call-1".to_owned()),
                content: serde_json::json!({ "stdout": "done" }),
            },
        ),
    ];

    update(
        &mut state,
        AppMsg::RunFinished {
            response: RunResponse {
                session_id: "workspace-0001".to_owned(),
                run_id: run_id.to_string(),
                status: CoreRunStatus::Completed,
                created_at: 1_000,
                updated_at: 1_500,
                answer: None,
                steps,
            },
        },
    );

    assert_eq!(
        state.steps()[0].id.to_string(),
        "workspace-0001-000001-000001"
    );
    assert_eq!(
        state.steps()[1].id.to_string(),
        "workspace-0001-000001-000002"
    );
    let buffer = render_to_buffer(&mut state, 80, 24);
    assert_buffer_does_not_contain(&buffer, "Step  User message");
    assert_buffer_does_not_contain(&buffer, "Step  Tool result");
}

#[test]
fn tui_keeps_run_errors_in_the_message_timeline() {
    let mut state = TestState::new("/workspace");

    update(
        &mut state,
        AppMsg::RunFailed {
            error: "provider timed out".to_owned(),
        },
    );
    update(
        &mut state,
        AppMsg::AssistantMessage {
            content: "Later message".to_owned(),
        },
    );

    assert_eq!(state.run_status(), TuiRunStatus::Failed);
    let buffer = render_to_buffer(&mut state, 80, 24);
    assert_buffer_contains(&buffer, "Error");
    assert_buffer_contains(&buffer, "provider timed out");
    assert_buffer_contains(&buffer, "Later message");
}

#[test]
fn tui_shell_renders_workspace_and_idle_status() {
    let mut state = TestState::new("/workspace");

    let buffer = render_to_buffer(&mut state, 80, 24);

    assert_buffer_contains(&buffer, "Jux");
    assert_buffer_contains(&buffer, "Environment");
    assert_buffer_contains(&buffer, "Idle");
    assert_buffer_contains(&buffer, "← focus · Ctrl+C quit");
    assert_buffer_does_not_contain(&buffer, "keys");
    assert_buffer_does_not_contain(&buffer, "Session - | Run -");
    assert_buffer_does_not_contain(&buffer, "conversation");
    assert_buffer_does_not_contain(&buffer, "status");
    assert_buffer_has_no_panel_frames(&buffer);
    assert_buffer_fragment_has_background(&buffer, "Environment", sidebar_background());
    assert_eq!(find_fragment_position(&buffer, "▶"), Some((12, 48)));
    assert_input_block_has_background(&buffer, "> Start typing", input_active_background());
}

#[test]
fn tui_selects_the_status_panel_with_right_arrow() {
    let mut state = TestState::new("/workspace");

    update(
        &mut state,
        test_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE)),
    );

    assert_eq!(state.focused_panel(), FocusedPanel::Sidebar);
    let buffer = render_to_buffer(&mut state, 80, 24);
    assert_buffer_fragment_has_background(&buffer, "Environment", sidebar_background());
    assert_input_block_does_not_have_background(
        &buffer,
        "> Start typing",
        input_active_background(),
    );
}

#[test]
fn tui_returns_focus_to_the_conversation_with_left_arrow() {
    let mut state = TestState::new("/workspace");
    update(
        &mut state,
        test_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE)),
    );
    update(
        &mut state,
        test_key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE)),
    );

    assert_eq!(state.focused_panel(), FocusedPanel::Conversation);
    let buffer = render_to_buffer(&mut state, 80, 24);
    assert_buffer_fragment_has_background(&buffer, "Environment", sidebar_background());
    assert_input_block_has_background(&buffer, "> Start typing", input_active_background());
}

#[test]
fn tui_selects_and_copies_text_inside_the_conversation_panel_only() {
    let mut state = TestState::new("/workspace");
    update(
        &mut state,
        AppMsg::AssistantMessage {
            content: "Selectable message".to_owned(),
        },
    );
    let viewport = TuiViewport {
        width: 100,
        height: 24,
    };

    update(
        &mut state,
        TestInput::Mouse {
            event: mouse_event(MouseEventKind::Down(MouseButton::Left), 1, 2),
            viewport,
        },
    );
    update(
        &mut state,
        TestInput::Mouse {
            event: mouse_event(MouseEventKind::Drag(MouseButton::Left), 90, 2),
            viewport,
        },
    );
    let command = update(
        &mut state,
        TestInput::Mouse {
            event: mouse_event(MouseEventKind::Up(MouseButton::Left), 90, 2),
            viewport,
        },
    );

    assert_eq!(
        command,
        Some(AppCommand::CopyText {
            content: "\u{00a0}\u{00a0}\u{00a0}Selectable message\u{00a0}\u{00a0}\u{00a0}"
                .to_owned(),
        })
    );
    assert_eq!(
        state.text_selection().map(|selection| selection.panel),
        Some(SelectionPanel::Conversation)
    );
    let buffer = render_to_buffer(&mut state, 100, 24);
    assert_buffer_fragment_has_fg_bg(&buffer, "Selectable message", Color::Black, Color::Yellow);
    let completed_selection = state.text_selection();
    let command = update(
        &mut state,
        TestInput::Mouse {
            event: mouse_event(MouseEventKind::Drag(MouseButton::Left), 10, 3),
            viewport,
        },
    );
    assert_eq!(command, None);
    assert_eq!(state.text_selection(), completed_selection);
}

#[test]
fn tui_selects_and_copies_text_inside_the_status_panel() {
    let mut state = TestState::new("/workspace");
    let viewport = TuiViewport {
        width: 100,
        height: 24,
    };

    update(
        &mut state,
        TestInput::Mouse {
            event: mouse_event(MouseEventKind::Down(MouseButton::Left), 63, 4),
            viewport,
        },
    );
    update(
        &mut state,
        TestInput::Mouse {
            event: mouse_event(MouseEventKind::Drag(MouseButton::Left), 70, 4),
            viewport,
        },
    );
    let command = update(
        &mut state,
        TestInput::Mouse {
            event: mouse_event(MouseEventKind::Up(MouseButton::Left), 70, 4),
            viewport,
        },
    );

    assert_eq!(
        command,
        Some(AppCommand::CopyText {
            content: "Session".to_owned(),
        })
    );
    assert_eq!(
        state.text_selection().map(|selection| selection.panel),
        Some(SelectionPanel::Sidebar)
    );
    let buffer = render_to_buffer(&mut state, 100, 24);
    assert_buffer_fragment_has_fg_bg(&buffer, "Session", Color::Black, Color::Yellow);
}

#[test]
fn tui_drags_the_divider_to_resize_both_panels() {
    let mut state = TestState::new("/workspace");
    let viewport = TuiViewport {
        width: 100,
        height: 24,
    };

    update(
        &mut state,
        TestInput::Mouse {
            event: mouse_event(MouseEventKind::Down(MouseButton::Left), 60, 2),
            viewport,
        },
    );
    update(
        &mut state,
        TestInput::Mouse {
            event: mouse_event(MouseEventKind::Drag(MouseButton::Left), 70, 2),
            viewport,
        },
    );
    update(
        &mut state,
        TestInput::Mouse {
            event: mouse_event(MouseEventKind::Up(MouseButton::Left), 70, 2),
            viewport,
        },
    );

    let buffer = render_to_buffer(&mut state, 100, 24);
    assert_eq!(find_fragment_position(&buffer, "▶"), Some((12, 70)));
}

#[test]
fn tui_toggles_the_sidebar_with_the_divider_arrow() {
    let mut state = TestState::new("/workspace");
    let viewport = TuiViewport {
        width: 100,
        height: 24,
    };

    update(
        &mut state,
        TestInput::Mouse {
            event: mouse_event(MouseEventKind::Down(MouseButton::Left), 60, 12),
            viewport,
        },
    );

    assert!(!state.sidebar_visible());
    let hidden = render_to_buffer(&mut state, 100, 24);
    assert_buffer_does_not_contain(&hidden, "Environment");
    assert_eq!(find_fragment_position(&hidden, "◀"), Some((12, 99)));

    update(
        &mut state,
        TestInput::Mouse {
            event: mouse_event(MouseEventKind::Down(MouseButton::Left), 99, 12),
            viewport,
        },
    );

    assert!(state.sidebar_visible());
    let visible = render_to_buffer(&mut state, 100, 24);
    assert_buffer_contains(&visible, "Environment");
    assert_eq!(find_fragment_position(&visible, "▶"), Some((12, 60)));
}

#[test]
fn tui_click_focus_does_not_start_text_selection() {
    let mut state = TestState::new("/workspace");
    let viewport = TuiViewport {
        width: 100,
        height: 24,
    };

    update(
        &mut state,
        TestInput::Mouse {
            event: mouse_event(MouseEventKind::Down(MouseButton::Left), 1, 1),
            viewport,
        },
    );
    let command = update(
        &mut state,
        TestInput::Mouse {
            event: mouse_event(MouseEventKind::Up(MouseButton::Left), 1, 1),
            viewport,
        },
    );

    assert_eq!(command, None);
    assert_eq!(state.text_selection(), None);
}

#[test]
fn tui_displays_workspace_model_sandbox_and_summary_status() {
    let mut state = TestState::new("/workspace");
    state.set_runtime_info(TuiRuntimeInfo {
        workspace_id: Some("workspace-1234".to_owned()),
        model_provider: "deepseek".to_owned(),
        model_name: "deepseek-chat".to_owned(),
        sandbox: TuiSandboxSummary {
            filesystem: "read-only".to_owned(),
            network: "deny by default".to_owned(),
            native_commands: "disabled".to_owned(),
        },
        config_error: None,
        ..TuiRuntimeInfo::default()
    });

    let buffer = render_to_buffer(&mut state, 120, 30);

    assert_buffer_contains(&buffer, "deepseek/deepseek-chat");
    assert_buffer_contains(&buffer, "FS  read-only");
    assert_buffer_contains(&buffer, "Net deny by default");
    assert_buffer_contains(&buffer, "Cmd disabled");
    assert_buffer_contains(&buffer, "No session");
    assert_buffer_contains(&buffer, "Idle");
    assert_buffer_does_not_contain(&buffer, "Session - | Run -");
}

#[test]
fn tui_displays_configuration_errors_and_runtime_logs() {
    let mut state = TestState::new("/workspace");
    state.set_runtime_info(TuiRuntimeInfo {
        workspace_id: Some("workspace-1234".to_owned()),
        model_provider: "deepseek".to_owned(),
        model_name: "deepseek-chat".to_owned(),
        sandbox: TuiSandboxSummary {
            filesystem: "read-only".to_owned(),
            network: "deny by default".to_owned(),
            native_commands: "disabled".to_owned(),
        },
        config_error: Some("invalid config shape: unknown field".to_owned()),
        ..TuiRuntimeInfo::default()
    });
    update(
        &mut state,
        AppMsg::RunFailed {
            error: "model unavailable".to_owned(),
        },
    );
    type_text(&mut state, "/logs");
    update(
        &mut state,
        test_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
    );

    let buffer = render_to_buffer(&mut state, 120, 30);

    assert_buffer_contains(&buffer, "Configuration error");
    assert_buffer_contains(&buffer, "invalid config shape: unknown field");
    assert_buffer_contains(&buffer, "Run failed");
    assert_buffer_contains(&buffer, "model unavailable");
}

fn render_to_buffer(state: &mut TestState, width: u16, height: u16) -> Buffer {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("test terminal is created");
    terminal
        .draw(|frame| state.app.render(frame))
        .expect("app renders");
    terminal.backend().buffer().clone()
}

fn render_cursor_position(state: &mut TestState, width: u16, height: u16) -> Position {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("test terminal is created");
    terminal
        .draw(|frame| state.app.render(frame))
        .expect("app renders");
    terminal
        .get_cursor_position()
        .expect("cursor position is available")
}

fn type_text(state: &mut TestState, text: &str) {
    for character in text.chars() {
        update(
            state,
            test_key(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE)),
        );
    }
}

fn wait_for_file(service: &FileIndexService, expected: &str) -> FileIndexSnapshot {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if let Some(snapshot) = service.recv_timeout(Duration::from_millis(500))
            && snapshot.files.iter().any(|path| path == expected)
        {
            return snapshot;
        }
    }
    panic!("file index did not include {expected}");
}

fn mouse_event(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
    MouseEvent {
        kind,
        column,
        row,
        modifiers: KeyModifiers::NONE,
    }
}

fn skill_definition(name: &str, scope: SkillScope, directory: &str) -> SkillDefinition {
    SkillDefinition {
        name: name.to_owned(),
        description: format!("Description for {name}"),
        content: format!("# {name}"),
        scope,
        path: PathBuf::from(directory).join("SKILL.md"),
    }
}

fn waiting_human_input_state(allow_free_text: bool) -> TestState {
    waiting_human_input_state_with_kind(allow_free_text, None)
}

fn waiting_human_input_state_with_kind(allow_free_text: bool, kind: Option<&str>) -> TestState {
    let mut state = TestState::new("/workspace");
    let run_id = RunId::from("workspace-0001-000001".to_owned());
    let mut arguments = serde_json::json!({
        "prompt": "Which implementation should Jux use?",
        "options": [
            { "id": "safe", "label": "Safer implementation" },
            { "id": "fast", "label": "Faster implementation" }
        ],
        "allow_free_text": allow_free_text
    });
    if let Some(kind) = kind {
        arguments["kind"] = serde_json::Value::String(kind.to_owned());
    }
    let step = Step::new(
        StepId::new(&run_id, 1),
        StepKind::AssistantResponse,
        StepPayload::AssistantResponse {
            message_id: None,
            usage: LlmUsage::default(),
            items: vec![AssistantResponseItem::ToolCall {
                id: "human-1".to_owned(),
                call_id: Some("call-1".to_owned()),
                name: "human_input".to_owned(),
                arguments,
            }],
        },
    );
    update(
        &mut state,
        AppMsg::RunFinished {
            response: RunResponse {
                session_id: "workspace-0001".to_owned(),
                run_id: run_id.to_string(),
                status: CoreRunStatus::WaitingForHumanInput,
                created_at: 1_000,
                updated_at: 1_250,
                answer: None,
                steps: vec![step],
            },
        },
    );
    state
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

fn assert_buffer_does_not_contain(buffer: &Buffer, unexpected: &str) {
    let content = buffer
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect::<String>();
    assert!(
        !content.contains(unexpected),
        "buffer unexpectedly contains {unexpected:?}:\n{content}"
    );
}

fn assert_buffer_has_no_panel_frames(buffer: &Buffer) {
    let area = *buffer.area();
    let has_border = (area.y..area.y + area.height)
        .flat_map(|y| (area.x..area.x + area.width).map(move |x| (x, y)))
        .filter_map(|position| buffer.cell(position))
        .any(|cell| matches!(cell.symbol(), "─" | "┌" | "┐" | "└" | "┘"));
    assert!(!has_border, "buffer unexpectedly contains panel frames");
}

fn assert_buffer_fragment_has_background(buffer: &Buffer, expected: &str, background: Color) {
    let (y, x) = find_fragment_position(buffer, expected)
        .unwrap_or_else(|| panic!("buffer does not contain fragment {expected:?}"));
    let end = x + expected.chars().count() as u16;
    assert!(
        (x..end).all(|column| buffer
            .cell((column, y))
            .is_some_and(|cell| cell.bg == background)),
        "fragment {expected:?} does not have expected background"
    );
}

fn assert_buffer_fragment_has_foreground(buffer: &Buffer, expected: &str, foreground: Color) {
    let (y, x) = find_fragment_position(buffer, expected)
        .unwrap_or_else(|| panic!("buffer does not contain fragment {expected:?}"));
    let end = x + expected.chars().count() as u16;
    assert!(
        (x..end).all(|column| buffer
            .cell((column, y))
            .is_some_and(|cell| cell.fg == foreground)),
        "fragment {expected:?} does not have expected foreground"
    );
}

fn assert_buffer_fragments_have_different_foregrounds(buffer: &Buffer, first: &str, second: &str) {
    let (first_y, first_x) = find_fragment_position(buffer, first)
        .unwrap_or_else(|| panic!("buffer does not contain fragment {first:?}"));
    let (second_y, second_x) = find_fragment_position(buffer, second)
        .unwrap_or_else(|| panic!("buffer does not contain fragment {second:?}"));
    let first_foreground = buffer
        .cell((first_x, first_y))
        .expect("first cell exists")
        .fg;
    let second_foreground = buffer
        .cell((second_x, second_y))
        .expect("second cell exists")
        .fg;
    assert_ne!(
        first_foreground, second_foreground,
        "fragments {first:?} and {second:?} have the same foreground"
    );
}

fn assert_row_has_background(buffer: &Buffer, y: u16, start: u16, end: u16, background: Color) {
    assert!(
        (start..end).all(|x| buffer
            .cell((x, y))
            .is_some_and(|cell| cell.bg == background)),
        "row {y} does not have the expected background from {start} to {end}"
    );
}

fn scrollbar_thumb_start(buffer: &Buffer, column: u16) -> u16 {
    (0..buffer.area().height)
        .find(|row| {
            buffer
                .cell((column, *row))
                .is_some_and(|cell| cell.symbol() == "█")
        })
        .expect("conversation scrollbar thumb is rendered")
}

fn assert_buffer_fragment_has_fg_bg(buffer: &Buffer, expected: &str, fg: Color, bg: Color) {
    let mut found = false;
    let area = *buffer.area();
    for y in area.y..area.y + area.height {
        let row_cells = (area.x..area.x + area.width)
            .filter_map(|x| buffer.cell((x, y)))
            .collect::<Vec<_>>();
        let row = row_cells
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        let Some(start_byte) = row.find(expected) else {
            continue;
        };
        found = true;
        let start = row[..start_byte].chars().count();
        let end = start + expected.chars().count();
        if row_cells[start..end]
            .iter()
            .all(|cell| cell.fg == fg && cell.bg == bg)
        {
            return;
        }
    }
    assert!(found, "buffer does not contain fragment {expected:?}");
    panic!("buffer does not contain styled fragment {expected:?}");
}

fn assert_input_block_has_background(buffer: &Buffer, expected: &str, background: Color) {
    let (y, x) = find_fragment_position(buffer, expected)
        .unwrap_or_else(|| panic!("buffer does not contain fragment {expected:?}"));
    for row_y in y.saturating_sub(1)..=y.saturating_add(1) {
        assert_input_row_has_background(buffer, row_y, x, background);
    }
}

fn assert_input_block_does_not_have_background(buffer: &Buffer, expected: &str, background: Color) {
    let (y, x) = find_fragment_position(buffer, expected)
        .unwrap_or_else(|| panic!("buffer does not contain fragment {expected:?}"));
    for row_y in y.saturating_sub(1)..=y.saturating_add(1) {
        assert_input_row_does_not_have_background(buffer, row_y, x, background);
    }
}

fn find_fragment_position(buffer: &Buffer, expected: &str) -> Option<(u16, u16)> {
    let area = *buffer.area();
    for y in area.y..area.y + area.height {
        let row_cells = (area.x..area.x + area.width)
            .filter_map(|x| buffer.cell((x, y)))
            .collect::<Vec<_>>();
        let row = row_cells
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        if let Some(x) = row.find(expected) {
            return Some((y, area.x + row[..x].chars().count() as u16));
        }
    }
    None
}

fn assert_input_row_has_background(buffer: &Buffer, y: u16, x: u16, background: Color) {
    let row_cells = input_row_cells(buffer, y, x);
    assert!(
        row_cells.iter().all(|cell| cell.bg == background),
        "row {y} does not have expected input background"
    );
}

fn assert_input_row_does_not_have_background(buffer: &Buffer, y: u16, x: u16, background: Color) {
    let row_cells = input_row_cells(buffer, y, x);
    assert!(
        row_cells.iter().all(|cell| cell.bg != background),
        "row {y} unexpectedly has active input background"
    );
}

fn input_row_cells(buffer: &Buffer, y: u16, x: u16) -> Vec<&ratatui::buffer::Cell> {
    let area = *buffer.area();
    let panel_width = if area.width < 60 {
        area.width
    } else {
        area.width.saturating_mul(60) / 100
    };
    let panel_start = if x < area.x.saturating_add(panel_width) {
        area.x
    } else {
        area.x.saturating_add(panel_width)
    };
    let panel_end = if panel_start == area.x {
        area.x.saturating_add(panel_width)
    } else {
        area.x.saturating_add(area.width)
    };
    (panel_start.saturating_add(1)..panel_end.saturating_sub(1))
        .filter_map(|column| buffer.cell((column, y)))
        .collect()
}

fn input_active_background() -> Color {
    Color::Rgb(20, 38, 48)
}

fn conversation_background() -> Color {
    Color::Rgb(12, 18, 24)
}

fn sidebar_background() -> Color {
    Color::Rgb(18, 24, 32)
}

fn input_background() -> Color {
    Color::Rgb(20, 38, 48)
}

fn assistant_step(run_id: &RunId, index: u64, content: &str) -> Step {
    Step::new(
        StepId::new(run_id, index),
        StepKind::AssistantResponse,
        StepPayload::AssistantResponse {
            message_id: None,
            usage: LlmUsage::default(),
            items: vec![AssistantResponseItem::Text {
                content: content.to_owned(),
            }],
        },
    )
}

fn status_bar_background() -> Color {
    Color::Rgb(18, 28, 36)
}

#[test]
fn tui_completes_commands_and_restores_submitted_input_history() {
    let mut state = TestState::new("/workspace");
    type_text(&mut state, "/ver");
    update(
        &mut state,
        test_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
    );
    assert_eq!(state.input_text(), "/version");
    update(
        &mut state,
        test_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
    );
    type_text(&mut state, "remember this");
    update(
        &mut state,
        test_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
    );
    update(&mut state, AppMsg::RunCanceled);
    update(
        &mut state,
        test_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE)),
    );
    assert_eq!(state.input_text(), "remember this");
}

#[test]
fn tui_browses_input_history_and_restores_the_current_draft() {
    let mut state = TestState::new("/workspace");
    for request in ["first request", "second request"] {
        type_text(&mut state, request);
        update(
            &mut state,
            test_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        );
        update(&mut state, AppMsg::RunCanceled);
    }
    type_text(&mut state, "current draft");

    update(
        &mut state,
        test_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE)),
    );
    assert_eq!(state.input_text(), "second request");
    update(
        &mut state,
        test_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE)),
    );
    assert_eq!(state.input_text(), "first request");
    update(
        &mut state,
        test_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE)),
    );
    assert_eq!(state.input_text(), "second request");
    update(
        &mut state,
        test_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE)),
    );
    assert_eq!(state.input_text(), "current draft");
}

#[test]
fn tui_browses_persisted_session_input_history() {
    let workspace = assert_fs::TempDir::new().expect("temp workspace exists");
    let store = SqliteWorkspaceStore::new(workspace.path());
    let _first_run = store
        .create_run("first request".to_owned())
        .expect("first run is created");
    let _second_run = store
        .create_run("second request".to_owned())
        .expect("second run is created");

    let mut state = TestState::new(workspace.path());
    state
        .app
        .load_active_session_history(&store)
        .expect("history loads");
    type_text(&mut state, "draft");

    update(
        &mut state,
        test_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE)),
    );
    assert_eq!(state.input_text(), "second request");
    update(
        &mut state,
        test_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE)),
    );
    assert_eq!(state.input_text(), "first request");
    update(
        &mut state,
        test_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE)),
    );
    update(
        &mut state,
        test_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE)),
    );
    assert_eq!(state.input_text(), "draft");
}

#[test]
fn tui_keeps_up_and_down_for_cursor_movement_in_multiline_input() {
    let mut state = TestState::new("/workspace");
    type_text(&mut state, "first");
    update(
        &mut state,
        test_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT)),
    );
    type_text(&mut state, "second");

    update(
        &mut state,
        test_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE)),
    );
    type_text(&mut state, "!");

    assert_eq!(state.input_text(), "first!\nsecond");
}

#[test]
fn tui_completes_inline_skill_references_for_the_next_run() {
    let mut state = TestState::new("/workspace");
    state.set_skill_catalog(SkillCatalog {
        skills: vec![skill_definition(
            "review-code",
            SkillScope::Project,
            ".jux/skills",
        )],
        overrides: Vec::new(),
    });
    type_text(&mut state, "Use $review");
    let popup = render_to_buffer(&mut state, 100, 30);
    assert_buffer_contains(&popup, "$review-code");
    update(
        &mut state,
        test_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
    );
    assert_eq!(state.input_text(), "Use $review-code");
    assert_eq!(state.selected_skill_names(), &["review-code"]);
}

#[test]
fn tui_renders_markdown_and_code_blocks_with_terminal_styles() {
    let mut state = TestState::new("/workspace");
    update(
        &mut state,
        AppMsg::AssistantMessage {
            content: "# Heading\n- item\n> quote\n```rust\nfn main() {}\n```\n| Command | 说明 |\n|---|---|\n| `cargo test` | 运行测试 |"
                .to_owned(),
        },
    );
    let buffer = render_to_buffer(&mut state, 100, 30);
    assert_buffer_contains(&buffer, "Heading");
    assert_buffer_contains(&buffer, "• item");
    assert_buffer_contains(&buffer, "│ quote");
    assert_buffer_does_not_contain(&buffer, "rust");
    assert_buffer_contains(&buffer, "┌");
    assert_buffer_contains(&buffer, "┬");
    assert_buffer_contains(&buffer, "┐");
    assert_buffer_contains(&buffer, "└");
    assert_buffer_contains(&buffer, "┴");
    assert_buffer_contains(&buffer, "┘");
    assert_buffer_contains(&buffer, "cargo test");
    assert_buffer_does_not_contain(&buffer, "`cargo test`");
    assert_buffer_fragment_has_fg_bg(&buffer, "cargo test", Color::Yellow, Color::Rgb(38, 44, 52));
}

#[test]
fn tui_highlights_fenced_code_for_a_known_language() {
    let mut state = TestState::new("/workspace");
    update(
        &mut state,
        AppMsg::AssistantMessage {
            content: "```rust\nlet syntax_probe = \"highlighted\";\n```".to_owned(),
        },
    );

    let buffer = render_to_buffer(&mut state, 100, 30);
    assert_buffer_fragments_have_different_foregrounds(&buffer, "let", "highlighted");
    assert_buffer_fragment_has_background(&buffer, "let", Color::Rgb(24, 28, 34));
    assert_buffer_fragment_has_background(&buffer, "highlighted", Color::Rgb(24, 28, 34));
}

#[test]
fn tui_fills_the_markdown_content_width_with_the_code_background() {
    let mut state = TestState::new("/workspace");
    update(
        &mut state,
        AppMsg::AssistantMessage {
            content: "```rust\nlet width_probe = 1;\n\nwidth_probe\n```".to_owned(),
        },
    );

    let buffer = render_to_buffer(&mut state, 100, 30);
    let (code_y, code_x) = find_fragment_position(&buffer, "width_probe = 1")
        .expect("buffer contains the first code line");
    assert_row_has_background(&buffer, code_y, code_x, code_x + 35, Color::Rgb(24, 28, 34));
    assert_row_has_background(
        &buffer,
        code_y + 1,
        code_x,
        code_x + 35,
        Color::Rgb(24, 28, 34),
    );
}

#[test]
fn tui_highlights_common_fenced_code_language_aliases() {
    for language in ["ts", "typescript", "tsx"] {
        let mut state = TestState::new("/workspace");
        update(
            &mut state,
            AppMsg::AssistantMessage {
                content: format!(
                    "```{language}\nconst syntaxAliasProbe: string = \"highlighted\";\n```"
                ),
            },
        );

        let buffer = render_to_buffer(&mut state, 100, 30);
        assert_buffer_fragments_have_different_foregrounds(&buffer, "const", "highlighted");
    }

    for language in ["js", "javascript", "jsx"] {
        let mut state = TestState::new("/workspace");
        update(
            &mut state,
            AppMsg::AssistantMessage {
                content: format!("```{language}\nconst syntaxAliasProbe = \"highlighted\";\n```"),
            },
        );

        let buffer = render_to_buffer(&mut state, 100, 30);
        assert_buffer_fragments_have_different_foregrounds(&buffer, "const", "highlighted");
    }
}

#[test]
fn tui_falls_back_to_plain_text_without_a_known_code_language() {
    for opening_fence in ["```", "```not-a-language"] {
        let mut state = TestState::new("/workspace");
        update(
            &mut state,
            AppMsg::AssistantMessage {
                content: format!("{opening_fence}\nplain fallback content\n```"),
            },
        );

        let buffer = render_to_buffer(&mut state, 100, 30);
        assert_buffer_fragment_has_fg_bg(
            &buffer,
            "plain fallback content",
            Color::White,
            Color::Rgb(24, 28, 34),
        );
    }
}

#[test]
fn tui_selects_copies_and_loads_a_user_message_for_editing() {
    let mut state = TestState::new("/workspace");
    type_text(&mut state, "original request");
    update(
        &mut state,
        test_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
    );
    update(&mut state, AppMsg::RunCanceled);
    update(
        &mut state,
        test_key(KeyEvent::new(KeyCode::Up, KeyModifiers::ALT)),
    );
    assert_eq!(
        update(
            &mut state,
            test_key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::CONTROL)),
        ),
        Some(AppCommand::CopyText {
            content: "original request".to_owned(),
        })
    );
    update(
        &mut state,
        test_key(KeyEvent::new(KeyCode::Char('e'), KeyModifiers::CONTROL)),
    );
    assert_eq!(state.input_text(), "original request");
}

#[test]
fn tui_archives_a_session_and_preserves_a_working_active_session() {
    let workspace = assert_fs::TempDir::new().expect("temp workspace exists");
    let store = SqliteWorkspaceStore::new(workspace.path());
    let initial = store.init_workspace().expect("workspace initializes");
    let mut state = TestState::new(workspace.path());
    state
        .app
        .load_active_session_history(&store)
        .expect("history loads");
    type_text(&mut state, "/session");
    update(
        &mut state,
        test_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
    );
    let command = update(
        &mut state,
        test_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL)),
    )
    .expect("archive command is emitted");
    state
        .app
        .execute_session_command(&store, &command)
        .expect("session archives");
    assert!(
        store
            .load_session(&initial.active_session_id)
            .expect("archived session loads")
            .archived
    );
    assert_ne!(
        store
            .load_active_session()
            .expect("active session loads")
            .id,
        initial.active_session_id
    );
}

#[test]
fn tui_retries_failed_and_continues_canceled_requests() {
    let mut failed = TestState::new("/workspace");
    type_text(&mut failed, "retry me");
    update(
        &mut failed,
        test_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
    );
    update(
        &mut failed,
        AppMsg::RunFailed {
            error: "failed".to_owned(),
        },
    );
    type_text(&mut failed, "/retry");
    assert_eq!(
        update(
            &mut failed,
            test_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        ),
        Some(AppCommand::StartRun {
            request: "retry me".to_owned(),
        })
    );

    let mut canceled = TestState::new("/workspace");
    type_text(&mut canceled, "continue me");
    update(
        &mut canceled,
        test_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
    );
    update(&mut canceled, AppMsg::RunCanceled);
    type_text(&mut canceled, "/continue");
    assert_eq!(
        update(
            &mut canceled,
            test_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        ),
        Some(AppCommand::StartRun {
            request: "continue me".to_owned(),
        })
    );
}

#[test]
fn tui_searches_conversation_and_cycles_matching_messages() {
    let mut state = TestState::new("/workspace");
    for request in ["first needle", "middle", "last needle"] {
        type_text(&mut state, request);
        update(
            &mut state,
            test_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        );
        update(&mut state, AppMsg::RunCanceled);
    }
    type_text(&mut state, "/search needle");
    update(
        &mut state,
        test_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
    );
    let first = render_to_buffer(&mut state, 100, 30);
    assert_buffer_contains(&first, "▶ first needle");
    type_text(&mut state, "/search needle");
    update(
        &mut state,
        test_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
    );
    assert_eq!(state.messages()[2].content, "last needle");
}

#[test]
fn tui_applies_high_contrast_theme_and_custom_shortcuts() {
    let mut state = TestState::new("/workspace");
    state.set_runtime_info(TuiRuntimeInfo {
        theme: TuiTheme::HighContrast,
        shortcuts: TuiShortcutConfig {
            quit: jux_core::QuitShortcut::CtrlQ,
            copy_message: jux_core::CopyMessageShortcut::CtrlShiftC,
        },
        ..TuiRuntimeInfo::default()
    });
    let buffer = render_to_buffer(&mut state, 100, 24);
    assert_eq!(buffer[(1, 1)].bg, Color::Black);
    update(
        &mut state,
        test_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::CONTROL)),
    );
    assert!(state.should_quit);
}

#[test]
fn tui_shows_update_at_startup_and_in_sidebar() {
    let mut state = TestState::new("/workspace");
    let metadata = DistributionMetadata::unbranded();
    let notice = UpdateNotice {
        current_version: Version::parse("0.1.0").expect("current version"),
        latest_version: Version::parse("0.2.0").expect("latest version"),
        release_url: "https://github.com/jux-2026/jux/releases/tag/v0.2.0".to_owned(),
        recommendation: UpdateRecommendation::for_distribution(&metadata),
    };

    update(
        &mut state,
        AppMsg::UpdateAvailable {
            notice,
            show_startup_message: true,
        },
    );

    assert!(
        state.messages()[0]
            .content
            .contains("Jux 0.2.0 is available")
    );
    let buffer = render_to_buffer(&mut state, 100, 30);
    assert_buffer_contains(&buffer, "↑ 0.2.0 available");
}
