use crate::wasm_assets::WasmAsset;
use std::error::Error;
use std::fmt::{self, Display};
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use wasmer::sys::{BaseTunables, Cranelift, EngineBuilder, NativeEngineExt};
use wasmer::{Instance, Module, Store, imports};
use wasmer_package::utils::from_bytes;
use wasmer_types::Features;
use wasmer_wasix::PluggableRuntime;
use wasmer_wasix::bin_factory::BinaryPackage;
use wasmer_wasix::runners::MappedDirectory;
use wasmer_wasix::runners::wasi::{RuntimeOrEngine, WasiRunner};
use wasmer_wasix::runtime::package_loader::BuiltinPackageLoader;
use wasmer_wasix::runtime::task_manager::tokio::TokioTaskManager;
use wasmer_wasix::virtual_fs::{ArcFile, AsyncReadExt, AsyncSeekExt, BufferFile, NullFile};

const COREUTILS_ASSET: WasmAsset = WasmAsset {
    package: "wasmer/coreutils",
    version: "1.0.25",
    filename: "coreutils-1.0.25.webc",
    download_url: "https://cdn.wasmer.io/webcimages/36ea48f185ca15fe8454b1defb6a11754659dbed6330549662b62874d509f95f.webc",
    relative_dir: "coreutils",
};
const COREUTILS_COMMANDS: &[&str] = &[
    "basename", "base32", "base64", "cat", "dirname", "echo", "env", "ls", "mkdir", "mv", "printf",
    "pwd", "sum", "wc",
];

#[derive(Clone, Debug, Default)]
pub struct WasmerRuntime;

impl WasmerRuntime {
    #[must_use]
    pub fn new() -> Self {
        Self
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
        if !COREUTILS_COMMANDS.contains(&request.program.as_str()) {
            return Err(WasmRuntimeError::UnsupportedCommand(request.program));
        }

        self.run_wasi_package_command(&COREUTILS_ASSET, request)
    }

    fn run_wasi_package_command(
        &self,
        asset: &WasmAsset,
        request: WasmCommandRequest,
    ) -> Result<WasmCommandOutput, WasmRuntimeError> {
        let package_bytes = load_wasm_asset(asset)?;
        run_webc_command(package_bytes, request)
    }
}

fn load_wasm_asset(asset: &WasmAsset) -> Result<Vec<u8>, WasmRuntimeError> {
    let package_path = asset
        .ensure_local_file()
        .map_err(|source| WasmRuntimeError::Run(source.to_string()))?;
    fs::read(&package_path).map_err(|source| WasmRuntimeError::Io(source.to_string()))
}

fn run_webc_command(
    package_bytes: Vec<u8>,
    request: WasmCommandRequest,
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
        std::thread::spawn(move || run_webc_command_in_thread(package_bytes, invocation))
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
    package_bytes: Vec<u8>,
    invocation: WasiCommandInvocation,
) -> Result<WasiCommandResult, WasmRuntimeError> {
    let tokio_runtime = build_tokio_runtime()?;
    let runtime = build_wasix_runtime(&tokio_runtime)?;
    let package = load_webc_package(&tokio_runtime, package_bytes, &runtime)?;
    let mut stdout = invocation.stdout.clone();
    let mut stderr = invocation.stderr.clone();
    let result = run_wasi_command(&tokio_runtime, runtime, package, invocation);
    let stdout = read_virtual_file(&tokio_runtime, &mut stdout).map_err(WasmRuntimeError::Io)?;
    let stderr = read_virtual_file(&tokio_runtime, &mut stderr).map_err(WasmRuntimeError::Io)?;
    let exit_code = match result {
        Ok(()) => 0,
        Err(error) => {
            wasm_exit_code(&error).ok_or_else(|| WasmRuntimeError::Run(error_chain(&error)))?
        }
    };

    Ok(WasiCommandResult {
        exit_code,
        stdout,
        stderr,
    })
}

fn build_tokio_runtime() -> Result<tokio::runtime::Runtime, WasmRuntimeError> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|error| WasmRuntimeError::Run(error.to_string()))
}

fn build_wasix_runtime(
    tokio_runtime: &tokio::runtime::Runtime,
) -> Result<PluggableRuntime, WasmRuntimeError> {
    let _guard = tokio_runtime.enter();
    let tasks = Arc::new(TokioTaskManager::new(tokio_runtime.handle().clone()));
    let mut runtime = PluggableRuntime::new(Arc::clone(&tasks) as Arc<_>);
    runtime.set_engine(wasmer_engine_with_exceptions());
    let http_client: Arc<dyn wasmer_wasix::http::HttpClient + Send + Sync> = Arc::new(
        wasmer_wasix::http::default_http_client()
            .ok_or_else(|| WasmRuntimeError::Run("wasm http client is unavailable".to_owned()))?,
    );
    runtime
        .set_package_loader(
            BuiltinPackageLoader::new().with_shared_http_client(http_client.clone()),
        )
        .set_http_client(http_client);
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

struct WasiCommandInvocation {
    program: String,
    args: Vec<String>,
    host_directory: PathBuf,
    stdout: ArcFile<BufferFile>,
    stderr: ArcFile<BufferFile>,
}

struct WasiCommandResult {
    exit_code: i32,
    stdout: String,
    stderr: String,
}

fn run_wasi_command(
    tokio_runtime: &tokio::runtime::Runtime,
    runtime: PluggableRuntime,
    package: BinaryPackage,
    invocation: WasiCommandInvocation,
) -> Result<(), anyhow::Error> {
    let _guard = tokio_runtime.enter();
    let mut runner = WasiRunner::new();
    runner
        .with_args(invocation.args)
        .with_stdin(Box::<NullFile>::default())
        .with_stdout(Box::new(invocation.stdout))
        .with_stderr(Box::new(invocation.stderr))
        .with_current_dir("/")
        .with_forward_host_env(false)
        .with_mapped_directories([MappedDirectory {
            host: invocation.host_directory,
            guest: "/".to_owned(),
        }]);

    runner.run_command(
        &invocation.program,
        &package,
        RuntimeOrEngine::Runtime(Arc::new(runtime)),
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WasmCommandRequest {
    pub program: String,
    pub args: Vec<String>,
    pub host_directory: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WasmCommandOutput {
    pub success: bool,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

fn wasm_exit_code(error: &anyhow::Error) -> Option<i32> {
    error
        .chain()
        .find_map(|source| source.downcast_ref::<wasmer_wasix::WasiError>())
        .map(|error| match error {
            wasmer_wasix::WasiError::Exit(code) => code.raw(),
            _ => 1,
        })
}

fn error_chain(error: &anyhow::Error) -> String {
    error
        .chain()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(": ")
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
