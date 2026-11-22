//! Challenger 2: Milestone 4 Full .knr Toolchain Subcommand Stress Tests
//!
//! Empirical adversarial test suite covering:
//! 1. Roundtrip: .knr -> .knrbin -> .ssa -> knr check (and schema inlining failure on exec_graph.knr)
//! 2. Invalid syntax error reporting across check, build, asm, run (exit code 1, no panic, diagnostic lines)
//! 3. Graph generation via `knr check <file.knr> --graph` vs `knr check <file.ssa> --graph`
//! 4. WASM compilation via `knr build <file.knr> --target wasm -o <out>`
//! 5. CLI boundary edge cases (missing -o, missing file, empty file)

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

struct TempFile {
    path: PathBuf,
}

impl TempFile {
    fn new(suffix: &str) -> Self {
        static COUNTER: AtomicUsize = AtomicUsize::new(200);
        let id = COUNTER.fetch_add(1, Ordering::SeqCst);
        let pid = std::process::id();
        let file_name = format!("knr_chal2_toolchain_{pid}_{id}_{suffix}");
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
    cmd.env_remove("DISPLAY");
    cmd
}

fn repo_root() -> PathBuf {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest_dir.parent().unwrap().parent().unwrap().to_path_buf()
}

// =============================================================================
// 1. Roundtrip Consistency: .knr -> .knrbin -> .ssa -> check
// =============================================================================

#[test]
fn test_roundtrip_minimal_knr() {
    let root = repo_root();
    let src_knr = root.join("tests/fixtures/knr_valid/01_minimal.knr");
    let bin_temp = TempFile::new("min.knrbin");
    let ssa_temp = TempFile::new("min.ssa");

    // 1. knr asm 01_minimal.knr -o min.knrbin
    let asm_out = knr_cmd()
        .args(["asm", src_knr.to_str().unwrap(), "-o", bin_temp.path().to_str().unwrap()])
        .output()
        .expect("knr asm");
    assert!(asm_out.status.success(), "asm minimal.knr must succeed");
    assert!(bin_temp.path().exists(), "binary output must exist");

    // 2. knr disasm min.knrbin -o min.ssa
    let disasm_out = knr_cmd()
        .args(["disasm", bin_temp.path().to_str().unwrap(), "-o", ssa_temp.path().to_str().unwrap()])
        .output()
        .expect("knr disasm");
    assert!(disasm_out.status.success(), "disasm minimal.knrbin must succeed");
    assert!(ssa_temp.path().exists(), "disassembled ssa must exist");

    // 3. knr check min.ssa
    let check_out = knr_cmd()
        .args(["check", ssa_temp.path().to_str().unwrap()])
        .output()
        .expect("knr check");
    assert!(
        check_out.status.success(),
        "check roundtripped min.ssa must succeed, stderr: {}",
        String::from_utf8_lossy(&check_out.stderr)
    );
}

#[test]
fn test_roundtrip_arithmetic_knr() {
    let root = repo_root();
    let src_knr = root.join("tests/fixtures/knr_valid/02_arithmetic.knr");
    let bin_temp = TempFile::new("arith.knrbin");
    let ssa_temp = TempFile::new("arith.ssa");

    let asm_out = knr_cmd()
        .args(["asm", src_knr.to_str().unwrap(), "-o", bin_temp.path().to_str().unwrap()])
        .output()
        .expect("knr asm");
    assert!(asm_out.status.success());

    let disasm_out = knr_cmd()
        .args(["disasm", bin_temp.path().to_str().unwrap(), "-o", ssa_temp.path().to_str().unwrap()])
        .output()
        .expect("knr disasm");
    assert!(disasm_out.status.success());

    let check_out = knr_cmd()
        .args(["check", ssa_temp.path().to_str().unwrap()])
        .output()
        .expect("knr check");
    assert!(check_out.status.success());
}

#[test]
fn test_roundtrip_branching_knr() {
    let root = repo_root();
    let src_knr = root.join("tests/fixtures/knr_valid/03_branching.knr");
    let bin_temp = TempFile::new("branch.knrbin");
    let ssa_temp = TempFile::new("branch.ssa");

    let asm_out = knr_cmd()
        .args(["asm", src_knr.to_str().unwrap(), "-o", bin_temp.path().to_str().unwrap()])
        .output()
        .expect("knr asm");
    assert!(asm_out.status.success());

    let disasm_out = knr_cmd()
        .args(["disasm", bin_temp.path().to_str().unwrap(), "-o", ssa_temp.path().to_str().unwrap()])
        .output()
        .expect("knr disasm");
    assert!(disasm_out.status.success());

    let check_out = knr_cmd()
        .args(["check", ssa_temp.path().to_str().unwrap()])
        .output()
        .expect("knr check");
    assert!(check_out.status.success());
}

#[test]
fn test_roundtrip_inter_fn_knr() {
    let root = repo_root();
    let src_knr = root.join("tests/fixtures/knr_valid/08_inter_fn_calls.knr");
    let bin_temp = TempFile::new("interfn.knrbin");
    let ssa_temp = TempFile::new("interfn.ssa");

    let asm_out = knr_cmd()
        .args(["asm", src_knr.to_str().unwrap(), "-o", bin_temp.path().to_str().unwrap()])
        .output()
        .expect("knr asm");
    assert!(asm_out.status.success());

    let disasm_out = knr_cmd()
        .args(["disasm", bin_temp.path().to_str().unwrap(), "-o", ssa_temp.path().to_str().unwrap()])
        .output()
        .expect("knr disasm");
    assert!(disasm_out.status.success());

    let check_out = knr_cmd()
        .args(["check", ssa_temp.path().to_str().unwrap()])
        .output()
        .expect("knr check");
    assert!(check_out.status.success());
}

/// Adversarial empirical test: roundtripping a .knr file that contains structs (exec_graph.knr).
/// Disassembly prints schema declarations inline in the generated .ssa file.
/// Because knr-core strictly forbids inline schemas in code files (only .ssa.schema may define schemas),
/// knr check on the disassembled .ssa reveals an architectural friction point: InlineSchemaInCodeFile.
#[test]
fn test_roundtrip_exec_graph_schema_inlining_adversarial() {
    let root = repo_root();
    let src_knr = root.join("tests/fixtures/legacy_site/components/exec_graph.knr");
    let bin_temp = TempFile::new("exec_graph.knrbin");
    let ssa_temp = TempFile::new("exec_graph.ssa");

    let asm_out = knr_cmd()
        .args(["asm", src_knr.to_str().unwrap(), "-o", bin_temp.path().to_str().unwrap()])
        .output()
        .expect("knr asm exec_graph.knr");
    assert!(asm_out.status.success(), "asm on exec_graph.knr should succeed: {}", String::from_utf8_lossy(&asm_out.stderr));

    let disasm_out = knr_cmd()
        .args(["disasm", bin_temp.path().to_str().unwrap(), "-o", ssa_temp.path().to_str().unwrap()])
        .output()
        .expect("knr disasm");
    assert!(disasm_out.status.success(), "disasm should succeed: {}", String::from_utf8_lossy(&disasm_out.stderr));

    let ssa_content = fs::read_to_string(ssa_temp.path()).expect("read ssa");
    assert!(
        ssa_content.contains("import \"") && ssa_content.contains(".ssa.schema\""),
        "Disassembled SSA includes schema import: {ssa_content}"
    );
    assert!(
        !ssa_content.contains("schema Leaf"),
        "Disassembled SSA code file must NOT contain inline schema definitions"
    );

    // Verify accompanying schema file
    let ssa_filename = ssa_temp.path().file_name().unwrap().to_string_lossy();
    let stem = ssa_filename.strip_suffix(".ssa").unwrap_or(&ssa_filename);
    let schema_path = ssa_temp.path().parent().unwrap().join(format!("{stem}.ssa.schema"));
    assert!(schema_path.exists(), "Accompanying schema file must exist: {:?}", schema_path);
    let schema_content = fs::read_to_string(&schema_path).expect("read schema file");
    assert!(schema_content.contains("schema Leaf"), "Accompanying schema file must contain schema Leaf");

    // knr check must succeed
    let check_out = knr_cmd()
        .args(["check", ssa_temp.path().to_str().unwrap()])
        .output()
        .expect("knr check");
    assert!(
        check_out.status.success(),
        "knr check on roundtripped .ssa must succeed, stderr: {}",
        String::from_utf8_lossy(&check_out.stderr)
    );

    // Re-assembly from disassembled .ssa to bytecode must succeed
    let bin_temp2 = TempFile::new("exec_graph_rebuilt.knrbin");
    let reasm_out = knr_cmd()
        .args(["asm", ssa_temp.path().to_str().unwrap(), "-o", bin_temp2.path().to_str().unwrap()])
        .output()
        .expect("knr asm roundtrip");
    assert!(
        reasm_out.status.success(),
        "knr asm on roundtripped .ssa must succeed, stderr: {}",
        String::from_utf8_lossy(&reasm_out.stderr)
    );

    let _ = fs::remove_file(&schema_path);
}

// =============================================================================
// 2. Invalid .knr Syntax Across Toolchain (check, build, asm, run)
// =============================================================================

#[test]
fn test_invalid_knr_syntax_across_all_subcommands() {
    let root = repo_root();
    let invalid_knr = root.join("tests/fixtures/knr_invalid/err01_syntax_unclosed_brace.knr");
    let temp_out = TempFile::new("dummy.out");

    let commands: Vec<(&str, Vec<&str>)> = vec![
        ("check", vec!["check", invalid_knr.to_str().unwrap()]),
        ("run", vec!["run", invalid_knr.to_str().unwrap()]),
        ("asm", vec!["asm", invalid_knr.to_str().unwrap(), "-o", temp_out.path().to_str().unwrap()]),
        ("build_native", vec!["build", invalid_knr.to_str().unwrap(), "--target", "native", "-o", temp_out.path().to_str().unwrap()]),
        ("build_wasm", vec!["build", invalid_knr.to_str().unwrap(), "--target", "wasm", "-o", temp_out.path().to_str().unwrap()]),
    ];

    for (name, args) in commands {
        let out = knr_cmd()
            .args(&args)
            .output()
            .unwrap_or_else(|e| panic!("failed to run knr {name}: {e}"));

        assert!(
            !out.status.success(),
            "Subcommand {name} must fail on invalid syntax, got success"
        );
        assert_eq!(
            out.status.code(),
            Some(1),
            "Subcommand {name} must exit with code 1"
        );

        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            !stderr.contains("panicked at"),
            "Subcommand {name} must not panic on invalid syntax: {stderr}"
        );
        assert!(
            stderr.contains("Parse error") || stderr.contains("InvalidSyntax") || stderr.contains("unexpected end"),
            "Subcommand {name} stderr must report parse error: {stderr}"
        );
    }
}

#[test]
fn test_invalid_token_across_all_subcommands() {
    let root = repo_root();
    let invalid_knr = root.join("tests/fixtures/knr_invalid/err02_syntax_invalid_token.knr");
    let temp_out = TempFile::new("dummy.out");

    let commands: Vec<(&str, Vec<&str>)> = vec![
        ("check", vec!["check", invalid_knr.to_str().unwrap()]),
        ("run", vec!["run", invalid_knr.to_str().unwrap()]),
        ("asm", vec!["asm", invalid_knr.to_str().unwrap(), "-o", temp_out.path().to_str().unwrap()]),
        ("build_wasm", vec!["build", invalid_knr.to_str().unwrap(), "--target", "wasm", "-o", temp_out.path().to_str().unwrap()]),
    ];

    for (name, args) in commands {
        let out = knr_cmd()
            .args(&args)
            .output()
            .unwrap_or_else(|e| panic!("failed to run knr {name}: {e}"));

        assert!(!out.status.success(), "Subcommand {name} must fail on invalid token");
        assert_eq!(out.status.code(), Some(1));
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(!stderr.contains("panicked at"));
    }
}

#[test]
fn test_check_json_invalid_knr_diagnostics() {
    let root = repo_root();
    let invalid_knr = root.join("tests/fixtures/knr_invalid/err01_syntax_unclosed_brace.knr");

    let out = knr_cmd()
        .args(["check", invalid_knr.to_str().unwrap(), "--json"])
        .output()
        .expect("knr check --json");

    assert!(!out.status.success());
    assert_eq!(out.status.code(), Some(1));

    let stdout = String::from_utf8_lossy(&out.stdout);
    let diags: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("JSON output invalid: {stdout}, err: {e}"));
    assert!(diags.is_array());
    let arr = diags.as_array().unwrap();
    assert!(!arr.is_empty(), "Must produce diagnostic array");
    let error_kind = arr[0]["error_kind"].as_str().unwrap();
    assert!(
        error_kind == "MissingClosingBrace" || error_kind == "InvalidSyntax",
        "Expected syntax error kind, got {error_kind}"
    );
    let err_line = arr[0]["line"].as_u64().unwrap();
    assert!(
        err_line == 4 || err_line == 5,
        "Line number must be 4 or 5 (EOF where brace is missing), got {err_line}"
    );
    assert!(arr[0]["fix_hint"].is_string(), "Must provide remediation hint");
}

#[test]
fn test_check_json_invalid_token_line_and_col() {
    let root = repo_root();
    let invalid_knr = root.join("tests/fixtures/knr_invalid/err02_syntax_invalid_token.knr");

    let out = knr_cmd()
        .args(["check", invalid_knr.to_str().unwrap(), "--json"])
        .output()
        .expect("knr check --json");

    assert!(!out.status.success());
    assert_eq!(out.status.code(), Some(1));

    let stdout = String::from_utf8_lossy(&out.stdout);
    let diags: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("JSON output invalid: {stdout}, err: {e}"));
    let arr = diags.as_array().unwrap();
    assert!(!arr.is_empty());
    assert_eq!(arr[0]["error_kind"], "InvalidSyntax");
    assert_eq!(arr[0]["line"], 3, "Invalid token '@' is on line 3");
    let has_col_11 = arr[0]["column"] == 11
        || arr[0]["instruction"]
            .as_str()
            .is_some_and(|m| m.contains("column 11") || m.contains("col 11"))
        || arr[0]["fix_hint"]
            .as_str()
            .is_some_and(|m| m.contains("column 11") || m.contains("col 11"));
    assert!(
        has_col_11,
        "Invalid token '@' must report column 11, got: {}",
        arr[0]
    );
}

// =============================================================================
// 3. Graph Flag Empirical Verification: site/components/exec_graph.knr vs ssa
// =============================================================================

/// Stress-tests `knr check <file> --graph`.
/// Both .ssa and .knr files with `--graph` must emit topology, schemas, and call graphs.
#[test]
fn test_check_graph_ssa_vs_knr_behavior() {
    let root = repo_root();
    let ssa_file = root.join("tests/fixtures/legacy_site/components/exec_graph.ssa");
    let knr_file = root.join("tests/fixtures/legacy_site/components/exec_graph.knr");

    // 1. .ssa file with --graph produces rich topology
    let ssa_out = knr_cmd()
        .args(["check", ssa_file.to_str().unwrap(), "--graph"])
        .output()
        .expect("knr check ssa --graph");
    assert!(ssa_out.status.success());
    let ssa_stdout = String::from_utf8_lossy(&ssa_out.stdout);
    assert!(
        ssa_stdout.contains("[TOPOLOGY & IMPORTS]"),
        "SSA --graph must emit topology"
    );
    assert!(
        ssa_stdout.contains("[SCHEMAS]"),
        "SSA --graph must emit schemas"
    );
    assert!(
        ssa_stdout.contains("[CALL GRAPH & SIGNATURES]"),
        "SSA --graph must emit call graph"
    );

    // 2. .knr file with --graph produces matching rich topology
    let knr_out = knr_cmd()
        .args(["check", knr_file.to_str().unwrap(), "--graph"])
        .output()
        .expect("knr check knr --graph");
    assert!(knr_out.status.success(), "check --graph on .knr must succeed");
    let knr_stdout = String::from_utf8_lossy(&knr_out.stdout);
    assert!(
        knr_stdout.contains("[TOPOLOGY & IMPORTS]"),
        "KNR --graph must emit topology"
    );
    assert!(
        knr_stdout.contains("[SCHEMAS]"),
        "KNR --graph must emit schemas"
    );
    assert!(
        knr_stdout.contains("[CALL GRAPH & SIGNATURES]"),
        "KNR --graph must emit call graph"
    );
}

// =============================================================================
// 4. WASM Compilation: knr build <file.knr> --target wasm
// =============================================================================

#[test]
fn test_build_wasm_minimal_knr() {
    let root = repo_root();
    let src_knr = root.join("tests/fixtures/knr_valid/01_minimal.knr");
    let wasm_out = TempFile::new("min.wasm");

    let out = knr_cmd()
        .args(["build", src_knr.to_str().unwrap(), "--target", "wasm", "-o", wasm_out.path().to_str().unwrap()])
        .output()
        .expect("knr build wasm");

    assert!(
        out.status.success(),
        "knr build --target wasm on 01_minimal.knr must succeed, stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let bytes = fs::read(wasm_out.path()).expect("read wasm output");
    assert!(bytes.len() >= 8, "WASM binary must be at least 8 bytes");
    // WebAssembly magic number "\0asm" = [0x00, 0x61, 0x73, 0x6d]
    assert_eq!(&bytes[0..4], &[0x00, 0x61, 0x73, 0x6d], "Must start with WASM magic");
    // WebAssembly version 1 = [0x01, 0x00, 0x00, 0x00]
    assert_eq!(&bytes[4..8], &[0x01, 0x00, 0x00, 0x00], "Must start with WASM v1");
}

#[test]
fn test_build_wasm_arithmetic_knr() {
    let root = repo_root();
    let src_knr = root.join("tests/fixtures/knr_valid/02_arithmetic.knr");
    let wasm_out = TempFile::new("arith.wasm");

    let out = knr_cmd()
        .args(["build", src_knr.to_str().unwrap(), "--target", "wasm", "-o", wasm_out.path().to_str().unwrap()])
        .output()
        .expect("knr build wasm");

    assert!(out.status.success());
    let bytes = fs::read(wasm_out.path()).expect("read wasm");
    assert_eq!(&bytes[0..4], &[0x00, 0x61, 0x73, 0x6d]);
}

#[test]
fn test_build_wasm_branching_knr() {
    let root = repo_root();
    let src_knr = root.join("tests/fixtures/knr_valid/03_branching.knr");
    let wasm_out = TempFile::new("branch.wasm");

    let out = knr_cmd()
        .args(["build", src_knr.to_str().unwrap(), "--target", "wasm", "-o", wasm_out.path().to_str().unwrap()])
        .output()
        .expect("knr build wasm");

    assert!(out.status.success());
    let bytes = fs::read(wasm_out.path()).expect("read wasm");
    assert_eq!(&bytes[0..4], &[0x00, 0x61, 0x73, 0x6d]);
}

/// Empirical test: WASM compilation on a .knr file with structs (exec_graph.knr)
/// succeeds cleanly with the embedded in-WASM linear-memory struct runtime.
#[test]
fn test_build_wasm_struct_supported_clean_output() {
    let root = repo_root();
    let src_knr = root.join("tests/fixtures/legacy_site/components/exec_graph.knr");
    let wasm_out = TempFile::new("exec_graph.wasm");

    let out = knr_cmd()
        .args(["build", src_knr.to_str().unwrap(), "--target", "wasm", "-o", wasm_out.path().to_str().unwrap()])
        .output()
        .expect("knr build wasm on exec_graph");

    assert!(
        out.status.success(),
        "WASM compilation on exec_graph.knr must succeed with struct support, stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let bytes = fs::read(wasm_out.path()).expect("read exec_graph.wasm");
    assert!(bytes.len() > 8, "WASM binary must not be empty");
    assert_eq!(&bytes[0..4], &[0x00, 0x61, 0x73, 0x6d], "Valid WASM magic header");
}

// =============================================================================
// 5. CLI Parameter Boundaries & Edge Cases
// =============================================================================

#[test]
fn test_build_missing_output_flag() {
    let root = repo_root();
    let src_knr = root.join("tests/fixtures/knr_valid/01_minimal.knr");

    let out = knr_cmd()
        .args(["build", src_knr.to_str().unwrap(), "--target", "wasm"])
        .output()
        .expect("knr build without -o");

    assert!(!out.status.success());
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("Missing output path '-o'"),
        "Must report missing -o: {stderr}"
    );
}

#[test]
fn test_nonexistent_file_handling() {
    let fake_path = "/tmp/knr_nonexistent_file_xyz_12345.knr";

    let commands: Vec<(&str, Vec<&str>)> = vec![
        ("check", vec!["check", fake_path]),
        ("run", vec!["run", fake_path]),
        ("asm", vec!["asm", fake_path, "-o", "/tmp/dummy.knrbin"]),
        ("build", vec!["build", fake_path, "--target", "wasm", "-o", "/tmp/dummy.wasm"]),
        ("disasm", vec!["disasm", fake_path]),
    ];

    for (name, args) in commands {
        let out = knr_cmd()
            .args(&args)
            .output()
            .unwrap_or_else(|e| panic!("run knr {name}: {e}"));

        assert!(!out.status.success(), "Subcommand {name} must fail on nonexistent file");
        assert_eq!(out.status.code(), Some(1));
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(!stderr.contains("panicked at"));
    }
}
