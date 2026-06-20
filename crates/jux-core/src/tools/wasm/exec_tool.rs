use super::{WasmCommandRequest, WasmerRuntime, available_wasm_command_names};
use crate::RuntimePolicy;
use crate::tools::{JuxTool, ToolExecutionContext};
use rig::completion::ToolDefinition;
use serde::Deserialize;
use serde::Serialize;
use serde_json::json;

const EXEC_TOOL_NAME: &str = "exec";

#[must_use]
pub(crate) fn exec_tool() -> WasmExecTool {
    WasmExecTool
}

pub(crate) struct WasmExecTool;

impl JuxTool for WasmExecTool {
    fn name(&self) -> &'static str {
        EXEC_TOOL_NAME
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: EXEC_TOOL_NAME.to_owned(),
            description: format!(
                "Execute one command from the Jux WASI runtime. Provide the command name in \
                program and each argument as a separate string in args. Available commands: {}. \
                Do not use shell syntax such as &&, ||, ;, |, >, <, backticks, $(), wildcard \
                expansion, or newlines. The tool returns structured execution data as JSON: \
                success, exit_code, stdout, and stderr.",
                available_wasm_command_names().join(", ")
            ),
            parameters: json!({
                "type": "object",
                "properties": {
                    "program": { "type": "string" },
                    "args": {
                        "type": "array",
                        "items": { "type": "string" }
                    }
                },
                "required": ["program", "args"]
            }),
        }
    }

    fn execute(
        &self,
        context: &ToolExecutionContext<'_>,
        args: &serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        let args = serde_json::from_value::<ExecToolArgs>(args.clone())
            .map_err(|error| format!("invalid exec tool arguments: {error}"))?;
        let output = run_exec_command(&args.program, &args.args, context.policy)?;
        serde_json::to_value(ExecToolOutput::from(output))
            .map_err(|error| format!("exec output serialization failed: {error}"))
    }
}

pub(crate) fn run_exec_command_line(
    command: &str,
    policy: &RuntimePolicy,
) -> Result<WasmCommandExecution, String> {
    reject_shell_token(command)?;
    let parts = shlex::split(command).ok_or_else(|| "invalid command syntax".to_owned())?;
    let (program, args) = parts
        .split_first()
        .ok_or_else(|| "command cannot be empty".to_owned())?;
    run_exec_command(program, args, policy)
}

fn run_exec_command(
    program: &str,
    args: &[String],
    policy: &RuntimePolicy,
) -> Result<WasmCommandExecution, String> {
    reject_shell_token(program)?;
    for arg in args {
        reject_shell_token(arg)?;
    }

    let program = program.to_owned();
    let args = args.to_vec();
    let policy = policy.clone();
    std::thread::spawn(move || run_exec_command_in_thread(program, args, policy))
        .join()
        .map_err(|_| "wasi coreutils execution thread panicked".to_owned())?
}

fn run_exec_command_in_thread(
    program: String,
    args: Vec<String>,
    policy: RuntimePolicy,
) -> Result<WasmCommandExecution, String> {
    let output = WasmerRuntime::with_wasm_policy(&policy.wasm)
        .run_coreutils_command(WasmCommandRequest {
            program,
            args,
            host_directory: policy.workspace_root,
        })
        .map_err(|error| format!("wasi coreutils execution failed: {error}"))?;

    Ok(WasmCommandExecution {
        success: output.success,
        status_code: output.exit_code,
        stdout: output.stdout,
        stderr: output.stderr,
    })
}

fn reject_shell_token(value: &str) -> Result<(), String> {
    const REJECTED_TOKENS: [&str; 12] = [
        "&&", "||", ";", "|", ">", "<", "`", "$(", "\n", "\r", "*", "?",
    ];
    if let Some(token) = REJECTED_TOKENS.iter().find(|token| value.contains(**token)) {
        return Err(format!("shell syntax is not supported: {token}"));
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
struct ExecToolArgs {
    program: String,
    args: Vec<String>,
}

#[derive(Debug)]
pub(crate) struct WasmCommandExecution {
    pub success: bool,
    pub status_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

#[derive(Debug, Serialize)]
struct ExecToolOutput {
    success: bool,
    exit_code: Option<i32>,
    stdout: String,
    stderr: String,
}

impl From<WasmCommandExecution> for ExecToolOutput {
    fn from(output: WasmCommandExecution) -> Self {
        Self {
            success: output.success,
            exit_code: output.status_code,
            stdout: output.stdout,
            stderr: output.stderr,
        }
    }
}
