//! Agent run-loop orchestration.
//!
//! This module owns the top-level control flow for one user request. It starts
//! and persists runs, builds LLM completion requests from stored context and
//! visible history, records assistant responses, executes requested tools, and
//! decides when the run is complete or failed.
//!
//! The run loop is intentionally the only layer that writes run and step state.
//! Lower-level modules receive narrower context traits so they can execute work
//! without knowing about persistence, model calls, or the full agent runtime.

mod context;
mod event;

pub use self::context::RunLoopContext;
pub use self::event::{
    AgentEvent, AgentEventData, AgentEventId, AgentEventKind, AgentEventSink, NoopAgentEventSink,
};

use crate::state::{
    AssistantResponseItem, LlmUsage, Run, RunStatus, Session, SessionContextKind,
    SessionContextPayload, Step, StepKind, StepPayload, Workspace,
};
use crate::state::{SqliteWorkspaceStore, StoreError};
use crate::tools::{execute_tool, tool_definitions};
use rig::OneOrMany;
use rig::completion::{
    CompletionError, CompletionModel, CompletionResponse, ToolDefinition, Usage,
};
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
        let mut events = NoopAgentEventSink;
        self.run_with_events(request, &mut events).await
    }

    pub async fn run_with_events(
        &self,
        request: String,
        events: &mut impl AgentEventSink,
    ) -> Result<RunLoopOutput, RunLoopError> {
        let run = self.start_run(request)?;
        self.emit_run_started(events, &run.request);

        for iteration_index in 1..=MAX_LOOP_ITERATIONS {
            self.emit_iteration_started(events, iteration_index);
            let response = self
                .complete_llm_call(&run, iteration_index, events)
                .await?;
            let assistant_turn = match assistant_turn_from_response(response.choice) {
                Ok(assistant_turn) => assistant_turn,
                Err(message) => {
                    return self.fail_runtime(run.clone(), message, events);
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
                self.emit_iteration_completed(events, iteration_index);
                return self.complete_run(run, final_answer, events);
            }

            for (tool_index, tool_call) in tool_calls.iter().enumerate() {
                self.execute_tool_call(&run, tool_call, events, iteration_index, tool_index + 1)?;
            }
            self.emit_iteration_completed(events, iteration_index);
        }

        self.fail_runtime(
            run,
            "run loop reached the maximum number of iterations".to_owned(),
            events,
        )
    }

    async fn complete_llm_call(
        &self,
        run: &Run,
        iteration_index: usize,
        events: &mut impl AgentEventSink,
    ) -> Result<CompletionResponse<M::Response>, RunLoopError> {
        let llm_request = self.llm_request_or_fail(run, iteration_index, events)?;
        let history_len = llm_request.history.len();
        tracing::debug!(run_id = %run.id, history_len, "built llm completion request");

        let llm_id = AgentEventId::llm(iteration_index, 1);
        events.emit(AgentEvent::new(
            llm_id.clone(),
            AgentEventKind::Started,
            AgentEventData::LlmStarted,
        ));
        let request = self
            .context
            .model
            .completion_request(llm_request.prompt)
            .messages(llm_request.history)
            .tools(llm_request.tools)
            .build();
        let response = self
            .context
            .model
            .completion(request)
            .await
            .map_err(|error| {
                self.fail_completion_call(run.clone(), error, events, iteration_index)
                    .expect_err("completion failure returns a run-loop error")
            })?;
        events.emit(AgentEvent::new(
            llm_id,
            AgentEventKind::Completed,
            AgentEventData::LlmCompleted,
        ));
        Ok(response)
    }

    fn llm_request_or_fail(
        &self,
        run: &Run,
        iteration_index: usize,
        events: &mut impl AgentEventSink,
    ) -> Result<LlmCompletionRequest, RunLoopError> {
        match self.build_completion_request(run) {
            Ok(request) => Ok(request),
            Err(RunLoopError::Store(error)) => Err(error.into()),
            Err(RunLoopError::Prompt { source, .. }) => {
                self.fail_completion_call(run.clone(), *source, events, iteration_index)?;
                unreachable!("fail_completion_call always returns an error")
            }
            Err(RunLoopError::Runtime { message, .. }) => {
                self.fail_runtime(run.clone(), message, events)?;
                unreachable!("fail_runtime always returns an error")
            }
        }
    }

    fn start_run(&self, request: String) -> Result<Run, RunLoopError> {
        let run = self.context.store.create_run(request.clone())?;
        tracing::info!(run_id = %run.id, "created run");
        let context_items =
            default_session_context_items(&self.context.instructions, &self.context.skills);
        self.context
            .store
            .ensure_session_context_items(&run.id.session_id(), context_items)?;
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
        events: &mut impl AgentEventSink,
        iteration_index: usize,
        tool_index: usize,
    ) -> Result<(), StoreError> {
        let Some(tool_call) = ToolCallRequest::from_assistant_item(tool_call) else {
            return Ok(());
        };
        tracing::info!(
            run_id = %run.id,
            tool_name = %tool_call.name,
            "executing tool call"
        );

        let tool_id = AgentEventId::tool(iteration_index, &tool_call.name, tool_index);
        events.emit(AgentEvent::new(
            tool_id.clone(),
            AgentEventKind::Started,
            AgentEventData::ToolStarted {
                name: tool_call.name.clone(),
                call_id: tool_call.call_id.clone(),
            },
        ));
        let content = self.execute_tool_content(run, &tool_call, tool_id, events);
        self.context.store.append_step(
            &run.id,
            StepKind::ToolResult,
            StepPayload::ToolResult {
                id: tool_call.id,
                call_id: tool_call.call_id,
                content,
            },
        )?;
        Ok(())
    }

    fn execute_tool_content(
        &self,
        run: &Run,
        tool_call: &ToolCallRequest,
        tool_id: AgentEventId,
        events: &mut impl AgentEventSink,
    ) -> serde_json::Value {
        match execute_tool(&self.context, &tool_call.name, &tool_call.arguments) {
            Ok(output) => self.record_tool_output(tool_call, tool_id, events, output),
            Err(error) => self.record_tool_failure(run, tool_call, tool_id, events, error),
        }
    }

    fn record_tool_output(
        &self,
        tool_call: &ToolCallRequest,
        tool_id: AgentEventId,
        events: &mut impl AgentEventSink,
        output: serde_json::Value,
    ) -> serde_json::Value {
        events.emit(AgentEvent::new(
            tool_id.clone(),
            AgentEventKind::Output,
            AgentEventData::ToolOutput {
                name: tool_call.name.clone(),
                content: output.clone(),
            },
        ));
        events.emit(AgentEvent::new(
            tool_id,
            AgentEventKind::Completed,
            AgentEventData::ToolCompleted {
                name: tool_call.name.clone(),
                call_id: tool_call.call_id.clone(),
            },
        ));
        output
    }

    fn record_tool_failure(
        &self,
        run: &Run,
        tool_call: &ToolCallRequest,
        tool_id: AgentEventId,
        events: &mut impl AgentEventSink,
        error: String,
    ) -> serde_json::Value {
        tracing::warn!(
            run_id = %run.id,
            tool_name = %tool_call.name,
            error = %error,
            "tool call failed"
        );
        events.emit(AgentEvent::new(
            tool_id,
            AgentEventKind::Failed,
            AgentEventData::ToolFailed {
                name: tool_call.name.clone(),
                call_id: tool_call.call_id.clone(),
                error: error.clone(),
            },
        ));
        json!({ "success": false, "error": error })
    }

    fn complete_run(
        &self,
        run: Run,
        answer: String,
        events: &mut impl AgentEventSink,
    ) -> Result<RunLoopOutput, RunLoopError> {
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

        events.emit(AgentEvent::new(
            AgentEventId::run(),
            AgentEventKind::Completed,
            AgentEventData::RunCompleted {
                answer: answer.clone(),
            },
        ));
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
        events: &mut impl AgentEventSink,
        iteration_index: usize,
    ) -> Result<RunLoopOutput, RunLoopError> {
        let message = error.to_string();
        events.emit(AgentEvent::new(
            AgentEventId::llm(iteration_index, 1),
            AgentEventKind::Failed,
            AgentEventData::LlmFailed {
                error: message.clone(),
            },
        ));
        self.context.store.append_step(
            &run.id,
            StepKind::Error,
            StepPayload::Error {
                message: message.clone(),
            },
        )?;

        let run = self
            .context
            .store
            .update_run_status(&run.id, RunStatus::Failed)?;
        tracing::error!(run_id = %run.id, "failed run");
        self.emit_run_failed(events, message);
        Err(RunLoopError::Prompt {
            run: Box::new(run),
            source: Box::new(error),
        })
    }

    fn fail_runtime(
        &self,
        run: Run,
        message: String,
        events: &mut impl AgentEventSink,
    ) -> Result<RunLoopOutput, RunLoopError> {
        self.record_runtime_error(&run, message.clone())?;
        let run = self
            .context
            .store
            .update_run_status(&run.id, RunStatus::Failed)?;
        tracing::error!(run_id = %run.id, "failed run");
        self.emit_run_failed(events, message.clone());
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

    fn emit_iteration_started(&self, events: &mut impl AgentEventSink, index: usize) {
        events.emit(AgentEvent::new(
            AgentEventId::iteration(index),
            AgentEventKind::Started,
            AgentEventData::IterationStarted { index },
        ));
    }

    fn emit_run_started(&self, events: &mut impl AgentEventSink, request: &str) {
        events.emit(AgentEvent::new(
            AgentEventId::run(),
            AgentEventKind::Started,
            AgentEventData::RunStarted {
                request: request.to_owned(),
            },
        ));
    }

    fn emit_iteration_completed(&self, events: &mut impl AgentEventSink, index: usize) {
        events.emit(AgentEvent::new(
            AgentEventId::iteration(index),
            AgentEventKind::Completed,
            AgentEventData::IterationCompleted { index },
        ));
    }

    fn emit_run_failed(&self, events: &mut impl AgentEventSink, error: String) {
        events.emit(AgentEvent::new(
            AgentEventId::run(),
            AgentEventKind::Failed,
            AgentEventData::RunFailed { error },
        ));
    }
}

struct AssistantTurn {
    final_answer: String,
    tool_calls: Vec<AssistantResponseItem>,
    items: Vec<AssistantResponseItem>,
}

struct ToolCallRequest {
    id: String,
    call_id: Option<String>,
    name: String,
    arguments: serde_json::Value,
}

impl ToolCallRequest {
    fn from_assistant_item(item: &AssistantResponseItem) -> Option<Self> {
        let AssistantResponseItem::ToolCall {
            id,
            call_id,
            name,
            arguments,
        } = item
        else {
            return None;
        };
        Some(Self {
            id: id.clone(),
            call_id: call_id.clone(),
            name: name.clone(),
            arguments: arguments.clone(),
        })
    }
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
    item: &crate::state::SessionContextItem,
) -> Option<Message> {
    match &item.payload {
        SessionContextPayload::SystemPrompt { content } => Some(Message::System {
            content: content.clone(),
        }),
        SessionContextPayload::ToolDefinition { .. } => None,
    }
}

fn session_context_item_to_tool_definition(
    item: &crate::state::SessionContextItem,
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

fn default_session_context_items(
    instructions: &[crate::InstructionDocument],
    skills: &[crate::SkillDefinition],
) -> Vec<(SessionContextKind, SessionContextPayload)> {
    let mut items = Vec::new();
    items.push((
        SessionContextKind::SystemPrompt,
        SessionContextPayload::SystemPrompt {
            content: system_prompt_with_context(instructions, skills),
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

fn system_prompt_with_context(
    instructions: &[crate::InstructionDocument],
    skills: &[crate::SkillDefinition],
) -> String {
    if instructions.is_empty() && skills.is_empty() {
        return SYSTEM_PROMPT.to_owned();
    }
    let mut prompt = SYSTEM_PROMPT.to_owned();
    if !instructions.is_empty() {
        prompt.push_str("\n\n");
        prompt.push_str(&crate::render_instruction_documents(instructions));
    }
    if !skills.is_empty() {
        prompt.push_str("\n\n");
        prompt.push_str(&crate::render_skill_index(skills));
    }
    prompt
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
