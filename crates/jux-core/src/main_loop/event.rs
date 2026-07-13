use serde::{Deserialize, Serialize};
use std::fmt::{self, Display};

use crate::util::time::now_millis;

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
    pub fn skills() -> Self {
        Self("run.skills".to_owned())
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
    pub sequence: u64,
    pub timestamp: u128,
    pub id: AgentEventId,
    pub parent_id: Option<AgentEventId>,
    pub kind: AgentEventKind,
    pub data: AgentEventData,
}

impl AgentEvent {
    #[must_use]
    pub fn new(id: AgentEventId, kind: AgentEventKind, data: AgentEventData) -> Self {
        let parent_id = id.parent();
        Self {
            sequence: 0,
            timestamp: now_millis(),
            id,
            parent_id,
            kind,
            data,
        }
    }
}

impl AgentEventId {
    fn parent(&self) -> Option<Self> {
        let (parent, _) = self.0.rsplit_once('.')?;
        Some(Self(parent.to_owned()))
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

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum ToolOutputStream {
    Stdout,
    Stderr,
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
    SkillsSelected {
        skills: Vec<String>,
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
    AssistantTextDelta {
        content: String,
    },
    AssistantReasoningDelta {
        content: String,
    },
    ToolCallDelta {
        call_id: String,
        content: String,
    },
    UsageDelta {
        usage: crate::state::LlmUsage,
    },
    OutputCompleted,
    ToolStarted {
        name: String,
        call_id: Option<String>,
        arguments: serde_json::Value,
    },
    ToolOutput {
        name: String,
        content: serde_json::Value,
    },
    ToolOutputChunk {
        name: String,
        stream: ToolOutputStream,
        content: String,
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

/// Assigns one monotonic sequence to every event emitted by a run.
pub struct SequencedAgentEventSink<'a, S> {
    inner: &'a mut S,
    next_sequence: u64,
}

impl<'a, S> SequencedAgentEventSink<'a, S> {
    #[must_use]
    pub fn new(inner: &'a mut S) -> Self {
        Self {
            inner,
            next_sequence: 1,
        }
    }
}

impl<S> AgentEventSink for SequencedAgentEventSink<'_, S>
where
    S: AgentEventSink,
{
    fn emit(&mut self, mut event: AgentEvent) {
        event.sequence = self.next_sequence;
        self.next_sequence += 1;
        self.inner.emit(event);
    }
}

#[derive(Default)]
/// Event sink that discards all events.
pub struct NoopAgentEventSink;

impl AgentEventSink for NoopAgentEventSink {
    fn emit(&mut self, _event: AgentEvent) {}
}
