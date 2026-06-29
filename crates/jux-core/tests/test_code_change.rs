use jux_core::{
    CodeChangePlan, CodeChangeProposal, CodeChangeReview, PolicyDecision, ProposedFileContent,
    ReviewStatus, RiskLevel,
};

#[test]
fn code_change_proposal_prepares_plan_files_and_diff() {
    let workspace = temp_workspace_root();
    std::fs::create_dir_all(workspace.join("src")).expect("source directory exists");
    std::fs::write(workspace.join("src/lib.rs"), "pub fn old() {}\n")
        .expect("source file is written");
    let plan = CodeChangePlan::new("Rename the function", vec!["Update src/lib.rs".to_owned()]);

    let proposal = CodeChangeProposal::prepare(
        &workspace,
        plan.clone(),
        vec![ProposedFileContent::new("src/lib.rs", "pub fn new() {}\n")],
    )
    .expect("proposal is prepared");

    assert_eq!(proposal.plan, plan);
    assert_eq!(proposal.files.len(), 1);
    assert_eq!(proposal.files[0].path.as_str(), "src/lib.rs");
    assert_eq!(proposal.policy, PolicyDecision::Confirm);
    assert!(proposal.files[0].diff.contains("-pub fn old() {}"));
    assert!(proposal.files[0].diff.contains("+pub fn new() {}"));
    assert_eq!(
        std::fs::read_to_string(workspace.join("src/lib.rs")).expect("source file loads"),
        "pub fn old() {}\n"
    );
}

#[test]
fn approved_code_change_applies_new_file_content() {
    let workspace = temp_workspace_root();
    std::fs::write(workspace.join("README.md"), "old\n").expect("source file is written");
    let proposal = CodeChangeProposal::prepare(
        &workspace,
        CodeChangePlan::new("Update README", vec!["Replace content".to_owned()]),
        vec![ProposedFileContent::new("README.md", "new\n")],
    )
    .expect("proposal is prepared");
    let mut review = CodeChangeReview::new(proposal);

    review.approve().expect("review is approved");
    review.apply(&workspace).expect("review is applied");

    assert_eq!(review.status, ReviewStatus::Applied);
    assert_eq!(
        std::fs::read_to_string(workspace.join("README.md")).expect("source file loads"),
        "new\n"
    );
}

#[test]
fn code_change_review_can_be_rejected_or_returned_for_changes() {
    let workspace = temp_workspace_root();
    let proposal = CodeChangeProposal::prepare(
        &workspace,
        CodeChangePlan::new("Create README", vec!["Write README".to_owned()]),
        vec![ProposedFileContent::new("README.md", "content\n")],
    )
    .expect("proposal is prepared");
    let mut rejected = CodeChangeReview::new(proposal.clone());
    let mut changes_requested = CodeChangeReview::new(proposal);

    rejected.reject().expect("review is rejected");
    changes_requested
        .request_changes("Add an example".to_owned())
        .expect("changes are requested");

    assert_eq!(rejected.status, ReviewStatus::Rejected);
    assert_eq!(
        changes_requested.status,
        ReviewStatus::ChangesRequested {
            feedback: "Add an example".to_owned(),
        }
    );
    assert!(!workspace.join("README.md").exists());
}

#[test]
fn code_change_apply_detects_files_changed_after_proposal() {
    let workspace = temp_workspace_root();
    std::fs::write(workspace.join("README.md"), "original\n").expect("source file is written");
    let proposal = CodeChangeProposal::prepare(
        &workspace,
        CodeChangePlan::new("Update README", vec!["Replace content".to_owned()]),
        vec![ProposedFileContent::new("README.md", "proposed\n")],
    )
    .expect("proposal is prepared");
    let mut review = CodeChangeReview::new(proposal);
    review.approve().expect("review is approved");
    std::fs::write(workspace.join("README.md"), "changed externally\n")
        .expect("source file changes");

    let error = review.apply(&workspace).expect_err("apply conflicts");

    assert_eq!(
        review.status,
        ReviewStatus::Conflict {
            paths: vec!["README.md".to_owned()],
        }
    );
    assert!(error.to_string().contains("README.md"));
    assert_eq!(
        std::fs::read_to_string(workspace.join("README.md")).expect("source file loads"),
        "changed externally\n"
    );
}

#[test]
fn code_change_proposal_denies_sensitive_paths_with_a_warning() {
    let workspace = temp_workspace_root();
    std::fs::write(workspace.join(".env"), "TOKEN=old\n").expect("env file is written");

    let proposal = CodeChangeProposal::prepare(
        &workspace,
        CodeChangePlan::new("Update token", vec!["Modify .env".to_owned()]),
        vec![ProposedFileContent::new(".env", "TOKEN=new\n")],
    )
    .expect("proposal is prepared");

    assert_eq!(proposal.policy, PolicyDecision::Deny);
    assert_eq!(proposal.warnings.len(), 1);
    assert_eq!(proposal.warnings[0].level, RiskLevel::High);
    assert_eq!(proposal.warnings[0].path.as_str(), ".env");
    let mut review = CodeChangeReview::new(proposal);
    assert!(review.approve().is_err());
}

fn temp_workspace_root() -> std::path::PathBuf {
    let root = std::env::temp_dir().join(format!("jux-code-change-test-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).expect("temp workspace root is created");
    root
}
