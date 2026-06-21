//! Small shared utilities.
//!
//! This module contains low-level helpers that do not belong to a specific
//! domain module. Utilities should stay small and dependency-light; domain
//! behavior belongs in modules such as `state`, `policy`, or `tools`.

pub(crate) mod time;
