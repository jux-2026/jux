use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use jux_cli::tui::{
    AgentEventSender, AppAction, AppCommand, AppState, BackgroundRun, RunResponse, TuiRunRequest,
    TuiRunStatus, TuiRuntimeInfo, TuiSandboxSummary, execute_code_change_command,
    execute_session_command, load_active_session_history, render_app, update,
};
use jux_core::{
    AgentEvent, AgentEventData, AgentEventId, AgentEventKind, AssistantResponseItem,
    CodeChangePlan, CodeChangeProposal, LlmUsage, ProposedFileContent, ReviewStatus, RunId,
    RunStatus as CoreRunStatus, SkillCatalog, SkillDefinition, SkillOverride, SkillScope,
    SqliteWorkspaceStore, Step, StepId, StepKind, StepPayload,
};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use std::path::PathBuf;
use std::sync::{Arc, Barrier};
use std::time::Duration;

#[test]
fn tui_accepts_q_as_text_input() {
    let mut state = AppState::new("/workspace");

    update(
        &mut state,
        AppAction::Key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE)),
    );

    assert_eq!(state.input_text(), "q");
    assert!(!state.should_quit);
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
fn tui_accepts_multiline_text_input() {
    let mut state = AppState::new("/workspace");

    type_text(&mut state, "Fix");
    update(
        &mut state,
        AppAction::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT)),
    );
    type_text(&mut state, "query");

    assert_eq!(state.input_text(), "Fix\nquery");
    let buffer = render_to_buffer(&state, 80, 24);
    assert_buffer_contains(&buffer, "Fix");
    assert_buffer_contains(&buffer, "query");
}

#[test]
fn tui_inserts_text_at_the_cursor() {
    let mut state = AppState::new("/workspace");

    type_text(&mut state, "ac");
    update(
        &mut state,
        AppAction::Key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE)),
    );
    type_text(&mut state, "b");

    assert_eq!(state.input_text(), "abc");
}

#[test]
fn tui_deletes_text_around_the_cursor() {
    let mut state = AppState::new("/workspace");

    type_text(&mut state, "ab中c");
    update(
        &mut state,
        AppAction::Key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE)),
    );
    update(
        &mut state,
        AppAction::Key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE)),
    );
    update(
        &mut state,
        AppAction::Key(KeyEvent::new(KeyCode::Delete, KeyModifiers::NONE)),
    );

    assert_eq!(state.input_text(), "ab");
}

#[test]
fn tui_moves_the_cursor_across_lines() {
    let mut state = AppState::new("/workspace");

    type_text(&mut state, "abcd");
    update(
        &mut state,
        AppAction::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT)),
    );
    type_text(&mut state, "xy");
    update(
        &mut state,
        AppAction::Key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE)),
    );
    type_text(&mut state, "Z");
    update(
        &mut state,
        AppAction::Key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE)),
    );
    type_text(&mut state, "Q");
    update(
        &mut state,
        AppAction::Key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE)),
    );
    update(
        &mut state,
        AppAction::Key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE)),
    );
    type_text(&mut state, "!");

    assert_eq!(state.input_text(), "abZcd\nxyQ!");
}

#[test]
fn tui_submits_input_as_a_new_run_request() {
    let mut state = AppState::new("/workspace");
    type_text(&mut state, "Fix the failing test");

    let command = update(
        &mut state,
        AppAction::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
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
    let mut state = AppState::new("/workspace");
    type_text(&mut state, " \n ");

    let command = update(
        &mut state,
        AppAction::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
    );

    assert_eq!(command, None);
    assert_eq!(state.input_text(), " \n ");
}

#[test]
fn tui_displays_the_submitted_user_request() {
    let mut state = AppState::new("/workspace");
    type_text(&mut state, "Explain this workspace");
    update(
        &mut state,
        AppAction::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
    );

    let buffer = render_to_buffer(&state, 80, 24);

    assert_buffer_contains(&buffer, "You");
    assert_buffer_contains(&buffer, "Explain this workspace");
}

#[test]
fn tui_displays_the_agent_response() {
    let mut state = AppState::new("/workspace");

    update(
        &mut state,
        AppAction::AssistantMessage {
            content: "The workspace contains two crates.".to_owned(),
        },
    );

    let buffer = render_to_buffer(&state, 80, 24);
    assert_buffer_contains(&buffer, "Jux");
    assert_buffer_contains(&buffer, "The workspace contains two crates.");
}

#[test]
fn tui_scrolls_through_messages_longer_than_the_viewport() {
    let mut state = AppState::new("/workspace");
    for index in 0..12 {
        update(
            &mut state,
            AppAction::AssistantMessage {
                content: format!("response-{index}"),
            },
        );
    }

    let initial = render_to_buffer(&state, 80, 24);
    assert_buffer_does_not_contain(&initial, "response-8");

    update(
        &mut state,
        AppAction::Key(KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE)),
    );
    update(
        &mut state,
        AppAction::Key(KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE)),
    );

    let scrolled = render_to_buffer(&state, 80, 24);
    assert_buffer_contains(&scrolled, "response-8");
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
    let mut state = AppState::new("/workspace");

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
    let mut state = AppState::new("/workspace");
    let event_id = AgentEventId::llm(1, 1);

    update(
        &mut state,
        AppAction::AgentEvent(AgentEvent::new(
            event_id.clone(),
            AgentEventKind::Started,
            AgentEventData::LlmStarted,
        )),
    );
    let running = render_to_buffer(&state, 80, 24);
    assert_buffer_contains(&running, "LLM");
    assert_buffer_contains(&running, "Running");

    update(
        &mut state,
        AppAction::AgentEvent(AgentEvent::new(
            event_id,
            AgentEventKind::Completed,
            AgentEventData::LlmCompleted,
        )),
    );
    assert_eq!(state.timeline().len(), 1);
    let completed = render_to_buffer(&state, 80, 24);
    assert_buffer_contains(&completed, "LLM");
    assert_buffer_contains(&completed, "Completed");
}

#[test]
fn tui_displays_llm_failure_details_in_the_timeline() {
    let mut state = AppState::new("/workspace");

    update(
        &mut state,
        AppAction::AgentEvent(AgentEvent::new(
            AgentEventId::llm(1, 1),
            AgentEventKind::Failed,
            AgentEventData::LlmFailed {
                error: "model unavailable".to_owned(),
            },
        )),
    );

    let buffer = render_to_buffer(&state, 80, 24);
    assert_buffer_contains(&buffer, "LLM");
    assert_buffer_contains(&buffer, "Failed");
    assert_buffer_contains(&buffer, "model unavailable");
}

#[test]
fn tui_updates_tool_lifecycle_and_keeps_its_output() {
    let mut state = AppState::new("/workspace");
    let event_id = AgentEventId::tool(1, "exec", 1);

    update(
        &mut state,
        AppAction::AgentEvent(AgentEvent::new(
            event_id.clone(),
            AgentEventKind::Started,
            AgentEventData::ToolStarted {
                name: "exec".to_owned(),
                call_id: Some("call-1".to_owned()),
                arguments: serde_json::json!({ "command": "cargo test" }),
            },
        )),
    );
    update(
        &mut state,
        AppAction::AgentEvent(AgentEvent::new(
            event_id.clone(),
            AgentEventKind::Output,
            AgentEventData::ToolOutput {
                name: "exec".to_owned(),
                content: serde_json::json!({ "stdout": "tests passed" }),
            },
        )),
    );
    update(
        &mut state,
        AppAction::AgentEvent(AgentEvent::new(
            event_id,
            AgentEventKind::Completed,
            AgentEventData::ToolCompleted {
                name: "exec".to_owned(),
                call_id: Some("call-1".to_owned()),
            },
        )),
    );

    assert_eq!(state.timeline().len(), 1);
    let buffer = render_to_buffer(&state, 80, 24);
    assert_buffer_contains(&buffer, "Tool: exec");
    assert_buffer_contains(&buffer, "Completed");
    assert_buffer_contains(&buffer, "tests passed");
}

#[test]
fn tui_expands_tool_arguments_and_output() {
    let mut state = AppState::new("/workspace");
    let event_id = AgentEventId::tool(1, "exec", 1);
    update(
        &mut state,
        AppAction::AgentEvent(AgentEvent::new(
            event_id.clone(),
            AgentEventKind::Started,
            AgentEventData::ToolStarted {
                name: "exec".to_owned(),
                call_id: Some("call-1".to_owned()),
                arguments: serde_json::json!({ "command": "cargo test" }),
            },
        )),
    );
    update(
        &mut state,
        AppAction::AgentEvent(AgentEvent::new(
            event_id,
            AgentEventKind::Output,
            AgentEventData::ToolOutput {
                name: "exec".to_owned(),
                content: serde_json::json!({
                    "stdout": "all tests passed",
                    "exit_code": 0
                }),
            },
        )),
    );

    let collapsed = render_to_buffer(&state, 80, 24);
    assert_buffer_does_not_contain(&collapsed, "Arguments:");

    update(
        &mut state,
        AppAction::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
    );
    update(
        &mut state,
        AppAction::Key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE)),
    );

    let expanded = render_to_buffer(&state, 80, 24);
    assert_buffer_contains(&expanded, "Arguments:");
    assert_buffer_contains(&expanded, "cargo test");
    assert_buffer_contains(&expanded, "Output:");
    assert_buffer_contains(&expanded, "all tests passed");
}

#[test]
fn tui_truncates_long_tool_output() {
    let mut state = AppState::new("/workspace");
    update(
        &mut state,
        AppAction::AgentEvent(AgentEvent::new(
            AgentEventId::tool(1, "exec", 1),
            AgentEventKind::Output,
            AgentEventData::ToolOutput {
                name: "exec".to_owned(),
                content: serde_json::json!({ "stdout": "中".repeat(500) }),
            },
        )),
    );

    let buffer = render_to_buffer(&state, 120, 30);
    assert_buffer_contains(&buffer, "[truncated]");
}

#[test]
fn tui_displays_active_and_running_skills() {
    let mut state = AppState::new("/workspace");
    update(
        &mut state,
        AppAction::AgentEvent(AgentEvent::new(
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
        AppAction::AgentEvent(AgentEvent::new(
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
        AppAction::AgentEvent(AgentEvent::new(
            event_id,
            AgentEventKind::Completed,
            AgentEventData::ToolCompleted {
                name: "call_skill".to_owned(),
                call_id: Some("call-1".to_owned()),
            },
        )),
    );

    let buffer = render_to_buffer(&state, 80, 24);
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
    let mut state = AppState::new("/workspace");
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
        AppAction::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
    );

    let buffer = render_to_buffer(&state, 120, 30);
    assert_buffer_contains(&buffer, "Skills");
    assert_buffer_contains(&buffer, "review");
    assert_buffer_contains(&buffer, "Project");
    assert_buffer_contains(&buffer, "overrides User");
    assert_buffer_contains(&buffer, "Description for review");
    assert_buffer_contains(&buffer, "/workspace/.jux/skills/review/SKILL.md");
}

#[test]
fn tui_selects_explicit_skills_for_the_next_run() {
    let mut state = AppState::new("/workspace");
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
        AppAction::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
    );
    update(
        &mut state,
        AppAction::Key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE)),
    );
    update(
        &mut state,
        AppAction::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
    );
    type_text(&mut state, "Review this change");

    let command = update(
        &mut state,
        AppAction::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
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
    let mut state = AppState::new("/workspace");
    type_text(&mut state, "/quit");

    let command = update(
        &mut state,
        AppAction::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
    );

    assert_eq!(command, None);
    assert!(state.should_quit);
}

#[test]
fn tui_clear_command_removes_messages_without_starting_a_run() {
    let mut state = AppState::new("/workspace");
    update(
        &mut state,
        AppAction::AssistantMessage {
            content: "Temporary output".to_owned(),
        },
    );
    type_text(&mut state, "/clear");

    let command = update(
        &mut state,
        AppAction::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
    );

    assert_eq!(command, None);
    assert!(state.messages().is_empty());
    assert_eq!(state.input_text(), "");
}

#[test]
fn tui_help_command_displays_commands_and_shortcuts() {
    let mut state = AppState::new("/workspace");
    type_text(&mut state, "/help");

    let command = update(
        &mut state,
        AppAction::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
    );

    assert_eq!(command, None);
    let buffer = render_to_buffer(&state, 80, 24);
    assert_buffer_contains(&buffer, "/clear");
    assert_buffer_contains(&buffer, "/quit");
    assert_buffer_contains(&buffer, "PageUp");
    assert_buffer_contains(&buffer, "Shift+Enter");
}

#[test]
fn tui_keeps_the_input_visible_in_a_small_terminal() {
    let mut state = AppState::new("/workspace");
    type_text(&mut state, "compact request");

    let buffer = render_to_buffer(&state, 30, 8);

    assert_buffer_contains(&buffer, "compact request");
}

#[test]
fn tui_does_not_start_a_second_run_while_running() {
    let mut state = AppState::new("/workspace");
    type_text(&mut state, "first request");
    let first = update(
        &mut state,
        AppAction::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
    );
    type_text(&mut state, "second request");

    let second = update(
        &mut state,
        AppAction::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
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
    let buffer = render_to_buffer(&state, 80, 24);
    assert_buffer_contains(&buffer, "Status: Running");
}

#[test]
fn tui_cancels_the_running_run_with_escape() {
    let mut state = AppState::new("/workspace");
    type_text(&mut state, "long-running request");
    update(
        &mut state,
        AppAction::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
    );

    let command = update(
        &mut state,
        AppAction::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
    );
    update(&mut state, AppAction::RunCanceled);

    assert_eq!(command, Some(AppCommand::CancelRun));
    assert_eq!(state.run_status(), TuiRunStatus::Canceled);
    let buffer = render_to_buffer(&state, 80, 24);
    assert_buffer_contains(&buffer, "Status: Canceled");
}

#[test]
fn tui_displays_waiting_run_status_and_metadata() {
    let mut state = AppState::new("/workspace");

    update(
        &mut state,
        AppAction::RunFinished {
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
    let buffer = render_to_buffer(&state, 80, 24);
    assert_buffer_contains(&buffer, "Status: Waiting");
    assert_buffer_contains(&buffer, "Session: workspace-0001");
    assert_buffer_contains(&buffer, "Run: workspace-0001-000001");
    assert_buffer_contains(&buffer, "Elapsed: 250 ms");
}

#[test]
fn tui_displays_the_pending_human_input_question() {
    let mut state = AppState::new("/workspace");
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
        AppAction::RunFinished {
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

    let buffer = render_to_buffer(&state, 80, 24);
    assert_buffer_contains(&buffer, "Which implementation should Jux use?");
    assert_buffer_contains(&buffer, "safe  Safer implementation");
    assert_buffer_contains(&buffer, "fast  Faster implementation");
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
    let mut state = AppState::new(workspace.path());

    load_active_session_history(&mut state, &store).expect("history loads");

    assert_eq!(state.run_status(), TuiRunStatus::Completed);
    let buffer = render_to_buffer(&state, 80, 24);
    assert_buffer_contains(&buffer, "Historical request");
    assert_buffer_contains(&buffer, "Historical answer");
    assert_buffer_contains(&buffer, &run.id.to_string());
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
    let mut state = AppState::new(workspace.path());

    load_active_session_history(&mut state, &store).expect("history loads");

    assert_eq!(state.run_status(), TuiRunStatus::Failed);
    let buffer = render_to_buffer(&state, 80, 24);
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
    let mut state = AppState::new(workspace.path());

    load_active_session_history(&mut state, &store).expect("history loads");
    let command = update(
        &mut state,
        AppAction::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
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
    let mut state = AppState::new(workspace.path());
    load_active_session_history(&mut state, &store).expect("history loads");
    type_text(&mut state, "/sessions");

    let command = update(
        &mut state,
        AppAction::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
    );

    assert_eq!(command, None);
    let buffer = render_to_buffer(&state, 80, 24);
    assert_buffer_contains(&buffer, "Sessions");
    assert_buffer_contains(&buffer, "* default");
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
    let mut state = AppState::new(workspace.path());
    load_active_session_history(&mut state, &store).expect("history loads");

    type_text(&mut state, "/session new feature-a");
    let create = update(
        &mut state,
        AppAction::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
    )
    .expect("create command is emitted");
    assert!(execute_session_command(&mut state, &store, &create).expect("session is created"));
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
        AppAction::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
    )
    .expect("rename command is emitted");
    assert!(execute_session_command(&mut state, &store, &rename).expect("session is renamed"));
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
        AppAction::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
    )
    .expect("switch command is emitted");
    assert!(execute_session_command(&mut state, &store, &switch).expect("session is switched"));
    assert_eq!(state.session_id(), Some(default_session.as_str()));
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
    let mut state = AppState::new(workspace.path());
    load_active_session_history(&mut state, &store).expect("history loads");
    type_text(&mut state, "/sessions");
    update(
        &mut state,
        AppAction::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
    );

    let buffer = render_to_buffer(&state, 120, 30);
    assert_buffer_contains(&buffer, "Default session run");
    assert_buffer_contains(&buffer, "Feature session run");
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
    let mut state = AppState::new(workspace.path());

    update(
        &mut state,
        AppAction::AgentEvent(AgentEvent::new(
            AgentEventId::tool(1, "propose_code_change", 1),
            AgentEventKind::Output,
            AgentEventData::ToolOutput {
                name: "propose_code_change".to_owned(),
                content: serde_json::to_value(proposal).expect("proposal serializes"),
            },
        )),
    );
    let first = render_to_buffer(&state, 120, 40);
    assert_buffer_contains(&first, "Plan: Rename both functions");
    assert_buffer_contains(&first, "src/a.rs");
    assert_buffer_contains(&first, "src/b.rs");
    assert_buffer_contains(&first, "Policy: Confirm");
    assert_buffer_contains(&first, "-fn old_a() {}");
    assert_buffer_contains(&first, "+fn new_a() {}");

    update(
        &mut state,
        AppAction::Key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE)),
    );
    let second = render_to_buffer(&state, 120, 40);
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
    let mut state = AppState::new(workspace.path());
    update(&mut state, AppAction::CodeChangeProposed { proposal });
    type_text(&mut state, "/review accept");
    let command = update(
        &mut state,
        AppAction::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
    )
    .expect("accept command is emitted");

    execute_code_change_command(&mut state, &command).expect("change is applied");

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
        AppAction::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
    );
    let buffer = render_to_buffer(&state, 120, 30);
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
    let mut state = AppState::new(workspace.path());
    update(&mut state, AppAction::CodeChangeProposed { proposal });
    type_text(&mut state, "/review reject");
    let command = update(
        &mut state,
        AppAction::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
    )
    .expect("reject command is emitted");

    execute_code_change_command(&mut state, &command).expect("change is rejected");

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
    let mut state = AppState::new(workspace.path());
    update(&mut state, AppAction::CodeChangeProposed { proposal });
    type_text(&mut state, "/review changes Add an example");
    let command = update(
        &mut state,
        AppAction::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
    )
    .expect("changes command is emitted");

    let handled = execute_code_change_command(&mut state, &command).expect("changes are requested");

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
    let mut state = AppState::new(workspace.path());
    update(&mut state, AppAction::CodeChangeProposed { proposal });
    std::fs::write(workspace.path().join("README.md"), "external\n").expect("source file changes");
    type_text(&mut state, "/review accept");
    let command = update(
        &mut state,
        AppAction::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
    )
    .expect("accept command is emitted");

    execute_code_change_command(&mut state, &command).expect("conflict is displayed");

    assert_eq!(
        state.code_change_review().expect("review exists").status,
        ReviewStatus::Conflict {
            paths: vec!["README.md".to_owned()],
        }
    );
    let buffer = render_to_buffer(&state, 120, 30);
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
    let mut state = AppState::new(workspace.path());
    update(&mut state, AppAction::CodeChangeProposed { proposal });
    let proposal_view = render_to_buffer(&state, 120, 30);
    assert_buffer_contains(&proposal_view, "Policy: Deny");
    assert_buffer_contains(&proposal_view, "Risk [High] .env");
    type_text(&mut state, "/review accept");
    let command = update(
        &mut state,
        AppAction::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
    )
    .expect("accept command is emitted");

    execute_code_change_command(&mut state, &command).expect("denial is displayed");

    let denied = render_to_buffer(&state, 120, 30);
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
    let mut state = AppState::new(workspace.path());
    update(
        &mut state,
        AppAction::RunFinished {
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
        AppAction::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
    );

    let buffer = render_to_buffer(&state, 120, 40);
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
        AppAction::Key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE)),
    );
    let command = update(
        &mut state,
        AppAction::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
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
        AppAction::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
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
        AppAction::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
    );

    assert_eq!(command, None);
    assert_eq!(state.run_status(), TuiRunStatus::WaitingForHumanInput);
    assert_eq!(state.input_text(), "invalid");
    let buffer = render_to_buffer(&state, 120, 24);
    assert_buffer_contains(&buffer, "must match one of the option ids");
    assert_buffer_contains(&buffer, "safe, fast");
}

#[test]
fn tui_distinguishes_operation_confirmation_from_clarification() {
    let state = waiting_human_input_state_with_kind(false, Some("confirmation"));

    let buffer = render_to_buffer(&state, 80, 24);

    assert_buffer_contains(&buffer, "Confirmation required");
    assert_buffer_does_not_contain(&buffer, "Input required");
}

#[test]
fn tui_displays_the_completed_run_answer() {
    let mut state = AppState::new("/workspace");

    update(
        &mut state,
        AppAction::RunFinished {
            response: RunResponse {
                session_id: "workspace-0001".to_owned(),
                run_id: "workspace-0001-000001".to_owned(),
                status: CoreRunStatus::Completed,
                created_at: 1_000,
                updated_at: 1_500,
                answer: Some("The requested change is complete.".to_owned()),
                steps: Vec::new(),
            },
        },
    );

    assert_eq!(state.run_status(), TuiRunStatus::Completed);
    let buffer = render_to_buffer(&state, 80, 24);
    assert_buffer_contains(&buffer, "Status: Completed");
    assert_buffer_contains(&buffer, "The requested change is complete.");
}

#[test]
fn tui_displays_persisted_steps_in_id_order() {
    let mut state = AppState::new("/workspace");
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
        AppAction::RunFinished {
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
    let buffer = render_to_buffer(&state, 80, 24);
    assert_buffer_contains(&buffer, "User message");
    assert_buffer_contains(&buffer, "Tool result");
}

#[test]
fn tui_keeps_run_errors_in_the_message_timeline() {
    let mut state = AppState::new("/workspace");

    update(
        &mut state,
        AppAction::RunFailed {
            error: "provider timed out".to_owned(),
        },
    );
    update(
        &mut state,
        AppAction::AssistantMessage {
            content: "Later message".to_owned(),
        },
    );

    assert_eq!(state.run_status(), TuiRunStatus::Failed);
    let buffer = render_to_buffer(&state, 80, 24);
    assert_buffer_contains(&buffer, "Error");
    assert_buffer_contains(&buffer, "provider timed out");
    assert_buffer_contains(&buffer, "Later message");
}

#[test]
fn tui_shell_renders_workspace_and_idle_status() {
    let state = AppState::new("/workspace");

    let buffer = render_to_buffer(&state, 80, 24);

    assert_buffer_contains(&buffer, "Jux");
    assert_buffer_contains(&buffer, "What should Jux work on?");
    assert_buffer_contains(&buffer, "Workspace: /workspace");
    assert_buffer_contains(&buffer, "Status: Idle");
    assert_buffer_contains(&buffer, "Ctrl+C quit");
}

#[test]
fn tui_displays_workspace_model_sandbox_and_summary_status() {
    let mut state = AppState::new("/workspace");
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
    });

    let buffer = render_to_buffer(&state, 120, 30);

    assert_buffer_contains(&buffer, "Workspace ID: workspace-1234");
    assert_buffer_contains(&buffer, "Model: deepseek/deepseek-chat");
    assert_buffer_contains(&buffer, "Filesystem: read-only");
    assert_buffer_contains(&buffer, "Network: deny by default");
    assert_buffer_contains(&buffer, "Native commands: disabled");
    assert_buffer_contains(&buffer, "Session - | Run - | deepseek/deepseek-chat | Idle");
}

#[test]
fn tui_displays_configuration_errors_and_runtime_logs() {
    let mut state = AppState::new("/workspace");
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
    });
    update(
        &mut state,
        AppAction::RunFailed {
            error: "model unavailable".to_owned(),
        },
    );
    type_text(&mut state, "/logs");
    update(
        &mut state,
        AppAction::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
    );

    let buffer = render_to_buffer(&state, 120, 30);

    assert_buffer_contains(&buffer, "Configuration error");
    assert_buffer_contains(&buffer, "invalid config shape: unknown field");
    assert_buffer_contains(&buffer, "Run failed");
    assert_buffer_contains(&buffer, "model unavailable");
}

fn render_to_buffer(state: &AppState, width: u16, height: u16) -> Buffer {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("test terminal is created");
    terminal
        .draw(|frame| render_app(frame, state))
        .expect("app renders");
    terminal.backend().buffer().clone()
}

fn type_text(state: &mut AppState, text: &str) {
    for character in text.chars() {
        update(
            state,
            AppAction::Key(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE)),
        );
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

fn waiting_human_input_state(allow_free_text: bool) -> AppState {
    waiting_human_input_state_with_kind(allow_free_text, None)
}

fn waiting_human_input_state_with_kind(allow_free_text: bool, kind: Option<&str>) -> AppState {
    let mut state = AppState::new("/workspace");
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
        AppAction::RunFinished {
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
