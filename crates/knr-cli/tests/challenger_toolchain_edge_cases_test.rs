//! Challenger 2 (teamwork_challenger_m4_it2_2): Milestone 4 Iteration 2 Adversarial Edge Cases
//!
//! Empirical test suite covering:
//! 1. Adversarial Schemas & Disassembly:
//!    - Nested non-existent target directories (`mkdir -p` behavior)
//!    - Target paths without `.ssa` extension (`.txt`, raw names)
//!    - Schemas with 0 fields (`schema Empty {}`)
//!    - Modules with multiple schemas
//!    - Accurate `{stem}.ssa.schema` naming and matching `import` directives
//! 2. Adversarial Graph Exploration (`knr check <file> --graph`):
//!    - Multi-level topology and transitive imports
//!    - Circular imports detection and exit code 1
//!    - Missing import error reporting and exit code 1
//!    - Syntax error reporting in imported files with exit code 1
//! 3. Multi-command parse error fidelity:
//!    - `asm`, `build --target wasm`, `build --target native`, `run`, `disasm`
//!    - Clean error messages with exit code 1 on malformed `.knr` files
//!    - Nonexistent file error reporting with exit code 1

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(suffix: &str) -> Self {
        static COUNTER: AtomicUsize = AtomicUsize::new(500);
        let id = COUNTER.fetch_add(1, Ordering::SeqCst);
        let pid = std::process::id();
        let dir_name = format!("knr_chal2_adv_{pid}_{id}_{suffix}");
        let path = std::env::temp_dir().join(dir_name);
        fs::create_dir_all(&path).expect("create temp dir");
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
    let mut cmd = Command::new(bin);
    cmd.env_remove("DISPLAY");
    cmd
}

fn repo_root() -> PathBuf {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest_dir.parent().unwrap().parent().unwrap().to_path_buf()
}

// =============================================================================
// 1. Adversarial Schemas & Disassembly
// =============================================================================

#[test]
fn test_disasm_nested_nonexistent_directory() {
    let temp = TempDir::new("nested_dir");
    let root = repo_root();
    let src_knr = root.join("site/components/exec_graph.knr");
    let bin_path = temp.path().join("exec.knrbin");
    let nested_ssa_path = temp
        .path()
        .join("deeply")
        .join("nested")
        .join("target")
        .join("output.ssa");

    // 1. Assemble to binary
    let asm_out = knr_cmd()
        .args(["asm", src_knr.to_str().unwrap(), "-o", bin_path.to_str().unwrap()])
        .current_dir(&root)
        .output()
        .expect("knr asm");
    assert!(asm_out.status.success(), "knr asm must succeed");

    // 2. Disassemble into deeply nested non-existent directory
    let disasm_out = knr_cmd()
        .args(["disasm", bin_path.to_str().unwrap(), "-o", nested_ssa_path.to_str().unwrap()])
        .current_dir(&root)
        .output()
        .expect("knr disasm");
    assert!(
        disasm_out.status.success(),
        "knr disasm into nested nonexistent directory must succeed, stderr: {}",
        String::from_utf8_lossy(&disasm_out.stderr)
    );
    assert!(nested_ssa_path.exists(), "nested .ssa output must exist");

    // 3. Verify {stem}.ssa.schema is in the same nested directory
    let schema_path = temp
        .path()
        .join("deeply")
        .join("nested")
        .join("target")
        .join("output.ssa.schema");
    assert!(
        schema_path.exists(),
        "Accompanying schema file must exist in nested directory: {:?}",
        schema_path
    );

    // 4. Verify import statement in .ssa matches schema filename
    let ssa_content = fs::read_to_string(&nested_ssa_path).expect("read ssa");
    assert!(
        ssa_content.starts_with("import \"output.ssa.schema\"\n"),
        "SSA file must import output.ssa.schema directly"
    );

    // 5. Verify roundtrip check on the nested .ssa file
    let check_out = knr_cmd()
        .args(["check", nested_ssa_path.to_str().unwrap()])
        .current_dir(&root)
        .output()
        .expect("knr check");
    assert!(
        check_out.status.success(),
        "knr check on nested disassembled file must succeed: {}",
        String::from_utf8_lossy(&check_out.stderr)
    );
}

#[test]
fn test_disasm_non_ssa_extensions() {
    let temp = TempDir::new("no_ext");
    let root = repo_root();
    let src_knr = root.join("site/components/exec_graph.knr");
    let bin_path = temp.path().join("exec.knrbin");

    // Assemble
    let asm_out = knr_cmd()
        .args(["asm", src_knr.to_str().unwrap(), "-o", bin_path.to_str().unwrap()])
        .current_dir(&root)
        .output()
        .expect("knr asm");
    assert!(asm_out.status.success());

    // Disasm to .txt extension
    let txt_path = temp.path().join("result.txt");
    let disasm_txt = knr_cmd()
        .args(["disasm", bin_path.to_str().unwrap(), "-o", txt_path.to_str().unwrap()])
        .current_dir(&root)
        .output()
        .expect("knr disasm txt");
    assert!(disasm_txt.status.success());
    let txt_schema = temp.path().join("result.ssa.schema");
    assert!(txt_schema.exists(), "Schema for result.txt must be result.ssa.schema");
    let txt_content = fs::read_to_string(&txt_path).unwrap();
    assert!(txt_content.starts_with("import \"result.ssa.schema\"\n"));

    // Disasm to raw name without any extension
    let raw_path = temp.path().join("binary_dump");
    let disasm_raw = knr_cmd()
        .args(["disasm", bin_path.to_str().unwrap(), "-o", raw_path.to_str().unwrap()])
        .current_dir(&root)
        .output()
        .expect("knr disasm raw");
    assert!(disasm_raw.status.success());
    let raw_schema = temp.path().join("binary_dump.ssa.schema");
    assert!(raw_schema.exists(), "Schema for binary_dump must be binary_dump.ssa.schema");
    let raw_content = fs::read_to_string(&raw_path).unwrap();
    assert!(raw_content.starts_with("import \"binary_dump.ssa.schema\"\n"));
}

#[test]
fn test_disasm_zero_field_schema_and_multiple_schemas() {
    let temp = TempDir::new("multi_schema");
    let knr_src = r#"schema Empty {
}

schema Alpha {
    0: val i64,
}

schema Beta {
    0: name str,
    1: count i64,
}

fn test_fn(x: i64) -> i64 {
    return x;
}
"#;
    let knr_file = temp.path().join("multi.knr");
    fs::write(&knr_file, knr_src).expect("write multi.knr");
    let bin_file = temp.path().join("multi.knrbin");
    let ssa_file = temp.path().join("multi.ssa");
    let schema_file = temp.path().join("multi.ssa.schema");

    // 1. Assemble
    let asm_out = knr_cmd()
        .args(["asm", knr_file.to_str().unwrap(), "-o", bin_file.to_str().unwrap()])
        .output()
        .expect("knr asm");
    assert!(asm_out.status.success(), "asm must succeed for 0-field and multi schemas");

    // 2. Disassemble
    let disasm_out = knr_cmd()
        .args(["disasm", bin_file.to_str().unwrap(), "-o", ssa_file.to_str().unwrap()])
        .output()
        .expect("knr disasm");
    assert!(disasm_out.status.success(), "disasm must succeed");

    assert!(schema_file.exists(), "multi.ssa.schema must exist");
    let schema_text = fs::read_to_string(&schema_file).expect("read schema");
    assert!(schema_text.contains("schema Empty {"), "Must contain Empty schema");
    assert!(schema_text.contains("schema Alpha {"), "Must contain Alpha schema");
    assert!(schema_text.contains("schema Beta {"), "Must contain Beta schema");

    let ssa_text = fs::read_to_string(&ssa_file).expect("read ssa");
    assert!(ssa_text.starts_with("import \"multi.ssa.schema\"\n"));
    assert!(!ssa_text.contains("schema Empty"), "SSA must not inline schemas");

    // 3. Roundtrip check
    let check_out = knr_cmd()
        .args(["check", ssa_file.to_str().unwrap()])
        .output()
        .expect("knr check");
    assert!(
        check_out.status.success(),
        "knr check on disassembled multi-schema ssa must succeed: {}",
        String::from_utf8_lossy(&check_out.stderr)
    );

    // 4. Re-assemble and compare bytecode
    let bin_reasm = temp.path().join("multi_reasm.knrbin");
    let reasm_out = knr_cmd()
        .args(["asm", ssa_file.to_str().unwrap(), "-o", bin_reasm.to_str().unwrap()])
        .output()
        .expect("knr reasm");
    assert!(reasm_out.status.success());
    let b1 = fs::read(&bin_file).unwrap();
    let b2 = fs::read(&bin_reasm).unwrap();
    assert_eq!(b1, b2, "Bytecode before and after disassembly roundtrip must be bitwise identical");
}

// =============================================================================
// 2. Adversarial .knr Graph Exploration
// =============================================================================

#[test]
fn test_check_graph_circular_import_rejected() {
    let temp = TempDir::new("circ_import");
    let file_a = temp.path().join("node_a.knr");
    let file_b = temp.path().join("node_b.knr");

    fs::write(&file_a, "import \"node_b.knr\"\nfn a() -> i64 { return 1; }\n").unwrap();
    fs::write(&file_b, "import \"node_a.knr\"\nfn b() -> i64 { return 2; }\n").unwrap();

    let check_out = knr_cmd()
        .args(["check", file_a.to_str().unwrap(), "--graph"])
        .current_dir(temp.path())
        .output()
        .expect("knr check");

    assert_eq!(check_out.status.code(), Some(1), "Circular import must exit code 1");
    let stderr = String::from_utf8_lossy(&check_out.stderr);
    assert!(
        stderr.contains("Circular import") || stderr.contains("cycle"),
        "Must report circular import diagnostic: {stderr}"
    );
    assert!(!stderr.contains("panicked"), "Must not panic on cycle");
}

#[test]
fn test_check_graph_missing_import_rejected() {
    let temp = TempDir::new("missing_import");
    let main_knr = temp.path().join("main.knr");
    fs::write(&main_knr, "import \"nonexistent_dependency.ssa.schema\"\nfn main() -> i64 { return 0; }\n").unwrap();

    let check_out = knr_cmd()
        .args(["check", main_knr.to_str().unwrap(), "--graph"])
        .current_dir(temp.path())
        .output()
        .expect("knr check");

    assert_eq!(check_out.status.code(), Some(1), "Missing import must exit code 1");
    let stderr = String::from_utf8_lossy(&check_out.stderr);
    assert!(
        stderr.contains("File not found") || stderr.contains("FileNotFound") || stderr.contains("No such file"),
        "Must report file not found diagnostic: {stderr}"
    );
    assert!(!stderr.contains("panicked"), "Must not panic on missing import");
}

#[test]
fn test_check_graph_valid_relative_schema_import() {
    let temp = TempDir::new("valid_graph");
    let schema_file = temp.path().join("vdom.ssa.schema");
    let code_file = temp.path().join("app.knr");

    fs::write(&schema_file, "schema Header {\n    0: title str,\n}\n").unwrap();
    fs::write(&code_file, "import \"vdom.ssa.schema\"\n\nfn make_header() -> Header {\n    let h = Header { title: \"Welcome\" };\n    return h;\n}\n").unwrap();

    let check_out = knr_cmd()
        .args(["check", "app.knr", "--graph"])
        .current_dir(temp.path())
        .output()
        .expect("knr check");

    assert!(
        check_out.status.success(),
        "knr check --graph with relative schema import must succeed, stderr: {}",
        String::from_utf8_lossy(&check_out.stderr)
    );
    let stdout = String::from_utf8_lossy(&check_out.stdout);
    assert!(stdout.contains("[TOPOLOGY & IMPORTS]"));
    assert!(stdout.contains("FILE vdom.ssa.schema (Schema)"));
    assert!(stdout.contains("FILE app.knr (Code)"));
    assert!(stdout.contains("[SCHEMAS]"));
    assert!(stdout.contains("SCHEMA Header (vdom.ssa.schema:1)"));
}

// =============================================================================
// 3. Multi-Command Parse Error Fidelity
// =============================================================================

#[test]
fn test_multicommand_parse_error_fidelity_unclosed_brace() {
    let root = repo_root();
    let invalid_knr = root.join("tests/fixtures/knr_invalid/err01_syntax_unclosed_brace.knr");
    let temp = TempDir::new("err_out");
    let dummy_out = temp.path().join("out.bin");

    let subcommands: Vec<(&str, Vec<&str>)> = vec![
        ("check", vec!["check", invalid_knr.to_str().unwrap()]),
        ("asm", vec!["asm", invalid_knr.to_str().unwrap(), "-o", dummy_out.to_str().unwrap()]),
        ("build_wasm", vec!["build", invalid_knr.to_str().unwrap(), "--target", "wasm", "-o", dummy_out.to_str().unwrap()]),
        ("build_native", vec!["build", invalid_knr.to_str().unwrap(), "--target", "native", "-o", dummy_out.to_str().unwrap()]),
        ("run", vec!["run", invalid_knr.to_str().unwrap()]),
        ("disasm", vec!["disasm", invalid_knr.to_str().unwrap(), "-o", dummy_out.to_str().unwrap()]),
    ];

    for (name, args) in subcommands {
        let out = knr_cmd()
            .args(&args)
            .current_dir(&root)
            .output()
            .unwrap_or_else(|e| panic!("run {name}: {e}"));

        assert_eq!(
            out.status.code(),
            Some(1),
            "Subcommand {name} must exit with code 1"
        );
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(!stderr.contains("panicked"), "Subcommand {name} must not panic");
        assert!(
            stderr.contains("line 5") && stderr.contains("missing closing brace"),
            "Subcommand {name} must report exact parse error on line 5: {stderr}"
        );
    }
}

#[test]
fn test_multicommand_parse_error_fidelity_invalid_token() {
    let root = repo_root();
    let invalid_knr = root.join("tests/fixtures/knr_invalid/err02_syntax_invalid_token.knr");
    let temp = TempDir::new("err_out2");
    let dummy_out = temp.path().join("out.bin");

    let subcommands: Vec<(&str, Vec<&str>)> = vec![
        ("check", vec!["check", invalid_knr.to_str().unwrap()]),
        ("asm", vec!["asm", invalid_knr.to_str().unwrap(), "-o", dummy_out.to_str().unwrap()]),
        ("build_wasm", vec!["build", invalid_knr.to_str().unwrap(), "--target", "wasm", "-o", dummy_out.to_str().unwrap()]),
        ("build_native", vec!["build", invalid_knr.to_str().unwrap(), "--target", "native", "-o", dummy_out.to_str().unwrap()]),
        ("run", vec!["run", invalid_knr.to_str().unwrap()]),
        ("disasm", vec!["disasm", invalid_knr.to_str().unwrap(), "-o", dummy_out.to_str().unwrap()]),
    ];

    for (name, args) in subcommands {
        let out = knr_cmd()
            .args(&args)
            .current_dir(&root)
            .output()
            .unwrap_or_else(|e| panic!("run {name}: {e}"));

        assert_eq!(
            out.status.code(),
            Some(1),
            "Subcommand {name} must exit with code 1"
        );
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(!stderr.contains("panicked"), "Subcommand {name} must not panic");
        assert!(
            stderr.contains("line 3") && (stderr.contains("col 9") || stderr.contains("@")),
            "Subcommand {name} must report token error on line 3, col 9: {stderr}"
        );
    }
}

#[test]
fn test_multicommand_nonexistent_file_handling() {
    let temp = TempDir::new("nonexistent");
    let nonexistent = temp.path().join("ghost_program.knr");
    let dummy_out = temp.path().join("out.bin");

    let subcommands: Vec<(&str, Vec<&str>)> = vec![
        ("check", vec!["check", nonexistent.to_str().unwrap()]),
        ("asm", vec!["asm", nonexistent.to_str().unwrap(), "-o", dummy_out.to_str().unwrap()]),
        ("build_wasm", vec!["build", nonexistent.to_str().unwrap(), "--target", "wasm", "-o", dummy_out.to_str().unwrap()]),
        ("build_native", vec!["build", nonexistent.to_str().unwrap(), "--target", "native", "-o", dummy_out.to_str().unwrap()]),
        ("run", vec!["run", nonexistent.to_str().unwrap()]),
        ("disasm", vec!["disasm", nonexistent.to_str().unwrap(), "-o", dummy_out.to_str().unwrap()]),
    ];

    for (name, args) in subcommands {
        let out = knr_cmd()
            .args(&args)
            .output()
            .unwrap_or_else(|e| panic!("run {name}: {e}"));

        assert_eq!(
            out.status.code(),
            Some(1),
            "Subcommand {name} must exit code 1 for nonexistent file"
        );
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(!stderr.contains("panicked"), "Subcommand {name} must not panic");
        assert!(
            stderr.contains("No such file") || stderr.contains("FileNotFound") || stderr.contains("ghost_program.knr"),
            "Subcommand {name} must report file not found: {stderr}"
        );
    }
}
