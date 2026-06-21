use jux_core::{
    MatchPattern, MatchPatternKind, NativeCommandPolicy, RuntimePolicy, WasmEnvironmentCapability,
    WasmEnvironmentPolicy, WasmFilesystemAccess, WasmFilesystemCapability, WasmFilesystemDecision,
    WasmFilesystemPermissions, WasmFilesystemPolicy, WasmFilesystemRule, WasmHttpDecision,
    WasmHttpMatchKind, WasmHttpMethod, WasmHttpRule, WasmHttpRuleEffect, WasmNetworkCapability,
    WasmNetworkPolicy, WasmPackageLoadingCapability, WasmPackageRule, WasmPackageSource,
    WasmSandboxPolicy, WasmStdioCapability, WasmerRuntimeCapabilities,
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
            filesystem: WasmFilesystemPolicy::read_write_workdir(workspace_root.clone()),
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
        filesystem: WasmFilesystemPolicy::disabled(),
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
fn wasm_filesystem_policy_uses_ordered_rules_and_default_deny() {
    let policy = WasmFilesystemPolicy::new(
        vec!["/workspace/a".into(), "/workspace/b".into()],
        vec![
            WasmFilesystemRule::deny("secrets/**"),
            WasmFilesystemRule::allow_read_write("src/**"),
            WasmFilesystemRule::allow_read("README.md"),
            WasmFilesystemRule::allow_read_write_absolute("/tmp/jux/**"),
            WasmFilesystemRule::workdir_regex("docs/.+\\.md", WasmFilesystemPermissions::read()),
        ],
    );

    assert_eq!(
        policy
            .decide_path_access("src/main.rs", WasmFilesystemAccess::ReadWrite)
            .expect("filesystem decision succeeds"),
        WasmFilesystemDecision::Allow
    );
    assert_eq!(
        policy
            .decide_path_access("/workspace/a/secrets/token.txt", WasmFilesystemAccess::Read)
            .expect("filesystem decision succeeds"),
        WasmFilesystemDecision::Deny
    );
    assert_eq!(
        policy
            .decide_path_access(
                "/workspace/a/public/../secrets/token.txt",
                WasmFilesystemAccess::Read
            )
            .expect("filesystem decision succeeds"),
        WasmFilesystemDecision::Deny
    );
    assert_eq!(
        policy
            .decide_path_access("/workspace/b/README.md", WasmFilesystemAccess::Write)
            .expect("filesystem decision succeeds"),
        WasmFilesystemDecision::Deny
    );
    assert_eq!(
        policy
            .decide_path_access("/workspace/b/docs/guide.md", WasmFilesystemAccess::Read)
            .expect("filesystem decision succeeds"),
        WasmFilesystemDecision::Allow
    );
    assert_eq!(
        policy
            .decide_path_access("/tmp/jux/session/out.txt", WasmFilesystemAccess::Write)
            .expect("filesystem decision succeeds"),
        WasmFilesystemDecision::Allow
    );
    assert_eq!(
        policy
            .decide_path_access("/workspace/a/notes.txt", WasmFilesystemAccess::Read)
            .expect("filesystem decision succeeds"),
        WasmFilesystemDecision::Deny
    );
}

#[test]
fn wasm_sandbox_policy_allows_configured_packages() {
    let policy = WasmSandboxPolicy {
        filesystem: WasmFilesystemPolicy::read_write_workdir("/workspace"),
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
        filesystem: WasmFilesystemPolicy::read_write_workdir("/workspace"),
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
        filesystem: WasmFilesystemPolicy::read_write_workdir("/workspace"),
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
                    pattern: "https://api.example.com/v1/**".to_owned(),
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
fn policy_pattern_supports_glob_like_wildcards() {
    let direct_child = MatchPattern::new(MatchPatternKind::Wildcard, "/workspace/*.rs");
    let recursive = MatchPattern::new(MatchPatternKind::Wildcard, "/workspace/**/*.rs");
    let single_char = MatchPattern::new(MatchPatternKind::Wildcard, "/workspace/file-?.txt");
    let escaped = MatchPattern::new(MatchPatternKind::Wildcard, "/workspace/\\*.txt");

    assert!(
        direct_child
            .matches("/workspace/main.rs")
            .expect("wildcard matches")
    );
    assert!(
        !direct_child
            .matches("/workspace/src/main.rs")
            .expect("wildcard matches")
    );
    assert!(
        recursive
            .matches("/workspace/src/main.rs")
            .expect("recursive wildcard matches")
    );
    assert!(
        single_char
            .matches("/workspace/file-a.txt")
            .expect("single-character wildcard matches")
    );
    assert!(
        !single_char
            .matches("/workspace/file-ab.txt")
            .expect("single-character wildcard matches")
    );
    assert!(
        escaped
            .matches("/workspace/*.txt")
            .expect("escaped wildcard matches")
    );
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
