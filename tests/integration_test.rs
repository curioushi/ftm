//! Integration tests for ftm CLI commands.
//!
//! Run with: cargo test

use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::tempdir;

/// Helper to get ftm command (uses path set by cargo test)
fn ftm() -> Command {
    Command::from_std(std::process::Command::new(env!("CARGO_BIN_EXE_ftm")))
}

/// Create a test directory and return its path
fn setup_test_dir() -> tempfile::TempDir {
    tempdir().unwrap()
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

    #[test]
    fn test_ls_entry_count_after_many_modifications() {
        const NUM_MODIFICATIONS: usize = 20;
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
        for i in 0..NUM_MODIFICATIONS {
            let header = format!("version: {}\n", i);
            let padding = target_size.saturating_sub(header.len());
            let content = format!("{}{}", header, "x".repeat(padding));
            std::fs::write(&file_path, content).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        let expected_count = format!("{} entries", NUM_MODIFICATIONS);
        ftm()
            .current_dir(dir.path())
            .arg("ls")
            .assert()
            .success()
            .stdout(predicate::str::contains("data.yaml"))
            .stdout(predicate::str::contains(&expected_count));

        let _ = watch_child.kill();
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

