use jux_core::{
    WasmCommandRequest, WasmEnvironmentCapability, WasmEnvironmentPolicy, WasmFilesystemCapability,
    WasmFilesystemPolicy, WasmHttpMatchKind, WasmHttpMethod, WasmHttpRule, WasmHttpRuleEffect,
    WasmNetworkCapability, WasmNetworkPolicy, WasmPackageLoadingCapability, WasmRuntimeError,
    WasmSandboxPolicy, WasmStdioCapability, WasmerRuntime, WasmerRuntimeCapabilities,
};
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};

static WASMER_COMMAND_TEST_LOCK: Mutex<()> = Mutex::new(());

#[test]
fn wasmer_runtime_calls_exported_i32_function() {
    let wasm = wasmer::wat2wasm(
        br#"
        (module
          (func (export "answer") (result i32)
            i32.const 42))
        "#,
    )
    .expect("wat compiles");
    let runtime = WasmerRuntime::new();

    let result = runtime
        .call_exported_i32_function(&wasm, "answer")
        .expect("wasm function runs");

    assert_eq!(result, 42);
}

#[test]
fn wasmer_runtime_runs_coreutils_command() {
    let root = temp_workspace_root();
    std::fs::write(root.join("hello.txt"), "hello").expect("fixture file is written");
    let runtime = WasmerRuntime::new();

    let output = runtime
        .run_coreutils_command(WasmCommandRequest {
            program: "cat".to_owned(),
            args: vec!["hello.txt".to_owned()],
            host_directory: root,
        })
        .expect("coreutils command runs");

    assert!(output.success);
    assert_eq!(output.exit_code, Some(0));
    assert_eq!(output.stdout, "hello");
    assert_eq!(output.stderr, "");
}

#[test]
fn wasmer_runtime_exposes_default_capabilities() {
    let runtime = WasmerRuntime::new();

    assert_eq!(
        runtime.capabilities(),
        &WasmerRuntimeCapabilities {
            filesystem: WasmFilesystemCapability::MappedHostDirectory,
            environment: WasmEnvironmentCapability::Isolated,
            stdio: WasmStdioCapability::Buffered,
            network: WasmNetworkCapability::HttpClient,
            http_policy: None,
            package_loading: WasmPackageLoadingCapability::BuiltinWithHttpClient,
        }
    );
}

#[test]
fn wasmer_runtime_can_be_created_from_wasm_policy() {
    let network = WasmNetworkPolicy {
        http_rules: vec![WasmHttpRule {
            effect: WasmHttpRuleEffect::Allow,
            method: WasmHttpMethod::Get,
            match_kind: WasmHttpMatchKind::Literal,
            pattern: "https://api.example.com/v1/status".to_owned(),
        }],
    };
    let policy = WasmSandboxPolicy {
        filesystem: WasmFilesystemPolicy::ReadWriteWorkspace,
        environment: WasmEnvironmentPolicy::Isolated,
        network: network.clone(),
        packages: Vec::new(),
    };

    let runtime = WasmerRuntime::with_wasm_policy(&policy);

    assert_eq!(
        runtime.capabilities().network,
        WasmNetworkCapability::HttpClient
    );
    assert_eq!(runtime.capabilities().http_policy, Some(network));
}

#[test]
fn wasmer_runtime_can_disable_host_filesystem_mapping() {
    let _guard = wasmer_command_test_lock();
    let root = temp_workspace_root();
    std::fs::write(root.join("hello.txt"), "hello").expect("fixture file is written");
    let runtime = WasmerRuntime::with_capabilities(WasmerRuntimeCapabilities {
        filesystem: WasmFilesystemCapability::Disabled,
        ..WasmerRuntimeCapabilities::default()
    });

    let output = runtime
        .run_coreutils_command(WasmCommandRequest {
            program: "cat".to_owned(),
            args: vec!["hello.txt".to_owned()],
            host_directory: root,
        })
        .expect("coreutils command runs without a mapped host directory");

    assert!(!output.success);
    assert_eq!(output.exit_code, Some(1));
    assert_eq!(output.stdout, "");
}

#[test]
fn wasmer_runtime_rejects_http_package_loading_when_network_is_disabled() {
    let _guard = wasmer_command_test_lock();
    let runtime = WasmerRuntime::with_capabilities(WasmerRuntimeCapabilities {
        network: WasmNetworkCapability::Disabled,
        package_loading: WasmPackageLoadingCapability::BuiltinWithHttpClient,
        ..WasmerRuntimeCapabilities::default()
    });

    let error = runtime
        .run_coreutils_command(WasmCommandRequest {
            program: "true".to_owned(),
            args: Vec::new(),
            host_directory: temp_workspace_root(),
        })
        .expect_err("http package loading requires network capability");

    assert_eq!(
        error,
        WasmRuntimeError::Run("wasm package loading requires an enabled http client".to_owned())
    );
}

#[test]
fn wasmer_runtime_rejects_non_coreutils_command() {
    let runtime = WasmerRuntime::new();

    let error = runtime
        .run_coreutils_command(WasmCommandRequest {
            program: "definitely-not-coreutils".to_owned(),
            args: Vec::new(),
            host_directory: temp_workspace_root(),
        })
        .expect_err("unsupported command is rejected");

    assert_eq!(
        error,
        WasmRuntimeError::UnsupportedCommand("definitely-not-coreutils".to_owned())
    );
}

fn temp_workspace_root() -> PathBuf {
    let root = std::env::temp_dir().join(format!("jux-wasm-test-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).expect("temp workspace is created");
    root
}

fn wasmer_command_test_lock() -> MutexGuard<'static, ()> {
    WASMER_COMMAND_TEST_LOCK
        .lock()
        .expect("wasmer command test lock is not poisoned")
}
