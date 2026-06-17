use jux_core::{
    AssistantResponseItem, LlmUsage, RunLoop, RunLoopError, RunStatus, SYSTEM_PROMPT,
    SessionContextKind, SessionContextPayload, SqliteWorkspaceStore, StepKind, StepPayload,
};
use rig::OneOrMany;
use rig::completion::{
    CompletionError, CompletionModel, CompletionRequest, CompletionResponse, GetTokenUsage, Usage,
};
use rig::message::{AssistantContent, Reasoning, ToolCall, ToolFunction};
use rig::streaming::StreamingCompletionResponse;
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
    assert_eq!(context_items.len(), 3);
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
    assert_eq!(context_items.len(), 3);
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
    let model = TestModel::responses([
        Ok(vec![AssistantContent::ToolCall(test_tool_call(
            "call_1",
            "exec",
            serde_json::json!({ "program": "printf", "args": ["hello"] }),
        ))]),
        Ok(vec![AssistantContent::text("Exec returned hello")]),
    ]);
    let run_loop = RunLoop::new(store.clone(), model.clone());

    let output = futures::executor::block_on(run_loop.run("Use the exec tool".to_owned()))
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
    assert!(requests[0].contains("success"));
    assert!(requests[0].contains("exit_code"));
    assert!(requests[1].contains("hello"));
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
        RunLoopError::Store(_) | RunLoopError::Runtime { .. } => panic!("expected prompt error"),
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

#[derive(Clone, Debug)]
struct TestModel {
    responses: Arc<Mutex<TestResponses>>,
    recorded_requests: Arc<Mutex<Vec<String>>>,
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
        }
    }

    fn fixed_error(message: impl Into<String>) -> Self {
        Self {
            responses: Arc::new(Mutex::new(vec![Err(message.into())])),
            recorded_requests: Arc::new(Mutex::new(Vec::new())),
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
        _request: CompletionRequest,
    ) -> Result<StreamingCompletionResponse<Self::StreamingResponse>, CompletionError> {
        Err(CompletionError::ProviderError(
            "test streaming is not implemented".to_owned(),
        ))
    }
}

fn test_tool_call(id: &str, name: &str, arguments: serde_json::Value) -> ToolCall {
    ToolCall::new(id.to_owned(), ToolFunction::new(name.to_owned(), arguments))
}

fn temp_workspace_root() -> std::path::PathBuf {
    let root = std::env::temp_dir().join(format!("jux-core-test-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).expect("temp workspace root is created");
    root
}
