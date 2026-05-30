use serde::{Deserialize, Serialize};
use std::fmt::{self, Display};
use uuid::Uuid;

macro_rules! id_type {
    ($name:ident, $prefix:literal) => {
        #[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
        pub struct $name(String);

        impl $name {
            #[must_use]
            pub fn new() -> Self {
                Self(format!("{}-{}", $prefix, Uuid::new_v4()))
            }

            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(&self.0)
            }
        }
    };
}

id_type!(WorkspaceId, "workspace");
id_type!(SessionId, "session");
id_type!(RunId, "run");
id_type!(TurnId, "turn");
id_type!(PlanId, "plan");
id_type!(PlanItemId, "plan-item");
id_type!(StepId, "step");
id_type!(ArtifactId, "artifact");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_use_stable_type_prefixes() {
        assert!(WorkspaceId::new().as_str().starts_with("workspace-"));
        assert!(SessionId::new().as_str().starts_with("session-"));
        assert!(RunId::new().as_str().starts_with("run-"));
        assert!(TurnId::new().as_str().starts_with("turn-"));
        assert!(PlanId::new().as_str().starts_with("plan-"));
        assert!(PlanItemId::new().as_str().starts_with("plan-item-"));
        assert!(StepId::new().as_str().starts_with("step-"));
        assert!(ArtifactId::new().as_str().starts_with("artifact-"));
    }
}
