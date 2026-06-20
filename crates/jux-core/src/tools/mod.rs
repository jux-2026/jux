pub(crate) mod lua;
pub(crate) mod wasm;

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
    vec![Box::new(exec_tool()), Box::new(lua_tool())]
}
