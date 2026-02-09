use crate::config::Config;
use crate::storage::Storage;
use anyhow::Result;
use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Arc, RwLock};
use std::thread;
use tracing::info;

enum WorkerTask {
    Snapshot(PathBuf),
    DeletePrefix(PathBuf),
}

pub struct FileWatcher {
    root_dir: PathBuf,
    config: Arc<RwLock<Config>>,
    _storage: Storage,
}

impl FileWatcher {
    pub fn new(root_dir: PathBuf, config: Arc<RwLock<Config>>, storage: Storage) -> Self {
        Self {
            root_dir,
            config,
            _storage: storage,
        }
    }

    fn should_watch(&self, path: &Path) -> bool {
        let cfg = self.config.read().unwrap();
        cfg.matches_path(path, &self.root_dir)
    }

    /// Recursively walk a directory and send Snapshot for each matching file. Skips .ftm.
    fn walk_dir_and_snapshot(
        watcher: &FileWatcher,
        dir: &Path,
        task_tx: &mpsc::Sender<WorkerTask>,
    ) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if path.file_name().and_then(|n| n.to_str()) != Some(".ftm") {
                    Self::walk_dir_and_snapshot(watcher, &path, task_tx);
                }
            } else if watcher.should_watch(&path) {
                let _ = task_tx.send(WorkerTask::Snapshot(path));
            }
        }
    }

    fn handle_event(&self, event: Event, task_tx: &mpsc::Sender<WorkerTask>) {
        use notify::event::{ModifyKind, RenameMode};

        if matches!(event.kind, notify::EventKind::Remove(_)) {
            // Direct removal (e.g., `rm` command). Use DeletePrefix so directory removal
            // records deletes for all tracked files under that path.
            for path in event.paths {
                if path.starts_with(&self.root_dir) {
                    let _ = task_tx.send(WorkerTask::DeletePrefix(path));
                }
            }
        } else if let notify::EventKind::Modify(ModifyKind::Name(mode)) = event.kind {
            // Rename/move events. Treat RenameMode to avoid relying on filesystem timing.
            let paths = event.paths;
            let handle_from = |path: &PathBuf, task_tx: &mpsc::Sender<WorkerTask>| {
                if path.starts_with(&self.root_dir) {
                    let _ = task_tx.send(WorkerTask::DeletePrefix(path.clone()));
                }
            };
            let handle_to = |path: &PathBuf, task_tx: &mpsc::Sender<WorkerTask>| {
                if !path.starts_with(&self.root_dir) {
                    return;
                }
                if path.is_dir() {
                    Self::walk_dir_and_snapshot(self, path, task_tx);
                } else if path.is_file() && self.should_watch(path) {
                    let _ = task_tx.send(WorkerTask::Snapshot(path.clone()));
                }
            };

            match mode {
                RenameMode::From => {
                    for path in &paths {
                        handle_from(path, task_tx);
                    }
                }
                RenameMode::To => {
                    for path in &paths {
                        handle_to(path, task_tx);
                    }
                }
                RenameMode::Both => {
                    if paths.len() >= 2 {
                        let from = &paths[0];
                        let to = &paths[1];
                        handle_from(from, task_tx);
                        handle_to(to, task_tx);
                    } else {
                        for path in &paths {
                            handle_from(path, task_tx);
                            handle_to(path, task_tx);
                        }
                    }
                }
                _ => {
                    for path in &paths {
                        handle_from(path, task_tx);
                        handle_to(path, task_tx);
                    }
                }
            }
        } else if matches!(event.kind, notify::EventKind::Create(_)) {
            // Ensure newly created directories are scanned in case file events are missed.
            for path in event.paths {
                if path.is_dir() && path.starts_with(&self.root_dir) {
                    Self::walk_dir_and_snapshot(self, &path, task_tx);
                }
            }
        } else if Self::is_snapshot_trigger(&event.kind) {
            for path in event.paths {
                if path.is_file() && self.should_watch(&path) {
                    let _ = task_tx.send(WorkerTask::Snapshot(path));
                }
            }
        }
    }

    /// Check if the event kind should trigger a file snapshot.
    ///
    /// On Linux, `inotify` provides `CloseWrite` which fires once after a
    /// file is fully written — ideal for atomic snapshots.
    ///
    /// On macOS (FSEvents) and Windows, `CloseWrite` is not available.
    /// We use `Create` and `Modify` events instead. The storage layer's
    /// checksum-based deduplication prevents duplicate index entries.
    fn is_snapshot_trigger(kind: &notify::EventKind) -> bool {
        use notify::event::{AccessKind, AccessMode};
        use notify::EventKind::*;

        match kind {
            Access(AccessKind::Close(AccessMode::Write)) => true,
            Create(_) | Modify(_) => cfg!(not(target_os = "linux")),
            _ => false,
        }
    }

    /// Start watching in a background thread (non-blocking).
    /// Returns the JoinHandle for the watcher thread.
    pub fn watch_background(self) -> std::thread::JoinHandle<Result<()>> {
        thread::spawn(move || self.watch())
    }

    pub fn watch(&self) -> Result<()> {
        let (tx, rx) = mpsc::channel();
        let (task_tx, task_rx) = mpsc::channel();

        let root_dir = self.root_dir.clone();
        let config = self.config.clone();
        thread::spawn(move || {
            for task in task_rx {
                // Read max_history from shared config on each task so changes
                // via `config set` are picked up immediately.
                let max_history = config.read().unwrap().settings.max_history;
                let storage = Storage::new(root_dir.join(".ftm"), max_history);
                match task {
                    WorkerTask::Snapshot(path) => {
                        if let Ok(Some(entry)) = storage.save_snapshot(&path, &root_dir) {
                            info!(
                                "Snapshot saved: {} [{}] checksum={}",
                                entry.file,
                                entry.op,
                                entry.checksum.as_deref().unwrap_or("none")
                            );
                        }
                    }
                    WorkerTask::DeletePrefix(path) => {
                        if let Ok(count) = storage.record_deletes_under_prefix(&path, &root_dir) {
                            if count > 0 {
                                info!(
                                    "Files under {} recorded as deleted ({} entries)",
                                    path.display(),
                                    count
                                );
                            }
                        }
                    }
                }
            }
        });

        let mut watcher = RecommendedWatcher::new(
            move |res: Result<Event, notify::Error>| {
                if let Ok(event) = res {
                    let _ = tx.send(event);
                }
            },
            notify::Config::default(),
        )?;

        watcher.watch(&self.root_dir, RecursiveMode::Recursive)?;
        info!("Watching directory: {}", self.root_dir.display());

        for event in rx {
            self.handle_event(event, &task_tx);
        }

        Ok(())
    }
}
