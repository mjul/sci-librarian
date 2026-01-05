use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash, sqlx::Type)]
#[sqlx(transparent)]
pub struct DropboxId(pub String);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, sqlx::Type)]
#[sqlx(transparent)]
pub struct RemotePath(pub String);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LocalPath(pub PathBuf);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkDirectory(pub PathBuf);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DropboxInbox(pub String);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, sqlx::Type)]
#[sqlx(transparent)]
pub struct FileHash(pub String);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, sqlx::Type)]
#[sqlx(transparent)]
pub struct OneLineSummary(pub String);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArticleMetadata {
    pub title: String,
    pub authors: Vec<String>,
    pub summary: OneLineSummary,
    pub abstract_text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, sqlx::Type)]
#[sqlx(rename_all = "UPPERCASE")]
pub enum FileStatus {
    Pending,
    Downloaded,
    Processed,
    Archived,
    Error,
    Skipped,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct FileRecord {
    pub dropbox_id: DropboxId,
    pub file_name: Option<String>,
    pub content_hash: FileHash,
    pub status: FileStatus,
    pub title: Option<String>,
    pub authors: Option<String>, // JSON array string
    pub summary: Option<String>,
    pub target_path: Option<String>,
    pub last_error: Option<String>,
    pub updated_at: DateTime<Utc>,
}

pub struct Job {
    pub id: DropboxId,
    pub path: RemotePath,
}

pub enum JobResult {
    Success {
        id: DropboxId,
        meta: ArticleMetadata,
        target_paths: Vec<RemotePath>,
    },
    Failure {
        id: DropboxId,
        error: String,
    },
}

/** This is a struct representing a rule for categorizing files. */
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rule {
    pub description: String,
    pub target: String,
}

/** This is a struct representing all the rules for categorizing files. */
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rules(pub(crate) Vec<Rule>);

impl From<Vec<Rule>> for Rules {
    fn from(rules: Vec<Rule>) -> Self {
        Rules(rules)
    }
}
