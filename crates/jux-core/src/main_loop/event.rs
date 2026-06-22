use serde::{Deserialize, Serialize};
use std::fmt::{self, Display};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
/// Hierarchical event id used by streamed run-loop events.
///
/// The id is intentionally path-like so logs remain readable without requiring
/// a separate tree renderer.
pub struct AgentEventId(String);

impl AgentEventId {
    #[must_use]
    pub fn run() -> Self {
        Self("run".to_owned())
    }

    #[must_use]
    pub fn iteration(index: usize) -> Self {
        Self(format!("run.iteration.{index}"))
    }

    #[must_use]
    pub fn llm(iteration_index: usize, llm_index: usize) -> Self {
        Self(format!("run.iteration.{iteration_index}.llm.{llm_index}"))
    }

    #[must_use]
    pub fn tool(iteration_index: usize, name: &str, tool_index: usize) -> Self {
        let name = name.replace('.', "_");
        Self(format!(
            "run.iteration.{iteration_index}.tool.{name}.{tool_index}"
        ))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Display for AgentEventId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
/// Streamed event emitted by the run loop.
pub struct AgentEvent {
    pub id: AgentEventId,
    pub kind: AgentEventKind,
    pub data: AgentEventData,
}

impl AgentEvent {
    #[must_use]
    pub fn new(id: AgentEventId, kind: AgentEventKind, data: AgentEventData) -> Self {
        Self { id, kind, data }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
/// High-level event lifecycle state.
pub enum AgentEventKind {
    Started,
    Output,
    Completed,
    Failed,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
/// Strongly typed event payload.
pub enum AgentEventData {
    RunStarted {
        request: String,
    },
    RunCompleted {
        answer: String,
    },
    RunFailed {
        error: String,
    },
    IterationStarted {
        index: usize,
    },
    IterationCompleted {
        index: usize,
    },
    LlmStarted,
    LlmCompleted,
    LlmFailed {
        error: String,
    },
    ToolStarted {
        name: String,
        call_id: Option<String>,
    },
    ToolOutput {
        name: String,
        content: serde_json::Value,
    },
    ToolCompleted {
        name: String,
        call_id: Option<String>,
    },
    ToolFailed {
        name: String,
        call_id: Option<String>,
        error: String,
    },
}

/// Receives streamed run-loop events.
pub trait AgentEventSink {
    fn emit(&mut self, event: AgentEvent);
}

#[derive(Default)]
/// Event sink that discards all events.
pub struct NoopAgentEventSink;

impl AgentEventSink for NoopAgentEventSink {
    fn emit(&mut self, _event: AgentEvent) {}
}
