//! User-facing configuration for Jux.
//!
//! This module owns the customer configuration format. It is intentionally kept
//! separate from runtime policy types: config files are optimized for stable
//! user-facing semantics, while policy types are optimized for execution.
//!
//! The first configuration layer supported by this module is the user-level
//! config under `~/.jux/`. Project-local configuration is intentionally not
//! loaded here yet. `JuxConfigLoader` looks for the first existing file in this
//! order:
//!
//! 1. `.jux/config.yaml`
//! 2. `.jux/config.yml`
//! 3. `.jux/config.jsonc`
//! 4. `.jux/config.json`
//!
//! A config file is merged over the built-in defaults before it is deserialized
//! into strongly typed config structs. Merge rules are deliberately simple:
//! objects are merged recursively, arrays are replaced as a whole, scalar values
//! override lower-priority values, and `null` is rejected.
//!
//! Resolution is a separate step. `JuxConfig::resolve` takes the runtime
//! workspace root, resolves relative filesystem workdirs against it, and
//! converts the customer config into the internal `RuntimePolicy` consumed by
//! execution backends.

use crate::{
    MatchPattern, MatchPatternKind, NativeCommandPolicy, NativeCommandRule, RuntimePolicy,
    WasmEnvironmentPolicy, WasmFilesystemPermissions, WasmFilesystemPolicy, WasmFilesystemRule,
    WasmFilesystemRuleBase, WasmHttpMatchKind, WasmHttpMethod, WasmHttpRule, WasmHttpRuleEffect,
    WasmNetworkPolicy, WasmPackageRule, WasmPackageSource, WasmSandboxPolicy,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::error::Error;
use std::fmt::{self, Display};
use std::fs;
use std::path::{Path, PathBuf};

const CONFIG_RELATIVE_PATHS: &[&str] = &[
    ".jux/config.yaml",
    ".jux/config.yml",
    ".jux/config.jsonc",
    ".jux/config.json",
];

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
/// Customer-facing Jux configuration.
///
/// This is the top-level config shape accepted from YAML, JSON, or JSONC. It
/// should stay focused on user preferences and sandbox policy. Internal runtime
/// wiring such as tool registration, storage paths, prompt files, and built-in
/// WASM packages should not be exposed here.
pub struct JuxConfig {
    /// Config format version.
    pub version: u32,
    /// Optional JSON Schema hint used by JSON/JSONC editors.
    #[serde(rename = "$schema")]
    pub schema: Option<String>,
    /// Model preference used by higher-level application code.
    pub model: ModelConfig,
    /// Agent behavior settings that are safe to expose to users.
    pub agent: AgentConfig,
    /// User-facing sandbox policy.
    pub sandbox: SandboxConfig,
    /// Logging preference used by higher-level application code.
    pub logging: LoggingConfig,
}

impl JuxConfig {
    /// Parses one YAML config string and merges it over built-in defaults.
    pub fn from_yaml_str(input: &str) -> Result<Self, ConfigError> {
        let value = serde_yaml::from_str(input)
            .map_err(|error| ConfigError::new(format!("failed to parse YAML config: {error}")))?;
        Self::from_overlay_value(value)
    }

    /// Parses one JSONC config string and merges it over built-in defaults.
    ///
    /// Line comments and block comments are stripped before JSON parsing.
    pub fn from_jsonc_str(input: &str) -> Result<Self, ConfigError> {
        let json = strip_jsonc_comments(input)?;
        let value = serde_json::from_str(&json)
            .map_err(|error| ConfigError::new(format!("failed to parse JSONC config: {error}")))?;
        Self::from_overlay_value(value)
    }

    /// Resolves this user config into runtime-ready configuration.
    ///
    /// `workspace_root` is supplied by the caller because user-level config is
    /// global and does not belong to one project. Relative filesystem workdirs
    /// are resolved against this root during conversion.
    pub fn resolve(
        self,
        workspace_root: impl Into<PathBuf>,
    ) -> Result<ResolvedConfig, ConfigError> {
        let workspace_root = workspace_root.into();
        let runtime_policy = self.sandbox.into_runtime_policy(workspace_root)?;
        Ok(ResolvedConfig {
            model: self.model,
            agent: self.agent,
            logging: self.logging,
            runtime_policy,
        })
    }

    fn from_overlay_value(overlay: Value) -> Result<Self, ConfigError> {
        let mut value = serde_json::to_value(JuxConfig::default()).map_err(|error| {
            ConfigError::new(format!("failed to build default config: {error}"))
        })?;
        reject_null_values(&overlay, "config")?;
        merge_value(&mut value, overlay);
        serde_json::from_value(value)
            .map_err(|error| ConfigError::new(format!("invalid config shape: {error}")))
    }
}

impl Default for JuxConfig {
    fn default() -> Self {
        Self {
            version: 1,
            schema: None,
            model: ModelConfig::default(),
            agent: AgentConfig::default(),
            sandbox: SandboxConfig::default(),
            logging: LoggingConfig::default(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// Resolved configuration ready for runtime construction.
///
/// This type keeps user-facing preferences next to the internal runtime policy
/// produced from sandbox settings. Application layers can decide when to apply
/// model, agent, and logging preferences; execution layers should consume the
/// `runtime_policy` field.
pub struct ResolvedConfig {
    /// Resolved model preference.
    pub model: ModelConfig,
    /// Resolved agent settings.
    pub agent: AgentConfig,
    /// Resolved logging preference.
    pub logging: LoggingConfig,
    /// Runtime policy consumed by tools and execution backends.
    pub runtime_policy: RuntimePolicy,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
/// Model preference from the user config.
pub struct ModelConfig {
    /// Model provider identifier, for example `openai`.
    pub provider: String,
    /// Model name within the selected provider.
    pub name: String,
}

impl Default for ModelConfig {
    fn default() -> Self {
        Self {
            provider: "openai".to_owned(),
            name: "gpt-5".to_owned(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
/// Agent-level user settings.
pub struct AgentConfig {
    /// Maximum number of main-loop iterations allowed for one run.
    pub max_iterations: usize,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self { max_iterations: 20 }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
/// Sandbox configuration exposed to users.
///
/// WASM package configuration is not exposed here. Built-in WASM packages are
/// selected by the program and added during resolution.
pub struct SandboxConfig {
    /// Filesystem sandbox settings.
    pub filesystem: FilesystemConfig,
    /// Network sandbox settings.
    pub network: NetworkConfig,
    /// Native command settings.
    pub native: NativeConfig,
}

impl SandboxConfig {
    fn into_runtime_policy(self, workspace_root: PathBuf) -> Result<RuntimePolicy, ConfigError> {
        let wasm = WasmSandboxPolicy {
            filesystem: self.filesystem.into_policy(&workspace_root)?,
            environment: WasmEnvironmentPolicy::Isolated,
            network: self.network.into_policy()?,
            packages: builtin_wasm_packages(),
        };
        Ok(RuntimePolicy {
            workspace_root,
            wasm,
            native: self.native.into_policy(),
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
/// Filesystem sandbox configuration.
///
/// `workdirs` define the roots used by workdir-based filesystem rules. Relative
/// workdirs are resolved against the runtime workspace root. Rules are ordered:
/// the first matching rule decides the request, and requests with no matching
/// rule are denied by the policy layer.
pub struct FilesystemConfig {
    /// Workdir roots used for relative filesystem rules.
    pub workdirs: Vec<PathBuf>,
    /// Ordered filesystem access rules.
    pub rules: Vec<FilesystemRuleConfig>,
}

impl FilesystemConfig {
    fn into_policy(self, workspace_root: &Path) -> Result<WasmFilesystemPolicy, ConfigError> {
        let workdirs = self.resolve_workdirs(workspace_root);
        let rules = self
            .rules
            .into_iter()
            .map(FilesystemRuleConfig::into_rule)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(WasmFilesystemPolicy::new(workdirs, rules))
    }

    fn resolve_workdirs(&self, workspace_root: &Path) -> Vec<PathBuf> {
        self.workdirs
            .iter()
            .map(|workdir| {
                if workdir.is_absolute() {
                    workdir.to_path_buf()
                } else {
                    workspace_root.join(workdir)
                }
            })
            .collect()
    }
}

impl Default for FilesystemConfig {
    fn default() -> Self {
        Self {
            workdirs: vec![PathBuf::from(".")],
            rules: vec![FilesystemRuleConfig::allow_read_write("**")],
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
/// One ordered filesystem rule from the user config.
///
/// For `base = "workdir"`, `path` is matched relative to each configured
/// workdir. For `base = "absolute"`, `path` is matched against the normalized
/// absolute requested path. `permissions` controls which operations are affected
/// by this rule.
pub struct FilesystemRuleConfig {
    /// Whether a matching request is allowed or denied.
    pub effect: RuleEffect,
    /// Path base used by this rule.
    pub base: FilesystemRuleBaseConfig,
    /// Pattern matching strategy.
    #[serde(rename = "match")]
    pub match_kind: MatchKindConfig,
    /// Path pattern interpreted using `match_kind`.
    pub path: String,
    /// Filesystem operations affected by this rule.
    pub permissions: Vec<FilesystemPermissionConfig>,
}

impl FilesystemRuleConfig {
    fn allow_read_write(path: impl Into<String>) -> Self {
        let permissions = vec![
            FilesystemPermissionConfig::Read,
            FilesystemPermissionConfig::Write,
        ];
        Self {
            effect: RuleEffect::Allow,
            base: FilesystemRuleBaseConfig::Workdir,
            match_kind: MatchKindConfig::Wildcard,
            path: path.into(),
            permissions,
        }
    }

    fn into_rule(self) -> Result<WasmFilesystemRule, ConfigError> {
        let permissions = self.permissions();
        let pattern = MatchPattern::new(self.match_kind.into(), self.path);
        Ok(WasmFilesystemRule::new(
            self.base.into(),
            pattern,
            permissions,
        ))
    }

    fn permissions(&self) -> WasmFilesystemPermissions {
        let allow_read = self.permissions.contains(&FilesystemPermissionConfig::Read);
        let allow_write = self
            .permissions
            .contains(&FilesystemPermissionConfig::Write);
        match self.effect {
            RuleEffect::Allow => filesystem_permissions(allow_read, allow_write),
            RuleEffect::Deny => filesystem_permissions(!allow_read, !allow_write),
        }
    }
}

impl Default for FilesystemRuleConfig {
    fn default() -> Self {
        Self::allow_read_write("**")
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
/// Network sandbox configuration.
///
/// Only HTTP client rules are user-configurable in the first config version.
pub struct NetworkConfig {
    /// HTTP client policy.
    pub http: HttpConfig,
}

impl NetworkConfig {
    fn into_policy(self) -> Result<WasmNetworkPolicy, ConfigError> {
        let http_rules = self
            .http
            .rules
            .into_iter()
            .map(HttpRuleConfig::into_rule)
            .collect::<Vec<_>>();
        Ok(WasmNetworkPolicy { http_rules })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
/// HTTP client sandbox configuration.
///
/// HTTP rules are ordered. The first rule whose method and URL pattern match
/// decides the request; requests with no matching rule are denied.
pub struct HttpConfig {
    /// Ordered HTTP access rules.
    pub rules: Vec<HttpRuleConfig>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
/// One ordered HTTP access rule from the user config.
pub struct HttpRuleConfig {
    /// Whether a matching HTTP request is allowed or denied.
    pub effect: RuleEffect,
    /// HTTP method matched by this rule.
    pub method: HttpMethodConfig,
    /// URL pattern matching strategy.
    #[serde(rename = "match")]
    pub match_kind: MatchKindConfig,
    /// Full URL pattern interpreted using `match_kind`.
    pub pattern: String,
}

impl HttpRuleConfig {
    fn into_rule(self) -> WasmHttpRule {
        WasmHttpRule {
            effect: self.effect.into(),
            method: self.method.into(),
            match_kind: self.match_kind.into(),
            pattern: self.pattern,
        }
    }
}

impl Default for HttpRuleConfig {
    fn default() -> Self {
        Self {
            effect: RuleEffect::Deny,
            method: HttpMethodConfig::Get,
            match_kind: MatchKindConfig::Literal,
            pattern: String::new(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
/// Native host command configuration.
///
/// Native commands are disabled by default. A non-empty command list enables
/// native execution and acts as an allow-list for higher-level native command
/// policy checks.
pub struct NativeConfig {
    /// Allowed native program names.
    pub commands: Vec<String>,
}

impl NativeConfig {
    fn into_policy(self) -> NativeCommandPolicy {
        let enabled = !self.commands.is_empty();
        let allowed_commands = self
            .commands
            .into_iter()
            .map(|program| NativeCommandRule { program })
            .collect();
        NativeCommandPolicy {
            enabled,
            allowed_commands,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
/// Logging configuration.
pub struct LoggingConfig {
    /// Minimum logging level requested by the user.
    pub level: LoggingLevelConfig,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: LoggingLevelConfig::Info,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
/// Shared allow/deny rule effect.
pub enum RuleEffect {
    /// Permit matching access.
    Allow,
    /// Reject matching access.
    Deny,
}

impl From<RuleEffect> for WasmHttpRuleEffect {
    fn from(effect: RuleEffect) -> Self {
        match effect {
            RuleEffect::Allow => Self::Allow,
            RuleEffect::Deny => Self::Deny,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
/// User-facing match strategy.
pub enum MatchKindConfig {
    /// Exact string match.
    Literal,
    /// Regular expression match.
    Regex,
    /// Glob-like wildcard match using `*`, `**`, and `?`.
    Wildcard,
}

impl From<MatchKindConfig> for MatchPatternKind {
    fn from(kind: MatchKindConfig) -> Self {
        match kind {
            MatchKindConfig::Literal => Self::Literal,
            MatchKindConfig::Regex => Self::Regex,
            MatchKindConfig::Wildcard => Self::Wildcard,
        }
    }
}

impl From<MatchKindConfig> for WasmHttpMatchKind {
    fn from(kind: MatchKindConfig) -> Self {
        match kind {
            MatchKindConfig::Literal => Self::Literal,
            MatchKindConfig::Regex => Self::Regex,
            MatchKindConfig::Wildcard => Self::Wildcard,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
/// User-facing filesystem rule base.
pub enum FilesystemRuleBaseConfig {
    /// Match paths relative to configured filesystem workdirs.
    Workdir,
    /// Match normalized absolute paths.
    Absolute,
}

impl From<FilesystemRuleBaseConfig> for WasmFilesystemRuleBase {
    fn from(base: FilesystemRuleBaseConfig) -> Self {
        match base {
            FilesystemRuleBaseConfig::Workdir => Self::Workdir,
            FilesystemRuleBaseConfig::Absolute => Self::Absolute,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
/// User-facing filesystem permission token.
pub enum FilesystemPermissionConfig {
    /// Read access.
    Read,
    /// Write access.
    Write,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
/// User-facing HTTP method.
pub enum HttpMethodConfig {
    /// HTTP GET.
    Get,
    /// HTTP POST.
    Post,
    /// HTTP PUT.
    Put,
    /// HTTP PATCH.
    Patch,
    /// HTTP DELETE.
    Delete,
    /// HTTP HEAD.
    Head,
    /// HTTP OPTIONS.
    Options,
}

impl From<HttpMethodConfig> for WasmHttpMethod {
    fn from(method: HttpMethodConfig) -> Self {
        match method {
            HttpMethodConfig::Get => Self::Get,
            HttpMethodConfig::Post => Self::Post,
            HttpMethodConfig::Put => Self::Put,
            HttpMethodConfig::Patch => Self::Patch,
            HttpMethodConfig::Delete => Self::Delete,
            HttpMethodConfig::Head => Self::Head,
            HttpMethodConfig::Options => Self::Options,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
/// User-facing logging level.
pub enum LoggingLevelConfig {
    /// Error logs only.
    Error,
    /// Warning and error logs.
    Warn,
    /// Informational logs and above.
    Info,
    /// Debug logs and above.
    Debug,
    /// Trace logs and above.
    Trace,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// Loads Jux config from the user home directory.
///
/// The loader accepts a home directory rather than reading the process
/// environment directly. This keeps tests deterministic and lets higher-level
/// application code decide how to discover the real user home.
pub struct JuxConfigLoader {
    home_dir: PathBuf,
}

impl JuxConfigLoader {
    /// Creates a loader rooted at one user home directory.
    #[must_use]
    pub fn new(home_dir: impl Into<PathBuf>) -> Self {
        Self {
            home_dir: home_dir.into(),
        }
    }

    /// Loads the first supported user config file, or returns defaults.
    ///
    /// No project-local config is read by this method.
    pub fn load(&self) -> Result<JuxConfig, ConfigError> {
        let Some(path) = self.find_config_path() else {
            return Ok(JuxConfig::default());
        };
        let content = fs::read_to_string(&path)
            .map_err(|error| ConfigError::new(format!("failed to read config file: {error}")))?;
        match path.extension().and_then(|extension| extension.to_str()) {
            Some("yaml" | "yml") => JuxConfig::from_yaml_str(&content),
            Some("json" | "jsonc") => JuxConfig::from_jsonc_str(&content),
            _ => Err(ConfigError::new("unsupported config file extension")),
        }
    }

    fn find_config_path(&self) -> Option<PathBuf> {
        CONFIG_RELATIVE_PATHS
            .iter()
            .map(|relative_path| self.home_dir.join(relative_path))
            .find(|path| path.is_file())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// Configuration error shown to callers.
///
/// Error messages are intended to be surfaced to users, so they avoid exposing
/// internal implementation details.
pub struct ConfigError {
    message: String,
}

impl ConfigError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl Display for ConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.message)
    }
}

impl Error for ConfigError {}

fn merge_value(base: &mut Value, overlay: Value) {
    match (base, overlay) {
        (Value::Object(base_map), Value::Object(overlay_map)) => {
            merge_object(base_map, overlay_map);
        }
        (base_value, overlay_value) => {
            *base_value = overlay_value;
        }
    }
}

fn merge_object(base: &mut Map<String, Value>, overlay: Map<String, Value>) {
    for (key, value) in overlay {
        match base.get_mut(&key) {
            Some(base_value) => merge_value(base_value, value),
            None => {
                base.insert(key, value);
            }
        }
    }
}

fn reject_null_values(value: &Value, path: &str) -> Result<(), ConfigError> {
    match value {
        Value::Null => Err(ConfigError::new(format!(
            "null values are not supported at {path}"
        ))),
        Value::Array(items) => reject_null_array(items, path),
        Value::Object(entries) => reject_null_object(entries, path),
        _ => Ok(()),
    }
}

fn reject_null_array(items: &[Value], path: &str) -> Result<(), ConfigError> {
    for (index, value) in items.iter().enumerate() {
        reject_null_values(value, &format!("{path}[{index}]"))?;
    }
    Ok(())
}

fn reject_null_object(entries: &Map<String, Value>, path: &str) -> Result<(), ConfigError> {
    for (key, value) in entries {
        reject_null_values(value, &format!("{path}.{key}"))?;
    }
    Ok(())
}

fn strip_jsonc_comments(input: &str) -> Result<String, ConfigError> {
    let mut output = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    let mut in_string = false;
    let mut escaped = false;
    while let Some(char) = chars.next() {
        strip_jsonc_char(char, &mut chars, &mut output, &mut in_string, &mut escaped)?;
    }
    Ok(output)
}

fn strip_jsonc_char<I>(
    char: char,
    chars: &mut std::iter::Peekable<I>,
    output: &mut String,
    in_string: &mut bool,
    escaped: &mut bool,
) -> Result<(), ConfigError>
where
    I: Iterator<Item = char>,
{
    if *in_string {
        push_json_string_char(char, output, in_string, escaped);
        return Ok(());
    }
    match (char, chars.peek()) {
        ('"', _) => {
            *in_string = true;
            output.push(char);
        }
        ('/', Some('/')) => consume_line_comment(chars, output),
        ('/', Some('*')) => consume_block_comment(chars)?,
        _ => output.push(char),
    }
    Ok(())
}

fn push_json_string_char(
    char: char,
    output: &mut String,
    in_string: &mut bool,
    escaped: &mut bool,
) {
    output.push(char);
    if *escaped {
        *escaped = false;
    } else if char == '\\' {
        *escaped = true;
    } else if char == '"' {
        *in_string = false;
    }
}

fn consume_line_comment<I>(chars: &mut std::iter::Peekable<I>, output: &mut String)
where
    I: Iterator<Item = char>,
{
    chars.next();
    for char in chars.by_ref() {
        if char == '\n' {
            output.push('\n');
            break;
        }
    }
}

fn consume_block_comment<I>(chars: &mut std::iter::Peekable<I>) -> Result<(), ConfigError>
where
    I: Iterator<Item = char>,
{
    chars.next();
    let mut previous = '\0';
    for char in chars.by_ref() {
        if previous == '*' && char == '/' {
            return Ok(());
        }
        previous = char;
    }
    Err(ConfigError::new("unterminated JSONC block comment"))
}

fn filesystem_permissions(read: bool, write: bool) -> WasmFilesystemPermissions {
    WasmFilesystemPermissions { read, write }
}

fn builtin_wasm_packages() -> Vec<WasmPackageRule> {
    vec![WasmPackageRule {
        package: "wasmer/coreutils".to_owned(),
        version: Some("1.0.25".to_owned()),
        source: WasmPackageSource::Builtin,
    }]
}
