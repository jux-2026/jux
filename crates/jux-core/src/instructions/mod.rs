//! User and project instruction document discovery.
//!
//! Instruction documents are plain Markdown files intended for the language
//! model. They are separate from structured config because they express natural
//! language guidance rather than machine-validated settings.

use std::error::Error;
use std::fmt::{self, Display};
use std::fs;
use std::path::{Path, PathBuf};

const INSTRUCTION_FILENAMES: &[&str] = &["AGENTS.md", "agents.md"];

#[derive(Clone, Debug, Eq, PartialEq)]
/// One loaded instruction document.
pub struct InstructionDocument {
    pub scope: InstructionScope,
    pub path: PathBuf,
    pub content: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// Source layer for an instruction document.
pub enum InstructionScope {
    User,
    Project,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// Resolves user-level and project-level instruction documents.
pub struct InstructionResolver {
    user_home: Option<PathBuf>,
    workspace_root: PathBuf,
}

impl InstructionResolver {
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

    pub fn resolve(&self) -> Result<Vec<InstructionDocument>, InstructionError> {
        let mut documents = Vec::new();
        if let Some(user_home) = &self.user_home {
            self.push_document(
                &mut documents,
                InstructionScope::User,
                &user_home.join(".jux"),
            )?;
        }
        self.push_document(
            &mut documents,
            InstructionScope::Project,
            &self.workspace_root.join(".jux"),
        )?;
        Ok(documents)
    }

    fn push_document(
        &self,
        documents: &mut Vec<InstructionDocument>,
        scope: InstructionScope,
        directory: &Path,
    ) -> Result<(), InstructionError> {
        let Some(path) = first_instruction_path(directory) else {
            return Ok(());
        };
        let content = fs::read_to_string(&path).map_err(|error| {
            InstructionError::new(format!(
                "failed to read instruction document {}: {error}",
                path.display()
            ))
        })?;
        if content.trim().is_empty() {
            return Ok(());
        }
        documents.push(InstructionDocument {
            scope,
            path,
            content,
        });
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// Instruction discovery error.
pub struct InstructionError {
    message: String,
}

impl InstructionError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl Display for InstructionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.message)
    }
}

impl Error for InstructionError {}

#[must_use]
/// Renders instruction documents for inclusion in the system prompt.
pub fn render_instruction_documents(documents: &[InstructionDocument]) -> String {
    let mut output = String::from(
        "## Jux Instruction Documents\n\n\
         The following instruction documents are loaded for this run.\n\n\
         Priority rule:\n\
         - Project instructions have higher priority than user instructions.\n\
         - If instructions conflict, follow project instructions.\n",
    );
    for document in documents {
        output.push_str(&render_instruction_document(document));
    }
    output
}

fn render_instruction_document(document: &InstructionDocument) -> String {
    format!(
        "\n### {} Instructions\nSource: {}\n\n{}\n",
        document.scope.title(),
        document.path.display(),
        document.content
    )
}

fn first_instruction_path(directory: &Path) -> Option<PathBuf> {
    INSTRUCTION_FILENAMES
        .iter()
        .map(|name| directory.join(name))
        .find(|path| path.is_file())
}

impl InstructionScope {
    fn title(&self) -> &'static str {
        match self {
            Self::User => "User",
            Self::Project => "Project",
        }
    }
}
