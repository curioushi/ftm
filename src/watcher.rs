use crate::config::Config;
use crate::scanner::Scanner;
use crate::storage::Storage;
use anyhow::Result;
use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};
use std::path::PathBuf;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, RwLock};
use std::thread;
use std::time::{Duration, Instant};
use tracing::info;

pub struct FileWatcher {
    root_dir: PathBuf,
    config: Arc<RwLock<Config>>,
}

impl FileWatcher {
    pub fn new(root_dir: PathBuf, config: Arc<RwLock<Config>>) -> Self {
        Self { root_dir, config }
    }

    /// Start watching in a background thread (non-blocking).
    /// Returns the JoinHandle for the watcher thread.
    pub fn watch_background(self) -> std::thread::JoinHandle<Result<()>> {
        thread::spawn(move || self.watch())
    }

    pub fn watch(&self) -> Result<()> {
        let (tx, rx) = mpsc::channel();
        let ftm_dir = self.root_dir.join(".ftm");

        let _watcher = {
            let mut w = RecommendedWatcher::new(
                move |res: Result<Event, notify::Error>| {
                    if let Ok(event) = res {
                        let _ = tx.send(event);
                    }
                },
                notify::Config::default(),
            )?;
            w.watch(&self.root_dir, RecursiveMode::Recursive)?;
            w
        };

        info!("Watching directory: {}", self.root_dir.display());

        loop {
            // Block until a relevant event arrives.
            // Skip:
            //  - Events whose paths are all inside .ftm/ (internal writes)
            //  - Access/Other events (only react to actual mutations)
            match rx.recv() {
                Ok(event) => {
                    if !Self::is_mutation(&event.kind) {
                        continue;
                    }
                    if event.paths.iter().all(|p| p.starts_with(&ftm_dir)) {
                        continue;
                    }
                }
                Err(_) => break, // channel closed
            }

            // Debounce: drain events until 500ms of silence.
            // Only non-.ftm mutation events reset the deadline; irrelevant
            // events (Access, .ftm writes) are consumed without extending it.
            let mut deadline = Instant::now() + Duration::from_millis(500);
            loop {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    break;
                }
                match rx.recv_timeout(remaining) {
                    Ok(event) => {
                        if Self::is_mutation(&event.kind)
                            && !event.paths.iter().all(|p| p.starts_with(&ftm_dir))
                        {
                            // Relevant mutation — reset deadline
                            deadline = Instant::now() + Duration::from_millis(500);
                        }
                        // Irrelevant events consumed without resetting deadline
                    }
                    Err(RecvTimeoutError::Timeout) => break,
                    Err(RecvTimeoutError::Disconnected) => return Ok(()),
                }
            }

            // Perform a full directory scan to detect creates, modifies, and deletes
            let cfg = {
                let c = self.config.read().unwrap();
                c.clone()
            };
            let storage = Storage::for_settings(ftm_dir.clone(), &cfg.settings);
            match Scanner::new(self.root_dir.clone(), cfg, storage).scan() {
                Ok(r) => {
                    info!(
                        "Watcher scan: +{} ~{} -{} ={}",
                        r.created, r.modified, r.deleted, r.unchanged
                    );
                }
                Err(e) => {
                    tracing::warn!("Watcher scan error: {}", e);
                }
            }
        }

        Ok(())
    }

    /// Returns true for event kinds that represent actual filesystem mutations
    /// (create, modify, remove, rename). Access and Other events are ignored.
    fn is_mutation(kind: &notify::EventKind) -> bool {
        matches!(
            kind,
            notify::EventKind::Create(_)
                | notify::EventKind::Modify(_)
                | notify::EventKind::Remove(_)
        )
    }
}
