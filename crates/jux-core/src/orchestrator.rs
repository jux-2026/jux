use crate::model::{Run, RunStatus, Session, Step, StepKind, StepPayload, Workspace};
use crate::store::{SqliteWorkspaceStore, StoreError};
use mlua::{Lua, Value};
use rig::completion::{CompletionError, CompletionModel, ToolDefinition};
use rig::message::{
    AssistantContent, Message, ToolCall, ToolFunction, ToolResult, ToolResultContent,
};
use serde::Deserialize;
use serde_json::json;
use std::error::Error;
use std::fmt::{self, Display};

const MAX_LOOP_ITERATIONS: usize = 8;
const ECHO_TOOL_NAME: &str = "echo";
const LUA_TOOL_NAME: &str = "lua";
pub const SYSTEM_PROMPT: &str = "You are Jux, a concise coding agent.";

pub struct RunLoop<M> {
    store: SqliteWorkspaceStore,
    model: M,
}

impl<M> RunLoop<M>
where
    M: CompletionModel,
{
    #[must_use]
    pub fn new(store: SqliteWorkspaceStore, model: M) -> Self {
        Self { store, model }
    }

    pub async fn run(&self, request: String) -> Result<RunLoopOutput, RunLoopError> {
        let run = self.store.create_run(request.clone())?;
        tracing::info!(run_id = %run.id, "created run");
        self.store.append_step(
            &run.id,
            StepKind::SystemMessage,
            StepPayload::SystemMessage {
                content: SYSTEM_PROMPT.to_owned(),
            },
        )?;
        self.record_tool_definitions(&run)?;
        self.store.append_step(
            &run.id,
            StepKind::UserMessage,
            StepPayload::UserMessage { content: request },
        )?;

        for _ in 0..MAX_LOOP_ITERATIONS {
            let llm_request = self.build_completion_request(&run)?;
            tracing::debug!(
                run_id = %run.id,
                history_len = llm_request.history.len(),
                "built llm completion request"
            );

            let request = self
                .model
                .completion_request(llm_request.prompt)
                .messages(llm_request.history)
                .tools(llm_request.tools)
                .build();
            let response = match self.model.completion(request).await {
                Ok(response) => response,
                Err(error) => return self.fail_completion_call(run, error),
            };

            let mut final_answer = String::new();
            let mut has_tool_call = false;
            for content in response.choice {
                match content {
                    AssistantContent::Text(text) => {
                        final_answer.push_str(&text.text);
                        self.store.append_step(
                            &run.id,
                            StepKind::AssistantMessage,
                            StepPayload::AssistantMessage { content: text.text },
                        )?;
                    }
                    AssistantContent::ToolCall(tool_call) => {
                        has_tool_call = true;
                        self.store.append_step(
                            &run.id,
                            StepKind::AssistantToolCall,
                            StepPayload::AssistantToolCall {
                                id: tool_call.id.clone(),
                                call_id: tool_call.call_id.clone(),
                                name: tool_call.function.name.clone(),
                                arguments: tool_call.function.arguments.clone(),
                            },
                        )?;
                        let tool_name = tool_call.function.name.clone();
                        let args = tool_call.function.arguments.clone();
                        tracing::info!(
                            run_id = %run.id,
                            tool_name = %tool_name,
                            "executing tool call"
                        );

                        let output = match execute_tool(&tool_name, &args) {
                            Ok(output) => output,
                            Err(error) => return self.fail_runtime(run, error),
                        };
                        self.store.append_step(
                            &run.id,
                            StepKind::ToolResult,
                            StepPayload::ToolResult {
                                id: tool_call.id,
                                call_id: tool_call.call_id,
                                content: output,
                            },
                        )?;
                    }
                    AssistantContent::Reasoning(reasoning) => {
                        let text = reasoning.display_text();
                        if !text.is_empty() {
                            self.store.append_step(
                                &run.id,
                                StepKind::AssistantMessage,
                                StepPayload::AssistantMessage { content: text },
                            )?;
                        }
                    }
                    AssistantContent::Image(_) => {
                        return self.fail_runtime(
                            run,
                            "LLM returned an unsupported image response".to_owned(),
                        );
                    }
                }
            }

            if !has_tool_call {
                return self.complete_run(run, final_answer);
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

    fn fail_completion_call(
        &self,
        run: Run,
        error: CompletionError,
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

    fn record_tool_definitions(&self, run: &Run) -> Result<(), RunLoopError> {
        for tool in jux_tool_definitions() {
            self.store.append_step(
                &run.id,
                StepKind::LlmToolDefinition,
                StepPayload::LlmToolDefinition {
                    name: tool.name,
                    description: tool.description,
                    parameters: tool.parameters,
                },
            )?;
        }
        Ok(())
    }

    fn build_completion_request(&self, run: &Run) -> Result<LlmCompletionRequest, RunLoopError> {
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
        let tools = self.load_run_tool_definitions(run)?;

        Ok(LlmCompletionRequest {
            prompt: prompt.clone(),
            history,
            tools,
        })
    }

    fn load_run_tool_definitions(&self, run: &Run) -> Result<Vec<ToolDefinition>, RunLoopError> {
        self.store
            .load_run_steps(&run.id)?
            .iter()
            .filter_map(step_to_tool_definition)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|message| RunLoopError::Runtime {
                run: Box::new(run.clone()),
                message,
            })
    }
}

#[derive(Clone, Debug)]
struct LlmCompletionRequest {
    prompt: Message,
    history: Vec<Message>,
    tools: Vec<ToolDefinition>,
}

fn step_to_chat_message(step: &Step) -> Option<Message> {
    match &step.payload {
        StepPayload::SystemMessage { content } => Some(Message::System {
            content: content.clone(),
        }),
        StepPayload::UserMessage { content } => Some(Message::user(content.clone())),
        StepPayload::AssistantMessage { content } => Some(Message::assistant(content.clone())),
        StepPayload::AssistantToolCall {
            id,
            call_id,
            name,
            arguments,
        } => {
            let tool_call = ToolCall::new(
                id.clone(),
                ToolFunction::new(name.clone(), arguments.clone()),
            );
            let tool_call = match call_id {
                Some(call_id) => tool_call.with_call_id(call_id.clone()),
                None => tool_call,
            };
            Some(tool_call.into())
        }
        StepPayload::ToolResult {
            id,
            call_id,
            content,
        } => Some(Message::User {
            content: rig::OneOrMany::one(rig::message::UserContent::ToolResult(ToolResult {
                id: id.clone(),
                call_id: call_id.clone(),
                content: rig::OneOrMany::one(ToolResultContent::text(content.clone())),
            })),
        }),
        StepPayload::LlmToolDefinition { .. } | StepPayload::Error { .. } => None,
    }
}

fn step_to_tool_definition(step: &Step) -> Option<Result<ToolDefinition, String>> {
    match &step.payload {
        StepPayload::LlmToolDefinition {
            name,
            description,
            parameters,
        } => Some(Ok(ToolDefinition {
            name: name.clone(),
            description: description.clone(),
            parameters: parameters.clone(),
        })),
        _ => None,
    }
}

fn jux_tool_definitions() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: ECHO_TOOL_NAME.to_owned(),
            description: "Return the input text unchanged.".to_owned(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "input": { "type": "string" }
                },
                "required": ["input"]
            }),
        },
        ToolDefinition {
            name: LUA_TOOL_NAME.to_owned(),
            description: "Execute Lua code and return the first returned value.".to_owned(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "code": { "type": "string" }
                },
                "required": ["code"]
            }),
        },
    ]
}

fn execute_tool(tool_name: &str, args: &serde_json::Value) -> Result<String, String> {
    match tool_name {
        ECHO_TOOL_NAME => {
            let args = serde_json::from_value::<EchoToolArgs>(args.clone())
                .map_err(|error| format!("invalid echo tool arguments: {error}"))?;
            Ok(args.input)
        }
        LUA_TOOL_NAME => {
            let args = serde_json::from_value::<LuaToolArgs>(args.clone())
                .map_err(|error| format!("invalid lua tool arguments: {error}"))?;
            execute_lua(&args.code)
        }
        _ => Err(format!("unsupported tool call: {tool_name}")),
    }
}

#[derive(Debug, Deserialize)]
struct EchoToolArgs {
    input: String,
}

#[derive(Debug, Deserialize)]
struct LuaToolArgs {
    code: String,
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
        source: Box<CompletionError>,
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
