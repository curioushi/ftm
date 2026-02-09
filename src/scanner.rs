use crate::config::Config;
use crate::storage::{IndexView, Storage};
use crate::types::{Index, Operation};
use anyhow::Result;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use tracing::info;

#[derive(serde::Serialize)]
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

        let mut index = self.storage.load_index()?;
        let mut view = self.storage.build_index_view(&index);
        let mut index_changed = false;

        // Phase 1: Walk directory and snapshot all matching files
        let mut scanned_files = HashSet::new();
        self.walk_and_snapshot(
            &self.root_dir.clone(),
            &mut scanned_files,
            &mut result,
            &mut index,
            &mut view,
            &mut index_changed,
        )?;

        // Phase 2: Detect deleted files (in index but not on disk)
        self.detect_deletes(
            &scanned_files,
            &mut result,
            &mut index,
            &mut view,
            &mut index_changed,
        )?;

        if index_changed {
            if self.storage.trim_history(&mut index) {
                view.rebuild(&index);
            }
            self.storage.save_index(&index)?;
        }

        Ok(result)
    }

    fn walk_and_snapshot(
        &self,
        dir: &Path,
        scanned_files: &mut HashSet<String>,
        result: &mut ScanResult,
        index: &mut Index,
        view: &mut IndexView,
        index_changed: &mut bool,
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
                    self.walk_and_snapshot(
                        &path,
                        scanned_files,
                        result,
                        index,
                        view,
                        index_changed,
                    )?;
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

                match self
                    .storage
                    .save_snapshot_with_index(&path, &self.root_dir, index, view)?
                {
                    Some(entry) => match entry.op {
                        Operation::Create => {
                            info!("Scan: new file {}", entry.file);
                            result.created += 1;
                            *index_changed = true;
                        }
                        Operation::Modify => {
                            info!("Scan: modified file {}", entry.file);
                            result.modified += 1;
                            *index_changed = true;
                        }
                        _ => {}
                    },
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
        index: &mut Index,
        view: &mut IndexView,
        index_changed: &mut bool,
    ) -> Result<()> {
        let mut to_delete = Vec::new();
        for (file_key, idx) in &view.last_by_file {
            let last_entry = &index.history[*idx];
            if last_entry.op == Operation::Delete {
                continue;
            }
            if !scanned_files.contains(file_key) {
                to_delete.push(file_key.clone());
            }
        }

        for file_key in to_delete {
            let abs_path = self.root_dir.join(&file_key);
            if self
                .storage
                .record_delete_with_index(&abs_path, &self.root_dir, index, view)?
                .is_some()
            {
                info!("Scan: deleted file {}", file_key);
                result.deleted += 1;
                *index_changed = true;
            }
        }

        Ok(())
    }
}
