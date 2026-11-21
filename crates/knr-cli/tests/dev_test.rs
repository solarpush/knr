use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(suffix: &str) -> Self {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let id = COUNTER.fetch_add(1, Ordering::SeqCst);
        let pid = std::process::id();
        let dir_name = format!("knr_dev_test_{pid}_{id}_{suffix}");
        let path = std::env::temp_dir().join(dir_name);
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("failed to create temp dir");
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn knr_cmd() -> Command {
    let bin = env!("CARGO_BIN_EXE_knr");
    Command::new(bin)
}

#[test]
fn test_dev_help_flags() {
    let out_long = knr_cmd()
        .args(["dev", "--help"])
        .output()
        .expect("knr dev --help");
    assert!(out_long.status.success());
    let stdout_long = String::from_utf8_lossy(&out_long.stdout);
    assert!(stdout_long.contains("knr dev"));
    assert!(stdout_long.contains("--debounce"));
    assert!(stdout_long.contains("--poll"));
    assert!(stdout_long.contains("--once"));

    let out_short = knr_cmd()
        .args(["dev", "-h"])
        .output()
        .expect("knr dev -h");
    assert!(out_short.status.success());
}

#[test]
fn test_dev_once_mode() {
    let temp = TempDir::new("once");
    let init_out = knr_cmd()
        .args(["init", temp.path().to_str().unwrap()])
        .output()
        .expect("init app");
    assert!(init_out.status.success());

    let dev_out = knr_cmd()
        .args(["dev", temp.path().to_str().unwrap(), "--once"])
        .output()
        .expect("knr dev --once");
    assert!(dev_out.status.success(), "knr dev --once should succeed");
    let stdout = String::from_utf8_lossy(&dev_out.stdout);
    assert!(stdout.contains('0'), "Output should contain return value 0: {stdout}");
}

#[test]
fn test_dev_test_once_mode() {
    let temp = TempDir::new("test_once");
    let init_out = knr_cmd()
        .args(["init", temp.path().to_str().unwrap()])
        .output()
        .expect("init app");
    assert!(init_out.status.success());

    let dev_out = knr_cmd()
        .args(["dev", temp.path().to_str().unwrap(), "--test-once"])
        .output()
        .expect("knr dev --test-once");
    assert!(dev_out.status.success(), "knr dev --test-once should succeed");
}

#[test]
fn test_run_watch_flag() {
    let temp = TempDir::new("run_watch");
    let init_out = knr_cmd()
        .args(["init", temp.path().to_str().unwrap()])
        .output()
        .expect("init app");
    assert!(init_out.status.success());

    let entry = temp.path().join("src/main.knr");
    let run_out = knr_cmd()
        .args(["run", entry.to_str().unwrap(), "--watch", "--once"])
        .output()
        .expect("run --watch --once");

    assert!(run_out.status.success(), "knr run --watch --once should succeed");
    let stdout = String::from_utf8_lossy(&run_out.stdout);
    assert!(stdout.contains('0'), "Output should contain return value 0: {stdout}");

    let help_out = knr_cmd().args(["run", "--help"]).output().unwrap();
    let help_txt = String::from_utf8_lossy(&help_out.stdout);
    assert!(help_txt.contains("--watch") || help_txt.contains("-w"));
}

#[test]
fn test_dev_detects_file_modification() {
    let temp = TempDir::new("modify");
    let init_out = knr_cmd()
        .args(["init", temp.path().to_str().unwrap()])
        .output()
        .expect("init");
    assert!(init_out.status.success());

    let mut child = knr_cmd()
        .args([
            "dev",
            temp.path().to_str().unwrap(),
            "--poll",
            "40",
            "--debounce",
            "40",
            "--no-clear",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn knr dev");

    // Wait 300ms for initial cycle
    std::thread::sleep(Duration::from_millis(300));

    // Modify file
    let entry = temp.path().join("src/main.knr");
    let new_src = r#"// Modified application
fn main() -> i64 {
    let code = 42;
    return code;
}
"#;
    fs::write(&entry, new_src).expect("write modified knr");

    // Wait 500ms for change detection and re-run
    std::thread::sleep(Duration::from_millis(500));

    // Kill process
    let _ = child.kill();
    let _ = child.wait();

    let mut stdout_buf = Vec::new();
    if let Some(mut out) = child.stdout.take() {
        let _ = out.read_to_end(&mut stdout_buf);
    }
    let stdout_str = String::from_utf8_lossy(&stdout_buf);

    assert!(
        stdout_str.contains("42"),
        "knr dev stdout should contain updated return value 42: {stdout_str}"
    );
}

#[test]
fn test_dev_debounce_batching() {
    let temp = TempDir::new("debounce");
    let init_out = knr_cmd()
        .args(["init", temp.path().to_str().unwrap()])
        .output()
        .expect("init");
    assert!(init_out.status.success());

    let mut child = knr_cmd()
        .args([
            "dev",
            temp.path().to_str().unwrap(),
            "--poll",
            "50",
            "--debounce",
            "150",
            "--no-clear",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn knr dev");

    std::thread::sleep(Duration::from_millis(300));

    let entry = temp.path().join("src/main.knr");
    // Perform 5 rapid writes within 30ms
    for i in 1..=5 {
        let src = format!("fn main() -> i64 {{\n    return {i};\n}}\n");
        let _ = fs::write(&entry, src);
        std::thread::sleep(Duration::from_millis(5));
    }

    std::thread::sleep(Duration::from_millis(500));

    let _ = child.kill();
    let _ = child.wait();

    let mut stdout_buf = Vec::new();
    if let Some(mut out) = child.stdout.take() {
        let _ = out.read_to_end(&mut stdout_buf);
    }
    let stdout_str = String::from_utf8_lossy(&stdout_buf);

    assert!(
        stdout_str.contains('5'),
        "Expected final debounced value 5 in stdout: {stdout_str}"
    );
}

#[test]
fn test_dev_resilience_to_syntax_error() {
    let temp = TempDir::new("syntax_resilience");
    let init_out = knr_cmd()
        .args(["init", temp.path().to_str().unwrap()])
        .output()
        .expect("init");
    assert!(init_out.status.success());

    let mut child = knr_cmd()
        .args([
            "dev",
            temp.path().to_str().unwrap(),
            "--poll",
            "40",
            "--debounce",
            "40",
            "--no-clear",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn knr dev");

    std::thread::sleep(Duration::from_millis(300));

    let entry = temp.path().join("src/main.knr");

    // Introduce syntax error
    fs::write(&entry, "INVALID SYNTAX // !!!").expect("write invalid syntax");
    std::thread::sleep(Duration::from_millis(400));

    // Verify watcher child is STILL alive (did not crash or exit on error)
    let try_wait = child.try_wait().expect("try_wait failed");
    assert!(
        try_wait.is_none(),
        "knr dev process must NOT exit on syntax error; it must remain alive!"
    );

    // Fix syntax
    let fixed_src = r#"// Fixed
fn main() -> i64 {
    return 777;
}
"#;
    fs::write(&entry, fixed_src).expect("write fixed syntax");
    std::thread::sleep(Duration::from_millis(500));

    // Kill process
    let _ = child.kill();
    let _ = child.wait();

    let mut stdout_buf = Vec::new();
    if let Some(mut out) = child.stdout.take() {
        let _ = out.read_to_end(&mut stdout_buf);
    }
    let stdout_str = String::from_utf8_lossy(&stdout_buf);

    assert!(
        stdout_str.contains("777"),
        "knr dev should have recovered and executed fixed code returning 777: {stdout_str}"
    );
}
