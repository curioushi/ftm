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
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Index {
    pub history: Vec<HistoryEntry>,
}

/// Result of clean_orphan_snapshots: counts of removed files and bytes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CleanResult {
    pub files_removed: usize,
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
