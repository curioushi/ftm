use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Operation {
    Create,
    Modify,
    Delete,
}

impl std::fmt::Display for Operation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Operation::Create => write!(f, "create"),
            Operation::Modify => write!(f, "modify"),
            Operation::Delete => write!(f, "delete"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub timestamp: DateTime<Utc>,
    pub op: Operation,
    pub file: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checksum: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    /// File mtime in nanoseconds since Unix epoch; used for fast skip (avoids same-second false skip).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mtime_nanos: Option<i64>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Index {
    pub history: Vec<HistoryEntry>,
}

/// Result of clean (trim + orphan removal): counts for both phases.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CleanResult {
    /// History entries removed by trim (max_history / max_quota).
    pub entries_trimmed: usize,
    /// Bytes freed by trim (snapshots deleted due to trim).
    pub bytes_freed_trim: u64,
    /// Orphan snapshot files removed (not referenced by any history).
    pub files_removed: usize,
    /// Bytes freed by orphan removal.
    pub bytes_removed: u64,
}

/// Tree node for structured file listing (ls). Directories have children; files have count.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileTreeNode {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub count: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub children: Option<Vec<FileTreeNode>>,
}
