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
        let dir = std::env::temp_dir().join(format!("knr_cli_adv_{prefix}_{pid}_{id}"));
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
    manifest_dir
        .parent()
        .expect("knr-cli parent")
        .parent()
        .expect("crates parent")
        .to_path_buf()
}

// ============================================================================
// Group A: Combinations of flags with --json
// ============================================================================

#[test]
fn test_adv_json_with_symbol_schema_found() {
    let site = repo_root().join("site");
    let output = knr_bin()
        .args(["explore", site.to_str().unwrap(), "--json", "--symbol", "Card"])
        .output()
        .expect("run explore --json --symbol Card");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON expected");
    assert_eq!(v["symbol"], "Card");
    let schemas = v["schemas"].as_array().expect("schemas array");
    assert_eq!(schemas.len(), 1);
    assert_eq!(schemas[0]["name"], "Card");
    assert!(schemas[0]["usages"].as_array().is_some());
}

#[test]
fn test_adv_json_with_symbol_function_found() {
    let site = repo_root().join("site");
    let output = knr_bin()
        .args(["explore", site.to_str().unwrap(), "--json", "-s", "render_page"])
        .output()
        .expect("run explore --json -s render_page");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON expected");
    assert_eq!(v["symbol"], "render_page");
    let funcs = v["functions"].as_array().expect("functions array");
    assert_eq!(funcs.len(), 1);
    assert_eq!(funcs[0]["name"], "render_page");
    assert!(funcs[0]["calls"].as_array().is_some());
}

#[test]
fn test_adv_json_with_symbol_not_found() {
    let site = repo_root().join("site");
    let output = knr_bin()
        .args(["explore", site.to_str().unwrap(), "--json", "--symbol=NonExistentSymbol999"])
        .output()
        .expect("run explore --json --symbol=NonExistentSymbol999");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON expected");
    assert_eq!(v["symbol"], "NonExistentSymbol999");
    let schemas = v["schemas"].as_array().expect("schemas array");
    let funcs = v["functions"].as_array().expect("functions array");
    assert!(schemas.is_empty(), "schemas must be empty for missing symbol");
    assert!(funcs.is_empty(), "functions must be empty for missing symbol");
}

#[test]
fn test_adv_json_with_callers() {
    let site = repo_root().join("site");
    let output = knr_bin()
        .args(["explore", site.to_str().unwrap(), "--json", "--callers", "build_nav"])
        .output()
        .expect("run explore --json --callers build_nav");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON expected");
    assert_eq!(v["target"], "build_nav");
    let callers = v["callers"].as_array().expect("callers array");
    assert!(!callers.is_empty(), "build_nav must have callers");
    assert!(callers.iter().any(|c| c["caller"] == "render_page"));
}

#[test]
fn test_adv_json_with_callers_nonexistent() {
    let site = repo_root().join("site");
    let output = knr_bin()
        .args(["explore", site.to_str().unwrap(), "--json", "--callers=unknown_fn_xyz"])
        .output()
        .expect("run explore --json --callers=unknown_fn_xyz");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON expected");
    assert_eq!(v["target"], "unknown_fn_xyz");
    let callers = v["callers"].as_array().expect("callers array");
    assert!(callers.is_empty(), "unknown fn must have 0 callers");
}

#[test]
fn test_adv_json_with_callees() {
    let site = repo_root().join("site");
    let output = knr_bin()
        .args(["explore", site.to_str().unwrap(), "--json", "--callees", "render_page"])
        .output()
        .expect("run explore --json --callees render_page");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON expected");
    assert_eq!(v["target"], "render_page");
    let callees = v["callees"].as_array().expect("callees array");
    assert!(!callees.is_empty(), "render_page must have callees");
    assert!(callees.iter().any(|c| c["callee"] == "build_nav"));
    assert!(v.get("syscalls").is_some());
}

#[test]
fn test_adv_json_with_effects_case_insensitive() {
    let site = repo_root().join("site");

    // Lowercase
    let out_lower = knr_bin()
        .args(["explore", site.to_str().unwrap(), "--json", "--effects", "ui"])
        .output()
        .expect("run explore --json --effects ui");
    assert!(out_lower.status.success());
    let v_lower: serde_json::Value = serde_json::from_str(&String::from_utf8_lossy(&out_lower.stdout)).unwrap();
    assert_eq!(v_lower["effect"], "ui");
    let funcs_lower = v_lower["functions"].as_array().unwrap();
    assert!(!funcs_lower.is_empty());

    // Uppercase
    let out_upper = knr_bin()
        .args(["explore", site.to_str().unwrap(), "--json", "--effects=UI"])
        .output()
        .expect("run explore --json --effects=UI");
    assert!(out_upper.status.success());
    let v_upper: serde_json::Value = serde_json::from_str(&String::from_utf8_lossy(&out_upper.stdout)).unwrap();
    assert_eq!(v_upper["effect"], "UI");
    let funcs_upper = v_upper["functions"].as_array().unwrap();
    assert_eq!(funcs_lower.len(), funcs_upper.len());
}

#[test]
fn test_adv_json_with_effects_nonexistent() {
    let site = repo_root().join("site");
    let output = knr_bin()
        .args(["explore", site.to_str().unwrap(), "--json", "--effects", "nonexistent_effect"])
        .output()
        .expect("run explore --json --effects nonexistent_effect");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON expected");
    assert_eq!(v["effect"], "nonexistent_effect");
    let funcs = v["functions"].as_array().expect("functions array");
    assert!(funcs.is_empty(), "no functions should match bogus effect");
}

#[test]
fn test_adv_json_with_orphans() {
    let http_schema = repo_root().join("site").join("schemas").join("http.ssa.schema");
    let output = knr_bin()
        .args(["explore", http_schema.to_str().unwrap(), "--json", "--orphans"])
        .output()
        .expect("run explore --json --orphans");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON expected");
    assert!(v.get("orphans").is_some());
    let orphans = &v["orphans"];
    let unused_schemas = orphans["unused_schemas"].as_array().expect("unused_schemas array");
    assert!(unused_schemas.iter().any(|s| s == "HttpRequest"));
    assert!(v.get("files").is_none(), "orphans-only JSON should not emit full graph");
}

// ============================================================================
// Group B: Conflicting flags and argument validation
// ============================================================================

#[test]
fn test_adv_conflicting_json_and_format_mermaid() {
    let site = repo_root().join("site");
    let output = knr_bin()
        .args(["explore", site.to_str().unwrap(), "--json", "--format", "mermaid"])
        .output()
        .expect("run conflicting --json and --format mermaid");
    assert!(!output.status.success(), "Conflicting flags must exit non-zero");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Cannot specify conflicting options '--json' and '--format'"),
        "Expected error message not found: {stderr}"
    );
}

#[test]
fn test_adv_conflicting_json_and_format_ai() {
    let site = repo_root().join("site");
    let output = knr_bin()
        .args(["explore", site.to_str().unwrap(), "--format=ai", "--json"])
        .output()
        .expect("run conflicting --format=ai and --json");
    assert!(!output.status.success(), "Conflicting flags must exit non-zero");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Cannot specify conflicting options '--json' and '--format'"),
        "Expected error message not found: {stderr}"
    );
}

#[test]
fn test_adv_compatible_json_and_format_json() {
    let site = repo_root().join("site");
    let output = knr_bin()
        .args(["explore", site.to_str().unwrap(), "--format=json", "--json"])
        .output()
        .expect("run compatible --format=json and --json");
    assert!(output.status.success(), "Compatible json options should succeed");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let _: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON expected");
}

#[test]
fn test_adv_missing_flag_values() {
    let site = repo_root().join("site");
    let site_str = site.to_str().unwrap();

    // Missing value for --symbol
    let out = knr_bin().args(["explore", site_str, "--symbol"]).output().unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("Missing value for '--symbol'"));

    // Missing value for --callers
    let out = knr_bin().args(["explore", site_str, "--callers"]).output().unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("Missing value for '--callers'"));

    // Missing value for --callees
    let out = knr_bin().args(["explore", site_str, "--callees"]).output().unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("Missing value for '--callees'"));

    // Missing value for --effects
    let out = knr_bin().args(["explore", site_str, "--effects"]).output().unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("Missing value for '--effects'"));

    // Missing value for --format
    let out = knr_bin().args(["explore", site_str, "--format"]).output().unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("Missing value for '--format'"));
}

// ============================================================================
// Group C: Path and file error handling
// ============================================================================

#[test]
fn test_adv_nonexistent_path() {
    let bogus = "/tmp/does_not_exist_knr_adv_path_12345.ssa";
    let output = knr_bin()
        .args(["explore", bogus])
        .output()
        .expect("run explore with non-existent path");
    assert!(!output.status.success(), "Non-existent path must exit non-zero");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Check that file") || stderr.contains("FileNotFound") || stderr.contains("not found"),
        "Expected file not found error on stderr: {stderr}"
    );
}

#[test]
fn test_adv_invalid_extension() {
    let temp = TempDir::new("invalid_ext");
    let txt_file = temp.path().join("source.txt");
    fs::write(&txt_file, "fn main() -> i64 {\nbb0:\n    %0 = const_i64 0\n    ret %0\n}\n").unwrap();

    let output = knr_bin()
        .args(["explore", txt_file.to_str().unwrap()])
        .output()
        .expect("run explore with invalid extension");
    assert!(!output.status.success(), "Invalid extension must exit non-zero");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("invalid extension") && stderr.contains(".ssa"),
        "Expected invalid extension message on stderr: {stderr}"
    );
}

#[test]
fn test_adv_directory_without_ssa_files() {
    let temp = TempDir::new("no_ssa_dir");
    let readme = temp.path().join("README.md");
    fs::write(&readme, "# No SSA files here\n").unwrap();

    // AI format
    let output = knr_bin()
        .args(["explore", temp.path().to_str().unwrap()])
        .output()
        .expect("run explore on empty dir");
    assert!(output.status.success(), "Directory with 0 ssa files should succeed gracefully");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("[TOPOLOGY & IMPORTS]"));
    assert!(stdout.contains("[ORPHANS]"));

    // JSON format
    let out_json = knr_bin()
        .args(["explore", temp.path().to_str().unwrap(), "--json"])
        .output()
        .expect("run explore on empty dir --json");
    assert!(out_json.status.success());
    let v: serde_json::Value = serde_json::from_str(&String::from_utf8_lossy(&out_json.stdout)).unwrap();
    assert!(v["files"].as_array().unwrap().is_empty());
    assert!(v["schemas"].as_array().unwrap().is_empty());
    assert!(v["functions"].as_array().unwrap().is_empty());
}

#[test]
fn test_adv_directory_with_syntax_error_ssa() {
    let temp = TempDir::new("dir_syntax_error");
    let bad_file = temp.path().join("broken.ssa");
    fs::write(&bad_file, "fn broken( {\nbb0:\n    ret %0\n}\n").unwrap();

    let output = knr_bin()
        .args(["explore", temp.path().to_str().unwrap()])
        .output()
        .expect("run explore on dir with broken ssa");
    assert!(!output.status.success(), "Broken SSA file in dir must cause non-zero exit");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stderr.is_empty(), "Error diagnostics must be emitted to stderr");
}

// ============================================================================
// Group D: knr check <invalid> --graph suppresses graph output
// ============================================================================

#[test]
fn test_adv_check_verify_error_suppresses_graph_ai() {
    let temp = TempDir::new("check_verify_error");
    let bad_ssa = temp.path().join("bad_verify.ssa");
    // Register %99 used without definition -> verification fails
    fs::write(&bad_ssa, "fn bad() -> i64 {\nbb0:\n    ret %99\n}\n").unwrap();

    let output = knr_bin()
        .args(["check", bad_ssa.to_str().unwrap(), "--graph"])
        .output()
        .expect("run check bad_verify.ssa --graph");
    assert!(!output.status.success(), "Check with verify error must exit non-zero");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stdout.contains("[TOPOLOGY & IMPORTS]"),
        "Graph must not be emitted to stdout on verification error"
    );
    assert!(
        !stderr.contains("[TOPOLOGY & IMPORTS]"),
        "Graph must not be emitted to stderr on verification error"
    );
    assert!(stderr.contains("UndefinedRegister") || stderr.contains("bad"), "Diagnostic expected: {stderr}");
}

#[test]
fn test_adv_check_verify_error_suppresses_graph_json() {
    let temp = TempDir::new("check_verify_error_json");
    let bad_ssa = temp.path().join("bad_verify.ssa");
    fs::write(&bad_ssa, "fn bad() -> i64 {\nbb0:\n    ret %99\n}\n").unwrap();

    let output = knr_bin()
        .args(["check", bad_ssa.to_str().unwrap(), "--json", "--graph"])
        .output()
        .expect("run check bad_verify.ssa --json --graph");
    assert!(!output.status.success(), "Check with verify error must exit non-zero");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("Diagnostics JSON array expected");
    assert!(v.is_array(), "Output must be JSON array of diagnostics");
    assert!(!v.as_array().unwrap().is_empty(), "Must have diagnostics");
    assert!(!stdout.contains("\"files\":"), "ProjectGraph must not be emitted in JSON output");
}

#[test]
fn test_adv_check_parse_error_suppresses_graph_ai() {
    let temp = TempDir::new("check_parse_error");
    let bad_ssa = temp.path().join("bad_syntax.ssa");
    fs::write(&bad_ssa, "not_a_valid_ssa_file\n").unwrap();

    let output = knr_bin()
        .args(["check", bad_ssa.to_str().unwrap(), "--graph"])
        .output()
        .expect("run check bad_syntax.ssa --graph");
    assert!(!output.status.success(), "Check with parse error must exit non-zero");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stdout.contains("[TOPOLOGY & IMPORTS]"), "Graph must not be emitted on parse error");
    assert!(!stderr.contains("[TOPOLOGY & IMPORTS]"), "Graph must not be emitted on parse error");
}

#[test]
fn test_adv_check_parse_error_suppresses_graph_json() {
    let temp = TempDir::new("check_parse_error_json");
    let bad_ssa = temp.path().join("bad_syntax.ssa");
    fs::write(&bad_ssa, "not_a_valid_ssa_file\n").unwrap();

    let output = knr_bin()
        .args(["check", bad_ssa.to_str().unwrap(), "--json", "--graph"])
        .output()
        .expect("run check bad_syntax.ssa --json --graph");
    assert!(!output.status.success(), "Check with parse error must exit non-zero");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("Diagnostics JSON array expected");
    assert!(v.is_array(), "Output must be JSON array of diagnostics");
    assert!(!stdout.contains("\"files\":"), "ProjectGraph must not be emitted on parse error");
}

#[test]
fn test_adv_check_nonexistent_file_suppresses_graph() {
    let bogus = "/tmp/does_not_exist_knr_check_file.ssa";
    let output = knr_bin()
        .args(["check", bogus, "--graph"])
        .output()
        .expect("run check nonexistent file --graph");
    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!stdout.contains("[TOPOLOGY & IMPORTS]"));
}
