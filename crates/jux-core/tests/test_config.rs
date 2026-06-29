use jux_core::{
    JuxConfig, JuxConfigLoader, NativeCommandPolicy, NativeCommandRule, RuntimePolicy,
    WasmFilesystemAccess, WasmFilesystemDecision, WasmHttpDecision, WasmHttpMethod,
};
use std::fs;
use std::path::PathBuf;

#[test]
fn yaml_config_resolves_to_runtime_policy() {
    let config = JuxConfig::from_yaml_str(
        r#"
version: 1
model:
  provider: "openai"
  name: "gpt-5"
agent:
  max_iterations: 20
sandbox:
  filesystem:
    workdirs:
      - "."
    rules:
      - effect: "deny"
        base: "workdir"
        match: "wildcard"
        path: "secrets/**"
        permissions: ["read", "write"]
      - effect: "allow"
        base: "workdir"
        match: "wildcard"
        path: "src/**"
        permissions: ["read", "write"]
  network:
    http:
      rules:
        - effect: "allow"
          method: "GET"
          match: "wildcard"
          pattern: "https://api.github.com/**"
  native:
    commands:
      - "git"
logging:
  level: "info"
"#,
    )
    .expect("YAML config parses");

    let resolved = config
        .resolve(PathBuf::from("/workspace/project"))
        .expect("config resolves");
    let policy = resolved.runtime_policy;

    assert_eq!(
        policy
            .wasm
            .filesystem
            .decide_path_access(
                "/workspace/project/secrets/token.txt",
                WasmFilesystemAccess::Read
            )
            .expect("filesystem decision succeeds"),
        WasmFilesystemDecision::Deny
    );
    assert_eq!(
        policy
            .wasm
            .filesystem
            .decide_path_access("/workspace/project/src/main.rs", WasmFilesystemAccess::Read)
            .expect("filesystem decision succeeds"),
        WasmFilesystemDecision::Allow
    );
    assert_eq!(
        policy
            .wasm
            .filesystem
            .decide_path_access(
                "/workspace/project/src/main.rs",
                WasmFilesystemAccess::ReadWrite
            )
            .expect("filesystem decision succeeds"),
        WasmFilesystemDecision::Allow
    );
    assert_eq!(
        policy
            .wasm
            .network
            .decide_http_request(
                WasmHttpMethod::Get,
                "https://api.github.com/repos/jux-project/jux"
            )
            .expect("network decision succeeds"),
        WasmHttpDecision::Allow
    );
    assert_eq!(
        policy.native,
        NativeCommandPolicy {
            enabled: true,
            allowed_commands: vec![NativeCommandRule {
                program: "git".to_owned()
            }],
        }
    );
}

#[test]
fn jsonc_config_supports_comments_and_default_policy() {
    let config = JuxConfig::from_jsonc_str(
        r#"
{
  // User-level model preference.
  "model": {
    "provider": "openai",
    "name": "gpt-5"
  },
  "sandbox": {
    "network": {
      "http": {
        "rules": [
          {
            "effect": "allow",
            "method": "GET",
            "match": "literal",
            "pattern": "https://api.example.com/ping"
          }
        ]
      }
    }
  }
}
"#,
    )
    .expect("JSONC config parses");

    let resolved = config
        .resolve(PathBuf::from("/workspace/project"))
        .expect("config resolves");
    let policy = resolved.runtime_policy;

    assert_eq!(
        policy
            .wasm
            .filesystem
            .decide_path_access("/workspace/project/src/main.rs", WasmFilesystemAccess::Read)
            .expect("filesystem decision succeeds"),
        WasmFilesystemDecision::Allow
    );
    assert_eq!(
        policy
            .wasm
            .filesystem
            .decide_path_access(
                "/workspace/project/src/main.rs",
                WasmFilesystemAccess::ReadWrite
            )
            .expect("filesystem decision succeeds"),
        WasmFilesystemDecision::Deny
    );
    assert_eq!(
        policy
            .wasm
            .network
            .decide_http_request(WasmHttpMethod::Get, "https://api.example.com/ping")
            .expect("network decision succeeds"),
        WasmHttpDecision::Allow
    );
}

#[test]
fn loader_merges_user_config_over_code_defaults() {
    let home = unique_temp_dir("jux-config-home");
    fs::create_dir_all(home.join(".jux")).expect("config directory is created");
    fs::write(
        home.join(".jux/config.yaml"),
        r#"
sandbox:
  filesystem:
    rules:
      - effect: "deny"
        base: "workdir"
        match: "wildcard"
        path: "**"
        permissions: ["read", "write"]
agent:
  max_iterations: 12
"#,
    )
    .expect("config file is written");

    let config = JuxConfigLoader::new(home.clone())
        .load()
        .expect("user config loads");
    let resolved = config
        .resolve(PathBuf::from("/workspace/project"))
        .expect("config resolves");

    assert_eq!(resolved.agent.max_iterations, 12);
    assert_eq!(
        resolved
            .runtime_policy
            .wasm
            .filesystem
            .decide_path_access("/workspace/project/src/main.rs", WasmFilesystemAccess::Read)
            .expect("filesystem decision succeeds"),
        WasmFilesystemDecision::Deny
    );
    fs::remove_dir_all(home).expect("temporary directory is removed");
}

#[test]
fn loader_rejects_null_values() {
    let home = unique_temp_dir("jux-config-null-home");
    fs::create_dir_all(home.join(".jux")).expect("config directory is created");
    fs::write(home.join(".jux/config.yaml"), "model: null").expect("config file is written");

    let error = JuxConfigLoader::new(home.clone())
        .load()
        .expect_err("null config values are rejected");

    assert!(error.to_string().contains("null values are not supported"));
    fs::remove_dir_all(home).expect("temporary directory is removed");
}

#[test]
fn default_config_preserves_workspace_runtime_policy() {
    let config = JuxConfig::default();
    let policy = config
        .resolve(PathBuf::from("/workspace/project"))
        .expect("default config resolves")
        .runtime_policy;

    assert_eq!(
        policy,
        RuntimePolicy::workspace_default("/workspace/project")
    );
}

fn unique_temp_dir(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("{name}-{}", uuid::Uuid::new_v4()))
}
