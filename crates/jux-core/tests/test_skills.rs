use jux_core::{
    MAX_SKILL_FILE_BYTES, SkillDefinition, SkillResolver, SkillScope, match_auto_skills,
    render_active_skills, render_skill_index, select_explicit_skills,
};
use std::fs;
use std::path::PathBuf;

#[test]
fn skill_resolver_discovers_user_and_project_skills() {
    let home = unique_temp_dir("jux-skills-home");
    let workspace = unique_temp_dir("jux-skills-workspace");
    write_skill(
        &home,
        "format",
        "Format code",
        "Use this skill to format code.",
    );
    write_skill(
        &workspace,
        "review",
        "Review code",
        "Use this skill to review code.",
    );

    let skills = SkillResolver::new(home.clone(), workspace.clone())
        .resolve()
        .expect("skills resolve");

    assert_eq!(
        skills,
        vec![
            SkillDefinition {
                name: "format".to_owned(),
                description: "Format code".to_owned(),
                content: "Use this skill to format code.".to_owned(),
                scope: SkillScope::User,
                path: home.join(".jux/skills/format/SKILL.md"),
            },
            SkillDefinition {
                name: "review".to_owned(),
                description: "Review code".to_owned(),
                content: "Use this skill to review code.".to_owned(),
                scope: SkillScope::Project,
                path: workspace.join(".jux/skills/review/SKILL.md"),
            },
        ]
    );
    remove_temp_dir(home);
    remove_temp_dir(workspace);
}

#[test]
fn project_skill_overrides_user_skill_with_same_name() {
    let home = unique_temp_dir("jux-skills-override-home");
    let workspace = unique_temp_dir("jux-skills-override-workspace");
    write_skill(&home, "review", "User review", "User review body.");
    write_skill(
        &workspace,
        "review",
        "Project review",
        "Project review body.",
    );

    let skills = SkillResolver::new(home.clone(), workspace.clone())
        .resolve()
        .expect("skills resolve");

    assert_eq!(
        skills,
        vec![SkillDefinition {
            name: "review".to_owned(),
            description: "Project review".to_owned(),
            content: "Project review body.".to_owned(),
            scope: SkillScope::Project,
            path: workspace.join(".jux/skills/review/SKILL.md"),
        }]
    );
    remove_temp_dir(home);
    remove_temp_dir(workspace);
}

#[test]
fn skill_resolver_rejects_missing_description() {
    let home = unique_temp_dir("jux-skills-missing-description-home");
    let workspace = unique_temp_dir("jux-skills-missing-description-workspace");
    write_raw_skill(&home, "review", "---\nname: review\n---\nReview body.");

    let error = SkillResolver::new(home.clone(), workspace.clone())
        .resolve()
        .expect_err("missing description fails");

    assert!(error.to_string().contains("missing description"));
    remove_temp_dir(home);
    remove_temp_dir(workspace);
}

#[test]
fn skill_resolver_rejects_missing_name() {
    let home = unique_temp_dir("jux-skills-missing-name-home");
    let workspace = unique_temp_dir("jux-skills-missing-name-workspace");
    write_raw_skill(
        &home,
        "review",
        "---\ndescription: Review code\n---\nReview body.",
    );

    let error = SkillResolver::new(home.clone(), workspace.clone())
        .resolve()
        .expect_err("missing name fails");

    assert!(error.to_string().contains("missing name"));
    remove_temp_dir(home);
    remove_temp_dir(workspace);
}

#[test]
fn skill_resolver_rejects_directory_name_mismatch() {
    let home = unique_temp_dir("jux-skills-name-mismatch-home");
    let workspace = unique_temp_dir("jux-skills-name-mismatch-workspace");
    write_raw_skill(
        &home,
        "review",
        "---\nname: different\ndescription: Review code\n---\nReview body.",
    );

    let error = SkillResolver::new(home.clone(), workspace.clone())
        .resolve()
        .expect_err("name mismatch fails");

    assert!(error.to_string().contains("does not match directory name"));
    remove_temp_dir(home);
    remove_temp_dir(workspace);
}

#[test]
fn skill_resolver_rejects_empty_skill_file() {
    let home = unique_temp_dir("jux-skills-empty-home");
    let workspace = unique_temp_dir("jux-skills-empty-workspace");
    write_raw_skill(&home, "review", "   \n");

    let error = SkillResolver::new(home.clone(), workspace.clone())
        .resolve()
        .expect_err("empty skill fails");

    assert!(error.to_string().contains("empty skill file"));
    remove_temp_dir(home);
    remove_temp_dir(workspace);
}

#[test]
fn skill_resolver_rejects_oversized_skill_file() {
    let home = unique_temp_dir("jux-skills-oversized-home");
    let workspace = unique_temp_dir("jux-skills-oversized-workspace");
    let body = "x".repeat(MAX_SKILL_FILE_BYTES as usize + 1);
    write_raw_skill(&home, "review", &body);

    let error = SkillResolver::new(home.clone(), workspace.clone())
        .resolve()
        .expect_err("oversized skill fails");

    assert!(
        error
            .to_string()
            .contains("exceeds maximum skill file size")
    );
    remove_temp_dir(home);
    remove_temp_dir(workspace);
}

#[test]
fn skill_index_renders_available_skill_names_and_descriptions() {
    let skills = vec![SkillDefinition {
        name: "review".to_owned(),
        description: "Review code".to_owned(),
        content: "Full review instructions stay out of the index.".to_owned(),
        scope: SkillScope::Project,
        path: "/workspace/.jux/skills/review/SKILL.md".into(),
    }];

    let index = render_skill_index(&skills);

    assert!(index.contains("## Available Skills"));
    assert!(index.contains("- review: Review code"));
    assert!(index.contains("Project skills override user skills with the same name."));
    assert!(!index.contains("Full review instructions stay out of the index."));
}

#[test]
fn explicit_skill_selection_returns_full_skill_content() {
    let skills = test_skills();

    let selected = select_explicit_skills(&skills, &["review".to_owned()])
        .expect("explicit skill selection succeeds");
    let rendered = render_active_skills(&selected);

    assert_eq!(selected.len(), 1);
    assert_eq!(selected[0].name, "review");
    assert!(rendered.contains("## Active Skills"));
    assert!(rendered.contains("### review"));
    assert!(rendered.contains("Full review instructions."));
}

#[test]
fn explicit_skill_selection_reports_missing_skill() {
    let error = select_explicit_skills(&test_skills(), &["missing".to_owned()])
        .expect_err("missing explicit skill fails");

    assert!(error.to_string().contains("skill not found: missing"));
}

#[test]
fn auto_skill_matching_uses_request_name_and_description() {
    let skills = test_skills();

    let selected = match_auto_skills(&skills, "Please review this patch", 3);

    assert_eq!(selected.len(), 1);
    assert_eq!(selected[0].name, "review");
}

#[test]
fn auto_skill_matching_respects_limit() {
    let skills = test_skills();

    let selected = match_auto_skills(&skills, "Please review and format this patch", 1);

    assert_eq!(selected.len(), 1);
}

fn test_skills() -> Vec<SkillDefinition> {
    vec![
        SkillDefinition {
            name: "review".to_owned(),
            description: "Review code changes".to_owned(),
            content: "Full review instructions.".to_owned(),
            scope: SkillScope::Project,
            path: "/workspace/.jux/skills/review/SKILL.md".into(),
        },
        SkillDefinition {
            name: "format".to_owned(),
            description: "Format code".to_owned(),
            content: "Full format instructions.".to_owned(),
            scope: SkillScope::User,
            path: "/home/user/.jux/skills/format/SKILL.md".into(),
        },
    ]
}

fn write_skill(root: &std::path::Path, name: &str, description: &str, content: &str) {
    let content = format!("---\nname: {name}\ndescription: {description}\n---\n{content}");
    write_raw_skill(root, name, &content);
}

fn write_raw_skill(root: &std::path::Path, name: &str, content: &str) {
    let directory = root.join(".jux/skills").join(name);
    fs::create_dir_all(&directory).expect("skill directory is created");
    fs::write(directory.join("SKILL.md"), content).expect("skill file is written");
}

fn unique_temp_dir(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("{name}-{}", uuid::Uuid::new_v4()))
}

fn remove_temp_dir(path: PathBuf) {
    if path.exists() {
        fs::remove_dir_all(path).expect("temp dir is removed");
    }
}
