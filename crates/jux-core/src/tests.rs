use super::*;
use rig::OneOrMany;
use rig::completion::{
    CompletionError, CompletionModel, CompletionRequest, CompletionResponse, GetTokenUsage, Usage,
};
use rig::message::{AssistantContent, ToolCall, ToolFunction};
use rig::streaming::StreamingCompletionResponse;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};

#[test]
fn exposes_package_version() {
    assert_eq!(version(), "0.1.0");
}

#[test]
fn ids_derive_parent_ids_from_hierarchical_segments() {
    let workspace_id = WorkspaceId::from("8f3a".to_owned());
    let session_id = SessionId::new(&workspace_id, 1);
    let run_id = RunId::new(&session_id, 1);
    let step_id = StepId::new(&run_id, 1);

    assert_eq!(session_id.to_string(), "8f3a-0001");
    assert_eq!(run_id.to_string(), "8f3a-0001-000001");
    assert_eq!(step_id.to_string(), "8f3a-0001-000001-000001");
    assert_eq!(step_id.run_id(), run_id);
    assert_eq!(step_id.session_id(), session_id);
    assert_eq!(step_id.workspace_id(), workspace_id);
}

#[test]
fn sqlite_store_persists_workspace_session_run_and_ordered_steps() {
    let store = SqliteWorkspaceStore::new(temp_workspace_root());

    let workspace = store.init_workspace().expect("workspace initializes");
    let session = store
        .load_active_session()
        .expect("active session can be loaded");
    let run = store
        .create_run("Explain this project".to_owned())
        .expect("run is created");
    let second_run = store
        .create_run("Explain the second run".to_owned())
        .expect("second run is created");

    let first_step = store
        .append_step(
            &run.id,
            StepKind::UserMessage,
            StepPayload::UserMessage {
                content: "Explain this project".to_owned(),
            },
        )
        .expect("first step is saved");
    let second_step = store
        .append_step(
            &run.id,
            StepKind::AssistantMessage,
            StepPayload::AssistantMessage {
                content: "Done".to_owned(),
            },
        )
        .expect("second step is saved");
    let steps = store.load_run_steps(&run.id).expect("steps load");

    assert_eq!(workspace.active_session_id, session.id);
    assert_eq!(run.id.session_id(), session.id);
    assert_eq!(run.id.to_string(), format!("{}-000001", session.id));
    assert_eq!(second_run.id.to_string(), format!("{}-000002", session.id));
    assert_eq!(first_step.id.to_string(), format!("{}-000001", run.id));
    assert_eq!(second_step.id.to_string(), format!("{}-000002", run.id));
    assert_eq!(steps, vec![first_step, second_step]);

    let connection = rusqlite::Connection::open(store.database_path()).expect("database opens");
    let sequence_table_count: u64 = connection
        .query_row(
            "select count(*) from sqlite_master where type = 'table' and name = 'sequences'",
            [],
            |row| row.get(0),
        )
        .expect("schema table count can be queried");
    assert_eq!(sequence_table_count, 0);
}

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
        vec![
            StepKind::SystemMessage,
            StepKind::LlmToolDefinition,
            StepKind::LlmToolDefinition,
            StepKind::UserMessage,
            StepKind::AssistantMessage,
        ]
    );
    assert_eq!(
        output.steps[0].payload,
        StepPayload::SystemMessage {
            content: SYSTEM_PROMPT.to_owned(),
        }
    );
    assert_eq!(
        output.steps[3].payload,
        StepPayload::UserMessage {
            content: "Explain this project".to_owned(),
        }
    );
    assert_eq!(output.steps[1].payload.to_tool_name(), Some("echo"));
    assert_eq!(output.steps[2].payload.to_tool_name(), Some("lua"));
    assert_eq!(requests.len(), 1);
    assert!(requests[0].contains("You are Jux"));
    assert!(requests[0].contains("Explain this project"));
    assert!(requests[0].contains("\"tools\""));
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
}

#[test]
fn run_loop_executes_echo_tool_call_and_continues_until_final_answer() {
    let store = SqliteWorkspaceStore::new(temp_workspace_root());
    let model = TestModel::responses([
        Ok(vec![AssistantContent::ToolCall(test_tool_call(
            "call_1",
            "echo",
            serde_json::json!({ "input": "hello from tool" }),
        ))]),
        Ok(vec![AssistantContent::text(
            "Tool returned hello from tool",
        )]),
    ]);
    let run_loop = RunLoop::new(store.clone(), model.clone());

    let output = futures::executor::block_on(run_loop.run("Use the echo tool".to_owned()))
        .expect("run loop succeeds");
    let requests = model.recorded_requests();

    assert_eq!(output.run.status, RunStatus::Completed);
    assert_eq!(
        output.answer.as_deref(),
        Some("Tool returned hello from tool")
    );
    assert_eq!(
        output
            .steps
            .iter()
            .map(|step| step.kind.clone())
            .collect::<Vec<_>>(),
        vec![
            StepKind::SystemMessage,
            StepKind::LlmToolDefinition,
            StepKind::LlmToolDefinition,
            StepKind::UserMessage,
            StepKind::AssistantToolCall,
            StepKind::ToolResult,
            StepKind::AssistantMessage,
        ]
    );
    assert_eq!(output.steps[4].payload.to_tool_call_name(), Some("echo"));
    assert_eq!(
        output.steps[5].payload.to_tool_result_content(),
        Some("hello from tool")
    );
    assert_eq!(requests.len(), 2);
    assert!(requests[0].contains("You are Jux"));
    assert!(requests[0].contains("Use the echo tool"));
    assert!(requests[1].contains("hello from tool"));
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
            StepKind::SystemMessage,
            StepKind::LlmToolDefinition,
            StepKind::LlmToolDefinition,
            StepKind::UserMessage,
            StepKind::AssistantToolCall,
            StepKind::ToolResult,
            StepKind::AssistantMessage,
        ]
    );
    assert_eq!(requests.len(), 2);
    assert!(requests[0].contains("You are Jux"));
    assert!(requests[0].contains("Use the lua tool"));
    assert!(requests[1].contains("hello from lua"));
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
    fn to_tool_name(&self) -> Option<&str>;
    fn to_tool_call_name(&self) -> Option<&str>;
    fn to_tool_result_content(&self) -> Option<&str>;
}

impl StepPayloadTestExt for StepPayload {
    fn to_tool_name(&self) -> Option<&str> {
        match self {
            StepPayload::LlmToolDefinition { name, .. } => Some(name),
            _ => None,
        }
    }

    fn to_tool_call_name(&self) -> Option<&str> {
        match self {
            StepPayload::AssistantToolCall { name, .. } => Some(name),
            _ => None,
        }
    }

    fn to_tool_result_content(&self) -> Option<&str> {
        match self {
            StepPayload::ToolResult { content, .. } => Some(content),
            _ => None,
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
