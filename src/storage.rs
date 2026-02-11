use crate::path_util;
use crate::types::{CleanResult, FileTreeNode, HistoryEntry, Index, Operation};
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};

pub struct Storage {
    ftm_dir: PathBuf,
    max_history: usize,
    max_quota: u64,
}

pub struct IndexView {
    pub(crate) last_by_file: HashMap<String, usize>,
}

enum BuildNode {
    File(usize),
    Dir(BTreeMap<String, BuildNode>),
}

impl IndexView {
    fn from_index(index: &Index) -> Self {
        let mut last_by_file = HashMap::new();
        for (i, entry) in index.history.iter().enumerate() {
            last_by_file.insert(entry.file.clone(), i);
        }
        Self { last_by_file }
    }

    pub(crate) fn last_entry_for_file<'a>(
        &self,
        index: &'a Index,
        file: &str,
    ) -> Option<&'a HistoryEntry> {
        self.last_by_file
            .get(file)
            .and_then(|i| index.history.get(*i))
    }

    fn update_last_for_file(&mut self, file: String, index: usize) {
        self.last_by_file.insert(file, index);
    }

    #[allow(dead_code)]
    pub(crate) fn rebuild(&mut self, index: &Index) {
        self.last_by_file.clear();
        for (i, entry) in index.history.iter().enumerate() {
            self.last_by_file.insert(entry.file.clone(), i);
        }
    }
}

impl Storage {
    pub fn new(ftm_dir: PathBuf, max_history: usize, max_quota: u64) -> Self {
        Self {
            ftm_dir,
            max_history,
            max_quota,
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
        let content = serde_json::to_string(index)?;
        std::fs::write(self.index_path(), content)?;
        Ok(())
    }

    pub fn build_index_view(&self, index: &Index) -> IndexView {
        IndexView::from_index(index)
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
    /// Returns (checksum, size), or None if the file was modified during read.
    /// Caller must remove temp on same-checksum early return.
    fn stream_hash_and_save(
        &self,
        file_path: &Path,
        tmp_path: &Path,
    ) -> Result<Option<(String, u64)>> {
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

        // Verify the file was not modified during our read.
        // If the current on-disk size differs from what we read, another write
        // has started (truncate + partial write), so discard this snapshot.
        let current_size = std::fs::metadata(file_path).map(|m| m.len()).unwrap_or(0);
        if current_size != size {
            return Ok(None);
        }

        Ok(Some((checksum, size)))
    }

    #[allow(dead_code)]
    pub fn save_snapshot(&self, file_path: &Path, root_dir: &Path) -> Result<Option<HistoryEntry>> {
        let mut index = self.load_index()?;
        let mut view = IndexView::from_index(&index);
        let entry = self.save_snapshot_with_index(file_path, root_dir, &mut index, &mut view)?;
        if entry.is_some() {
            self.save_index(&index)?;
        }
        Ok(entry)
    }

    pub fn save_snapshot_with_index(
        &self,
        file_path: &Path,
        root_dir: &Path,
        index: &mut Index,
        view: &mut IndexView,
    ) -> Result<Option<HistoryEntry>> {
        let rel_path = file_path.strip_prefix(root_dir).unwrap_or(file_path);
        let file_key = path_util::normalize_rel_path(&rel_path.to_string_lossy());

        let tmp_dir = self.snapshots_dir().join(".tmp");
        std::fs::create_dir_all(&tmp_dir)?;
        let tmp_path = tmp_dir.join(uuid::Uuid::new_v4().to_string());

        let (checksum, size) = match self.stream_hash_and_save(file_path, &tmp_path)? {
            Some(v) => v,
            None => {
                std::fs::remove_file(&tmp_path).ok();
                return Ok(None);
            }
        };

        if size == 0 {
            std::fs::remove_file(&tmp_path).ok();
            return Ok(None);
        }

        let last_entry = view.last_entry_for_file(index, &file_key);
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

        let mtime_nanos = std::fs::metadata(file_path)
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_nanos() as i64);

        let entry = HistoryEntry {
            timestamp: Utc::now(),
            op,
            file: file_key,
            checksum: Some(checksum),
            size: Some(size),
            mtime_nanos,
        };

        index.history.push(entry.clone());
        view.update_last_for_file(entry.file.clone(), index.history.len() - 1);
        Ok(Some(entry))
    }

    pub fn record_delete_with_index(
        &self,
        file_path: &Path,
        root_dir: &Path,
        index: &mut Index,
        view: &mut IndexView,
    ) -> Result<Option<HistoryEntry>> {
        let rel_path = file_path.strip_prefix(root_dir).unwrap_or(file_path);
        let file_key = path_util::normalize_rel_path(&rel_path.to_string_lossy());

        if !view.last_by_file.contains_key(&file_key) {
            return Ok(None);
        }

        let entry = HistoryEntry {
            timestamp: Utc::now(),
            op: Operation::Delete,
            file: file_key,
            checksum: None,
            size: None,
            mtime_nanos: None,
        };

        index.history.push(entry.clone());
        view.update_last_for_file(entry.file.clone(), index.history.len() - 1);
        Ok(Some(entry))
    }

    /// Record delete for every file in the index whose path equals or is under `path_prefix`.
    /// Used when a directory (or single file) is removed/renamed so all tracked files under
    /// that path get a delete entry. Returns the number of delete entries added.
    #[allow(dead_code)]
    pub fn record_deletes_under_prefix(
        &self,
        path_prefix: &Path,
        root_dir: &Path,
    ) -> Result<usize> {
        let mut index = self.load_index()?;
        let mut view = IndexView::from_index(&index);
        let count = self.record_deletes_under_prefix_with_index(
            path_prefix,
            root_dir,
            &mut index,
            &mut view,
        )?;
        if count > 0 {
            self.save_index(&index)?;
        }
        Ok(count)
    }

    /// Like `record_deletes_under_prefix` but operates on a caller-owned Index + IndexView,
    /// avoiding redundant load/save when batching multiple operations.
    pub fn record_deletes_under_prefix_with_index(
        &self,
        path_prefix: &Path,
        root_dir: &Path,
        index: &mut Index,
        view: &mut IndexView,
    ) -> Result<usize> {
        let rel_prefix = path_prefix.strip_prefix(root_dir).unwrap_or(path_prefix);
        let rel_prefix_str = rel_prefix.to_string_lossy().replace('\\', "/");
        let rel_prefix_trimmed = rel_prefix_str.trim_end_matches('/');
        if rel_prefix_trimmed.is_empty() {
            return Ok(0);
        }
        let prefix_with_slash = format!("{}/", rel_prefix_trimmed);

        // Use IndexView for O(1) last-entry lookup instead of O(n) linear scan
        let files_to_delete: Vec<String> = view
            .last_by_file
            .iter()
            .filter_map(|(file_key, &idx)| {
                let file_norm = file_key.replace('\\', "/");
                if file_norm == rel_prefix_trimmed || file_norm.starts_with(&prefix_with_slash) {
                    if index
                        .history
                        .get(idx)
                        .map(|e| e.op != Operation::Delete)
                        .unwrap_or(false)
                    {
                        return Some(file_key.clone());
                    }
                }
                None
            })
            .collect();

        let count = files_to_delete.len();
        for file_key in files_to_delete {
            let entry = HistoryEntry {
                timestamp: Utc::now(),
                op: Operation::Delete,
                file: file_key,
                checksum: None,
                size: None,
                mtime_nanos: None,
            };
            index.history.push(entry.clone());
            view.update_last_for_file(entry.file.clone(), index.history.len() - 1);
        }
        Ok(count)
    }

    /// Trim oldest history entries until both max_history and max_quota are satisfied.
    /// Removes snapshot files that become unreferenced.
    /// Returns (entries_removed, bytes_freed).
    pub(crate) fn trim_history_and_quota(&self, index: &mut Index) -> Result<(usize, u64)> {
        let n = index.history.len();
        if n == 0 {
            return Ok((0, 0));
        }

        let mut checksum_size: HashMap<String, u64> = HashMap::new();
        let mut ref_count: HashMap<String, usize> = HashMap::new();
        for entry in &index.history {
            if let Some(ref c) = entry.checksum {
                *ref_count.entry(c.clone()).or_default() += 1;
                if !checksum_size.contains_key(c) {
                    let size = entry.size.unwrap_or_else(|| {
                        std::fs::metadata(self.snapshot_path(c))
                            .map(|m| m.len())
                            .unwrap_or(0)
                    });
                    checksum_size.insert(c.clone(), size);
                }
            }
        }
        let mut total_volume: u64 = checksum_size.values().sum();

        let mut to_remove = 0usize;
        while (n - to_remove > self.max_history || total_volume > self.max_quota) && to_remove < n {
            let entry = &index.history[to_remove];
            if let Some(ref c) = entry.checksum {
                if let Some(count) = ref_count.get_mut(c) {
                    *count = count.saturating_sub(1);
                    if *count == 0 {
                        if let Some(&size) = checksum_size.get(c) {
                            total_volume = total_volume.saturating_sub(size);
                        }
                    }
                }
            }
            to_remove += 1;
        }

        if to_remove == 0 {
            return Ok((0, 0));
        }

        let snapshots_to_delete: HashSet<String> = index.history[..to_remove]
            .iter()
            .filter_map(|e| e.checksum.as_ref().cloned())
            .collect();
        let mut bytes_freed = 0u64;
        for c in &snapshots_to_delete {
            if ref_count.get(c).copied().unwrap_or(0) == 0 {
                if let Some(&size) = checksum_size.get(c) {
                    bytes_freed += size;
                }
            }
        }
        index.history.drain(0..to_remove);

        for c in &snapshots_to_delete {
            if ref_count.get(c).copied().unwrap_or(0) == 0 {
                let path = self.snapshot_path(c);
                let _ = std::fs::remove_file(&path);
            }
        }

        Ok((to_remove, bytes_freed))
    }

    /// Run full clean: trim history/quota then remove orphan snapshots.
    /// Returns combined stats (trim + orphan).
    pub fn clean(&self) -> Result<CleanResult> {
        let mut index = self.load_index()?;
        let (entries_trimmed, bytes_freed_trim) = self.trim_history_and_quota(&mut index)?;
        if entries_trimmed > 0 {
            self.save_index(&index)?;
        }
        let (files_removed, bytes_removed) = self.clean_orphan_snapshots_inner()?;
        Ok(CleanResult {
            entries_trimmed,
            bytes_freed_trim,
            files_removed,
            bytes_removed,
        })
    }

    /// Read the raw bytes of a snapshot by its full checksum.
    pub fn read_snapshot(&self, checksum: &str) -> Result<Vec<u8>> {
        let path = self.snapshot_path(checksum);
        if !path.exists() {
            anyhow::bail!("Snapshot not found: {}", &checksum[..8.min(checksum.len())]);
        }
        let content = std::fs::read(&path)?;
        Ok(content)
    }

    /// Check whether a snapshot file exists for the given checksum.
    #[allow(dead_code)]
    pub fn snapshot_exists(&self, checksum: &str) -> bool {
        self.snapshot_path(checksum).exists()
    }

    /// Remove snapshot files that are not referenced by any HistoryEntry in the index.
    /// Returns (files_removed, bytes_removed). Skips `.tmp/` under snapshots.
    fn clean_orphan_snapshots_inner(&self) -> Result<(usize, u64)> {
        let index = self.load_index()?;
        let referenced: HashSet<String> = index
            .history
            .iter()
            .filter_map(|e| e.checksum.clone())
            .collect();

        let snap_dir = self.snapshots_dir();
        if !snap_dir.exists() {
            return Ok((0, 0));
        }

        let mut files_removed = 0usize;
        let mut bytes_removed = 0u64;
        let to_delete = Self::collect_orphan_snapshot_paths(&snap_dir, &referenced)?;
        for path in to_delete {
            if let Ok(meta) = std::fs::metadata(&path) {
                bytes_removed += meta.len();
            }
            std::fs::remove_file(&path).context("Failed to remove orphan snapshot")?;
            files_removed += 1;
        }

        Ok((files_removed, bytes_removed))
    }

    /// Returns true if s is exactly 64 hex chars (SHA-256).
    fn is_sha256_hex(s: &str) -> bool {
        s.len() == 64 && s.chars().all(|c| c.is_ascii_hexdigit())
    }

    /// Recursively collect paths of snapshot files whose checksum is not in referenced. Skips .tmp.
    fn collect_orphan_snapshot_paths(
        dir: &Path,
        referenced: &HashSet<String>,
    ) -> Result<Vec<PathBuf>> {
        let mut out = Vec::new();
        for entry in std::fs::read_dir(dir).context("Failed to read snapshots directory")? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                if path.file_name().map(|n| n == ".tmp").unwrap_or(false) {
                    continue;
                }
                out.extend(Self::collect_orphan_snapshot_paths(&path, referenced)?);
            } else if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if Self::is_sha256_hex(name) && !referenced.contains(name) {
                    out.push(path);
                }
            }
        }
        Ok(out)
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

    /// Return all history entries within the given time range.
    /// Both `since` and `until` are inclusive bounds.
    /// When `include_deleted` is false, entries for files whose last history entry is Delete are excluded.
    pub fn list_activity(
        &self,
        since: DateTime<Utc>,
        until: DateTime<Utc>,
        include_deleted: bool,
    ) -> Result<Vec<HistoryEntry>> {
        let index = self.load_index()?;
        let mut entries: Vec<HistoryEntry> = index
            .history
            .iter()
            .filter(|e| e.timestamp >= since && e.timestamp <= until)
            .cloned()
            .collect();
        if !include_deleted {
            entries.retain(|e| {
                self.get_last_entry_for_file(&index, &e.file)
                    .map(|last| last.op != Operation::Delete)
                    .unwrap_or(true)
            });
        }
        Ok(entries)
    }

    pub fn list_files(&self, include_deleted: bool) -> Result<Vec<(String, usize)>> {
        use std::collections::HashMap;

        let index = self.load_index()?;
        let mut file_counts: HashMap<String, usize> = HashMap::new();

        for entry in &index.history {
            *file_counts.entry(entry.file.clone()).or_insert(0) += 1;
        }

        let mut files: Vec<(String, usize)> = if include_deleted {
            file_counts.into_iter().collect()
        } else {
            file_counts
                .into_iter()
                .filter(|(file, _)| {
                    self.get_last_entry_for_file(&index, file)
                        .map(|e| e.op != Operation::Delete)
                        .unwrap_or(true)
                })
                .collect()
        };
        files.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(files)
    }

    /// Path segments from a path string using platform-agnostic Path::components().
    fn path_segments(path_str: &str) -> Vec<String> {
        Path::new(path_str)
            .components()
            .filter_map(|c| match c {
                Component::Normal(s) => Some(s.to_string_lossy().into_owned()),
                _ => None,
            })
            .collect()
    }

    pub fn list_files_tree(&self, include_deleted: bool) -> Result<Vec<FileTreeNode>> {
        let flat = self.list_files(include_deleted)?;
        let mut root: BTreeMap<String, BuildNode> = BTreeMap::new();
        for (path_str, count) in flat {
            let segments = Self::path_segments(&path_str);
            if segments.is_empty() {
                continue;
            }
            Self::insert_path(&mut root, &segments, count);
        }
        Ok(Self::build_nodes_to_tree(root))
    }

    fn insert_path(root: &mut BTreeMap<String, BuildNode>, segments: &[String], count: usize) {
        if segments.len() == 1 {
            root.insert(segments[0].clone(), BuildNode::File(count));
            return;
        }
        let (name, rest) = (&segments[0], &segments[1..]);
        let entry = root
            .entry(name.clone())
            .or_insert_with(|| BuildNode::Dir(BTreeMap::new()));
        match entry {
            BuildNode::File(_) => {
                *entry = BuildNode::Dir(BTreeMap::new());
                if let BuildNode::Dir(ref mut map) = entry {
                    Self::insert_path(map, rest, count);
                }
            }
            BuildNode::Dir(ref mut map) => {
                Self::insert_path(map, rest, count);
            }
        }
    }

    fn build_nodes_to_tree(nodes: BTreeMap<String, BuildNode>) -> Vec<FileTreeNode> {
        nodes
            .into_iter()
            .map(|(name, n)| match n {
                BuildNode::File(c) => FileTreeNode {
                    name,
                    count: Some(c),
                    children: None,
                },
                BuildNode::Dir(map) => FileTreeNode {
                    name,
                    count: None,
                    children: Some(Self::build_nodes_to_tree(map)),
                },
            })
            .collect()
    }

    pub fn restore(&self, file_path: &str, checksum_prefix: &str, root_dir: &Path) -> Result<()> {
        let index = self.load_index()?;
        let file_path_norm = path_util::normalize_rel_path(file_path);

        // Find entry matching the checksum prefix (compare normalized paths for Windows compatibility)
        let entry = index
            .history
            .iter()
            .find(|e| {
                path_util::normalize_rel_path(&e.file) == file_path_norm
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
