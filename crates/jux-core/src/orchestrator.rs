use crate::model::{
    LlmMessageRole, Run, RunStatus, Session, Step, StepKind, StepPayload, Workspace,
};
use crate::store::{SqliteWorkspaceStore, StoreError};
use mlua::{Lua, Value};
use rig::completion::{Chat, PromptError};
use rig::message::{AssistantContent, Message, UserContent};
use serde::Deserialize;
use std::error::Error;
use std::fmt::{self, Display};

const MAX_LOOP_ITERATIONS: usize = 8;
const ECHO_TOOL_NAME: &str = "echo";
const LUA_TOOL_NAME: &str = "lua";
pub const SYSTEM_PROMPT: &str = "You are Jux, a concise coding agent.\n\
Return JSON only. Use exactly one of these shapes:\n\
{\"type\":\"final_answer\",\"answer\":\"...\"}\n\
{\"type\":\"tool_call\",\"tool_name\":\"echo\",\"input\":\"...\"}\n\
Available tools:\n\
- echo: returns the input text unchanged.\n\
- lua: executes the input as Lua code and returns the first returned value.";

pub struct RunLoop<P> {
    store: SqliteWorkspaceStore,
    chat: P,
}

impl<P> RunLoop<P>
where
    P: Chat,
{
    #[must_use]
    pub fn new(store: SqliteWorkspaceStore, chat: P) -> Self {
        Self { store, chat }
    }

    pub async fn run(&self, request: String) -> Result<RunLoopOutput, RunLoopError> {
        let run = self.store.create_run(request.clone())?;
        tracing::info!(run_id = %run.id, "created run");
        self.store.append_step(
            &run.id,
            StepKind::LlmMessage,
            StepPayload::LlmMessage {
                role: LlmMessageRole::System,
                content: SYSTEM_PROMPT.to_owned(),
            },
        )?;
        self.store.append_step(
            &run.id,
            StepKind::LlmMessage,
            StepPayload::LlmMessage {
                role: LlmMessageRole::User,
                content: request,
            },
        )?;

        for _ in 0..MAX_LOOP_ITERATIONS {
            let llm_request = self.build_chat_request(&run)?;
            tracing::debug!(
                run_id = %run.id,
                history_len = llm_request.history.len(),
                "built llm chat request"
            );

            let response = match self
                .chat
                .chat(llm_request.prompt.clone(), llm_request.history.clone())
                .await
            {
                Ok(response) => response,
                Err(error) => return self.fail_prompt_call(run, error),
            };

            self.store.append_step(
                &run.id,
                StepKind::LlmMessage,
                StepPayload::LlmMessage {
                    role: LlmMessageRole::Assistant,
                    content: response.clone(),
                },
            )?;

            match LlmDecision::parse(&response) {
                Ok(LlmDecision::FinalAnswer { answer }) => {
                    return self.complete_run(run, answer);
                }
                Ok(LlmDecision::ToolCall { tool_name, input }) => {
                    tracing::info!(run_id = %run.id, tool_name = %tool_name, "executing tool call");

                    let output = match execute_tool(&tool_name, &input) {
                        Ok(output) => output,
                        Err(error) => return self.fail_runtime(run, error),
                    };
                    self.store.append_step(
                        &run.id,
                        StepKind::LlmMessage,
                        StepPayload::LlmMessage {
                            role: LlmMessageRole::Tool,
                            content: format!("Tool {tool_name}: {output}"),
                        },
                    )?;
                }
                Err(error) => return self.fail_runtime(run, error),
            }
        }

        self.fail_runtime(
            run,
            "run loop reached the maximum number of iterations".to_owned(),
        )
    }

    fn complete_run(&self, run: Run, answer: String) -> Result<RunLoopOutput, RunLoopError> {
        let run = self
            .store
            .update_run_status(&run.id, RunStatus::Completed)?;
        let workspace = self.store.load_workspace()?;
        let session = self.store.load_session(&run.id.session_id())?;
        let steps = self.store.load_run_steps(&run.id)?;
        tracing::info!(
            run_id = %run.id,
            step_count = steps.len(),
            "completed run"
        );

        Ok(RunLoopOutput {
            workspace,
            session,
            run,
            steps,
            answer: Some(answer),
        })
    }

    fn fail_prompt_call(
        &self,
        run: Run,
        error: PromptError,
    ) -> Result<RunLoopOutput, RunLoopError> {
        let message = error.to_string();
        self.store
            .append_step(&run.id, StepKind::Error, StepPayload::Error { message })?;

        let run = self.store.update_run_status(&run.id, RunStatus::Failed)?;
        tracing::error!(run_id = %run.id, "failed run");
        Err(RunLoopError::Prompt {
            run: Box::new(run),
            source: Box::new(error),
        })
    }

    fn fail_runtime(&self, run: Run, message: String) -> Result<RunLoopOutput, RunLoopError> {
        self.record_runtime_error(&run, message.clone())?;
        let run = self.store.update_run_status(&run.id, RunStatus::Failed)?;
        tracing::error!(run_id = %run.id, "failed run");
        Err(RunLoopError::Runtime {
            run: Box::new(run),
            message,
        })
    }

    fn record_runtime_error(&self, run: &Run, message: String) -> Result<(), RunLoopError> {
        self.store
            .append_step(&run.id, StepKind::Error, StepPayload::Error { message })?;
        Ok(())
    }

    fn build_chat_request(&self, run: &Run) -> Result<LlmChatRequest, RunLoopError> {
        let steps = self.store.load_session_steps(&run.id.session_id())?;
        let messages = steps
            .iter()
            .filter(|step| step.visible_to_llm())
            .filter_map(step_to_chat_message)
            .collect::<Vec<_>>();

        let Some((prompt, history)) = messages.split_last() else {
            return Err(RunLoopError::Runtime {
                run: Box::new(run.clone()),
                message: "run has no visible message for the LLM".to_owned(),
            });
        };
        let history = history.to_vec();

        Ok(LlmChatRequest {
            prompt: prompt.clone(),
            history,
        })
    }
}

#[derive(Clone, Debug)]
struct LlmChatRequest {
    prompt: Message,
    history: Vec<Message>,
}

fn step_to_chat_message(step: &Step) -> Option<Message> {
    match &step.payload {
        StepPayload::LlmMessage { role, content } => match role {
            LlmMessageRole::System => Some(Message::System {
                content: content.clone(),
            }),
            LlmMessageRole::User => Some(UserContent::text(content.clone()).into()),
            LlmMessageRole::Assistant => Some(AssistantContent::text(content.clone()).into()),
            LlmMessageRole::Tool => Some(UserContent::text(content.clone()).into()),
        },
        _ => None,
    }
}

#[derive(Debug, Deserialize, Eq, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
enum LlmDecision {
    FinalAnswer { answer: String },
    ToolCall { tool_name: String, input: String },
}

impl LlmDecision {
    fn parse(response: &str) -> Result<Self, String> {
        serde_json::from_str(response).map_err(|error| {
            format!("LLM response must be a valid Jux decision JSON object: {error}")
        })
    }
}

fn execute_tool(tool_name: &str, input: &str) -> Result<String, String> {
    match tool_name {
        ECHO_TOOL_NAME => Ok(input.to_owned()),
        LUA_TOOL_NAME => execute_lua(input),
        _ => Err(format!("unsupported tool call: {tool_name}")),
    }
}

fn execute_lua(script: &str) -> Result<String, String> {
    let lua = Lua::new();
    let value = lua
        .load(script)
        .eval::<Value>()
        .map_err(|error| format!("lua execution failed: {error}"))?;
    lua_value_to_string(value).map_err(|error| format!("lua result conversion failed: {error}"))
}

fn lua_value_to_string(value: Value) -> mlua::Result<String> {
    match value {
        Value::Nil => Ok("nil".to_owned()),
        Value::Boolean(value) => Ok(value.to_string()),
        Value::Integer(value) => Ok(value.to_string()),
        Value::Number(value) => Ok(value.to_string()),
        Value::String(value) => Ok(value.to_string_lossy()),
        other => Ok(format!("{other:?}")),
    }
}

#[derive(Clone, Debug)]
pub struct RunLoopOutput {
    pub workspace: Workspace,
    pub session: Session,
    pub run: Run,
    pub steps: Vec<Step>,
    pub answer: Option<String>,
}

#[derive(Debug)]
pub enum RunLoopError {
    Store(StoreError),
    Prompt {
        run: Box<Run>,
        source: Box<PromptError>,
    },
    Runtime {
        run: Box<Run>,
        message: String,
    },
}

impl Display for RunLoopError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Store(error) => write!(formatter, "run loop store error: {error}"),
            Self::Prompt { source, .. } => write!(formatter, "run loop prompt error: {source}"),
            Self::Runtime { message, .. } => write!(formatter, "run loop runtime error: {message}"),
        }
    }
}

impl Error for RunLoopError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Store(error) => Some(error),
            Self::Prompt { source, .. } => Some(source),
            Self::Runtime { .. } => None,
        }
    }
}

impl From<StoreError> for RunLoopError {
    fn from(error: StoreError) -> Self {
        Self::Store(error)
    }
}
