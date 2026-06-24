//! Skill discovery.
//!
//! Skills are reusable instruction packages stored under `.jux/skills`.
//! This module discovers available skills and validates the basic `SKILL.md`
//! shape. Prompt injection is handled by later feature slices.

use serde::Deserialize;
use std::collections::BTreeMap;
use std::error::Error;
use std::fmt::{self, Display};
use std::fs;
use std::path::{Path, PathBuf};

const SKILLS_DIRECTORY: &str = ".jux/skills";
const SKILL_FILE_NAME: &str = "SKILL.md";
pub const MAX_SKILL_FILE_BYTES: u64 = 64 * 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
/// One discovered skill.
pub struct SkillDefinition {
    pub name: String,
    pub description: String,
    pub content: String,
    pub scope: SkillScope,
    pub path: PathBuf,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// Resolved skill catalog with active skills and override metadata.
pub struct SkillCatalog {
    pub skills: Vec<SkillDefinition>,
    pub overrides: Vec<SkillOverride>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// Records that one skill definition replaced another with the same name.
pub struct SkillOverride {
    pub name: String,
    pub overridden: SkillDefinition,
    pub active: SkillDefinition,
}

#[derive(Debug, Deserialize)]
struct SkillFrontmatter {
    name: Option<String>,
    description: Option<String>,
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
        Ok(self.resolve_catalog()?.skills)
    }

    pub fn resolve_catalog(&self) -> Result<SkillCatalog, SkillError> {
        let mut skills = BTreeMap::new();
        let mut overrides = Vec::new();
        if let Some(user_home) = &self.user_home {
            discover_skills(
                &mut skills,
                &mut overrides,
                SkillScope::User,
                &user_home.join(SKILLS_DIRECTORY),
            )?;
        }
        discover_skills(
            &mut skills,
            &mut overrides,
            SkillScope::Project,
            &self.workspace_root.join(SKILLS_DIRECTORY),
        )?;
        Ok(SkillCatalog {
            skills: skills.into_values().collect(),
            overrides,
        })
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

#[must_use]
/// Renders the available skill index for inclusion in the system prompt.
pub fn render_skill_index(skills: &[SkillDefinition]) -> String {
    let mut output = String::from(
        "## Available Skills\n\n\
         Project skills override user skills with the same name.\n\n",
    );
    for skill in skills {
        output.push_str(&format!("- {}: {}\n", skill.name, skill.description));
    }
    output
}

#[must_use]
/// Renders active skill bodies for inclusion in the system prompt.
pub fn render_active_skills(skills: &[SkillDefinition]) -> String {
    let mut output = String::from("## Active Skills\n\n");
    for skill in skills {
        output.push_str(&format!(
            "### {}\nSource: {}\nScope: {}\n\n{}\n\n",
            skill.name,
            skill.path.display(),
            skill.scope.label(),
            skill.content
        ));
    }
    output
}

/// Selects explicitly requested skills by name.
pub fn select_explicit_skills(
    skills: &[SkillDefinition],
    names: &[String],
) -> Result<Vec<SkillDefinition>, SkillError> {
    let mut selected = Vec::new();
    for name in names {
        let Some(skill) = skills.iter().find(|skill| skill.name == *name) else {
            return Err(SkillError::new(format!("skill not found: {name}")));
        };
        if !selected
            .iter()
            .any(|selected: &SkillDefinition| selected.name == skill.name)
        {
            selected.push(skill.clone());
        }
    }
    Ok(selected)
}

#[must_use]
/// Matches skills by request text using skill name and description tokens.
pub fn match_auto_skills(
    skills: &[SkillDefinition],
    request: &str,
    limit: usize,
) -> Vec<SkillDefinition> {
    if limit == 0 {
        return Vec::new();
    }
    let request = request.to_lowercase();
    skills
        .iter()
        .filter(|skill| skill_matches_request(skill, &request))
        .take(limit)
        .cloned()
        .collect()
}

fn discover_skills(
    skills: &mut BTreeMap<String, SkillDefinition>,
    overrides: &mut Vec<SkillOverride>,
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
        insert_skill(skills, overrides, scope, entry.path())?;
    }
    Ok(())
}

fn insert_skill(
    skills: &mut BTreeMap<String, SkillDefinition>,
    overrides: &mut Vec<SkillOverride>,
    scope: SkillScope,
    path: PathBuf,
) -> Result<(), SkillError> {
    let Some(name) = skill_directory_name(&path) else {
        return Ok(());
    };
    let skill_file = path.join(SKILL_FILE_NAME);
    if !skill_file.is_file() {
        return Ok(());
    }
    let skill = parse_skill_file(name, scope, skill_file)?;
    if let Some(overridden) = skills.insert(skill.name.clone(), skill.clone()) {
        overrides.push(SkillOverride {
            name: skill.name.clone(),
            overridden,
            active: skill,
        });
    }
    Ok(())
}

fn skill_directory_name(path: &Path) -> Option<String> {
    path.file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
}

fn parse_skill_file(
    expected_name: String,
    scope: SkillScope,
    path: PathBuf,
) -> Result<SkillDefinition, SkillError> {
    reject_oversized_skill_file(&path)?;
    let raw = fs::read_to_string(&path).map_err(|error| {
        SkillError::new(format!(
            "failed to read skill file {}: {error}",
            path.display()
        ))
    })?;
    if raw.trim().is_empty() {
        return Err(SkillError::new(format!(
            "empty skill file {}",
            path.display()
        )));
    }
    let (frontmatter, content) = split_skill_file(&raw, &path)?;
    let metadata = parse_frontmatter(frontmatter, &path)?;
    let name = required_field(metadata.name, "name", &path)?;
    let description = required_field(metadata.description, "description", &path)?;
    if name != expected_name {
        return Err(SkillError::new(format!(
            "skill name {name:?} does not match directory name {expected_name:?}"
        )));
    }
    Ok(SkillDefinition {
        name,
        description,
        content: content.trim().to_owned(),
        scope,
        path,
    })
}

fn split_skill_file<'a>(raw: &'a str, path: &Path) -> Result<(&'a str, &'a str), SkillError> {
    let Some(rest) = raw.strip_prefix("---") else {
        return Err(SkillError::new(format!(
            "skill file {} is missing frontmatter",
            path.display()
        )));
    };
    let Some((frontmatter, content)) = rest.split_once("\n---") else {
        return Err(SkillError::new(format!(
            "skill file {} has unterminated frontmatter",
            path.display()
        )));
    };
    Ok((frontmatter, content.trim_start_matches(['\r', '\n'])))
}

fn parse_frontmatter(frontmatter: &str, path: &Path) -> Result<SkillFrontmatter, SkillError> {
    serde_yaml::from_str(frontmatter).map_err(|error| {
        SkillError::new(format!(
            "failed to parse skill frontmatter {}: {error}",
            path.display()
        ))
    })
}

fn required_field(value: Option<String>, field: &str, path: &Path) -> Result<String, SkillError> {
    let value = value.unwrap_or_default();
    if value.trim().is_empty() {
        return Err(SkillError::new(format!(
            "skill file {} is missing {field}",
            path.display()
        )));
    }
    Ok(value)
}

fn reject_oversized_skill_file(path: &Path) -> Result<(), SkillError> {
    let size = fs::metadata(path).map_err(|error| {
        SkillError::new(format!(
            "failed to read skill file metadata {}: {error}",
            path.display()
        ))
    })?;
    if size.len() > MAX_SKILL_FILE_BYTES {
        return Err(SkillError::new(format!(
            "skill file {} exceeds maximum skill file size of {MAX_SKILL_FILE_BYTES} bytes",
            path.display()
        )));
    }
    Ok(())
}

fn skill_matches_request(skill: &SkillDefinition, request: &str) -> bool {
    request.contains(&skill.name.to_lowercase())
        || description_words(&skill.description)
            .iter()
            .any(|word| request.contains(word))
}

fn description_words(description: &str) -> Vec<String> {
    description
        .split(|char: char| !char.is_ascii_alphanumeric())
        .filter(|word| word.len() >= 4)
        .map(str::to_lowercase)
        .collect()
}

impl SkillScope {
    fn label(&self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Project => "project",
        }
    }
}
