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
mod permissions;
mod runtime;

pub use self::capability::{
    WasmEnvironmentCapability, WasmFilesystemCapability, WasmNetworkCapability,
    WasmPackageLoadingCapability, WasmStdioCapability, WasmerRuntimeCapabilities,
};
pub use self::commands::{WasmCommandOutput, WasmCommandRequest};
pub use self::permissions::{
    WasmEnvironmentPermission, WasmFilesystemPermission, WasmNetworkPermission, WasmPermissions,
};
pub use self::runtime::{WasmRuntimeError, WasmerRuntime};
