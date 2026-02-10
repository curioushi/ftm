//! Integration tests for ftm CLI commands (server/client architecture).
//!
//! Run with: cargo test --release -- --test-threads=1

use ctor::ctor;
use serde::Deserialize;
use std::io::{BufRead, BufReader, Read};
use std::path::Path;
use std::process::Command;
use tempfile::tempdir;

// ---------------------------------------------------------------------------
// Run once before any test: kill all ftm processes in the system
// ---------------------------------------------------------------------------

#[ctor]
fn kill_all_ftm_before_tests() {
    kill_all_ftm_processes();
}

/// Kill all ftm processes in the system (Windows: taskkill ftm.exe; Unix: pkill ftm).
fn kill_all_ftm_processes() {
    #[cfg(windows)]
    {
        let _ = Command::new("taskkill")
            .args(["/F", "/IM", "ftm.exe"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
    #[cfg(unix)]
    {
        let _ = Command::new("pkill")
            .args(["-9", "ftm"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

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
    let path_s = dir.to_str().unwrap();
    let out = run_ftm_with_port(port, &["checkout", path_s]);
    assert!(
        out.status.success(),
        "checkout should succeed: stdout={}, stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    // Brief delay for watcher to initialize
    std::thread::sleep(std::time::Duration::from_millis(50));
    (child, port)
}

fn stop_server(child: &mut std::process::Child) {
    let _ = child.kill();
    let _ = child.wait();
}

/// Run ftm with given args, draining stdout/stderr in background to avoid pipe deadlock (Windows/Unix).
fn run_ftm_output(args: &[&str]) -> std::process::Output {
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_ftm"))
        .args(args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("failed to spawn ftm");
    let stdout_handle = child.stdout.take();
    let stderr_handle = child.stderr.take();
    let stdout_collector = std::sync::Arc::new(std::sync::Mutex::new(Vec::<u8>::new()));
    let stderr_collector = std::sync::Arc::new(std::sync::Mutex::new(Vec::<u8>::new()));
    let so = stdout_collector.clone();
    let se = stderr_collector.clone();
    std::thread::spawn(move || {
        if let Some(mut out) = stdout_handle {
            let mut buf = [0u8; 4096];
            loop {
                match std::io::Read::read(&mut out, &mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        if let Ok(mut v) = so.lock() {
                            v.extend_from_slice(&buf[..n]);
                        }
                    }
                    Err(_) => break,
                }
            }
        }
    });
    std::thread::spawn(move || {
        if let Some(mut err) = stderr_handle {
            let mut buf = [0u8; 4096];
            loop {
                match std::io::Read::read(&mut err, &mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        if let Ok(mut v) = se.lock() {
                            v.extend_from_slice(&buf[..n]);
                        }
                    }
                    Err(_) => break,
                }
            }
        }
    });
    let status = child.wait().expect("failed to wait on ftm");
    let stdout = std::mem::take(&mut *stdout_collector.lock().unwrap());
    let stderr = std::mem::take(&mut *stderr_collector.lock().unwrap());
    std::process::Output {
        status,
        stdout,
        stderr,
    }
}

/// Run ftm with --port and given args (uses run_ftm_output to avoid pipe deadlock).
fn run_ftm_with_port(port: u16, args: &[&str]) -> std::process::Output {
    let port_s = port.to_string();
    let all: Vec<&str> = std::iter::once("--port")
        .chain(std::iter::once(port_s.as_str()))
        .chain(args.iter().copied())
        .collect();
    run_ftm_output(&all)
}

/// Kill a process by PID (cross-platform: kill on Unix, taskkill on Windows).
fn kill_process(pid: u32) {
    #[cfg(unix)]
    {
        let _ = std::process::Command::new("kill")
            .arg(pid.to_string())
            .output();
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("taskkill")
            .args(["/F", "/PID", &pid.to_string()])
            .output();
    }
}

/// Pre-initialize .ftm in a directory with custom settings.
/// Optional scan_interval and clean_interval use server defaults when None.
fn pre_init_ftm(
    dir: &Path,
    max_history: usize,
    max_file_size: u64,
    scan_interval: Option<u64>,
    clean_interval: Option<u64>,
) {
    let ftm_dir = dir.join(".ftm");
    std::fs::create_dir_all(&ftm_dir).unwrap();
    let mut settings = format!(
        "  max_history: {}\n  max_file_size: {}",
        max_history, max_file_size
    );
    if let Some(s) = scan_interval {
        settings.push_str(&format!("\n  scan_interval: {}", s));
    }
    if let Some(c) = clean_interval {
        settings.push_str(&format!("\n  clean_interval: {}", c));
    }
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
{}
"#,
        settings
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

#[derive(Deserialize)]
struct HealthPid {
    pid: Option<u32>,
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

        let path_s = dir.path().to_str().unwrap();
        let out = run_ftm_with_port(port, &["checkout", path_s]);
        assert!(
            out.status.success(),
            "checkout should succeed: stdout={}, stderr={}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr),
        );
        assert!(
            String::from_utf8_lossy(&out.stdout).contains("Checked out and watching"),
            "stdout should contain success message"
        );

        assert!(dir.path().join(".ftm").exists());
        assert!(dir.path().join(".ftm/config.yaml").exists());
        assert!(dir.path().join(".ftm/index.json").exists());

        stop_server(&mut server);
    }

    #[test]
    fn test_checkout_auto_starts_server() {
        let dir = setup_test_dir();

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);

        let path_s = dir.path().to_str().unwrap();
        let out = run_ftm_with_port(port, &["checkout", path_s]);
        assert!(
            out.status.success(),
            "checkout should succeed: stdout={}, stderr={}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr),
        );

        assert!(dir.path().join(".ftm").exists());
        assert!(dir.path().join(".ftm/config.yaml").exists());
        assert!(dir.path().join(".ftm/index.json").exists());

        let client = reqwest::blocking::Client::builder()
            .no_proxy()
            .build()
            .unwrap();
        let health_resp = client
            .get(format!("http://127.0.0.1:{}/api/health", port))
            .timeout(std::time::Duration::from_secs(2))
            .send()
            .expect("health request failed");
        assert!(
            health_resp.status().is_success(),
            "Server should be healthy after auto-start"
        );
        let server_pid: Option<u32> = health_resp.json::<HealthPid>().ok().and_then(|h| h.pid);

        std::fs::write(dir.path().join("autotest.yaml"), "key: value").unwrap();
        assert!(
            wait_for_index(dir.path(), "autotest.yaml", 1, 3000),
            "Watcher should be functional after auto-start"
        );

        if let Some(pid) = server_pid {
            kill_process(pid);
        }
    }

    /// Checkout should kill all ftm processes (including healthy ones) and start fresh.
    #[test]
    fn test_checkout_kills_all_servers() {
        if !cfg!(unix) {
            return;
        }

        let dir = setup_test_dir();
        let (mut server_a, _port_a) = start_server();
        let (mut server_b, _port_b) = start_server();

        // Freeze server B so it becomes an unreachable stale process
        std::process::Command::new("kill")
            .args(["-STOP", &server_b.id().to_string()])
            .output()
            .unwrap();

        // Use a fresh random port for checkout
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);

        // Checkout triggers kill_all_servers — both A and B should be killed
        let path_s = dir.path().to_str().unwrap();
        let out = run_ftm_with_port(port, &["checkout", path_s]);
        assert!(
            out.status.success(),
            "checkout should succeed: stdout={}, stderr={}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr),
        );

        // Both servers should have been killed
        std::thread::sleep(std::time::Duration::from_millis(200));
        assert!(
            server_b.try_wait().unwrap().is_some(),
            "stale server B should be dead"
        );
        assert!(
            server_a.try_wait().unwrap().is_some(),
            "server A should also be dead (kill_all_servers kills everything)"
        );

        // Clean up: kill the auto-started server (get PID from health API)
        let client = reqwest::blocking::Client::builder()
            .no_proxy()
            .build()
            .unwrap();
        if let Ok(resp) = client
            .get(format!("http://127.0.0.1:{}/api/health", port))
            .timeout(std::time::Duration::from_secs(2))
            .send()
        {
            if let Ok(health) = resp.json::<HealthPid>() {
                if let Some(pid) = health.pid {
                    kill_process(pid);
                }
            }
        }
    }

    #[test]
    fn test_checkout_same_dir_is_noop() {
        let dir = setup_test_dir();
        let (mut server, port) = start_server();

        let path_s = dir.path().to_str().unwrap();

        // First checkout
        let out1 = run_ftm_with_port(port, &["checkout", path_s]);
        assert!(
            out1.status.success(),
            "first checkout should succeed: stdout={}, stderr={}",
            String::from_utf8_lossy(&out1.stdout),
            String::from_utf8_lossy(&out1.stderr),
        );

        // Second checkout of the same directory should be a no-op
        let out2 = run_ftm_with_port(port, &["checkout", path_s]);
        assert!(
            out2.status.success(),
            "second checkout should succeed: stdout={}, stderr={}",
            String::from_utf8_lossy(&out2.stdout),
            String::from_utf8_lossy(&out2.stderr),
        );
        assert!(
            String::from_utf8_lossy(&out2.stdout).contains("Already watching"),
            "stdout should contain Already watching"
        );

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

        let path_a = dir_a.path().to_str().unwrap();
        let path_b = dir_b.path().to_str().unwrap();

        // Checkout dir A (auto-starts server)
        let out_a = run_ftm_with_port(port, &["checkout", path_a]);
        assert!(
            out_a.status.success(),
            "First checkout should succeed: stdout={}, stderr={}",
            String::from_utf8_lossy(&out_a.stdout),
            String::from_utf8_lossy(&out_a.stderr),
        );

        assert!(dir_a.path().join(".ftm").exists());
        std::fs::write(dir_a.path().join("a.yaml"), "a: 1").unwrap();
        assert!(
            wait_for_index(dir_a.path(), "a.yaml", 1, 3000),
            "Watcher should be functional on dir A"
        );

        // Checkout dir B — should switch: shutdown old server, start new, checkout
        let out_b = run_ftm_with_port(port, &["checkout", path_b]);
        assert!(
            out_b.status.success(),
            "Switch checkout should succeed: stdout={}, stderr={}",
            String::from_utf8_lossy(&out_b.stdout),
            String::from_utf8_lossy(&out_b.stderr),
        );

        assert!(dir_b.path().join(".ftm").exists());
        std::fs::write(dir_b.path().join("b.yaml"), "b: 1").unwrap();
        assert!(
            wait_for_index(dir_b.path(), "b.yaml", 1, 3000),
            "Watcher should be functional on dir B after switch"
        );

        // ls should show dir B
        let ls_out = run_ftm_with_port(port, &["ls"]);
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

        // Clean up: get server PID from health API and kill
        let client = reqwest::blocking::Client::builder()
            .no_proxy()
            .build()
            .unwrap();
        if let Ok(resp) = client
            .get(format!("http://127.0.0.1:{}/api/health", port))
            .timeout(std::time::Duration::from_secs(2))
            .send()
        {
            if let Ok(health) = resp.json::<HealthPid>() {
                if let Some(pid) = health.pid {
                    kill_process(pid);
                }
            }
        }
    }
}

mod ls_tests {
    use super::*;

    #[test]
    fn test_ls_not_checked_out() {
        let (mut server, port) = start_server();

        let out = run_ftm_with_port(port, &["ls"]);
        assert!(!out.status.success(), "ls should fail when not checked out");
        assert!(
            String::from_utf8_lossy(&out.stderr).contains("No directory checked out"),
            "stderr: {}",
            String::from_utf8_lossy(&out.stderr)
        );

        stop_server(&mut server);
    }

    #[test]
    fn test_ls_shows_watch_dir() {
        let dir = setup_test_dir();
        let (mut server, port) = start_server_and_checkout(dir.path());

        let output = run_ftm_with_port(port, &["ls"]);
        assert!(output.status.success(), "ls should succeed");
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

        let out = run_ftm_with_port(port, &["ls"]);
        assert!(out.status.success(), "ls should succeed");
        assert!(
            String::from_utf8_lossy(&out.stdout).contains("No files tracked yet"),
            "stdout: {}",
            String::from_utf8_lossy(&out.stdout)
        );

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

        let out = run_ftm_with_port(port, &["ls"]);
        assert!(out.status.success(), "ls should succeed");
        let s = String::from_utf8_lossy(&out.stdout);
        assert!(s.contains("a.yaml"), "stdout: {}", s);
        assert!(s.contains("b.yaml"), "stdout: {}", s);
        assert!(s.contains("c.yaml"), "stdout: {}", s);

        stop_server(&mut server);
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

        let ls_output = run_ftm_with_port(port, &["ls"]);
        assert!(ls_output.status.success(), "ls should succeed");
        let ls_stdout = String::from_utf8_lossy(&ls_output.stdout);
        assert!(
            ls_stdout.contains("foo.rs") && ls_stdout.contains("sub") && ls_stdout.contains("deep"),
            "ls should show sub/deep/foo.rs (tree format); got:\n{}",
            ls_stdout
        );

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

mod rename_tests {
    use super::*;

    /// Simulate file-manager "delete" (e.g. Finder, Nautilus, Explorer):
    /// move (rename) a tracked file out of the watched directory.
    /// The watcher should detect this as a delete.
    #[test]
    fn test_file_moved_out_detected_as_delete() {
        let dir = setup_test_dir();
        // Create a directory outside the watched tree to move files into
        // (simulates Trash / Recycle Bin or any external location).
        let outside = tempfile::tempdir().unwrap();

        let (mut server, _port) = start_server_and_checkout(dir.path());
        let file_path = dir.path().join("finder_del.txt");

        // Create the file so watcher records it
        std::fs::write(&file_path, "will be moved to trash").unwrap();
        assert!(
            wait_for_index(dir.path(), "finder_del.txt", 1, 2000),
            "Initial create should be recorded"
        );

        // Move the file out of the watched directory (mimics move-to-trash)
        let dest = outside.path().join("finder_del.txt");
        std::fs::rename(&file_path, &dest).unwrap();

        assert!(
            wait_for_index(dir.path(), "finder_del.txt", 2, 4000),
            "Move-out (rename) should be recorded as delete"
        );

        let index = load_test_index(dir.path());
        let entries: Vec<_> = index
            .history
            .iter()
            .filter(|e| e.file == "finder_del.txt")
            .collect();
        assert_eq!(entries.len(), 2, "Should have 2 entries (create + delete)");
        assert_eq!(entries[0].op, "create");
        assert_eq!(entries[1].op, "delete");
        assert!(
            entries[1].checksum.is_none(),
            "Delete should have no checksum"
        );

        stop_server(&mut server);
    }

    /// Move a file from outside into the watched directory.
    /// The watcher should detect this as a new file (create/snapshot).
    #[test]
    fn test_file_moved_in_detected_as_create() {
        let dir = setup_test_dir();
        let outside = tempfile::tempdir().unwrap();

        let (mut server, _port) = start_server_and_checkout(dir.path());

        // Create a file outside the watched directory
        let external_file = outside.path().join("incoming.txt");
        std::fs::write(&external_file, "moved in from outside").unwrap();

        // Move it into the watched directory
        let dest = dir.path().join("incoming.txt");
        std::fs::rename(&external_file, &dest).unwrap();

        assert!(
            wait_for_index(dir.path(), "incoming.txt", 1, 4000),
            "Move-in (rename) should be recorded as create"
        );

        let index = load_test_index(dir.path());
        let entry = index
            .history
            .iter()
            .find(|e| e.file == "incoming.txt")
            .expect("incoming.txt should have a history entry");
        assert_eq!(entry.op, "create");
        assert!(
            entry.checksum.is_some(),
            "Create entry should have a checksum"
        );

        stop_server(&mut server);
    }

    /// Rename a file within the watched directory.  The watcher should record
    /// a delete for the old name and a create for the new name.
    #[test]
    fn test_rename_within_watched_dir() {
        let dir = setup_test_dir();
        let (mut server, _port) = start_server_and_checkout(dir.path());

        let old_path = dir.path().join("before.txt");
        std::fs::write(&old_path, "rename me").unwrap();
        assert!(
            wait_for_index(dir.path(), "before.txt", 1, 2000),
            "Initial create should be recorded"
        );

        // Rename within the watched directory
        let new_path = dir.path().join("after.txt");
        std::fs::rename(&old_path, &new_path).unwrap();

        assert!(
            wait_for_index(dir.path(), "before.txt", 2, 4000),
            "Old name should get a delete entry"
        );
        assert!(
            wait_for_index(dir.path(), "after.txt", 1, 4000),
            "New name should get a create entry"
        );

        let index = load_test_index(dir.path());

        let old_entries: Vec<_> = index
            .history
            .iter()
            .filter(|e| e.file == "before.txt")
            .collect();
        assert_eq!(
            old_entries.len(),
            2,
            "Old name should have 2 entries (create + delete)"
        );
        assert_eq!(old_entries[0].op, "create");
        assert_eq!(old_entries[1].op, "delete");

        let new_entries: Vec<_> = index
            .history
            .iter()
            .filter(|e| e.file == "after.txt")
            .collect();
        assert_eq!(
            new_entries.len(),
            1,
            "New name should have 1 entry (create)"
        );
        assert_eq!(new_entries[0].op, "create");

        stop_server(&mut server);
    }

    /// Rename a folder within the watched directory. Old path files should get delete
    /// entries; new path files should get create entries.
    #[test]
    fn test_rename_folder_within_watched_dir() {
        let dir = setup_test_dir();
        let (mut server, _port) = start_server_and_checkout(dir.path());

        let old_dir = dir.path().join("old_name");
        std::fs::create_dir_all(&old_dir).unwrap();
        std::fs::write(old_dir.join("a.txt"), "content a").unwrap();
        std::fs::write(old_dir.join("b.rs"), "content b").unwrap();

        assert!(
            wait_for_index(dir.path(), "old_name/a.txt", 1, 3000),
            "old_name/a.txt should be recorded"
        );
        assert!(
            wait_for_index(dir.path(), "old_name/b.rs", 1, 3000),
            "old_name/b.rs should be recorded"
        );

        let new_dir = dir.path().join("new_name");
        std::fs::rename(&old_dir, &new_dir).unwrap();

        assert!(
            wait_for_index(dir.path(), "old_name/a.txt", 2, 5000),
            "old_name/a.txt should have create + delete"
        );
        assert!(
            wait_for_index(dir.path(), "old_name/b.rs", 2, 5000),
            "old_name/b.rs should have create + delete"
        );
        assert!(
            wait_for_index(dir.path(), "new_name/a.txt", 1, 5000),
            "new_name/a.txt should be recorded after folder rename"
        );
        assert!(
            wait_for_index(dir.path(), "new_name/b.rs", 1, 5000),
            "new_name/b.rs should be recorded after folder rename"
        );

        let index = load_test_index(dir.path());
        for file in &["old_name/a.txt", "old_name/b.rs"] {
            let entries: Vec<_> = index.history.iter().filter(|e| e.file == *file).collect();
            assert_eq!(
                entries.len(),
                2,
                "{} should have 2 entries (create + delete)",
                file
            );
            assert_eq!(entries[0].op, "create");
            assert_eq!(entries[1].op, "delete");
        }
        for file in &["new_name/a.txt", "new_name/b.rs"] {
            let entries: Vec<_> = index.history.iter().filter(|e| e.file == *file).collect();
            assert_eq!(entries.len(), 1, "{} should have 1 create entry", file);
            assert_eq!(entries[0].op, "create");
        }

        stop_server(&mut server);
    }

    /// Move a folder (with tracked files) out of the watched directory.
    /// Index should record delete for all files under that path.
    #[test]
    fn test_rename_folder_move_out() {
        let dir = setup_test_dir();
        let outside = tempfile::tempdir().unwrap();
        let (mut server, _port) = start_server_and_checkout(dir.path());

        let subdir = dir.path().join("subdir");
        std::fs::create_dir_all(&subdir).unwrap();
        std::fs::write(subdir.join("f.txt"), "moved out").unwrap();

        assert!(
            wait_for_index(dir.path(), "subdir/f.txt", 1, 3000),
            "subdir/f.txt should be recorded"
        );

        let dest = outside.path().join("subdir");
        std::fs::rename(&subdir, &dest).unwrap();

        assert!(
            wait_for_index(dir.path(), "subdir/f.txt", 2, 5000),
            "subdir/f.txt should have create + delete after folder move-out"
        );

        let index = load_test_index(dir.path());
        let entries: Vec<_> = index
            .history
            .iter()
            .filter(|e| e.file == "subdir/f.txt")
            .collect();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].op, "create");
        assert_eq!(entries[1].op, "delete");

        stop_server(&mut server);
    }

    /// Move a folder from outside into the watched directory.
    /// Index should record create for all matching files under the new path.
    #[test]
    fn test_rename_folder_move_in() {
        let dir = setup_test_dir();
        let outside = tempfile::tempdir().unwrap();
        let (mut server, _port) = start_server_and_checkout(dir.path());

        let external_dir = outside.path().join("incoming");
        std::fs::create_dir_all(&external_dir).unwrap();
        std::fs::write(external_dir.join("x.yaml"), "moved in").unwrap();

        let dest = dir.path().join("incoming");
        std::fs::rename(&external_dir, &dest).unwrap();

        assert!(
            wait_for_index(dir.path(), "incoming/x.yaml", 1, 5000),
            "incoming/x.yaml should be recorded after folder move-in"
        );

        let index = load_test_index(dir.path());
        let entry = index
            .history
            .iter()
            .find(|e| e.file == "incoming/x.yaml")
            .expect("incoming/x.yaml should have a history entry");
        assert_eq!(entry.op, "create");
        assert!(entry.checksum.is_some());

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

        let out = run_ftm_with_port(port, &["history", "test.rs"]);
        assert!(!out.status.success());
        assert!(String::from_utf8_lossy(&out.stderr).contains("No directory checked out"));

        stop_server(&mut server);
    }

    #[test]
    fn test_history_no_entries() {
        let dir = setup_test_dir();
        let (mut server, port) = start_server_and_checkout(dir.path());

        let out = run_ftm_with_port(port, &["history", "nonexistent.rs"]);
        assert!(out.status.success());
        assert!(String::from_utf8_lossy(&out.stdout).contains("No history for"));

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

    /// Default `ftm ls` excludes deleted files; `ftm ls --include-deleted` shows them.
    #[test]
    fn test_ls_hides_deleted_by_default() {
        let dir = setup_test_dir();
        let (mut server, port) = start_server_and_checkout(dir.path());
        let file_path = dir.path().join("ls_hide_deleted.yaml");

        std::fs::write(&file_path, "content").unwrap();
        assert!(
            wait_for_index(dir.path(), "ls_hide_deleted.yaml", 1, 2000),
            "Create should be recorded"
        );

        let ls_default = run_ftm_with_port(port, &["ls"]);
        assert!(ls_default.status.success(), "ftm ls should succeed");
        let ls_stdout = String::from_utf8_lossy(&ls_default.stdout);
        assert!(
            ls_stdout.contains("ls_hide_deleted.yaml"),
            "ls (default) should show file before delete; got:\n{}",
            ls_stdout
        );

        std::fs::remove_file(&file_path).unwrap();
        assert!(
            wait_for_index(dir.path(), "ls_hide_deleted.yaml", 2, 2000),
            "Delete event should be recorded"
        );

        let ls_after_delete = run_ftm_with_port(port, &["ls"]);
        assert!(ls_after_delete.status.success(), "ftm ls should succeed");
        let ls_stdout = String::from_utf8_lossy(&ls_after_delete.stdout);
        assert!(
            !ls_stdout.contains("ls_hide_deleted.yaml"),
            "ls (default) should hide deleted file; got:\n{}",
            ls_stdout
        );

        let ls_include_deleted = run_ftm_with_port(port, &["ls", "--include-deleted"]);
        assert!(
            ls_include_deleted.status.success(),
            "ftm ls --include-deleted should succeed"
        );
        let ls_stdout = String::from_utf8_lossy(&ls_include_deleted.stdout);
        assert!(
            ls_stdout.contains("ls_hide_deleted.yaml"),
            "ls --include-deleted should show deleted file; got:\n{}",
            ls_stdout
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

        let out = run_ftm_with_port(port, &["restore", "test.rs", "abc12345"]);
        assert!(!out.status.success());
        assert!(String::from_utf8_lossy(&out.stderr).contains("No directory checked out"));

        stop_server(&mut server);
    }

    #[test]
    fn test_restore_version_not_found() {
        let dir = setup_test_dir();
        let (mut server, port) = start_server_and_checkout(dir.path());

        let out = run_ftm_with_port(port, &["restore", "test.rs", "abc12345"]);
        assert!(!out.status.success());
        assert!(String::from_utf8_lossy(&out.stderr).contains("Version not found"));

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
        let out = run_ftm_with_port(port, &["restore", "roundtrip.yaml", v1_checksum]);
        assert!(
            out.status.success(),
            "restore: {}",
            String::from_utf8_lossy(&out.stderr)
        );

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
        let out = run_ftm_with_port(port, &["restore", "prefix.yaml", short_prefix]);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );

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
        let out = run_ftm_with_port(port, &["restore", "willdelete.yaml", &checksum]);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );

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
        let out = run_ftm_with_port(port, &["restore", "nested/dir/deep.yaml", checksum]);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );

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
        pre_init_ftm(dir.path(), 3, 30 * 1024 * 1024, None, None);

        let (mut server, _port) = start_server_and_checkout(dir.path());
        let file_path = dir.path().join("trimme.yaml");

        // Write 5 different versions with delay between each
        for i in 0..5 {
            std::fs::write(&file_path, format!("version: {}", i)).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(50));
        }

        // Write a sync marker so we have 6 total entries and trigger trim to 3
        std::fs::write(dir.path().join("sync.yaml"), "sync: done").unwrap();
        assert!(
            wait_for_index(dir.path(), "sync.yaml", 1, 5000),
            "Sync marker should be recorded"
        );

        let index = load_test_index(dir.path());
        assert!(
            index.history.len() <= 3,
            "global max_history=3: expected at most 3 total entries, got {}",
            index.history.len()
        );

        let entries: Vec<_> = index
            .history
            .iter()
            .filter(|e| e.file == "trimme.yaml")
            .collect();
        assert!(
            entries.len() >= 1 && entries.len() <= 2,
            "trimme.yaml should have 1 or 2 entries (sync may take one slot), got {}",
            entries.len()
        );

        use sha2::{Digest, Sha256};
        let expected_checksums: Vec<String> = (3..5)
            .map(|i| hex::encode(Sha256::digest(format!("version: {}", i).as_bytes())))
            .collect();
        let expected = if entries.len() == 2 {
            &expected_checksums[..]
        } else {
            &expected_checksums[1..]
        };
        for (entry, expected_cs) in entries.iter().zip(expected.iter()) {
            let cs = entry.checksum.as_ref().expect("entry should have checksum");
            assert_eq!(
                cs, expected_cs,
                "Trimmed entries for trimme should be the newest versions (v3, v4) in order"
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

        let out = run_ftm_with_port(port, &["scan"]);
        assert!(!out.status.success());
        assert!(String::from_utf8_lossy(&out.stderr).contains("No directory checked out"));

        stop_server(&mut server);
    }

    #[test]
    fn test_scan_detects_new_files() {
        let dir = setup_test_dir();

        // Create files BEFORE checkout (watcher won't see them)
        std::fs::write(dir.path().join("hello.rs"), "fn main() {}").unwrap();
        std::fs::write(dir.path().join("world.py"), "print('hi')").unwrap();

        let (mut server, port) = start_server_and_checkout(dir.path());

        let out = run_ftm_with_port(port, &["scan"]);
        assert!(out.status.success());
        let s = String::from_utf8_lossy(&out.stdout);
        assert!(s.contains("2 created"));
        assert!(s.contains("0 modified"));
        assert!(s.contains("0 deleted"));

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
        let out = run_ftm_with_port(port, &["scan"]);
        assert!(out.status.success());
        assert!(String::from_utf8_lossy(&out.stdout).contains("1 created"));

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
        let out = run_ftm_with_port(port, &["scan"]);
        assert!(out.status.success());
        assert!(String::from_utf8_lossy(&out.stdout).contains("1 created"));

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
        let out = run_ftm_with_port(port, &["scan"]);
        assert!(out.status.success());
        assert!(String::from_utf8_lossy(&out.stdout).contains("1 created"));

        // Second scan - nothing changed
        let out = run_ftm_with_port(port, &["scan"]);
        assert!(out.status.success());
        let s = String::from_utf8_lossy(&out.stdout);
        assert!(s.contains("0 created"));
        assert!(s.contains("0 modified"));
        assert!(s.contains("0 deleted"));
        assert!(s.contains("1 unchanged"));

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

        let out = run_ftm_with_port(port, &["scan"]);
        assert!(out.status.success());
        assert!(String::from_utf8_lossy(&out.stdout).contains("1 created"));

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
        pre_init_ftm(dir.path(), 100, 100, None, None);

        // Create files BEFORE checkout
        std::fs::write(dir.path().join("small.txt"), "tiny").unwrap();
        std::fs::write(dir.path().join("large.txt"), "x".repeat(200)).unwrap();

        let (mut server, port) = start_server_and_checkout(dir.path());

        let out = run_ftm_with_port(port, &["scan"]);
        assert!(out.status.success());
        assert!(String::from_utf8_lossy(&out.stdout).contains("1 created"));

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

        let out = run_ftm_with_port(port, &["scan"]);
        assert!(out.status.success());
        assert!(String::from_utf8_lossy(&out.stdout).contains("2 created"));

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

        let out = run_ftm_with_port(port, &["scan"]);
        assert!(out.status.success());
        assert!(String::from_utf8_lossy(&out.stdout).contains("1 created"));

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

        let out = run_ftm_with_port(port, &["scan"]);
        assert!(out.status.success());
        assert!(String::from_utf8_lossy(&out.stdout).contains("1 created"));

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

        let out = run_ftm_with_port(port, &["scan"]);
        assert!(out.status.success());
        assert!(String::from_utf8_lossy(&out.stdout).contains("2 created"));

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

mod clean_tests {
    use super::*;

    #[test]
    fn test_clean_not_checked_out() {
        let (mut server, port) = start_server();

        let out = run_ftm_with_port(port, &["clean"]);
        assert!(!out.status.success());
        assert!(String::from_utf8_lossy(&out.stderr).contains("No directory checked out"));

        stop_server(&mut server);
    }

    #[test]
    fn test_clean_removes_orphan_snapshots() {
        let dir = setup_test_dir();
        pre_init_ftm(dir.path(), 1, 30 * 1024 * 1024, None, None);

        std::fs::write(dir.path().join("clean_orphan.yaml"), "v1").unwrap();

        let (mut server, port) = start_server_and_checkout(dir.path());

        let out = run_ftm_with_port(port, &["scan"]);
        assert!(out.status.success());
        assert!(String::from_utf8_lossy(&out.stdout).contains("1 created"));

        std::fs::write(dir.path().join("clean_orphan.yaml"), "v2").unwrap();
        let out = run_ftm_with_port(port, &["scan"]);
        assert!(out.status.success());
        assert!(String::from_utf8_lossy(&out.stdout).contains("1 modified"));

        let index = load_test_index(dir.path());
        let entries: Vec<_> = index
            .history
            .iter()
            .filter(|e| e.file == "clean_orphan.yaml")
            .collect();
        assert_eq!(
            entries.len(),
            1,
            "max_history=1 should trim to single entry"
        );
        let snap_before = count_snapshot_files(dir.path());
        assert_eq!(
            snap_before, 2,
            "Two snapshots on disk before clean (v1 orphan + v2)"
        );

        let out = run_ftm_with_port(port, &["clean"]);
        assert!(out.status.success());
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            stdout.contains("1 snapshot(s) removed"),
            "Expected '1 snapshot(s) removed' in: {}",
            stdout
        );

        let snap_after = count_snapshot_files(dir.path());
        assert_eq!(
            snap_after, 1,
            "One snapshot should remain after clean, got {}",
            snap_after
        );

        let out = run_ftm_with_port(port, &["history", "clean_orphan.yaml"]);
        assert!(out.status.success());
        let entry = index
            .history
            .iter()
            .find(|e| e.file == "clean_orphan.yaml")
            .unwrap();
        let checksum = entry.checksum.as_ref().unwrap();
        let out = run_ftm_with_port(port, &["restore", "clean_orphan.yaml", &checksum[..8]]);
        assert!(out.status.success());
        let content = std::fs::read_to_string(dir.path().join("clean_orphan.yaml")).unwrap();
        assert_eq!(content, "v2", "Restore should yield current version");

        stop_server(&mut server);
    }

    #[test]
    fn test_periodic_clean_removes_orphans_after_interval() {
        let dir = setup_test_dir();
        pre_init_ftm(dir.path(), 1, 30 * 1024 * 1024, None, Some(2));

        std::fs::write(dir.path().join("periodic_clean.yaml"), "v1").unwrap();

        let (mut server, port) = start_server_and_checkout(dir.path());

        let out = run_ftm_with_port(port, &["scan"]);
        assert!(out.status.success());
        std::fs::write(dir.path().join("periodic_clean.yaml"), "v2").unwrap();
        let out = run_ftm_with_port(port, &["scan"]);
        assert!(out.status.success());

        let snap_before = count_snapshot_files(dir.path());
        assert_eq!(
            snap_before, 2,
            "Two snapshots before periodic clean (v1 orphan + v2)"
        );

        std::thread::sleep(std::time::Duration::from_secs(4));

        let snap_after = count_snapshot_files(dir.path());
        assert_eq!(
            snap_after, 1,
            "Periodic clean should remove orphan; expected 1 snapshot, got {}",
            snap_after
        );

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
        let out = run_ftm_with_port(19999, &["version"]);
        assert!(out.status.success());
        let s = String::from_utf8_lossy(&out.stdout);
        assert!(s.contains("Client version:"));
        assert!(s.contains("not running"));
    }

    #[test]
    fn test_version_with_server() {
        let (mut server, port) = start_server();

        let out = run_ftm_with_port(port, &["version"]);
        assert!(out.status.success());
        let s = String::from_utf8_lossy(&out.stdout);
        assert!(s.contains("Client version:"));
        assert!(s.contains("Server version:"));

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

        let out = run_ftm_with_port(port, &["config", "get"]);
        assert!(out.status.success());
        let s = String::from_utf8_lossy(&out.stdout);
        assert!(s.contains("max_history"));
        assert!(s.contains("patterns"));

        stop_server(&mut server);
    }

    #[test]
    fn test_config_get_single_key() {
        let dir = setup_test_dir();
        let (mut server, port) = start_server_and_checkout(dir.path());

        let out = run_ftm_with_port(port, &["config", "get", "settings.max_history"]);
        assert!(out.status.success());
        assert!(String::from_utf8_lossy(&out.stdout).contains("10000"));

        stop_server(&mut server);
    }

    #[test]
    fn test_config_get_invalid_key() {
        let dir = setup_test_dir();
        let (mut server, port) = start_server_and_checkout(dir.path());

        let out = run_ftm_with_port(port, &["config", "get", "nonexistent.key"]);
        assert!(!out.status.success());
        assert!(String::from_utf8_lossy(&out.stderr).contains("Unknown config key"));

        stop_server(&mut server);
    }

    #[test]
    fn test_config_set_and_get() {
        let dir = setup_test_dir();
        let (mut server, port) = start_server_and_checkout(dir.path());

        // Set max_history to 200
        let out = run_ftm_with_port(port, &["config", "set", "settings.max_history", "200"]);
        assert!(out.status.success());
        assert!(String::from_utf8_lossy(&out.stdout).contains("Set settings.max_history = 200"));

        // Verify it was changed
        let out = run_ftm_with_port(port, &["config", "get", "settings.max_history"]);
        assert!(out.status.success());
        assert!(String::from_utf8_lossy(&out.stdout).contains("200"));

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
        let out = run_ftm_with_port(
            port,
            &["config", "set", "settings.max_history", "not_a_number"],
        );
        assert!(!out.status.success());
        assert!(String::from_utf8_lossy(&out.stderr).contains("Invalid value"));

        stop_server(&mut server);
    }

    #[test]
    fn test_config_set_scan_interval_minimum_2() {
        let dir = setup_test_dir();
        let (mut server, port) = start_server_and_checkout(dir.path());

        let out = run_ftm_with_port(port, &["config", "set", "settings.scan_interval", "1"]);
        assert!(!out.status.success());
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains("scan_interval must be >= 2") || stderr.contains(">= 2"),
            "expected scan_interval minimum 2 error, got: {}",
            stderr
        );

        let out = run_ftm_with_port(port, &["config", "set", "settings.scan_interval", "2"]);
        assert!(out.status.success());

        stop_server(&mut server);
    }

    #[test]
    fn test_config_set_watch_patterns() {
        let dir = setup_test_dir();
        let (mut server, port) = start_server_and_checkout(dir.path());

        let out = run_ftm_with_port(port, &["config", "set", "watch.patterns", "*.rs,*.go,*.py"]);
        assert!(out.status.success());

        let out = run_ftm_with_port(port, &["config", "get", "watch.patterns"]);
        assert!(out.status.success());
        let s = String::from_utf8_lossy(&out.stdout);
        assert!(s.contains("*.rs"));
        assert!(s.contains("*.go"));
        assert!(s.contains("*.py"));

        stop_server(&mut server);
    }

    #[test]
    fn test_config_not_checked_out() {
        let (mut server, port) = start_server();

        let out = run_ftm_with_port(port, &["config", "get"]);
        assert!(!out.status.success());
        assert!(String::from_utf8_lossy(&out.stderr).contains("No directory checked out"));

        stop_server(&mut server);
    }
}

// ===========================================================================
// Config hot-reload tests
// ===========================================================================

mod config_hot_reload_tests {
    use super::*;

    /// After `config set watch.patterns`, the watcher should immediately use
    /// the new patterns — newly added extensions get tracked.
    #[test]
    fn test_config_set_patterns_adds_new_extension_to_watcher() {
        let dir = setup_test_dir();
        let (mut server, port) = start_server_and_checkout(dir.path());

        // Default patterns do NOT include *.go
        // Add *.go via config set
        let out = run_ftm_with_port(
            port,
            &["config", "set", "watch.patterns", "*.rs,*.go,*.yaml"],
        );
        assert!(out.status.success());

        // Write a .go file — should now be tracked
        std::fs::write(dir.path().join("main.go"), "package main").unwrap();

        assert!(
            wait_for_index(dir.path(), "main.go", 1, 3000),
            "After adding *.go to patterns, .go files should be tracked by the watcher"
        );

        stop_server(&mut server);
    }

    /// After `config set watch.patterns` to remove an extension, the watcher
    /// should stop tracking files with that extension.
    #[test]
    fn test_config_set_patterns_removes_extension_from_watcher() {
        let dir = setup_test_dir();
        let (mut server, port) = start_server_and_checkout(dir.path());

        // Verify .yaml is tracked by default
        std::fs::write(dir.path().join("before.yaml"), "before: change").unwrap();
        assert!(
            wait_for_index(dir.path(), "before.yaml", 1, 2000),
            "before.yaml should be tracked with default patterns"
        );

        // Remove *.yaml from patterns (keep only *.rs)
        let out = run_ftm_with_port(port, &["config", "set", "watch.patterns", "*.rs"]);
        assert!(out.status.success());

        // Write a .yaml file — should NOT be tracked anymore
        std::fs::write(dir.path().join("after.yaml"), "after: change").unwrap();

        // Write a .rs file as sync marker — should be tracked
        std::fs::write(dir.path().join("sync.rs"), "fn sync() {}").unwrap();
        assert!(
            wait_for_index(dir.path(), "sync.rs", 1, 2000),
            "sync.rs should be tracked (proves watcher is still running)"
        );

        let index = load_test_index(dir.path());
        assert!(
            !index.history.iter().any(|e| e.file == "after.yaml"),
            "after.yaml should NOT be tracked after removing *.yaml from patterns"
        );

        stop_server(&mut server);
    }

    /// After `config set watch.patterns`, manual scan should use the new patterns.
    #[test]
    fn test_config_set_patterns_applied_to_manual_scan() {
        let dir = setup_test_dir();

        // Create a .go file BEFORE checkout (watcher won't see it)
        std::fs::write(dir.path().join("lib.go"), "package lib").unwrap();

        let (mut server, port) = start_server_and_checkout(dir.path());

        // Default scan should NOT pick up .go files
        let out = run_ftm_with_port(port, &["scan"]);
        assert!(out.status.success());
        assert!(String::from_utf8_lossy(&out.stdout).contains("0 created"));

        // Add *.go to patterns
        let out = run_ftm_with_port(port, &["config", "set", "watch.patterns", "*.rs,*.go"]);
        assert!(out.status.success());

        // Scan again — should now find the .go file
        let out = run_ftm_with_port(port, &["scan"]);
        assert!(out.status.success());
        assert!(String::from_utf8_lossy(&out.stdout).contains("1 created"));

        let index = load_test_index(dir.path());
        assert!(
            index.history.iter().any(|e| e.file == "lib.go"),
            "lib.go should appear in history after pattern change + scan"
        );

        stop_server(&mut server);
    }

    /// After `config set settings.scan_interval` to a shorter value,
    /// the new interval takes effect immediately (within ~1s).
    #[test]
    fn test_config_set_scan_interval_enables_periodic_scan() {
        let dir = setup_test_dir();

        std::fs::write(
            dir.path().join("pre_existing.txt"),
            "created before checkout",
        )
        .unwrap();

        // Pre-init with 8s interval; no scan in 1s
        pre_init_ftm(dir.path(), 100, 30 * 1024 * 1024, Some(8), None);

        let (mut server, port) = start_server_and_checkout(dir.path());

        std::thread::sleep(std::time::Duration::from_secs(1));
        let index = load_test_index(dir.path());
        assert!(
            !index.history.iter().any(|e| e.file == "pre_existing.txt"),
            "With 8s scan_interval, file should not be scanned in 1s"
        );

        // Shorten to 2s; takes effect on next tick (~1s), then 2s wait, then scan
        let out = run_ftm_with_port(port, &["config", "set", "settings.scan_interval", "2"]);
        assert!(out.status.success());

        let found = wait_for_index(dir.path(), "pre_existing.txt", 1, 5000);
        assert!(
            found,
            "After setting scan_interval=2, periodic scanner should pick up pre_existing.txt"
        );

        stop_server(&mut server);
    }

    /// After `config set settings.max_file_size`, scan should respect the new limit.
    #[test]
    fn test_config_set_max_file_size_applied_to_scan() {
        let dir = setup_test_dir();

        // Create a 200-byte file BEFORE checkout
        std::fs::write(dir.path().join("medium.txt"), "x".repeat(200)).unwrap();

        // Pre-init with max_file_size=100 — file will be skipped
        pre_init_ftm(dir.path(), 100, 100, None, None);

        let (mut server, port) = start_server_and_checkout(dir.path());

        // Scan — file exceeds 100 bytes, should be skipped
        let out = run_ftm_with_port(port, &["scan"]);
        assert!(out.status.success());
        assert!(String::from_utf8_lossy(&out.stdout).contains("0 created"));

        // Raise max_file_size to 1000
        let out = run_ftm_with_port(port, &["config", "set", "settings.max_file_size", "1000"]);
        assert!(out.status.success());

        // Scan again — file should now be picked up
        let out = run_ftm_with_port(port, &["scan"]);
        assert!(out.status.success());
        assert!(String::from_utf8_lossy(&out.stdout).contains("1 created"));

        let index = load_test_index(dir.path());
        assert!(
            index.history.iter().any(|e| e.file == "medium.txt"),
            "medium.txt should be tracked after raising max_file_size"
        );

        stop_server(&mut server);
    }

    /// After `config set watch.exclude`, the watcher should respect the new
    /// exclude patterns.
    #[test]
    fn test_config_set_exclude_applied_to_watcher() {
        let dir = setup_test_dir();
        let custom_dir = dir.path().join("custom");
        std::fs::create_dir_all(&custom_dir).unwrap();

        let (mut server, port) = start_server_and_checkout(dir.path());

        // Verify files in custom/ ARE tracked before exclude change
        std::fs::write(custom_dir.join("before.yaml"), "tracked: yes").unwrap();
        assert!(
            wait_for_index(dir.path(), "custom/before.yaml", 1, 2000),
            "custom/before.yaml should be tracked before exclude change"
        );

        // Add **/custom/** to exclude patterns
        let out = run_ftm_with_port(
            port,
            &[
                "config",
                "set",
                "watch.exclude",
                "**/target/**,**/node_modules/**,**/.git/**,**/.ftm/**,**/custom/**",
            ],
        );
        assert!(out.status.success());

        // Write a new file in custom/ — should NOT be tracked
        std::fs::write(custom_dir.join("after.yaml"), "tracked: no").unwrap();

        // Write a sync marker in root — should be tracked
        std::fs::write(dir.path().join("sync.yaml"), "sync: yes").unwrap();
        assert!(
            wait_for_index(dir.path(), "sync.yaml", 1, 2000),
            "sync.yaml should be tracked (proves watcher still running)"
        );

        let index = load_test_index(dir.path());
        assert!(
            !index.history.iter().any(|e| e.file == "custom/after.yaml"),
            "custom/after.yaml should NOT be tracked after adding **/custom/** to exclude"
        );

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

        // The server auto-creates a log file on startup, so "logs" should
        // find it and print "Opening: ..." instead of "No log files".
        let out = run_ftm_with_port(port, &["logs"]);
        assert!(out.status.success());
        assert!(String::from_utf8_lossy(&out.stdout).contains("Opening:"));

        stop_server(&mut server);
    }

    #[test]
    fn test_logs_with_log_file() {
        let dir = setup_test_dir();
        let (mut server, port) = start_server_and_checkout(dir.path());

        // Create a log file with a far-future timestamp so it is picked as
        // the newest (the server auto-creates its own log on startup).
        let log_dir = dir.path().join(".ftm/logs");
        std::fs::create_dir_all(&log_dir).unwrap();
        std::fs::write(
            log_dir.join("30000101-120000.log"),
            "INFO test log line 1\nINFO test log line 2\n",
        )
        .unwrap();

        // logs command should find the file and try less, then fallback to print
        let out = run_ftm_with_port(port, &["logs"]);
        assert!(out.status.success());
        assert!(String::from_utf8_lossy(&out.stdout).contains("30000101-120000.log"));

        stop_server(&mut server);
    }

    #[test]
    fn test_logs_picks_latest_file() {
        let dir = setup_test_dir();
        let (mut server, port) = start_server_and_checkout(dir.path());

        // Create multiple log files with far-future timestamps so both are
        // newer than the server's auto-created log file.
        let log_dir = dir.path().join(".ftm/logs");
        std::fs::create_dir_all(&log_dir).unwrap();
        std::fs::write(log_dir.join("30000101-100000.log"), "old log\n").unwrap();
        std::fs::write(log_dir.join("30000201-150000.log"), "new log\n").unwrap();

        // Should pick the newest one (30000201-150000.log)
        let out = run_ftm_with_port(port, &["logs"]);
        assert!(out.status.success());
        assert!(String::from_utf8_lossy(&out.stdout).contains("30000201-150000.log"));

        stop_server(&mut server);
    }

    #[test]
    fn test_logs_not_checked_out() {
        let (mut server, port) = start_server();

        let out = run_ftm_with_port(port, &["logs"]);
        assert!(!out.status.success());
        assert!(String::from_utf8_lossy(&out.stderr).contains("No directory checked out"));

        stop_server(&mut server);
    }

    /// Pruning: when server starts with file logging, only the 100 most recent log files are kept.
    #[test]
    fn test_logs_prune_keeps_only_100() {
        const KEEP: usize = 100;
        let total_before = 105;

        let dir = setup_test_dir();
        let log_dir = dir.path().join(".ftm/logs");
        std::fs::create_dir_all(&log_dir).unwrap();

        for i in 0..total_before {
            let name = format!("20000101-000000.{:03}.log", i);
            std::fs::write(log_dir.join(&name), format!("log content {}\n", i)).unwrap();
        }
        let count_before: usize = std::fs::read_dir(&log_dir)
            .unwrap()
            .filter(|e| {
                e.as_ref()
                    .ok()
                    .and_then(|e| e.path().extension().map(|ext| ext == "log"))
                    .unwrap_or(false)
            })
            .count();
        assert_eq!(
            count_before, total_before,
            "should have 105 log files before checkout"
        );

        let (mut server, _port) = start_server_and_checkout(dir.path());
        stop_server(&mut server);

        let entries: Vec<_> = std::fs::read_dir(&log_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().map_or(false, |ext| ext == "log"))
            .collect();
        assert_eq!(
            entries.len(),
            KEEP + 1,
            "after prune: 100 kept + 1 new server log = 101 total"
        );
        let names: Vec<String> = entries
            .iter()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert!(
            !names.iter().any(|n| n == "20000101-000000.000.log"),
            "oldest file should be pruned"
        );
        assert!(
            names.iter().any(|n| n == "20000101-000000.005.log"),
            "file just after prune cutoff should still exist"
        );
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
        let out = run_ftm_with_port(port, &["ls"]);
        assert!(
            out.status.success(),
            "ls: {}",
            String::from_utf8_lossy(&out.stderr)
        );

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

        // Pre-init with 2s scan interval (minimum)
        pre_init_ftm(dir.path(), 100, 30 * 1024 * 1024, Some(2), None);

        let (mut server, _port) = start_server_and_checkout(dir.path());

        let found = wait_for_index(dir.path(), "pre_existing.txt", 1, 5000);
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
    fn test_periodic_scan_respects_interval() {
        let dir = setup_test_dir();

        // Create a file BEFORE checkout
        std::fs::write(dir.path().join("should_not_scan.txt"), "no scan").unwrap();

        // Pre-init with 5s interval so no scan runs within 2s
        pre_init_ftm(dir.path(), 100, 30 * 1024 * 1024, Some(5), None);

        let (mut server, _port) = start_server_and_checkout(dir.path());

        std::thread::sleep(std::time::Duration::from_secs(2));

        let index = load_test_index(dir.path());
        let entries: Vec<_> = index
            .history
            .iter()
            .filter(|e| e.file == "should_not_scan.txt")
            .collect();
        assert!(
            entries.is_empty(),
            "With 5s scan_interval, no periodic scan should run within 2s"
        );

        stop_server(&mut server);
    }
}
