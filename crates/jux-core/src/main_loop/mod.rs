mod context;

pub use self::context::RunLoopContext;

use crate::model::{
    AssistantResponseItem, LlmUsage, Run, RunStatus, Session, SessionContextKind,
    SessionContextPayload, Step, StepKind, StepPayload, Workspace,
};
use crate::store::{SqliteWorkspaceStore, StoreError};
use crate::tools::{execute_tool, tool_definitions};
use rig::OneOrMany;
use rig::completion::{CompletionError, CompletionModel, ToolDefinition, Usage};
use rig::message::{
    AssistantContent, Message, ToolCall, ToolFunction, ToolResult, ToolResultContent,
};
use serde_json::json;
use std::error::Error;
use std::fmt::{self, Display};

const MAX_LOOP_ITERATIONS: usize = 8;
pub const SYSTEM_PROMPT: &str = "You are Jux, a concise coding agent.";

impl From<Usage> for LlmUsage {
    fn from(usage: Usage) -> Self {
        Self {
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
            total_tokens: usage.total_tokens,
            cached_input_tokens: usage.cached_input_tokens,
            cache_creation_input_tokens: usage.cache_creation_input_tokens,
        }
    }
}

pub struct RunLoop<M> {
    context: RunLoopContext<M>,
}

impl<M> RunLoop<M>
where
    M: CompletionModel,
{
    #[must_use]
    pub fn new(store: SqliteWorkspaceStore, model: M) -> Self {
        let context = RunLoopContext::workspace_default(store, model);
        Self::with_context(context)
    }

    #[must_use]
    pub fn with_context(context: RunLoopContext<M>) -> Self {
        Self { context }
    }

    pub async fn run(&self, request: String) -> Result<RunLoopOutput, RunLoopError> {
        let run = self.start_run(request)?;

        for _ in 0..MAX_LOOP_ITERATIONS {
            let llm_request = match self.build_completion_request(&run) {
                Ok(request) => request,
                Err(RunLoopError::Store(error)) => return Err(error.into()),
                Err(RunLoopError::Prompt { source, .. }) => {
                    return self.fail_completion_call(run, *source);
                }
                Err(RunLoopError::Runtime { message, .. }) => {
                    return self.fail_runtime(run, message);
                }
            };
            let history_len = llm_request.history.len();
            tracing::debug!(run_id = %run.id, history_len, "built llm completion request");

            let request = self
                .context
                .model
                .completion_request(llm_request.prompt)
                .messages(llm_request.history)
                .tools(llm_request.tools)
                .build();
            let response = match self.context.model.completion(request).await {
                Ok(response) => response,
                Err(error) => {
                    return self.fail_completion_call(run, error);
                }
            };
            let assistant_turn = match assistant_turn_from_response(response.choice) {
                Ok(assistant_turn) => assistant_turn,
                Err(message) => {
                    return self.fail_runtime(run, message);
                }
            };
            let AssistantTurn {
                final_answer,
                tool_calls,
                items,
            } = assistant_turn;

            self.record_assistant_response(
                &run,
                response.message_id,
                LlmUsage::from(response.usage),
                items,
            )?;

            if tool_calls.is_empty() {
                return self.complete_run(run, final_answer);
            }

            for tool_call in &tool_calls {
                self.execute_tool_call(&run, tool_call)?;
            }
        }

        self.fail_runtime(
            run,
            "run loop reached the maximum number of iterations".to_owned(),
        )
    }

    fn start_run(&self, request: String) -> Result<Run, RunLoopError> {
        let run = self.context.store.create_run(request.clone())?;
        tracing::info!(run_id = %run.id, "created run");
        self.context
            .store
            .ensure_session_context_items(&run.id.session_id(), default_session_context_items())?;
        self.context.store.append_step(
            &run.id,
            StepKind::UserMessage,
            StepPayload::UserMessage { content: request },
        )?;

        Ok(run)
    }

    fn record_assistant_response(
        &self,
        run: &Run,
        message_id: Option<String>,
        usage: LlmUsage,
        items: Vec<AssistantResponseItem>,
    ) -> Result<(), StoreError> {
        self.context.store.append_step(
            &run.id,
            StepKind::AssistantResponse,
            StepPayload::AssistantResponse {
                message_id,
                usage,
                items,
            },
        )?;
        Ok(())
    }

    fn execute_tool_call(
        &self,
        run: &Run,
        tool_call: &AssistantResponseItem,
    ) -> Result<(), StoreError> {
        let AssistantResponseItem::ToolCall {
            id,
            call_id,
            name,
            arguments,
        } = tool_call
        else {
            return Ok(());
        };
        tracing::info!(
            run_id = %run.id,
            tool_name = %name,
            "executing tool call"
        );

        let content = match execute_tool(&self.context, name, arguments) {
            Ok(output) => output,
            Err(error) => {
                tracing::warn!(
                    run_id = %run.id,
                    tool_name = %name,
                    error = %error,
                    "tool call failed"
                );
                json!({
                    "success": false,
                    "error": error,
                })
            }
        };
        self.context.store.append_step(
            &run.id,
            StepKind::ToolResult,
            StepPayload::ToolResult {
                id: id.clone(),
                call_id: call_id.clone(),
                content,
            },
        )?;
        Ok(())
    }

    fn complete_run(&self, run: Run, answer: String) -> Result<RunLoopOutput, RunLoopError> {
        let run = self
            .context
            .store
            .update_run_status(&run.id, RunStatus::Completed)?;
        let workspace = self.context.store.load_workspace()?;
        let session = self.context.store.load_session(&run.id.session_id())?;
        let steps = self.context.store.load_run_steps(&run.id)?;
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
        self.context
            .store
            .append_step(&run.id, StepKind::Error, StepPayload::Error { message })?;

        let run = self
            .context
            .store
            .update_run_status(&run.id, RunStatus::Failed)?;
        tracing::error!(run_id = %run.id, "failed run");
        Err(RunLoopError::Prompt {
            run: Box::new(run),
            source: Box::new(error),
        })
    }

    fn fail_runtime(&self, run: Run, message: String) -> Result<RunLoopOutput, RunLoopError> {
        self.record_runtime_error(&run, message.clone())?;
        let run = self
            .context
            .store
            .update_run_status(&run.id, RunStatus::Failed)?;
        tracing::error!(run_id = %run.id, "failed run");
        Err(RunLoopError::Runtime {
            run: Box::new(run),
            message,
        })
    }

    fn record_runtime_error(&self, run: &Run, message: String) -> Result<(), RunLoopError> {
        self.context
            .store
            .append_step(&run.id, StepKind::Error, StepPayload::Error { message })?;
        Ok(())
    }

    fn build_completion_request(&self, run: &Run) -> Result<LlmCompletionRequest, RunLoopError> {
        let context = self
            .context
            .store
            .load_session_context_items(&run.id.session_id())?;
        let mut messages = context
            .iter()
            .filter_map(session_context_item_to_chat_message)
            .collect::<Vec<_>>();
        let tools = context
            .iter()
            .filter_map(session_context_item_to_tool_definition)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|message| RunLoopError::Runtime {
                run: Box::new(run.clone()),
                message,
            })?;

        let steps = self
            .context
            .store
            .load_session_steps(&run.id.session_id())?;
        messages.extend(
            steps
                .iter()
                .filter(|step| step.visible_to_llm())
                .filter_map(step_to_chat_message),
        );

        let Some((prompt, history)) = messages.split_last() else {
            return Err(RunLoopError::Runtime {
                run: Box::new(run.clone()),
                message: "run has no visible message for the LLM".to_owned(),
            });
        };
        let history = history.to_vec();

        Ok(LlmCompletionRequest {
            prompt: prompt.clone(),
            history,
            tools,
        })
    }
}

struct AssistantTurn {
    final_answer: String,
    tool_calls: Vec<AssistantResponseItem>,
    items: Vec<AssistantResponseItem>,
}

fn assistant_turn_from_response(
    content: OneOrMany<AssistantContent>,
) -> Result<AssistantTurn, String> {
    let mut final_answer = String::new();
    let mut tool_calls = Vec::new();
    let mut items = Vec::new();

    for content in content.into_iter() {
        match content {
            AssistantContent::Text(text) => {
                final_answer.push_str(&text.text);
                items.push(AssistantResponseItem::Text { content: text.text });
            }
            AssistantContent::ToolCall(tool_call) => {
                let item = AssistantResponseItem::ToolCall {
                    id: tool_call.id.clone(),
                    call_id: tool_call.call_id.clone(),
                    name: tool_call.function.name.clone(),
                    arguments: tool_call.function.arguments.clone(),
                };
                tool_calls.push(item.clone());
                items.push(item);
            }
            AssistantContent::Reasoning(reasoning) => {
                let text = reasoning.display_text();
                if !text.is_empty() {
                    items.push(AssistantResponseItem::Reasoning { content: text });
                }
            }
            AssistantContent::Image(_) => {
                return Err("LLM returned an unsupported image response".to_owned());
            }
        }
    }

    Ok(AssistantTurn {
        final_answer,
        tool_calls,
        items,
    })
}

#[derive(Clone, Debug)]
struct LlmCompletionRequest {
    prompt: Message,
    history: Vec<Message>,
    tools: Vec<ToolDefinition>,
}

fn step_to_chat_message(step: &Step) -> Option<Message> {
    match &step.payload {
        StepPayload::UserMessage { content } => Some(Message::user(content.clone())),
        StepPayload::AssistantResponse {
            message_id, items, ..
        } => assistant_response_to_chat_message(message_id, items),
        StepPayload::ToolResult {
            id,
            call_id,
            content,
        } => Some(Message::User {
            content: rig::OneOrMany::one(rig::message::UserContent::ToolResult(ToolResult {
                id: id.clone(),
                call_id: call_id.clone(),
                content: rig::OneOrMany::one(ToolResultContent::text(content.to_string())),
            })),
        }),
        StepPayload::Error { .. } => None,
    }
}

fn assistant_response_to_chat_message(
    message_id: &Option<String>,
    items: &[AssistantResponseItem],
) -> Option<Message> {
    let content = items
        .iter()
        .filter_map(assistant_response_item_to_chat_content)
        .collect::<Vec<_>>();
    let content = OneOrMany::many(content).ok()?;
    Some(Message::Assistant {
        id: message_id.clone(),
        content,
    })
}

fn assistant_response_item_to_chat_content(
    item: &AssistantResponseItem,
) -> Option<AssistantContent> {
    match item {
        AssistantResponseItem::Text { content } => Some(AssistantContent::text(content.clone())),
        AssistantResponseItem::ToolCall {
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
            Some(AssistantContent::ToolCall(tool_call))
        }
        AssistantResponseItem::Reasoning { .. } => None,
    }
}

fn session_context_item_to_chat_message(
    item: &crate::model::SessionContextItem,
) -> Option<Message> {
    match &item.payload {
        SessionContextPayload::SystemPrompt { content } => Some(Message::System {
            content: content.clone(),
        }),
        SessionContextPayload::ToolDefinition { .. } => None,
    }
}

fn session_context_item_to_tool_definition(
    item: &crate::model::SessionContextItem,
) -> Option<Result<ToolDefinition, String>> {
    match &item.payload {
        SessionContextPayload::ToolDefinition {
            name,
            description,
            parameters,
        } => Some(Ok(ToolDefinition {
            name: name.clone(),
            description: description.clone(),
            parameters: parameters.clone(),
        })),
        SessionContextPayload::SystemPrompt { .. } => None,
    }
}

fn default_session_context_items() -> Vec<(SessionContextKind, SessionContextPayload)> {
    let mut items = Vec::new();
    items.push((
        SessionContextKind::SystemPrompt,
        SessionContextPayload::SystemPrompt {
            content: SYSTEM_PROMPT.to_owned(),
        },
    ));

    for tool in tool_definitions() {
        items.push((
            SessionContextKind::ToolDefinition,
            SessionContextPayload::ToolDefinition {
                name: tool.name,
                description: tool.description,
                parameters: tool.parameters,
            },
        ));
    }

    items
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
