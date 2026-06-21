//! Shared match pattern logic.
//!
//! Policy modules use match patterns to evaluate controlled resources such as
//! HTTP URLs and filesystem paths. Matching is always against the full input
//! string, and wildcard matching treats `/` as the segment separator. Callers
//! should normalize domain-specific values before evaluating a pattern.
//!
//! Matching modes:
//!
//! - `Literal` matches only when the pattern and input are exactly equal.
//! - `Regex` evaluates the pattern as a regular expression against the full
//!   input string. The implementation wraps the pattern as `^(?:pattern)$`, so
//!   callers do not need to add anchors for whole-string matching.
//! - `Wildcard` uses a small glob-like syntax shared by policy modules:
//!   - `*` matches zero or more characters inside one `/`-delimited segment.
//!   - `**` matches zero or more characters across segment boundaries.
//!   - `?` matches exactly one non-`/` character.
//!   - `\` escapes the next character so wildcard tokens can be matched
//!     literally.
//!
//! Examples:
//!
//! - `/workspace/*.rs` matches `/workspace/main.rs`, but not
//!   `/workspace/src/main.rs`.
//! - `/workspace/**/*.rs` matches both `/workspace/main.rs` and
//!   `/workspace/src/main.rs`.
//! - `/workspace/file-?.txt` matches `/workspace/file-a.txt`, but not
//!   `/workspace/file-ab.txt`.
//! - `/workspace/\*.txt` matches the literal path `/workspace/*.txt`.

use regex::Regex;

#[derive(Clone, Debug, Eq, PartialEq)]
/// Reusable resource pattern for ordered match rules.
pub struct MatchPattern {
    pub kind: MatchPatternKind,
    pub value: String,
}

impl MatchPattern {
    #[must_use]
    pub fn new(kind: MatchPatternKind, value: impl Into<String>) -> Self {
        Self {
            kind,
            value: value.into(),
        }
    }

    pub fn matches(&self, input: &str) -> Result<bool, String> {
        match self.kind {
            MatchPatternKind::Literal => Ok(self.value == input),
            MatchPatternKind::Wildcard => wildcard_matches(&self.value, input),
            MatchPatternKind::Regex => regex_matches(&self.value, input),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// Matching strategy used by `MatchPattern`.
pub enum MatchPatternKind {
    Literal,
    Wildcard,
    Regex,
}

fn regex_matches(pattern: &str, input: &str) -> Result<bool, String> {
    Regex::new(&format!("^(?:{pattern})$"))
        .map(|regex| regex.is_match(input))
        .map_err(|error| format!("invalid match regex pattern: {error}"))
}

fn wildcard_matches(pattern: &str, input: &str) -> Result<bool, String> {
    let regex = wildcard_to_regex(pattern)?;
    Regex::new(&regex)
        .map(|regex| regex.is_match(input))
        .map_err(|error| format!("invalid generated match wildcard pattern: {error}"))
}

fn wildcard_to_regex(pattern: &str) -> Result<String, String> {
    let mut regex = String::from("^");
    let mut chars = pattern.chars().peekable();

    while let Some(char) = chars.next() {
        match char {
            '*' if chars.peek() == Some(&'*') => {
                chars.next();
                if chars.peek() == Some(&'/') {
                    chars.next();
                    regex.push_str("(?:.*/)?");
                } else {
                    regex.push_str(".*");
                }
            }
            '*' => regex.push_str("[^/]*"),
            '?' => regex.push_str("[^/]"),
            '\\' => push_escaped_char(&mut regex, chars.next())?,
            other => regex.push_str(&regex::escape(&other.to_string())),
        }
    }

    regex.push('$');
    Ok(regex)
}

fn push_escaped_char(regex: &mut String, char: Option<char>) -> Result<(), String> {
    let Some(char) = char else {
        return Err("match wildcard pattern cannot end with an escape".to_owned());
    };
    regex.push_str(&regex::escape(&char.to_string()));
    Ok(())
}
