use crate::model::{
    AssistantResponseItem, LlmUsage, Run, RunStatus, Session, SessionContextKind,
    SessionContextPayload, Step, StepKind, StepPayload, Workspace,
};
use crate::store::{SqliteWorkspaceStore, StoreError};
use crate::wasm_runtime::{WasmCommandRequest, WasmerRuntime};
use mlua::{Lua, LuaOptions, StdLib, UserData, UserDataMethods, Value};
use rig::OneOrMany;
use rig::completion::{CompletionError, CompletionModel, ToolDefinition, Usage};
use rig::message::{
    AssistantContent, Message, ToolCall, ToolFunction, ToolResult, ToolResultContent,
};
use serde::Deserialize;
use serde::Serialize;
use serde_json::json;
use std::error::Error;
use std::fmt::{self, Display};

const MAX_LOOP_ITERATIONS: usize = 8;
const EXEC_TOOL_NAME: &str = "exec";
const LUA_TOOL_NAME: &str = "lua";
const EXEC_TOOL_DESCRIPTION: &str = "Execute one command from the Jux WASI coreutils runtime. \
Provide the command name in program and each argument as a separate string in args. \
Supported commands are basename, base32, base64, cat, dirname, echo, env, ls, mkdir, \
mv, printf, pwd, sum, and wc. Do not use shell syntax such as &&, ||, ;, |, >, <, \
backticks, $(), wildcard expansion, or newlines. The tool returns structured execution \
data as JSON: success, exit_code, stdout, and stderr.";
const LUA_TOOL_DESCRIPTION: &str = "Execute Lua code in a restricted Jux Lua runtime. \
All Lua standard libraries are disabled by default. Only these globals are available: \
os.execute(command), which executes one non-shell command; and io.popen(command, 'r'), \
which executes one non-shell command and returns a readable handle. Commands are parsed \
into one program plus arguments and are not executed through a shell. Shell syntax such \
as &&, ||, ;, |, >, <, backticks, $(), wildcard expansion, and newlines is rejected. \
io.popen handles support read('*a'), read('*l'), lines(), and close(). Return the first \
Lua value as the tool result. Do not call print. Use return to send the result back to Jux.";
pub const SYSTEM_PROMPT: &str = "You are Jux, a concise coding agent.";

impl From<Usage> for LlmUsage {
    fn from(usage: Usage) -> Self {
        Self {
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
            total_tokens: usage.total_tokens,
            cached_input_tokens: usage.cached_input_tokens,
            cache_creation_input_tokens: usage.cache_creation_input_tokens,
        }
    }
}

pub struct RunLoop<M> {
    store: SqliteWorkspaceStore,
    model: M,
}

impl<M> RunLoop<M>
where
    M: CompletionModel,
{
    #[must_use]
    pub fn new(store: SqliteWorkspaceStore, model: M) -> Self {
        Self { store, model }
    }

    pub async fn run(&self, request: String) -> Result<RunLoopOutput, RunLoopError> {
        let run = self.store.create_run(request.clone())?;
        tracing::info!(run_id = %run.id, "created run");
        self.store
            .ensure_session_context_items(&run.id.session_id(), default_session_context_items())?;
        self.store.append_step(
            &run.id,
            StepKind::UserMessage,
            StepPayload::UserMessage { content: request },
        )?;

        for _ in 0..MAX_LOOP_ITERATIONS {
            let llm_request = self.build_completion_request(&run)?;
            tracing::debug!(
                run_id = %run.id,
                history_len = llm_request.history.len(),
                "built llm completion request"
            );

            let request = self
                .model
                .completion_request(llm_request.prompt)
                .messages(llm_request.history)
                .tools(llm_request.tools)
                .build();
            let response = match self.model.completion(request).await {
                Ok(response) => response,
                Err(error) => return self.fail_completion_call(run, error),
            };

            let mut final_answer = String::new();
            let mut tool_calls = Vec::new();
            let mut items = Vec::new();
            for content in response.choice.into_iter() {
                match content {
                    AssistantContent::Text(text) => {
                        final_answer.push_str(&text.text);
                        items.push(AssistantResponseItem::Text { content: text.text });
                    }
                    AssistantContent::ToolCall(tool_call) => {
                        let item = AssistantResponseItem::ToolCall {
                            id: tool_call.id.clone(),
                            call_id: tool_call.call_id.clone(),
                            name: tool_call.function.name.clone(),
                            arguments: tool_call.function.arguments.clone(),
                        };
                        tool_calls.push(item.clone());
                        items.push(item);
                    }
                    AssistantContent::Reasoning(reasoning) => {
                        let text = reasoning.display_text();
                        if !text.is_empty() {
                            items.push(AssistantResponseItem::Reasoning { content: text });
                        }
                    }
                    AssistantContent::Image(_) => {
                        return self.fail_runtime(
                            run,
                            "LLM returned an unsupported image response".to_owned(),
                        );
                    }
                }
            }
            self.store.append_step(
                &run.id,
                StepKind::AssistantResponse,
                StepPayload::AssistantResponse {
                    message_id: response.message_id,
                    usage: LlmUsage::from(response.usage),
                    items,
                },
            )?;

            for tool_call in &tool_calls {
                let AssistantResponseItem::ToolCall {
                    id,
                    call_id,
                    name,
                    arguments,
                } = tool_call
                else {
                    continue;
                };
                tracing::info!(
                    run_id = %run.id,
                    tool_name = %name,
                    "executing tool call"
                );

                let content = match execute_tool(name, arguments) {
                    Ok(output) => output,
                    Err(error) => {
                        tracing::warn!(
                            run_id = %run.id,
                            tool_name = %name,
                            error = %error,
                            "tool call failed"
                        );
                        json!({
                            "success": false,
                            "error": error,
                        })
                    }
                };
                self.store.append_step(
                    &run.id,
                    StepKind::ToolResult,
                    StepPayload::ToolResult {
                        id: id.clone(),
                        call_id: call_id.clone(),
                        content,
                    },
                )?;
            }

            if tool_calls.is_empty() {
                return self.complete_run(run, final_answer);
            }
        }

        self.fail_runtime(
            run,
            "run loop reached the maximum number of iterations".to_owned(),
        )
    }

    fn complete_run(&self, run: Run, answer: String) -> Result<RunLoopOutput, RunLoopError> {
        let run = self
            .store
            .update_run_status(&run.id, RunStatus::Completed)?;
        let workspace = self.store.load_workspace()?;
        let session = self.store.load_session(&run.id.session_id())?;
        let steps = self.store.load_run_steps(&run.id)?;
        tracing::info!(
            run_id = %run.id,
            step_count = steps.len(),
            "completed run"
        );

        Ok(RunLoopOutput {
            workspace,
            session,
            run,
            steps,
            answer: Some(answer),
        })
    }

    fn fail_completion_call(
        &self,
        run: Run,
        error: CompletionError,
    ) -> Result<RunLoopOutput, RunLoopError> {
        let message = error.to_string();
        self.store
            .append_step(&run.id, StepKind::Error, StepPayload::Error { message })?;

        let run = self.store.update_run_status(&run.id, RunStatus::Failed)?;
        tracing::error!(run_id = %run.id, "failed run");
        Err(RunLoopError::Prompt {
            run: Box::new(run),
            source: Box::new(error),
        })
    }

    fn fail_runtime(&self, run: Run, message: String) -> Result<RunLoopOutput, RunLoopError> {
        self.record_runtime_error(&run, message.clone())?;
        let run = self.store.update_run_status(&run.id, RunStatus::Failed)?;
        tracing::error!(run_id = %run.id, "failed run");
        Err(RunLoopError::Runtime {
            run: Box::new(run),
            message,
        })
    }

    fn record_runtime_error(&self, run: &Run, message: String) -> Result<(), RunLoopError> {
        self.store
            .append_step(&run.id, StepKind::Error, StepPayload::Error { message })?;
        Ok(())
    }

    fn build_completion_request(&self, run: &Run) -> Result<LlmCompletionRequest, RunLoopError> {
        let context = self
            .store
            .load_session_context_items(&run.id.session_id())?;
        let mut messages = context
            .iter()
            .filter_map(session_context_item_to_chat_message)
            .collect::<Vec<_>>();
        let tools = context
            .iter()
            .filter_map(session_context_item_to_tool_definition)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|message| RunLoopError::Runtime {
                run: Box::new(run.clone()),
                message,
            })?;

        let steps = self.store.load_session_steps(&run.id.session_id())?;
        messages.extend(
            steps
                .iter()
                .filter(|step| step.visible_to_llm())
                .filter_map(step_to_chat_message),
        );

        let Some((prompt, history)) = messages.split_last() else {
            return Err(RunLoopError::Runtime {
                run: Box::new(run.clone()),
                message: "run has no visible message for the LLM".to_owned(),
            });
        };
        let history = history.to_vec();

        Ok(LlmCompletionRequest {
            prompt: prompt.clone(),
            history,
            tools,
        })
    }
}

#[derive(Clone, Debug)]
struct LlmCompletionRequest {
    prompt: Message,
    history: Vec<Message>,
    tools: Vec<ToolDefinition>,
}

fn step_to_chat_message(step: &Step) -> Option<Message> {
    match &step.payload {
        StepPayload::UserMessage { content } => Some(Message::user(content.clone())),
        StepPayload::AssistantResponse {
            message_id, items, ..
        } => assistant_response_to_chat_message(message_id, items),
        StepPayload::ToolResult {
            id,
            call_id,
            content,
        } => Some(Message::User {
            content: rig::OneOrMany::one(rig::message::UserContent::ToolResult(ToolResult {
                id: id.clone(),
                call_id: call_id.clone(),
                content: rig::OneOrMany::one(ToolResultContent::text(content.to_string())),
            })),
        }),
        StepPayload::Error { .. } => None,
    }
}

fn assistant_response_to_chat_message(
    message_id: &Option<String>,
    items: &[AssistantResponseItem],
) -> Option<Message> {
    let content = items
        .iter()
        .filter_map(assistant_response_item_to_chat_content)
        .collect::<Vec<_>>();
    let content = OneOrMany::many(content).ok()?;
    Some(Message::Assistant {
        id: message_id.clone(),
        content,
    })
}

fn assistant_response_item_to_chat_content(
    item: &AssistantResponseItem,
) -> Option<AssistantContent> {
    match item {
        AssistantResponseItem::Text { content } => Some(AssistantContent::text(content.clone())),
        AssistantResponseItem::ToolCall {
            id,
            call_id,
            name,
            arguments,
        } => {
            let tool_call = ToolCall::new(
                id.clone(),
                ToolFunction::new(name.clone(), arguments.clone()),
            );
            let tool_call = match call_id {
                Some(call_id) => tool_call.with_call_id(call_id.clone()),
                None => tool_call,
            };
            Some(AssistantContent::ToolCall(tool_call))
        }
        AssistantResponseItem::Reasoning { .. } => None,
    }
}

fn session_context_item_to_chat_message(
    item: &crate::model::SessionContextItem,
) -> Option<Message> {
    match &item.payload {
        SessionContextPayload::SystemPrompt { content } => Some(Message::System {
            content: content.clone(),
        }),
        SessionContextPayload::ToolDefinition { .. } => None,
    }
}

fn session_context_item_to_tool_definition(
    item: &crate::model::SessionContextItem,
) -> Option<Result<ToolDefinition, String>> {
    match &item.payload {
        SessionContextPayload::ToolDefinition {
            name,
            description,
            parameters,
        } => Some(Ok(ToolDefinition {
            name: name.clone(),
            description: description.clone(),
            parameters: parameters.clone(),
        })),
        SessionContextPayload::SystemPrompt { .. } => None,
    }
}

fn default_session_context_items() -> Vec<(SessionContextKind, SessionContextPayload)> {
    let mut items = Vec::new();
    items.push((
        SessionContextKind::SystemPrompt,
        SessionContextPayload::SystemPrompt {
            content: SYSTEM_PROMPT.to_owned(),
        },
    ));

    for tool in jux_tool_definitions() {
        items.push((
            SessionContextKind::ToolDefinition,
            SessionContextPayload::ToolDefinition {
                name: tool.name,
                description: tool.description,
                parameters: tool.parameters,
            },
        ));
    }

    items
}

fn jux_tool_definitions() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: EXEC_TOOL_NAME.to_owned(),
            description: EXEC_TOOL_DESCRIPTION.to_owned(),
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
        },
        ToolDefinition {
            name: LUA_TOOL_NAME.to_owned(),
            description: LUA_TOOL_DESCRIPTION.to_owned(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "code": { "type": "string" }
                },
                "required": ["code"]
            }),
        },
    ]
}

fn execute_tool(tool_name: &str, args: &serde_json::Value) -> Result<serde_json::Value, String> {
    match tool_name {
        EXEC_TOOL_NAME => {
            let args = serde_json::from_value::<ExecToolArgs>(args.clone())
                .map_err(|error| format!("invalid exec tool arguments: {error}"))?;
            execute_exec(args)
        }
        LUA_TOOL_NAME => {
            let args = serde_json::from_value::<LuaToolArgs>(args.clone())
                .map_err(|error| format!("invalid lua tool arguments: {error}"))?;
            execute_lua(&args.code)
        }
        _ => Err(format!("unsupported tool call: {tool_name}")),
    }
}

#[derive(Debug, Deserialize)]
struct ExecToolArgs {
    program: String,
    args: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct LuaToolArgs {
    code: String,
}

fn execute_exec(args: ExecToolArgs) -> Result<serde_json::Value, String> {
    let output = run_exec_command(&args.program, &args.args)?;
    serde_json::to_value(ExecToolOutput::from(output))
        .map_err(|error| format!("exec output serialization failed: {error}"))
}

fn execute_lua(script: &str) -> Result<serde_json::Value, String> {
    let lua = Lua::new_with(StdLib::NONE, LuaOptions::default())
        .map_err(|error| format!("lua initialization failed: {error}"))?;
    install_lua_command_api(&lua).map_err(|error| format!("lua api setup failed: {error}"))?;
    let value = lua
        .load(script)
        .eval::<Value>()
        .map_err(|error| format!("lua execution failed: {error}"))?;
    lua_value_to_json(value).map_err(|error| format!("lua result conversion failed: {error}"))
}

fn lua_value_to_json(value: Value) -> mlua::Result<serde_json::Value> {
    match value {
        Value::Nil => Ok(json!({ "value": null })),
        Value::Boolean(value) => Ok(json!({ "value": value })),
        Value::Integer(value) => Ok(json!({ "value": value })),
        Value::Number(value) => Ok(json!({ "value": value })),
        Value::String(value) => Ok(json!({ "value": value.to_string_lossy() })),
        other => Ok(json!({ "value": format!("{other:?}") })),
    }
}

fn install_lua_command_api(lua: &Lua) -> mlua::Result<()> {
    let globals = lua.globals();
    globals.set(
        "print",
        lua.create_function(|_, _: mlua::Variadic<Value>| {
            Err::<(), _>(mlua::Error::external(
                "print is disabled in the Jux Lua runtime; use return to send a tool result",
            ))
        })?,
    )?;

    let os = lua.create_table()?;
    os.set(
        "execute",
        lua.create_function(|lua, command: String| {
            let output = run_single_command(&command).map_err(mlua::Error::external)?;
            let status = output.status_code.unwrap_or(1);
            let status_text = lua.create_string("exit")?;
            if output.success {
                Ok((Value::Boolean(true), Value::String(status_text), status))
            } else {
                Ok((Value::Nil, Value::String(status_text), status))
            }
        })?,
    )?;
    globals.set("os", os)?;

    let io = lua.create_table()?;
    io.set(
        "popen",
        lua.create_function(|lua, (command, mode): (String, Option<String>)| {
            let mode = mode.unwrap_or_else(|| "r".to_owned());
            if mode != "r" {
                return Err(mlua::Error::external(
                    "io.popen only supports read mode: io.popen(command, 'r')",
                ));
            }

            let output = run_single_command(&command).map_err(mlua::Error::external)?;
            if !output.success {
                return Err(mlua::Error::external(format!(
                    "command exited with status {}",
                    output.status_code.unwrap_or(1)
                )));
            }

            lua.create_userdata(JuxProcessHandle::new(output.stdout))
        })?,
    )?;
    globals.set("io", io)?;

    Ok(())
}

#[derive(Debug)]
struct CommandOutput {
    success: bool,
    status_code: Option<i32>,
    stdout: String,
    stderr: String,
}

fn run_single_command(command: &str) -> Result<CommandOutput, String> {
    reject_shell_command(command)?;
    let parts = shlex::split(command).ok_or_else(|| "invalid command syntax".to_owned())?;
    let (program, args) = parts
        .split_first()
        .ok_or_else(|| "command cannot be empty".to_owned())?;
    run_exec_command(program, args)
}

fn run_exec_command(program: &str, args: &[String]) -> Result<CommandOutput, String> {
    reject_shell_token(program)?;
    for arg in args {
        reject_shell_token(arg)?;
    }

    let output = WasmerRuntime::new()
        .run_coreutils_command(WasmCommandRequest {
            program: program.to_owned(),
            args: args.to_vec(),
            host_directory: std::env::current_dir()
                .map_err(|error| format!("current directory cannot be loaded: {error}"))?,
        })
        .map_err(|error| format!("wasi coreutils execution failed: {error}"))?;

    Ok(CommandOutput {
        success: output.success,
        status_code: output.exit_code,
        stdout: output.stdout,
        stderr: output.stderr,
    })
}

fn reject_shell_command(command: &str) -> Result<(), String> {
    reject_shell_token(command)
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

#[derive(Debug, Serialize)]
struct ExecToolOutput {
    success: bool,
    exit_code: Option<i32>,
    stdout: String,
    stderr: String,
}

impl From<CommandOutput> for ExecToolOutput {
    fn from(output: CommandOutput) -> Self {
        Self {
            success: output.success,
            exit_code: output.status_code,
            stdout: output.stdout,
            stderr: output.stderr,
        }
    }
}

#[derive(Clone, Debug)]
struct JuxProcessHandle {
    output: String,
    read_offset: usize,
    closed: bool,
}

impl JuxProcessHandle {
    fn new(output: String) -> Self {
        Self {
            output,
            read_offset: 0,
            closed: false,
        }
    }

    fn ensure_open(&self) -> mlua::Result<()> {
        if self.closed {
            Err(mlua::Error::external("process handle is closed"))
        } else {
            Ok(())
        }
    }

    fn read_all(&mut self) -> String {
        let output = self.output[self.read_offset..].to_owned();
        self.read_offset = self.output.len();
        output
    }

    fn read_line(&mut self) -> Option<String> {
        if self.read_offset >= self.output.len() {
            return None;
        }

        let remaining = &self.output[self.read_offset..];
        match remaining.find('\n') {
            Some(newline_index) => {
                let end = self.read_offset + newline_index;
                let line = self.output[self.read_offset..end].to_owned();
                self.read_offset = end + 1;
                Some(line)
            }
            None => {
                let line = remaining.to_owned();
                self.read_offset = self.output.len();
                Some(line)
            }
        }
    }
}

impl UserData for JuxProcessHandle {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method_mut("read", |lua, this, mode: Option<String>| {
            this.ensure_open()?;
            match mode.as_deref().unwrap_or("*l") {
                "*a" | "a" => Ok(Value::String(lua.create_string(this.read_all())?)),
                "*l" | "l" => match this.read_line() {
                    Some(line) => Ok(Value::String(lua.create_string(line)?)),
                    None => Ok(Value::Nil),
                },
                other => Err(mlua::Error::external(format!(
                    "unsupported io.popen read mode: {other}"
                ))),
            }
        });

        methods.add_method_mut("lines", |lua, this, ()| {
            this.ensure_open()?;
            let lines = this
                .output
                .lines()
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>();
            let mut index = 0usize;
            lua.create_function_mut(move |lua, ()| {
                let Some(line) = lines.get(index) else {
                    return Ok(Value::Nil);
                };
                index += 1;
                Ok(Value::String(lua.create_string(line)?))
            })
        });

        methods.add_method_mut("close", |_, this, ()| {
            this.closed = true;
            Ok(true)
        });
    }
}

#[derive(Clone, Debug)]
pub struct RunLoopOutput {
    pub workspace: Workspace,
    pub session: Session,
    pub run: Run,
    pub steps: Vec<Step>,
    pub answer: Option<String>,
}

#[derive(Debug)]
pub enum RunLoopError {
    Store(StoreError),
    Prompt {
        run: Box<Run>,
        source: Box<CompletionError>,
    },
    Runtime {
        run: Box<Run>,
        message: String,
    },
}

impl Display for RunLoopError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Store(error) => write!(formatter, "run loop store error: {error}"),
            Self::Prompt { source, .. } => write!(formatter, "run loop prompt error: {source}"),
            Self::Runtime { message, .. } => write!(formatter, "run loop runtime error: {message}"),
        }
    }
}

impl Error for RunLoopError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Store(error) => Some(error),
            Self::Prompt { source, .. } => Some(source),
            Self::Runtime { .. } => None,
        }
    }
}

impl From<StoreError> for RunLoopError {
    fn from(error: StoreError) -> Self {
        Self::Store(error)
    }
}
