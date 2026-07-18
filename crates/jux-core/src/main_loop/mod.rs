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

mod cancellation;
mod context;
mod event;

pub use self::cancellation::{RunCancellationHandle, RunCancellationToken, run_cancellation_pair};
pub use self::context::RunLoopContext;
pub use self::event::{
    AgentEvent, AgentEventData, AgentEventId, AgentEventKind, AgentEventSink, NoopAgentEventSink,
    SequencedAgentEventSink, ToolOutputStream,
};

use crate::state::{
    AssistantResponseItem, LlmUsage, Run, RunStatus, Session, SessionContextKind,
    SessionContextPayload, Step, StepKind, StepPayload, Workspace,
};
use crate::state::{SqliteWorkspaceStore, StoreError};
use crate::tools::{HUMAN_INPUT_TOOL_NAME, execute_tool, tool_definitions};
use futures::StreamExt;
use futures::future::Abortable;
use rig::OneOrMany;
use rig::completion::{CompletionError, CompletionModel, GetTokenUsage, ToolDefinition, Usage};
use rig::message::{
    AssistantContent, Message, ToolCall, ToolFunction, ToolResult, ToolResultContent,
};
use rig::streaming::{StreamedAssistantContent, ToolCallDeltaContent};
use serde::Deserialize;
use serde_json::json;
use std::collections::HashSet;
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

    pub async fn run_cancellable(
        &self,
        request: String,
        cancellation: RunCancellationToken,
    ) -> Result<RunLoopOutput, RunLoopError> {
        let mut events = NoopAgentEventSink;
        self.run_with_events_cancellable(request, &mut events, cancellation)
            .await
    }

    pub async fn run_with_events_cancellable(
        &self,
        request: String,
        events: &mut impl AgentEventSink,
        cancellation: RunCancellationToken,
    ) -> Result<RunLoopOutput, RunLoopError> {
        // Observe deltas without persisting each token. Cancellation writes the
        // accumulated text once as a recovery checkpoint; normal completion
        // persists the authoritative assistant response instead.
        let mut checkpointing_events = PartialOutputEventSink::new(events);
        match Abortable::new(
            self.run_with_events(request, &mut checkpointing_events),
            cancellation.registration,
        )
        .await
        {
            Ok(result) => result,
            Err(_) => {
                let run = self.cancel_latest_running_run()?;
                if let Some(run) = &run
                    && !checkpointing_events.partial_output.is_empty()
                {
                    self.context.store.append_step(
                        &run.id,
                        StepKind::AssistantOutputCheckpoint,
                        StepPayload::AssistantOutputCheckpoint {
                            content: checkpointing_events.partial_output.clone(),
                        },
                    )?;
                }
                Err(RunLoopError::Canceled {
                    run: run.map(Box::new),
                })
            }
        }
    }

    pub async fn run_with_events(
        &self,
        request: String,
        events: &mut impl AgentEventSink,
    ) -> Result<RunLoopOutput, RunLoopError> {
        let mut events = SequencedAgentEventSink::new(events);
        self.run_with_sequenced_events(request, &mut events).await
    }

    async fn run_with_sequenced_events(
        &self,
        request: String,
        events: &mut impl AgentEventSink,
    ) -> Result<RunLoopOutput, RunLoopError> {
        self.refresh_active_session_context()?;
        if let Some(run) = self.latest_waiting_run()? {
            return match self.resume_waiting_run(run, request)? {
                ResumeTarget::Main(run) => self.continue_run(run, events).await,
                ResumeTarget::Skill { run, invocation } => {
                    self.continue_resumed_skill(run, invocation, events).await
                }
            };
        }

        let run = self.start_run(request)?;
        self.emit_run_started(events, &run.request);
        self.emit_skills_selected(events);
        self.continue_requested_skills(run, events).await
    }

    fn refresh_active_session_context(&self) -> Result<(), RunLoopError> {
        let session = match self.context.store.load_active_session() {
            Ok(session) => session,
            Err(StoreError::MissingWorkspace) => return Ok(()),
            Err(error) => return Err(error.into()),
        };
        self.context.store.replace_session_context_items(
            &session.id,
            default_session_context_items(
                &self.context.instructions,
                &self.context.skills,
                &self.context.active_skills,
            ),
        )?;
        Ok(())
    }

    async fn continue_run(
        &self,
        run: Run,
        events: &mut impl AgentEventSink,
    ) -> Result<RunLoopOutput, RunLoopError> {
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

            if human_input_tool_call(&tool_calls).is_some() {
                self.emit_iteration_completed(events, iteration_index);
                return self.wait_for_human_input(run, events);
            }

            if tool_calls.is_empty() {
                self.emit_iteration_completed(events, iteration_index);
                return self.complete_run(run, final_answer, events);
            }

            for (tool_index, tool_call) in tool_calls.iter().enumerate() {
                let outcome = self
                    .execute_tool_call(&run, tool_call, events, iteration_index, tool_index + 1)
                    .await?;
                if outcome == ToolCallOutcome::WaitingForHumanInput {
                    self.emit_iteration_completed(events, iteration_index);
                    return self.wait_for_human_input(run, events);
                }
            }
            self.emit_iteration_completed(events, iteration_index);
        }

        self.fail_runtime(
            run,
            "run loop reached the maximum number of iterations".to_owned(),
            events,
        )
    }

    fn latest_waiting_run(&self) -> Result<Option<Run>, RunLoopError> {
        let session = match self.context.store.load_active_session() {
            Ok(session) => session,
            Err(StoreError::MissingWorkspace) => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let Some(run) = self
            .context
            .store
            .load_session_runs(&session.id)?
            .into_iter()
            .last()
        else {
            return Ok(None);
        };
        Ok((run.status == RunStatus::WaitingForHumanInput).then_some(run))
    }

    fn cancel_latest_running_run(&self) -> Result<Option<Run>, RunLoopError> {
        let session = match self.context.store.load_active_session() {
            Ok(session) => session,
            Err(StoreError::MissingWorkspace) => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let Some(run) = self
            .context
            .store
            .load_session_runs(&session.id)?
            .into_iter()
            .next_back()
        else {
            return Ok(None);
        };
        if run.status != RunStatus::Running {
            return Ok(None);
        }
        let run = self
            .context
            .store
            .update_run_status(&run.id, RunStatus::Canceled)?;
        Ok(Some(run))
    }

    fn resume_waiting_run(&self, run: Run, input: String) -> Result<ResumeTarget, RunLoopError> {
        let steps = self.context.store.load_run_steps(&run.id)?;
        let pending = latest_pending_human_input(&steps).ok_or_else(|| RunLoopError::Runtime {
            run: Box::new(run.clone()),
            message: "waiting run has no pending human_input tool call".to_owned(),
        })?;
        let tool_call = pending.tool_call();
        validate_human_input(&tool_call.arguments, &input).map_err(|message| {
            RunLoopError::Runtime {
                run: Box::new(run.clone()),
                message,
            }
        })?;
        let payload = match &pending {
            PendingHumanInput::Main { tool_call } => StepPayload::ToolResult {
                id: tool_call.id.clone(),
                call_id: tool_call.call_id.clone(),
                content: json!({ "input": input }),
            },
            PendingHumanInput::Skill {
                invocation,
                tool_call,
            } => StepPayload::SkillToolResult {
                invocation_id: invocation.id.clone(),
                id: tool_call.id.clone(),
                call_id: tool_call.call_id.clone(),
                content: json!({ "input": input }),
            },
        };
        let kind = match pending {
            PendingHumanInput::Main { .. } => StepKind::ToolResult,
            PendingHumanInput::Skill { .. } => StepKind::SkillExecution,
        };
        self.context.store.append_step(&run.id, kind, payload)?;
        let run = self
            .context
            .store
            .update_run_status(&run.id, RunStatus::Running)?;
        Ok(match pending {
            PendingHumanInput::Main { .. } => ResumeTarget::Main(run),
            PendingHumanInput::Skill { invocation, .. } => ResumeTarget::Skill { run, invocation },
        })
    }

    async fn complete_llm_call(
        &self,
        run: &Run,
        iteration_index: usize,
        events: &mut impl AgentEventSink,
    ) -> Result<LlmResponse, RunLoopError> {
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
        let response = if self.context.stream_model_output {
            self.stream_completion(run, request, events, iteration_index)
                .await?
        } else {
            let response = self
                .context
                .model
                .completion(request)
                .await
                .map_err(|error| {
                    self.fail_completion_call(run.clone(), error, events, iteration_index)
                        .expect_err("completion failure returns a run-loop error")
                })?;
            LlmResponse {
                choice: response.choice,
                usage: response.usage,
                message_id: response.message_id,
            }
        };
        events.emit(AgentEvent::new(
            llm_id,
            AgentEventKind::Completed,
            AgentEventData::LlmCompleted,
        ));
        Ok(response)
    }

    async fn stream_completion(
        &self,
        run: &Run,
        request: rig::completion::CompletionRequest,
        events: &mut impl AgentEventSink,
        iteration_index: usize,
    ) -> Result<LlmResponse, RunLoopError> {
        let mut stream = self.context.model.stream(request).await.map_err(|error| {
            self.fail_completion_call(run.clone(), error, events, iteration_index)
                .expect_err("streaming completion failure returns a run-loop error")
        })?;
        let llm_id = AgentEventId::llm(iteration_index, 1);
        let mut usage = Usage::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|error| {
                self.fail_completion_call(run.clone(), error, events, iteration_index)
                    .expect_err("streaming completion failure returns a run-loop error")
            })?;
            let data = match chunk {
                StreamedAssistantContent::Text(text) => {
                    Some(AgentEventData::AssistantTextDelta { content: text.text })
                }
                StreamedAssistantContent::Reasoning(reasoning) => {
                    Some(AgentEventData::AssistantReasoningDelta {
                        content: reasoning.display_text(),
                    })
                }
                StreamedAssistantContent::ReasoningDelta { reasoning, .. } => {
                    Some(AgentEventData::AssistantReasoningDelta { content: reasoning })
                }
                StreamedAssistantContent::ToolCallDelta {
                    internal_call_id,
                    content,
                    ..
                } => Some(AgentEventData::ToolCallDelta {
                    call_id: internal_call_id,
                    content: match content {
                        ToolCallDeltaContent::Name(name) | ToolCallDeltaContent::Delta(name) => {
                            name
                        }
                    },
                }),
                StreamedAssistantContent::Final(response) => {
                    if let Some(final_usage) = response.token_usage() {
                        usage = final_usage;
                        Some(AgentEventData::UsageDelta {
                            usage: usage.into(),
                        })
                    } else {
                        None
                    }
                }
                StreamedAssistantContent::ToolCall { .. } => None,
            };
            if let Some(data) = data {
                events.emit(AgentEvent::new(
                    llm_id.clone(),
                    AgentEventKind::Output,
                    data,
                ));
            }
        }
        events.emit(AgentEvent::new(
            llm_id,
            AgentEventKind::Completed,
            AgentEventData::OutputCompleted,
        ));
        Ok(LlmResponse {
            choice: stream.choice.clone(),
            usage,
            message_id: stream.message_id.clone(),
        })
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
            Err(error @ RunLoopError::Canceled { .. }) => Err(error),
        }
    }

    fn start_run(&self, request: String) -> Result<Run, RunLoopError> {
        let run = self.context.store.create_run(request.clone())?;
        tracing::info!(run_id = %run.id, "created run");
        let context_items = default_session_context_items(
            &self.context.instructions,
            &self.context.skills,
            &self.context.active_skills,
        );
        self.context
            .store
            .replace_session_context_items(&run.id.session_id(), context_items)?;
        self.context.store.append_step(
            &run.id,
            StepKind::UserMessage,
            StepPayload::UserMessage { content: request },
        )?;
        if !self.context.requested_skills.is_empty() {
            self.context.store.append_step(
                &run.id,
                StepKind::SkillExecution,
                StepPayload::SkillsRequested {
                    names: self
                        .context
                        .requested_skills
                        .iter()
                        .map(|skill| skill.name.clone())
                        .collect(),
                },
            )?;
        }

        Ok(run)
    }

    async fn continue_requested_skills(
        &self,
        run: Run,
        events: &mut impl AgentEventSink,
    ) -> Result<RunLoopOutput, RunLoopError> {
        let steps = self.context.store.load_run_steps(&run.id)?;
        let Some(names) = requested_skill_names(&steps) else {
            return self.continue_run(run, events).await;
        };

        for (index, name) in names.iter().enumerate() {
            let invocation_id = explicit_skill_invocation_id(index);
            if has_tool_result(&steps, &invocation_id) {
                continue;
            }
            let tool_call = AssistantResponseItem::ToolCall {
                id: invocation_id,
                call_id: None,
                name: crate::CALL_SKILL_TOOL_NAME.to_owned(),
                arguments: json!({
                    "name": name,
                    "task": run.request.clone(),
                }),
            };
            self.record_assistant_response(
                &run,
                None,
                LlmUsage::default(),
                vec![tool_call.clone()],
            )?;
            let outcome = self
                .execute_tool_call(&run, &tool_call, events, 0, index + 1)
                .await?;
            if outcome == ToolCallOutcome::WaitingForHumanInput {
                return self.wait_for_human_input(run, events);
            }
        }

        self.continue_run(run, events).await
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

    async fn execute_tool_call(
        &self,
        run: &Run,
        tool_call: &AssistantResponseItem,
        events: &mut impl AgentEventSink,
        iteration_index: usize,
        tool_index: usize,
    ) -> Result<ToolCallOutcome, StoreError> {
        let Some(tool_call) = ToolCallRequest::from_assistant_item(tool_call) else {
            return Ok(ToolCallOutcome::Completed);
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
                arguments: tool_call.arguments.clone(),
            },
        ));
        let content = if tool_call.name == crate::CALL_SKILL_TOOL_NAME {
            match self.execute_skill_call(run, &tool_call).await {
                Ok(SkillSubflowOutcome::Completed(output)) => {
                    self.record_tool_output(&tool_call, tool_id, events, output)
                }
                Ok(SkillSubflowOutcome::WaitingForHumanInput) => {
                    return Ok(ToolCallOutcome::WaitingForHumanInput);
                }
                Err(error) => self.record_tool_failure(run, &tool_call, tool_id, events, error),
            }
        } else {
            self.execute_tool_content(run, &tool_call, tool_id, events)
        };
        self.context.store.append_step(
            &run.id,
            StepKind::ToolResult,
            StepPayload::ToolResult {
                id: tool_call.id,
                call_id: tool_call.call_id,
                content,
            },
        )?;
        Ok(ToolCallOutcome::Completed)
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

    async fn execute_skill_call(
        &self,
        run: &Run,
        tool_call: &ToolCallRequest,
    ) -> Result<SkillSubflowOutcome, String> {
        let args = serde_json::from_value::<SkillCallArgs>(tool_call.arguments.clone())
            .map_err(|error| format!("invalid call_skill tool arguments: {error}"))?;
        let skill = self
            .context
            .skills
            .iter()
            .find(|skill| skill.name == args.name)
            .ok_or_else(|| format!("skill not found: {}", args.name))?;
        let invocation = SkillInvocation {
            id: tool_call.id.clone(),
            call_id: tool_call.call_id.clone(),
            skill_name: skill.name.clone(),
            task: args.task,
        };
        self.context
            .store
            .append_step(
                &run.id,
                StepKind::SkillExecution,
                StepPayload::SkillStarted {
                    invocation_id: invocation.id.clone(),
                    call_id: invocation.call_id.clone(),
                    skill_name: invocation.skill_name.clone(),
                    task: invocation.task.clone(),
                },
            )
            .map_err(|error| error.to_string())?;
        let steps = self
            .context
            .store
            .load_run_steps(&run.id)
            .map_err(|error| error.to_string())?;
        let context = self.skill_completion_context(run, &invocation, skill, &steps)?;
        self.run_skill_subflow(run, &invocation, context, 0).await
    }

    async fn run_skill_subflow(
        &self,
        run: &Run,
        invocation: &SkillInvocation,
        context: SkillCompletionContext,
        completed_iterations: usize,
    ) -> Result<SkillSubflowOutcome, String> {
        let SkillCompletionContext {
            mut messages,
            tools,
        } = context;
        for _ in completed_iterations..MAX_LOOP_ITERATIONS {
            let Some((prompt, history)) = messages.split_last() else {
                return Err("skill subflow has no message for the LLM".to_owned());
            };
            let response = self
                .context
                .model
                .completion(
                    self.context
                        .model
                        .completion_request(prompt.clone())
                        .messages(history.to_vec())
                        .tools(tools.clone())
                        .build(),
                )
                .await
                .map_err(|error| format!("skill subflow completion failed: {error}"))?;
            let message_id = response.message_id.clone();
            let assistant_turn = assistant_turn_from_response(response.choice)
                .map_err(|message| format!("skill subflow returned invalid response: {message}"))?;
            self.context
                .store
                .append_step(
                    &run.id,
                    StepKind::SkillExecution,
                    StepPayload::SkillAssistantResponse {
                        invocation_id: invocation.id.clone(),
                        message_id,
                        items: assistant_turn.items.clone(),
                    },
                )
                .map_err(|error| error.to_string())?;

            if assistant_turn.tool_calls.is_empty() {
                return Ok(SkillSubflowOutcome::Completed(skill_result(
                    invocation,
                    assistant_turn.final_answer,
                )));
            }

            if human_input_tool_call(&assistant_turn.tool_calls).is_some() {
                if assistant_turn.tool_calls.len() != 1 {
                    return Err(
                        "human_input must be the only tool call in a skill response".to_owned()
                    );
                }
                return Ok(SkillSubflowOutcome::WaitingForHumanInput);
            }

            let assistant_message =
                assistant_response_to_chat_message(&None, &assistant_turn.items)
                    .ok_or_else(|| "skill subflow returned no assistant message".to_owned())?;
            messages.push(assistant_message);

            for tool_call in &assistant_turn.tool_calls {
                let Some(tool_call) = ToolCallRequest::from_assistant_item(tool_call) else {
                    continue;
                };
                if tool_call.name == crate::CALL_SKILL_TOOL_NAME {
                    return Err("skill subflows cannot call other skills".to_owned());
                }
                let output = execute_tool(&self.context, &tool_call.name, &tool_call.arguments)
                    .unwrap_or_else(|error| json!({ "success": false, "error": error }));
                self.context
                    .store
                    .append_step(
                        &run.id,
                        StepKind::SkillExecution,
                        StepPayload::SkillToolResult {
                            invocation_id: invocation.id.clone(),
                            id: tool_call.id.clone(),
                            call_id: tool_call.call_id.clone(),
                            content: output.clone(),
                        },
                    )
                    .map_err(|error| error.to_string())?;
                messages.push(tool_result_chat_message(&tool_call, output));
            }
        }

        Err("skill subflow reached the maximum number of iterations".to_owned())
    }

    fn skill_completion_context(
        &self,
        run: &Run,
        invocation: &SkillInvocation,
        skill: &crate::SkillDefinition,
        run_steps: &[Step],
    ) -> Result<SkillCompletionContext, String> {
        let mut context = self.parent_context_before_skill(run, &invocation.id)?;
        inject_skill_instructions(
            &mut context.messages,
            &crate::render_skill_execution_prompt(skill),
        );
        context
            .messages
            .push(Message::user(invocation.task.clone()));
        append_skill_transcript(&mut context.messages, invocation, run_steps)?;
        Ok(context)
    }

    fn parent_context_before_skill(
        &self,
        run: &Run,
        invocation_id: &str,
    ) -> Result<SkillCompletionContext, String> {
        let context_items = self
            .context
            .store
            .load_session_context_items(&run.id.session_id())
            .map_err(|error| error.to_string())?;
        let messages = context_items
            .iter()
            .filter_map(session_context_item_to_chat_message)
            .collect::<Vec<_>>();
        let tools = context_items
            .iter()
            .filter_map(session_context_item_to_tool_definition)
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .filter(|tool| tool.name != crate::CALL_SKILL_TOOL_NAME)
            .collect();
        let steps = self
            .context
            .store
            .load_session_steps(&run.id.session_id())
            .map_err(|error| error.to_string())?;
        Ok(SkillCompletionContext {
            messages: parent_messages_before_skill_call(
                messages,
                &steps,
                run.id.as_str(),
                invocation_id,
            )?,
            tools,
        })
    }

    async fn continue_resumed_skill(
        &self,
        run: Run,
        invocation: SkillInvocation,
        events: &mut impl AgentEventSink,
    ) -> Result<RunLoopOutput, RunLoopError> {
        let steps = self.context.store.load_run_steps(&run.id)?;
        let Some(skill) = self
            .context
            .skills
            .iter()
            .find(|skill| skill.name == invocation.skill_name)
        else {
            self.append_skill_tool_result(
                &run,
                &invocation,
                json!({
                    "success": false,
                    "error": format!("skill not found while resuming: {}", invocation.skill_name),
                }),
            )?;
            return self.continue_requested_skills(run, events).await;
        };
        let context = self
            .skill_completion_context(&run, &invocation, skill, &steps)
            .map_err(|message| RunLoopError::Runtime {
                run: Box::new(run.clone()),
                message,
            })?;
        let completed_iterations = skill_iteration_count(&steps, &invocation.id);
        match self
            .run_skill_subflow(&run, &invocation, context, completed_iterations)
            .await
        {
            Ok(SkillSubflowOutcome::Completed(content)) => {
                self.append_skill_tool_result(&run, &invocation, content)?;
                self.continue_requested_skills(run, events).await
            }
            Ok(SkillSubflowOutcome::WaitingForHumanInput) => self.wait_for_human_input(run, events),
            Err(error) => {
                self.append_skill_tool_result(
                    &run,
                    &invocation,
                    json!({ "success": false, "error": error }),
                )?;
                self.continue_requested_skills(run, events).await
            }
        }
    }

    fn append_skill_tool_result(
        &self,
        run: &Run,
        invocation: &SkillInvocation,
        content: serde_json::Value,
    ) -> Result<(), StoreError> {
        self.context.store.append_step(
            &run.id,
            StepKind::ToolResult,
            StepPayload::ToolResult {
                id: invocation.id.clone(),
                call_id: invocation.call_id.clone(),
                content,
            },
        )?;
        Ok(())
    }

    fn record_tool_output(
        &self,
        tool_call: &ToolCallRequest,
        tool_id: AgentEventId,
        events: &mut impl AgentEventSink,
        output: serde_json::Value,
    ) -> serde_json::Value {
        self.emit_tool_output_chunks(tool_call, &tool_id, events, &output);
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

    fn emit_tool_output_chunks(
        &self,
        tool_call: &ToolCallRequest,
        tool_id: &AgentEventId,
        events: &mut impl AgentEventSink,
        output: &serde_json::Value,
    ) {
        // The current exec adapter returns a completed structured result. This
        // seam emits bounded stdout/stderr chunks so clients do not need to
        // understand that result schema. A future streaming adapter can emit
        // the same event while its process is still running.
        if tool_call.name != "exec" {
            return;
        }
        for (field, stream) in [
            ("stdout", ToolOutputStream::Stdout),
            ("stderr", ToolOutputStream::Stderr),
        ] {
            let Some(content) = output.get(field).and_then(serde_json::Value::as_str) else {
                continue;
            };
            for chunk in content.as_bytes().chunks(4096) {
                events.emit(AgentEvent::new(
                    tool_id.clone(),
                    AgentEventKind::Output,
                    AgentEventData::ToolOutputChunk {
                        name: tool_call.name.clone(),
                        stream,
                        content: String::from_utf8_lossy(chunk).into_owned(),
                    },
                ));
            }
        }
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

    fn wait_for_human_input(
        &self,
        run: Run,
        _events: &mut impl AgentEventSink,
    ) -> Result<RunLoopOutput, RunLoopError> {
        let run = self
            .context
            .store
            .update_run_status(&run.id, RunStatus::WaitingForHumanInput)?;
        let workspace = self.context.store.load_workspace()?;
        let session = self.context.store.load_session(&run.id.session_id())?;
        let steps = self.context.store.load_run_steps(&run.id)?;
        Ok(RunLoopOutput {
            workspace,
            session,
            run,
            steps,
            answer: None,
        })
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
        // Project prompts from two fact sources: static session context and
        // LLM-visible steps. Skill transcripts, errors, and canceled-output
        // checkpoints remain available for recovery/audit but do not cross the
        // model-context seam.
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

    fn emit_skills_selected(&self, events: &mut impl AgentEventSink) {
        let mut skills = self
            .context
            .active_skills
            .iter()
            .chain(&self.context.requested_skills)
            .map(|skill| skill.name.clone())
            .collect::<Vec<_>>();
        skills.sort();
        skills.dedup();
        if skills.is_empty() {
            return;
        }
        events.emit(AgentEvent::new(
            AgentEventId::skills(),
            AgentEventKind::Output,
            AgentEventData::SkillsSelected { skills },
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

struct PartialOutputEventSink<'a, S> {
    inner: &'a mut S,
    partial_output: String,
}

impl<'a, S> PartialOutputEventSink<'a, S> {
    fn new(inner: &'a mut S) -> Self {
        Self {
            inner,
            partial_output: String::new(),
        }
    }
}

impl<S> AgentEventSink for PartialOutputEventSink<'_, S>
where
    S: AgentEventSink,
{
    fn emit(&mut self, event: AgentEvent) {
        if let AgentEventData::AssistantTextDelta { content } = &event.data {
            self.partial_output.push_str(content);
        }
        self.inner.emit(event);
    }
}

struct AssistantTurn {
    final_answer: String,
    tool_calls: Vec<AssistantResponseItem>,
    items: Vec<AssistantResponseItem>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ToolCallOutcome {
    Completed,
    WaitingForHumanInput,
}

enum SkillSubflowOutcome {
    Completed(serde_json::Value),
    WaitingForHumanInput,
}

struct SkillCompletionContext {
    messages: Vec<Message>,
    tools: Vec<ToolDefinition>,
}

struct SkillInvocation {
    id: String,
    call_id: Option<String>,
    skill_name: String,
    task: String,
}

enum PendingHumanInput {
    Main {
        tool_call: ToolCallRequest,
    },
    Skill {
        invocation: SkillInvocation,
        tool_call: ToolCallRequest,
    },
}

impl PendingHumanInput {
    fn tool_call(&self) -> &ToolCallRequest {
        match self {
            Self::Main { tool_call } | Self::Skill { tool_call, .. } => tool_call,
        }
    }
}

enum ResumeTarget {
    Main(Run),
    Skill {
        run: Run,
        invocation: SkillInvocation,
    },
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

#[derive(Debug, Deserialize)]
struct SkillCallArgs {
    name: String,
    task: String,
}

fn human_input_tool_call(tool_calls: &[AssistantResponseItem]) -> Option<&AssistantResponseItem> {
    tool_calls.iter().find(|item| {
        matches!(
            item,
            AssistantResponseItem::ToolCall { name, .. } if name == HUMAN_INPUT_TOOL_NAME
        )
    })
}

fn latest_pending_human_input(steps: &[Step]) -> Option<PendingHumanInput> {
    let mut resolved_main_ids = HashSet::new();
    let mut resolved_skill_ids = HashSet::new();
    for step in steps {
        match &step.payload {
            StepPayload::ToolResult { id, .. } => {
                resolved_main_ids.insert(id.clone());
            }
            StepPayload::SkillToolResult {
                invocation_id, id, ..
            } => {
                resolved_skill_ids.insert((invocation_id.clone(), id.clone()));
            }
            _ => {}
        }
    }

    steps.iter().rev().find_map(|step| match &step.payload {
        StepPayload::AssistantResponse { items, .. } => {
            unresolved_human_input_call(items, |id| resolved_main_ids.contains(id))
                .map(|tool_call| PendingHumanInput::Main { tool_call })
        }
        StepPayload::SkillAssistantResponse {
            invocation_id,
            items,
            ..
        } => {
            let tool_call = unresolved_human_input_call(items, |id| {
                resolved_skill_ids.contains(&(invocation_id.clone(), id.to_owned()))
            })?;
            let invocation = skill_invocation(steps, invocation_id)?;
            Some(PendingHumanInput::Skill {
                invocation,
                tool_call,
            })
        }
        _ => None,
    })
}

fn unresolved_human_input_call(
    items: &[AssistantResponseItem],
    is_resolved: impl Fn(&str) -> bool,
) -> Option<ToolCallRequest> {
    items.iter().rev().find_map(|item| {
        let tool_call = ToolCallRequest::from_assistant_item(item)?;
        (tool_call.name == HUMAN_INPUT_TOOL_NAME && !is_resolved(&tool_call.id))
            .then_some(tool_call)
    })
}

fn skill_invocation(steps: &[Step], invocation_id: &str) -> Option<SkillInvocation> {
    steps.iter().find_map(|step| {
        let StepPayload::SkillStarted {
            invocation_id: id,
            call_id,
            skill_name,
            task,
        } = &step.payload
        else {
            return None;
        };
        (id == invocation_id).then(|| SkillInvocation {
            id: id.clone(),
            call_id: call_id.clone(),
            skill_name: skill_name.clone(),
            task: task.clone(),
        })
    })
}

fn parent_messages_before_skill_call(
    mut messages: Vec<Message>,
    steps: &[Step],
    run_id: &str,
    invocation_id: &str,
) -> Result<Vec<Message>, String> {
    for step in steps {
        if step.id.run_id().as_str() == run_id && step_contains_tool_call(step, invocation_id) {
            return Ok(messages);
        }
        if step.visible_to_llm() {
            messages.extend(step_to_chat_message(step));
        }
    }
    Err(format!(
        "parent assistant response for skill invocation was not found: {invocation_id}"
    ))
}

fn step_contains_tool_call(step: &Step, invocation_id: &str) -> bool {
    let StepPayload::AssistantResponse { items, .. } = &step.payload else {
        return false;
    };
    items.iter().any(|item| {
        matches!(
            item,
            AssistantResponseItem::ToolCall { id, .. } if id == invocation_id
        )
    })
}

fn inject_skill_instructions(messages: &mut Vec<Message>, instructions: &str) {
    if let Some(Message::System { content }) = messages
        .iter_mut()
        .find(|message| matches!(message, Message::System { .. }))
    {
        content.push_str("\n\n");
        content.push_str(instructions);
    } else {
        messages.insert(
            0,
            Message::System {
                content: instructions.to_owned(),
            },
        );
    }
}

fn append_skill_transcript(
    messages: &mut Vec<Message>,
    invocation: &SkillInvocation,
    steps: &[Step],
) -> Result<(), String> {
    for step in steps {
        match &step.payload {
            StepPayload::SkillAssistantResponse {
                invocation_id,
                message_id,
                items,
            } if invocation_id == &invocation.id => {
                let message = assistant_response_to_chat_message(message_id, items)
                    .ok_or_else(|| "skill subflow has an empty assistant response".to_owned())?;
                messages.push(message);
            }
            StepPayload::SkillToolResult {
                invocation_id,
                id,
                call_id,
                content,
            } if invocation_id == &invocation.id => {
                messages.push(tool_result_chat_message(
                    &ToolCallRequest {
                        id: id.clone(),
                        call_id: call_id.clone(),
                        name: String::new(),
                        arguments: serde_json::Value::Null,
                    },
                    content.clone(),
                ));
            }
            _ => {}
        }
    }
    Ok(())
}

fn skill_iteration_count(steps: &[Step], invocation_id: &str) -> usize {
    steps
        .iter()
        .filter(|step| {
            matches!(
                &step.payload,
                StepPayload::SkillAssistantResponse {
                    invocation_id: id,
                    ..
                } if id == invocation_id
            )
        })
        .count()
}

fn requested_skill_names(steps: &[Step]) -> Option<Vec<String>> {
    steps.iter().find_map(|step| {
        let StepPayload::SkillsRequested { names } = &step.payload else {
            return None;
        };
        Some(names.clone())
    })
}

fn explicit_skill_invocation_id(index: usize) -> String {
    format!("jux_explicit_skill_{index}")
}

fn has_tool_result(steps: &[Step], tool_call_id: &str) -> bool {
    steps.iter().any(|step| {
        matches!(
            &step.payload,
            StepPayload::ToolResult { id, .. } if id == tool_call_id
        )
    })
}

fn skill_result(invocation: &SkillInvocation, summary: String) -> serde_json::Value {
    json!({
        "success": true,
        "skill": invocation.skill_name,
        "summary": summary,
    })
}

fn validate_human_input(arguments: &serde_json::Value, input: &str) -> Result<(), String> {
    crate::HumanInputRequest::parse(arguments)?.validate(input)
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

struct LlmResponse {
    choice: OneOrMany<AssistantContent>,
    usage: Usage,
    message_id: Option<String>,
}

fn step_to_chat_message(step: &Step) -> Option<Message> {
    match &step.payload {
        StepPayload::UserMessage { content } => {
            Some(Message::user(workspace_relative_references(content)))
        }
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
        StepPayload::SkillsRequested { .. }
        | StepPayload::AssistantOutputCheckpoint { .. }
        | StepPayload::SkillStarted { .. }
        | StepPayload::SkillAssistantResponse { .. }
        | StepPayload::SkillToolResult { .. }
        | StepPayload::Error { .. } => None,
    }
}

fn tool_result_chat_message(tool_call: &ToolCallRequest, content: serde_json::Value) -> Message {
    Message::User {
        content: rig::OneOrMany::one(rig::message::UserContent::ToolResult(ToolResult {
            id: tool_call.id.clone(),
            call_id: tool_call.call_id.clone(),
            content: rig::OneOrMany::one(ToolResultContent::text(content.to_string())),
        })),
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
            let arguments = workspace_relative_exec_arguments(name, arguments);
            let tool_call = ToolCall::new(id.clone(), ToolFunction::new(name.clone(), arguments));
            let tool_call = match call_id {
                Some(call_id) => tool_call.with_call_id(call_id.clone()),
                None => tool_call,
            };
            Some(AssistantContent::ToolCall(tool_call))
        }
        AssistantResponseItem::Reasoning { .. } => None,
    }
}

fn workspace_relative_references(content: &str) -> String {
    content
        .replace("@{/workspace/", "@{")
        .replace("@/workspace/", "@")
}

fn workspace_relative_exec_arguments(
    name: &str,
    arguments: &serde_json::Value,
) -> serde_json::Value {
    let mut arguments = arguments.clone();
    if name != "exec" {
        return arguments;
    }
    let Some(paths) = arguments
        .get_mut("args")
        .and_then(serde_json::Value::as_array_mut)
    else {
        return arguments;
    };
    for path in paths {
        let Some(value) = path.as_str() else {
            continue;
        };
        let relative = if value == "/workspace" {
            Some(".")
        } else {
            value.strip_prefix("/workspace/")
        };
        if let Some(relative) = relative {
            *path = serde_json::Value::String(relative.to_owned());
        }
    }
    arguments
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
    active_skills: &[crate::SkillDefinition],
) -> Vec<(SessionContextKind, SessionContextPayload)> {
    let mut items = Vec::new();
    items.push((
        SessionContextKind::SystemPrompt,
        SessionContextPayload::SystemPrompt {
            content: system_prompt_with_context(instructions, skills, active_skills),
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
    if !skills.is_empty() {
        let tool = crate::call_skill_tool_definition();
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
    active_skills: &[crate::SkillDefinition],
) -> String {
    if instructions.is_empty() && skills.is_empty() && active_skills.is_empty() {
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
    let _ = active_skills;
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
    Canceled {
        run: Option<Box<Run>>,
    },
}

impl Display for RunLoopError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Store(error) => write!(formatter, "run loop store error: {error}"),
            Self::Prompt { source, .. } => write!(formatter, "run loop prompt error: {source}"),
            Self::Runtime { message, .. } => write!(formatter, "run loop runtime error: {message}"),
            Self::Canceled { .. } => formatter.write_str("run was canceled"),
        }
    }
}

impl Error for RunLoopError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Store(error) => Some(error),
            Self::Prompt { source, .. } => Some(source),
            Self::Runtime { .. } => None,
            Self::Canceled { .. } => None,
        }
    }
}

impl From<StoreError> for RunLoopError {
    fn from(error: StoreError) -> Self {
        Self::Store(error)
    }
}
