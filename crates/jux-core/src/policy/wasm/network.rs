//! WASM HTTP network policy.
//!
//! HTTP access is expressed as one ordered list of rules. Each rule declares an
//! action (`Allow` or `Deny`), one HTTP method, one URL matching strategy, and
//! one pattern. Request evaluation is intentionally simple:
//!
//! 1. Iterate over `http_rules` from first to last.
//! 2. The first rule whose method and URL both match decides the request.
//! 3. If no rule matches, the request is denied.
//!
//! This makes rule order the only priority mechanism. A broad allow rule can be
//! placed after narrow deny rules, or a broad deny rule can be placed after
//! narrow allow rules, depending on the desired policy.

use regex::Regex;

#[derive(Clone, Debug, Eq, PartialEq)]
/// Network policy for WASM execution.
///
/// An empty `http_rules` list means HTTP access is disabled. A non-empty list
/// enables the WASM HTTP client capability, but individual requests must still
/// pass `decide_http_request`.
pub struct WasmNetworkPolicy {
    pub http_rules: Vec<WasmHttpRule>,
}

impl WasmNetworkPolicy {
    #[must_use]
    pub fn http_client_enabled(&self) -> bool {
        !self.http_rules.is_empty()
    }

    /// Decides whether one HTTP request is allowed.
    ///
    /// Rules are evaluated in list order. The first matching rule returns its
    /// effect as the decision. If every rule misses, the default decision is
    /// `Deny`.
    pub fn decide_http_request(
        &self,
        method: WasmHttpMethod,
        url: &str,
    ) -> Result<WasmHttpDecision, String> {
        for rule in &self.http_rules {
            if rule.matches_request(method, url)? {
                return Ok(WasmHttpDecision::from(rule.effect));
            }
        }
        Ok(WasmHttpDecision::Deny)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// Ordered HTTP request rule for WASM execution.
///
/// A rule matches only when both the HTTP method and URL pattern match. The rule
/// `effect` is applied only after both checks pass.
pub struct WasmHttpRule {
    pub effect: WasmHttpRuleEffect,
    pub method: WasmHttpMethod,
    pub match_kind: WasmHttpMatchKind,
    pub pattern: String,
}

impl WasmHttpRule {
    #[must_use]
    pub fn matches_method(&self, method: WasmHttpMethod) -> bool {
        self.method == method
    }

    /// Returns whether this rule matches one HTTP request.
    ///
    /// Method mismatch returns `Ok(false)` without evaluating the URL pattern.
    /// Invalid regex patterns return an error so callers can reject malformed
    /// policies instead of silently allowing or denying traffic.
    pub fn matches_request(&self, method: WasmHttpMethod, url: &str) -> Result<bool, String> {
        if !self.matches_method(method) {
            return Ok(false);
        }
        self.matches_url(url)
    }

    fn matches_url(&self, url: &str) -> Result<bool, String> {
        match self.match_kind {
            WasmHttpMatchKind::Literal => Ok(self.pattern == url),
            WasmHttpMatchKind::Regex => Regex::new(&self.pattern)
                .map(|regex| regex.is_match(url))
                .map_err(|error| format!("invalid HTTP URL regex pattern: {error}")),
            WasmHttpMatchKind::Wildcard => wildcard_matches(&self.pattern, url),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// Result of an HTTP request policy decision.
pub enum WasmHttpDecision {
    Allow,
    Deny,
}

impl From<WasmHttpRuleEffect> for WasmHttpDecision {
    fn from(effect: WasmHttpRuleEffect) -> Self {
        match effect {
            WasmHttpRuleEffect::Allow => Self::Allow,
            WasmHttpRuleEffect::Deny => Self::Deny,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// Action applied when an HTTP rule matches.
pub enum WasmHttpRuleEffect {
    Allow,
    Deny,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// URL matching strategy for a WASM HTTP rule.
pub enum WasmHttpMatchKind {
    /// Exact string match against the full request URL.
    ///
    /// Example pattern: `https://api.example.com/v1/users`.
    Literal,
    /// Regular expression match against the full request URL.
    ///
    /// Example pattern: `^https://api\\.example\\.com(:443)?/v1/.*$`.
    Regex,
    /// Star wildcard match against the full request URL.
    ///
    /// `*` matches any sequence of characters. All other characters are treated
    /// literally.
    ///
    /// Example pattern: `https://*.example.com/v1/*`.
    Wildcard,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// HTTP method allowed by a WASM HTTP rule.
pub enum WasmHttpMethod {
    Get,
    Post,
    Put,
    Patch,
    Delete,
    Head,
    Options,
}

fn wildcard_matches(pattern: &str, value: &str) -> Result<bool, String> {
    let pattern = pattern
        .split('*')
        .map(regex::escape)
        .collect::<Vec<_>>()
        .join(".*");
    Regex::new(&format!("^{pattern}$"))
        .map(|regex| regex.is_match(value))
        .map_err(|error| format!("invalid generated HTTP wildcard pattern: {error}"))
}
