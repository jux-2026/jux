//! Core library for the Jux agent runtime.

mod ids;
mod model;
mod store;
mod time;

pub use ids::{ArtifactId, PlanId, PlanItemId, RunId, SessionId, StepId, TurnId, WorkspaceId};
pub use model::{
    Artifact, ArtifactKind, Plan, PlanItem, PlanItemStatus, Run, RunStatus, Session, Step,
    StepKind, Turn, TurnKind, Workspace,
};
pub use store::{FileWorkspaceStore, StoreError, WorkspaceStore};

/// Returns the current workspace package version.
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exposes_package_version() {
        assert_eq!(version(), "0.1.0");
    }
}
