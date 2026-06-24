//! Tool registry and execution boundary.
//!
//! This module exposes the tools that the run loop can advertise to the LLM and
//! execute after a tool call. Tool implementations receive a narrow
//! `ToolExecutionContext` instead of the full run-loop context so each tool can
//! access policy-controlled capabilities without reaching into persistence or
//! model state.
//!
//! Concrete tool families live in submodules such as `lua` and `wasm`.

mod human_input;
pub(crate) mod lua;
pub(crate) mod wasm;

pub(crate) use self::human_input::HUMAN_INPUT_TOOL_NAME;
use self::human_input::human_input_tool;
use self::lua::lua_tool;
use self::wasm::exec_tool;
use crate::RuntimePolicy;
use rig::completion::ToolDefinition;

pub(crate) trait JuxTool {
    fn name(&self) -> &'static str;
    fn definition(&self) -> ToolDefinition;
    fn execute(
        &self,
        context: &dyn ToolExecutionContext,
        args: &serde_json::Value,
    ) -> Result<serde_json::Value, String>;
}

pub(crate) trait ToolExecutionContext {
    fn policy(&self) -> &RuntimePolicy;
}

impl ToolExecutionContext for RuntimePolicy {
    fn policy(&self) -> &RuntimePolicy {
        self
    }
}

pub(crate) fn tool_definitions() -> Vec<ToolDefinition> {
    tools().into_iter().map(|tool| tool.definition()).collect()
}

pub(crate) fn execute_tool(
    context: &dyn ToolExecutionContext,
    tool_name: &str,
    args: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    let Some(tool) = tools().into_iter().find(|tool| tool.name() == tool_name) else {
        return Err(format!("unsupported tool call: {tool_name}"));
    };
    tool.execute(context, args)
}

fn tools() -> Vec<Box<dyn JuxTool>> {
    vec![
        Box::new(exec_tool()),
        Box::new(lua_tool()),
        Box::new(human_input_tool()),
    ]
}
