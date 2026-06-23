//! Skill discovery.
//!
//! Skills are reusable instruction packages stored under `.jux/skills`.
//! This module currently discovers available skills only; metadata parsing and
//! prompt injection are handled by later feature slices.

use std::collections::BTreeMap;
use std::error::Error;
use std::fmt::{self, Display};
use std::fs;
use std::path::{Path, PathBuf};

const SKILLS_DIRECTORY: &str = ".jux/skills";
const SKILL_FILE_NAME: &str = "SKILL.md";

#[derive(Clone, Debug, Eq, PartialEq)]
/// One discovered skill.
pub struct SkillDefinition {
    pub name: String,
    pub scope: SkillScope,
    pub path: PathBuf,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// Source layer for a skill.
pub enum SkillScope {
    User,
    Project,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// Resolves user-level and project-level skills.
pub struct SkillResolver {
    user_home: Option<PathBuf>,
    workspace_root: PathBuf,
}

impl SkillResolver {
    #[must_use]
    pub fn new(user_home: impl Into<PathBuf>, workspace_root: impl Into<PathBuf>) -> Self {
        Self {
            user_home: Some(user_home.into()),
            workspace_root: workspace_root.into(),
        }
    }

    #[must_use]
    pub fn project_only(workspace_root: impl Into<PathBuf>) -> Self {
        Self {
            user_home: None,
            workspace_root: workspace_root.into(),
        }
    }

    pub fn resolve(&self) -> Result<Vec<SkillDefinition>, SkillError> {
        let mut skills = BTreeMap::new();
        if let Some(user_home) = &self.user_home {
            discover_skills(
                &mut skills,
                SkillScope::User,
                &user_home.join(SKILLS_DIRECTORY),
            )?;
        }
        discover_skills(
            &mut skills,
            SkillScope::Project,
            &self.workspace_root.join(SKILLS_DIRECTORY),
        )?;
        Ok(skills.into_values().collect())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// Skill discovery error.
pub struct SkillError {
    message: String,
}

impl SkillError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl Display for SkillError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.message)
    }
}

impl Error for SkillError {}

fn discover_skills(
    skills: &mut BTreeMap<String, SkillDefinition>,
    scope: SkillScope,
    directory: &Path,
) -> Result<(), SkillError> {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(SkillError::new(format!(
                "failed to read skills directory {}: {error}",
                directory.display()
            )));
        }
    };
    for entry in entries {
        let entry = entry.map_err(|error| {
            SkillError::new(format!(
                "failed to read skills directory entry {}: {error}",
                directory.display()
            ))
        })?;
        insert_skill(skills, scope, entry.path());
    }
    Ok(())
}

fn insert_skill(skills: &mut BTreeMap<String, SkillDefinition>, scope: SkillScope, path: PathBuf) {
    let Some(name) = skill_directory_name(&path) else {
        return;
    };
    let skill_file = path.join(SKILL_FILE_NAME);
    if !skill_file.is_file() {
        return;
    }
    skills.insert(
        name.clone(),
        SkillDefinition {
            name,
            scope,
            path: skill_file,
        },
    );
}

fn skill_directory_name(path: &Path) -> Option<String> {
    path.file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
}
