use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(prefix: &str) -> Self {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let id = COUNTER.fetch_add(1, Ordering::SeqCst);
        let pid = std::process::id();
        let dir = std::env::temp_dir().join(format!("knr_cli_test_{prefix}_{pid}_{id}"));
        fs::create_dir_all(&dir).expect("failed to create temp dir");
        Self { path: dir }
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

fn knr_bin() -> Command {
    let bin = env!("CARGO_BIN_EXE_knr");
    Command::new(bin)
}

fn repo_root() -> PathBuf {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest_dir.parent().unwrap().parent().unwrap().to_path_buf()
}

// ============================================================================
// 1. CLI Help & Flag Verification
// ============================================================================

#[test]
fn test_cli_main_help_lists_explore() {
    let output = knr_bin()
        .arg("--help")
        .output()
        .expect("failed to execute knr --help");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("explore"), "Main help must list 'explore' subcommand: {stdout}");
}

#[test]
fn test_cli_explore_help() {
    let output = knr_bin()
        .args(["explore", "--help"])
        .output()
        .expect("failed to execute knr explore --help");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("knr explore <PATH> [OPTIONS]"));
    assert!(stdout.contains("--format"));
    assert!(stdout.contains("--json"));
    assert!(stdout.contains("--symbol"));
    assert!(stdout.contains("--callers"));
    assert!(stdout.contains("--callees"));
    assert!(stdout.contains("--effects"));
    assert!(stdout.contains("--orphans"));
}

#[test]
fn test_cli_check_help_lists_graph() {
    let output = knr_bin()
        .args(["check", "--help"])
        .output()
        .expect("failed to execute knr check --help");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("--graph"), "Check help must document '--graph': {stdout}");
}

// ============================================================================
// 2. knr explore Subcommand Invocations
// ============================================================================

#[test]
fn test_cli_explore_directory_ai() {
    let site = repo_root().join("site");
    let output = knr_bin()
        .args(["explore", site.to_str().unwrap()])
        .output()
        .expect("failed to execute knr explore site/");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("[TOPOLOGY & IMPORTS]"));
    assert!(stdout.contains("[SCHEMAS]"));
    assert!(stdout.contains("[CALL GRAPH & SIGNATURES]"));
    assert!(stdout.contains("[ORPHANS]"));
    assert!(stdout.contains("HttpRequest"));
    assert!(stdout.contains("render_page"));
}

#[test]
fn test_cli_explore_single_file_ai() {
    let site_file = repo_root().join("site").join("site.ssa");
    let output = knr_bin()
        .args(["explore", site_file.to_str().unwrap()])
        .output()
        .expect("failed to execute knr explore site/site.ssa");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("site.ssa"));
    assert!(stdout.contains("page.ssa"));
}

#[test]
fn test_cli_explore_json() {
    let site = repo_root().join("site");
    let output = knr_bin()
        .args(["explore", site.to_str().unwrap(), "--json"])
        .output()
        .expect("failed to execute knr explore site/ --json");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("Must be valid JSON");
    assert!(parsed.get("files").is_some());
    assert!(parsed.get("schemas").is_some());
    assert!(parsed.get("functions").is_some());
    assert!(parsed.get("orphans").is_some());
}

#[test]
fn test_cli_explore_mermaid() {
    let site = repo_root().join("site");
    let output = knr_bin()
        .args(["explore", site.to_str().unwrap(), "--format", "mermaid"])
        .output()
        .expect("failed to execute knr explore site/ --format mermaid");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.starts_with("flowchart TD\n"));
    assert!(stdout.contains("subgraph"));
    assert!(stdout.contains("[\""));
}

#[test]
fn test_cli_explore_symbol_filters() {
    let site = repo_root().join("site");

    // Short flag -s
    let out_short = knr_bin()
        .args(["explore", site.to_str().unwrap(), "-s", "Card"])
        .output()
        .expect("failed to run knr explore -s Card");
    assert!(out_short.status.success());
    let stdout_short = String::from_utf8_lossy(&out_short.stdout);
    assert!(stdout_short.contains("SCHEMA Card"));

    // Long flag --symbol=Card
    let out_long = knr_bin()
        .args(["explore", site.to_str().unwrap(), "--symbol=Card"])
        .output()
        .expect("failed to run knr explore --symbol=Card");
    assert!(out_long.status.success());
    let stdout_long = String::from_utf8_lossy(&out_long.stdout);
    assert!(stdout_long.contains("SCHEMA Card"));
}

#[test]
fn test_cli_explore_callers_and_callees() {
    let site = repo_root().join("site");

    let out_callers = knr_bin()
        .args(["explore", site.to_str().unwrap(), "--callers", "build_nav"])
        .output()
        .expect("failed to run knr explore --callers build_nav");
    assert!(out_callers.status.success());
    let stdout_callers = String::from_utf8_lossy(&out_callers.stdout);
    assert!(stdout_callers.contains("CALLERS OF build_nav:"));
    assert!(stdout_callers.contains("render_page"));

    let out_callees = knr_bin()
        .args(["explore", site.to_str().unwrap(), "--callees", "render_page"])
        .output()
        .expect("failed to run knr explore --callees render_page");
    assert!(out_callees.status.success());
    let stdout_callees = String::from_utf8_lossy(&out_callees.stdout);
    assert!(stdout_callees.contains("CALLEES OF render_page:"));
    assert!(stdout_callees.contains("build_nav"));
}

#[test]
fn test_cli_explore_effects() {
    let site = repo_root().join("site");
    let output = knr_bin()
        .args(["explore", site.to_str().unwrap(), "--effects", "ui"])
        .output()
        .expect("failed to run knr explore --effects ui");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("render_page"));
}

#[test]
fn test_cli_explore_orphans() {
    let http_schema = repo_root().join("site").join("schemas").join("http.ssa.schema");
    let output = knr_bin()
        .args(["explore", http_schema.to_str().unwrap(), "--orphans"])
        .output()
        .expect("failed to run knr explore --orphans");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("[ORPHANS]"));
    assert!(stdout.contains("HttpRequest"));
    assert!(!stdout.contains("[TOPOLOGY & IMPORTS]"));
}

#[test]
fn test_cli_explore_argument_errors() {
    // Missing path
    let out_missing = knr_bin().args(["explore"]).output().unwrap();
    assert!(!out_missing.status.success());

    // Conflicting --format and --json
    let site = repo_root().join("site");
    let out_conflict = knr_bin()
        .args(["explore", site.to_str().unwrap(), "--format", "mermaid", "--json"])
        .output()
        .unwrap();
    assert!(!out_conflict.status.success());

    // Invalid format
    let out_invalid_fmt = knr_bin()
        .args(["explore", site.to_str().unwrap(), "--format", "pdf"])
        .output()
        .unwrap();
    assert!(!out_invalid_fmt.status.success());
}

// ============================================================================
// 3. knr check --graph Invocations
// ============================================================================

#[test]
fn test_cli_check_graph_valid_ai() {
    let site_file = repo_root().join("site").join("site.ssa");
    let output = knr_bin()
        .args(["check", site_file.to_str().unwrap(), "--graph"])
        .output()
        .expect("failed to run knr check site.ssa --graph");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("[TOPOLOGY & IMPORTS]"));
    assert!(stdout.contains("site.ssa"));
}

#[test]
fn test_cli_check_graph_valid_json() {
    let site_file = repo_root().join("site").join("site.ssa");
    let output = knr_bin()
        .args(["check", site_file.to_str().unwrap(), "--json", "--graph"])
        .output()
        .expect("failed to run knr check site.ssa --json --graph");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("Must be valid JSON");
    assert!(parsed.get("files").is_some());
    assert!(parsed.get("schemas").is_some());
}

#[test]
fn test_cli_check_graph_on_invalid_file_does_not_emit_graph() {
    let temp = TempDir::new("invalid_graph");
    let bad_file = temp.path().join("bad.ssa");
    fs::write(&bad_file, "fn broken() -> i64 {\nbb0:\n    ret %999\n}\n").unwrap();

    let output = knr_bin()
        .args(["check", bad_file.to_str().unwrap(), "--graph"])
        .output()
        .expect("run check --graph on bad file");
    assert!(!output.status.success(), "Check on invalid file must exit non-zero");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!stdout.contains("[TOPOLOGY & IMPORTS]"), "Graph must not be emitted on check failure");
}

#[test]
fn test_cli_check_graph_json_on_invalid_file_emits_diagnostics_not_graph() {
    let temp = TempDir::new("invalid_graph_json");
    let bad_file = temp.path().join("bad.ssa");
    fs::write(&bad_file, "fn broken() -> i64 {\nbb0:\n    ret %999\n}\n").unwrap();

    let output = knr_bin()
        .args(["check", bad_file.to_str().unwrap(), "--json", "--graph"])
        .output()
        .expect("run check --json --graph on bad file");
    assert!(!output.status.success(), "Check on invalid file must exit non-zero");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("Diagnostics must be JSON array");
    assert!(parsed.is_array(), "Must be diagnostics array");
    assert!(!stdout.contains("\"files\":"), "ProjectGraph must not be emitted on check failure");
}
