use crate::types::{HistoryEntry, Index, Operation};
use anyhow::{Context, Result};
use chrono::Utc;
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

pub struct Storage {
    ftm_dir: PathBuf,
    max_history: usize,
}

impl Storage {
    pub fn new(ftm_dir: PathBuf, max_history: usize) -> Self {
        Self {
            ftm_dir,
            max_history,
        }
    }

    fn index_path(&self) -> PathBuf {
        self.ftm_dir.join("index.json")
    }

    fn snapshots_dir(&self) -> PathBuf {
        self.ftm_dir.join("snapshots")
    }

    /// Get snapshot path using two-level directory structure: {checksum[0]}/{checksum[1]}/{checksum}
    fn snapshot_path(&self, checksum: &str) -> PathBuf {
        let c1 = &checksum[0..1];
        let c2 = &checksum[1..2];
        self.snapshots_dir().join(c1).join(c2).join(checksum)
    }

    pub fn load_index(&self) -> Result<Index> {
        let path = self.index_path();
        if path.exists() {
            let content = std::fs::read_to_string(&path)?;
            Ok(serde_json::from_str(&content)?)
        } else {
            Ok(Index::default())
        }
    }

    pub fn save_index(&self, index: &Index) -> Result<()> {
        let content = serde_json::to_string_pretty(index)?;
        std::fs::write(self.index_path(), content)?;
        Ok(())
    }

    pub fn compute_checksum(content: &[u8]) -> String {
        let mut hasher = Sha256::new();
        hasher.update(content);
        hex::encode(hasher.finalize())
    }

    /// Get the last entry for a specific file (any operation type)
    fn get_last_entry_for_file<'a>(
        &self,
        index: &'a Index,
        file: &str,
    ) -> Option<&'a HistoryEntry> {
        index.history.iter().rev().find(|e| e.file == file)
    }

    /// Stream file: read in chunks, hash and write to temp in one pass, then rename to snapshot path.
    /// Returns (checksum, size). Caller must remove temp on same-checksum early return.
    fn stream_hash_and_save(
        &self,
        file_path: &Path,
        tmp_path: &Path,
    ) -> Result<(String, u64)> {
        const BUF_SIZE: usize = 65536;
        let mut reader = std::fs::File::open(file_path).context("Failed to read file")?;
        let mut tmp_file = std::fs::File::create(tmp_path)?;
        let mut hasher = Sha256::new();
        let mut buf = [0u8; BUF_SIZE];
        loop {
            let n = reader.read(&mut buf)?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
            tmp_file.write_all(&buf[..n])?;
        }
        let checksum = hex::encode(hasher.finalize());
        let size = std::fs::metadata(tmp_path)?.len();
        Ok((checksum, size))
    }

    pub fn save_snapshot(&self, file_path: &Path, root_dir: &Path) -> Result<Option<HistoryEntry>> {
        let rel_path = file_path.strip_prefix(root_dir).unwrap_or(file_path);
        let file_key = rel_path.to_string_lossy().to_string();

        let tmp_dir = self.snapshots_dir().join(".tmp");
        std::fs::create_dir_all(&tmp_dir)?;
        let tmp_path = tmp_dir.join(uuid::Uuid::new_v4().to_string());

        let (checksum, size) = self.stream_hash_and_save(file_path, &tmp_path)?;

        let mut index = self.load_index()?;
        let last_entry = self.get_last_entry_for_file(&index, &file_key);
        let op = match last_entry {
            Some(entry) => {
                if entry.op == Operation::Delete {
                    Operation::Create
                } else if entry.checksum.as_deref() == Some(checksum.as_str()) {
                    std::fs::remove_file(&tmp_path).ok();
                    return Ok(None);
                } else {
                    Operation::Modify
                }
            }
            None => Operation::Create,
        };

        let snapshot_path = self.snapshot_path(&checksum);
        if !snapshot_path.exists() {
            if let Some(parent) = snapshot_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::rename(&tmp_path, &snapshot_path)?;
        } else {
            std::fs::remove_file(&tmp_path)?;
        }

        let entry = HistoryEntry {
            timestamp: Utc::now(),
            op,
            file: file_key,
            checksum: Some(checksum),
            size: Some(size),
        };

        index.history.push(entry.clone());
        self.trim_history(&mut index);
        self.save_index(&index)?;
        Ok(Some(entry))
    }

    pub fn record_delete(&self, file_path: &Path, root_dir: &Path) -> Result<Option<HistoryEntry>> {
        let rel_path = file_path.strip_prefix(root_dir).unwrap_or(file_path);
        let file_key = rel_path.to_string_lossy().to_string();

        let mut index = self.load_index()?;

        // Only record delete if we have history for this file
        let has_history = index.history.iter().any(|e| e.file == file_key);
        if !has_history {
            return Ok(None);
        }

        let entry = HistoryEntry {
            timestamp: Utc::now(),
            op: Operation::Delete,
            file: file_key,
            checksum: None,
            size: None,
        };

        index.history.push(entry.clone());
        self.save_index(&index)?;
        Ok(Some(entry))
    }

    fn trim_history(&self, index: &mut Index) {
        use std::collections::HashMap;

        // Count entries per file
        let mut file_counts: HashMap<&str, usize> = HashMap::new();
        for entry in index.history.iter().rev() {
            *file_counts.entry(&entry.file).or_insert(0) += 1;
        }

        // Find files that exceed max_history
        let files_to_trim: Vec<String> = file_counts
            .iter()
            .filter(|(_, &count)| count > self.max_history)
            .map(|(&file, _)| file.to_string())
            .collect();

        for file in files_to_trim {
            let mut count = 0;
            let mut to_remove = Vec::new();

            // Iterate from newest to oldest, mark old entries for removal
            for (i, entry) in index.history.iter().enumerate().rev() {
                if entry.file == file {
                    count += 1;
                    if count > self.max_history {
                        to_remove.push(i);
                    }
                }
            }

            // Remove from end to start to preserve indices
            for i in to_remove {
                index.history.remove(i);
            }
        }
    }

    pub fn list_history(&self, file_path: &str) -> Result<Vec<HistoryEntry>> {
        let index = self.load_index()?;
        let entries: Vec<HistoryEntry> = index
            .history
            .iter()
            .filter(|e| e.file == file_path)
            .cloned()
            .collect();
        Ok(entries)
    }

    pub fn list_files(&self) -> Result<Vec<(String, usize)>> {
        use std::collections::HashMap;

        let index = self.load_index()?;
        let mut file_counts: HashMap<String, usize> = HashMap::new();

        for entry in &index.history {
            *file_counts.entry(entry.file.clone()).or_insert(0) += 1;
        }

        let mut files: Vec<(String, usize)> = file_counts.into_iter().collect();
        files.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(files)
    }

    pub fn restore(&self, file_path: &str, checksum_prefix: &str, root_dir: &Path) -> Result<()> {
        let index = self.load_index()?;

        // Find entry matching the checksum prefix
        let entry = index
            .history
            .iter()
            .find(|e| {
                e.file == file_path
                    && e.checksum
                        .as_ref()
                        .map(|c| c.starts_with(checksum_prefix))
                        .unwrap_or(false)
            })
            .context("Version not found in history")?;

        let full_checksum = entry.checksum.as_ref().unwrap().clone();
        let snapshot_path = self.snapshot_path(&full_checksum);
        if !snapshot_path.exists() {
            anyhow::bail!("Snapshot file not found");
        }

        let content = std::fs::read(&snapshot_path)?;

        // Verify checksum
        if Self::compute_checksum(&content) != full_checksum {
            anyhow::bail!("Snapshot checksum mismatch");
        }

        // Simply copy the snapshot to the target location
        let target = root_dir.join(file_path);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(target, &content)?;

        Ok(())
    }
}
