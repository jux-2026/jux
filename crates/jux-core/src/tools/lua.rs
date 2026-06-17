use crate::tools::JuxTool;
use crate::tools::wasm::run_exec_command_line;
use mlua::{Lua, LuaOptions, StdLib, UserData, UserDataMethods, Value};
use rig::completion::ToolDefinition;
use serde::Deserialize;
use serde_json::json;

const LUA_TOOL_NAME: &str = "lua";
const LUA_TOOL_DESCRIPTION: &str = "Execute Lua code in a restricted Jux Lua runtime. \
All Lua standard libraries are disabled by default. Only these globals are available: \
os.execute(command), which executes one non-shell command; and io.popen(command, 'r'), \
which executes one non-shell command and returns a readable handle. Commands are parsed \
into one program plus arguments and are not executed through a shell. Shell syntax such \
as &&, ||, ;, |, >, <, backticks, $(), wildcard expansion, and newlines is rejected. \
io.popen handles support read('*a'), read('*l'), lines(), and close(). Return the first \
Lua value as the tool result. Do not call print. Use return to send the result back to Jux.";

#[must_use]
pub(crate) fn lua_tool() -> LuaTool {
    LuaTool
}

pub(crate) struct LuaTool;

impl JuxTool for LuaTool {
    fn name(&self) -> &'static str {
        LUA_TOOL_NAME
    }

    fn definition(&self) -> ToolDefinition {
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
        }
    }

    fn execute(&self, args: &serde_json::Value) -> Result<serde_json::Value, String> {
        let args = serde_json::from_value::<LuaToolArgs>(args.clone())
            .map_err(|error| format!("invalid lua tool arguments: {error}"))?;
        execute_lua(&args.code)
    }
}

#[derive(Debug, Deserialize)]
struct LuaToolArgs {
    code: String,
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
}

fn run_single_command(command: &str) -> Result<CommandOutput, String> {
    let output = run_exec_command_line(command)?;
    Ok(CommandOutput {
        success: output.success,
        status_code: output.status_code,
        stdout: output.stdout,
    })
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
