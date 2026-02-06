//! Integration tests for ftm CLI commands.
//!
//! Run with: cargo test --release -- --test-threads=1

use assert_cmd::Command;
use predicates::prelude::*;
use serde::Deserialize;
use std::path::Path;
use tempfile::tempdir;

/// Helper to get ftm command (uses path set by cargo test)
fn ftm() -> Command {
    Command::from_std(std::process::Command::new(env!("CARGO_BIN_EXE_ftm")))
}

/// Guard that keeps the temp dir on test failure and prints its path.
struct TestDirGuard {
    inner: Option<tempfile::TempDir>,
}

impl TestDirGuard {
    fn path(&self) -> &Path {
        self.inner.as_ref().unwrap().path()
    }
}

impl Drop for TestDirGuard {
    fn drop(&mut self) {
        if std::thread::panicking() {
            if let Some(temp_dir) = self.inner.take() {
                let p = temp_dir.keep();
                eprintln!(
                    "\nTest failed. Temporary directory preserved at: {}",
                    p.display()
                );
            }
        }
    }
}

/// Create a test directory. On test failure the dir is preserved and its path is printed.
fn setup_test_dir() -> TestDirGuard {
    TestDirGuard {
        inner: Some(tempdir().unwrap()),
    }
}

// ---------------------------------------------------------------------------
// New helpers
// ---------------------------------------------------------------------------

/// Spawn `ftm watch` in the given directory, returning the child process.
/// Sleeps briefly to let the watcher initialize.
fn start_watch(dir: &Path) -> std::process::Child {
    let child = std::process::Command::new(env!("CARGO_BIN_EXE_ftm"))
        .current_dir(dir)
        .arg("watch")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("failed to spawn ftm watch");
    std::thread::sleep(std::time::Duration::from_millis(50));
    child
}

/// Minimal deserialization structs for index.json (avoids needing chrono).
#[derive(Debug, Deserialize)]
struct TestIndex {
    history: Vec<TestHistoryEntry>,
}

#[derive(Debug, Deserialize)]
struct TestHistoryEntry {
    op: String,
    file: String,
    #[serde(default)]
    checksum: Option<String>,
    #[serde(default)]
    size: Option<u64>,
}

/// Load and parse `.ftm/index.json` from the test directory.
fn load_test_index(dir: &Path) -> TestIndex {
    let content =
        std::fs::read_to_string(dir.join(".ftm/index.json")).expect("failed to read index.json");
    serde_json::from_str(&content).expect("failed to parse index.json")
}

/// Poll index.json until `file` has at least `min_count` entries, or timeout.
/// Returns true if the condition was met before the timeout.
fn wait_for_index(dir: &Path, file: &str, min_count: usize, timeout_ms: u64) -> bool {
    let index_path = dir.join(".ftm/index.json");
    let start = std::time::Instant::now();
    loop {
        if let Ok(content) = std::fs::read_to_string(&index_path) {
            if let Ok(index) = serde_json::from_str::<TestIndex>(&content) {
                let count = index.history.iter().filter(|e| e.file == file).count();
                if count >= min_count {
                    return true;
                }
            }
        }
        if start.elapsed().as_millis() as u64 > timeout_ms {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

/// Count snapshot files (non-directory entries) under `.ftm/snapshots/`, excluding `.tmp/`.
fn count_snapshot_files(dir: &Path) -> usize {
    let snapshots_dir = dir.join(".ftm/snapshots");
    if !snapshots_dir.exists() {
        return 0;
    }
    count_files_recursive(&snapshots_dir)
}

fn count_files_recursive(dir: &Path) -> usize {
    let mut count = 0;
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                // Skip .tmp directory
                if path.file_name().map(|n| n == ".tmp").unwrap_or(false) {
                    continue;
                }
                count += count_files_recursive(&path);
            } else {
                count += 1;
            }
        }
    }
    count
}

// ===========================================================================
// Test modules
// ===========================================================================

mod init_tests {
    use super::*;

    #[test]
    fn test_init_creates_ftm_directory() {
        let dir = setup_test_dir();

        ftm()
            .current_dir(dir.path())
            .arg("init")
            .assert()
            .success()
            .stdout(predicate::str::contains("Initialized .ftm"));

        assert!(dir.path().join(".ftm").exists());
        assert!(dir.path().join(".ftm/config.yaml").exists());
        assert!(dir.path().join(".ftm/index.json").exists());
    }

    #[test]
    fn test_init_already_initialized() {
        let dir = setup_test_dir();

        // First init
        ftm().current_dir(dir.path()).arg("init").assert().success();

        // Second init should say already initialized
        ftm()
            .current_dir(dir.path())
            .arg("init")
            .assert()
            .success()
            .stdout(predicate::str::contains("Already initialized"));
    }
}

mod ls_tests {
    use super::*;

    #[test]
    fn test_ls_not_initialized() {
        let dir = setup_test_dir();

        ftm()
            .current_dir(dir.path())
            .arg("ls")
            .assert()
            .failure()
            .stderr(predicate::str::contains("Not initialized"));
    }

    #[test]
    fn test_ls_empty() {
        let dir = setup_test_dir();

        ftm().current_dir(dir.path()).arg("init").assert().success();

        ftm()
            .current_dir(dir.path())
            .arg("ls")
            .assert()
            .success()
            .stdout(predicate::str::contains("No files tracked yet"));
    }

    #[test]
    fn test_ls_after_watch_and_write() {
        let dir = setup_test_dir();

        ftm().current_dir(dir.path()).arg("init").assert().success();

        let mut watch_child = std::process::Command::new(env!("CARGO_BIN_EXE_ftm"))
            .current_dir(dir.path())
            .arg("watch")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("failed to spawn ftm watch");

        std::thread::sleep(std::time::Duration::from_millis(10));

        std::fs::write(dir.path().join("a.yaml"), "a: 1").unwrap();
        std::fs::write(dir.path().join("b.yaml"), "b: 2").unwrap();
        std::fs::write(dir.path().join("c.yaml"), "c: 3").unwrap();

        std::thread::sleep(std::time::Duration::from_millis(10));

        ftm()
            .current_dir(dir.path())
            .arg("ls")
            .assert()
            .success()
            .stdout(predicate::str::contains("a.yaml"))
            .stdout(predicate::str::contains("b.yaml"))
            .stdout(predicate::str::contains("c.yaml"));

        let _ = watch_child.kill();
    }

    /// Helper: write `num_writes` versions of a 5 MB file with `interval_ms` between writes,
    /// then return (entry_count, all_sizes_correct).
    fn write_and_check(num_writes: usize, interval_ms: u64) -> (usize, bool) {
        let dir = setup_test_dir();

        ftm().current_dir(dir.path()).arg("init").assert().success();

        let mut watch_child = std::process::Command::new(env!("CARGO_BIN_EXE_ftm"))
            .current_dir(dir.path())
            .arg("watch")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("failed to spawn ftm watch");

        std::thread::sleep(std::time::Duration::from_millis(10));

        let file_path = dir.path().join("data.yaml");
        const FILE_SIZE_MB: usize = 5;
        let target_size = FILE_SIZE_MB * 1024 * 1024;
        for i in 0..num_writes {
            let header = format!("version: {}\n", i);
            let padding = target_size.saturating_sub(header.len());
            let content = format!("{}{}", header, "x".repeat(padding));
            std::fs::write(&file_path, &content).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(interval_ms));
        }

        // Parse entry count from ls output
        let ls_output = ftm()
            .current_dir(dir.path())
            .arg("ls")
            .output()
            .expect("failed to run ftm ls");
        assert!(ls_output.status.success(), "ls command failed");
        let ls_stdout = String::from_utf8_lossy(&ls_output.stdout);
        assert!(
            ls_stdout.contains("data.yaml"),
            "Expected data.yaml in ls output.\nls output:\n{}",
            ls_stdout
        );
        let entry_count: usize = ls_stdout
            .lines()
            .find(|l| l.contains("data.yaml"))
            .and_then(|l| {
                l.trim()
                    .strip_prefix("data.yaml (")?
                    .strip_suffix(" entries)")?
                    .parse()
                    .ok()
            })
            .expect("failed to parse entry count from ls output");

        // Check every recorded entry has the correct file size
        let expected_size_str = format!("{} bytes", target_size);
        let history_output = ftm()
            .current_dir(dir.path())
            .args(["history", "data.yaml"])
            .output()
            .expect("failed to run ftm history");
        assert!(history_output.status.success(), "history command failed");
        let history_stdout = String::from_utf8_lossy(&history_output.stdout);

        let total_size_entries = history_stdout
            .lines()
            .filter(|l| l.contains(" bytes"))
            .count();
        let correct_size_entries = history_stdout.matches(&expected_size_str).count();
        assert_eq!(
            correct_size_entries, total_size_entries,
            "All entries must have size '{}', but {}/{} matched.\nHistory output:\n{}",
            expected_size_str, correct_size_entries, total_size_entries, history_stdout
        );

        let _ = watch_child.kill();
        (entry_count, correct_size_entries == total_size_entries)
    }

    #[test]
    fn test_low_freq_writes_exact_entry_count() {
        const NUM_WRITES: usize = 50;
        const INTERVAL_MS: u64 = 20;

        let (entry_count, _) = write_and_check(NUM_WRITES, INTERVAL_MS);
        assert_eq!(
            entry_count, NUM_WRITES,
            "Low-freq ({}ms): expected exactly {} entries, got {}",
            INTERVAL_MS, NUM_WRITES, entry_count
        );
    }

    #[test]
    fn test_high_freq_writes_no_corrupt_entries() {
        const NUM_WRITES: usize = 50;
        const INTERVAL_MS: u64 = 5;

        let (entry_count, _) = write_and_check(NUM_WRITES, INTERVAL_MS);
        assert!(
            entry_count > 0 && entry_count <= NUM_WRITES,
            "High-freq ({}ms): expected 1..={} entries, got {}",
            INTERVAL_MS,
            NUM_WRITES,
            entry_count
        );
    }
}

mod watch_tests {
    use super::*;

    #[test]
    fn test_watch_not_initialized() {
        let dir = setup_test_dir();

        ftm()
            .current_dir(dir.path())
            .arg("watch")
            .timeout(std::time::Duration::from_secs(3))
            .assert()
            .failure()
            .stderr(predicate::str::contains("Not initialized"));
    }

    #[test]
    fn test_excluded_files_not_tracked() {
        let dir = setup_test_dir();
        ftm().current_dir(dir.path()).arg("init").assert().success();

        let mut watch = start_watch(dir.path());

        // Write a file inside .ftm/ (excluded by default)
        std::fs::write(dir.path().join(".ftm/sneaky.yaml"), "should: ignore").unwrap();
        // Write a non-matching extension file
        std::fs::write(dir.path().join("data.bin"), "binary stuff").unwrap();
        // Write a tracked file as a reference (to confirm watcher is running)
        std::fs::write(dir.path().join("tracked.yaml"), "key: value").unwrap();

        assert!(
            wait_for_index(dir.path(), "tracked.yaml", 1, 2000),
            "tracked.yaml should be recorded"
        );

        let index = load_test_index(dir.path());
        assert!(
            !index.history.iter().any(|e| e.file.contains("sneaky.yaml")),
            "Files inside .ftm/ should not be tracked"
        );
        assert!(
            !index.history.iter().any(|e| e.file.contains("data.bin")),
            "Non-matching extension files should not be tracked"
        );

        let _ = watch.kill();
    }

    #[test]
    fn test_non_matching_extension_ignored() {
        let dir = setup_test_dir();
        ftm().current_dir(dir.path()).arg("init").assert().success();

        let mut watch = start_watch(dir.path());

        std::fs::write(dir.path().join("app.exe"), "not tracked").unwrap();
        std::fs::write(dir.path().join("image.png"), "not tracked").unwrap();
        // Reference file to prove watcher is running
        std::fs::write(dir.path().join("ref.rs"), "fn main() {}").unwrap();

        assert!(
            wait_for_index(dir.path(), "ref.rs", 1, 2000),
            "ref.rs should be recorded"
        );

        let index = load_test_index(dir.path());
        assert!(
            !index.history.iter().any(|e| e.file == "app.exe"),
            ".exe files should not be tracked"
        );
        assert!(
            !index.history.iter().any(|e| e.file == "image.png"),
            ".png files should not be tracked"
        );

        let _ = watch.kill();
    }

    #[test]
    fn test_subdirectory_files_tracked() {
        let dir = setup_test_dir();
        ftm().current_dir(dir.path()).arg("init").assert().success();

        let sub_dir = dir.path().join("sub/deep");
        std::fs::create_dir_all(&sub_dir).unwrap();

        let mut watch = start_watch(dir.path());

        std::fs::write(sub_dir.join("foo.rs"), "fn hello() {}").unwrap();

        assert!(
            wait_for_index(dir.path(), "sub/deep/foo.rs", 1, 2000),
            "sub/deep/foo.rs should be recorded with relative path"
        );

        ftm()
            .current_dir(dir.path())
            .arg("ls")
            .assert()
            .success()
            .stdout(predicate::str::contains("sub/deep/foo.rs"));

        let _ = watch.kill();
    }

    #[test]
    fn test_empty_file_ignored() {
        let dir = setup_test_dir();
        ftm().current_dir(dir.path()).arg("init").assert().success();

        let mut watch = start_watch(dir.path());

        // Write an empty file
        std::fs::write(dir.path().join("empty.txt"), "").unwrap();
        // Write a non-empty reference file
        std::fs::write(dir.path().join("notempty.txt"), "hello").unwrap();

        assert!(
            wait_for_index(dir.path(), "notempty.txt", 1, 2000),
            "notempty.txt should be recorded"
        );

        let index = load_test_index(dir.path());
        assert!(
            !index.history.iter().any(|e| e.file == "empty.txt"),
            "Empty files should not be tracked"
        );

        let _ = watch.kill();
    }

    #[test]
    fn test_watch_creates_default_log_file() {
        let dir = setup_test_dir();
        ftm().current_dir(dir.path()).arg("init").assert().success();

        let mut watch = start_watch(dir.path());

        // Write a file to ensure the watcher is running
        std::fs::write(dir.path().join("probe.yaml"), "key: val").unwrap();
        assert!(
            wait_for_index(dir.path(), "probe.yaml", 1, 2000),
            "probe.yaml should be recorded"
        );

        let _ = watch.kill();

        // Verify log directory and log file exist under .ftm/log/
        let log_dir = dir.path().join(".ftm/log");
        assert!(log_dir.exists(), ".ftm/log/ directory should be created");

        let log_files: Vec<_> = std::fs::read_dir(&log_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.path()
                    .extension()
                    .map(|ext| ext == "log")
                    .unwrap_or(false)
            })
            .collect();
        assert_eq!(
            log_files.len(),
            1,
            "Should have exactly 1 log file, found {}",
            log_files.len()
        );

        // Verify filename format: YYYYMMDD-hhmmss.log
        let name = log_files[0].file_name();
        let name_str = name.to_string_lossy();
        assert!(
            name_str.len() == "YYYYMMDD-hhmmss.log".len(),
            "Log filename '{}' should match YYYYMMDD-hhmmss.log length",
            name_str
        );
        assert!(
            name_str.ends_with(".log"),
            "Log filename '{}' should end with .log",
            name_str
        );
    }

    #[test]
    fn test_watch_custom_log_dir() {
        let dir = setup_test_dir();
        ftm().current_dir(dir.path()).arg("init").assert().success();

        let custom_log_dir = dir.path().join("my-logs");

        let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_ftm"))
            .current_dir(dir.path())
            .args(["watch", "--log-dir", custom_log_dir.to_str().unwrap()])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("failed to spawn ftm watch with --log-dir");

        std::thread::sleep(std::time::Duration::from_millis(50));

        // Write a file to ensure the watcher is running
        std::fs::write(dir.path().join("probe.yaml"), "key: val").unwrap();
        assert!(
            wait_for_index(dir.path(), "probe.yaml", 1, 2000),
            "probe.yaml should be recorded"
        );

        let _ = child.kill();

        // Verify custom log directory exists and contains a log file
        assert!(
            custom_log_dir.exists(),
            "Custom log directory should be created"
        );

        let log_files: Vec<_> = std::fs::read_dir(&custom_log_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.path()
                    .extension()
                    .map(|ext| ext == "log")
                    .unwrap_or(false)
            })
            .collect();
        assert_eq!(
            log_files.len(),
            1,
            "Custom log dir should have exactly 1 log file, found {}",
            log_files.len()
        );

        // Default log dir should NOT have been created
        assert!(
            !dir.path().join(".ftm/log").exists(),
            ".ftm/log/ should not exist when --log-dir is used"
        );
    }
}

mod dedup_tests {
    use super::*;

    #[test]
    fn test_same_content_no_duplicate_entry() {
        let dir = setup_test_dir();
        ftm().current_dir(dir.path()).arg("init").assert().success();

        let mut watch = start_watch(dir.path());

        let content = "key: same_content";

        // First write
        std::fs::write(dir.path().join("dup.yaml"), content).unwrap();
        assert!(
            wait_for_index(dir.path(), "dup.yaml", 1, 2000),
            "First write should be recorded"
        );

        // Second write with identical content
        std::fs::write(dir.path().join("dup.yaml"), content).unwrap();

        // Write a sync marker to ensure the second write was processed by the watcher.
        // The worker thread processes tasks sequentially, so once the sync file
        // appears in the index, the earlier dup.yaml write must have been handled.
        std::fs::write(dir.path().join("sync.yaml"), "sync: marker").unwrap();
        assert!(
            wait_for_index(dir.path(), "sync.yaml", 1, 2000),
            "Sync marker should be recorded"
        );

        let index = load_test_index(dir.path());
        let count = index
            .history
            .iter()
            .filter(|e| e.file == "dup.yaml")
            .count();
        assert_eq!(
            count, 1,
            "Same content written twice should produce only 1 entry, got {}",
            count
        );

        let _ = watch.kill();
    }

    #[test]
    fn test_different_files_same_content_share_snapshot() {
        let dir = setup_test_dir();
        ftm().current_dir(dir.path()).arg("init").assert().success();

        let mut watch = start_watch(dir.path());

        let content = "shared: content_value_12345";
        std::fs::write(dir.path().join("file_a.yaml"), content).unwrap();
        assert!(
            wait_for_index(dir.path(), "file_a.yaml", 1, 2000),
            "file_a.yaml should be recorded"
        );

        std::fs::write(dir.path().join("file_b.yaml"), content).unwrap();
        assert!(
            wait_for_index(dir.path(), "file_b.yaml", 1, 2000),
            "file_b.yaml should be recorded"
        );

        let index = load_test_index(dir.path());
        // Both files should have their own history entries
        assert!(index.history.iter().any(|e| e.file == "file_a.yaml"));
        assert!(index.history.iter().any(|e| e.file == "file_b.yaml"));

        // But there should be only 1 snapshot file (content-addressable dedup)
        let snap_count = count_snapshot_files(dir.path());
        assert_eq!(
            snap_count, 1,
            "Two files with same content should share 1 snapshot, got {}",
            snap_count
        );

        // Both entries should have the same checksum
        let checksum_a = index
            .history
            .iter()
            .find(|e| e.file == "file_a.yaml")
            .and_then(|e| e.checksum.as_ref());
        let checksum_b = index
            .history
            .iter()
            .find(|e| e.file == "file_b.yaml")
            .and_then(|e| e.checksum.as_ref());
        assert_eq!(
            checksum_a, checksum_b,
            "Both files should have the same checksum"
        );

        let _ = watch.kill();
    }
}

mod history_tests {
    use super::*;

    #[test]
    fn test_history_not_initialized() {
        let dir = setup_test_dir();

        ftm()
            .current_dir(dir.path())
            .args(["history", "test.rs"])
            .assert()
            .failure()
            .stderr(predicate::str::contains("Not initialized"));
    }

    #[test]
    fn test_history_no_entries() {
        let dir = setup_test_dir();

        ftm().current_dir(dir.path()).arg("init").assert().success();

        ftm()
            .current_dir(dir.path())
            .args(["history", "nonexistent.rs"])
            .assert()
            .success()
            .stdout(predicate::str::contains("No history for"));
    }
}

mod history_ops_tests {
    use super::*;

    #[test]
    fn test_history_create_then_modify_ops() {
        let dir = setup_test_dir();
        ftm().current_dir(dir.path()).arg("init").assert().success();

        let mut watch = start_watch(dir.path());
        let file_path = dir.path().join("ops.yaml");

        // Create
        std::fs::write(&file_path, "version: 1").unwrap();
        assert!(wait_for_index(dir.path(), "ops.yaml", 1, 2000));

        // Modify
        std::fs::write(&file_path, "version: 2").unwrap();
        assert!(wait_for_index(dir.path(), "ops.yaml", 2, 2000));

        let index = load_test_index(dir.path());
        let entries: Vec<_> = index
            .history
            .iter()
            .filter(|e| e.file == "ops.yaml")
            .collect();
        assert_eq!(entries.len(), 2, "Should have 2 entries");
        assert_eq!(entries[0].op, "create", "First op should be create");
        assert_eq!(entries[1].op, "modify", "Second op should be modify");

        let _ = watch.kill();
    }

    #[test]
    fn test_history_delete_recorded() {
        let dir = setup_test_dir();
        ftm().current_dir(dir.path()).arg("init").assert().success();

        let mut watch = start_watch(dir.path());
        let file_path = dir.path().join("todelete.yaml");

        // Create
        std::fs::write(&file_path, "will be deleted").unwrap();
        assert!(wait_for_index(dir.path(), "todelete.yaml", 1, 2000));

        // Delete
        std::fs::remove_file(&file_path).unwrap();
        // Wait for delete to be recorded (entry count should become 2)
        assert!(
            wait_for_index(dir.path(), "todelete.yaml", 2, 2000),
            "Delete event should be recorded"
        );

        let index = load_test_index(dir.path());
        let entries: Vec<_> = index
            .history
            .iter()
            .filter(|e| e.file == "todelete.yaml")
            .collect();
        assert_eq!(entries.len(), 2, "Should have 2 entries (create + delete)");
        assert_eq!(entries[0].op, "create");
        assert_eq!(entries[1].op, "delete");
        assert!(
            entries[1].checksum.is_none(),
            "Delete entry should have no checksum"
        );
        assert!(
            entries[1].size.is_none(),
            "Delete entry should have no size"
        );

        let _ = watch.kill();
    }

    #[test]
    fn test_history_recreate_after_delete() {
        let dir = setup_test_dir();
        ftm().current_dir(dir.path()).arg("init").assert().success();

        let mut watch = start_watch(dir.path());
        let file_path = dir.path().join("recreate.yaml");

        // Create
        std::fs::write(&file_path, "original content").unwrap();
        assert!(wait_for_index(dir.path(), "recreate.yaml", 1, 2000));

        // Delete
        std::fs::remove_file(&file_path).unwrap();
        assert!(wait_for_index(dir.path(), "recreate.yaml", 2, 2000));

        // Recreate with new content
        std::fs::write(&file_path, "recreated content").unwrap();
        assert!(wait_for_index(dir.path(), "recreate.yaml", 3, 2000));

        let index = load_test_index(dir.path());
        let entries: Vec<_> = index
            .history
            .iter()
            .filter(|e| e.file == "recreate.yaml")
            .collect();
        assert_eq!(entries.len(), 3, "Should have 3 entries");
        assert_eq!(entries[0].op, "create", "First should be create");
        assert_eq!(entries[1].op, "delete", "Second should be delete");
        assert_eq!(
            entries[2].op, "create",
            "Third should be create (after delete)"
        );

        let _ = watch.kill();
    }

    #[test]
    fn test_history_multiple_files_independent() {
        let dir = setup_test_dir();
        ftm().current_dir(dir.path()).arg("init").assert().success();

        let mut watch = start_watch(dir.path());

        std::fs::write(dir.path().join("alpha.yaml"), "a: 1").unwrap();
        std::fs::write(dir.path().join("beta.yaml"), "b: 1").unwrap();
        assert!(wait_for_index(dir.path(), "alpha.yaml", 1, 2000));
        assert!(wait_for_index(dir.path(), "beta.yaml", 1, 2000));

        // Modify only alpha
        std::fs::write(dir.path().join("alpha.yaml"), "a: 2").unwrap();
        assert!(wait_for_index(dir.path(), "alpha.yaml", 2, 2000));

        let index = load_test_index(dir.path());
        let alpha_count = index
            .history
            .iter()
            .filter(|e| e.file == "alpha.yaml")
            .count();
        let beta_count = index
            .history
            .iter()
            .filter(|e| e.file == "beta.yaml")
            .count();
        assert_eq!(
            alpha_count, 2,
            "alpha should have 2 entries (create + modify)"
        );
        assert_eq!(
            beta_count, 1,
            "beta should still have 1 entry (create only)"
        );

        let _ = watch.kill();
    }
}

mod restore_tests {
    use super::*;

    #[test]
    fn test_restore_not_initialized() {
        let dir = setup_test_dir();

        ftm()
            .current_dir(dir.path())
            .args(["restore", "test.rs", "-c", "abc12345"])
            .assert()
            .failure()
            .stderr(predicate::str::contains("Not initialized"));
    }

    #[test]
    fn test_restore_version_not_found() {
        let dir = setup_test_dir();

        ftm().current_dir(dir.path()).arg("init").assert().success();

        ftm()
            .current_dir(dir.path())
            .args(["restore", "test.rs", "-c", "abc12345"])
            .assert()
            .failure()
            .stderr(predicate::str::contains("Version not found"));
    }

    #[test]
    fn test_restore_roundtrip() {
        let dir = setup_test_dir();
        ftm().current_dir(dir.path()).arg("init").assert().success();

        let mut watch = start_watch(dir.path());
        let file_path = dir.path().join("roundtrip.yaml");

        let v1_content = "version: 1\ndata: original";
        let v2_content = "version: 2\ndata: modified";

        // Write v1
        std::fs::write(&file_path, v1_content).unwrap();
        assert!(wait_for_index(dir.path(), "roundtrip.yaml", 1, 2000));

        // Write v2
        std::fs::write(&file_path, v2_content).unwrap();
        assert!(wait_for_index(dir.path(), "roundtrip.yaml", 2, 2000));

        let _ = watch.kill();

        // Get v1's checksum from index
        let index = load_test_index(dir.path());
        let v1_entry = index
            .history
            .iter()
            .find(|e| e.file == "roundtrip.yaml" && e.op == "create")
            .expect("v1 create entry not found");
        let v1_checksum = v1_entry.checksum.as_ref().unwrap();

        // Verify current content is v2
        let current = std::fs::read_to_string(&file_path).unwrap();
        assert_eq!(current, v2_content, "File should currently be v2");

        // Restore v1
        ftm()
            .current_dir(dir.path())
            .args(["restore", "roundtrip.yaml", "-c", v1_checksum])
            .assert()
            .success();

        // Verify content is back to v1
        let restored = std::fs::read_to_string(&file_path).unwrap();
        assert_eq!(
            restored, v1_content,
            "File content should be restored to v1"
        );
    }

    #[test]
    fn test_restore_with_short_checksum_prefix() {
        let dir = setup_test_dir();
        ftm().current_dir(dir.path()).arg("init").assert().success();

        let mut watch = start_watch(dir.path());
        let file_path = dir.path().join("prefix.yaml");

        let original = "data: for_prefix_test";

        std::fs::write(&file_path, original).unwrap();
        assert!(wait_for_index(dir.path(), "prefix.yaml", 1, 2000));

        std::fs::write(&file_path, "data: modified version").unwrap();
        assert!(wait_for_index(dir.path(), "prefix.yaml", 2, 2000));

        let _ = watch.kill();

        let index = load_test_index(dir.path());
        let entry = index
            .history
            .iter()
            .find(|e| e.file == "prefix.yaml" && e.op == "create")
            .unwrap();
        let full_checksum = entry.checksum.as_ref().unwrap();
        let short_prefix = &full_checksum[..8];

        // Restore using only the first 8 chars of the checksum
        ftm()
            .current_dir(dir.path())
            .args(["restore", "prefix.yaml", "-c", short_prefix])
            .assert()
            .success();

        let restored = std::fs::read_to_string(&file_path).unwrap();
        assert_eq!(
            restored, original,
            "Restore with 8-char prefix should recover original content"
        );
    }

    #[test]
    fn test_restore_deleted_file() {
        let dir = setup_test_dir();
        ftm().current_dir(dir.path()).arg("init").assert().success();

        // Keep watcher running throughout the entire create → delete → restore cycle
        let mut watch = start_watch(dir.path());
        let file_path = dir.path().join("willdelete.yaml");

        let content = "precious: data";
        std::fs::write(&file_path, content).unwrap();
        assert!(wait_for_index(dir.path(), "willdelete.yaml", 1, 2000));

        // Delete the file and wait for the delete event to be recorded
        std::fs::remove_file(&file_path).unwrap();
        assert!(!file_path.exists(), "File should be deleted");
        assert!(
            wait_for_index(dir.path(), "willdelete.yaml", 2, 2000),
            "Delete event should be recorded"
        );

        // Get the checksum from the create entry
        let index = load_test_index(dir.path());
        let entry = index
            .history
            .iter()
            .find(|e| e.file == "willdelete.yaml" && e.op == "create")
            .unwrap();
        let checksum = entry.checksum.as_ref().unwrap().clone();

        // Restore the deleted file (watcher is still running and will pick this up)
        ftm()
            .current_dir(dir.path())
            .args(["restore", "willdelete.yaml", "-c", &checksum])
            .assert()
            .success();

        assert!(file_path.exists(), "File should be restored after deletion");
        let restored = std::fs::read_to_string(&file_path).unwrap();
        assert_eq!(restored, content, "Restored content should match original");

        // Wait for the watcher to record the restored file as a new create
        // (since the previous entry was delete, the new write becomes create)
        assert!(
            wait_for_index(dir.path(), "willdelete.yaml", 3, 2000),
            "Restored file should be recorded as a new create entry"
        );

        let _ = watch.kill();

        // Verify the full index: create → delete → create
        let index_after = load_test_index(dir.path());
        let entries: Vec<_> = index_after
            .history
            .iter()
            .filter(|e| e.file == "willdelete.yaml")
            .collect();
        assert_eq!(
            entries.len(),
            3,
            "Should have 3 entries: create, delete, create"
        );
        assert_eq!(entries[0].op, "create", "First entry should be create");
        assert_eq!(entries[1].op, "delete", "Second entry should be delete");
        assert_eq!(
            entries[2].op, "create",
            "Third entry (after restore) should be create"
        );

        // The newest create entry must be the last one and its checksum
        // should match the original content
        let last_entry = entries.last().unwrap();
        assert_eq!(last_entry.op, "create", "Latest entry must be create");
        use sha2::{Digest, Sha256};
        let expected_checksum = hex::encode(Sha256::digest(content.as_bytes()));
        assert_eq!(
            last_entry.checksum.as_ref().unwrap(),
            &expected_checksum,
            "Latest create entry checksum should match the original content hash"
        );
    }

    #[test]
    fn test_restore_to_subdirectory() {
        let dir = setup_test_dir();
        ftm().current_dir(dir.path()).arg("init").assert().success();

        let sub_dir = dir.path().join("nested/dir");
        std::fs::create_dir_all(&sub_dir).unwrap();

        let mut watch = start_watch(dir.path());
        let file_path = sub_dir.join("deep.yaml");

        let content = "nested: file content";
        std::fs::write(&file_path, content).unwrap();
        assert!(wait_for_index(dir.path(), "nested/dir/deep.yaml", 1, 2000));

        let _ = watch.kill();

        // Delete the entire subdirectory tree
        std::fs::remove_dir_all(dir.path().join("nested")).unwrap();
        assert!(!file_path.exists());

        let index = load_test_index(dir.path());
        let entry = index
            .history
            .iter()
            .find(|e| e.file == "nested/dir/deep.yaml")
            .unwrap();
        let checksum = entry.checksum.as_ref().unwrap();

        // Restore should recreate parent directories automatically
        ftm()
            .current_dir(dir.path())
            .args(["restore", "nested/dir/deep.yaml", "-c", checksum])
            .assert()
            .success();

        assert!(
            file_path.exists(),
            "File should be restored with parent dirs recreated"
        );
        let restored = std::fs::read_to_string(&file_path).unwrap();
        assert_eq!(restored, content);
    }
}

mod trim_tests {
    use super::*;

    #[test]
    fn test_max_history_trims_old_entries() {
        let dir = setup_test_dir();
        ftm().current_dir(dir.path()).arg("init").assert().success();

        // Override max_history to 3 in config.yaml
        let config_path = dir.path().join(".ftm/config.yaml");
        let config_content = std::fs::read_to_string(&config_path).unwrap();
        let new_config = config_content.replace("max_history: 100", "max_history: 3");
        std::fs::write(&config_path, new_config).unwrap();

        let mut watch = start_watch(dir.path());
        let file_path = dir.path().join("trimme.yaml");

        // Write 5 different versions with delay between each
        for i in 0..5 {
            std::fs::write(&file_path, format!("version: {}", i)).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(150));
        }

        // Write a sync marker to ensure all previous writes were processed.
        // The worker thread is sequential, so once the sync file is indexed,
        // all trimme.yaml writes must have been handled.
        std::fs::write(dir.path().join("sync.yaml"), "sync: done").unwrap();
        assert!(
            wait_for_index(dir.path(), "sync.yaml", 1, 3000),
            "Sync marker should be recorded"
        );

        let _ = watch.kill();

        let index = load_test_index(dir.path());
        let entries: Vec<_> = index
            .history
            .iter()
            .filter(|e| e.file == "trimme.yaml")
            .collect();

        assert!(
            entries.len() <= 3,
            "max_history=3: expected at most 3 entries, got {}",
            entries.len()
        );
        assert!(
            !entries.is_empty(),
            "Should have at least 1 entry for trimme.yaml"
        );

        // If all 5 writes were captured (very likely with 150ms interval),
        // verify the retained entries are the newest 3 versions (2, 3, 4).
        if entries.len() == 3 {
            use sha2::{Digest, Sha256};
            let expected_checksums: Vec<String> = (2..5)
                .map(|i| hex::encode(Sha256::digest(format!("version: {}", i).as_bytes())))
                .collect();
            for (entry, expected) in entries.iter().zip(expected_checksums.iter()) {
                let cs = entry.checksum.as_ref().expect("entry should have checksum");
                assert_eq!(
                    cs, expected,
                    "Trimmed entries should be the newest 3 versions in order"
                );
            }
            // Also verify the oldest versions were indeed trimmed away
            let oldest_checksum = hex::encode(Sha256::digest(b"version: 0"));
            assert!(
                !entries
                    .iter()
                    .any(|e| e.checksum.as_deref() == Some(oldest_checksum.as_str())),
                "Oldest version (version: 0) should have been trimmed"
            );
        }
    }
}

mod scan_tests {
    use super::*;

    #[test]
    fn test_scan_not_initialized() {
        let dir = setup_test_dir();

        ftm()
            .current_dir(dir.path())
            .arg("scan")
            .assert()
            .failure()
            .stderr(predicate::str::contains("Not initialized"));
    }

    #[test]
    fn test_scan_detects_new_files() {
        let dir = setup_test_dir();
        ftm().current_dir(dir.path()).arg("init").assert().success();

        // Create files without the watcher running
        std::fs::write(dir.path().join("hello.rs"), "fn main() {}").unwrap();
        std::fs::write(dir.path().join("world.py"), "print('hi')").unwrap();

        ftm()
            .current_dir(dir.path())
            .arg("scan")
            .assert()
            .success()
            .stdout(predicate::str::contains("2 created"))
            .stdout(predicate::str::contains("0 modified"))
            .stdout(predicate::str::contains("0 deleted"));

        let index = load_test_index(dir.path());
        let entries: Vec<_> = index.history.iter().collect();
        assert_eq!(entries.len(), 2, "Should have 2 entries after scan");
        assert!(entries.iter().all(|e| e.op == "create"));
        assert!(entries.iter().any(|e| e.file == "hello.rs"));
        assert!(entries.iter().any(|e| e.file == "world.py"));
    }

    #[test]
    fn test_scan_detects_modifications() {
        let dir = setup_test_dir();
        ftm().current_dir(dir.path()).arg("init").assert().success();

        // Create baseline
        std::fs::write(dir.path().join("app.rs"), "fn main() {}").unwrap();
        ftm()
            .current_dir(dir.path())
            .arg("scan")
            .assert()
            .success()
            .stdout(predicate::str::contains("1 created"));

        // Modify the file
        std::fs::write(dir.path().join("app.rs"), "fn main() { println!(\"hi\"); }").unwrap();
        ftm()
            .current_dir(dir.path())
            .arg("scan")
            .assert()
            .success()
            .stdout(predicate::str::contains("1 modified"))
            .stdout(predicate::str::contains("0 created"));

        let index = load_test_index(dir.path());
        let entries: Vec<_> = index
            .history
            .iter()
            .filter(|e| e.file == "app.rs")
            .collect();
        assert_eq!(entries.len(), 2, "Should have create + modify");
        assert_eq!(entries[0].op, "create");
        assert_eq!(entries[1].op, "modify");
    }

    #[test]
    fn test_scan_detects_deletions() {
        let dir = setup_test_dir();
        ftm().current_dir(dir.path()).arg("init").assert().success();

        // Create and scan to establish baseline
        std::fs::write(dir.path().join("temp.txt"), "temporary content").unwrap();
        ftm()
            .current_dir(dir.path())
            .arg("scan")
            .assert()
            .success()
            .stdout(predicate::str::contains("1 created"));

        // Delete the file
        std::fs::remove_file(dir.path().join("temp.txt")).unwrap();
        ftm()
            .current_dir(dir.path())
            .arg("scan")
            .assert()
            .success()
            .stdout(predicate::str::contains("1 deleted"))
            .stdout(predicate::str::contains("0 created"));

        let index = load_test_index(dir.path());
        let entries: Vec<_> = index
            .history
            .iter()
            .filter(|e| e.file == "temp.txt")
            .collect();
        assert_eq!(entries.len(), 2, "Should have create + delete");
        assert_eq!(entries[0].op, "create");
        assert_eq!(entries[1].op, "delete");
    }

    #[test]
    fn test_scan_no_changes_second_run() {
        let dir = setup_test_dir();
        ftm().current_dir(dir.path()).arg("init").assert().success();

        std::fs::write(dir.path().join("stable.md"), "# Stable").unwrap();

        // First scan
        ftm()
            .current_dir(dir.path())
            .arg("scan")
            .assert()
            .success()
            .stdout(predicate::str::contains("1 created"));

        // Second scan - nothing changed
        ftm()
            .current_dir(dir.path())
            .arg("scan")
            .assert()
            .success()
            .stdout(predicate::str::contains("0 created"))
            .stdout(predicate::str::contains("0 modified"))
            .stdout(predicate::str::contains("0 deleted"))
            .stdout(predicate::str::contains("1 unchanged"));

        // Index should still only have 1 entry
        let index = load_test_index(dir.path());
        let count = index
            .history
            .iter()
            .filter(|e| e.file == "stable.md")
            .count();
        assert_eq!(count, 1, "No new entries should be added on unchanged scan");
    }

    #[test]
    fn test_scan_ignores_non_matching_patterns() {
        let dir = setup_test_dir();
        ftm().current_dir(dir.path()).arg("init").assert().success();

        // Create files with non-matching extensions
        std::fs::write(dir.path().join("image.png"), "not tracked").unwrap();
        std::fs::write(dir.path().join("binary.exe"), "not tracked").unwrap();
        // Create a matching file as reference
        std::fs::write(dir.path().join("code.rs"), "fn test() {}").unwrap();

        ftm()
            .current_dir(dir.path())
            .arg("scan")
            .assert()
            .success()
            .stdout(predicate::str::contains("1 created"));

        let index = load_test_index(dir.path());
        assert_eq!(
            index.history.len(),
            1,
            "Only matching file should be tracked"
        );
        assert_eq!(index.history[0].file, "code.rs");
    }

    #[test]
    fn test_scan_skips_large_files() {
        let dir = setup_test_dir();
        ftm().current_dir(dir.path()).arg("init").assert().success();

        // Override max_file_size to 100 bytes in config
        let config_path = dir.path().join(".ftm/config.yaml");
        let config_content = std::fs::read_to_string(&config_path).unwrap();
        let new_config = config_content.replace("max_file_size: 31457280", "max_file_size: 100");
        std::fs::write(&config_path, new_config).unwrap();

        // Create a small file and a large file
        std::fs::write(dir.path().join("small.txt"), "tiny").unwrap();
        std::fs::write(dir.path().join("large.txt"), "x".repeat(200)).unwrap();

        ftm()
            .current_dir(dir.path())
            .arg("scan")
            .assert()
            .success()
            .stdout(predicate::str::contains("1 created"));

        let index = load_test_index(dir.path());
        assert_eq!(index.history.len(), 1);
        assert_eq!(index.history[0].file, "small.txt");
    }

    #[test]
    fn test_scan_subdirectories() {
        let dir = setup_test_dir();
        ftm().current_dir(dir.path()).arg("init").assert().success();

        let sub_dir = dir.path().join("src/lib");
        std::fs::create_dir_all(&sub_dir).unwrap();
        std::fs::write(sub_dir.join("mod.rs"), "pub mod lib;").unwrap();
        std::fs::write(dir.path().join("main.rs"), "fn main() {}").unwrap();

        ftm()
            .current_dir(dir.path())
            .arg("scan")
            .assert()
            .success()
            .stdout(predicate::str::contains("2 created"));

        let index = load_test_index(dir.path());
        assert!(index.history.iter().any(|e| e.file == "src/lib/mod.rs"));
        assert!(index.history.iter().any(|e| e.file == "main.rs"));
    }

    #[test]
    fn test_scan_skips_excluded_directories() {
        let dir = setup_test_dir();
        ftm().current_dir(dir.path()).arg("init").assert().success();

        // Create files in excluded directories
        let target_dir = dir.path().join("target/debug");
        std::fs::create_dir_all(&target_dir).unwrap();
        std::fs::write(target_dir.join("build.rs"), "// build artifact").unwrap();

        let node_dir = dir.path().join("node_modules/pkg");
        std::fs::create_dir_all(&node_dir).unwrap();
        std::fs::write(node_dir.join("index.js"), "module.exports = {}").unwrap();

        // Create a normal tracked file
        std::fs::write(dir.path().join("app.rs"), "fn main() {}").unwrap();

        ftm()
            .current_dir(dir.path())
            .arg("scan")
            .assert()
            .success()
            .stdout(predicate::str::contains("1 created"));

        let index = load_test_index(dir.path());
        assert_eq!(index.history.len(), 1);
        assert_eq!(index.history[0].file, "app.rs");
    }

    #[test]
    fn test_scan_empty_files_ignored() {
        let dir = setup_test_dir();
        ftm().current_dir(dir.path()).arg("init").assert().success();

        std::fs::write(dir.path().join("empty.rs"), "").unwrap();
        std::fs::write(dir.path().join("notempty.rs"), "fn x() {}").unwrap();

        ftm()
            .current_dir(dir.path())
            .arg("scan")
            .assert()
            .success()
            .stdout(predicate::str::contains("1 created"));

        let index = load_test_index(dir.path());
        assert_eq!(index.history.len(), 1);
        assert_eq!(index.history[0].file, "notempty.rs");
    }

    #[test]
    fn test_scan_dedup_same_content() {
        let dir = setup_test_dir();
        ftm().current_dir(dir.path()).arg("init").assert().success();

        let content = "shared: content";
        std::fs::write(dir.path().join("a.yaml"), content).unwrap();
        std::fs::write(dir.path().join("b.yaml"), content).unwrap();

        ftm()
            .current_dir(dir.path())
            .arg("scan")
            .assert()
            .success()
            .stdout(predicate::str::contains("2 created"));

        // Both entries should share the same snapshot
        let snap_count = count_snapshot_files(dir.path());
        assert_eq!(
            snap_count, 1,
            "Two files with same content should share 1 snapshot"
        );

        let index = load_test_index(dir.path());
        let checksums: Vec<_> = index
            .history
            .iter()
            .filter_map(|e| e.checksum.as_ref())
            .collect();
        assert_eq!(checksums.len(), 2);
        assert_eq!(checksums[0], checksums[1], "Checksums should match");
    }
}
