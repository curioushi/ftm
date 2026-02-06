use crate::config::Config;
use crate::storage::Storage;
use crate::types::Operation;
use anyhow::Result;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use tracing::info;

pub struct ScanResult {
    pub created: usize,
    pub modified: usize,
    pub deleted: usize,
    pub unchanged: usize,
}

pub struct Scanner {
    root_dir: PathBuf,
    config: Config,
    storage: Storage,
}

impl Scanner {
    pub fn new(root_dir: PathBuf, config: Config, storage: Storage) -> Self {
        Self {
            root_dir,
            config,
            storage,
        }
    }

    /// Perform a full scan of the directory, detecting creates, modifies, and deletes.
    pub fn scan(&self) -> Result<ScanResult> {
        let mut result = ScanResult {
            created: 0,
            modified: 0,
            deleted: 0,
            unchanged: 0,
        };

        // Phase 1: Walk directory and snapshot all matching files
        let mut scanned_files = HashSet::new();
        self.walk_and_snapshot(&self.root_dir.clone(), &mut scanned_files, &mut result)?;

        // Phase 2: Detect deleted files (in index but not on disk)
        self.detect_deletes(&scanned_files, &mut result)?;

        Ok(result)
    }

    fn walk_and_snapshot(
        &self,
        dir: &Path,
        scanned_files: &mut HashSet<String>,
        result: &mut ScanResult,
    ) -> Result<()> {
        let entries = match std::fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(_) => return Ok(()),
        };

        for entry in entries {
            let entry = entry?;
            let path = entry.path();

            if path.is_dir() {
                // Skip excluded directories
                if !self.is_excluded_dir(&path) {
                    self.walk_and_snapshot(&path, scanned_files, result)?;
                }
            } else if path.is_file() && self.config.matches_path(&path, &self.root_dir) {
                // Skip files exceeding max_file_size
                if let Ok(meta) = std::fs::metadata(&path) {
                    if meta.len() > self.config.settings.max_file_size {
                        continue;
                    }
                }

                let rel_path = path.strip_prefix(&self.root_dir).unwrap_or(&path);
                let file_key = rel_path.to_string_lossy().to_string();
                scanned_files.insert(file_key);

                match self.storage.save_snapshot(&path, &self.root_dir)? {
                    Some(entry) => {
                        match entry.op {
                            Operation::Create => {
                                info!("Scan: new file {}", entry.file);
                                result.created += 1;
                            }
                            Operation::Modify => {
                                info!("Scan: modified file {}", entry.file);
                                result.modified += 1;
                            }
                            _ => {}
                        }
                    }
                    None => {
                        result.unchanged += 1;
                    }
                }
            }
        }

        Ok(())
    }

    /// Check if a directory path matches any exclude pattern.
    /// Used to skip entire directory trees early.
    fn is_excluded_dir(&self, path: &Path) -> bool {
        let rel_path = path.strip_prefix(&self.root_dir).unwrap_or(path);
        let path_str = rel_path.to_string_lossy();

        // Append separator so patterns like "**/target/**" match directory paths
        let dir_str = format!("{}/", path_str);

        for pattern in &self.config.watch.exclude {
            if let Ok(p) = glob::Pattern::new(pattern) {
                if p.matches(&dir_str) || p.matches(&path_str) {
                    return true;
                }
            }
        }

        false
    }

    fn detect_deletes(
        &self,
        scanned_files: &HashSet<String>,
        result: &mut ScanResult,
    ) -> Result<()> {
        let index = self.storage.load_index()?;

        // Collect unique files and their last operation
        let mut last_ops: std::collections::HashMap<&str, &Operation> =
            std::collections::HashMap::new();
        for entry in &index.history {
            last_ops.insert(&entry.file, &entry.op);
        }

        for (file_key, last_op) in &last_ops {
            // Skip files already marked as deleted
            if **last_op == Operation::Delete {
                continue;
            }

            // If the file was not found during scan, it has been deleted
            if !scanned_files.contains(*file_key) {
                let abs_path = self.root_dir.join(file_key);
                if self.storage.record_delete(&abs_path, &self.root_dir)?.is_some() {
                    info!("Scan: deleted file {}", file_key);
                    result.deleted += 1;
                }
            }
        }

        Ok(())
    }
}
