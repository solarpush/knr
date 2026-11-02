use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::harness::*;

struct TempFile {
    path: PathBuf,
}

impl TempFile {
    fn new(suffix: &str) -> Self {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let id = COUNTER.fetch_add(1, Ordering::SeqCst);
        let pid = std::process::id();
        let file_name = format!("knr_e2e_{pid}_{id}_{suffix}");
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

fn fixture_file(subpath: &str) -> PathBuf {
    let loader = FixtureLoader::new();
    loader.fixtures_dir().join(subpath)
}

fn get_knr_bin() -> String {
    ProcessRunner::find_knr_binary().expect("knr binary must exist in workspace target or PATH")
}

#[test]
fn test_wave3_f61_main_dispatcher() {
    let bin = get_knr_bin();

    // 1. No arguments -> exits non-zero, displays usage
    let res = ProcessRunner::run(&bin, &[]).expect("run knr no args");
    assert!(!res.success, "knr without args should fail");
    let combined = format!("{}\n{}", res.stdout, res.stderr);
    assert!(
        combined.contains("check") && combined.contains("run") && combined.contains("build"),
        "Usage must list subcommands: {combined}"
    );

    // 2. --help -> exits 0
    let res = ProcessRunner::run(&bin, &["--help"]).expect("run knr --help");
    assert!(res.success, "knr --help must exit 0");
    assert!(
        res.stdout.contains("check") && res.stdout.contains("run") && res.stdout.contains("build"),
        "Help must mention subcommands: {}",
        res.stdout
    );

    // 3. --version -> exits 0
    let res = ProcessRunner::run(&bin, &["--version"]).expect("run knr --version");
    assert!(res.success, "knr --version must exit 0");
    assert!(
        res.stdout.contains("knr 0.1.0"),
        "Version output mismatch: {}",
        res.stdout
    );

    // 4. Unknown subcommand -> exits non-zero
    let res = ProcessRunner::run(&bin, &["unknown_subcmd"]).expect("run knr unknown");
    assert!(!res.success, "Unknown subcommand must fail");
}

#[test]
fn test_wave3_f62_check_subcommand() {
    let bin = get_knr_bin();
    let valid_file = fixture_file("valid/01_minimal.ssa");
    let invalid_file = fixture_file("invalid/err04_use_after_move_map.ssa");

    // 1. Valid file -> exits 0
    let res = ProcessRunner::run(&bin, &["check", valid_file.to_str().unwrap()])
        .expect("knr check valid");
    assert!(res.success, "knr check valid must exit 0: {}", res.stderr);

    // 2. Valid file with --json -> exits 0, outputs empty array []
    let res = ProcessRunner::run(&bin, &["check", valid_file.to_str().unwrap(), "--json"])
        .expect("knr check valid --json");
    assert!(res.success, "knr check --json must exit 0");
    let json_val: serde_json::Value =
        serde_json::from_str(res.stdout.trim()).expect("Valid JSON array []");
    assert_eq!(json_val, serde_json::json!([]));

    // 3. Invalid file -> exits non-zero
    let res = ProcessRunner::run(&bin, &["check", invalid_file.to_str().unwrap()])
        .expect("knr check invalid");
    assert!(!res.success, "knr check on invalid fixture must exit non-zero");

    // 4. Invalid file with --json -> exits non-zero, outputs diagnostics JSON
    let res = ProcessRunner::run(&bin, &["check", invalid_file.to_str().unwrap(), "--json"])
        .expect("knr check invalid --json");
    assert!(!res.success, "knr check --json must exit non-zero on invalid fixture");
    let json_val: serde_json::Value =
        serde_json::from_str(res.stdout.trim()).expect("Diagnostics JSON array");
    let arr = json_val.as_array().expect("Must be JSON array");
    assert!(!arr.is_empty(), "Diagnostics array must not be empty");
    assert_eq!(arr[0]["status"], "error");
    assert_eq!(arr[0]["error_kind"], "UseAfterMove");
}

#[test]
fn test_wave3_f63_run_subcommand() {
    let bin = get_knr_bin();
    let minimal_file = fixture_file("valid/01_minimal.ssa");
    let canonical_file = fixture_file("valid/canonical_15_lines.ssa");
    let loop_file = fixture_file("valid/cli_loop_gas.ssa");

    // 1. Nominal execution -> exits 0, returns 42
    let res = ProcessRunner::run(&bin, &["run", minimal_file.to_str().unwrap()])
        .expect("knr run minimal");
    assert!(res.success, "knr run must exit 0: {}", res.stderr);
    assert!(res.stdout.contains("42"), "Run output should contain 42: {}", res.stdout);

    // 2. Gas exhaustion on step limit
    let res = ProcessRunner::run(
        &bin,
        &["run", canonical_file.to_str().unwrap(), "--gas", "2"],
    )
    .expect("knr run --gas 2");
    assert!(!res.success, "knr run --gas 2 must abort");
    let combined = format!("{}\n{}", res.stdout, res.stderr);
    assert!(
        combined.to_lowercase().contains("gas") && combined.to_lowercase().contains("exhausted"),
        "Must mention gas exhaustion: {combined}"
    );

    // 3. Gas exhaustion on infinite loop
    let res = ProcessRunner::run(
        &bin,
        &["run", loop_file.to_str().unwrap(), "--gas", "50"],
    )
    .expect("knr run loop --gas 50");
    assert!(!res.success, "Loop must exhaust gas");
    let combined = format!("{}\n{}", res.stdout, res.stderr);
    assert!(
        combined.to_lowercase().contains("gas") && combined.to_lowercase().contains("exhausted"),
        "Must mention gas exhaustion: {combined}"
    );

    // 4. Trace mode -> outputs JSON trace records
    let res = ProcessRunner::run(&bin, &["run", minimal_file.to_str().unwrap(), "--trace"])
        .expect("knr run --trace");
    assert!(res.success, "knr run --trace must exit 0");
    let start_idx = res.stdout.find('[').expect("Trace JSON array '['");
    let end_idx = res.stdout.rfind(']').expect("Trace JSON array ']'");
    let trace: Vec<serde_json::Value> =
        serde_json::from_str(&res.stdout[start_idx..=end_idx]).expect("Parse trace JSON");
    assert!(!trace.is_empty(), "Trace must contain records");
    assert_eq!(trace[0]["opcode"], "const_i64");
    assert_eq!(trace[0]["dst_reg"], "%0");
    assert_eq!(trace[0]["new_value"], 42);
}

#[test]
fn test_wave3_f64_build_subcommand() {
    let bin = get_knr_bin();
    let minimal_file = fixture_file("valid/01_minimal.ssa");
    let arith_file = fixture_file("valid/cli_native_arith.ssa");

    let wasm_temp = TempFile::new("out.wasm");
    let native_temp = TempFile::new("out_exe");
    let arith_temp = TempFile::new("out_arith_exe");

    // 1. Build WASM
    let res = ProcessRunner::run(
        &bin,
        &[
            "build",
            minimal_file.to_str().unwrap(),
            "--target",
            "wasm",
            "-o",
            wasm_temp.path().to_str().unwrap(),
        ],
    )
    .expect("knr build wasm");
    assert!(res.success, "knr build --target wasm must exit 0: {}", res.stderr);
    let wasm_bytes = fs::read(wasm_temp.path()).expect("read wasm");
    assert!(wasm_bytes.len() >= 8);
    assert_eq!(&wasm_bytes[0..4], b"\0asm");
    assert_eq!(&wasm_bytes[4..8], &[0x01, 0x00, 0x00, 0x00]);

    // 2. Build Native & Execute
    let res = ProcessRunner::run(
        &bin,
        &[
            "build",
            minimal_file.to_str().unwrap(),
            "--target",
            "native",
            "-o",
            native_temp.path().to_str().unwrap(),
        ],
    )
    .expect("knr build native");
    assert!(res.success, "knr build --target native must exit 0: {}", res.stderr);

    let bin_bytes = fs::read(native_temp.path()).expect("read native binary");
    assert_eq!(&bin_bytes[0..4], b"\x7fELF", "Must be ELF binary");

    let run_output = Command::new(native_temp.path())
        .output()
        .expect("execute native binary");
    assert_eq!(run_output.status.code(), Some(42), "Native exit code must be 42");

    // 3. Build Native Arithmetic & Execute
    let res = ProcessRunner::run(
        &bin,
        &[
            "build",
            arith_file.to_str().unwrap(),
            "--target",
            "native",
            "-o",
            arith_temp.path().to_str().unwrap(),
        ],
    )
    .expect("knr build native arith");
    assert!(res.success, "knr build native arith must exit 0: {}", res.stderr);

    let run_output = Command::new(arith_temp.path())
        .output()
        .expect("execute arith binary");
    assert_eq!(run_output.status.code(), Some(42), "Arith exit code must be 42");
}

#[test]
fn test_wave3_f65_and_f66_asm_disasm_roundtrip() {
    let bin = get_knr_bin();
    let canonical_file = fixture_file("valid/canonical_15_lines.ssa");
    let original_text = fs::read_to_string(&canonical_file).expect("read canonical ssa");

    let bin_temp = TempFile::new("canonical.knrbin");
    let roundtrip_temp = TempFile::new("roundtrip.ssa");

    // 1. knr asm
    let res = ProcessRunner::run(
        &bin,
        &[
            "asm",
            canonical_file.to_str().unwrap(),
            "-o",
            bin_temp.path().to_str().unwrap(),
        ],
    )
    .expect("knr asm");
    assert!(res.success, "knr asm must exit 0: {}", res.stderr);

    let bin_bytes = fs::read(bin_temp.path()).expect("read knrbin");
    assert!(bin_bytes.len() >= 6);
    assert_eq!(&bin_bytes[0..4], b"KNIR");

    // 2. knr disasm to file
    let res = ProcessRunner::run(
        &bin,
        &[
            "disasm",
            bin_temp.path().to_str().unwrap(),
            "-o",
            roundtrip_temp.path().to_str().unwrap(),
        ],
    )
    .expect("knr disasm to file");
    assert!(res.success, "knr disasm must exit 0: {}", res.stderr);

    let roundtrip_text = fs::read_to_string(roundtrip_temp.path()).expect("read roundtrip ssa");
    assert_eq!(
        roundtrip_text.trim(),
        original_text.trim(),
        "Roundtrip disassembly must match canonical SSA"
    );

    // 3. knr disasm to stdout
    let res = ProcessRunner::run(
        &bin,
        &["disasm", bin_temp.path().to_str().unwrap()],
    )
    .expect("knr disasm to stdout");
    assert!(res.success, "knr disasm to stdout must exit 0");
    assert!(
        res.stdout.contains("fn main() -> i64"),
        "Stdout should contain canonical disassembly: {}",
        res.stdout
    );
}

#[test]
fn test_wave3_f67_fixtures_suite() {
    let loader = FixtureLoader::new();
    let valids = loader.load_valid_fixtures();
    let invalids = loader.load_invalid_fixtures();

    assert!(valids.len() >= 13, "Must have at least 13 valid fixtures (found {})", valids.len());
    assert_eq!(invalids.len(), 20, "Must have exactly 20 invalid fixtures");

    assert!(
        valids.iter().any(|f| f.filename == "cli_loop_gas.ssa"),
        "Must contain cli_loop_gas.ssa fixture"
    );
    assert!(
        valids.iter().any(|f| f.filename == "cli_native_arith.ssa"),
        "Must contain cli_native_arith.ssa fixture"
    );
}
