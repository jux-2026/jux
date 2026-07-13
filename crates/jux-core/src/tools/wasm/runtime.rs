//! Wasmer execution adapter.
//!
//! This module owns the mechanics of compiling, instantiating, and running WASM
//! or WASIX packages through Wasmer. It consumes capability objects prepared by
//! the WASM capability layer and keeps command execution details out of the
//! higher-level agent orchestration code.

use super::assets::WasmAsset;
use super::capability::{
    GUEST_WORKSPACE_DIRECTORY, WasmerRuntimeCapabilities, apply_runner_capabilities,
    apply_runtime_capabilities,
};
use super::commands::{
    COREUTILS_ASSET, WasmCommandOutput, WasmCommandRequest, is_supported_coreutils_command,
};
use crate::WasmSandboxPolicy;
use std::collections::HashMap;
use std::error::Error;
use std::fmt::{self, Display};
use std::fs;
use std::sync::{Arc, Mutex};
use wasmer::sys::{BaseTunables, Cranelift, EngineBuilder, NativeEngineExt};
use wasmer::{Instance, Module, Store, imports};
use wasmer_package::utils::from_bytes;
use wasmer_types::Features;
use wasmer_wasix::PluggableRuntime;
use wasmer_wasix::bin_factory::BinaryPackage;
use wasmer_wasix::runners::wasi::{RuntimeOrEngine, WasiRunner};
use wasmer_wasix::runtime::task_manager::tokio::TokioTaskManager;
use wasmer_wasix::virtual_fs::{ArcFile, AsyncReadExt, AsyncSeekExt, BufferFile, NullFile};

#[derive(Clone)]
/// Wasmer-backed WASM runtime used by Jux tools.
///
/// The runtime is intentionally configured through `WasmerRuntimeCapabilities`
/// instead of accepting raw Wasmer builder options from callers.
pub struct WasmerRuntime {
    capabilities: WasmerRuntimeCapabilities,
    package_sessions: Arc<Mutex<HashMap<WasiPackageSessionKey, Arc<WasiPackageSession>>>>,
}

impl WasmerRuntime {
    #[must_use]
    pub fn new() -> Self {
        Self::with_capabilities(WasmerRuntimeCapabilities::default())
    }

    #[must_use]
    pub fn with_capabilities(capabilities: WasmerRuntimeCapabilities) -> Self {
        Self {
            capabilities,
            package_sessions: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    #[must_use]
    pub fn with_wasm_policy(policy: &WasmSandboxPolicy) -> Self {
        Self::with_capabilities(WasmerRuntimeCapabilities::from(policy))
    }

    #[must_use]
    pub fn capabilities(&self) -> &WasmerRuntimeCapabilities {
        &self.capabilities
    }

    pub fn call_exported_i32_function(
        &self,
        wasm: &[u8],
        function_name: &str,
    ) -> Result<i32, WasmRuntimeError> {
        let mut store = Store::default();
        let module = Module::new(&store, wasm)
            .map_err(|source| WasmRuntimeError::Compile(source.to_string()))?;
        let instance = Instance::new(&mut store, &module, &imports! {})
            .map_err(|source| WasmRuntimeError::Instantiate(source.to_string()))?;
        let function = instance
            .exports
            .get_typed_function::<(), i32>(&store, function_name)
            .map_err(|source| WasmRuntimeError::Export(source.to_string()))?;

        function
            .call(&mut store)
            .map_err(|source| WasmRuntimeError::Call(source.to_string()))
    }

    pub fn run_coreutils_command(
        &self,
        request: WasmCommandRequest,
    ) -> Result<WasmCommandOutput, WasmRuntimeError> {
        if !is_supported_coreutils_command(&request.program) {
            return Err(WasmRuntimeError::UnsupportedCommand(request.program));
        }

        self.run_wasi_package_command(&COREUTILS_ASSET, request)
    }

    fn run_wasi_package_command(
        &self,
        asset: &WasmAsset,
        request: WasmCommandRequest,
    ) -> Result<WasmCommandOutput, WasmRuntimeError> {
        let session = self.wasi_package_session(asset)?;
        run_webc_command(session, request, self.capabilities.clone())
    }

    fn wasi_package_session(
        &self,
        asset: &WasmAsset,
    ) -> Result<Arc<WasiPackageSession>, WasmRuntimeError> {
        let key = WasiPackageSessionKey::from(asset);
        let mut package_sessions = self
            .package_sessions
            .lock()
            .map_err(|_| WasmRuntimeError::Run("wasm package session lock poisoned".to_owned()))?;

        if let Some(session) = package_sessions.get(&key) {
            return Ok(Arc::clone(session));
        }

        let session = Arc::new(load_wasi_package_session(asset, &self.capabilities)?);
        package_sessions.insert(key, Arc::clone(&session));
        Ok(session)
    }
}

impl fmt::Debug for WasmerRuntime {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WasmerRuntime")
            .field("capabilities", &self.capabilities)
            .finish_non_exhaustive()
    }
}

impl Default for WasmerRuntime {
    fn default() -> Self {
        Self::new()
    }
}

fn load_wasi_package_session(
    asset: &WasmAsset,
    capabilities: &WasmerRuntimeCapabilities,
) -> Result<WasiPackageSession, WasmRuntimeError> {
    let asset = *asset;
    let capabilities = capabilities.clone();

    std::thread::spawn(move || load_wasi_package_session_in_thread(&asset, &capabilities))
        .join()
        .map_err(|_| WasmRuntimeError::Run("wasi package session thread panicked".to_owned()))?
}

fn load_wasi_package_session_in_thread(
    asset: &WasmAsset,
    capabilities: &WasmerRuntimeCapabilities,
) -> Result<WasiPackageSession, WasmRuntimeError> {
    let package_path = asset
        .ensure_local_file()
        .map_err(|source| WasmRuntimeError::Run(source.to_string()))?;
    let package_bytes =
        fs::read(&package_path).map_err(|source| WasmRuntimeError::Io(source.to_string()))?;
    let tokio_runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|error| WasmRuntimeError::Run(error.to_string()))?;
    let runtime = Arc::new(build_wasix_runtime(&tokio_runtime, capabilities)?);
    let package = load_webc_package(&tokio_runtime, package_bytes, runtime.as_ref())?;

    Ok(WasiPackageSession {
        tokio_runtime,
        runtime,
        package,
    })
}

fn run_webc_command(
    session: Arc<WasiPackageSession>,
    request: WasmCommandRequest,
    capabilities: WasmerRuntimeCapabilities,
) -> Result<WasmCommandOutput, WasmRuntimeError> {
    let stdout = ArcFile::new(Box::<BufferFile>::default());
    let stderr = ArcFile::new(Box::<BufferFile>::default());
    let invocation = WasiCommandInvocation {
        program: request.program,
        args: request.args,
        host_directory: request.host_directory,
        stdout: stdout.clone(),
        stderr: stderr.clone(),
    };
    let thread_output =
        std::thread::spawn(move || run_webc_command_in_thread(session, invocation, capabilities))
            .join()
            .map_err(|_| WasmRuntimeError::Run("wasi command thread panicked".to_owned()))?;
    let command_result = thread_output?;

    Ok(WasmCommandOutput {
        success: command_result.exit_code == 0,
        exit_code: Some(command_result.exit_code),
        stdout: command_result.stdout,
        stderr: command_result.stderr,
    })
}

fn run_webc_command_in_thread(
    session: Arc<WasiPackageSession>,
    invocation: WasiCommandInvocation,
    capabilities: WasmerRuntimeCapabilities,
) -> Result<WasiCommandResult, WasmRuntimeError> {
    let mut stdout = invocation.stdout.clone();
    let mut stderr = invocation.stderr.clone();
    let result = run_wasi_command(&session, invocation, &capabilities);
    let stdout =
        read_virtual_file(&session.tokio_runtime, &mut stdout).map_err(WasmRuntimeError::Io)?;
    let stderr =
        read_virtual_file(&session.tokio_runtime, &mut stderr).map_err(WasmRuntimeError::Io)?;
    let exit_code = match result {
        Ok(()) => 0,
        Err(error) => wasm_exit_code(&error).ok_or_else(|| {
            WasmRuntimeError::Run(
                error
                    .chain()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(": "),
            )
        })?,
    };

    Ok(WasiCommandResult {
        exit_code,
        stdout,
        stderr,
    })
}

fn build_wasix_runtime(
    tokio_runtime: &tokio::runtime::Runtime,
    capabilities: &WasmerRuntimeCapabilities,
) -> Result<PluggableRuntime, WasmRuntimeError> {
    let _guard = tokio_runtime.enter();
    let tasks = Arc::new(TokioTaskManager::new(tokio_runtime.handle().clone()));
    let mut runtime = PluggableRuntime::new(Arc::clone(&tasks) as Arc<_>);
    runtime.set_engine(wasmer_engine_with_exceptions());
    apply_runtime_capabilities(&mut runtime, capabilities)?;
    Ok(runtime)
}

fn load_webc_package(
    tokio_runtime: &tokio::runtime::Runtime,
    package_bytes: Vec<u8>,
    runtime: &PluggableRuntime,
) -> Result<BinaryPackage, WasmRuntimeError> {
    let container =
        from_bytes(package_bytes).map_err(|error| WasmRuntimeError::Run(error.to_string()))?;
    tokio_runtime
        .block_on(BinaryPackage::from_webc(&container, runtime))
        .map_err(|error| WasmRuntimeError::Run(error.to_string()))
}

pub(super) struct WasiCommandInvocation {
    program: String,
    args: Vec<String>,
    host_directory: std::path::PathBuf,
    stdout: ArcFile<BufferFile>,
    stderr: ArcFile<BufferFile>,
}

struct WasiPackageSession {
    tokio_runtime: tokio::runtime::Runtime,
    runtime: Arc<PluggableRuntime>,
    package: BinaryPackage,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct WasiPackageSessionKey {
    package: &'static str,
    version: &'static str,
    filename: &'static str,
}

impl From<&WasmAsset> for WasiPackageSessionKey {
    fn from(asset: &WasmAsset) -> Self {
        Self {
            package: asset.package,
            version: asset.version,
            filename: asset.filename,
        }
    }
}

struct WasiCommandResult {
    exit_code: i32,
    stdout: String,
    stderr: String,
}

fn run_wasi_command(
    session: &WasiPackageSession,
    invocation: WasiCommandInvocation,
    capabilities: &WasmerRuntimeCapabilities,
) -> Result<(), anyhow::Error> {
    let _guard = session.tokio_runtime.enter();
    let mut runner = WasiRunner::new();
    runner
        .with_args(invocation.args)
        .with_stdin(Box::<NullFile>::default())
        .with_stdout(Box::new(invocation.stdout))
        .with_stderr(Box::new(invocation.stderr))
        .with_current_dir(GUEST_WORKSPACE_DIRECTORY);
    apply_runner_capabilities(&mut runner, capabilities, invocation.host_directory);

    runner.run_command(
        &invocation.program,
        &session.package,
        RuntimeOrEngine::Runtime(session.runtime.clone()),
    )
}

fn wasmer_engine_with_exceptions() -> wasmer::Engine {
    let mut features = Features::new();
    features.exceptions(true);
    let mut engine: wasmer::Engine = EngineBuilder::new(Cranelift::default())
        .set_features(Some(features))
        .engine()
        .into();
    let tunables = BaseTunables::for_target(engine.target());
    engine.set_tunables(tunables);
    engine
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WasmRuntimeError {
    Compile(String),
    Instantiate(String),
    Export(String),
    Call(String),
    Io(String),
    Run(String),
    UnsupportedCommand(String),
}

impl Display for WasmRuntimeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Compile(message) => write!(formatter, "wasm compile error: {message}"),
            Self::Instantiate(message) => write!(formatter, "wasm instantiate error: {message}"),
            Self::Export(message) => write!(formatter, "wasm export error: {message}"),
            Self::Call(message) => write!(formatter, "wasm call error: {message}"),
            Self::Io(message) => write!(formatter, "wasm io error: {message}"),
            Self::Run(message) => write!(formatter, "wasm run error: {message}"),
            Self::UnsupportedCommand(command) => {
                write!(formatter, "unsupported coreutils command: {command}")
            }
        }
    }
}

impl Error for WasmRuntimeError {}

fn wasm_exit_code(error: &anyhow::Error) -> Option<i32> {
    error
        .chain()
        .find_map(|source| source.downcast_ref::<wasmer_wasix::WasiError>())
        .map(|error| match error {
            wasmer_wasix::WasiError::Exit(code) => code.raw(),
            _ => 1,
        })
}

fn read_virtual_file(
    runtime: &tokio::runtime::Runtime,
    file: &mut ArcFile<BufferFile>,
) -> Result<String, String> {
    runtime.block_on(async {
        file.rewind().await.map_err(|error| error.to_string())?;
        let mut output = Vec::new();
        file.read_to_end(&mut output)
            .await
            .map_err(|error| error.to_string())?;
        Ok(String::from_utf8_lossy(&output).to_string())
    })
}
