use jux_core::{
    NativeCommandPolicy, RuntimePolicy, WasmEnvironmentCapability, WasmEnvironmentPolicy,
    WasmFilesystemCapability, WasmFilesystemPolicy, WasmHttpDecision, WasmHttpMatchKind,
    WasmHttpMethod, WasmHttpRule, WasmHttpRuleEffect, WasmNetworkCapability, WasmNetworkPolicy,
    WasmPackageLoadingCapability, WasmPackageRule, WasmPackageSource, WasmSandboxPolicy,
    WasmStdioCapability, WasmerRuntimeCapabilities,
};

#[test]
fn runtime_policy_workspace_default_disables_native_commands() {
    let workspace_root = std::path::PathBuf::from("/workspace");
    let policy = RuntimePolicy::workspace_default(workspace_root.clone());

    assert_eq!(policy.workspace_root, workspace_root);
    assert_eq!(policy.native, NativeCommandPolicy::disabled());
    assert_eq!(
        policy.wasm,
        WasmSandboxPolicy {
            filesystem: WasmFilesystemPolicy::ReadWriteWorkspace,
            environment: WasmEnvironmentPolicy::Isolated,
            network: WasmNetworkPolicy {
                http_rules: Vec::new()
            },
            packages: vec![WasmPackageRule {
                package: "wasmer/coreutils".to_owned(),
                version: Some("1.0.25".to_owned()),
                source: WasmPackageSource::Builtin,
            }],
        }
    );
    assert!(
        policy
            .wasm
            .allows_package("wasmer/coreutils", Some("1.0.25"))
    );
    assert!(!policy.wasm.allows_package("wasmer/python", None));
}

#[test]
fn runtime_policy_derives_wasm_capabilities() {
    let policy = RuntimePolicy::workspace_default("/workspace");
    let capabilities = policy.wasm_capabilities();

    assert_eq!(
        capabilities,
        WasmerRuntimeCapabilities {
            filesystem: WasmFilesystemCapability::MappedHostDirectory,
            environment: WasmEnvironmentCapability::Isolated,
            stdio: WasmStdioCapability::Buffered,
            network: WasmNetworkCapability::Disabled,
            http_policy: None,
            package_loading: WasmPackageLoadingCapability::Builtin,
        }
    );
}

#[test]
fn wasm_sandbox_policy_derives_disabled_wasmer_capabilities() {
    let policy = WasmSandboxPolicy {
        filesystem: WasmFilesystemPolicy::Disabled,
        environment: WasmEnvironmentPolicy::Isolated,
        network: WasmNetworkPolicy {
            http_rules: Vec::new(),
        },
        packages: Vec::new(),
    };
    let capabilities = WasmerRuntimeCapabilities::from(&policy);

    assert_eq!(
        capabilities,
        WasmerRuntimeCapabilities {
            filesystem: WasmFilesystemCapability::Disabled,
            environment: WasmEnvironmentCapability::Isolated,
            stdio: WasmStdioCapability::Buffered,
            network: WasmNetworkCapability::Disabled,
            http_policy: None,
            package_loading: WasmPackageLoadingCapability::Disabled,
        }
    );
}

#[test]
fn wasm_sandbox_policy_allows_configured_packages() {
    let policy = WasmSandboxPolicy {
        filesystem: WasmFilesystemPolicy::ReadWriteWorkspace,
        environment: WasmEnvironmentPolicy::Isolated,
        network: WasmNetworkPolicy {
            http_rules: Vec::new(),
        },
        packages: vec![
            WasmPackageRule {
                package: "wasmer/coreutils".to_owned(),
                version: Some("1.0.25".to_owned()),
                source: WasmPackageSource::Builtin,
            },
            WasmPackageRule {
                package: "example/custom-tools".to_owned(),
                version: None,
                source: WasmPackageSource::ConfiguredLocal,
            },
        ],
    };

    assert!(policy.allows_package("wasmer/coreutils", Some("1.0.25")));
    assert!(policy.allows_package("example/custom-tools", Some("0.1.0")));
    assert!(!policy.allows_package("example/other-tools", None));
}

#[test]
fn wasm_package_policy_enables_http_package_loading_when_needed() {
    let policy = WasmSandboxPolicy {
        filesystem: WasmFilesystemPolicy::ReadWriteWorkspace,
        environment: WasmEnvironmentPolicy::Isolated,
        network: WasmNetworkPolicy {
            http_rules: Vec::new(),
        },
        packages: vec![WasmPackageRule {
            package: "example/remote-tools".to_owned(),
            version: Some("0.1.0".to_owned()),
            source: WasmPackageSource::ConfiguredHttp,
        }],
    };
    let capabilities = WasmerRuntimeCapabilities::from(&policy);

    assert_eq!(
        capabilities.package_loading,
        WasmPackageLoadingCapability::BuiltinWithHttpClient
    );
}

#[test]
fn wasm_network_policy_uses_ordered_http_rules() {
    let policy = WasmSandboxPolicy {
        filesystem: WasmFilesystemPolicy::ReadWriteWorkspace,
        environment: WasmEnvironmentPolicy::Isolated,
        network: WasmNetworkPolicy {
            http_rules: vec![
                WasmHttpRule {
                    effect: WasmHttpRuleEffect::Deny,
                    method: WasmHttpMethod::Get,
                    match_kind: WasmHttpMatchKind::Literal,
                    pattern: "https://api.example.com/v1/private".to_owned(),
                },
                WasmHttpRule {
                    effect: WasmHttpRuleEffect::Allow,
                    method: WasmHttpMethod::Get,
                    match_kind: WasmHttpMatchKind::Wildcard,
                    pattern: "https://api.example.com/v1/*".to_owned(),
                },
            ],
        },
        packages: Vec::new(),
    };
    let expected_http_policy = policy.network.clone();
    let capabilities = WasmerRuntimeCapabilities::from(&policy);

    assert_eq!(
        policy
            .network
            .decide_http_request(WasmHttpMethod::Get, "https://api.example.com/v1/private")
            .expect("HTTP policy decision succeeds"),
        WasmHttpDecision::Deny
    );
    assert_eq!(
        policy
            .network
            .decide_http_request(WasmHttpMethod::Get, "https://api.example.com/v1/users")
            .expect("HTTP policy decision succeeds"),
        WasmHttpDecision::Allow
    );
    assert_eq!(
        policy
            .network
            .decide_http_request(WasmHttpMethod::Post, "https://api.example.com/v1/users")
            .expect("HTTP policy decision succeeds"),
        WasmHttpDecision::Deny
    );
    assert_eq!(capabilities.network, WasmNetworkCapability::HttpClient);
    assert_eq!(capabilities.http_policy, Some(expected_http_policy));
}

#[test]
fn wasm_network_policy_supports_regex_http_rules() {
    let policy = WasmNetworkPolicy {
        http_rules: vec![WasmHttpRule {
            effect: WasmHttpRuleEffect::Allow,
            method: WasmHttpMethod::Post,
            match_kind: WasmHttpMatchKind::Regex,
            pattern: "^https://api\\.example\\.com:443/v1/items/[0-9]+$".to_owned(),
        }],
    };

    assert_eq!(
        policy
            .decide_http_request(
                WasmHttpMethod::Post,
                "https://api.example.com:443/v1/items/42"
            )
            .expect("HTTP policy decision succeeds"),
        WasmHttpDecision::Allow
    );
    assert_eq!(
        policy
            .decide_http_request(
                WasmHttpMethod::Post,
                "https://api.example.com:443/v1/items/new"
            )
            .expect("HTTP policy decision succeeds"),
        WasmHttpDecision::Deny
    );
}
