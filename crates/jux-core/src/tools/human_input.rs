use crate::tools::{JuxTool, ToolExecutionContext};
use crate::{AssistantResponseItem, Step, StepPayload};
use rig::completion::ToolDefinition;
use serde::{Deserialize, Serialize};
use serde_json::json;

pub const HUMAN_INPUT_TOOL_NAME: &str = "human_input";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HumanInputRequest {
    #[serde(default)]
    pub kind: HumanInputKind,
    pub prompt: String,
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub options: Vec<HumanInputOption>,
    #[serde(default)]
    pub allow_free_text: bool,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HumanInputKind {
    #[default]
    Clarification,
    Confirmation,
}

impl HumanInputRequest {
    pub fn parse(arguments: &serde_json::Value) -> Result<Self, String> {
        serde_json::from_value(arguments.clone())
            .map_err(|error| format!("invalid human_input tool arguments: {error}"))
    }

    pub fn validate(&self, input: &str) -> Result<(), String> {
        if self.allow_free_text || self.options.iter().any(|option| option.id == input) {
            return Ok(());
        }
        let option_ids = self
            .options
            .iter()
            .map(|option| option.id.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        Err(format!(
            "human input must match one of the option ids: {option_ids}"
        ))
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HumanInputOption {
    pub id: String,
    pub label: String,
}

#[must_use]
pub fn latest_human_input_request(steps: &[Step]) -> Option<HumanInputRequest> {
    steps.iter().rev().find_map(|step| {
        let items = match &step.payload {
            StepPayload::AssistantResponse { items, .. }
            | StepPayload::SkillAssistantResponse { items, .. } => items,
            _ => return None,
        };
        items.iter().rev().find_map(|item| {
            let AssistantResponseItem::ToolCall {
                name, arguments, ..
            } = item
            else {
                return None;
            };
            (name == HUMAN_INPUT_TOOL_NAME)
                .then(|| HumanInputRequest::parse(arguments).ok())
                .flatten()
        })
    })
}

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
                    "kind": {
                        "type": "string",
                        "enum": ["clarification", "confirmation"]
                    },
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
