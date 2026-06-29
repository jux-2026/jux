use futures::future::{AbortHandle, AbortRegistration};

#[derive(Clone, Debug)]
pub struct RunCancellationHandle(AbortHandle);

impl RunCancellationHandle {
    pub fn cancel(&self) {
        self.0.abort();
    }
}

#[derive(Debug)]
pub struct RunCancellationToken {
    pub(super) registration: AbortRegistration,
}

#[must_use]
pub fn run_cancellation_pair() -> (RunCancellationHandle, RunCancellationToken) {
    let (handle, registration) = AbortHandle::new_pair();
    (
        RunCancellationHandle(handle),
        RunCancellationToken { registration },
    )
}
