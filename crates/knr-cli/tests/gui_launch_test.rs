use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

struct TempFile {
    path: PathBuf,
}

impl TempFile {
    fn new(suffix: &str) -> Self {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let id = COUNTER.fetch_add(1, Ordering::SeqCst);
        let pid = std::process::id();
        let file_name = format!("knr_gui_test_{pid}_{id}_{suffix}");
        let path = std::env::temp_dir().join(file_name);
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn knr_cmd() -> Command {
    let bin = env!("CARGO_BIN_EXE_knr");
    let mut cmd = Command::new(bin);
    // Ensure headless environment for deterministic assertions across CI
    cmd.env_remove("DISPLAY");
    cmd
}

fn repo_root() -> PathBuf {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest_dir.parent().unwrap().parent().unwrap().to_path_buf()
}

#[test]
fn test_gui_flag_help_documentation() {
    // 1. Check knr run --help
    let run_help = knr_cmd()
        .args(["run", "--help"])
        .output()
        .expect("execute knr run --help");
    assert!(run_help.status.success());
    let run_stdout = String::from_utf8_lossy(&run_help.stdout);
    assert!(
        run_stdout.contains("--gui"),
        "knr run --help must document --gui"
    );
    assert!(
        run_stdout.contains("-g"),
        "knr run --help must document -g"
    );
    assert!(
        run_stdout.contains("Launch native desktop GUI window with execution graph visualization"),
        "knr run --help must document GUI description"
    );

    // 2. Check main knr --help
    let main_help = knr_cmd()
        .arg("--help")
        .output()
        .expect("execute knr --help");
    assert!(main_help.status.success());
    let main_stdout = String::from_utf8_lossy(&main_help.stdout);
    assert!(
        main_stdout.contains("--gui"),
        "knr --help must document --gui"
    );
}

#[test]
fn test_gui_launch_knr_file_headless() {
    let temp = TempFile::new("simple.knr");
    fs::write(
        temp.path(),
        r#"
fn main() -> i64 {
    let x = 10;
    let y = 20;
    if x < y {
        return x + y;
    } else {
        return x - y;
    }
}
"#,
    )
    .expect("write temp.knr");

    let out = knr_cmd()
        .args(["run", temp.path().to_str().unwrap(), "--gui"])
        .output()
        .expect("execute knr run --gui");

    assert!(
        out.status.success(),
        "knr run <file.knr> --gui must exit 0, got {:?}, stderr: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("[GUI] Headless environment detected: rendered off-screen canvas snapshot"),
        "Must log headless snapshot notice, got: {stdout}"
    );
}

#[test]
fn test_gui_launch_ssa_file_headless() {
    let ssa_path = repo_root().join("tests/fixtures/valid/04_branching.ssa");
    assert!(ssa_path.exists(), "fixture must exist: {:?}", ssa_path);

    let out = knr_cmd()
        .args(["run", ssa_path.to_str().unwrap(), "--gui"])
        .output()
        .expect("execute knr run <file.ssa> --gui");

    assert!(
        out.status.success(),
        "knr run <file.ssa> --gui must exit 0, got {:?}, stderr: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("[GUI] Headless environment detected: rendered off-screen canvas snapshot"),
        "Must log headless notice, got: {stdout}"
    );
}

#[test]
fn test_gui_launch_reference_exec_graph_component() {
    let exec_graph_path = repo_root().join("site/components/exec_graph.knr");
    assert!(exec_graph_path.exists(), "exec_graph.knr must exist");

    let out = knr_cmd()
        .args(["run", exec_graph_path.to_str().unwrap(), "--gui"])
        .output()
        .expect("execute knr run exec_graph.knr --gui");

    assert!(
        out.status.success(),
        "knr run exec_graph.knr --gui must exit 0, got {:?}, stderr: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("[GUI] Headless environment detected: rendered off-screen canvas snapshot"),
        "Must log headless notice, got: {stdout}"
    );
}

#[test]
fn test_gui_launch_short_flag() {
    let exec_graph_path = repo_root().join("site/components/exec_graph.knr");
    let out = knr_cmd()
        .args(["run", exec_graph_path.to_str().unwrap(), "-g"])
        .output()
        .expect("execute knr run exec_graph.knr -g");

    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("[GUI] Headless environment detected: rendered off-screen canvas snapshot"));
}

#[test]
fn test_gui_launch_with_trace_and_gas_limit() {
    let exec_graph_path = repo_root().join("site/components/exec_graph.knr");

    let out = knr_cmd()
        .args([
            "run",
            exec_graph_path.to_str().unwrap(),
            "--gui",
            "--trace",
            "--gas",
            "1000000",
        ])
        .output()
        .expect("execute knr run exec_graph.knr --gui --trace --gas 1000000");

    assert!(
        out.status.success(),
        "knr run with --gui --trace must exit 0, got {:?}, stderr: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("[GUI] Headless environment detected: rendered off-screen canvas snapshot"),
        "Must contain headless notice"
    );

    // Verify valid JSON trace output is present
    let json_start = stdout.find("\n[").expect("JSON trace array must start with [ after newline");
    let json_text = stdout[json_start + 1..].trim();
    let trace_value: serde_json::Value =
        serde_json::from_str(json_text).expect("trace output must be valid JSON");
    assert!(trace_value.is_array());
    assert!(!trace_value.as_array().unwrap().is_empty());
}

#[test]
fn test_gui_launch_unlimited_gas() {
    let ssa_path = repo_root().join("tests/fixtures/valid/02_arithmetic.ssa");
    let out = knr_cmd()
        .args([
            "run",
            ssa_path.to_str().unwrap(),
            "--gui",
            "--unlimited-gas",
        ])
        .output()
        .expect("execute knr run --gui --unlimited-gas");

    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("[GUI] Headless environment detected: rendered off-screen canvas snapshot"));
}

#[test]
fn test_knr_check_subcommand() {
    let exec_graph_path = repo_root().join("site/components/exec_graph.knr");

    // 1. Textual check
    let out = knr_cmd()
        .args(["check", exec_graph_path.to_str().unwrap()])
        .output()
        .expect("execute knr check");
    assert!(out.status.success());

    // 2. JSON check
    let out_json = knr_cmd()
        .args(["check", exec_graph_path.to_str().unwrap(), "--json"])
        .output()
        .expect("execute knr check --json");
    assert!(out_json.status.success());
    let stdout_json = String::from_utf8_lossy(&out_json.stdout);
    assert_eq!(stdout_json.trim(), "[]");
}

#[test]
fn test_knr_build_subcommand() {
    let temp_knr = TempFile::new("calc.knr");
    fs::write(
        temp_knr.path(),
        r#"
fn calculate(a: i64, b: i64) -> i64 {
    let sum = a + b;
    return sum * 2;
}
"#,
    )
    .expect("write calc.knr");

    // Build WASM
    let wasm_out = TempFile::new("calc.wasm");
    let out_wasm = knr_cmd()
        .args([
            "build",
            temp_knr.path().to_str().unwrap(),
            "--target",
            "wasm",
            "-o",
            wasm_out.path().to_str().unwrap(),
        ])
        .output()
        .expect("knr build wasm");

    assert!(
        out_wasm.status.success(),
        "WASM build must succeed, stderr: {}",
        String::from_utf8_lossy(&out_wasm.stderr)
    );
    assert!(wasm_out.path().exists());
    assert!(fs::metadata(wasm_out.path()).unwrap().len() > 0);

    // Build Native
    let native_out = TempFile::new("calc_bin");
    let out_native = knr_cmd()
        .args([
            "build",
            temp_knr.path().to_str().unwrap(),
            "--target",
            "native",
            "-o",
            native_out.path().to_str().unwrap(),
        ])
        .output()
        .expect("knr build native");

    assert!(
        out_native.status.success(),
        "Native build must succeed, stderr: {}",
        String::from_utf8_lossy(&out_native.stderr)
    );
    assert!(native_out.path().exists());
    assert!(fs::metadata(native_out.path()).unwrap().len() > 0);
}

#[test]
fn test_knr_asm_and_disasm_subcommands() {
    let exec_graph_path = repo_root().join("site/components/exec_graph.knr");
    let knrbin_out = TempFile::new("exec_graph.knrbin");

    // 1. knr asm <file.knr> -o <out.knrbin>
    let asm_res = knr_cmd()
        .args([
            "asm",
            exec_graph_path.to_str().unwrap(),
            "-o",
            knrbin_out.path().to_str().unwrap(),
        ])
        .output()
        .expect("knr asm");

    assert!(
        asm_res.status.success(),
        "knr asm must succeed, stderr: {}",
        String::from_utf8_lossy(&asm_res.stderr)
    );
    assert!(knrbin_out.path().exists());
    assert!(fs::metadata(knrbin_out.path()).unwrap().len() > 0);

    // 2. knr disasm <out.knrbin>
    let disasm_res = knr_cmd()
        .args(["disasm", knrbin_out.path().to_str().unwrap()])
        .output()
        .expect("knr disasm");

    assert!(disasm_res.status.success());
    let disasm_txt = String::from_utf8_lossy(&disasm_res.stdout);
    assert!(disasm_txt.contains("schema Leaf"));
    assert!(disasm_txt.contains("fn build_exec_graph"));

    // 3. knr disasm <file.knr> directly
    let disasm_direct = knr_cmd()
        .args(["disasm", exec_graph_path.to_str().unwrap()])
        .output()
        .expect("knr disasm .knr");

    assert!(disasm_direct.status.success());
    let direct_txt = String::from_utf8_lossy(&disasm_direct.stdout);
    assert!(direct_txt.contains("schema Leaf"));
    assert!(direct_txt.contains("schema Card"));
    assert!(direct_txt.contains("fn build_exec_graph"));
}
