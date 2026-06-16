use jux_core::available_wasm_command_names;
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use std::io::{self, Cursor, Seek as StdSeek, SeekFrom};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, AsyncSeek, AsyncWrite, ReadBuf};
use wasmer::sys::{BaseTunables, Cranelift, EngineBuilder, NativeEngineExt};
use wasmer_package::utils::from_bytes;
use wasmer_types::Features;
use wasmer_wasix::PluggableRuntime;
use wasmer_wasix::bin_factory::BinaryPackage;
use wasmer_wasix::runners::MappedDirectory;
use wasmer_wasix::runners::wasi::{RuntimeOrEngine, WasiRunner};
use wasmer_wasix::runtime::package_loader::BuiltinPackageLoader;
use wasmer_wasix::runtime::task_manager::tokio::TokioTaskManager;
use wasmer_wasix::virtual_fs::{
    ArcFile, AsyncReadExt, AsyncSeekExt, AsyncWriteExt, BufferFile, VirtualFile,
};

const COREUTILS_WEBC: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/assets/coreutils/coreutils-1.0.25.webc"
);
#[test]
fn raw_wasmer_command_tests_cover_exposed_coreutils_commands() {
    let exposed_commands = sorted_strings(available_wasm_command_names());
    let tested_commands: BTreeSet<String> = command_case_map()
        .keys()
        .copied()
        .map(str::to_owned)
        .collect();

    assert_eq!(tested_commands, exposed_commands);
}

#[test]
fn raw_wasmer_runs_basic_coreutils_commands() {
    let runner = RawCoreutilsRunner::new().expect("raw wasmer coreutils runner loads");
    let mut mismatches = Vec::new();
    let mut records = Vec::new();
    let mut commands = command_cases();
    commands.sort_by_key(|command| !command.expected_success);

    for command in commands {
        let workspace = temp_workspace(command.program);
        prepare_workspace(&workspace);
        let output = runner
            .run_in_workspace(
                command.manifest_program,
                command.args.iter().copied(),
                command.stdin,
                command.stdout_byte_limit,
                &workspace,
            )
            .unwrap_or_else(|error| panic!("{} runs through raw wasmer: {error}", command.program));
        print_command_status(command, &output);
        records.push(command_status_json(command, &output));
        if let Err(error) = command_status_error(command, &output) {
            mismatches.push(error);
        }
    }

    write_command_status_json(&records).expect("command status json is written");

    if !mismatches.is_empty() {
        panic!("command status mismatches:\n{}", mismatches.join("\n\n"));
    }
}

struct RawCoreutilsRunner {
    tokio_runtime: tokio::runtime::Runtime,
    runtime: Arc<PluggableRuntime>,
    package: BinaryPackage,
}

impl RawCoreutilsRunner {
    fn new() -> Result<Self, String> {
        let tokio_runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .map_err(|error| error.to_string())?;
        let runtime = Arc::new(raw_wasmer_runtime(&tokio_runtime)?);
        let package = load_coreutils_package(&tokio_runtime, runtime.as_ref())?;

        Ok(Self {
            tokio_runtime,
            runtime,
            package,
        })
    }

    fn run_in_workspace<I, S>(
        &self,
        program: &str,
        args: I,
        stdin: &str,
        stdout_byte_limit: Option<usize>,
        workspace: &Path,
    ) -> Result<RawCommandOutput, String>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.run_with_workspace(program, args, stdin, stdout_byte_limit, Some(workspace))
    }

    fn run_with_workspace<I, S>(
        &self,
        program: &str,
        args: I,
        stdin: &str,
        stdout_byte_limit: Option<usize>,
        workspace: Option<&Path>,
    ) -> Result<RawCommandOutput, String>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let args = args.into_iter().map(Into::into).collect::<Vec<_>>();

        if let Some(max_len) = stdout_byte_limit {
            let stdout = ArcFile::new(Box::new(CappedBufferFile::new(max_len)));
            return self.run_with_stdout_file(program, args, stdin, stdout, workspace);
        }

        let stdout = ArcFile::new(Box::<BufferFile>::default());
        self.run_with_stdout_file(program, args, stdin, stdout, workspace)
    }

    fn run_with_stdout_file<T>(
        &self,
        program: &str,
        args: Vec<String>,
        stdin: &str,
        mut stdout: ArcFile<T>,
        workspace: Option<&Path>,
    ) -> Result<RawCommandOutput, String>
    where
        T: VirtualFile + Send + Sync + 'static,
    {
        let _guard = self.tokio_runtime.enter();
        let stdin = virtual_file_with_contents(&self.tokio_runtime, stdin)?;
        let mut stderr = ArcFile::new(Box::<BufferFile>::default());
        let mut runner = WasiRunner::new();
        runner
            .with_args(args)
            .with_stdin(Box::new(stdin))
            .with_stdout(Box::new(stdout.clone()))
            .with_stderr(Box::new(stderr.clone()))
            .with_current_dir("/workspace");
        if let Some(workspace) = workspace {
            runner.with_mapped_directories([MappedDirectory {
                host: workspace.to_path_buf(),
                guest: "/workspace".to_owned(),
            }]);
        }
        let result = runner.run_command(
            program,
            &self.package,
            RuntimeOrEngine::Runtime(self.runtime.clone()),
        );
        let stdout = read_virtual_file(&self.tokio_runtime, &mut stdout)?;
        let stderr = read_virtual_file(&self.tokio_runtime, &mut stderr)?;
        let exit_code = match result {
            Ok(()) => 0,
            Err(error) => raw_wasmer_exit_code(&error).ok_or_else(|| error.to_string())?,
        };

        Ok(RawCommandOutput {
            success: exit_code == 0,
            exit_code: Some(exit_code),
            stdout,
            stderr,
        })
    }
}

#[derive(Clone, Copy)]
struct CommandCase {
    program: &'static str,
    manifest_program: &'static str,
    args: &'static [&'static str],
    stdin: &'static str,
    stdout_byte_limit: Option<usize>,
    expected_success: bool,
    expected_exit_code: Option<i32>,
    stdout_contains: &'static [&'static str],
    stderr_contains: &'static [&'static str],
}

fn command_cases() -> Vec<CommandCase> {
    let cases = command_case_map();
    let tested_commands: BTreeSet<String> = cases.keys().copied().map(str::to_owned).collect();
    let exposed_commands = sorted_strings(available_wasm_command_names());

    assert_eq!(tested_commands, exposed_commands);

    cases.into_values().collect()
}

fn command_case_map() -> BTreeMap<&'static str, CommandCase> {
    let mut cases = BTreeMap::new();

    for case in [
        case_ok("arch", &[], &[]),
        case_ok(
            "base32",
            &["/workspace/input.txt"],
            &["NBSWY3DPEB3W64TMMQFA===="],
        ),
        case_ok("base64", &["/workspace/input.txt"], &["aGVsbG8gd29ybGQK"]),
        case_ok("basename", &["/tmp/example.txt"], &["example.txt"]),
        case_ok("cat", &["/workspace/input.txt"], &["hello world"]),
        case_ok("cksum", &["/workspace/input.txt"], &["input.txt"]),
        case_ok(
            "comm",
            &["/workspace/sorted-a.txt", "/workspace/sorted-b.txt"],
            &["alpha"],
        ),
        case_ok(
            "cp",
            &["/workspace/input.txt", "/workspace/copied.txt"],
            &[],
        ),
        case_ok("csplit", &["/workspace/lines.txt", "2"], &[]),
        case_ok("cut", &["-c", "1-5", "/workspace/input.txt"], &["hello"]),
        case_ok("date", &["+%Y"], &[]),
        case_ok("dircolors", &["--print-database"], &["Configuration file"]),
        case_ok("dirname", &["/tmp/example.txt"], &["/tmp"]),
        case_ok("echo", &["hello"], &["hello"]),
        case_ok("env", &[], &[]),
        case_ok("expand", &["-t", "4", "/workspace/tabs.txt"], &["a   b"]),
        case_ok("factor", &["15"], &["15: 3 5"]),
        case_exit_code("false", &[], 1),
        case_ok("fmt", &["-w", "20", "/workspace/long.txt"], &["alpha beta"]),
        case_ok("fold", &["-w", "5", "/workspace/input.txt"], &["hello"]),
        case_ok(
            "hashsum",
            &["--md5", "/workspace/input.txt"],
            &["input.txt"],
        ),
        case_ok("head", &["-n", "1", "/workspace/lines.txt"], &["one"]),
        case_ok(
            "join",
            &["/workspace/join-a.txt", "/workspace/join-b.txt"],
            &["1 alpha beta"],
        ),
        case_ok(
            "link",
            &["/workspace/input.txt", "/workspace/hardlink.txt"],
            &[],
        ),
        case_ok("ln", &["/workspace/input.txt", "/workspace/ln.txt"], &[]),
        case_ok("ls", &["/workspace"], &["input.txt"]),
        case_ok_as(
            "md5sum",
            "hashsum",
            &["--md5", "/workspace/input.txt"],
            &["input.txt"],
        ),
        case_ok("mkdir", &["/workspace/newdir"], &[]),
        case_ok("mktemp", &["/workspace/tmp.XXXXXX"], &["/workspace/tmp."]),
        case_ok(
            "mv",
            &["/workspace/move-src.txt", "/workspace/move-dst.txt"],
            &[],
        ),
        case_ok("nl", &["/workspace/input.txt"], &["hello world"]),
        case_ok("nproc", &[], &[]),
        case_ok("numfmt", &["1000"], &["1000"]),
        case_ok("od", &["-An", "-tx1", "/workspace/input.txt"], &["68"]),
        case_ok(
            "paste",
            &["/workspace/sorted-a.txt", "/workspace/sorted-b.txt"],
            &["alpha"],
        ),
        case_ok("printenv", &[], &[]),
        case_ok("printf", &["hello"], &["hello"]),
        case_ok("ptx", &["-G", "/workspace/ptx.txt"], &[]),
        case_ok("pwd", &[], &["/workspace"]),
        case_ok("readlink", &["/workspace/symlink.txt"], &["input.txt"]),
        case_ok(
            "realpath",
            &["/workspace/input.txt"],
            &["/workspace/input.txt"],
        ),
        case_ok(
            "relpath",
            &["/workspace/input.txt", "/workspace"],
            &["input.txt"],
        ),
        case_ok("rm", &["/workspace/remove-me.txt"], &[]),
        case_ok(
            "rmdir",
            &["--ignore-fail-on-non-empty", "/workspace/."],
            &[],
        ),
        case_ok("seq", &["1", "3"], &["1\n2\n3"]),
        case_ok_as(
            "sha1sum",
            "hashsum",
            &["--sha1", "/workspace/input.txt"],
            &["input.txt"],
        ),
        case_ok_as(
            "sha224sum",
            "hashsum",
            &["--sha224", "/workspace/input.txt"],
            &["input.txt"],
        ),
        case_ok_as(
            "sha256sum",
            "hashsum",
            &["--sha256", "/workspace/input.txt"],
            &["input.txt"],
        ),
        case_ok_as(
            "sha3-224sum",
            "hashsum",
            &["--sha3-224", "/workspace/input.txt"],
            &["input.txt"],
        ),
        case_ok_as(
            "sha3-256sum",
            "hashsum",
            &["--sha3-256", "/workspace/input.txt"],
            &["input.txt"],
        ),
        case_ok_as(
            "sha3-384sum",
            "hashsum",
            &["--sha3-384", "/workspace/input.txt"],
            &["input.txt"],
        ),
        case_ok_as(
            "sha3-512sum",
            "hashsum",
            &["--sha3-512", "/workspace/input.txt"],
            &["input.txt"],
        ),
        case_ok_as(
            "sha384sum",
            "hashsum",
            &["--sha384", "/workspace/input.txt"],
            &["input.txt"],
        ),
        case_ok_as(
            "sha3sum",
            "hashsum",
            &["--sha3", "--bits", "256", "/workspace/input.txt"],
            &["input.txt"],
        ),
        case_ok_as(
            "sha512sum",
            "hashsum",
            &["--sha512", "/workspace/input.txt"],
            &["input.txt"],
        ),
        case_ok_as(
            "shake128sum",
            "hashsum",
            &["--shake128", "--bits", "128", "/workspace/input.txt"],
            &["input.txt"],
        ),
        case_ok_as(
            "shake256sum",
            "hashsum",
            &["--shake256", "--bits", "128", "/workspace/input.txt"],
            &["input.txt"],
        ),
        case_ok("shred", &["-n", "1", "/workspace/shred-me.txt"], &[]),
        case_ok(
            "shuf",
            &["--random-source", "/workspace/random.txt", "-i", "1-1"],
            &["1"],
        ),
        case_ok("sleep", &["0"], &[]),
        case_ok("sum", &["/workspace/input.txt"], &["3762"]),
        case_ok("tee", &["/workspace/tee-output.txt"], &[]),
        case_ok("touch", &["/workspace/touched.txt"], &[]),
        case_ok_with_stdin("tr", &["a-z", "A-Z"], "hello world\n", &["HELLO WORLD"]),
        case_ok("true", &[], &[]),
        case_ok("truncate", &["-s", "1", "/workspace/truncate-me.txt"], &[]),
        case_ok("tsort", &["/workspace/tsort.txt"], &["a\nb"]),
        case_ok("unexpand", &["-t", "4", "/workspace/spaces.txt"], &["a\tb"]),
        case_ok("uniq", &["/workspace/duplicates.txt"], &["alpha\nbeta"]),
        case_ok("unlink", &["/workspace/unlink-me.txt"], &[]),
        case_ok("wc", &["-l", "/workspace/lines.txt"], &["3"]),
        case_output_limited("yes", &["yes"], 32, &["yes\nyes"]),
    ] {
        assert!(
            cases.insert(case.program, case).is_none(),
            "duplicate command case for {}",
            case.program
        );
    }

    cases
}

fn case_ok(
    program: &'static str,
    args: &'static [&'static str],
    stdout_contains: &'static [&'static str],
) -> CommandCase {
    case_ok_with_stdin(program, args, "", stdout_contains)
}

fn case_ok_with_stdin(
    program: &'static str,
    args: &'static [&'static str],
    stdin: &'static str,
    stdout_contains: &'static [&'static str],
) -> CommandCase {
    CommandCase {
        program,
        manifest_program: program,
        args,
        stdin,
        stdout_byte_limit: None,
        expected_success: true,
        expected_exit_code: Some(0),
        stdout_contains,
        stderr_contains: &[],
    }
}

fn case_ok_as(
    program: &'static str,
    manifest_program: &'static str,
    args: &'static [&'static str],
    stdout_contains: &'static [&'static str],
) -> CommandCase {
    CommandCase {
        program,
        manifest_program,
        args,
        stdin: "",
        stdout_byte_limit: None,
        expected_success: true,
        expected_exit_code: Some(0),
        stdout_contains,
        stderr_contains: &[],
    }
}

fn case_output_limited(
    program: &'static str,
    args: &'static [&'static str],
    stdout_byte_limit: usize,
    stdout_contains: &'static [&'static str],
) -> CommandCase {
    CommandCase {
        program,
        manifest_program: program,
        args,
        stdin: "",
        stdout_byte_limit: Some(stdout_byte_limit),
        expected_success: true,
        expected_exit_code: Some(0),
        stdout_contains,
        stderr_contains: &[],
    }
}

fn case_exit_code(
    program: &'static str,
    args: &'static [&'static str],
    exit_code: i32,
) -> CommandCase {
    CommandCase {
        program,
        manifest_program: program,
        args,
        stdin: "",
        stdout_byte_limit: None,
        expected_success: false,
        expected_exit_code: Some(exit_code),
        stdout_contains: &[],
        stderr_contains: &[],
    }
}

#[derive(Debug, PartialEq, Eq)]
struct RawCommandOutput {
    success: bool,
    exit_code: Option<i32>,
    stdout: String,
    stderr: String,
}

#[derive(Debug)]
struct CappedBufferFile {
    data: Cursor<Vec<u8>>,
    max_len: usize,
}

impl CappedBufferFile {
    fn new(max_len: usize) -> Self {
        Self {
            data: Cursor::new(Vec::new()),
            max_len,
        }
    }
}

impl AsyncSeek for CappedBufferFile {
    fn start_seek(mut self: Pin<&mut Self>, position: SeekFrom) -> io::Result<()> {
        Pin::new(&mut self.data).start_seek(position)
    }

    fn poll_complete(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<u64>> {
        Pin::new(&mut self.data).poll_complete(cx)
    }
}

impl AsyncWrite for CappedBufferFile {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let current_len = self.data.get_ref().len();
        if current_len >= self.max_len {
            return Poll::Ready(Err(io::ErrorKind::BrokenPipe.into()));
        }

        let remaining = self.max_len - current_len;
        let write_len = remaining.min(buf.len());
        Pin::new(&mut self.data).poll_write(cx, &buf[..write_len])
    }

    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        let current_len = self.data.get_ref().len();
        if current_len >= self.max_len {
            return Poll::Ready(Err(io::ErrorKind::BrokenPipe.into()));
        }

        let remaining = self.max_len - current_len;
        let capped_bufs = bufs
            .iter()
            .scan(remaining, |remaining, buf| {
                if *remaining == 0 {
                    return None;
                }
                let write_len = (*remaining).min(buf.len());
                *remaining -= write_len;
                Some(io::IoSlice::new(&buf[..write_len]))
            })
            .collect::<Vec<_>>();

        Pin::new(&mut self.data).poll_write_vectored(cx, &capped_bufs)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.data).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.data).poll_shutdown(cx)
    }
}

impl AsyncRead for CappedBufferFile {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.data).poll_read(cx, buf)
    }
}

impl VirtualFile for CappedBufferFile {
    fn last_accessed(&self) -> u64 {
        1_000_000_000
    }

    fn last_modified(&self) -> u64 {
        1_000_000_000
    }

    fn created_time(&self) -> u64 {
        1_000_000_000
    }

    fn size(&self) -> u64 {
        self.data.get_ref().len() as u64
    }

    fn set_len(&mut self, new_size: u64) -> wasmer_wasix::virtual_fs::Result<()> {
        self.data.get_mut().resize(new_size as usize, 0);
        Ok(())
    }

    fn unlink(&mut self) -> wasmer_wasix::virtual_fs::Result<()> {
        Ok(())
    }

    fn poll_read_ready(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<usize>> {
        let current_position = StdSeek::stream_position(&mut self.data).unwrap_or_default();
        let len = StdSeek::seek(&mut self.data, SeekFrom::End(0)).unwrap_or_default();
        if current_position < len {
            Poll::Ready(Ok((len - current_position) as usize))
        } else {
            Poll::Ready(Ok(0))
        }
    }

    fn poll_write_ready(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<usize>> {
        let current_len = self.data.get_ref().len();
        if current_len >= self.max_len {
            Poll::Ready(Err(io::ErrorKind::BrokenPipe.into()))
        } else {
            Poll::Ready(Ok(self.max_len - current_len))
        }
    }
}

fn print_command_status(command: CommandCase, output: &RawCommandOutput) {
    eprintln!(
        "[{}] {} {}",
        if command.expected_success {
            "normal"
        } else {
            "abnormal"
        },
        command.program,
        shell_words(command.args)
    );
    if command.manifest_program != command.program {
        eprintln!("manifest command: {}", command.manifest_program);
    }
    eprintln!("stdin: {:?}", command.stdin);
    eprintln!(
        "status: success={} exit_code={:?}",
        output.success, output.exit_code
    );
    eprintln!("stdout:\n{}", output.stdout);
    eprintln!("stderr:\n{}", output.stderr);
    eprintln!("---");
}

fn command_status_json(command: CommandCase, output: &RawCommandOutput) -> serde_json::Value {
    json!({
        "group": if command.expected_success { "normal" } else { "abnormal" },
        "command": command.program,
        "manifest_command": command.manifest_program,
        "args": command.args,
        "stdin": command.stdin,
        "stdout_byte_limit": command.stdout_byte_limit,
        "expected_success": command.expected_success,
        "expected_exit_code": command.expected_exit_code,
        "success": output.success,
        "exit_code": output.exit_code,
        "stdout": output.stdout,
        "stderr": output.stderr,
    })
}

fn write_command_status_json(records: &[serde_json::Value]) -> Result<(), String> {
    let path = workspace_target_dir().join("wasm_coreutils_commands.json");
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let json = serde_json::to_string_pretty(records).map_err(|error| error.to_string())?;
    std::fs::write(path, json).map_err(|error| error.to_string())
}

fn workspace_target_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crate is inside workspace crates directory")
        .join("target")
}

fn shell_words(args: &[&str]) -> String {
    args.iter()
        .map(|arg| {
            if arg.contains(char::is_whitespace) {
                format!("{arg:?}")
            } else {
                (*arg).to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn command_status_error(command: CommandCase, output: &RawCommandOutput) -> Result<(), String> {
    if output.success != command.expected_success {
        return Err(format!(
            "{} success mismatch; expected: {}; actual: {}; exit: {:?}; stdout: {}; stderr: {}",
            command.program,
            command.expected_success,
            output.success,
            output.exit_code,
            output.stdout,
            output.stderr
        ));
    }
    if let Some(exit_code) = command.expected_exit_code
        && output.exit_code != Some(exit_code)
    {
        return Err(format!(
            "{} exit code mismatch; expected: {exit_code}; actual: {:?}; stdout: {}; stderr: {}",
            command.program, output.exit_code, output.stdout, output.stderr
        ));
    }
    for expected in command.stdout_contains {
        if !output.stdout.contains(expected) {
            return Err(format!(
                "{} stdout should contain {expected:?}, got: {}",
                command.program, output.stdout
            ));
        }
    }
    for expected in command.stderr_contains {
        if !output.stderr.contains(expected) {
            return Err(format!(
                "{} stderr should contain {expected:?}, got: {}",
                command.program, output.stderr
            ));
        }
    }
    Ok(())
}

fn temp_workspace(command: &str) -> PathBuf {
    let root =
        std::env::temp_dir().join(format!("jux-coreutils-{command}-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).expect("temp workspace is created");
    root
}

fn prepare_workspace(root: &Path) {
    std::fs::write(root.join("input.txt"), "hello world\n").expect("input fixture is written");
    std::fs::write(root.join("lines.txt"), "one\ntwo\nthree\n").expect("lines fixture is written");
    std::fs::write(root.join("sorted-a.txt"), "alpha\nbeta\n")
        .expect("sorted-a fixture is written");
    std::fs::write(root.join("sorted-b.txt"), "alpha\ngamma\n")
        .expect("sorted-b fixture is written");
    std::fs::write(root.join("join-a.txt"), "1 alpha\n2 left\n")
        .expect("join-a fixture is written");
    std::fs::write(root.join("join-b.txt"), "1 beta\n3 right\n")
        .expect("join-b fixture is written");
    std::fs::write(root.join("tabs.txt"), "a\tb\n").expect("tabs fixture is written");
    std::fs::write(root.join("spaces.txt"), "a   b\n").expect("spaces fixture is written");
    std::fs::write(root.join("long.txt"), "alpha beta gamma delta epsilon\n")
        .expect("long fixture is written");
    std::fs::write(root.join("unsorted.txt"), "beta\nalpha\n")
        .expect("unsorted fixture is written");
    std::fs::write(root.join("duplicates.txt"), "alpha\nalpha\nbeta\n")
        .expect("duplicates fixture is written");
    std::fs::write(root.join("tsort.txt"), "a b\n").expect("tsort fixture is written");
    std::fs::write(root.join("ptx.txt"), "alpha beta\n").expect("ptx fixture is written");
    std::fs::write(root.join("random.txt"), "stable random source\n")
        .expect("random source fixture is written");
    for file in [
        "move-src.txt",
        "remove-me.txt",
        "shred-me.txt",
        "truncate-me.txt",
        "unlink-me.txt",
    ] {
        std::fs::write(root.join(file), "data\n").expect("mutable fixture is written");
    }
    std::fs::create_dir_all(root.join("emptydir")).expect("empty dir fixture is created");
    #[cfg(unix)]
    std::os::unix::fs::symlink("input.txt", root.join("symlink.txt"))
        .expect("symlink fixture is created");
}

fn virtual_file_with_contents(
    runtime: &tokio::runtime::Runtime,
    contents: &str,
) -> Result<ArcFile<BufferFile>, String> {
    let mut file = ArcFile::new(Box::<BufferFile>::default());
    runtime.block_on(async {
        file.write_all(contents.as_bytes())
            .await
            .map_err(|error| error.to_string())?;
        file.rewind().await.map_err(|error| error.to_string())
    })?;
    Ok(file)
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

fn sorted_strings(strings: impl IntoIterator<Item = &'static str>) -> BTreeSet<String> {
    strings.into_iter().map(str::to_owned).collect()
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

fn read_virtual_file<T>(
    runtime: &tokio::runtime::Runtime,
    file: &mut ArcFile<T>,
) -> Result<String, String>
where
    T: VirtualFile + Send + Sync + 'static,
{
    runtime.block_on(async {
        file.rewind().await.map_err(|error| error.to_string())?;
        let mut output = Vec::new();
        file.read_to_end(&mut output)
            .await
            .map_err(|error| error.to_string())?;
        Ok(String::from_utf8_lossy(&output).to_string())
    })
}
