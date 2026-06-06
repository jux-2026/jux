use super::*;
use rig::completion::{Prompt, PromptError};
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
            StepKind::UserRequest,
            StepPayload::UserRequest {
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
    let prompt = TestPrompt::fixed_response(r#"{"type":"final_answer","answer":"Mocked answer"}"#);
    let run_loop = RunLoop::new(store.clone(), prompt.clone());

    let output = futures::executor::block_on(run_loop.run("Explain this project".to_owned()))
        .expect("run loop succeeds");
    let prompts = prompt.recorded_prompts();

    assert_eq!(output.run.status, RunStatus::Completed);
    assert_eq!(output.answer.as_deref(), Some("Mocked answer"));
    assert_eq!(
        output
            .steps
            .iter()
            .map(|step| step.kind.clone())
            .collect::<Vec<_>>(),
        vec![
            StepKind::UserRequest,
            StepKind::LlmCall,
            StepKind::AssistantMessage,
        ]
    );
    assert_eq!(prompts.len(), 1);
    assert!(prompts[0].contains("User: Explain this project"));
}

#[test]
fn run_loop_executes_echo_tool_call_and_continues_until_final_answer() {
    let store = SqliteWorkspaceStore::new(temp_workspace_root());
    let prompt = TestPrompt::responses([
        r#"{"type":"tool_call","tool_name":"echo","input":"hello from tool"}"#,
        r#"{"type":"final_answer","answer":"Tool returned hello from tool"}"#,
    ]);
    let run_loop = RunLoop::new(store.clone(), prompt.clone());

    let output = futures::executor::block_on(run_loop.run("Use the echo tool".to_owned()))
        .expect("run loop succeeds");
    let prompts = prompt.recorded_prompts();

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
            StepKind::UserRequest,
            StepKind::LlmCall,
            StepKind::AssistantToolCall,
            StepKind::ToolResult,
            StepKind::LlmCall,
            StepKind::AssistantMessage,
        ]
    );
    assert_eq!(prompts.len(), 2);
    assert!(prompts[0].contains("User: Use the echo tool"));
    assert!(prompts[1].contains("Tool echo: hello from tool"));
}

#[test]
fn run_loop_marks_run_failed_when_llm_fails() {
    let store = SqliteWorkspaceStore::new(temp_workspace_root());
    let prompt = TestPrompt::fixed_error("provider failed");
    let run_loop = RunLoop::new(store.clone(), prompt);

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

#[derive(Clone, Debug)]
struct TestPrompt {
    responses: Arc<Mutex<Vec<Result<String, String>>>>,
    recorded_prompts: Arc<Mutex<Vec<String>>>,
}

impl TestPrompt {
    fn fixed_response(response: impl Into<String>) -> Self {
        Self::responses([response.into()])
    }

    fn responses(responses: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            responses: Arc::new(Mutex::new(
                responses
                    .into_iter()
                    .map(|response| Ok(response.into()))
                    .collect(),
            )),
            recorded_prompts: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn fixed_error(message: impl Into<String>) -> Self {
        Self {
            responses: Arc::new(Mutex::new(vec![Err(message.into())])),
            recorded_prompts: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn recorded_prompts(&self) -> Vec<String> {
        self.recorded_prompts
            .lock()
            .expect("recorded prompts lock is available")
            .clone()
    }
}

impl Prompt for TestPrompt {
    #[allow(refining_impl_trait)]
    async fn prompt(
        &self,
        prompt: impl Into<rig::message::Message>,
    ) -> Result<String, PromptError> {
        let prompt = prompt.into();
        let prompt_json = serde_json::to_string(&prompt).expect("prompt is serializable");
        self.recorded_prompts
            .lock()
            .expect("recorded prompts lock is available")
            .push(prompt_json);

        let response = self
            .responses
            .lock()
            .expect("responses lock is available")
            .remove(0);

        match response {
            Ok(content) => Ok(content.clone()),
            Err(message) => Err(PromptError::PromptCancelled {
                chat_history: Vec::new(),
                reason: message,
            }),
        }
    }
}

fn temp_workspace_root() -> std::path::PathBuf {
    let root = std::env::temp_dir().join(format!("jux-core-test-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).expect("temp workspace root is created");
    root
}
