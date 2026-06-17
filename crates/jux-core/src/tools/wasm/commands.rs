//! Command-shaped WASM tool definitions.
//!
//! This module keeps the concrete command catalog and command input/output
//! types separate from the Wasmer runtime adapter. The current implementation
//! runs commands from the packaged `wasmer/coreutils` WEBC asset.

use super::assets::WasmAsset;
use std::path::PathBuf;

const COREUTILS_TOOL_ID: &str = "coreutils";

pub(super) const COREUTILS_ASSET: WasmAsset = WasmAsset {
    package: "wasmer/coreutils",
    version: "1.0.25",
    filename: "coreutils-1.0.25.webc",
    download_url: "https://cdn.wasmer.io/webcimages/36ea48f185ca15fe8454b1defb6a11754659dbed6330549662b62874d509f95f.webc",
    relative_dir: "coreutils",
};

const WASM_COMMANDS: &[WasmCommandDefinition] = &[
    coreutils_command("arch"),
    coreutils_command("base32"),
    coreutils_command("base64"),
    coreutils_command("basename"),
    coreutils_command("cat"),
    coreutils_command("cksum"),
    coreutils_command("comm"),
    coreutils_command("cp"),
    coreutils_command("csplit"),
    coreutils_command("cut"),
    coreutils_command("date"),
    coreutils_command("dircolors"),
    coreutils_command("dirname"),
    coreutils_command("echo"),
    coreutils_command("env"),
    coreutils_command("expand"),
    coreutils_command("factor"),
    coreutils_command("false"),
    coreutils_command("fmt"),
    coreutils_command("fold"),
    coreutils_command("hashsum"),
    coreutils_command("head"),
    coreutils_command("join"),
    coreutils_command("link"),
    coreutils_command("ln"),
    coreutils_command("ls"),
    coreutils_command("md5sum"),
    coreutils_command("mkdir"),
    coreutils_command("mktemp"),
    coreutils_command("mv"),
    coreutils_command("nl"),
    coreutils_command("nproc"),
    coreutils_command("numfmt"),
    coreutils_command("od"),
    coreutils_command("paste"),
    coreutils_command("printenv"),
    coreutils_command("printf"),
    coreutils_command("ptx"),
    coreutils_command("pwd"),
    coreutils_command("readlink"),
    coreutils_command("realpath"),
    coreutils_command("relpath"),
    coreutils_command("rm"),
    coreutils_command("rmdir"),
    coreutils_command("seq"),
    coreutils_command("sha1sum"),
    coreutils_command("sha224sum"),
    coreutils_command("sha256sum"),
    coreutils_command("sha3-224sum"),
    coreutils_command("sha3-256sum"),
    coreutils_command("sha3-384sum"),
    coreutils_command("sha3-512sum"),
    coreutils_command("sha384sum"),
    coreutils_command("sha3sum"),
    coreutils_command("sha512sum"),
    coreutils_command("shake128sum"),
    coreutils_command("shake256sum"),
    coreutils_command("shred"),
    coreutils_command("shuf"),
    coreutils_command("sleep"),
    coreutils_command("sum"),
    coreutils_command("tee"),
    coreutils_command("touch"),
    coreutils_command("tr"),
    coreutils_command("true"),
    coreutils_command("truncate"),
    coreutils_command("tsort"),
    coreutils_command("unexpand"),
    coreutils_command("uniq"),
    coreutils_command("unlink"),
    coreutils_command("wc"),
    coreutils_command("yes"),
];

const fn coreutils_command(program: &'static str) -> WasmCommandDefinition {
    WasmCommandDefinition {
        tool_id: COREUTILS_TOOL_ID,
        program,
    }
}

/// Command entry that can be exposed to the LLM.
///
/// The command catalog intentionally contains command names, not usage
/// instructions. The system prompt can list these names while relying on common
/// command knowledge and tool-call errors for command-specific behavior.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WasmCommandDefinition {
    pub tool_id: &'static str,
    pub program: &'static str,
}

#[must_use]
pub fn available_wasm_commands() -> &'static [WasmCommandDefinition] {
    WASM_COMMANDS
}

#[must_use]
pub fn available_wasm_command_names() -> Vec<&'static str> {
    WASM_COMMANDS
        .iter()
        .map(|command| command.program)
        .collect()
}

#[must_use]
pub(super) fn is_supported_coreutils_command(program: &str) -> bool {
    WASM_COMMANDS
        .iter()
        .any(|command| command.tool_id == COREUTILS_TOOL_ID && command.program == program)
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// Request to run one WASM-backed command.
///
/// This type describes the command invocation shape. Permission decisions are
/// represented separately by the WASM permissions and capability layers.
pub struct WasmCommandRequest {
    pub program: String,
    pub args: Vec<String>,
    pub host_directory: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// Structured output returned from a WASM-backed command.
pub struct WasmCommandOutput {
    pub success: bool,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}
