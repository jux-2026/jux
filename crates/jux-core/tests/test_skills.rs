use jux_core::{SkillDefinition, SkillResolver, SkillScope};
use std::fs;
use std::path::PathBuf;

#[test]
fn skill_resolver_discovers_user_and_project_skills() {
    let home = unique_temp_dir("jux-skills-home");
    let workspace = unique_temp_dir("jux-skills-workspace");
    write_skill(&home, "format", "user format skill");
    write_skill(&workspace, "review", "project review skill");

    let skills = SkillResolver::new(home.clone(), workspace.clone())
        .resolve()
        .expect("skills resolve");

    assert_eq!(
        skills,
        vec![
            SkillDefinition {
                name: "format".to_owned(),
                scope: SkillScope::User,
                path: home.join(".jux/skills/format/SKILL.md"),
            },
            SkillDefinition {
                name: "review".to_owned(),
                scope: SkillScope::Project,
                path: workspace.join(".jux/skills/review/SKILL.md"),
            },
        ]
    );
    fs::remove_dir_all(home).expect("user temp dir is removed");
    fs::remove_dir_all(workspace).expect("workspace temp dir is removed");
}

#[test]
fn project_skill_overrides_user_skill_with_same_name() {
    let home = unique_temp_dir("jux-skills-override-home");
    let workspace = unique_temp_dir("jux-skills-override-workspace");
    write_skill(&home, "review", "user review skill");
    write_skill(&workspace, "review", "project review skill");

    let skills = SkillResolver::new(home.clone(), workspace.clone())
        .resolve()
        .expect("skills resolve");

    assert_eq!(
        skills,
        vec![SkillDefinition {
            name: "review".to_owned(),
            scope: SkillScope::Project,
            path: workspace.join(".jux/skills/review/SKILL.md"),
        }]
    );
    fs::remove_dir_all(home).expect("user temp dir is removed");
    fs::remove_dir_all(workspace).expect("workspace temp dir is removed");
}

fn write_skill(root: &std::path::Path, name: &str, content: &str) {
    let directory = root.join(".jux/skills").join(name);
    fs::create_dir_all(&directory).expect("skill directory is created");
    fs::write(directory.join("SKILL.md"), content).expect("skill file is written");
}

fn unique_temp_dir(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("{name}-{}", uuid::Uuid::new_v4()))
}
