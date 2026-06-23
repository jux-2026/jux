use jux_core::{InstructionDocument, InstructionResolver, InstructionScope};
use std::fs;
use std::path::PathBuf;

#[test]
fn instruction_resolver_loads_user_and_project_documents_in_priority_order() {
    let home = unique_temp_dir("jux-instructions-home");
    let workspace = unique_temp_dir("jux-instructions-workspace");
    fs::create_dir_all(home.join(".jux")).expect("user instruction directory is created");
    fs::create_dir_all(workspace.join(".jux")).expect("project instruction directory is created");
    fs::write(home.join(".jux/AGENTS.md"), "Use user defaults.")
        .expect("user instructions are written");
    fs::write(
        workspace.join(".jux/AGENTS.md"),
        "Use project instructions first.",
    )
    .expect("project instructions are written");

    let documents = InstructionResolver::new(home.clone(), workspace.clone())
        .resolve()
        .expect("instructions resolve");

    assert_eq!(
        documents,
        vec![
            InstructionDocument {
                scope: InstructionScope::User,
                path: home.join(".jux/AGENTS.md"),
                content: "Use user defaults.".to_owned(),
            },
            InstructionDocument {
                scope: InstructionScope::Project,
                path: workspace.join(".jux/AGENTS.md"),
                content: "Use project instructions first.".to_owned(),
            },
        ]
    );
    fs::remove_dir_all(home).expect("user temp dir is removed");
    fs::remove_dir_all(workspace).expect("workspace temp dir is removed");
}

#[test]
fn instruction_resolver_prefers_uppercase_agents_file_and_skips_empty_files() {
    let home = unique_temp_dir("jux-instructions-uppercase-home");
    let workspace = unique_temp_dir("jux-instructions-uppercase-workspace");
    fs::create_dir_all(home.join(".jux")).expect("user instruction directory is created");
    fs::create_dir_all(workspace.join(".jux")).expect("project instruction directory is created");
    fs::write(home.join(".jux/agents.md"), "lowercase user")
        .expect("lowercase user file is written");
    fs::write(home.join(".jux/AGENTS.md"), "uppercase user")
        .expect("uppercase user file is written");
    fs::write(workspace.join(".jux/AGENTS.md"), "   \n").expect("empty project file is written");

    let documents = InstructionResolver::new(home.clone(), workspace.clone())
        .resolve()
        .expect("instructions resolve");

    assert_eq!(documents.len(), 1);
    assert_eq!(documents[0].scope, InstructionScope::User);
    assert_eq!(documents[0].content, "uppercase user");
    assert_eq!(documents[0].path, home.join(".jux/AGENTS.md"));
    fs::remove_dir_all(home).expect("user temp dir is removed");
    fs::remove_dir_all(workspace).expect("workspace temp dir is removed");
}

fn unique_temp_dir(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("{name}-{}", uuid::Uuid::new_v4()))
}
