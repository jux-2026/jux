//! Core library for the Jux agent runtime.

/// Returns the current workspace package version.
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[cfg(test)]
mod tests;
