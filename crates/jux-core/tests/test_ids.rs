use jux_core::{RunId, SessionId, StepId, WorkspaceId};

#[test]
fn ids_derive_parent_ids_from_hierarchical_segments() {
    let workspace_id = WorkspaceId::from("8f3a".to_owned());
    let session_id = SessionId::new(&workspace_id, 1);
    let run_id = RunId::new(&session_id, 1);
    let step_id = StepId::new(&run_id, 1);

    assert_eq!(session_id.to_string(), "8f3a-0001");
    assert_eq!(run_id.to_string(), "8f3a-0001-000001");
    assert_eq!(step_id.to_string(), "8f3a-0001-000001-000001");
    assert_eq!(step_id.run_id(), run_id);
    assert_eq!(step_id.session_id(), session_id);
    assert_eq!(step_id.workspace_id(), workspace_id);
}
