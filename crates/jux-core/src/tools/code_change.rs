use crate::tools::{JuxTool, ToolExecutionContext};
use crate::{CodeChangePlan, CodeChangeProposal, ProposedFileContent};
use rig::completion::ToolDefinition;
use serde::Deserialize;
use serde_json::json;

pub const PROPOSE_CODE_CHANGE_TOOL_NAME: &str = "propose_code_change";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProposeCodeChangeArgs {
    plan: CodeChangePlan,
    files: Vec<ProposedFileContent>,
}

pub(crate) struct ProposeCodeChangeTool;

impl JuxTool for ProposeCodeChangeTool {
    fn name(&self) -> &'static str {
        PROPOSE_CODE_CHANGE_TOOL_NAME
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: PROPOSE_CODE_CHANGE_TOOL_NAME.to_owned(),
            description: "Prepare a reviewable code change proposal without writing files. \
Use this tool for all source and documentation modifications."
                .to_owned(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "plan": {
                        "type": "object",
                        "properties": {
                            "summary": { "type": "string" },
                            "items": {
                                "type": "array",
                                "items": { "type": "string" }
                            }
                        },
                        "required": ["summary", "items"]
                    },
                    "files": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "path": { "type": "string" },
                                "new_content": { "type": "string" }
                            },
                            "required": ["path", "new_content"]
                        }
                    }
                },
                "required": ["plan", "files"]
            }),
        }
    }

    fn execute(
        &self,
        context: &dyn ToolExecutionContext,
        args: &serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        let args = serde_json::from_value::<ProposeCodeChangeArgs>(args.clone())
            .map_err(|error| format!("invalid propose_code_change arguments: {error}"))?;
        let proposal =
            CodeChangeProposal::prepare(&context.policy().workspace_root, args.plan, args.files)
                .map_err(|error| error.to_string())?;
        serde_json::to_value(proposal)
            .map_err(|error| format!("failed to serialize code change proposal: {error}"))
    }
}
