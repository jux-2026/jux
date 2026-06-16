//! WASM execution support for the Jux runtime.
//!
//! The module separates user-facing sandbox intent from the concrete Wasmer
//! runtime wiring:
//!
//! - `permissions` describes the permissions users and future configuration
//!   layers can request.
//! - `capability` translates those permissions into Wasmer/WASIX capabilities.
//! - `runtime` owns the Wasmer execution flow.
//! - `commands` defines the command-shaped WASM tools Jux can run.
//! - `assets` manages local WEBC/WASM package files.

mod assets;
mod capability;
mod commands;
mod exec_tool;
mod permissions;
mod runtime;

#[cfg(test)]
mod tests;

pub use self::capability::{
    WasmEnvironmentCapability, WasmFilesystemCapability, WasmNetworkCapability,
    WasmPackageLoadingCapability, WasmStdioCapability, WasmerRuntimeCapabilities,
};
pub use self::commands::{
    WasmCommandDefinition, WasmCommandOutput, WasmCommandRequest, available_wasm_command_names,
    available_wasm_commands,
};
pub(crate) use self::exec_tool::{exec_tool, run_exec_command_line};
pub use self::permissions::{
    WasmEnvironmentPermission, WasmFilesystemPermission, WasmNetworkPermission, WasmPermissions,
};
pub use self::runtime::{WasmRuntimeError, WasmerRuntime};
