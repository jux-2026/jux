use crate::policy::RuntimePolicy;
use crate::store::SqliteWorkspaceStore;
use crate::tools::ToolExecutionContext;

/// Complete runtime context owned by the agent run loop.
///
/// `RunLoopContext` is the root context for one Jux agent runtime. It owns the
/// top-level infrastructure and policy required by the run loop, including
/// persistence, LLM access, and execution permissions.
///
/// Lower-level modules should not depend on this concrete type directly unless
/// they are part of the run-loop layer. Tool, WASM, and future native execution
/// modules should depend on their own context traits instead. `RunLoopContext`
/// can implement those traits and expose only the subset of state each module
/// needs.
pub struct RunLoopContext<M> {
    pub store: SqliteWorkspaceStore,
    pub model: M,
    pub policy: RuntimePolicy,
}

impl<M> RunLoopContext<M> {
    #[must_use]
    pub fn new(store: SqliteWorkspaceStore, model: M, policy: RuntimePolicy) -> Self {
        Self {
            store,
            model,
            policy,
        }
    }

    #[must_use]
    pub fn workspace_default(store: SqliteWorkspaceStore, model: M) -> Self {
        let policy = RuntimePolicy::workspace_default(store.root().to_path_buf());
        Self::new(store, model, policy)
    }
}

impl<M> ToolExecutionContext for RunLoopContext<M> {
    fn policy(&self) -> &RuntimePolicy {
        &self.policy
    }
}
