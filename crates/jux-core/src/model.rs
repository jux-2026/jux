use crate::ids::{ArtifactId, PlanId, PlanItemId, RunId, SessionId, StepId, TurnId, WorkspaceId};
use crate::time::now_millis;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Workspace {
    pub id: WorkspaceId,
    pub root: PathBuf,
    pub active_session_id: SessionId,
    pub created_at: u128,
    pub updated_at: u128,
}

impl Workspace {
    #[must_use]
    pub fn new(root: PathBuf, active_session_id: SessionId) -> Self {
        let now = now_millis();

        Self {
            id: WorkspaceId::new(),
            root,
            active_session_id,
            created_at: now,
            updated_at: now,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Session {
    pub id: SessionId,
    pub workspace_id: WorkspaceId,
    pub name: Option<String>,
    pub run_ids: Vec<RunId>,
    pub created_at: u128,
    pub updated_at: u128,
}

impl Session {
    #[must_use]
    pub fn new(workspace_id: WorkspaceId, name: Option<String>) -> Self {
        let now = now_millis();

        Self {
            id: SessionId::new(),
            workspace_id,
            name,
            run_ids: Vec::new(),
            created_at: now,
            updated_at: now,
        }
    }

    pub fn add_run(&mut self, run_id: RunId) {
        self.run_ids.push(run_id);
        self.updated_at = now_millis();
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Run {
    pub id: RunId,
    pub session_id: SessionId,
    pub request: String,
    pub status: RunStatus,
    pub turn_ids: Vec<TurnId>,
    pub plan_id: Option<PlanId>,
    pub step_ids: Vec<StepId>,
    pub artifact_ids: Vec<ArtifactId>,
    pub created_at: u128,
    pub updated_at: u128,
}

impl Run {
    #[must_use]
    pub fn new(session_id: SessionId, request: String) -> Self {
        let now = now_millis();

        Self {
            id: RunId::new(),
            session_id,
            request,
            status: RunStatus::Created,
            turn_ids: Vec::new(),
            plan_id: None,
            step_ids: Vec::new(),
            artifact_ids: Vec::new(),
            created_at: now,
            updated_at: now,
        }
    }

    pub fn add_turn(&mut self, turn_id: TurnId) {
        self.turn_ids.push(turn_id);
        self.updated_at = now_millis();
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum RunStatus {
    Created,
    Planning,
    WaitingForInput,
    PatchReady,
    Applied,
    Failed,
    Canceled,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Turn {
    pub id: TurnId,
    pub run_id: RunId,
    pub kind: TurnKind,
    pub content: String,
    pub created_at: u128,
}

impl Turn {
    #[must_use]
    pub fn user_request(run_id: RunId, content: String) -> Self {
        Self {
            id: TurnId::new(),
            run_id,
            kind: TurnKind::UserRequest,
            content,
            created_at: now_millis(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum TurnKind {
    UserRequest,
    UserClarification,
    UserConfirmation,
    UserCorrection,
    AssistantMessage,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Plan {
    pub id: PlanId,
    pub run_id: RunId,
    pub item_ids: Vec<PlanItemId>,
    pub created_at: u128,
    pub updated_at: u128,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PlanItem {
    pub id: PlanItemId,
    pub plan_id: PlanId,
    pub description: String,
    pub status: PlanItemStatus,
    pub created_at: u128,
    pub updated_at: u128,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum PlanItemStatus {
    Pending,
    InProgress,
    Completed,
    Failed,
    Skipped,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Step {
    pub id: StepId,
    pub run_id: RunId,
    pub kind: StepKind,
    pub summary: String,
    pub created_at: u128,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum StepKind {
    ReadFile,
    SearchWorkspace,
    PolicyDecision,
    GeneratePlan,
    PreparePatch,
    ApplyPatch,
    RecordDiff,
    WaitForInput,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Artifact {
    pub id: ArtifactId,
    pub run_id: RunId,
    pub kind: ArtifactKind,
    pub path: Option<PathBuf>,
    pub created_at: u128,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum ArtifactKind {
    Patch,
    Diff,
    RequirementsDocument,
    Roadmap,
    AuditSummary,
    Other,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serializes_core_run_related_models() {
        let run_id = RunId::new();
        let plan_id = PlanId::new();
        let plan_item_id = PlanItemId::new();
        let step_id = StepId::new();
        let artifact_id = ArtifactId::new();
        let now = now_millis();

        let plan = Plan {
            id: plan_id.clone(),
            run_id: run_id.clone(),
            item_ids: vec![plan_item_id.clone()],
            created_at: now,
            updated_at: now,
        };
        let plan_item = PlanItem {
            id: plan_item_id,
            plan_id,
            description: "Inspect workspace".to_owned(),
            status: PlanItemStatus::Pending,
            created_at: now,
            updated_at: now,
        };
        let step = Step {
            id: step_id,
            run_id: run_id.clone(),
            kind: StepKind::SearchWorkspace,
            summary: "Searched workspace files".to_owned(),
            created_at: now,
        };
        let artifact = Artifact {
            id: artifact_id,
            run_id,
            kind: ArtifactKind::Diff,
            path: Some(PathBuf::from("artifacts/diff.patch")),
            created_at: now,
        };

        let json =
            serde_json::to_string(&(plan, plan_item, step, artifact)).expect("models serialize");
        let (plan, plan_item, step, artifact): (Plan, PlanItem, Step, Artifact) =
            serde_json::from_str(&json).expect("models deserialize");

        assert_eq!(plan.item_ids, vec![plan_item.id.clone()]);
        assert_eq!(plan_item.status, PlanItemStatus::Pending);
        assert_eq!(step.kind, StepKind::SearchWorkspace);
        assert_eq!(artifact.kind, ArtifactKind::Diff);
    }
}
