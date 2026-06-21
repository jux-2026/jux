//! WASM filesystem policy.
//!
//! Filesystem access is expressed as an ordered list of rules. Each rule
//! chooses a path base, a `MatchPattern`, and read/write permissions. The first
//! matching rule decides the request, and requests with no matching rule are
//! denied.

use crate::{MatchPattern, MatchPatternKind};
use std::path::{Component, Path, PathBuf};

#[derive(Clone, Debug, Eq, PartialEq)]
/// Filesystem policy for WASM execution.
pub struct WasmFilesystemPolicy {
    pub workdirs: Vec<PathBuf>,
    pub rules: Vec<WasmFilesystemRule>,
}

impl WasmFilesystemPolicy {
    #[must_use]
    pub fn disabled() -> Self {
        Self {
            workdirs: Vec::new(),
            rules: Vec::new(),
        }
    }

    #[must_use]
    pub fn new(workdirs: Vec<PathBuf>, rules: Vec<WasmFilesystemRule>) -> Self {
        Self { workdirs, rules }
    }

    #[must_use]
    pub fn read_write_workdir(workdir: impl Into<PathBuf>) -> Self {
        Self::new(
            vec![workdir.into()],
            vec![WasmFilesystemRule::allow_read_write("**")],
        )
    }

    #[must_use]
    pub fn has_rules(&self) -> bool {
        !self.rules.is_empty()
    }

    pub fn decide_path_access(
        &self,
        path: impl AsRef<Path>,
        access: WasmFilesystemAccess,
    ) -> Result<WasmFilesystemDecision, String> {
        let requested_paths = self.requested_paths(path.as_ref());
        for rule in &self.rules {
            if rule.matches_any(&requested_paths, &self.workdirs)? {
                return Ok(WasmFilesystemDecision::from(
                    rule.permissions.allows(access),
                ));
            }
        }
        Ok(WasmFilesystemDecision::Deny)
    }

    fn requested_paths(&self, path: &Path) -> Vec<PathBuf> {
        if path.is_absolute() {
            return vec![normalize_path(path)];
        }
        self.workdirs
            .iter()
            .map(|workdir| normalize_path(&workdir.join(path)))
            .collect()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// Ordered filesystem access rule for WASM execution.
pub struct WasmFilesystemRule {
    pub base: WasmFilesystemRuleBase,
    pub pattern: MatchPattern,
    pub permissions: WasmFilesystemPermissions,
}

impl WasmFilesystemRule {
    #[must_use]
    pub fn new(
        base: WasmFilesystemRuleBase,
        pattern: MatchPattern,
        permissions: WasmFilesystemPermissions,
    ) -> Self {
        Self {
            base,
            pattern,
            permissions,
        }
    }

    #[must_use]
    pub fn deny(pattern: impl Into<String>) -> Self {
        Self::workdir_wildcard(pattern, WasmFilesystemPermissions::deny())
    }

    #[must_use]
    pub fn allow_read(pattern: impl Into<String>) -> Self {
        Self::workdir_wildcard(pattern, WasmFilesystemPermissions::read())
    }

    #[must_use]
    pub fn allow_write(pattern: impl Into<String>) -> Self {
        Self::workdir_wildcard(pattern, WasmFilesystemPermissions::write())
    }

    #[must_use]
    pub fn allow_read_write(pattern: impl Into<String>) -> Self {
        Self::workdir_wildcard(pattern, WasmFilesystemPermissions::read_write())
    }

    #[must_use]
    pub fn deny_absolute(pattern: impl Into<String>) -> Self {
        Self::absolute_wildcard(pattern, WasmFilesystemPermissions::deny())
    }

    #[must_use]
    pub fn allow_read_absolute(pattern: impl Into<String>) -> Self {
        Self::absolute_wildcard(pattern, WasmFilesystemPermissions::read())
    }

    #[must_use]
    pub fn allow_write_absolute(pattern: impl Into<String>) -> Self {
        Self::absolute_wildcard(pattern, WasmFilesystemPermissions::write())
    }

    #[must_use]
    pub fn allow_read_write_absolute(pattern: impl Into<String>) -> Self {
        Self::absolute_wildcard(pattern, WasmFilesystemPermissions::read_write())
    }

    #[must_use]
    pub fn workdir_literal(
        pattern: impl Into<String>,
        permissions: WasmFilesystemPermissions,
    ) -> Self {
        Self::with_pattern(
            WasmFilesystemRuleBase::Workdir,
            MatchPatternKind::Literal,
            pattern,
            permissions,
        )
    }

    #[must_use]
    pub fn workdir_regex(
        pattern: impl Into<String>,
        permissions: WasmFilesystemPermissions,
    ) -> Self {
        Self::with_pattern(
            WasmFilesystemRuleBase::Workdir,
            MatchPatternKind::Regex,
            pattern,
            permissions,
        )
    }

    #[must_use]
    pub fn absolute_literal(
        pattern: impl Into<String>,
        permissions: WasmFilesystemPermissions,
    ) -> Self {
        Self::with_pattern(
            WasmFilesystemRuleBase::Absolute,
            MatchPatternKind::Literal,
            pattern,
            permissions,
        )
    }

    #[must_use]
    pub fn absolute_regex(
        pattern: impl Into<String>,
        permissions: WasmFilesystemPermissions,
    ) -> Self {
        Self::with_pattern(
            WasmFilesystemRuleBase::Absolute,
            MatchPatternKind::Regex,
            pattern,
            permissions,
        )
    }

    fn workdir_wildcard(
        pattern: impl Into<String>,
        permissions: WasmFilesystemPermissions,
    ) -> Self {
        Self::with_pattern(
            WasmFilesystemRuleBase::Workdir,
            MatchPatternKind::Wildcard,
            pattern,
            permissions,
        )
    }

    fn absolute_wildcard(
        pattern: impl Into<String>,
        permissions: WasmFilesystemPermissions,
    ) -> Self {
        Self::with_pattern(
            WasmFilesystemRuleBase::Absolute,
            MatchPatternKind::Wildcard,
            pattern,
            permissions,
        )
    }

    fn with_pattern(
        base: WasmFilesystemRuleBase,
        kind: MatchPatternKind,
        pattern: impl Into<String>,
        permissions: WasmFilesystemPermissions,
    ) -> Self {
        Self::new(base, MatchPattern::new(kind, pattern), permissions)
    }

    fn matches_any(&self, paths: &[PathBuf], workdirs: &[PathBuf]) -> Result<bool, String> {
        for path in paths {
            if self.matches_path(path, workdirs)? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn matches_path(&self, path: &Path, workdirs: &[PathBuf]) -> Result<bool, String> {
        match self.base {
            WasmFilesystemRuleBase::Absolute => self.pattern.matches(&path_to_match_string(path)),
            WasmFilesystemRuleBase::Workdir => self.matches_workdir_path(path, workdirs),
        }
    }

    fn matches_workdir_path(&self, path: &Path, workdirs: &[PathBuf]) -> Result<bool, String> {
        for workdir in workdirs {
            let workdir = normalize_path(workdir);
            if let Ok(relative_path) = path.strip_prefix(&workdir)
                && self.pattern.matches(&path_to_match_string(relative_path))?
            {
                return Ok(true);
            }
        }
        Ok(false)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// Path base used by a filesystem rule.
pub enum WasmFilesystemRuleBase {
    Workdir,
    Absolute,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// Filesystem permissions granted by a matching rule.
pub struct WasmFilesystemPermissions {
    pub read: bool,
    pub write: bool,
}

impl WasmFilesystemPermissions {
    #[must_use]
    pub fn deny() -> Self {
        Self {
            read: false,
            write: false,
        }
    }

    #[must_use]
    pub fn read() -> Self {
        Self {
            read: true,
            write: false,
        }
    }

    #[must_use]
    pub fn write() -> Self {
        Self {
            read: false,
            write: true,
        }
    }

    #[must_use]
    pub fn read_write() -> Self {
        Self {
            read: true,
            write: true,
        }
    }

    #[must_use]
    pub fn allows(&self, access: WasmFilesystemAccess) -> bool {
        match access {
            WasmFilesystemAccess::Read => self.read,
            WasmFilesystemAccess::Write => self.write,
            WasmFilesystemAccess::ReadWrite => self.read && self.write,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// Filesystem access requested by one operation.
pub enum WasmFilesystemAccess {
    Read,
    Write,
    ReadWrite,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// Result of a filesystem policy decision.
pub enum WasmFilesystemDecision {
    Allow,
    Deny,
}

impl From<bool> for WasmFilesystemDecision {
    fn from(allowed: bool) -> Self {
        if allowed { Self::Allow } else { Self::Deny }
    }
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Normal(part) => normalized.push(part),
            Component::RootDir | Component::Prefix(_) => normalized.push(component.as_os_str()),
        }
    }
    normalized
}

fn path_to_match_string(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}
