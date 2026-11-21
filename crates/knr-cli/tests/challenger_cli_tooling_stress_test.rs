use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(suffix: &str) -> Self {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let id = COUNTER.fetch_add(1, Ordering::SeqCst);
        let pid = std::process::id();
        let dir_name = format!("knr_challenger_stress_{pid}_{id}_{suffix}");
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

struct RunningProcess {
    child: Child,
    stdout: Arc<Mutex<String>>,
    stderr: Arc<Mutex<String>>,
}

impl RunningProcess {
    fn spawn(mut cmd: Command) -> Self {
        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = cmd.spawn().expect("failed to spawn child process");

        let stdout_data = Arc::new(Mutex::new(String::new()));
        let stderr_data = Arc::new(Mutex::new(String::new()));

        if let Some(mut out) = child.stdout.take() {
            let data = Arc::clone(&stdout_data);
            thread::spawn(move || {
                let mut buf = [0u8; 1024];
                while let Ok(n) = out.read(&mut buf) {
                    if n == 0 {
                        break;
                    }
                    let chunk = String::from_utf8_lossy(&buf[..n]);
                    let mut lock = data.lock().unwrap();
                    lock.push_str(&chunk);
                }
            });
        }

        if let Some(mut err) = child.stderr.take() {
            let data = Arc::clone(&stderr_data);
            thread::spawn(move || {
                let mut buf = [0u8; 1024];
                while let Ok(n) = err.read(&mut buf) {
                    if n == 0 {
                        break;
                    }
                    let chunk = String::from_utf8_lossy(&buf[..n]);
                    let mut lock = data.lock().unwrap();
                    lock.push_str(&chunk);
                }
            });
        }

        Self {
            child,
            stdout: stdout_data,
            stderr: stderr_data,
        }
    }

    fn stdout_text(&self) -> String {
        self.stdout.lock().unwrap().clone()
    }

    fn stderr_text(&self) -> String {
        self.stderr.lock().unwrap().clone()
    }

    fn wait_for_stdout(&self, target: &str, timeout: Duration) -> bool {
        let start = Instant::now();
        while start.elapsed() < timeout {
            if self.stdout_text().contains(target) {
                return true;
            }
            thread::sleep(Duration::from_millis(15));
        }
        false
    }

    fn wait_for_stderr(&self, target: &str, timeout: Duration) -> bool {
        let start = Instant::now();
        while start.elapsed() < timeout {
            if self.stderr_text().contains(target) {
                return true;
            }
            thread::sleep(Duration::from_millis(15));
        }
        false
    }

    fn is_alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    fn kill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for RunningProcess {
    fn drop(&mut self) {
        self.kill();
    }
}

// ============================================================================
// 1. Debounce Window & Rapid Modification Burst Stress Testing
// ============================================================================

#[test]
fn test_stress_debounce_burst_modifications() {
    let temp = TempDir::new("debounce_burst");
    let init_out = knr_cmd()
        .args(["init", temp.path().to_str().unwrap()])
        .output()
        .expect("init");
    assert!(init_out.status.success());

    let entry = temp.path().join("src/main.knr");
    let mut proc = RunningProcess::spawn({
        let mut cmd = knr_cmd();
        cmd.args([
            "dev",
            temp.path().to_str().unwrap(),
            "--poll",
            "25",
            "--debounce",
            "250",
            "--no-clear",
        ]);
        cmd
    });

    assert!(
        proc.wait_for_stdout("Ready. Watching for changes", Duration::from_secs(3)),
        "Watcher failed initial start: stdout={}",
        proc.stdout_text()
    );

    // Snapshot stdout before burst
    let stdout_before = proc.stdout_text();

    // Fire 20 rapid file modifications within ~60ms (well inside the 150ms debounce window)
    for i in 1..=20 {
        let src = format!("fn main() -> i64 {{\n    return {i};\n}}\n");
        let _ = fs::write(&entry, src);
        thread::sleep(Duration::from_millis(3));
    }

    // Wait for the watcher to settle and run the final debounced code
    assert!(
        proc.wait_for_stdout("\n20\n", Duration::from_secs(4)),
        "Watcher did not execute final debounced value 20! Stdout: {}",
        proc.stdout_text()
    );

    assert!(proc.is_alive(), "Watcher died after modification burst");

    // Check that intermediate writes did not each trigger an execution cycle
    let stdout_after = proc.stdout_text();
    let new_output = &stdout_after[stdout_before.len()..];
    let change_event_count = new_output.matches("knr dev — File changed").count();

    assert!(
        change_event_count <= 2,
        "Debounce failed to batch rapid writes: observed {change_event_count} change events (expected <= 2)"
    );
}

#[test]
fn test_stress_multi_file_simultaneous_save_batching() {
    let temp = TempDir::new("multi_file_burst");
    let init_out = knr_cmd()
        .args(["init", temp.path().to_str().unwrap()])
        .output()
        .expect("init");
    assert!(init_out.status.success());

    let main_knr = temp.path().join("src/main.knr");
    let helper_knr = temp.path().join("src/helper.knr");
    fs::write(&helper_knr, "schema Config { 0: flag i64 }\n").expect("write helper");

    let mut proc = RunningProcess::spawn({
        let mut cmd = knr_cmd();
        cmd.args([
            "dev",
            temp.path().to_str().unwrap(),
            "--poll",
            "25",
            "--debounce",
            "250",
            "--no-clear",
        ]);
        cmd
    });

    assert!(proc.wait_for_stdout("Ready. Watching for changes", Duration::from_secs(3)));
    let before_stdout = proc.stdout_text();

    // Concurrently write both helper.knr and main.knr within 10ms
    fs::write(&helper_knr, "schema Config { 0: flag i64, 1: extra bool }\n").expect("write helper");
    thread::sleep(Duration::from_millis(5));
    fs::write(&main_knr, "fn main() -> i64 {\n    return 951;\n}\n").expect("write main");

    assert!(
        proc.wait_for_stdout("\n951\n", Duration::from_secs(4)),
        "Watcher missed batched multi-file save: stdout={}",
        proc.stdout_text()
    );

    let after_stdout = proc.stdout_text();
    let new_output = &after_stdout[before_stdout.len()..];
    let change_events = new_output.matches("knr dev — File changed").count();
    assert!(
        change_events <= 2,
        "Debounce failed to coalesce multi-file save: {change_events} events"
    );
    assert!(proc.is_alive());
}

#[test]
fn test_stress_zero_debounce() {
    let temp = TempDir::new("zero_debounce");
    let init_out = knr_cmd()
        .args(["init", temp.path().to_str().unwrap()])
        .output()
        .expect("init");
    assert!(init_out.status.success());

    let entry = temp.path().join("src/main.knr");
    let mut proc = RunningProcess::spawn({
        let mut cmd = knr_cmd();
        cmd.args([
            "dev",
            temp.path().to_str().unwrap(),
            "--poll",
            "20",
            "--debounce",
            "0",
            "--no-clear",
        ]);
        cmd
    });

    assert!(proc.wait_for_stdout("Ready. Watching for changes", Duration::from_secs(3)));

    let new_src = "fn main() -> i64 {\n    return 42;\n}\n";
    fs::write(&entry, new_src).expect("write");

    assert!(
        proc.wait_for_stdout("\n42\n", Duration::from_secs(3)),
        "Zero-debounce watcher failed to execute update: {}",
        proc.stdout_text()
    );
    assert!(proc.is_alive());
}

// ============================================================================
// 2. Transient Syntax & Semantic Corruption Resilience
// ============================================================================

#[test]
fn test_stress_transient_syntax_corruption_and_recovery() {
    let temp = TempDir::new("syntax_corruption");
    let init_out = knr_cmd()
        .args(["init", temp.path().to_str().unwrap()])
        .output()
        .expect("init");
    assert!(init_out.status.success());

    let entry = temp.path().join("src/main.knr");
    let mut proc = RunningProcess::spawn({
        let mut cmd = knr_cmd();
        cmd.args([
            "dev",
            temp.path().to_str().unwrap(),
            "--poll",
            "25",
            "--debounce",
            "40",
            "--no-clear",
        ]);
        cmd
    });

    assert!(proc.wait_for_stdout("Ready. Watching for changes", Duration::from_secs(3)));

    // Inject invalid syntax
    fs::write(&entry, "$$$ INVALID TOKENS &&& {{{").expect("write invalid");

    // Watcher must log syntax error to stderr
    assert!(
        proc.wait_for_stderr("Syntax Error", Duration::from_secs(3)),
        "Expected Syntax Error in stderr. Stderr: {}",
        proc.stderr_text()
    );

    // Watcher MUST NOT terminate on syntax error
    assert!(proc.is_alive(), "Watcher crashed on syntax error!");

    // Recover with valid syntax returning 888
    let recovered_src = "fn main() -> i64 {\n    return 888;\n}\n";
    fs::write(&entry, recovered_src).expect("write recovered");

    // Watcher must cleanly re-execute and report 888
    assert!(
        proc.wait_for_stdout("\n888\n", Duration::from_secs(3)),
        "Watcher failed to recover after syntax error: stdout={}",
        proc.stdout_text()
    );
    assert!(proc.is_alive(), "Watcher died after recovering");
}

#[test]
fn test_stress_transient_verification_error_and_recovery() {
    let temp = TempDir::new("verify_corruption");
    let init_out = knr_cmd()
        .args(["init", temp.path().to_str().unwrap()])
        .output()
        .expect("init");
    assert!(init_out.status.success());

    let entry = temp.path().join("src/main.knr");
    let mut proc = RunningProcess::spawn({
        let mut cmd = knr_cmd();
        cmd.args([
            "dev",
            temp.path().to_str().unwrap(),
            "--poll",
            "25",
            "--debounce",
            "40",
            "--no-clear",
        ]);
        cmd
    });

    assert!(proc.wait_for_stdout("Ready. Watching for changes", Duration::from_secs(3)));

    // Code with semantic verification error (undeclared variable)
    let bad_code = r#"
fn main() -> i64 {
    let x = undeclared_variable_name;
    return x;
}
"#;
    fs::write(&entry, bad_code).expect("write semantically invalid code");

    assert!(
        proc.wait_for_stderr("Verification failed", Duration::from_secs(3)),
        "Expected Verification failed in stderr: {}",
        proc.stderr_text()
    );
    assert!(proc.is_alive(), "Watcher crashed on verification error!");

    // Fix verification error
    let fixed_code = r#"
fn main() -> i64 {
    let x = 555;
    return x;
}
"#;
    fs::write(&entry, fixed_code).expect("write fixed code");

    assert!(
        proc.wait_for_stdout("\n555\n", Duration::from_secs(3)),
        "Watcher failed to recover after verification error: {}",
        proc.stdout_text()
    );
    assert!(proc.is_alive());
}

#[test]
fn test_stress_oscillating_corruption_storm() {
    let temp = TempDir::new("oscillation_storm");
    let init_out = knr_cmd()
        .args(["init", temp.path().to_str().unwrap()])
        .output()
        .expect("init");
    assert!(init_out.status.success());

    let entry = temp.path().join("src/main.knr");
    let mut proc = RunningProcess::spawn({
        let mut cmd = knr_cmd();
        cmd.args([
            "dev",
            temp.path().to_str().unwrap(),
            "--poll",
            "20",
            "--debounce",
            "30",
            "--no-clear",
        ]);
        cmd
    });

    assert!(proc.wait_for_stdout("Ready. Watching for changes", Duration::from_secs(3)));

    // Rapidly oscillate between syntax errors and valid code 6 times
    for i in 1..=6 {
        // Corrupt
        let _ = fs::write(&entry, "INVALID SYNTAX // !?");
        thread::sleep(Duration::from_millis(35));

        // Restore
        let valid_src = format!("fn main() -> i64 {{\n    return {};\n}}\n", 2000 + i);
        let _ = fs::write(&entry, valid_src);
        thread::sleep(Duration::from_millis(35));
    }

    // Must settle and print 2006
    assert!(
        proc.wait_for_stdout("\n2006\n", Duration::from_secs(4)),
        "Watcher failed to survive oscillation storm: stdout={}, stderr={}",
        proc.stdout_text(),
        proc.stderr_text()
    );
    assert!(proc.is_alive(), "Watcher died during oscillation storm");
}

#[test]
fn test_stress_empty_file_handling() {
    let temp = TempDir::new("empty_file_stress");
    let init_out = knr_cmd()
        .args(["init", temp.path().to_str().unwrap()])
        .output()
        .expect("init");
    assert!(init_out.status.success());

    let entry = temp.path().join("src/main.knr");

    // Test in --once mode: empty file must fail with exit code != 0
    fs::write(&entry, "").expect("write empty file");
    let out_once = knr_cmd()
        .args(["dev", temp.path().to_str().unwrap(), "--once"])
        .output()
        .expect("knr dev --once");
    assert!(!out_once.status.success(), "Empty source file must fail --once mode");

    // Test in watcher mode: empty file must log error and survive
    let mut proc = RunningProcess::spawn({
        let mut cmd = knr_cmd();
        cmd.args([
            "dev",
            temp.path().to_str().unwrap(),
            "--poll",
            "20",
            "--debounce",
            "30",
            "--no-clear",
        ]);
        cmd
    });

    // Watcher should log error (no functions to execute or syntax error)
    assert!(
        proc.wait_for_stderr("no functions", Duration::from_secs(3))
            || proc.wait_for_stderr("Error", Duration::from_secs(3)),
        "Expected error logged for empty file: stderr={}",
        proc.stderr_text()
    );
    assert!(proc.is_alive(), "Watcher must not crash on empty file");

    // Restore valid file
    fs::write(&entry, "fn main() -> i64 { return 111; }\n").expect("write valid");
    assert!(
        proc.wait_for_stdout("\n111\n", Duration::from_secs(3)),
        "Watcher failed to recover after empty file: stdout={}",
        proc.stdout_text()
    );
    assert!(proc.is_alive());
}

// ============================================================================
// 3. File Creation, Deletion, and Renaming Dynamics While Watching
// ============================================================================

#[test]
fn test_stress_entrypoint_deletion_and_recreation() {
    let temp = TempDir::new("entry_lifecycle");
    let init_out = knr_cmd()
        .args(["init", temp.path().to_str().unwrap()])
        .output()
        .expect("init");
    assert!(init_out.status.success());

    let entry = temp.path().join("src/main.knr");
    let mut proc = RunningProcess::spawn({
        let mut cmd = knr_cmd();
        cmd.args([
            "dev",
            temp.path().to_str().unwrap(),
            "--poll",
            "25",
            "--debounce",
            "40",
            "--no-clear",
        ]);
        cmd
    });

    assert!(proc.wait_for_stdout("Ready. Watching for changes", Duration::from_secs(3)));

    // Delete entrypoint
    fs::remove_file(&entry).expect("delete entry");

    // Watcher should log error about missing entrypoint
    assert!(
        proc.wait_for_stderr("not found", Duration::from_secs(3)),
        "Watcher did not report missing entrypoint: stderr={}",
        proc.stderr_text()
    );
    assert!(proc.is_alive(), "Watcher died on entrypoint deletion!");

    // Re-create entrypoint with new logic
    let recreated = "fn main() -> i64 {\n    return 333;\n}\n";
    fs::write(&entry, recreated).expect("recreate entry");

    // Watcher must detect recreation and execute
    assert!(
        proc.wait_for_stdout("\n333\n", Duration::from_secs(3)),
        "Watcher failed to execute recreated entrypoint: stdout={}",
        proc.stdout_text()
    );
    assert!(proc.is_alive());
}

#[test]
fn test_stress_secondary_file_lifecycle() {
    let temp = TempDir::new("secondary_lifecycle");
    let init_out = knr_cmd()
        .args(["init", temp.path().to_str().unwrap()])
        .output()
        .expect("init");
    assert!(init_out.status.success());

    let mut proc = RunningProcess::spawn({
        let mut cmd = knr_cmd();
        cmd.args([
            "dev",
            temp.path().to_str().unwrap(),
            "--poll",
            "25",
            "--debounce",
            "40",
            "--no-clear",
        ]);
        cmd
    });

    assert!(proc.wait_for_stdout("Ready. Watching for changes", Duration::from_secs(3)));

    // Add a secondary file src/schema.knr
    let helper_file = temp.path().join("src/types.knr");
    fs::write(&helper_file, "// secondary schema\nschema Dummy { 0: a i64 }\n").expect("write secondary");

    assert!(
        proc.wait_for_stdout("File changed: src/types.knr", Duration::from_secs(3)),
        "Watcher missed creation of secondary file: stdout={}",
        proc.stdout_text()
    );

    // Delete the secondary file
    fs::remove_file(&helper_file).expect("remove secondary");

    assert!(
        proc.wait_for_stdout("File changed: src/types.knr", Duration::from_secs(3)),
        "Watcher missed deletion of secondary file: stdout={}",
        proc.stdout_text()
    );

    assert!(proc.is_alive());
}

#[test]
fn test_stress_entrypoint_rename_recovery() {
    let temp = TempDir::new("rename_recovery");
    let init_out = knr_cmd()
        .args(["init", temp.path().to_str().unwrap()])
        .output()
        .expect("init");
    assert!(init_out.status.success());

    let entry = temp.path().join("src/main.knr");
    let backup = temp.path().join("src/main.knr.bak");

    let mut proc = RunningProcess::spawn({
        let mut cmd = knr_cmd();
        cmd.args([
            "dev",
            temp.path().to_str().unwrap(),
            "--poll",
            "25",
            "--debounce",
            "40",
            "--no-clear",
        ]);
        cmd
    });

    assert!(proc.wait_for_stdout("Ready. Watching for changes", Duration::from_secs(3)));

    // Rename main.knr -> main.knr.bak
    fs::rename(&entry, &backup).expect("rename to bak");
    assert!(
        proc.wait_for_stderr("not found", Duration::from_secs(3)),
        "Watcher should report missing entrypoint: stderr={}",
        proc.stderr_text()
    );
    assert!(proc.is_alive());

    // Rename back
    fs::rename(&backup, &entry).expect("rename back");
    assert!(
        proc.wait_for_stdout("File changed: src/main.knr", Duration::from_secs(3)),
        "Watcher missed rename back to main.knr"
    );
    assert!(proc.is_alive());
}

#[test]
fn test_stress_dynamic_manifest_entrypoint_switch() {
    let temp = TempDir::new("dynamic_manifest_switch");
    let init_out = knr_cmd()
        .args(["init", temp.path().to_str().unwrap()])
        .output()
        .expect("init");
    assert!(init_out.status.success());

    // Create an alternate entrypoint
    let alt_entry = temp.path().join("src/alt.knr");
    fs::write(&alt_entry, "fn main() -> i64 {\n    return 77;\n}\n").expect("write alt");

    let mut proc = RunningProcess::spawn({
        let mut cmd = knr_cmd();
        cmd.args([
            "dev",
            temp.path().to_str().unwrap(),
            "--poll",
            "25",
            "--debounce",
            "40",
            "--no-clear",
        ]);
        cmd
    });

    assert!(proc.wait_for_stdout("Ready. Watching for changes", Duration::from_secs(3)));

    // Update knr.toml to point to src/alt.knr
    let manifest_path = temp.path().join("knr.toml");
    let new_manifest = r#"[package]
name = "dynamic_switch_app"
version = "0.1.0"
entry = "src/alt.knr"
edition = "2026"
"#;
    fs::write(&manifest_path, new_manifest).expect("update manifest");

    // Watcher should detect knr.toml change, re-resolve entry to src/alt.knr, and execute it
    assert!(
        proc.wait_for_stdout("\n77\n", Duration::from_secs(4)),
        "Watcher did not switch entrypoint according to updated knr.toml: stdout={}",
        proc.stdout_text()
    );
    assert!(proc.is_alive());
}

#[test]
fn test_stress_watcher_ignores_irrelevant_files() {
    let temp = TempDir::new("ignore_files");
    let init_out = knr_cmd()
        .args(["init", temp.path().to_str().unwrap()])
        .output()
        .expect("init");
    assert!(init_out.status.success());

    let mut proc = RunningProcess::spawn({
        let mut cmd = knr_cmd();
        cmd.args([
            "dev",
            temp.path().to_str().unwrap(),
            "--poll",
            "20",
            "--debounce",
            "30",
            "--no-clear",
        ]);
        cmd
    });

    assert!(proc.wait_for_stdout("Ready. Watching for changes", Duration::from_secs(3)));
    let initial_stdout = proc.stdout_text();

    // Create ignored files: swap, backup, target artifacts, hidden files, notes
    let _ = fs::write(temp.path().join("src/main.knr~"), "backup");
    let _ = fs::write(temp.path().join("src/.main.knr.swp"), "swap");
    let _ = fs::write(temp.path().join("src/temp.tmp"), "temp");
    let _ = fs::write(temp.path().join("notes.txt"), "text file");

    let target_dir = temp.path().join("target");
    let _ = fs::create_dir_all(&target_dir);
    let _ = fs::write(target_dir.join("build.knr"), "ignored in target");

    let git_dir = temp.path().join(".git");
    let _ = fs::create_dir_all(&git_dir);
    let _ = fs::write(git_dir.join("config.knr"), "ignored in .git");

    // Give watcher time to poll
    thread::sleep(Duration::from_millis(250));

    let diff_stdout = &proc.stdout_text()[initial_stdout.len()..];
    assert!(
        !diff_stdout.contains("File changed"),
        "Watcher triggered change event on ignored files: {diff_stdout}"
    );
    assert!(proc.is_alive());
}

// ============================================================================
// 4. `knr new` / `knr init` Collision & Force Handling
// ============================================================================

#[test]
fn test_stress_new_collision_without_force() {
    let temp_parent = TempDir::new("new_collision");
    let target = temp_parent.path().join("existing_non_empty");
    fs::create_dir_all(&target).expect("create dir");

    let sentinel_file = target.join("important_data.txt");
    fs::write(&sentinel_file, "DO NOT OVERWRITE").expect("write sentinel");

    let output = knr_cmd()
        .args(["new", target.to_str().unwrap()])
        .output()
        .expect("knr new");

    assert!(
        !output.status.success(),
        "knr new must FAIL when target directory is non-empty and --force is omitted"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let combined = format!("{stdout}\n{stderr}");

    assert!(
        combined.contains("already exists and is not empty"),
        "Error message must specify non-empty collision: {combined}"
    );

    // Verify sentinel was preserved
    assert!(sentinel_file.is_file());
    assert_eq!(
        fs::read_to_string(&sentinel_file).unwrap(),
        "DO NOT OVERWRITE"
    );

    // Verify knr.toml was NOT created
    assert!(!target.join("knr.toml").exists());
}

#[test]
fn test_stress_new_collision_with_force() {
    let temp_parent = TempDir::new("new_collision_force");
    let target = temp_parent.path().join("existing_non_empty_force");
    fs::create_dir_all(&target).expect("create dir");

    let sentinel_file = target.join("keep_this.txt");
    fs::write(&sentinel_file, "STILL HERE").expect("write sentinel");

    let output = knr_cmd()
        .args(["new", target.to_str().unwrap(), "--force"])
        .output()
        .expect("knr new --force");

    assert!(
        output.status.success(),
        "knr new --force must SUCCEED on non-empty directory: stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );

    assert!(target.join("knr.toml").is_file());
    assert!(target.join("src/main.knr").is_file());
    assert_eq!(fs::read_to_string(&sentinel_file).unwrap(), "STILL HERE");
}

#[test]
fn test_stress_new_on_empty_existing_dir() {
    let temp_parent = TempDir::new("new_empty_dir");
    let target = temp_parent.path().join("empty_folder");
    fs::create_dir_all(&target).expect("create dir");

    let output = knr_cmd()
        .args(["new", target.to_str().unwrap()])
        .output()
        .expect("knr new on empty dir");

    assert!(
        output.status.success(),
        "knr new must succeed when destination directory exists but is completely empty"
    );
    assert!(target.join("knr.toml").is_file());
    assert!(target.join("src/main.knr").is_file());
}

#[test]
fn test_stress_init_collision_without_force() {
    let temp = TempDir::new("init_collision");
    let manifest = temp.path().join("knr.toml");
    fs::write(&manifest, "[package]\nname = \"existing\"\nversion = \"0.1.0\"\n").expect("write manifest");

    let output = knr_cmd()
        .args(["init", temp.path().to_str().unwrap()])
        .output()
        .expect("knr init");

    assert!(
        !output.status.success(),
        "knr init must fail if knr.toml already exists and --force is not specified"
    );

    let err = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(err.contains("already exists"), "Expected already exists error: {err}");
}

#[test]
fn test_stress_init_preserves_existing_sources_without_force() {
    let temp = TempDir::new("init_preserve_sources");
    let src_dir = temp.path().join("src");
    fs::create_dir_all(&src_dir).expect("create src dir");

    let custom_main = src_dir.join("main.knr");
    let custom_code = "// UNIQUE USER CODE — DO NOT OVERWRITE\nfn main() -> i64 {\n    return 4321;\n}\n";
    fs::write(&custom_main, custom_code).expect("write custom main");

    // Run init without knr.toml present
    let output = knr_cmd()
        .args(["init", temp.path().to_str().unwrap()])
        .output()
        .expect("init");

    assert!(output.status.success());
    assert!(temp.path().join("knr.toml").is_file());

    // main.knr MUST NOT be overwritten by starter template when --force is absent
    let read_back = fs::read_to_string(&custom_main).unwrap();
    assert_eq!(
        read_back, custom_code,
        "knr init without --force must NOT clobber pre-existing src/main.knr!"
    );

    // Now run with --force and verify it DOES overwrite
    let output_force = knr_cmd()
        .args(["init", temp.path().to_str().unwrap(), "--force", "--template", "minimal"])
        .output()
        .expect("init with force");
    assert!(output_force.status.success());

    let overwritten = fs::read_to_string(&custom_main).unwrap();
    assert!(
        overwritten.contains("Knr Minimal Starter Application"),
        "knr init --force should overwrite src/main.knr with requested template"
    );
}

#[test]
fn test_stress_init_and_new_invalid_templates() {
    let temp = TempDir::new("invalid_template");

    // Invalid template in knr new
    let out_new = knr_cmd()
        .args(["new", temp.path().join("invalid_app").to_str().unwrap(), "--template", "imaginary_template"])
        .output()
        .expect("knr new invalid template");
    assert!(!out_new.status.success());
    let err_new = String::from_utf8_lossy(&out_new.stderr);
    assert!(
        err_new.contains("Unknown template 'imaginary_template'"),
        "Expected unknown template error: {err_new}"
    );

    // Invalid template in knr init
    let out_init = knr_cmd()
        .args(["init", temp.path().to_str().unwrap(), "--template", "bogus_ui"])
        .output()
        .expect("knr init invalid template");
    assert!(!out_init.status.success());
    let err_init = String::from_utf8_lossy(&out_init.stderr);
    assert!(
        err_init.contains("Unknown template 'bogus_ui'"),
        "Expected unknown template error: {err_init}"
    );
}

// ============================================================================
// 5. `--once` Single-Shot Mode Exit Codes & Diagnostics
// ============================================================================

#[test]
fn test_stress_dev_once_exit_codes() {
    let temp = TempDir::new("dev_once_codes");

    // 1. Success case: valid project
    let valid_proj = temp.path().join("valid");
    knr_cmd().args(["new", valid_proj.to_str().unwrap()]).output().unwrap();

    let out_valid = knr_cmd()
        .args(["dev", valid_proj.to_str().unwrap(), "--once"])
        .output()
        .expect("knr dev --once");
    assert!(out_valid.status.success(), "knr dev --once on valid project must exit 0");
    let stdout_valid = String::from_utf8_lossy(&out_valid.stdout);
    assert!(stdout_valid.contains('0'), "Valid project output should contain 0: {stdout_valid}");

    // 2. Syntax error case: must exit with non-zero status
    let syntax_err_proj = temp.path().join("syntax_err");
    knr_cmd().args(["new", syntax_err_proj.to_str().unwrap()]).output().unwrap();
    fs::write(syntax_err_proj.join("src/main.knr"), "BROKEN SYNTAX // !").unwrap();

    let out_syntax = knr_cmd()
        .args(["dev", syntax_err_proj.to_str().unwrap(), "--once"])
        .output()
        .expect("knr dev --once on syntax error");
    assert!(
        !out_syntax.status.success(),
        "knr dev --once on syntax error MUST exit non-zero!"
    );
    let stderr_syntax = String::from_utf8_lossy(&out_syntax.stderr);
    assert!(
        stderr_syntax.contains("Syntax Error"),
        "Expected Syntax Error in stderr: {stderr_syntax}"
    );

    // 3. Verification error case: must exit with non-zero status
    let verify_err_proj = temp.path().join("verify_err");
    knr_cmd().args(["new", verify_err_proj.to_str().unwrap()]).output().unwrap();
    fs::write(
        verify_err_proj.join("src/main.knr"),
        "fn main() -> i64 {\n    let val = undefined_identifier;\n    return val;\n}\n",
    )
    .unwrap();

    let out_verify = knr_cmd()
        .args(["dev", verify_err_proj.to_str().unwrap(), "--once"])
        .output()
        .expect("knr dev --once on verify error");
    assert!(
        !out_verify.status.success(),
        "knr dev --once on verification error MUST exit non-zero!"
    );
    let stderr_verify = String::from_utf8_lossy(&out_verify.stderr);
    assert!(
        stderr_verify.contains("Verification failed"),
        "Expected Verification failed in stderr: {stderr_verify}"
    );

    // 4. Gas exhaustion case: must exit non-zero
    let out_gas = knr_cmd()
        .args(["dev", valid_proj.to_str().unwrap(), "--once", "--gas", "0"])
        .output()
        .expect("knr dev --once with 0 gas");
    assert!(
        !out_gas.status.success(),
        "knr dev --once with gas 0 MUST exit non-zero!"
    );

    // 5. Non-existent path case: must exit non-zero
    let out_missing = knr_cmd()
        .args(["dev", "/tmp/definitely_nonexistent_knr_path_xyz", "--once"])
        .output()
        .expect("knr dev --once missing");
    assert!(
        !out_missing.status.success(),
        "knr dev --once on nonexistent directory MUST exit non-zero!"
    );
}

#[test]
fn test_stress_run_watch_once_exit_codes() {
    let temp = TempDir::new("run_watch_once_codes");
    let proj = temp.path().join("app");
    knr_cmd().args(["new", proj.to_str().unwrap()]).output().unwrap();

    let entry = proj.join("src/main.knr");

    // Success case
    let out_ok = knr_cmd()
        .args(["run", entry.to_str().unwrap(), "--watch", "--once"])
        .output()
        .unwrap();
    assert!(out_ok.status.success());

    // Error case
    fs::write(&entry, "INVALID PARSE").unwrap();
    let out_fail = knr_cmd()
        .args(["run", entry.to_str().unwrap(), "--watch", "--once"])
        .output()
        .unwrap();
    assert!(
        !out_fail.status.success(),
        "knr run --watch --once on syntax error MUST exit non-zero!"
    );
}

#[test]
fn test_stress_dev_once_trace_output() {
    let temp = TempDir::new("dev_trace_json");
    let app_dir = temp.path().join("app");
    knr_cmd().args(["new", app_dir.to_str().unwrap()]).output().unwrap();

    let out_trace = knr_cmd()
        .args(["dev", app_dir.to_str().unwrap(), "--once", "--trace"])
        .output()
        .expect("knr dev --once --trace");

    assert!(out_trace.status.success(), "knr dev --once --trace should succeed");
    let stdout_str = String::from_utf8_lossy(&out_trace.stdout);

    // Verify valid JSON array of trace records
    let parsed: serde_json::Value = serde_json::from_str(&stdout_str)
        .expect("knr dev --trace stdout should be valid JSON array");
    assert!(parsed.is_array(), "Trace output should be JSON array");
    assert!(!parsed.as_array().unwrap().is_empty(), "Trace records should not be empty");
}

// ============================================================================
// 6. Multi-Project Workspace & Target Path Stress
// ============================================================================

#[test]
fn test_stress_workspace_dev_watching() {
    let temp = TempDir::new("ws_dev_watch");
    let ws_dir = temp.path().join("my_workspace");
    let init_out = knr_cmd()
        .args(["new", ws_dir.to_str().unwrap(), "--workspace"])
        .output()
        .expect("new ws");
    assert!(init_out.status.success());

    let demo_main = ws_dir.join("apps/demo/src/main.knr");
    assert!(demo_main.is_file());

    let mut proc = RunningProcess::spawn({
        let mut cmd = knr_cmd();
        cmd.args([
            "dev",
            ws_dir.to_str().unwrap(),
            "--poll",
            "25",
            "--debounce",
            "50",
            "--no-clear",
        ]);
        cmd
    });

    assert!(
        proc.wait_for_stdout("Ready. Watching for changes", Duration::from_secs(3)),
        "Workspace dev watcher failed to start: stdout={}",
        proc.stdout_text()
    );

    // Modify apps/demo/src/main.knr to return 654
    let new_src = "fn main() -> i64 {\n    return 654;\n}\n";
    fs::write(&demo_main, new_src).expect("write demo main");

    assert!(
        proc.wait_for_stdout("\n654\n", Duration::from_secs(4)),
        "Workspace watcher failed to detect change in workspace member: stdout={}",
        proc.stdout_text()
    );
    assert!(proc.is_alive());
}

#[test]
fn test_stress_dev_path_with_spaces() {
    let temp = TempDir::new("space_path");
    let target = temp.path().join("folder with spaces in name");
    let init_out = knr_cmd()
        .args(["new", target.to_str().unwrap()])
        .output()
        .expect("new space");
    assert!(init_out.status.success());

    let dev_out = knr_cmd()
        .args(["dev", target.to_str().unwrap(), "--once"])
        .output()
        .expect("dev space path");
    assert!(
        dev_out.status.success(),
        "knr dev must handle directory paths containing spaces: stderr={}",
        String::from_utf8_lossy(&dev_out.stderr)
    );
}

#[test]
fn test_stress_dev_direct_file_and_manifest_targets() {
    let temp = TempDir::new("direct_targets");
    let app_dir = temp.path().join("app");
    knr_cmd().args(["new", app_dir.to_str().unwrap()]).output().unwrap();

    // 1. Direct file target: should execute src/main.knr cleanly
    let file_target = app_dir.join("src/main.knr");
    let out_file = knr_cmd()
        .args(["dev", file_target.to_str().unwrap(), "--once"])
        .output()
        .expect("dev direct file");
    assert!(
        out_file.status.success(),
        "knr dev targeting file directly must succeed: stderr={}",
        String::from_utf8_lossy(&out_file.stderr)
    );

    // 2. Direct manifest target: documented in knr dev --help:
    //    "[PATH]    Path to project directory, knr.toml manifest, or source file (default: .)"
    // Remediation:
    // If canonical.is_file() and canonical is knr.toml, dev resolves the watch directory as parent
    // and entrypoint from the manifest.
    let manifest_target = app_dir.join("knr.toml");
    let out_manifest = knr_cmd()
        .args(["dev", manifest_target.to_str().unwrap(), "--once"])
        .output()
        .expect("dev direct manifest");

    let err_str = String::from_utf8_lossy(&out_manifest.stderr);
    assert!(
        out_manifest.status.success(),
        "knr dev path/to/knr.toml must resolve manifest and execute entrypoint successfully: stderr={err_str}"
    );
}
