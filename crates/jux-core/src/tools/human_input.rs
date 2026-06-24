use crate::tools::{JuxTool, ToolExecutionContext};
use rig::completion::ToolDefinition;
use serde_json::json;

pub(crate) const HUMAN_INPUT_TOOL_NAME: &str = "human_input";

const HUMAN_INPUT_TOOL_DESCRIPTION: &str = "Ask the user for input before continuing the run. \
Use this when the task needs human confirmation, a choice from options, or free-form user input. \
If allow_free_text is false, the user must reply with one of the option ids.";

#[must_use]
pub(crate) fn human_input_tool() -> HumanInputTool {
    HumanInputTool
}

pub(crate) struct HumanInputTool;

impl JuxTool for HumanInputTool {
    fn name(&self) -> &'static str {
        HUMAN_INPUT_TOOL_NAME
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: HUMAN_INPUT_TOOL_NAME.to_owned(),
            description: HUMAN_INPUT_TOOL_DESCRIPTION.to_owned(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "prompt": { "type": "string" },
                    "options": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "id": { "type": "string" },
                                "label": { "type": "string" }
                            },
                            "required": ["id", "label"]
                        }
                    },
                    "allow_free_text": { "type": "boolean" }
                },
                "required": ["prompt", "allow_free_text"]
            }),
        }
    }

    fn execute(
        &self,
        _context: &dyn ToolExecutionContext,
        _args: &serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        Err("human_input is handled by the run loop".to_owned())
    }
}
