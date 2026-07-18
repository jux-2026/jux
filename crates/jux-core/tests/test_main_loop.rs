use jux_core::{
    AgentEvent, AgentEventKind, AgentEventSink, AssistantResponseItem, CodeChangeProposal,
    InstructionDocument, InstructionScope, LlmUsage, RunCancellationHandle, RunLoop,
    RunLoopContext, RunLoopError, RunStatus, RuntimePolicy, SYSTEM_PROMPT, SessionContextKind,
    SessionContextPayload, SkillDefinition, SkillScope, SqliteWorkspaceStore, StepKind,
    StepPayload, run_cancellation_pair,
};
use rig::OneOrMany;
use rig::completion::{
    CompletionError, CompletionModel, CompletionRequest, CompletionResponse, GetTokenUsage, Usage,
};
use rig::message::{AssistantContent, Reasoning, ToolCall, ToolFunction};
use rig::streaming::{RawStreamingChoice, StreamingCompletionResponse};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};

#[test]
fn run_loop_records_successful_llm_run_steps() {
    let store = SqliteWorkspaceStore::new(temp_workspace_root());
    let model = TestModel::fixed_text("Mocked answer");
    let run_loop = RunLoop::new(store.clone(), model.clone());

    let output = futures::executor::block_on(run_loop.run("Explain this project".to_owned()))
        .expect("run loop succeeds");
    let requests = model.recorded_requests();

    assert_eq!(output.run.status, RunStatus::Completed);
    assert_eq!(output.answer.as_deref(), Some("Mocked answer"));
    assert_eq!(
        output
            .steps
            .iter()
            .map(|step| step.kind.clone())
            .collect::<Vec<_>>(),
        vec![StepKind::UserMessage, StepKind::AssistantResponse]
    );
    assert_eq!(
        output.steps[0].payload,
        StepPayload::UserMessage {
            content: "Explain this project".to_owned(),
        }
    );
    let context_items = store
        .load_session_context_items(&output.session.id)
        .expect("session context loads");
    assert_eq!(context_items.len(), 5);
    assert_eq!(context_items[0].sequence, 1);
    assert_eq!(context_items[0].kind, SessionContextKind::SystemPrompt);
    assert_eq!(
        context_items[0].payload,
        SessionContextPayload::SystemPrompt {
            content: SYSTEM_PROMPT.to_owned(),
        }
    );
    assert_eq!(context_items[1].sequence, 2);
    assert_eq!(context_items[1].payload.to_tool_name(), Some("exec"));
    assert_eq!(context_items[2].sequence, 3);
    assert_eq!(context_items[2].payload.to_tool_name(), Some("lua"));
    assert_eq!(context_items[3].sequence, 4);
    assert_eq!(context_items[3].payload.to_tool_name(), Some("human_input"));
    assert_eq!(context_items[4].sequence, 5);
    assert_eq!(
        context_items[4].payload.to_tool_name(),
        Some("propose_code_change")
    );
    assert_eq!(requests.len(), 1);
    assert!(requests[0].contains("You are Jux"));
    assert!(requests[0].contains("Explain this project"));
    assert!(requests[0].contains("\"tools\""));
    assert!(requests[0].contains("restricted Jux Lua runtime"));
    assert!(requests[0].contains("All Lua standard libraries are disabled"));
    assert!(requests[0].contains("io.popen"));
    assert!(requests[0].contains("not executed through a shell"));
    assert!(requests[0].contains("Do not call print"));
    assert!(requests[0].contains("Use return to send the result back to Jux"));
    assert_eq!(
        output.steps[1].payload.to_assistant_text(),
        Some("Mocked answer")
    );
    assert_eq!(
        output.steps[1].payload.to_assistant_usage(),
        Some(&LlmUsage::default())
    );
}

#[test]
fn run_loop_cancellation_stops_and_persists_the_running_run() {
    let store = SqliteWorkspaceStore::new(temp_workspace_root());
    let (handle, token) = run_cancellation_pair();
    let model = TestModel::canceling(handle);
    let run_loop = RunLoop::new(store.clone(), model);

    let error = futures::executor::block_on(
        run_loop.run_cancellable("Long-running request".to_owned(), token),
    )
    .expect_err("run is canceled");

    assert!(matches!(error, RunLoopError::Canceled { .. }));
    let session = store.load_active_session().expect("active session exists");
    let runs = store
        .load_session_runs(&session.id)
        .expect("session runs load");
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].status, RunStatus::Canceled);
}

#[test]
fn run_loop_persists_streamed_partial_output_when_canceled() {
    let store = SqliteWorkspaceStore::new(temp_workspace_root());
    let (handle, token) = run_cancellation_pair();
    let model = TestModel::canceling_stream(handle);
    let policy = RuntimePolicy::workspace_default(store.root().to_path_buf());
    let context = RunLoopContext::new(store.clone(), model, policy).with_model_streaming(true);
    let run_loop = RunLoop::with_context(context);

    let error = futures::executor::block_on(
        run_loop.run_cancellable("Stream then cancel".to_owned(), token),
    )
    .expect_err("streaming run is canceled");

    assert!(matches!(error, RunLoopError::Canceled { .. }));
    let session = store.load_active_session().expect("active session exists");
    let run = store
        .load_session_runs(&session.id)
        .expect("session runs load")
        .remove(0);
    let steps = store.load_run_steps(&run.id).expect("run steps load");
    assert_eq!(run.status, RunStatus::Canceled);
    assert!(steps.iter().any(|step| {
        step.kind == StepKind::AssistantOutputCheckpoint
            && step.payload
                == StepPayload::AssistantOutputCheckpoint {
                    content: "partial".to_owned(),
                }
    }));
}

#[test]
fn run_loop_sends_user_and_project_instruction_documents_to_llm() {
    let store = SqliteWorkspaceStore::new(temp_workspace_root());
    let model = TestModel::fixed_text("Instruction-aware answer");
    let policy = RuntimePolicy::workspace_default(store.root().to_path_buf());
    let context =
        RunLoopContext::new(store.clone(), model.clone(), policy).with_instructions(vec![
            InstructionDocument {
                scope: InstructionScope::User,
                path: "/home/user/.jux/AGENTS.md".into(),
                content: "Always prefer user defaults.".to_owned(),
            },
            InstructionDocument {
                scope: InstructionScope::Project,
                path: "/workspace/.jux/AGENTS.md".into(),
                content: "Project instructions win.".to_owned(),
            },
        ]);
    let run_loop = RunLoop::with_context(context);

    futures::executor::block_on(run_loop.run("Read instructions".to_owned()))
        .expect("run loop succeeds");
    let request = model.recorded_requests().remove(0);

    assert!(request.contains("Project instructions have higher priority than user instructions."));
    assert!(request.contains("Always prefer user defaults."));
    assert!(request.contains("Project instructions win."));
    assert!(
        request.find("Always prefer user defaults.") < request.find("Project instructions win.")
    );
}

#[test]
fn run_loop_sends_available_skill_index_to_llm_without_full_skill_body() {
    let store = SqliteWorkspaceStore::new(temp_workspace_root());
    let model = TestModel::fixed_text("Skill-aware answer");
    let policy = RuntimePolicy::workspace_default(store.root().to_path_buf());
    let context = RunLoopContext::new(store.clone(), model.clone(), policy).with_skills(vec![
        SkillDefinition {
            name: "review".to_owned(),
            description: "Review code changes".to_owned(),
            content: "Full review skill body should not be in the index.".to_owned(),
            scope: SkillScope::Project,
            path: "/workspace/.jux/skills/review/SKILL.md".into(),
        },
    ]);
    let run_loop = RunLoop::with_context(context);

    futures::executor::block_on(run_loop.run("Use available skills".to_owned()))
        .expect("run loop succeeds");
    let request = model.recorded_requests().remove(0);

    assert!(request.contains("## Available Skills"));
    assert!(request.contains("- review: Review code changes"));
    assert!(request.contains("\"name\":\"call_skill\""));
    assert!(!request.contains("Full review skill body should not be in the index."));
}

#[test]
fn run_loop_executes_skill_with_read_only_parent_context() {
    let store = SqliteWorkspaceStore::new(temp_workspace_root());
    let model = TestModel::responses([
        Ok(vec![AssistantContent::ToolCall(test_tool_call(
            "call_1",
            "call_skill",
            serde_json::json!({
                "name": "review",
                "task": "Review current changes"
            }),
        ))]),
        Ok(vec![AssistantContent::text("Skill found one issue")]),
        Ok(vec![AssistantContent::text("Parent received skill result")]),
    ]);
    let policy = RuntimePolicy::workspace_default(store.root().to_path_buf());
    let skill = SkillDefinition {
        name: "review".to_owned(),
        description: "Review code changes".to_owned(),
        content: "Full isolated review instructions.".to_owned(),
        scope: SkillScope::Project,
        path: "/workspace/.jux/skills/review/SKILL.md".into(),
    };
    let context =
        RunLoopContext::new(store.clone(), model.clone(), policy).with_skills(vec![skill]);
    let run_loop = RunLoop::with_context(context);

    let output = futures::executor::block_on(run_loop.run("Use review skill".to_owned()))
        .expect("run loop succeeds");
    let requests = model.recorded_requests();
    let skill_result = output
        .steps
        .iter()
        .find_map(|step| step.payload.to_tool_result_content())
        .expect("skill tool result exists");

    assert_eq!(requests.len(), 3);
    assert!(!requests[0].contains("Full isolated review instructions."));
    assert!(requests[1].contains("Full isolated review instructions."));
    assert!(requests[1].contains("Use review skill"));
    assert!(requests[1].contains("Review current changes"));
    assert!(!requests[1].contains("\"name\":\"call_skill\""));
    assert_eq!(skill_result["success"], true);
    assert_eq!(skill_result["skill"], "review");
    assert_eq!(skill_result["summary"], "Skill found one issue");
    assert!(requests[2].contains("Skill found one issue"));
    assert!(!requests[2].contains("Full isolated review instructions."));
}

#[test]
fn skill_subflow_inherits_parent_history_across_runs() {
    let store = SqliteWorkspaceStore::new(temp_workspace_root());
    let model = TestModel::responses([
        Ok(vec![AssistantContent::text("Earlier parent answer")]),
        Ok(vec![AssistantContent::ToolCall(test_tool_call(
            "skill_call",
            "call_skill",
            serde_json::json!({
                "name": "review",
                "task": "Review with the earlier decision"
            }),
        ))]),
        Ok(vec![AssistantContent::text("Skill used parent history")]),
        Ok(vec![AssistantContent::text("Parent answer")]),
    ]);
    let policy = RuntimePolicy::workspace_default(store.root().to_path_buf());
    let context = RunLoopContext::new(store, model.clone(), policy).with_skills(vec![test_skill(
        "review",
        "Review code changes",
        "Use the inherited context.",
    )]);
    let run_loop = RunLoop::with_context(context);

    futures::executor::block_on(run_loop.run("Remember parent decision".to_owned()))
        .expect("first run succeeds");
    futures::executor::block_on(run_loop.run("Use the review skill now".to_owned()))
        .expect("second run succeeds");
    let requests = model.recorded_requests();

    assert_eq!(requests.len(), 4);
    assert!(requests[2].contains("Remember parent decision"));
    assert!(requests[2].contains("Earlier parent answer"));
    assert!(requests[2].contains("Use the review skill now"));
    assert!(requests[2].contains("Review with the earlier decision"));
    assert!(requests[2].contains("Use the inherited context."));
    assert!(!requests[3].contains("Use the inherited context."));
}

#[test]
fn explicit_skill_context_uses_the_current_run_when_invocation_ids_repeat() {
    let store = SqliteWorkspaceStore::new(temp_workspace_root());
    let model = TestModel::responses([
        Ok(vec![AssistantContent::text("First skill result")]),
        Ok(vec![AssistantContent::text("First parent answer")]),
        Ok(vec![AssistantContent::text("Second skill result")]),
        Ok(vec![AssistantContent::text("Second parent answer")]),
    ]);
    let policy = RuntimePolicy::workspace_default(store.root().to_path_buf());
    let skill = test_skill(
        "review",
        "Review code changes",
        "Use the inherited context.",
    );
    let context = RunLoopContext::new(store, model.clone(), policy)
        .with_skills(vec![skill.clone()])
        .with_requested_skills(vec![skill]);
    let run_loop = RunLoop::with_context(context);

    futures::executor::block_on(run_loop.run("First explicit request".to_owned()))
        .expect("first run succeeds");
    futures::executor::block_on(run_loop.run("Second explicit request".to_owned()))
        .expect("second run succeeds");
    let requests = model.recorded_requests();

    assert_eq!(requests.len(), 4);
    assert!(requests[2].contains("First explicit request"));
    assert!(requests[2].contains("First parent answer"));
    assert!(requests[2].contains("Second explicit request"));
}

#[test]
fn skill_subflow_inherits_instruction_documents() {
    let store = SqliteWorkspaceStore::new(temp_workspace_root());
    let model = TestModel::responses([
        Ok(vec![AssistantContent::ToolCall(test_tool_call(
            "skill_call",
            "call_skill",
            serde_json::json!({
                "name": "review",
                "task": "Review current changes"
            }),
        ))]),
        Ok(vec![AssistantContent::text("Skill followed instructions")]),
        Ok(vec![AssistantContent::text("Parent answer")]),
    ]);
    let policy = RuntimePolicy::workspace_default(store.root().to_path_buf());
    let context = RunLoopContext::new(store.clone(), model.clone(), policy)
        .with_instructions(vec![InstructionDocument {
            scope: InstructionScope::Project,
            path: "/workspace/AGENTS.md".into(),
            content: "Project skill instruction must be visible.".to_owned(),
        }])
        .with_skills(vec![test_skill(
            "review",
            "Review code changes",
            "Review carefully.",
        )]);
    let run_loop = RunLoop::with_context(context);

    futures::executor::block_on(run_loop.run("Use review skill".to_owned()))
        .expect("run loop succeeds");
    let requests = model.recorded_requests();

    assert!(requests[1].contains("Project skill instruction must be visible."));
}

#[test]
fn run_loop_refreshes_skill_index_between_runs_in_same_session() {
    let store = SqliteWorkspaceStore::new(temp_workspace_root());
    let first_model = TestModel::fixed_text("First answer");
    let first_policy = RuntimePolicy::workspace_default(store.root().to_path_buf());
    let first_context =
        RunLoopContext::new(store.clone(), first_model, first_policy).with_skills(vec![
            test_skill("review", "Review code changes", "Review carefully."),
        ]);
    futures::executor::block_on(
        RunLoop::with_context(first_context).run("First request".to_owned()),
    )
    .expect("first run succeeds");

    let second_model = TestModel::fixed_text("Second answer");
    let second_policy = RuntimePolicy::workspace_default(store.root().to_path_buf());
    let second_context = RunLoopContext::new(store, second_model.clone(), second_policy)
        .with_skills(vec![test_skill(
            "diagnose",
            "Diagnose failing behavior",
            "Diagnose carefully.",
        )]);
    futures::executor::block_on(
        RunLoop::with_context(second_context).run("Second request".to_owned()),
    )
    .expect("second run succeeds");
    let request = second_model.recorded_requests().remove(0);

    assert!(request.contains("- diagnose: Diagnose failing behavior"));
    assert!(!request.contains("- review: Review code changes"));
}

#[test]
fn skill_subflow_rejects_human_input_mixed_with_other_tool_calls() {
    let store = SqliteWorkspaceStore::new(temp_workspace_root());
    let model = TestModel::responses([
        Ok(vec![AssistantContent::ToolCall(test_tool_call(
            "skill_call",
            "call_skill",
            serde_json::json!({
                "name": "review",
                "task": "Review current changes"
            }),
        ))]),
        Ok(vec![
            AssistantContent::ToolCall(test_tool_call(
                "human_call",
                "human_input",
                serde_json::json!({
                    "prompt": "Choose review depth",
                    "allow_free_text": true
                }),
            )),
            AssistantContent::ToolCall(test_tool_call(
                "lua_call",
                "lua",
                serde_json::json!({ "code": "return 'unexpected'" }),
            )),
        ]),
        Ok(vec![AssistantContent::text(
            "Parent handled invalid skill response",
        )]),
    ]);
    let policy = RuntimePolicy::workspace_default(store.root().to_path_buf());
    let context =
        RunLoopContext::new(store.clone(), model.clone(), policy).with_skills(vec![test_skill(
            "review",
            "Review code changes",
            "Review carefully.",
        )]);
    let run_loop = RunLoop::with_context(context);

    let output = futures::executor::block_on(run_loop.run("Use review skill".to_owned()))
        .expect("invalid skill response is returned to parent");
    let requests = model.recorded_requests();

    assert_eq!(output.run.status, RunStatus::Completed);
    assert_eq!(
        output.answer.as_deref(),
        Some("Parent handled invalid skill response")
    );
    assert!(requests[2].contains("human_input must be the only tool call"));
}

#[test]
fn run_loop_resumes_human_input_inside_skill_subflow() {
    let store = SqliteWorkspaceStore::new(temp_workspace_root());
    let model = TestModel::responses([
        Ok(vec![AssistantContent::ToolCall(test_tool_call(
            "skill_call",
            "call_skill",
            serde_json::json!({
                "name": "review",
                "task": "Review current changes"
            }),
        ))]),
        Ok(vec![AssistantContent::ToolCall(test_tool_call(
            "human_call",
            "human_input",
            serde_json::json!({
                "prompt": "Choose review depth",
                "options": [{ "id": "deep", "label": "Deep review" }],
                "allow_free_text": false
            }),
        ))]),
        Ok(vec![AssistantContent::text(
            "Skill completed a deep review",
        )]),
        Ok(vec![AssistantContent::text(
            "Parent received resumed skill result",
        )]),
    ]);
    let policy = RuntimePolicy::workspace_default(store.root().to_path_buf());
    let skill = SkillDefinition {
        name: "review".to_owned(),
        description: "Review code changes".to_owned(),
        content: "Ask for review depth before reviewing.".to_owned(),
        scope: SkillScope::Project,
        path: "/workspace/.jux/skills/review/SKILL.md".into(),
    };
    let context =
        RunLoopContext::new(store.clone(), model.clone(), policy).with_skills(vec![skill]);
    let run_loop = RunLoop::with_context(context);

    let waiting_output = futures::executor::block_on(run_loop.run("Use review skill".to_owned()))
        .expect("skill subflow waits for human input");
    let resumed_output = futures::executor::block_on(run_loop.run("deep".to_owned()))
        .expect("skill subflow resumes and completes");
    let requests = model.recorded_requests();

    assert_eq!(waiting_output.run.status, RunStatus::WaitingForHumanInput);
    assert_eq!(resumed_output.run.id, waiting_output.run.id);
    assert_eq!(resumed_output.run.status, RunStatus::Completed);
    assert_eq!(
        resumed_output.answer.as_deref(),
        Some("Parent received resumed skill result")
    );
    assert_eq!(requests.len(), 4);
    assert!(requests[2].contains("deep"));
    assert!(requests[3].contains("Skill completed a deep review"));
    assert!(!requests[3].contains("Ask for review depth before reviewing."));
    assert!(
        resumed_output
            .steps
            .iter()
            .any(|step| step.kind == StepKind::SkillExecution)
    );
}

#[test]
fn skill_subflow_iteration_limit_spans_human_input_resumes() {
    let store = SqliteWorkspaceStore::new(temp_workspace_root());
    let mut responses = vec![Ok(vec![AssistantContent::ToolCall(test_tool_call(
        "skill_call",
        "call_skill",
        serde_json::json!({
            "name": "review",
            "task": "Review current changes"
        }),
    ))])];
    for index in 1..=8 {
        responses.push(Ok(vec![AssistantContent::ToolCall(test_tool_call(
            &format!("human_call_{index}"),
            "human_input",
            serde_json::json!({
                "prompt": format!("Input {index}"),
                "allow_free_text": true
            }),
        ))]));
    }
    responses.push(Ok(vec![AssistantContent::text("Parent saw skill limit")]));
    responses.push(Ok(vec![AssistantContent::text(
        "Unexpected extra response",
    )]));
    let model = TestModel::responses(responses);
    let policy = RuntimePolicy::workspace_default(store.root().to_path_buf());
    let context =
        RunLoopContext::new(store.clone(), model.clone(), policy).with_skills(vec![test_skill(
            "review",
            "Review code changes",
            "Review carefully.",
        )]);
    let run_loop = RunLoop::with_context(context);

    let mut output = futures::executor::block_on(run_loop.run("Use review skill".to_owned()))
        .expect("skill asks for first input");
    for index in 1..=8 {
        assert_eq!(output.run.status, RunStatus::WaitingForHumanInput);
        output = futures::executor::block_on(run_loop.run(format!("answer {index}")))
            .expect("skill resume succeeds");
    }

    assert_eq!(output.run.status, RunStatus::Completed);
    assert_eq!(output.answer.as_deref(), Some("Parent saw skill limit"));
    assert_eq!(model.recorded_requests().len(), 10);
}

#[test]
fn run_loop_uses_session_history_when_calling_llm() {
    let store = SqliteWorkspaceStore::new(temp_workspace_root());
    let model = TestModel::text_responses(["First answer", "Second answer"]);
    let run_loop = RunLoop::new(store.clone(), model.clone());

    futures::executor::block_on(run_loop.run("First request".to_owned()))
        .expect("first run loop succeeds");
    futures::executor::block_on(run_loop.run("Second request".to_owned()))
        .expect("second run loop succeeds");
    let requests = model.recorded_requests();

    assert_eq!(requests.len(), 2);
    assert!(requests[0].contains("You are Jux"));
    assert!(requests[0].contains("First request"));
    assert!(!requests[0].contains("First answer"));
    assert!(requests[1].contains("You are Jux"));
    assert!(requests[1].contains("First request"));
    assert!(requests[1].contains("First answer"));
    assert!(requests[1].contains("Second request"));
    let context_items = store
        .load_session_context_items(&store.load_active_session().expect("session loads").id)
        .expect("session context loads");
    assert_eq!(context_items.len(), 5);
}

#[test]
fn run_loop_records_reasoning_without_sending_it_back_to_llm() {
    let store = SqliteWorkspaceStore::new(temp_workspace_root());
    let model = TestModel::responses([
        Ok(vec![
            AssistantContent::Reasoning(Reasoning::new("hidden reasoning")),
            AssistantContent::text("Visible answer"),
        ]),
        Ok(vec![AssistantContent::text("Second answer")]),
    ]);
    let run_loop = RunLoop::new(store.clone(), model.clone());

    let first_output = futures::executor::block_on(run_loop.run("First request".to_owned()))
        .expect("first run loop succeeds");
    futures::executor::block_on(run_loop.run("Second request".to_owned()))
        .expect("second run loop succeeds");
    let requests = model.recorded_requests();

    assert_eq!(first_output.answer.as_deref(), Some("Visible answer"));
    assert_eq!(
        first_output
            .steps
            .iter()
            .map(|step| step.kind.clone())
            .collect::<Vec<_>>(),
        vec![StepKind::UserMessage, StepKind::AssistantResponse]
    );
    assert_eq!(
        first_output.steps[1].payload.to_assistant_reasoning(),
        Some("hidden reasoning")
    );
    assert_eq!(
        first_output.steps[1].payload.to_assistant_text(),
        Some("Visible answer")
    );
    assert!(requests[1].contains("Visible answer"));
    assert!(!requests[1].contains("hidden reasoning"));
}
#[test]
fn run_loop_executes_exec_tool_call_and_returns_structured_output() {
    let store = SqliteWorkspaceStore::new(temp_workspace_root());
    let execution_root = temp_workspace_root();
    std::fs::write(execution_root.join("hello.txt"), "hello").expect("fixture file is written");
    let model = TestModel::responses([
        Ok(vec![AssistantContent::ToolCall(test_tool_call(
            "call_1",
            "exec",
            serde_json::json!({ "program": "cat", "args": ["hello.txt"] }),
        ))]),
        Ok(vec![AssistantContent::text("Exec returned hello")]),
    ]);
    let policy = RuntimePolicy::workspace_default(execution_root);
    let context = RunLoopContext::new(store.clone(), model.clone(), policy);
    let run_loop = RunLoop::with_context(context);
    let mut events = VecAgentEventSink::default();

    let output = futures::executor::block_on(
        run_loop.run_with_events("Use the exec tool".to_owned(), &mut events),
    )
    .expect("run loop succeeds");
    let requests = model.recorded_requests();
    let exec_output = output.steps[2]
        .payload
        .to_tool_result_content()
        .expect("tool result exists");

    assert_eq!(output.run.status, RunStatus::Completed);
    assert_eq!(exec_output["success"], true);
    assert_eq!(exec_output["exit_code"], 0);
    assert_eq!(exec_output["stdout"], "hello");
    assert_eq!(exec_output["stderr"], "");
    assert!(requests[0].contains("\"name\":\"exec\""));
    assert!(requests[0].contains("Always use workspace-relative paths"));
    assert!(requests[0].contains("success"));
    assert!(requests[0].contains("exit_code"));
    assert!(requests[1].contains("hello"));
    assert!(events.events.iter().any(|event| matches!(
        &event.data,
        jux_core::AgentEventData::ToolOutputChunk {
            stream: jux_core::ToolOutputStream::Stdout,
            content,
            ..
        } if content == "hello"
    )));
}

#[test]
fn run_loop_normalizes_legacy_workspace_paths_before_sending_session_history() {
    let store = SqliteWorkspaceStore::new(temp_workspace_root());
    let execution_root = temp_workspace_root();
    std::fs::write(execution_root.join("hello.txt"), "hello").expect("fixture file is written");
    let model = TestModel::responses([
        Ok(vec![AssistantContent::ToolCall(test_tool_call(
            "call_1",
            "exec",
            serde_json::json!({ "program": "cat", "args": ["/workspace/hello.txt"] }),
        ))]),
        Ok(vec![AssistantContent::text("First run complete")]),
        Ok(vec![AssistantContent::text("Second run complete")]),
    ]);
    let policy = RuntimePolicy::workspace_default(execution_root);
    let run_loop = RunLoop::with_context(RunLoopContext::new(store, model.clone(), policy));

    futures::executor::block_on(run_loop.run("Read @/workspace/hello.txt".to_owned()))
        .expect("first run succeeds");
    futures::executor::block_on(run_loop.run("Continue".to_owned())).expect("second run succeeds");
    let requests = model.recorded_requests();

    assert!(requests[2].contains("@hello.txt"));
    assert!(requests[2].contains("\"args\":[\"hello.txt\"]"));
    assert!(!requests[2].contains("/workspace/hello.txt"));
}

#[test]
fn run_loop_returns_exec_shell_syntax_errors_to_llm() {
    let store = SqliteWorkspaceStore::new(temp_workspace_root());
    let model = TestModel::responses([
        Ok(vec![AssistantContent::ToolCall(test_tool_call(
            "call_1",
            "exec",
            serde_json::json!({ "program": "ls", "args": [">", "output.txt"] }),
        ))]),
        Ok(vec![AssistantContent::text("Exec shell syntax denied")]),
    ]);
    let run_loop = RunLoop::new(store.clone(), model.clone());

    let output = futures::executor::block_on(run_loop.run("Use exec shell syntax".to_owned()))
        .expect("run loop succeeds");
    let requests = model.recorded_requests();
    let tool_result = output.steps[2]
        .payload
        .to_tool_result_content()
        .expect("tool result exists");

    assert_eq!(output.run.status, RunStatus::Completed);
    assert_eq!(tool_result["success"], false);
    assert!(
        tool_result["error"]
            .as_str()
            .is_some_and(|error| error.contains("shell syntax is not supported: >"))
    );
    assert!(requests[1].contains("shell syntax is not supported"));
}

#[test]
fn run_loop_prepares_code_change_proposal_without_writing_files() {
    let workspace = temp_workspace_root();
    std::fs::write(workspace.join("README.md"), "old\n").expect("source file is written");
    let store = SqliteWorkspaceStore::new(&workspace);
    let model = TestModel::responses([
        Ok(vec![AssistantContent::ToolCall(test_tool_call(
            "call_1",
            "propose_code_change",
            serde_json::json!({
                "plan": {
                    "summary": "Update README",
                    "items": ["Replace the content"]
                },
                "files": [
                    {
                        "path": "README.md",
                        "new_content": "new\n"
                    }
                ]
            }),
        ))]),
        Ok(vec![AssistantContent::text("Proposal ready")]),
    ]);
    let run_loop = RunLoop::new(store, model);

    let output = futures::executor::block_on(run_loop.run("Prepare a README change".to_owned()))
        .expect("run succeeds");
    let proposal = output
        .steps
        .iter()
        .find_map(|step| step.payload.to_tool_result_content())
        .and_then(|content| serde_json::from_value::<CodeChangeProposal>(content.clone()).ok())
        .expect("code change proposal is returned");

    assert_eq!(proposal.plan.summary, "Update README");
    assert_eq!(proposal.files[0].path.as_str(), "README.md");
    assert_eq!(
        std::fs::read_to_string(workspace.join("README.md")).expect("source file loads"),
        "old\n"
    );
}

#[test]
fn run_loop_executes_lua_tool_call_and_continues_until_final_answer() {
    let store = SqliteWorkspaceStore::new(temp_workspace_root());
    let model = TestModel::responses([
        Ok(vec![AssistantContent::ToolCall(test_tool_call(
            "call_1",
            "lua",
            serde_json::json!({ "code": "return 'hello from lua'" }),
        ))]),
        Ok(vec![AssistantContent::text("Lua returned hello from lua")]),
    ]);
    let run_loop = RunLoop::new(store.clone(), model.clone());

    let output = futures::executor::block_on(run_loop.run("Use the lua tool".to_owned()))
        .expect("run loop succeeds");
    let requests = model.recorded_requests();

    assert_eq!(output.run.status, RunStatus::Completed);
    assert_eq!(
        output.answer.as_deref(),
        Some("Lua returned hello from lua")
    );
    assert_eq!(
        output
            .steps
            .iter()
            .map(|step| step.kind.clone())
            .collect::<Vec<_>>(),
        vec![
            StepKind::UserMessage,
            StepKind::AssistantResponse,
            StepKind::ToolResult,
            StepKind::AssistantResponse,
        ]
    );
    assert_eq!(requests.len(), 2);
    assert!(requests[0].contains("You are Jux"));
    assert!(requests[0].contains("Use the lua tool"));
    assert!(requests[1].contains("hello from lua"));
}

#[test]
fn run_loop_waits_for_human_input_and_resumes_same_run() {
    let store = SqliteWorkspaceStore::new(temp_workspace_root());
    let model = TestModel::responses([
        Ok(vec![AssistantContent::ToolCall(test_tool_call(
            "call_1",
            "human_input",
            serde_json::json!({
                "prompt": "Choose an action",
                "options": [{ "id": "continue", "label": "Continue" }],
                "allow_free_text": false
            }),
        ))]),
        Ok(vec![AssistantContent::text("Continued after human input")]),
    ]);
    let run_loop = RunLoop::new(store.clone(), model.clone());

    let waiting_output = futures::executor::block_on(run_loop.run("Ask a human".to_owned()))
        .expect("run loop waits for human input");
    let resumed_output = futures::executor::block_on(run_loop.run("continue".to_owned()))
        .expect("run loop resumes with human input");
    let requests = model.recorded_requests();

    assert_eq!(waiting_output.run.status, RunStatus::WaitingForHumanInput);
    assert_eq!(resumed_output.run.id, waiting_output.run.id);
    assert_eq!(resumed_output.run.status, RunStatus::Completed);
    assert_eq!(
        resumed_output.answer.as_deref(),
        Some("Continued after human input")
    );
    assert_eq!(
        resumed_output
            .steps
            .iter()
            .map(|step| step.kind.clone())
            .collect::<Vec<_>>(),
        vec![
            StepKind::UserMessage,
            StepKind::AssistantResponse,
            StepKind::ToolResult,
            StepKind::AssistantResponse,
        ]
    );
    assert!(
        resumed_output.steps[2]
            .payload
            .to_tool_result_content()
            .is_some_and(|content| content["input"] == "continue")
    );
    assert!(requests[0].contains("\"name\":\"human_input\""));
    assert!(requests[1].contains("continue"));
}

#[test]
fn run_loop_rejects_invalid_human_input_option() {
    let store = SqliteWorkspaceStore::new(temp_workspace_root());
    let model = TestModel::responses([Ok(vec![AssistantContent::ToolCall(test_tool_call(
        "call_1",
        "human_input",
        serde_json::json!({
            "prompt": "Choose an action",
            "options": [{ "id": "continue", "label": "Continue" }],
            "allow_free_text": false
        }),
    ))])]);
    let run_loop = RunLoop::new(store.clone(), model.clone());

    let waiting_output = futures::executor::block_on(run_loop.run("Ask a human".to_owned()))
        .expect("run loop waits for human input");
    let error = futures::executor::block_on(run_loop.run("different".to_owned()))
        .expect_err("invalid human input fails");

    assert_eq!(waiting_output.run.status, RunStatus::WaitingForHumanInput);
    assert!(
        error
            .to_string()
            .contains("must match one of the option ids")
    );
    assert_eq!(model.recorded_requests().len(), 1);
}

#[test]
fn run_loop_streams_hierarchical_events() {
    let store = SqliteWorkspaceStore::new(temp_workspace_root());
    let model = TestModel::responses([
        Ok(vec![AssistantContent::ToolCall(test_tool_call(
            "call_1",
            "lua",
            serde_json::json!({ "code": "return 'streamed lua'" }),
        ))]),
        Ok(vec![AssistantContent::text("Streamed answer")]),
    ]);
    let run_loop = RunLoop::new(store.clone(), model.clone());
    let mut events = VecAgentEventSink::default();

    let output = futures::executor::block_on(
        run_loop.run_with_events("Stream this run".to_owned(), &mut events),
    )
    .expect("run loop succeeds");
    let event_ids = events.ids();

    assert_eq!(output.answer.as_deref(), Some("Streamed answer"));
    assert_eq!(event_ids[0], "run");
    assert!(event_ids.contains(&"run.iteration.1".to_owned()));
    assert!(event_ids.contains(&"run.iteration.1.llm.1".to_owned()));
    assert!(event_ids.contains(&"run.iteration.1.tool.lua.1".to_owned()));
    assert!(events.contains(AgentEventKind::Output, "run.iteration.1.tool.lua.1"));
    assert!(events.contains(AgentEventKind::Completed, "run"));
    assert_eq!(
        events
            .events
            .iter()
            .map(|event| event.sequence)
            .collect::<Vec<_>>(),
        (1..=events.events.len() as u64).collect::<Vec<_>>()
    );
    assert!(events.events.iter().all(|event| event.timestamp > 0));
}

#[test]
fn run_loop_streams_text_deltas_and_persists_the_aggregated_response() {
    let store = SqliteWorkspaceStore::new(temp_workspace_root());
    let model = TestModel::fixed_text("Streamed text");
    let policy = RuntimePolicy::workspace_default(store.root().to_path_buf());
    let context = RunLoopContext::new(store, model, policy).with_model_streaming(true);
    let run_loop = RunLoop::with_context(context);
    let mut events = VecAgentEventSink::default();

    let output = futures::executor::block_on(
        run_loop.run_with_events("Stream content".to_owned(), &mut events),
    )
    .expect("streaming run succeeds");

    assert!(events.events.iter().any(|event| matches!(
        &event.data,
        jux_core::AgentEventData::AssistantTextDelta { content }
            if content == "Streamed text"
    )));
    assert_eq!(output.answer.as_deref(), Some("Streamed text"));
    assert_eq!(
        output.steps[1].payload.to_assistant_text(),
        Some("Streamed text")
    );
}

#[test]
fn run_loop_returns_lua_system_standard_library_access_errors_to_llm() {
    let store = SqliteWorkspaceStore::new(temp_workspace_root());
    let model = TestModel::responses([
        Ok(vec![AssistantContent::ToolCall(test_tool_call(
            "call_1",
            "lua",
            serde_json::json!({ "code": "return os.getenv('HOME')" }),
        ))]),
        Ok(vec![AssistantContent::text("Lua access denied")]),
    ]);
    let run_loop = RunLoop::new(store.clone(), model.clone());

    let output = futures::executor::block_on(run_loop.run("Use the lua os library".to_owned()))
        .expect("run loop succeeds");
    let requests = model.recorded_requests();
    let tool_result = output.steps[2]
        .payload
        .to_tool_result_content()
        .expect("tool result exists");

    assert_eq!(output.run.status, RunStatus::Completed);
    assert_eq!(tool_result["success"], false);
    assert!(
        tool_result["error"]
            .as_str()
            .is_some_and(|error| error.contains("lua execution failed"))
    );
    assert!(requests[1].contains("lua execution failed"));
}

#[test]
fn run_loop_returns_lua_print_errors_to_llm() {
    let store = SqliteWorkspaceStore::new(temp_workspace_root());
    let model = TestModel::responses([
        Ok(vec![AssistantContent::ToolCall(test_tool_call(
            "call_1",
            "lua",
            serde_json::json!({ "code": "print('hello from print')" }),
        ))]),
        Ok(vec![AssistantContent::text("Lua print denied")]),
    ]);
    let run_loop = RunLoop::new(store.clone(), model.clone());

    let output = futures::executor::block_on(run_loop.run("Use lua print".to_owned()))
        .expect("run loop succeeds");
    let requests = model.recorded_requests();
    let tool_result = output.steps[2]
        .payload
        .to_tool_result_content()
        .expect("tool result exists");

    assert_eq!(output.run.status, RunStatus::Completed);
    assert_eq!(tool_result["success"], false);
    assert!(tool_result["error"].as_str().is_some_and(|error| {
        error.contains("print is disabled in the Jux Lua runtime")
            && error.contains("use return to send a tool result")
    }));
    assert!(requests[1].contains("print is disabled"));
}

#[test]
fn run_loop_allows_lua_os_execute_with_single_command() {
    let store = SqliteWorkspaceStore::new(temp_workspace_root());
    let model = TestModel::responses([
        Ok(vec![AssistantContent::ToolCall(test_tool_call(
            "call_1",
            "lua",
            serde_json::json!({ "code": "local ok, kind, code = os.execute('printf hello'); return tostring(ok) .. ':' .. kind .. ':' .. tostring(code)" }),
        ))]),
        Ok(vec![AssistantContent::text("Lua command executed")]),
    ]);
    let run_loop = RunLoop::new(store.clone(), model.clone());

    let output = futures::executor::block_on(run_loop.run("Use lua os.execute".to_owned()))
        .expect("run loop succeeds");
    let requests = model.recorded_requests();

    assert_eq!(output.run.status, RunStatus::Completed);
    assert!(requests[1].contains("true:exit:0"));
}

#[test]
fn run_loop_allows_lua_io_popen_read_all() {
    let store = SqliteWorkspaceStore::new(temp_workspace_root());
    let model = TestModel::responses([
        Ok(vec![AssistantContent::ToolCall(test_tool_call(
            "call_1",
            "lua",
            serde_json::json!({ "code": "local f = io.popen('printf hello', 'r'); local output = f:read('*a'); f:close(); return output" }),
        ))]),
        Ok(vec![AssistantContent::text("Lua popen read output")]),
    ]);
    let run_loop = RunLoop::new(store.clone(), model.clone());

    let output = futures::executor::block_on(run_loop.run("Use lua io.popen".to_owned()))
        .expect("run loop succeeds");
    let requests = model.recorded_requests();

    assert_eq!(output.run.status, RunStatus::Completed);
    assert!(requests[1].contains("hello"));
}

#[test]
fn run_loop_allows_lua_io_popen_lines() {
    let store = SqliteWorkspaceStore::new(temp_workspace_root());
    let model = TestModel::responses([
        Ok(vec![AssistantContent::ToolCall(test_tool_call(
            "call_1",
            "lua",
            serde_json::json!({ "code": "local f = io.popen('printf hello', 'r'); local line = f:lines()(); f:close(); return line" }),
        ))]),
        Ok(vec![AssistantContent::text("Lua popen lines output")]),
    ]);
    let run_loop = RunLoop::new(store.clone(), model.clone());

    let output = futures::executor::block_on(run_loop.run("Use lua io.popen lines".to_owned()))
        .expect("run loop succeeds");
    let requests = model.recorded_requests();

    assert_eq!(output.run.status, RunStatus::Completed);
    assert!(requests[1].contains("hello"));
}

#[test]
fn run_loop_returns_lua_shell_style_command_errors_to_llm() {
    let store = SqliteWorkspaceStore::new(temp_workspace_root());
    let model = TestModel::responses([
        Ok(vec![AssistantContent::ToolCall(test_tool_call(
            "call_1",
            "lua",
            serde_json::json!({ "code": "local f = io.popen('printf hello > output.txt', 'r'); return f:read('*a')" }),
        ))]),
        Ok(vec![AssistantContent::ToolCall(test_tool_call(
            "call_2",
            "lua",
            serde_json::json!({ "code": "local f = io.popen('printf hello', 'r'); return f:read('*a')" }),
        ))]),
        Ok(vec![AssistantContent::text("Lua command recovered")]),
    ]);
    let run_loop = RunLoop::new(store.clone(), model.clone());

    let output = futures::executor::block_on(run_loop.run("Use shell syntax".to_owned()))
        .expect("run loop succeeds");
    let requests = model.recorded_requests();

    assert_eq!(output.run.status, RunStatus::Completed);
    assert_eq!(output.answer.as_deref(), Some("Lua command recovered"));
    let first_tool_result = output.steps[2]
        .payload
        .to_tool_result_content()
        .expect("first tool result exists");
    assert_eq!(first_tool_result["success"], false);
    assert!(
        first_tool_result["error"]
            .as_str()
            .is_some_and(|error| error.contains("shell syntax is not supported: >"))
    );
    assert!(requests[1].contains("shell syntax is not supported"));
    assert!(requests[2].contains("hello"));
}

#[test]
fn run_loop_marks_run_failed_when_llm_fails() {
    let store = SqliteWorkspaceStore::new(temp_workspace_root());
    let model = TestModel::fixed_error("provider failed");
    let run_loop = RunLoop::new(store.clone(), model);

    let error = futures::executor::block_on(run_loop.run("Explain this project".to_owned()))
        .expect_err("run loop fails");
    let run = match error {
        RunLoopError::Prompt { run, .. } => *run,
        RunLoopError::Store(_) | RunLoopError::Runtime { .. } | RunLoopError::Canceled { .. } => {
            panic!("expected prompt error")
        }
    };
    let steps = store.load_run_steps(&run.id).expect("steps load");

    assert_eq!(run.status, RunStatus::Failed);
    assert_eq!(
        steps.last().expect("error step exists").kind,
        StepKind::Error
    );
}

trait StepPayloadTestExt {
    fn to_assistant_text(&self) -> Option<&str>;
    fn to_assistant_reasoning(&self) -> Option<&str>;
    fn to_assistant_usage(&self) -> Option<&LlmUsage>;
    fn to_tool_result_content(&self) -> Option<&serde_json::Value>;
}

impl StepPayloadTestExt for StepPayload {
    fn to_assistant_text(&self) -> Option<&str> {
        match self {
            StepPayload::AssistantResponse { items, .. } => {
                items.iter().find_map(|item| match item {
                    AssistantResponseItem::Text { content } => Some(content.as_str()),
                    _ => None,
                })
            }
            _ => None,
        }
    }

    fn to_assistant_reasoning(&self) -> Option<&str> {
        match self {
            StepPayload::AssistantResponse { items, .. } => {
                items.iter().find_map(|item| match item {
                    AssistantResponseItem::Reasoning { content } => Some(content.as_str()),
                    _ => None,
                })
            }
            _ => None,
        }
    }

    fn to_assistant_usage(&self) -> Option<&LlmUsage> {
        match self {
            StepPayload::AssistantResponse { usage, .. } => Some(usage),
            _ => None,
        }
    }

    fn to_tool_result_content(&self) -> Option<&serde_json::Value> {
        match self {
            StepPayload::ToolResult { content, .. } => Some(content),
            _ => None,
        }
    }
}

trait SessionContextPayloadTestExt {
    fn to_tool_name(&self) -> Option<&str>;
}

impl SessionContextPayloadTestExt for SessionContextPayload {
    fn to_tool_name(&self) -> Option<&str> {
        match self {
            SessionContextPayload::ToolDefinition { name, .. } => Some(name),
            SessionContextPayload::SystemPrompt { .. } => None,
        }
    }
}

#[derive(Default)]
struct VecAgentEventSink {
    events: Vec<AgentEvent>,
}

impl VecAgentEventSink {
    fn ids(&self) -> Vec<String> {
        self.events
            .iter()
            .map(|event| event.id.to_string())
            .collect()
    }

    fn contains(&self, kind: AgentEventKind, id: &str) -> bool {
        self.events
            .iter()
            .any(|event| event.kind == kind && event.id.as_str() == id)
    }
}

impl AgentEventSink for VecAgentEventSink {
    fn emit(&mut self, event: AgentEvent) {
        self.events.push(event);
    }
}

#[derive(Clone, Debug)]
struct TestModel {
    responses: Arc<Mutex<TestResponses>>,
    recorded_requests: Arc<Mutex<Vec<String>>>,
    cancel_on_completion: Option<RunCancellationHandle>,
    cancel_on_stream: Option<RunCancellationHandle>,
}

type TestResponses = Vec<Result<Vec<AssistantContent>, String>>;

impl TestModel {
    fn fixed_text(response: impl Into<String>) -> Self {
        Self::text_responses([response.into()])
    }

    fn text_responses(responses: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self::responses(
            responses
                .into_iter()
                .map(|response| Ok(vec![AssistantContent::text(response.into())])),
        )
    }

    fn responses(
        responses: impl IntoIterator<Item = Result<Vec<AssistantContent>, String>>,
    ) -> Self {
        Self {
            responses: Arc::new(Mutex::new(responses.into_iter().collect())),
            recorded_requests: Arc::new(Mutex::new(Vec::new())),
            cancel_on_completion: None,
            cancel_on_stream: None,
        }
    }

    fn fixed_error(message: impl Into<String>) -> Self {
        Self {
            responses: Arc::new(Mutex::new(vec![Err(message.into())])),
            recorded_requests: Arc::new(Mutex::new(Vec::new())),
            cancel_on_completion: None,
            cancel_on_stream: None,
        }
    }

    fn canceling(handle: RunCancellationHandle) -> Self {
        Self {
            responses: Arc::new(Mutex::new(Vec::new())),
            recorded_requests: Arc::new(Mutex::new(Vec::new())),
            cancel_on_completion: Some(handle),
            cancel_on_stream: None,
        }
    }

    fn canceling_stream(handle: RunCancellationHandle) -> Self {
        Self {
            responses: Arc::new(Mutex::new(Vec::new())),
            recorded_requests: Arc::new(Mutex::new(Vec::new())),
            cancel_on_completion: None,
            cancel_on_stream: Some(handle),
        }
    }

    fn recorded_requests(&self) -> Vec<String> {
        self.recorded_requests
            .lock()
            .expect("recorded requests lock is available")
            .clone()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct TestStreamingResponse;

impl GetTokenUsage for TestStreamingResponse {
    fn token_usage(&self) -> Option<Usage> {
        None
    }
}

impl CompletionModel for TestModel {
    type Response = serde_json::Value;
    type StreamingResponse = TestStreamingResponse;
    type Client = ();

    fn make(_client: &Self::Client, _model: impl Into<String>) -> Self {
        Self::fixed_text("")
    }

    async fn completion(
        &self,
        request: CompletionRequest,
    ) -> Result<CompletionResponse<Self::Response>, CompletionError> {
        let request_json = serde_json::to_string(&request).expect("request serializes");
        self.recorded_requests
            .lock()
            .expect("recorded requests lock is available")
            .push(request_json);

        if let Some(handle) = &self.cancel_on_completion {
            handle.cancel();
            futures::future::pending().await
        }

        let response = self
            .responses
            .lock()
            .expect("responses lock is available")
            .remove(0);

        match response {
            Ok(content) => Ok(CompletionResponse {
                choice: OneOrMany::many(content).expect("test response has at least one choice"),
                usage: Usage::new(),
                raw_response: serde_json::Value::Null,
                message_id: None,
            }),
            Err(message) => Err(CompletionError::ProviderError(message)),
        }
    }

    async fn stream(
        &self,
        request: CompletionRequest,
    ) -> Result<StreamingCompletionResponse<Self::StreamingResponse>, CompletionError> {
        let request_json = serde_json::to_string(&request).expect("request serializes");
        self.recorded_requests
            .lock()
            .expect("recorded requests lock is available")
            .push(request_json);
        if let Some(handle) = &self.cancel_on_stream {
            let handle = handle.clone();
            let chunks = futures::stream::unfold(0_u8, move |state| {
                let handle = handle.clone();
                async move {
                    match state {
                        0 => Some((Ok(RawStreamingChoice::Message("partial".to_owned())), 1)),
                        _ => {
                            handle.cancel();
                            futures::future::pending().await
                        }
                    }
                }
            });
            return Ok(StreamingCompletionResponse::stream(Box::pin(chunks)));
        }
        let response = self
            .responses
            .lock()
            .expect("responses lock is available")
            .remove(0)
            .map_err(CompletionError::ProviderError)?;
        let mut chunks = Vec::new();
        for content in response {
            match content {
                AssistantContent::Text(text) => {
                    chunks.push(Ok(RawStreamingChoice::Message(text.text)));
                }
                _ => {
                    return Err(CompletionError::ProviderError(
                        "test streaming only supports text".to_owned(),
                    ));
                }
            }
        }
        chunks.push(Ok(RawStreamingChoice::FinalResponse(TestStreamingResponse)));
        Ok(StreamingCompletionResponse::stream(Box::pin(
            futures::stream::iter(chunks),
        )))
    }
}

fn test_tool_call(id: &str, name: &str, arguments: serde_json::Value) -> ToolCall {
    ToolCall::new(id.to_owned(), ToolFunction::new(name.to_owned(), arguments))
}

fn test_skill(name: &str, description: &str, content: &str) -> SkillDefinition {
    SkillDefinition {
        name: name.to_owned(),
        description: description.to_owned(),
        content: content.to_owned(),
        scope: SkillScope::Project,
        path: format!("/workspace/.jux/skills/{name}/SKILL.md").into(),
    }
}

fn temp_workspace_root() -> std::path::PathBuf {
    let root = std::env::temp_dir().join(format!("jux-core-test-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).expect("temp workspace root is created");
    root
}
