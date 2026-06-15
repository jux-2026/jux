//! Command-shaped WASM tool definitions.
//!
//! This module keeps the concrete command catalog and command input/output
//! types separate from the Wasmer runtime adapter. The current implementation
//! runs commands from the packaged `wasmer/coreutils` WEBC asset.

use super::assets::WasmAsset;
use std::path::PathBuf;

pub(super) const COREUTILS_ASSET: WasmAsset = WasmAsset {
    package: "wasmer/coreutils",
    version: "1.0.25",
    filename: "coreutils-1.0.25.webc",
    download_url: "https://cdn.wasmer.io/webcimages/36ea48f185ca15fe8454b1defb6a11754659dbed6330549662b62874d509f95f.webc",
    relative_dir: "coreutils",
};

const COREUTILS_COMMANDS: &[&str] = &[
    "arch",
    "base32",
    "base64",
    "baseenc",
    "basename",
    "cat",
    "chcon",
    "chgrp",
    "chmod",
    "chown",
    "chroot",
    "cksum",
    "comm",
    "cp",
    "csplit",
    "cut",
    "date",
    "dd",
    "df",
    "dircolors",
    "dirname",
    "du",
    "echo",
    "env",
    "expand",
    "expr",
    "factor",
    "false",
    "fmt",
    "fold",
    "groups",
    "hashsum",
    "head",
    "hostid",
    "hostname",
    "id",
    "install",
    "join",
    "kill",
    "link",
    "ln",
    "logname",
    "ls",
    "mkdir",
    "mkfifo",
    "mknod",
    "mktemp",
    "more",
    "mv",
    "nice",
    "nl",
    "nohup",
    "nproc",
    "numfmt",
    "od",
    "paste",
    "pathchk",
    "pinky",
    "pr",
    "printenv",
    "printf",
    "ptx",
    "pwd",
    "readlink",
    "realpath",
    "relpath",
    "rm",
    "rmdir",
    "runcon",
    "seq",
    "shred",
    "shuf",
    "sleep",
    "sort",
    "split",
    "stat",
    "stdbuf",
    "sum",
    "sync",
    "tac",
    "tail",
    "tee",
    "test",
    "timeout",
    "touch",
    "tr",
    "true",
    "truncate",
    "tsort",
    "tty",
    "uname",
    "unexpand",
    "uniq",
    "unlink",
    "uptime",
    "users",
    "wc",
    "who",
    "whoami",
    "yes",
];

#[must_use]
pub(super) fn is_supported_coreutils_command(program: &str) -> bool {
    COREUTILS_COMMANDS.contains(&program)
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
