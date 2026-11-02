//! Tier 5 Adversarial Hardening Test Suite for knr-cli
//!
//! Focus areas:
//! 1. CLI flag edge cases: missing flag values, unrecognized flags, missing files
//! 2. Subcommand help flags: --help and -h across all 5 subcommands
//! 3. Gas parameter boundaries: 0, negative, non-numeric, overflow
//! 4. Build target validation: invalid target strings, missing -o
//! 5. Corrupted bytecode input to disasm

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
        let file_name = format!("knr_cli_adv_{pid}_{id}_{suffix}");
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
    Command::new(bin)
}

fn fixture_path(subpath: &str) -> PathBuf {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let repo_root = manifest_dir.parent().unwrap().parent().unwrap();
    repo_root.join("tests").join("fixtures").join(subpath)
}

// ============================================================================
// 1. Subcommand Help Flags (-h and --help)
// ============================================================================

#[test]
fn test_adv_cli_subcommand_help_flags() {
    let subcommands = ["check", "run", "build", "asm", "disasm"];

    for sub in subcommands {
        // Test --help
        let out_long = knr_cmd()
            .args([sub, "--help"])
            .output()
            .unwrap_or_else(|e| panic!("failed to run knr {sub} --help: {e}"));
        assert!(
            out_long.status.success(),
            "knr {sub} --help must exit 0, got status {:?}",
            out_long.status
        );
        let stdout_long = String::from_utf8_lossy(&out_long.stdout);
        assert!(
            stdout_long.contains(sub),
            "Help output for {sub} must mention '{sub}': {stdout_long}"
        );

        // Test -h
        let out_short = knr_cmd()
            .args([sub, "-h"])
            .output()
            .unwrap_or_else(|e| panic!("failed to run knr {sub} -h: {e}"));
        assert!(
            out_short.status.success(),
            "knr {sub} -h must exit 0, got status {:?}",
            out_short.status
        );
    }
}

// ============================================================================
// 2. Missing Input File Arguments
// ============================================================================

#[test]
fn test_adv_cli_missing_input_file_errors() {
    let subcommands = ["check", "run", "build", "asm", "disasm"];

    for sub in subcommands {
        let out = knr_cmd()
            .arg(sub)
            .output()
            .unwrap_or_else(|e| panic!("failed to run knr {sub}: {e}"));
        assert!(
            !out.status.success(),
            "knr {sub} without input file must exit non-zero"
        );
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains("Missing") || stderr.contains("Usage") || stderr.contains("error"),
            "Stderr for knr {sub} must explain missing file argument: {stderr}"
        );
    }
}

// ============================================================================
// 3. Flag Parsing Boundaries: Gas parameter edge cases
// ============================================================================

#[test]
fn test_adv_cli_gas_parameter_boundaries() {
    let valid_file = fixture_path("valid/01_minimal.ssa");
    let file_str = valid_file.to_str().unwrap();

    // 1. Missing argument for --gas
    let out = knr_cmd()
        .args(["run", file_str, "--gas"])
        .output()
        .expect("knr run missing gas val");
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("Missing value for '--gas'"));

    // 2. Non-numeric value for --gas
    let out = knr_cmd()
        .args(["run", file_str, "--gas", "not_a_number"])
        .output()
        .expect("knr run invalid gas");
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("Invalid integer value"));

    // 3. Negative value for --gas
    let out = knr_cmd()
        .args(["run", file_str, "--gas", "-50"])
        .output()
        .expect("knr run negative gas");
    assert!(!out.status.success());

    // 4. Gas = 0 (must exhaust immediately)
    let out = knr_cmd()
        .args(["run", file_str, "--gas", "0"])
        .output()
        .expect("knr run gas 0");
    assert!(
        !out.status.success(),
        "knr run with --gas 0 must exhaust and fail"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("Gas exhausted"));

    // 5. Empty string for --gas
    let out = knr_cmd()
        .args(["run", file_str, "--gas", ""])
        .output()
        .expect("knr run empty gas");
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("Invalid integer value for '--gas'"));

    // 6. Empty string for --gas=
    let out = knr_cmd()
        .args(["run", file_str, "--gas="])
        .output()
        .expect("knr run empty gas eq");
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("Invalid integer value for '--gas'"));
}

#[test]
fn test_adv_cli_unlimited_gas_case_insensitivity_and_aliases() {
    let valid_file = fixture_path("valid/01_minimal.ssa");
    let file_str = valid_file.to_str().unwrap();

    let aliases = ["unlimited", "UNLIMITED", "Unlimited", "max", "MAX"];
    for alias in aliases {
        let out = knr_cmd()
            .args(["run", file_str, "--gas", alias])
            .output()
            .unwrap_or_else(|e| panic!("failed knr run --gas {alias}: {e}"));
        assert!(
            out.status.success(),
            "knr run --gas {alias} must exit 0, got status {:?}",
            out.status
        );
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(stdout.contains("42"), "Expected return value 42, got: {stdout}");

        // Also test --gas=<alias> syntax
        let out_eq = knr_cmd()
            .args(["run", file_str, &format!("--gas={alias}")])
            .output()
            .unwrap_or_else(|e| panic!("failed knr run --gas={alias}: {e}"));
        assert!(
            out_eq.status.success(),
            "knr run --gas={alias} must exit 0, got status {:?}",
            out_eq.status
        );
    }

    // Also test --unlimited-gas flag
    let out_flag = knr_cmd()
        .args(["run", file_str, "--unlimited-gas"])
        .output()
        .expect("failed knr run --unlimited-gas");
    assert!(out_flag.status.success());
    let stdout_flag = String::from_utf8_lossy(&out_flag.stdout);
    assert!(stdout_flag.contains("42"));
}

#[test]
fn test_adv_cli_site_server_ssa_verification_and_event_loop() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let repo_root = manifest_dir.parent().unwrap().parent().unwrap();
    let server_path = repo_root.join("site").join("server.ssa");
    let server_str = server_path.to_str().unwrap();

    // 1. Verify site/server.ssa parses and verifies cleanly via knr check CLI
    let out = knr_cmd()
        .args(["check", server_str])
        .output()
        .expect("knr check site/server.ssa");
    assert!(
        out.status.success(),
        "knr check site/server.ssa must succeed: stderr: {}, stdout: {}",
        String::from_utf8_lossy(&out.stderr),
        String::from_utf8_lossy(&out.stdout)
    );

    // 2. Multi-request server loop with --gas 20 executing beyond 20 instructions and completing cleanly
    // Loop of 50 requests (~350 steps). Under cumulative gas without auto-refill, it would die at step 20.
    let ssa_content = r#"
fn main() -> i64 effects(Ui, Async) {
bb0:
    %0 = const_i64 1
    %1 = chan_new i64 %0
    %2 = const_i64 0
    %3 = chan_send %1 %2
    %4 = const_i64 50
    br bb1

bb1:
    %5 = chan_recv %1
    %6 = cmp_lt %5 %4
    br_if %6 bb2 bb3

bb2:
    %7 = syscall Ui "ui.poll_event" %5
    %8 = add %5 %0
    %9 = chan_send %1 %8
    br bb1

bb3:
    free %1
    ret %5
}
"#;
    let ssa_temp = TempFile::new("multi_req_refill_adv.ssa");
    fs::write(ssa_temp.path(), ssa_content).expect("write ssa");

    let out_loop = knr_cmd()
        .args(["run", ssa_temp.path().to_str().unwrap(), "--gas", "20"])
        .output()
        .expect("knr run --gas 20 with refill");
    assert!(
        out_loop.status.success(),
        "knr run with gas refill must succeed: {}",
        String::from_utf8_lossy(&out_loop.stderr)
    );
    let stdout_loop = String::from_utf8_lossy(&out_loop.stdout);
    assert_eq!(stdout_loop.trim(), "50");

    // 3. Multi-request loop with --gas unlimited completes cleanly
    let out_unlim = knr_cmd()
        .args(["run", ssa_temp.path().to_str().unwrap(), "--gas", "unlimited"])
        .output()
        .expect("knr run --gas unlimited");
    assert!(out_unlim.status.success());
    let stdout_unlim = String::from_utf8_lossy(&out_unlim.stdout);
    assert_eq!(stdout_unlim.trim(), "50");
}

// ============================================================================
// 4. Build Subcommand Target & Output Validation
// ============================================================================

#[test]
fn test_adv_cli_build_flags_validation() {
    let valid_file = fixture_path("valid/01_minimal.ssa");
    let file_str = valid_file.to_str().unwrap();

    // 1. Missing value for --target
    let out = knr_cmd()
        .args(["build", file_str, "--target"])
        .output()
        .expect("knr build missing target val");
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("Missing value for '--target'"));

    // 2. Invalid target string
    let out = knr_cmd()
        .args(["build", file_str, "--target", "invalid_target", "-o", "/tmp/dummy"])
        .output()
        .expect("knr build invalid target");
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("Invalid target 'invalid_target'"));

    // 3. Missing -o / --output
    let out = knr_cmd()
        .args(["build", file_str, "--target", "wasm"])
        .output()
        .expect("knr build missing output");
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("Missing output path"));

    // 4. Missing value for -o
    let out = knr_cmd()
        .args(["build", file_str, "--target", "wasm", "-o"])
        .output()
        .expect("knr build trailing -o");
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("Missing value for output '-o'"));
}

// ============================================================================
// 5. Unknown Flag Rejection Across All Subcommands
// ============================================================================

#[test]
fn test_adv_cli_unknown_flags_rejected() {
    let valid_file = fixture_path("valid/01_minimal.ssa");
    let file_str = valid_file.to_str().unwrap();

    let cases: Vec<(&str, Vec<&str>)> = vec![
        ("check", vec!["check", file_str, "--bogus_flag"]),
        ("run", vec!["run", file_str, "--bogus_flag"]),
        ("build", vec!["build", file_str, "--target", "wasm", "-o", "/tmp/dummy", "--bogus_flag"]),
        ("asm", vec!["asm", file_str, "-o", "/tmp/dummy.knrbin", "--bogus_flag"]),
        ("disasm", vec!["disasm", file_str, "--bogus_flag"]),
    ];

    for (sub, args) in cases {
        let out = knr_cmd()
            .args(&args)
            .output()
            .unwrap_or_else(|e| panic!("failed knr {sub} with bogus flag: {e}"));
        assert!(
            !out.status.success(),
            "knr {sub} with --bogus_flag must fail"
        );
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains("Unknown option") || stderr.contains("error"),
            "Stderr for {sub} should mention unknown option: {stderr}"
        );
    }
}

// ============================================================================
// 6. Corrupted Input Handling (Disasm invalid binary bytes)
// ============================================================================

#[test]
fn test_adv_cli_corrupted_knrbin_disasm_rejection() {
    let temp_corrupt = TempFile::new("corrupted.knrbin");
    // Write corrupted bytes (invalid magic header)
    fs::write(temp_corrupt.path(), b"NOT_KNIR_MAGIC_GARBAGE").expect("write corrupt file");

    let out = knr_cmd()
        .args(["disasm", temp_corrupt.path().to_str().unwrap()])
        .output()
        .expect("run knr disasm on corrupt file");

    assert!(
        !out.status.success(),
        "knr disasm on corrupted binary must exit non-zero"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("magic") || stderr.contains("Invalid") || stderr.contains("Error") || stderr.contains("codec"),
        "Stderr should indicate magic header or binary error: {stderr}"
    );
}
