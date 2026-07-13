use crate::instructions::InstructionDocument;
use crate::policy::RuntimePolicy;
use crate::skills::SkillDefinition;
use crate::state::SqliteWorkspaceStore;
use crate::tools::ToolExecutionContext;

/// Complete runtime context owned by the agent run loop.
///
/// `RunLoopContext` is the root context for one Jux agent runtime. It owns the
/// top-level infrastructure and policy required by the run loop, including
/// persistence, LLM access, and execution permissions.
///
/// This type is dependency context, not the prompt sent to the model. Model
/// context is projected separately from persisted session context and visible
/// steps. Keeping those meanings separate prevents infrastructure handles,
/// policy state, or an unbounded session history from leaking into prompts.
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
    pub instructions: Vec<InstructionDocument>,
    pub skills: Vec<SkillDefinition>,
    pub requested_skills: Vec<SkillDefinition>,
    pub active_skills: Vec<SkillDefinition>,
    pub stream_model_output: bool,
}

impl<M> RunLoopContext<M> {
    #[must_use]
    pub fn new(store: SqliteWorkspaceStore, model: M, policy: RuntimePolicy) -> Self {
        Self {
            store,
            model,
            policy,
            instructions: Vec::new(),
            skills: Vec::new(),
            requested_skills: Vec::new(),
            active_skills: Vec::new(),
            stream_model_output: false,
        }
    }

    #[must_use]
    pub fn with_instructions(mut self, instructions: Vec<InstructionDocument>) -> Self {
        self.instructions = instructions;
        self
    }

    #[must_use]
    pub fn with_skills(mut self, skills: Vec<SkillDefinition>) -> Self {
        self.skills = skills;
        self
    }

    #[must_use]
    pub fn with_requested_skills(mut self, requested_skills: Vec<SkillDefinition>) -> Self {
        self.requested_skills = requested_skills;
        self
    }

    #[must_use]
    pub fn with_active_skills(mut self, active_skills: Vec<SkillDefinition>) -> Self {
        self.active_skills = active_skills;
        self
    }

    #[must_use]
    pub fn with_model_streaming(mut self, enabled: bool) -> Self {
        self.stream_model_output = enabled;
        self
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
