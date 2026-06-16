use std::path::{Path, PathBuf};
use std::sync::Arc;
use wasmer::sys::{BaseTunables, Cranelift, EngineBuilder, NativeEngineExt};
use wasmer_package::utils::from_bytes;
use wasmer_types::Features;
use wasmer_wasix::PluggableRuntime;
use wasmer_wasix::bin_factory::BinaryPackage;
use wasmer_wasix::runners::MappedDirectory;
use wasmer_wasix::runners::wasi::{RuntimeOrEngine, WasiRunner};
use wasmer_wasix::runtime::package_loader::BuiltinPackageLoader;
use wasmer_wasix::runtime::task_manager::tokio::TokioTaskManager;
use wasmer_wasix::virtual_fs::{ArcFile, AsyncReadExt, AsyncSeekExt, BufferFile, NullFile};

const COREUTILS_WEBC: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/assets/coreutils/coreutils-1.0.25.webc"
);

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(command) = args.next() else {
        eprintln!("usage: cargo run -p jux-core --example coreutils -- <command> [args...]");
        std::process::exit(2);
    };
    let command_args = args.collect::<Vec<_>>();

    match run_coreutils_command(&command, command_args) {
        Ok(output) => {
            print!("{}", output.stdout);
            eprint!("{}", output.stderr);
            std::process::exit(output.exit_code);
        }
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        }
    }
}

fn run_coreutils_command(command: &str, args: Vec<String>) -> Result<CommandOutput, String> {
    let tokio_runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|error| error.to_string())?;
    let runtime = Arc::new(raw_wasmer_runtime(&tokio_runtime)?);
    let package = load_coreutils_package(&tokio_runtime, runtime.as_ref())?;
    let workspace = std::env::current_dir().map_err(|error| error.to_string())?;

    run_raw_wasmer_package_command(&tokio_runtime, runtime, package, command, args, workspace)
}

fn run_raw_wasmer_package_command(
    tokio_runtime: &tokio::runtime::Runtime,
    runtime: Arc<PluggableRuntime>,
    package: BinaryPackage,
    command: &str,
    args: Vec<String>,
    workspace: PathBuf,
) -> Result<CommandOutput, String> {
    let _guard = tokio_runtime.enter();
    let mut stdout = ArcFile::new(Box::<BufferFile>::default());
    let mut stderr = ArcFile::new(Box::<BufferFile>::default());
    let result = WasiRunner::new()
        .with_args(args)
        .with_stdin(Box::<NullFile>::default())
        .with_stdout(Box::new(stdout.clone()))
        .with_stderr(Box::new(stderr.clone()))
        .with_current_dir("/workspace")
        .with_mapped_directories([MappedDirectory {
            host: workspace,
            guest: "/workspace".to_owned(),
        }])
        .run_command(command, &package, RuntimeOrEngine::Runtime(runtime));
    let stdout = read_virtual_file(tokio_runtime, &mut stdout)?;
    let stderr = read_virtual_file(tokio_runtime, &mut stderr)?;
    let exit_code = match result {
        Ok(()) => 0,
        Err(error) => raw_wasmer_exit_code(&error).ok_or_else(|| error.to_string())?,
    };

    Ok(CommandOutput {
        exit_code,
        stdout,
        stderr,
    })
}

struct CommandOutput {
    exit_code: i32,
    stdout: String,
    stderr: String,
}

fn load_coreutils_package(
    tokio_runtime: &tokio::runtime::Runtime,
    runtime: &PluggableRuntime,
) -> Result<BinaryPackage, String> {
    let package_bytes =
        std::fs::read(Path::new(COREUTILS_WEBC)).map_err(|error| error.to_string())?;
    let container = from_bytes(package_bytes).map_err(|error| error.to_string())?;

    tokio_runtime
        .block_on(BinaryPackage::from_webc(&container, runtime))
        .map_err(|error| error.to_string())
}

fn raw_wasmer_runtime(tokio_runtime: &tokio::runtime::Runtime) -> Result<PluggableRuntime, String> {
    let _guard = tokio_runtime.enter();
    let tasks = Arc::new(TokioTaskManager::new(tokio_runtime.handle().clone()));
    let mut runtime = PluggableRuntime::new(Arc::clone(&tasks) as Arc<_>);
    runtime.set_engine(raw_wasmer_engine_with_exceptions());
    runtime.set_package_loader(BuiltinPackageLoader::new());

    Ok(runtime)
}

fn raw_wasmer_engine_with_exceptions() -> wasmer::Engine {
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

fn raw_wasmer_exit_code(error: &anyhow::Error) -> Option<i32> {
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
