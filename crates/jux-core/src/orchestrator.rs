use crate::model::{Run, RunStatus, Session, Step, StepKind, StepPayload, Workspace};
use crate::store::{SqliteWorkspaceStore, StoreError};
use rig::completion::{Prompt, PromptError};
use serde::Deserialize;
use std::error::Error;
use std::fmt::{self, Display};

const MAX_LOOP_ITERATIONS: usize = 8;
const ECHO_TOOL_NAME: &str = "echo";

pub struct RunLoop<P> {
    store: SqliteWorkspaceStore,
    prompt: P,
}

impl<P> RunLoop<P>
where
    P: Prompt,
{
    #[must_use]
    pub fn new(store: SqliteWorkspaceStore, prompt: P) -> Self {
        Self { store, prompt }
    }

    pub async fn run(&self, request: String) -> Result<RunLoopOutput, RunLoopError> {
        let run = self.store.create_run(request.clone())?;
        tracing::info!(run_id = %run.id, "created run");
        self.store.append_step(
            &run.id,
            StepKind::UserRequest,
            StepPayload::UserRequest { content: request },
        )?;

        for _ in 0..MAX_LOOP_ITERATIONS {
            let llm_prompt = self.build_prompt(&run)?;
            tracing::debug!(run_id = %run.id, "built llm prompt");

            let response = match self.prompt.prompt(llm_prompt.clone()).await {
                Ok(response) => response,
                Err(error) => return self.fail_prompt_call(run, llm_prompt, error),
            };

            self.store.append_step(
                &run.id,
                StepKind::LlmCall,
                StepPayload::LlmCall {
                    prompt: llm_prompt,
                    response: Some(response.clone()),
                },
            )?;

            match LlmDecision::parse(&response) {
                Ok(LlmDecision::FinalAnswer { answer }) => {
                    return self.complete_run(run, answer);
                }
                Ok(LlmDecision::ToolCall { tool_name, input }) => {
                    self.store.append_step(
                        &run.id,
                        StepKind::AssistantToolCall,
                        StepPayload::AssistantToolCall {
                            tool_name: tool_name.clone(),
                            input: input.clone(),
                        },
                    )?;
                    tracing::info!(run_id = %run.id, tool_name = %tool_name, "executing tool call");

                    let output = match execute_tool(&tool_name, &input) {
                        Ok(output) => output,
                        Err(error) => return self.fail_runtime(run, error),
                    };
                    self.store.append_step(
                        &run.id,
                        StepKind::ToolResult,
                        StepPayload::ToolResult {
                            tool_name,
                            content: output,
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

    // Finalizes a run after the LLM has produced a final answer: persist the
    // user-visible assistant message, mark the run as complete, then reload the
    // fact steps so the CLI can report the finished execution trace.
    fn complete_run(&self, run: Run, answer: String) -> Result<RunLoopOutput, RunLoopError> {
        self.store.append_step(
            &run.id,
            StepKind::AssistantMessage,
            StepPayload::AssistantMessage {
                content: answer.clone(),
            },
        )?;

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
        prompt: String,
        error: PromptError,
    ) -> Result<RunLoopOutput, RunLoopError> {
        let message = error.to_string();
        self.store.append_step(
            &run.id,
            StepKind::LlmCall,
            StepPayload::LlmCall {
                prompt,
                response: None,
            },
        )?;
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

    fn build_prompt(&self, run: &Run) -> Result<String, RunLoopError> {
        let steps = self.store.load_run_steps(&run.id)?;
        let visible_context = steps
            .iter()
            .filter(|step| step.visible_to_llm())
            .filter_map(Step::to_llm_line)
            .collect::<Vec<_>>()
            .join("\n");

        Ok(format!(
            "You are Jux, a concise coding agent.\n\
             Return JSON only. Use exactly one of these shapes:\n\
             {{\"type\":\"final_answer\",\"answer\":\"...\"}}\n\
             {{\"type\":\"tool_call\",\"tool_name\":\"echo\",\"input\":\"...\"}}\n\
             Available tools:\n\
             - echo: returns the input text unchanged.\n\n\
             Context:\n{visible_context}"
        ))
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
        _ => Err(format!("unsupported tool call: {tool_name}")),
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
