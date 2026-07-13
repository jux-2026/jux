use jux_core::{
    AgentEvent, RunCancellationHandle, RunCancellationToken, RunLoopOutput, RunStatus, Step,
    run_cancellation_pair,
};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

#[derive(Clone, Debug, PartialEq)]
pub struct RunResponse {
    pub session_id: String,
    pub run_id: String,
    pub status: RunStatus,
    pub created_at: u128,
    pub updated_at: u128,
    pub answer: Option<String>,
    pub steps: Vec<Step>,
}

impl From<RunLoopOutput> for RunResponse {
    fn from(output: RunLoopOutput) -> Self {
        Self {
            session_id: output.session.id.to_string(),
            run_id: output.run.id.to_string(),
            status: output.run.status,
            created_at: output.run.created_at,
            updated_at: output.run.updated_at,
            answer: output.answer,
            steps: output.steps,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TuiRunRequest {
    pub request: String,
    pub explicit_skills: Vec<String>,
}

impl TuiRunRequest {
    #[must_use]
    pub fn new(request: impl Into<String>, explicit_skills: Vec<String>) -> Self {
        Self {
            request: request.into(),
            explicit_skills,
        }
    }
}

impl From<String> for TuiRunRequest {
    fn from(request: String) -> Self {
        Self::new(request, Vec::new())
    }
}

pub trait RunHandler: Send + Sync + 'static {
    fn run(
        &self,
        request: TuiRunRequest,
        cancellation: RunCancellationToken,
        events: AgentEventSender,
    ) -> Result<RunResponse, String>;
}

impl<F> RunHandler for F
where
    F: Fn(TuiRunRequest, RunCancellationToken, AgentEventSender) -> Result<RunResponse, String>
        + Send
        + Sync
        + 'static,
{
    fn run(
        &self,
        request: TuiRunRequest,
        cancellation: RunCancellationToken,
        events: AgentEventSender,
    ) -> Result<RunResponse, String> {
        self(request, cancellation, events)
    }
}

const AGENT_EVENT_CHANNEL_CAPACITY: usize = 256;

#[derive(Clone)]
pub struct AgentEventSender(mpsc::SyncSender<AgentEvent>);

impl AgentEventSender {
    pub fn send(&self, event: AgentEvent) {
        let _ = self.0.send(event);
    }
}

pub struct BackgroundRun {
    receiver: Receiver<Result<RunResponse, String>>,
    event_receiver: Receiver<AgentEvent>,
    cancellation: RunCancellationHandle,
    cancel_requested: Arc<AtomicBool>,
}

impl BackgroundRun {
    #[must_use]
    pub fn start(request: impl Into<TuiRunRequest>, handler: Arc<dyn RunHandler>) -> Self {
        let request = request.into();
        let (sender, receiver) = mpsc::channel();
        let (event_sender, event_receiver) = mpsc::sync_channel(AGENT_EVENT_CHANNEL_CAPACITY);
        let (cancellation, token) = run_cancellation_pair();
        let cancel_requested = Arc::new(AtomicBool::new(false));
        std::thread::spawn(move || {
            let result = handler.run(request, token, AgentEventSender(event_sender));
            let _ = sender.send(result);
        });
        Self {
            receiver,
            event_receiver,
            cancellation,
            cancel_requested,
        }
    }

    pub fn cancel(&self) {
        self.cancel_requested.store(true, Ordering::Release);
        self.cancellation.cancel();
    }

    #[must_use]
    pub fn is_cancel_requested(&self) -> bool {
        self.cancel_requested.load(Ordering::Acquire)
    }

    pub fn try_recv(&self) -> Option<Result<RunResponse, String>> {
        self.receiver.try_recv().ok()
    }

    pub fn recv_timeout(&self, timeout: Duration) -> Option<Result<RunResponse, String>> {
        self.receiver.recv_timeout(timeout).ok()
    }

    pub fn try_recv_event(&self) -> Option<AgentEvent> {
        self.event_receiver.try_recv().ok()
    }

    pub fn recv_event_timeout(&self, timeout: Duration) -> Option<AgentEvent> {
        self.event_receiver.recv_timeout(timeout).ok()
    }
}
