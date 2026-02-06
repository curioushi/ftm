use crate::config::Config;
use crate::storage::Storage;
use anyhow::Result;
use glob::Pattern;
use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;
use tracing::info;

enum WorkerTask {
    Snapshot(PathBuf),
    Delete(PathBuf),
}

pub struct FileWatcher {
    root_dir: PathBuf,
    config: Config,
    _storage: Storage,
}

impl FileWatcher {
    pub fn new(root_dir: PathBuf, config: Config, storage: Storage) -> Self {
        Self {
            root_dir,
            config,
            _storage: storage,
        }
    }

    fn should_watch(&self, path: &Path) -> bool {
        let rel_path = path.strip_prefix(&self.root_dir).unwrap_or(path);
        let path_str = rel_path.to_string_lossy();

        // Check exclude patterns
        for pattern in &self.config.watch.exclude {
            if let Ok(p) = Pattern::new(pattern) {
                if p.matches(&path_str) {
                    return false;
                }
            }
        }

        // Check include patterns
        if let Some(ext) = path.extension() {
            let ext_pattern = format!("*.{}", ext.to_string_lossy());
            for pattern in &self.config.watch.patterns {
                if pattern == &ext_pattern
                    || pattern.ends_with(&format!(".{}", ext.to_string_lossy()))
                {
                    return true;
                }
            }
        }

        false
    }

    fn handle_event(&self, event: Event, task_tx: &mpsc::Sender<WorkerTask>) {
        use notify::event::{AccessKind, AccessMode};
        use notify::EventKind::*;

        match event.kind {
            Access(AccessKind::Close(AccessMode::Write)) => {
                for path in event.paths {
                    if path.is_file() && self.should_watch(&path) {
                        let _ = task_tx.send(WorkerTask::Snapshot(path));
                    }
                }
            }
            Remove(_) => {
                for path in event.paths {
                    if self.should_watch(&path) {
                        let _ = task_tx.send(WorkerTask::Delete(path));
                    }
                }
            }
            _ => {}
        }
    }

    pub fn watch(&self) -> Result<()> {
        let (tx, rx) = mpsc::channel();
        let (task_tx, task_rx) = mpsc::channel();

        let root_dir = self.root_dir.clone();
        let config = self.config.clone();
        thread::spawn(move || {
            let storage = Storage::new(root_dir.join(".ftm"), config.settings.max_history);
            for task in task_rx {
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
