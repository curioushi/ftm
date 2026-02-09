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
    Delete(PathBuf),
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

    fn handle_event(&self, event: Event, task_tx: &mpsc::Sender<WorkerTask>) {
        use notify::event::ModifyKind;

        if matches!(event.kind, notify::EventKind::Remove(_)) {
            // Direct removal (e.g., `rm` command)
            for path in event.paths {
                if self.should_watch(&path) {
                    let _ = task_tx.send(WorkerTask::Delete(path));
                }
            }
        } else if matches!(event.kind, notify::EventKind::Modify(ModifyKind::Name(_))) {
            // Rename/move events.  File managers (e.g. Finder on macOS,
            // Nautilus on Linux, Explorer on Windows) often "delete" by
            // moving files to a trash folder, which the OS reports as a
            // rename rather than a removal.  If the file no longer exists
            // at its original path, treat it as a delete; if a new file
            // appears (moved into the watched tree), snapshot it.
            for path in event.paths {
                if self.should_watch(&path) {
                    if path.is_file() {
                        let _ = task_tx.send(WorkerTask::Snapshot(path));
                    } else if !path.exists() {
                        let _ = task_tx.send(WorkerTask::Delete(path));
                    }
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
                    WorkerTask::Delete(path) => {
                        if let Ok(Some(entry)) = storage.record_delete(&path, &root_dir) {
                            info!("File deleted: {}", entry.file);
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
