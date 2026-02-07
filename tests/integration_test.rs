//! Integration tests for ftm CLI commands (server/client architecture).
//!
//! Run with: cargo test --release -- --test-threads=1

use assert_cmd::Command;
use predicates::prelude::*;
use serde::Deserialize;
use std::io::{BufRead, BufReader, Read};
use std::path::Path;
use tempfile::tempdir;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Helper to get ftm command with --port set (for client commands).
fn ftm_client(port: u16) -> Command {
    let mut cmd = Command::from_std(std::process::Command::new(env!("CARGO_BIN_EXE_ftm")));
    cmd.args(["--port", &port.to_string()]);
    cmd
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

/// Create a test directory. On test failure the dir is preserved.
fn setup_test_dir() -> TestDirGuard {
    TestDirGuard {
        inner: Some(tempdir().unwrap()),
    }
}

/// Start the ftm server on a random port. Returns (child, actual_port).
fn start_server() -> (std::process::Child, u16) {
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_ftm"))
        .args(["--port", "0", "serve"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("failed to spawn ftm serve");

    let stdout = child.stdout.take().expect("failed to get stdout");
    let mut reader = BufReader::new(stdout);
    let mut line = String::new();
    reader
        .read_line(&mut line)
        .expect("failed to read server output");

    // Parse port from "Listening on 127.0.0.1:<port>"
    let port: u16 = line
        .trim()
        .rsplit(':')
        .next()
        .expect("failed to find port in output")
        .parse()
        .expect("failed to parse port");

    // Drain stdout in background to prevent SIGPIPE / buffer fill
    std::thread::spawn(move || {
        let mut buf = [0u8; 1024];
        while reader.read(&mut buf).unwrap_or(0) > 0 {}
    });

    (child, port)
}

/// Start server and checkout a directory. Returns (child, port).
fn start_server_and_checkout(dir: &Path) -> (std::process::Child, u16) {
    let (child, port) = start_server();
    ftm_client(port)
        .args(["checkout", dir.to_str().unwrap()])
        .assert()
        .success();
    // Brief delay for watcher to initialize
    std::thread::sleep(std::time::Duration::from_millis(50));
    (child, port)
}

fn stop_server(child: &mut std::process::Child) {
    let _ = child.kill();
    let _ = child.wait();
}

/// Pre-initialize .ftm in a directory with custom settings.
/// Useful for tests that need non-default config (e.g. max_history, max_file_size).
fn pre_init_ftm(dir: &Path, max_history: usize, max_file_size: u64) {
    let ftm_dir = dir.join(".ftm");
    std::fs::create_dir_all(&ftm_dir).unwrap();
    let config_yaml = format!(
        r#"watch:
  patterns:
  - '*.rs'
  - '*.py'
  - '*.md'
  - '*.txt'
  - '*.json'
  - '*.yml'
  - '*.yaml'
  - '*.toml'
  - '*.js'
  - '*.ts'
  exclude:
  - '**/target/**'
  - '**/node_modules/**'
  - '**/.git/**'
  - '**/.ftm/**'
settings:
  max_history: {}
  max_file_size: {}
  web_port: 8765
"#,
        max_history, max_file_size
    );
    std::fs::write(ftm_dir.join("config.yaml"), config_yaml).unwrap();
    std::fs::write(ftm_dir.join("index.json"), r#"{"history":[]}"#).unwrap();
}

/// Pre-initialize .ftm with custom settings including scan_interval.
fn pre_init_ftm_with_scan(dir: &Path, max_history: usize, max_file_size: u64, scan_interval: u64) {
    let ftm_dir = dir.join(".ftm");
    std::fs::create_dir_all(&ftm_dir).unwrap();
    let config_yaml = format!(
        r#"watch:
  patterns:
  - '*.rs'
  - '*.py'
  - '*.md'
  - '*.txt'
  - '*.json'
  - '*.yml'
  - '*.yaml'
  - '*.toml'
  - '*.js'
  - '*.ts'
  exclude:
  - '**/target/**'
  - '**/node_modules/**'
  - '**/.git/**'
  - '**/.ftm/**'
settings:
  max_history: {}
  max_file_size: {}
  web_port: 8765
  scan_interval: {}
"#,
        max_history, max_file_size, scan_interval
    );
    std::fs::write(ftm_dir.join("config.yaml"), config_yaml).unwrap();
    std::fs::write(ftm_dir.join("index.json"), r#"{"history":[]}"#).unwrap();
}

/// Wait for a server child process to exit on its own, within a timeout.
/// Returns `true` if the process exited, `false` on timeout.
fn wait_for_server_exit(child: &mut std::process::Child, timeout: std::time::Duration) -> bool {
    let start = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return true,
            Ok(None) => {}
            Err(_) => return false,
        }
        if start.elapsed() > timeout {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

/// Minimal deserialization structs for index.json.
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

mod checkout_tests {
    use super::*;

    #[test]
    fn test_checkout_creates_ftm_directory() {
        let dir = setup_test_dir();
        let (mut server, port) = start_server();

        ftm_client(port)
            .args(["checkout", dir.path().to_str().unwrap()])
            .assert()
            .success()
            .stdout(predicate::str::contains("Checked out and watching"));

        assert!(dir.path().join(".ftm").exists());
        assert!(dir.path().join(".ftm/config.yaml").exists());
        assert!(dir.path().join(".ftm/index.json").exists());

        stop_server(&mut server);
    }

    #[test]
    fn test_checkout_auto_starts_server() {
        let dir = setup_test_dir();

        // Use a random port to avoid conflicts
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener); // free the port

        // Checkout without a running server — should auto-start one.
        // Capture output so we can parse the server PID from stderr.
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_ftm"))
            .args([
                "--port",
                &port.to_string(),
                "checkout",
                dir.path().to_str().unwrap(),
            ])
            .output()
            .expect("failed to run ftm checkout");
        assert!(output.status.success(), "checkout should succeed");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains("Checked out and watching"),
            "stdout should contain success message, got: {}",
            stdout
        );

        // Parse server PID from stderr: "Starting FTM server on port X (pid: Y)..."
        let stderr = String::from_utf8_lossy(&output.stderr);
        let server_pid: Option<u32> = stderr.lines().find(|l| l.contains("pid:")).and_then(|l| {
            l.split("pid: ")
                .nth(1)?
                .trim_end_matches(|c: char| !c.is_ascii_digit())
                .parse()
                .ok()
        });

        // .ftm directory should have been created by the checkout handler
        assert!(dir.path().join(".ftm").exists());
        assert!(dir.path().join(".ftm/config.yaml").exists());
        assert!(dir.path().join(".ftm/index.json").exists());

        // Server should now be reachable
        let health_resp = reqwest::blocking::Client::builder()
            .no_proxy()
            .build()
            .unwrap()
            .get(format!("http://127.0.0.1:{}/api/health", port))
            .timeout(std::time::Duration::from_secs(2))
            .send();
        assert!(
            health_resp.is_ok() && health_resp.unwrap().status().is_success(),
            "Server should be healthy after auto-start"
        );

        // Write a file and verify the watcher is functional
        std::fs::write(dir.path().join("autotest.yaml"), "key: value").unwrap();
        assert!(
            wait_for_index(dir.path(), "autotest.yaml", 1, 3000),
            "Watcher should be functional after auto-start"
        );

        // Clean up: kill the auto-started server
        if let Some(pid) = server_pid {
            #[cfg(unix)]
            {
                std::process::Command::new("kill")
                    .arg(pid.to_string())
                    .output()
                    .ok();
                // Wait briefly for the process to exit
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            #[cfg(windows)]
            {
                std::process::Command::new("taskkill")
                    .args(["/F", "/PID", &pid.to_string()])
                    .output()
                    .ok();
            }
        }
    }

    /// Checkout should kill stale ftm processes while keeping the healthy server.
    #[test]
    fn test_checkout_kills_stale_server() {
        if !cfg!(unix) {
            return;
        }

        let dir = setup_test_dir();
        let (mut server_a, port_a) = start_server();
        let (mut server_b, _port_b) = start_server();

        // Freeze server B so it becomes an unreachable stale process
        std::process::Command::new("kill")
            .args(["-STOP", &server_b.id().to_string()])
            .output()
            .unwrap();

        // Checkout on port A triggers kill_stale_servers
        ftm_client(port_a)
            .args(["checkout", dir.path().to_str().unwrap()])
            .assert()
            .success();

        // Server B should have been killed
        std::thread::sleep(std::time::Duration::from_millis(200));
        assert!(
            server_b.try_wait().unwrap().is_some(),
            "stale server B should be dead"
        );

        // Server A should still be alive
        assert!(
            server_a.try_wait().unwrap().is_none(),
            "healthy server A should be alive"
        );

        stop_server(&mut server_a);
    }

    #[test]
    fn test_checkout_same_dir_is_noop() {
        let dir = setup_test_dir();
        let (mut server, port) = start_server();

        // First checkout
        ftm_client(port)
            .args(["checkout", dir.path().to_str().unwrap()])
            .assert()
            .success();

        // Second checkout of the same directory should be a no-op
        ftm_client(port)
            .args(["checkout", dir.path().to_str().unwrap()])
            .assert()
            .success()
            .stdout(predicate::str::contains("Already watching"));

        stop_server(&mut server);
    }

    #[test]
    fn test_checkout_switch_directory() {
        let dir_a = setup_test_dir();
        let dir_b = setup_test_dir();

        // Use a random port to avoid conflicts
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);

        // Checkout dir A (auto-starts server)
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_ftm"))
            .args([
                "--port",
                &port.to_string(),
                "checkout",
                dir_a.path().to_str().unwrap(),
            ])
            .output()
            .expect("failed to run ftm checkout A");
        assert!(
            output.status.success(),
            "First checkout should succeed: stdout={}, stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );

        // Verify dir A is being watched
        assert!(dir_a.path().join(".ftm").exists());

        // Write a file to dir A and confirm watcher is working
        std::fs::write(dir_a.path().join("a.yaml"), "a: 1").unwrap();
        assert!(
            wait_for_index(dir_a.path(), "a.yaml", 1, 3000),
            "Watcher should be functional on dir A"
        );

        // Checkout dir B — should switch: shutdown old server, start new, checkout
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_ftm"))
            .args([
                "--port",
                &port.to_string(),
                "checkout",
                dir_b.path().to_str().unwrap(),
            ])
            .output()
            .expect("failed to run ftm checkout B");
        let stdout_b = String::from_utf8_lossy(&output.stdout);
        let stderr_b = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success(),
            "Switch checkout should succeed: stdout={}, stderr={}",
            stdout_b,
            stderr_b,
        );
        assert!(
            stderr_b.contains("Switching"),
            "Expected 'Switching' message in stderr, got: {}",
            stderr_b
        );
        assert!(
            stdout_b.contains("Checked out and watching"),
            "Expected checkout success in stdout, got: {}",
            stdout_b
        );

        // Verify dir B is being watched
        assert!(dir_b.path().join(".ftm").exists());

        // Write to dir B and verify watcher works on the new directory
        std::fs::write(dir_b.path().join("b.yaml"), "b: 1").unwrap();
        assert!(
            wait_for_index(dir_b.path(), "b.yaml", 1, 3000),
            "Watcher should be functional on dir B after switch"
        );

        // ls should show dir B
        let ls_out = std::process::Command::new(env!("CARGO_BIN_EXE_ftm"))
            .args(["--port", &port.to_string(), "ls"])
            .output()
            .expect("failed to run ftm ls");
        let ls_stdout = String::from_utf8_lossy(&ls_out.stdout);
        let dir_b_str = dir_b
            .path()
            .canonicalize()
            .unwrap_or_else(|_| dir_b.path().to_path_buf())
            .to_string_lossy()
            .to_string();
        assert!(
            ls_stdout.contains(&dir_b_str),
            "ls should show dir B path, got: {}",
            ls_stdout
        );

        // Clean up: parse last server PID from stderr and kill
        let last_pid: Option<u32> = stderr_b
            .lines()
            .rev()
            .find(|l| l.contains("pid:"))
            .and_then(|l| {
                l.split("pid: ")
                    .nth(1)?
                    .trim_end_matches(|c: char| !c.is_ascii_digit())
                    .parse()
                    .ok()
            });
        if let Some(pid) = last_pid {
            #[cfg(unix)]
            {
                std::process::Command::new("kill")
                    .arg(pid.to_string())
                    .output()
                    .ok();
            }
            #[cfg(windows)]
            {
                std::process::Command::new("taskkill")
                    .args(["/F", "/PID", &pid.to_string()])
                    .output()
                    .ok();
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
    }
}

mod ls_tests {
    use super::*;

    #[test]
    fn test_ls_not_checked_out() {
        let (mut server, port) = start_server();

        ftm_client(port)
            .arg("ls")
            .assert()
            .failure()
            .stderr(predicate::str::contains("No directory checked out"));

        stop_server(&mut server);
    }

    #[test]
    fn test_ls_shows_watch_dir() {
        let dir = setup_test_dir();
        let (mut server, port) = start_server_and_checkout(dir.path());

        let output = ftm_client(port)
            .arg("ls")
            .output()
            .expect("failed to run ls");
        let stdout = String::from_utf8_lossy(&output.stdout);

        let dir_str = dir
            .path()
            .canonicalize()
            .unwrap_or_else(|_| dir.path().to_path_buf())
            .to_string_lossy()
            .to_string();
        assert!(
            stdout.contains(&dir_str),
            "ls should show watch directory, got: {}",
            stdout
        );
        assert!(
            stdout.contains("Watch directory:"),
            "ls output should have 'Watch directory:' label"
        );

        stop_server(&mut server);
    }

    #[test]
    fn test_ls_empty() {
        let dir = setup_test_dir();
        let (mut server, port) = start_server_and_checkout(dir.path());

        ftm_client(port)
            .arg("ls")
            .assert()
            .success()
            .stdout(predicate::str::contains("No files tracked yet"));

        stop_server(&mut server);
    }

    #[test]
    fn test_ls_after_checkout_and_write() {
        let dir = setup_test_dir();
        let (mut server, port) = start_server_and_checkout(dir.path());

        std::fs::write(dir.path().join("a.yaml"), "a: 1").unwrap();
        std::fs::write(dir.path().join("b.yaml"), "b: 2").unwrap();
        std::fs::write(dir.path().join("c.yaml"), "c: 3").unwrap();

        assert!(wait_for_index(dir.path(), "a.yaml", 1, 2000));
        assert!(wait_for_index(dir.path(), "b.yaml", 1, 2000));
        assert!(wait_for_index(dir.path(), "c.yaml", 1, 2000));

        ftm_client(port)
            .arg("ls")
            .assert()
            .success()
            .stdout(predicate::str::contains("a.yaml"))
            .stdout(predicate::str::contains("b.yaml"))
            .stdout(predicate::str::contains("c.yaml"));

        stop_server(&mut server);
    }

    /// Helper: write `num_writes` versions of a 5 MB file with `interval_ms` between writes,
    /// then return (entry_count, all_sizes_correct).
    fn write_and_check(num_writes: usize, interval_ms: u64) -> (usize, bool) {
        let dir = setup_test_dir();
        let (mut server, port) = start_server_and_checkout(dir.path());

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

        // Wait for at least one entry to be recorded
        assert!(
            wait_for_index(dir.path(), "data.yaml", 1, 5000),
            "data.yaml should have at least 1 entry"
        );

        // Allow pending events to settle (macOS FSEvents delivers in batches)
        std::thread::sleep(std::time::Duration::from_millis(100));

        // Parse entry count from ls output
        let ls_output = ftm_client(port)
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
        let history_output = ftm_client(port)
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

        stop_server(&mut server);
        (entry_count, correct_size_entries == total_size_entries)
    }

    #[test]
    fn test_low_freq_writes_exact_entry_count() {
        const NUM_WRITES: usize = 50;
        // On Linux, inotify CloseWrite fires per-close so 20ms is sufficient.
        // On macOS, FSEvents coalesces rapid events for the same file,
        // so we need a longer interval to guarantee each write produces
        // a separate event.
        #[cfg(target_os = "linux")]
        const INTERVAL_MS: u64 = 20;
        #[cfg(not(target_os = "linux"))]
        const INTERVAL_MS: u64 = 60;

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

mod watcher_tests {
    use super::*;

    #[test]
    fn test_excluded_files_not_tracked() {
        let dir = setup_test_dir();
        let (mut server, _port) = start_server_and_checkout(dir.path());

        // Write a file inside .ftm/ (excluded by default)
        std::fs::write(dir.path().join(".ftm/sneaky.yaml"), "should: ignore").unwrap();
        // Write a non-matching extension file
        std::fs::write(dir.path().join("data.bin"), "binary stuff").unwrap();
        // Write a tracked file as a reference
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

        stop_server(&mut server);
    }

    #[test]
    fn test_non_matching_extension_ignored() {
        let dir = setup_test_dir();
        let (mut server, _port) = start_server_and_checkout(dir.path());

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

        stop_server(&mut server);
    }

    #[test]
    fn test_subdirectory_files_tracked() {
        let dir = setup_test_dir();
        let sub_dir = dir.path().join("sub/deep");
        std::fs::create_dir_all(&sub_dir).unwrap();

        let (mut server, port) = start_server_and_checkout(dir.path());

        std::fs::write(sub_dir.join("foo.rs"), "fn hello() {}").unwrap();

        assert!(
            wait_for_index(dir.path(), "sub/deep/foo.rs", 1, 2000),
            "sub/deep/foo.rs should be recorded with relative path"
        );

        ftm_client(port)
            .arg("ls")
            .assert()
            .success()
            .stdout(predicate::str::contains("sub/deep/foo.rs"));

        stop_server(&mut server);
    }

    #[test]
    fn test_empty_file_ignored() {
        let dir = setup_test_dir();
        let (mut server, _port) = start_server_and_checkout(dir.path());

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

        stop_server(&mut server);
    }
}

mod dedup_tests {
    use super::*;

    #[test]
    fn test_same_content_no_duplicate_entry() {
        let dir = setup_test_dir();
        let (mut server, _port) = start_server_and_checkout(dir.path());

        let content = "key: same_content";

        // First write
        std::fs::write(dir.path().join("dup.yaml"), content).unwrap();
        assert!(
            wait_for_index(dir.path(), "dup.yaml", 1, 2000),
            "First write should be recorded"
        );

        // Second write with identical content
        std::fs::write(dir.path().join("dup.yaml"), content).unwrap();

        // Write a sync marker
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

        stop_server(&mut server);
    }

    #[test]
    fn test_different_files_same_content_share_snapshot() {
        let dir = setup_test_dir();
        let (mut server, _port) = start_server_and_checkout(dir.path());

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
        assert!(index.history.iter().any(|e| e.file == "file_a.yaml"));
        assert!(index.history.iter().any(|e| e.file == "file_b.yaml"));

        // Only 1 snapshot file (content-addressable dedup)
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

        stop_server(&mut server);
    }
}

mod history_tests {
    use super::*;

    #[test]
    fn test_history_not_checked_out() {
        let (mut server, port) = start_server();

        ftm_client(port)
            .args(["history", "test.rs"])
            .assert()
            .failure()
            .stderr(predicate::str::contains("No directory checked out"));

        stop_server(&mut server);
    }

    #[test]
    fn test_history_no_entries() {
        let dir = setup_test_dir();
        let (mut server, port) = start_server_and_checkout(dir.path());

        ftm_client(port)
            .args(["history", "nonexistent.rs"])
            .assert()
            .success()
            .stdout(predicate::str::contains("No history for"));

        stop_server(&mut server);
    }
}

mod history_ops_tests {
    use super::*;

    #[test]
    fn test_history_create_then_modify_ops() {
        let dir = setup_test_dir();
        let (mut server, _port) = start_server_and_checkout(dir.path());
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

        stop_server(&mut server);
    }

    #[test]
    fn test_history_delete_recorded() {
        let dir = setup_test_dir();
        let (mut server, _port) = start_server_and_checkout(dir.path());
        let file_path = dir.path().join("todelete.yaml");

        // Create
        std::fs::write(&file_path, "will be deleted").unwrap();
        assert!(wait_for_index(dir.path(), "todelete.yaml", 1, 2000));

        // Delete
        std::fs::remove_file(&file_path).unwrap();
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

        stop_server(&mut server);
    }

    #[test]
    fn test_history_recreate_after_delete() {
        let dir = setup_test_dir();
        let (mut server, _port) = start_server_and_checkout(dir.path());
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

        stop_server(&mut server);
    }

    #[test]
    fn test_history_multiple_files_independent() {
        let dir = setup_test_dir();
        let (mut server, _port) = start_server_and_checkout(dir.path());

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

        stop_server(&mut server);
    }
}

mod restore_tests {
    use super::*;

    #[test]
    fn test_restore_not_checked_out() {
        let (mut server, port) = start_server();

        ftm_client(port)
            .args(["restore", "test.rs", "-c", "abc12345"])
            .assert()
            .failure()
            .stderr(predicate::str::contains("No directory checked out"));

        stop_server(&mut server);
    }

    #[test]
    fn test_restore_version_not_found() {
        let dir = setup_test_dir();
        let (mut server, port) = start_server_and_checkout(dir.path());

        ftm_client(port)
            .args(["restore", "test.rs", "-c", "abc12345"])
            .assert()
            .failure()
            .stderr(predicate::str::contains("Version not found"));

        stop_server(&mut server);
    }

    #[test]
    fn test_restore_roundtrip() {
        let dir = setup_test_dir();
        let (mut server, port) = start_server_and_checkout(dir.path());
        let file_path = dir.path().join("roundtrip.yaml");

        let v1_content = "version: 1\ndata: original";
        let v2_content = "version: 2\ndata: modified";

        // Write v1
        std::fs::write(&file_path, v1_content).unwrap();
        assert!(wait_for_index(dir.path(), "roundtrip.yaml", 1, 2000));

        // Write v2
        std::fs::write(&file_path, v2_content).unwrap();
        assert!(wait_for_index(dir.path(), "roundtrip.yaml", 2, 2000));

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

        // Restore v1 via server
        ftm_client(port)
            .args(["restore", "roundtrip.yaml", "-c", v1_checksum])
            .assert()
            .success();

        // Verify content is back to v1
        let restored = std::fs::read_to_string(&file_path).unwrap();
        assert_eq!(
            restored, v1_content,
            "File content should be restored to v1"
        );

        stop_server(&mut server);
    }

    #[test]
    fn test_restore_with_short_checksum_prefix() {
        let dir = setup_test_dir();
        let (mut server, port) = start_server_and_checkout(dir.path());
        let file_path = dir.path().join("prefix.yaml");

        let original = "data: for_prefix_test";

        std::fs::write(&file_path, original).unwrap();
        assert!(wait_for_index(dir.path(), "prefix.yaml", 1, 2000));

        std::fs::write(&file_path, "data: modified version").unwrap();
        assert!(wait_for_index(dir.path(), "prefix.yaml", 2, 2000));

        let index = load_test_index(dir.path());
        let entry = index
            .history
            .iter()
            .find(|e| e.file == "prefix.yaml" && e.op == "create")
            .unwrap();
        let full_checksum = entry.checksum.as_ref().unwrap();
        let short_prefix = &full_checksum[..8];

        // Restore using only the first 8 chars of the checksum
        ftm_client(port)
            .args(["restore", "prefix.yaml", "-c", short_prefix])
            .assert()
            .success();

        let restored = std::fs::read_to_string(&file_path).unwrap();
        assert_eq!(
            restored, original,
            "Restore with 8-char prefix should recover original content"
        );

        stop_server(&mut server);
    }

    #[test]
    fn test_restore_deleted_file() {
        let dir = setup_test_dir();
        let (mut server, port) = start_server_and_checkout(dir.path());
        let file_path = dir.path().join("willdelete.yaml");

        let content = "precious: data";
        std::fs::write(&file_path, content).unwrap();
        assert!(wait_for_index(dir.path(), "willdelete.yaml", 1, 2000));

        // Delete the file and wait for the delete event
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

        // Restore the deleted file via server (watcher will pick this up)
        ftm_client(port)
            .args(["restore", "willdelete.yaml", "-c", &checksum])
            .assert()
            .success();

        assert!(file_path.exists(), "File should be restored after deletion");
        let restored = std::fs::read_to_string(&file_path).unwrap();
        assert_eq!(restored, content, "Restored content should match original");

        // Wait for the watcher to record the restored file as a new create
        assert!(
            wait_for_index(dir.path(), "willdelete.yaml", 3, 2000),
            "Restored file should be recorded as a new create entry"
        );

        // Verify the full index: create -> delete -> create
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

        // The newest create entry checksum should match the original content
        let last_entry = entries.last().unwrap();
        assert_eq!(last_entry.op, "create", "Latest entry must be create");
        use sha2::{Digest, Sha256};
        let expected_checksum = hex::encode(Sha256::digest(content.as_bytes()));
        assert_eq!(
            last_entry.checksum.as_ref().unwrap(),
            &expected_checksum,
            "Latest create entry checksum should match the original content hash"
        );

        stop_server(&mut server);
    }

    #[test]
    fn test_restore_to_subdirectory() {
        let dir = setup_test_dir();
        let sub_dir = dir.path().join("nested/dir");
        std::fs::create_dir_all(&sub_dir).unwrap();

        let (mut server, port) = start_server_and_checkout(dir.path());
        let file_path = sub_dir.join("deep.yaml");

        let content = "nested: file content";
        std::fs::write(&file_path, content).unwrap();
        assert!(wait_for_index(dir.path(), "nested/dir/deep.yaml", 1, 2000));

        // Get checksum
        let index = load_test_index(dir.path());
        let entry = index
            .history
            .iter()
            .find(|e| e.file == "nested/dir/deep.yaml")
            .unwrap();
        let checksum = entry.checksum.as_ref().unwrap();

        // Delete the entire subdirectory tree
        std::fs::remove_dir_all(dir.path().join("nested")).unwrap();
        assert!(!file_path.exists());

        // Restore should recreate parent directories automatically
        ftm_client(port)
            .args(["restore", "nested/dir/deep.yaml", "-c", checksum])
            .assert()
            .success();

        assert!(
            file_path.exists(),
            "File should be restored with parent dirs recreated"
        );
        let restored = std::fs::read_to_string(&file_path).unwrap();
        assert_eq!(restored, content);

        stop_server(&mut server);
    }
}

mod trim_tests {
    use super::*;

    #[test]
    fn test_max_history_trims_old_entries() {
        let dir = setup_test_dir();

        // Pre-init .ftm with max_history=3
        pre_init_ftm(dir.path(), 3, 30 * 1024 * 1024);

        let (mut server, _port) = start_server_and_checkout(dir.path());
        let file_path = dir.path().join("trimme.yaml");

        // Write 5 different versions with delay between each
        for i in 0..5 {
            std::fs::write(&file_path, format!("version: {}", i)).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(150));
        }

        // Write a sync marker to ensure all previous writes were processed
        std::fs::write(dir.path().join("sync.yaml"), "sync: done").unwrap();
        assert!(
            wait_for_index(dir.path(), "sync.yaml", 1, 3000),
            "Sync marker should be recorded"
        );

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

        // If all 5 writes were captured, verify the retained entries are the newest 3 versions
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
            let oldest_checksum = hex::encode(Sha256::digest(b"version: 0"));
            assert!(
                !entries
                    .iter()
                    .any(|e| e.checksum.as_deref() == Some(oldest_checksum.as_str())),
                "Oldest version (version: 0) should have been trimmed"
            );
        }

        stop_server(&mut server);
    }
}

mod scan_tests {
    use super::*;

    #[test]
    fn test_scan_not_checked_out() {
        let (mut server, port) = start_server();

        ftm_client(port)
            .arg("scan")
            .assert()
            .failure()
            .stderr(predicate::str::contains("No directory checked out"));

        stop_server(&mut server);
    }

    #[test]
    fn test_scan_detects_new_files() {
        let dir = setup_test_dir();

        // Create files BEFORE checkout (watcher won't see them)
        std::fs::write(dir.path().join("hello.rs"), "fn main() {}").unwrap();
        std::fs::write(dir.path().join("world.py"), "print('hi')").unwrap();

        let (mut server, port) = start_server_and_checkout(dir.path());

        ftm_client(port)
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

        stop_server(&mut server);
    }

    #[test]
    fn test_scan_detects_modifications() {
        let dir = setup_test_dir();

        // Create baseline file BEFORE checkout
        std::fs::write(dir.path().join("app.rs"), "fn main() {}").unwrap();

        let (mut server, port) = start_server_and_checkout(dir.path());

        // First scan: creates baseline
        ftm_client(port)
            .arg("scan")
            .assert()
            .success()
            .stdout(predicate::str::contains("1 created"));

        // Modify the file (watcher will also detect this, but we verify final state)
        std::fs::write(dir.path().join("app.rs"), "fn main() { println!(\"hi\"); }").unwrap();

        // Wait for either watcher or scan to pick up the change
        assert!(
            wait_for_index(dir.path(), "app.rs", 2, 2000),
            "Modification should be recorded"
        );

        let index = load_test_index(dir.path());
        let entries: Vec<_> = index
            .history
            .iter()
            .filter(|e| e.file == "app.rs")
            .collect();
        assert_eq!(entries.len(), 2, "Should have create + modify");
        assert_eq!(entries[0].op, "create");
        assert_eq!(entries[1].op, "modify");

        stop_server(&mut server);
    }

    #[test]
    fn test_scan_detects_deletions() {
        let dir = setup_test_dir();

        // Create file BEFORE checkout
        std::fs::write(dir.path().join("temp.txt"), "temporary content").unwrap();

        let (mut server, port) = start_server_and_checkout(dir.path());

        // Scan to create baseline
        ftm_client(port)
            .arg("scan")
            .assert()
            .success()
            .stdout(predicate::str::contains("1 created"));

        // Delete the file (watcher will also detect this)
        std::fs::remove_file(dir.path().join("temp.txt")).unwrap();

        // Wait for deletion to be recorded
        assert!(
            wait_for_index(dir.path(), "temp.txt", 2, 2000),
            "Deletion should be recorded"
        );

        let index = load_test_index(dir.path());
        let entries: Vec<_> = index
            .history
            .iter()
            .filter(|e| e.file == "temp.txt")
            .collect();
        assert_eq!(entries.len(), 2, "Should have create + delete");
        assert_eq!(entries[0].op, "create");
        assert_eq!(entries[1].op, "delete");

        stop_server(&mut server);
    }

    #[test]
    fn test_scan_no_changes_second_run() {
        let dir = setup_test_dir();

        // Create file BEFORE checkout
        std::fs::write(dir.path().join("stable.md"), "# Stable").unwrap();

        let (mut server, port) = start_server_and_checkout(dir.path());

        // First scan
        ftm_client(port)
            .arg("scan")
            .assert()
            .success()
            .stdout(predicate::str::contains("1 created"));

        // Second scan - nothing changed
        ftm_client(port)
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

        stop_server(&mut server);
    }

    #[test]
    fn test_scan_ignores_non_matching_patterns() {
        let dir = setup_test_dir();

        // Create files BEFORE checkout
        std::fs::write(dir.path().join("image.png"), "not tracked").unwrap();
        std::fs::write(dir.path().join("binary.exe"), "not tracked").unwrap();
        std::fs::write(dir.path().join("code.rs"), "fn test() {}").unwrap();

        let (mut server, port) = start_server_and_checkout(dir.path());

        ftm_client(port)
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

        stop_server(&mut server);
    }

    #[test]
    fn test_scan_skips_large_files() {
        let dir = setup_test_dir();

        // Pre-init .ftm with max_file_size=100
        pre_init_ftm(dir.path(), 100, 100);

        // Create files BEFORE checkout
        std::fs::write(dir.path().join("small.txt"), "tiny").unwrap();
        std::fs::write(dir.path().join("large.txt"), "x".repeat(200)).unwrap();

        let (mut server, port) = start_server_and_checkout(dir.path());

        ftm_client(port)
            .arg("scan")
            .assert()
            .success()
            .stdout(predicate::str::contains("1 created"));

        let index = load_test_index(dir.path());
        assert_eq!(index.history.len(), 1);
        assert_eq!(index.history[0].file, "small.txt");

        stop_server(&mut server);
    }

    #[test]
    fn test_scan_subdirectories() {
        let dir = setup_test_dir();

        // Create files in subdirectories BEFORE checkout
        let sub_dir = dir.path().join("src/lib");
        std::fs::create_dir_all(&sub_dir).unwrap();
        std::fs::write(sub_dir.join("mod.rs"), "pub mod lib;").unwrap();
        std::fs::write(dir.path().join("main.rs"), "fn main() {}").unwrap();

        let (mut server, port) = start_server_and_checkout(dir.path());

        ftm_client(port)
            .arg("scan")
            .assert()
            .success()
            .stdout(predicate::str::contains("2 created"));

        let index = load_test_index(dir.path());
        assert!(index.history.iter().any(|e| e.file == "src/lib/mod.rs"));
        assert!(index.history.iter().any(|e| e.file == "main.rs"));

        stop_server(&mut server);
    }

    #[test]
    fn test_scan_skips_excluded_directories() {
        let dir = setup_test_dir();

        // Create files in excluded directories BEFORE checkout
        let target_dir = dir.path().join("target/debug");
        std::fs::create_dir_all(&target_dir).unwrap();
        std::fs::write(target_dir.join("build.rs"), "// build artifact").unwrap();

        let node_dir = dir.path().join("node_modules/pkg");
        std::fs::create_dir_all(&node_dir).unwrap();
        std::fs::write(node_dir.join("index.js"), "module.exports = {}").unwrap();

        // Normal tracked file
        std::fs::write(dir.path().join("app.rs"), "fn main() {}").unwrap();

        let (mut server, port) = start_server_and_checkout(dir.path());

        ftm_client(port)
            .arg("scan")
            .assert()
            .success()
            .stdout(predicate::str::contains("1 created"));

        let index = load_test_index(dir.path());
        assert_eq!(index.history.len(), 1);
        assert_eq!(index.history[0].file, "app.rs");

        stop_server(&mut server);
    }

    #[test]
    fn test_scan_empty_files_ignored() {
        let dir = setup_test_dir();

        // Create files BEFORE checkout
        std::fs::write(dir.path().join("empty.rs"), "").unwrap();
        std::fs::write(dir.path().join("notempty.rs"), "fn x() {}").unwrap();

        let (mut server, port) = start_server_and_checkout(dir.path());

        ftm_client(port)
            .arg("scan")
            .assert()
            .success()
            .stdout(predicate::str::contains("1 created"));

        let index = load_test_index(dir.path());
        assert_eq!(index.history.len(), 1);
        assert_eq!(index.history[0].file, "notempty.rs");

        stop_server(&mut server);
    }

    #[test]
    fn test_scan_dedup_same_content() {
        let dir = setup_test_dir();

        // Create files BEFORE checkout
        let content = "shared: content";
        std::fs::write(dir.path().join("a.yaml"), content).unwrap();
        std::fs::write(dir.path().join("b.yaml"), content).unwrap();

        let (mut server, port) = start_server_and_checkout(dir.path());

        ftm_client(port)
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

        stop_server(&mut server);
    }
}

// ===========================================================================
// Version tests
// ===========================================================================

mod version_tests {
    use super::*;

    #[test]
    fn test_version_without_server() {
        // version should still print client version even when no server is running
        ftm_client(19999)
            .arg("version")
            .assert()
            .success()
            .stdout(predicate::str::contains("Client version:"))
            .stdout(predicate::str::contains("not running"));
    }

    #[test]
    fn test_version_with_server() {
        let (mut server, port) = start_server();

        ftm_client(port)
            .arg("version")
            .assert()
            .success()
            .stdout(predicate::str::contains("Client version:"))
            .stdout(predicate::str::contains("Server version:"));

        stop_server(&mut server);
    }
}

// ===========================================================================
// Config tests
// ===========================================================================

mod config_tests {
    use super::*;

    #[test]
    fn test_config_get_all() {
        let dir = setup_test_dir();
        let (mut server, port) = start_server_and_checkout(dir.path());

        ftm_client(port)
            .args(["config", "get"])
            .assert()
            .success()
            .stdout(predicate::str::contains("max_history"))
            .stdout(predicate::str::contains("patterns"));

        stop_server(&mut server);
    }

    #[test]
    fn test_config_get_single_key() {
        let dir = setup_test_dir();
        let (mut server, port) = start_server_and_checkout(dir.path());

        ftm_client(port)
            .args(["config", "get", "settings.max_history"])
            .assert()
            .success()
            .stdout(predicate::str::contains("100"));

        stop_server(&mut server);
    }

    #[test]
    fn test_config_get_invalid_key() {
        let dir = setup_test_dir();
        let (mut server, port) = start_server_and_checkout(dir.path());

        ftm_client(port)
            .args(["config", "get", "nonexistent.key"])
            .assert()
            .failure()
            .stderr(predicate::str::contains("Unknown config key"));

        stop_server(&mut server);
    }

    #[test]
    fn test_config_set_and_get() {
        let dir = setup_test_dir();
        let (mut server, port) = start_server_and_checkout(dir.path());

        // Set max_history to 200
        ftm_client(port)
            .args(["config", "set", "settings.max_history", "200"])
            .assert()
            .success()
            .stdout(predicate::str::contains("Set settings.max_history = 200"));

        // Verify it was changed
        ftm_client(port)
            .args(["config", "get", "settings.max_history"])
            .assert()
            .success()
            .stdout(predicate::str::contains("200"));

        // Verify persisted to config.yaml
        let config_content = std::fs::read_to_string(dir.path().join(".ftm/config.yaml")).unwrap();
        assert!(config_content.contains("200"));

        stop_server(&mut server);
    }

    #[test]
    fn test_config_set_invalid_value() {
        let dir = setup_test_dir();
        let (mut server, port) = start_server_and_checkout(dir.path());

        // max_history expects a number
        ftm_client(port)
            .args(["config", "set", "settings.max_history", "not_a_number"])
            .assert()
            .failure()
            .stderr(predicate::str::contains("Invalid value"));

        stop_server(&mut server);
    }

    #[test]
    fn test_config_set_watch_patterns() {
        let dir = setup_test_dir();
        let (mut server, port) = start_server_and_checkout(dir.path());

        ftm_client(port)
            .args(["config", "set", "watch.patterns", "*.rs,*.go,*.py"])
            .assert()
            .success();

        ftm_client(port)
            .args(["config", "get", "watch.patterns"])
            .assert()
            .success()
            .stdout(predicate::str::contains("*.rs"))
            .stdout(predicate::str::contains("*.go"))
            .stdout(predicate::str::contains("*.py"));

        stop_server(&mut server);
    }

    #[test]
    fn test_config_not_checked_out() {
        let (mut server, port) = start_server();

        ftm_client(port)
            .args(["config", "get"])
            .assert()
            .failure()
            .stderr(predicate::str::contains("No directory checked out"));

        stop_server(&mut server);
    }
}

// ===========================================================================
// Logs tests
// ===========================================================================

mod logs_tests {
    use super::*;

    #[test]
    fn test_logs_no_log_files() {
        let dir = setup_test_dir();
        let (mut server, port) = start_server_and_checkout(dir.path());

        // No log dir created yet, should report no log files
        ftm_client(port)
            .arg("logs")
            .assert()
            .success()
            .stdout(predicate::str::contains("No log files"));

        stop_server(&mut server);
    }

    #[test]
    fn test_logs_with_log_file() {
        let dir = setup_test_dir();
        let (mut server, port) = start_server_and_checkout(dir.path());

        // Manually create a log file to simulate server logging
        let log_dir = dir.path().join(".ftm/logs");
        std::fs::create_dir_all(&log_dir).unwrap();
        std::fs::write(
            log_dir.join("20260206-120000.log"),
            "INFO test log line 1\nINFO test log line 2\n",
        )
        .unwrap();

        // logs command should find the file and try less, then fallback to print
        ftm_client(port)
            .arg("logs")
            .assert()
            .success()
            .stdout(predicate::str::contains("20260206-120000.log"));

        stop_server(&mut server);
    }

    #[test]
    fn test_logs_picks_latest_file() {
        let dir = setup_test_dir();
        let (mut server, port) = start_server_and_checkout(dir.path());

        // Create multiple log files
        let log_dir = dir.path().join(".ftm/logs");
        std::fs::create_dir_all(&log_dir).unwrap();
        std::fs::write(log_dir.join("20260101-100000.log"), "old log\n").unwrap();
        std::fs::write(log_dir.join("20260206-150000.log"), "new log\n").unwrap();

        // Should pick the newest one (20260206-150000.log)
        ftm_client(port)
            .arg("logs")
            .assert()
            .success()
            .stdout(predicate::str::contains("20260206-150000.log"));

        stop_server(&mut server);
    }

    #[test]
    fn test_logs_not_checked_out() {
        let (mut server, port) = start_server();

        ftm_client(port)
            .arg("logs")
            .assert()
            .failure()
            .stderr(predicate::str::contains("No directory checked out"));

        stop_server(&mut server);
    }
}

// ===========================================================================
// Watchdog tests (.ftm deletion -> auto shutdown)
// ===========================================================================

mod watchdog_tests {
    use super::*;

    #[test]
    fn test_server_stops_on_ftm_deleted() {
        let dir = setup_test_dir();
        let (mut server, port) = start_server_and_checkout(dir.path());

        // Verify server is healthy
        ftm_client(port).args(["ls"]).assert().success();

        // Delete the entire .ftm directory
        let ftm_dir = dir.path().join(".ftm");
        assert!(ftm_dir.exists(), ".ftm should exist before deletion");
        std::fs::remove_dir_all(&ftm_dir).unwrap();
        assert!(!ftm_dir.exists(), ".ftm should be gone after deletion");

        // The watchdog checks every 2 seconds; allow up to 10 seconds
        let exited = wait_for_server_exit(&mut server, std::time::Duration::from_secs(10));
        assert!(
            exited,
            "Server should have exited after .ftm directory was deleted"
        );
    }
}

// ===========================================================================
// Periodic scan tests
// ===========================================================================

mod periodic_scan_tests {
    use super::*;

    #[test]
    fn test_periodic_scan_detects_existing_file() {
        let dir = setup_test_dir();

        // Create a file BEFORE checkout so the watcher won't catch it;
        // only the periodic scanner should pick it up.
        std::fs::write(
            dir.path().join("pre_existing.txt"),
            "hello from before checkout",
        )
        .unwrap();

        // Pre-init .ftm with a short scan interval (2 seconds)
        pre_init_ftm_with_scan(dir.path(), 100, 30 * 1024 * 1024, 2);

        let (mut server, _port) = start_server_and_checkout(dir.path());

        // Wait for the periodic scanner to fire (interval=2s, give it up to 8s)
        let found = wait_for_index(dir.path(), "pre_existing.txt", 1, 8000);
        assert!(
            found,
            "Periodic scanner should have picked up pre_existing.txt"
        );

        // Verify the entry in index
        let index = load_test_index(dir.path());
        let entries: Vec<_> = index
            .history
            .iter()
            .filter(|e| e.file == "pre_existing.txt")
            .collect();
        assert!(
            !entries.is_empty(),
            "Should have history for pre_existing.txt"
        );
        assert_eq!(entries[0].op, "create");

        stop_server(&mut server);
    }

    #[test]
    fn test_periodic_scan_disabled_when_zero() {
        let dir = setup_test_dir();

        // Create a file BEFORE checkout
        std::fs::write(dir.path().join("should_not_scan.txt"), "no scan").unwrap();

        // Pre-init with scan_interval=0 (disabled)
        pre_init_ftm_with_scan(dir.path(), 100, 30 * 1024 * 1024, 0);

        let (mut server, _port) = start_server_and_checkout(dir.path());

        // Wait 4 seconds — if scanning were enabled at 0, it would have fired
        std::thread::sleep(std::time::Duration::from_secs(4));

        let index = load_test_index(dir.path());
        let entries: Vec<_> = index
            .history
            .iter()
            .filter(|e| e.file == "should_not_scan.txt")
            .collect();
        assert!(
            entries.is_empty(),
            "With scan_interval=0, no periodic scan should run"
        );

        stop_server(&mut server);
    }
}
