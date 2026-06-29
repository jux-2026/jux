use serde::{Deserialize, Serialize};
use std::error::Error;
use std::fmt::{self, Display};
use std::fs;
use std::path::{Component, Path};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CodeChangePlan {
    pub summary: String,
    pub items: Vec<String>,
}

impl CodeChangePlan {
    #[must_use]
    pub fn new(summary: impl Into<String>, items: Vec<String>) -> Self {
        Self {
            summary: summary.into(),
            items,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ProposedFileContent {
    pub path: String,
    pub new_content: String,
}

impl ProposedFileContent {
    #[must_use]
    pub fn new(path: impl Into<String>, new_content: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            new_content: new_content.into(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CodeChangeProposal {
    pub plan: CodeChangePlan,
    pub files: Vec<ProposedFileChange>,
    pub policy: PolicyDecision,
    pub warnings: Vec<RiskWarning>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CodeChangeReview {
    pub proposal: CodeChangeProposal,
    pub status: ReviewStatus,
}

impl CodeChangeReview {
    #[must_use]
    pub fn new(proposal: CodeChangeProposal) -> Self {
        Self {
            proposal,
            status: ReviewStatus::Pending,
        }
    }

    pub fn approve(&mut self) -> Result<(), CodeChangeError> {
        if self.status != ReviewStatus::Pending {
            return Err(CodeChangeError::InvalidReviewState);
        }
        if self.proposal.policy == PolicyDecision::Deny {
            return Err(CodeChangeError::PolicyDenied);
        }
        self.status = ReviewStatus::Approved;
        Ok(())
    }

    pub fn reject(&mut self) -> Result<(), CodeChangeError> {
        if self.status != ReviewStatus::Pending {
            return Err(CodeChangeError::InvalidReviewState);
        }
        self.status = ReviewStatus::Rejected;
        Ok(())
    }

    pub fn request_changes(&mut self, feedback: String) -> Result<(), CodeChangeError> {
        if self.status != ReviewStatus::Pending {
            return Err(CodeChangeError::InvalidReviewState);
        }
        self.status = ReviewStatus::ChangesRequested { feedback };
        Ok(())
    }

    pub fn apply(&mut self, workspace_root: &Path) -> Result<(), CodeChangeError> {
        if self.status != ReviewStatus::Approved {
            return Err(CodeChangeError::InvalidReviewState);
        }
        let conflicts = self
            .proposal
            .files
            .iter()
            .filter_map(|file| {
                let path = workspace_root.join(file.path.as_str());
                let current = match fs::read_to_string(path) {
                    Ok(content) => content,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
                    Err(error) => return Some(Err(error)),
                };
                (current != file.original_content).then(|| Ok(file.path.as_str().to_owned()))
            })
            .collect::<Result<Vec<_>, _>>()
            .map_err(CodeChangeError::Io)?;
        if !conflicts.is_empty() {
            self.status = ReviewStatus::Conflict {
                paths: conflicts.clone(),
            };
            return Err(CodeChangeError::Conflict(conflicts));
        }
        for file in &self.proposal.files {
            let path = workspace_root.join(file.path.as_str());
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(path, &file.new_content)?;
        }
        self.status = ReviewStatus::Applied;
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum ReviewStatus {
    Pending,
    Rejected,
    ChangesRequested { feedback: String },
    Approved,
    Applied,
    Conflict { paths: Vec<String> },
}

impl CodeChangeProposal {
    pub fn prepare(
        workspace_root: &Path,
        plan: CodeChangePlan,
        files: Vec<ProposedFileContent>,
    ) -> Result<Self, CodeChangeError> {
        let files = files
            .into_iter()
            .map(|file| ProposedFileChange::prepare(workspace_root, file))
            .collect::<Result<Vec<_>, _>>()?;
        let warnings = files
            .iter()
            .filter(|file| is_sensitive_path(file.path.as_str()))
            .map(|file| RiskWarning {
                path: file.path.clone(),
                level: RiskLevel::High,
                reason: "sensitive file modification is denied".to_owned(),
            })
            .collect::<Vec<_>>();
        let policy = if warnings.is_empty() {
            PolicyDecision::Confirm
        } else {
            PolicyDecision::Deny
        };
        Ok(Self {
            plan,
            files,
            policy,
            warnings,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ProposedFileChange {
    pub path: WorkspaceRelativePath,
    pub original_content: String,
    pub new_content: String,
    pub diff: String,
}

impl ProposedFileChange {
    fn prepare(workspace_root: &Path, file: ProposedFileContent) -> Result<Self, CodeChangeError> {
        let path = WorkspaceRelativePath::parse(file.path)?;
        let absolute_path = workspace_root.join(path.as_str());
        let original_content = match fs::read_to_string(&absolute_path) {
            Ok(content) => content,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(error) => return Err(CodeChangeError::Io(error)),
        };
        let diff = whole_file_diff(path.as_str(), &original_content, &file.new_content);
        Ok(Self {
            path,
            original_content,
            new_content: file.new_content,
            diff,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceRelativePath(String);

impl WorkspaceRelativePath {
    pub fn parse(path: String) -> Result<Self, CodeChangeError> {
        let parsed = Path::new(&path);
        if path.is_empty()
            || parsed.is_absolute()
            || !parsed
                .components()
                .all(|component| matches!(component, Component::Normal(_)))
        {
            return Err(CodeChangeError::InvalidPath(path));
        }
        Ok(Self(path))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum PolicyDecision {
    Allow,
    Deny,
    Confirm,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RiskWarning {
    pub path: WorkspaceRelativePath,
    pub level: RiskLevel,
    pub reason: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum RiskLevel {
    Medium,
    High,
}

#[derive(Debug)]
pub enum CodeChangeError {
    InvalidPath(String),
    InvalidReviewState,
    PolicyDenied,
    Conflict(Vec<String>),
    Io(std::io::Error),
}

impl Display for CodeChangeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidPath(path) => write!(formatter, "invalid workspace-relative path: {path}"),
            Self::InvalidReviewState => formatter.write_str("code change review state is invalid"),
            Self::PolicyDenied => formatter.write_str("code change policy denied the operation"),
            Self::Conflict(paths) => {
                write!(
                    formatter,
                    "code change conflicts with current files: {}",
                    paths.join(", ")
                )
            }
            Self::Io(error) => write!(formatter, "code change IO error: {error}"),
        }
    }
}

impl Error for CodeChangeError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::InvalidPath(_)
            | Self::InvalidReviewState
            | Self::PolicyDenied
            | Self::Conflict(_) => None,
            Self::Io(error) => Some(error),
        }
    }
}

impl From<std::io::Error> for CodeChangeError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

fn whole_file_diff(path: &str, original_content: &str, new_content: &str) -> String {
    let mut diff = format!("--- a/{path}\n+++ b/{path}\n@@\n");
    for line in original_content.lines() {
        diff.push('-');
        diff.push_str(line);
        diff.push('\n');
    }
    for line in new_content.lines() {
        diff.push('+');
        diff.push_str(line);
        diff.push('\n');
    }
    diff
}

fn is_sensitive_path(path: &str) -> bool {
    let lowercase = path.to_ascii_lowercase();
    let file_name = Path::new(&lowercase)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    file_name == ".env"
        || file_name.starts_with(".env.")
        || file_name.ends_with(".pem")
        || file_name.ends_with(".key")
        || lowercase.contains("/.ssh/")
        || lowercase.starts_with(".ssh/")
        || lowercase.contains("/.aws/")
        || lowercase.starts_with(".aws/")
        || file_name.contains("credentials")
}
