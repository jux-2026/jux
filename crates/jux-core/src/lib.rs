//! Core library for the Jux agent runtime.

mod config;
mod instructions;
mod main_loop;
mod policy;
mod skills;
mod state;
mod tools;
mod util;

pub use config::{
    AgentConfig, ConfigError, FilesystemConfig, FilesystemPermissionConfig,
    FilesystemRuleBaseConfig, FilesystemRuleConfig, HttpConfig, HttpMethodConfig, HttpRuleConfig,
    JuxConfig, JuxConfigLoader, LoggingConfig, LoggingLevelConfig, MatchKindConfig, ModelConfig,
    NativeConfig, NetworkConfig, ResolvedConfig, RuleEffect, SandboxConfig,
};
pub use instructions::{
    InstructionDocument, InstructionError, InstructionResolver, InstructionScope,
    render_instruction_documents,
};
pub use main_loop::{
    AgentEvent, AgentEventData, AgentEventId, AgentEventKind, AgentEventSink, NoopAgentEventSink,
    RunLoop, RunLoopContext, RunLoopError, RunLoopOutput, SYSTEM_PROMPT,
};
pub use policy::{
    MatchPattern, MatchPatternKind, NativeCommandPolicy, NativeCommandRule, RuntimePolicy,
    WasmEnvironmentPolicy, WasmFilesystemAccess, WasmFilesystemDecision, WasmFilesystemPermissions,
    WasmFilesystemPolicy, WasmFilesystemRule, WasmFilesystemRuleBase, WasmHttpDecision,
    WasmHttpMatchKind, WasmHttpMethod, WasmHttpRule, WasmHttpRuleEffect, WasmNetworkPolicy,
    WasmPackageRule, WasmPackageSource, WasmSandboxPolicy,
};
pub use skills::{
    MAX_SKILL_FILE_BYTES, SkillCatalog, SkillDefinition, SkillError, SkillOverride, SkillResolver,
    SkillScope, match_auto_skills, render_active_skills, render_skill_index,
    select_explicit_skills,
};
pub use state::{
    AssistantResponseItem, LlmUsage, Run, RunId, RunStatus, Session, SessionContextItem,
    SessionContextKind, SessionContextPayload, SessionId, SqliteWorkspaceStore, Step, StepId,
    StepKind, StepPayload, StoreError, Workspace, WorkspaceId,
};
pub use tools::wasm::{
    WasmCommandDefinition, WasmCommandOutput, WasmCommandRequest, WasmEnvironmentCapability,
    WasmEnvironmentPermission, WasmFilesystemCapability, WasmFilesystemPermission,
    WasmNetworkCapability, WasmNetworkPermission, WasmPackageLoadingCapability, WasmPermissions,
    WasmRuntimeError, WasmStdioCapability, WasmerRuntime, WasmerRuntimeCapabilities,
    available_wasm_command_names, available_wasm_commands,
};

/// Returns the current workspace package version.
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
