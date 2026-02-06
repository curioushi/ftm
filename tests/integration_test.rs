//! Integration tests for ftm CLI commands.
//!
//! Run with: cargo test

use assert_cmd::Command;
use predicates::prelude::*;
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
            INTERVAL_MS, NUM_WRITES, entry_count
        );
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
}

