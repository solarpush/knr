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
        let file_name = format!("knr_test_{pid}_{id}_{suffix}");
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
// 1. CLI Usage & Help Tests
// ============================================================================

#[test]
fn test_cli_no_args_shows_usage() {
    let output = knr_cmd().output().expect("failed to execute knr");
    assert!(!output.status.success(), "knr with no args should exit non-zero");
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let combined = format!("{stdout}\n{stderr}");
    assert!(
        combined.contains("check") && combined.contains("run") && combined.contains("build"),
        "Usage must mention subcommands: {combined}"
    );
}

#[test]
fn test_cli_help_flag() {
    let output = knr_cmd().arg("--help").output().expect("failed to execute knr");
    assert!(output.status.success(), "knr --help must exit 0");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("check"), "Help should explain 'check'");
    assert!(stdout.contains("run"), "Help should explain 'run'");
    assert!(stdout.contains("build"), "Help should explain 'build'");
    assert!(stdout.contains("asm"), "Help should explain 'asm'");
    assert!(stdout.contains("disasm"), "Help should explain 'disasm'");
}

#[test]
fn test_cli_unknown_subcommand() {
    let output = knr_cmd().arg("nonexistent_command").output().expect("failed to execute knr");
    assert!(!output.status.success(), "Unknown command must fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Unknown") || stderr.contains("Usage") || stderr.contains("error"),
        "Stderr should indicate unknown command: {stderr}"
    );
}

// ============================================================================
// 2. Subcommand: knr check
// ============================================================================

#[test]
fn test_check_valid_file_human_output() {
    let file = fixture_path("valid/01_minimal.ssa");
    let output = knr_cmd()
        .args(["check", file.to_str().unwrap()])
        .output()
        .expect("failed to execute knr check");
    assert!(
        output.status.success(),
        "knr check on valid file must exit 0, got stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn test_check_valid_file_json_output() {
    let file = fixture_path("valid/01_minimal.ssa");
    let output = knr_cmd()
        .args(["check", file.to_str().unwrap(), "--json"])
        .output()
        .expect("failed to execute knr check --json");
    assert!(output.status.success(), "knr check --json on valid file must exit 0");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let json_val: serde_json::Value = serde_json::from_str(stdout.trim())
        .unwrap_or_else(|e| panic!("stdout is not valid JSON '{stdout}': {e}"));

    if let serde_json::Value::Array(arr) = json_val {
        assert!(arr.is_empty(), "Expected empty diagnostics array, got: {:?}", arr);
    } else {
        panic!("Expected JSON array, got: {json_val}");
    }
}

#[test]
fn test_check_invalid_file_json_output() {
    let invalid_files = [
        ("invalid/err04_use_after_move_map.ssa", "UseAfterMove"),
        ("invalid/err07_linear_leak_buf.ssa", "LinearLeak"),
        ("invalid/err09_type_mismatch_arith.ssa", "TypeMismatch"),
        ("invalid/err11_undeclared_effect_net.ssa", "UndeclaredEffect"),
    ];

    for (subpath, expected_kind) in invalid_files {
        let file = fixture_path(subpath);
        let output = knr_cmd()
            .args(["check", file.to_str().unwrap(), "--json"])
            .output()
            .expect("failed to execute knr check");
        assert!(
            !output.status.success(),
            "knr check --json on {subpath} must exit non-zero"
        );

        let stdout = String::from_utf8_lossy(&output.stdout);
        let json_val: serde_json::Value = serde_json::from_str(stdout.trim())
            .unwrap_or_else(|e| panic!("stdout for {subpath} is not valid JSON '{stdout}': {e}"));

        let arr = json_val.as_array().expect("Output must be a JSON array of diagnostics");
        assert!(!arr.is_empty(), "Diagnostics array must not be empty for {subpath}");

        let diag = &arr[0];
        assert_eq!(diag["status"], "error");
        assert_eq!(
            diag["error_kind"], expected_kind,
            "Mismatch error_kind for {subpath}"
        );
        assert!(diag["line"].is_number(), "Diagnostic must contain line number");
    }
}

#[test]
fn test_check_nonexistent_file() {
    let output = knr_cmd()
        .args(["check", "this_file_does_not_exist_12345.ssa"])
        .output()
        .expect("failed to execute knr check");
    assert!(!output.status.success(), "Checking nonexistent file must exit non-zero");
}

// ============================================================================
// 3. Subcommand: knr run
// ============================================================================

#[test]
fn test_run_valid_file_returns_value() {
    let file = fixture_path("valid/01_minimal.ssa");
    let output = knr_cmd()
        .args(["run", file.to_str().unwrap()])
        .output()
        .expect("failed to execute knr run");
    assert!(
        output.status.success(),
        "knr run must exit 0, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("42"), "Run output should contain return value 42: {stdout}");
}

#[test]
fn test_run_gas_limit_exhaustion() {
    let file = fixture_path("valid/canonical_15_lines.ssa");
    let output = knr_cmd()
        .args(["run", file.to_str().unwrap(), "--gas", "2"])
        .output()
        .expect("failed to execute knr run --gas 2");
    assert!(!output.status.success(), "knr run --gas 2 must abort due to gas exhaustion");
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        combined.to_lowercase().contains("gas") && combined.to_lowercase().contains("exhausted"),
        "Error message should mention Gas exhausted: {combined}"
    );
}

#[test]
fn test_run_loop_gas_exhaustion() {
    let file = fixture_path("valid/cli_loop_gas.ssa");
    let output = knr_cmd()
        .args(["run", file.to_str().unwrap(), "--gas", "50"])
        .output()
        .expect("failed to execute knr run --gas 50");
    assert!(!output.status.success(), "knr run loop with gas 50 must exhaust gas");
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        combined.to_lowercase().contains("gas") && combined.to_lowercase().contains("exhausted"),
        "Error message should mention Gas exhausted: {combined}"
    );
}

#[test]
fn test_run_trace_json_output() {
    let file = fixture_path("valid/01_minimal.ssa");
    let output = knr_cmd()
        .args(["run", file.to_str().unwrap(), "--trace"])
        .output()
        .expect("failed to execute knr run --trace");
    assert!(output.status.success(), "knr run --trace must exit 0");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let start_idx = stdout.find('[').expect("Trace output should contain JSON array '['");
    let end_idx = stdout.rfind(']').expect("Trace output should contain JSON array ']'");
    let json_slice = &stdout[start_idx..=end_idx];

    let trace: Vec<serde_json::Value> = serde_json::from_str(json_slice)
        .unwrap_or_else(|e| panic!("Failed to parse trace JSON '{json_slice}': {e}"));

    assert!(!trace.is_empty(), "Trace records should not be empty");
    assert!(
        trace[0]["step"] == 0 || trace[0]["step"] == 1,
        "Step count should be 0 or 1"
    );
    assert_eq!(trace[0]["opcode"], "const_i64");
    assert_eq!(trace[0]["dst_reg"], "%0");
    assert_eq!(trace[0]["new_value"], 42);
}

// ============================================================================
// 4. Subcommand: knr asm & knr disasm (Roundtrip)
// ============================================================================

#[test]
fn test_asm_and_disasm_roundtrip_bijection() {
    let src_file = fixture_path("valid/canonical_15_lines.ssa");
    let original_content = fs::read_to_string(&src_file).expect("read original ssa");

    let bin_temp = TempFile::new("test.knrbin");
    let ssa_temp = TempFile::new("roundtrip.ssa");

    let asm_output = knr_cmd()
        .args(["asm", src_file.to_str().unwrap(), "-o", bin_temp.path().to_str().unwrap()])
        .output()
        .expect("failed to execute knr asm");
    assert!(
        asm_output.status.success(),
        "knr asm failed: {}",
        String::from_utf8_lossy(&asm_output.stderr)
    );

    let bin_bytes = fs::read(bin_temp.path()).expect("read compiled knrbin");
    assert!(bin_bytes.len() >= 6, "knrbin too short");
    assert_eq!(&bin_bytes[0..4], b"KNIR", "Magic must be KNIR");

    let disasm_output = knr_cmd()
        .args(["disasm", bin_temp.path().to_str().unwrap(), "-o", ssa_temp.path().to_str().unwrap()])
        .output()
        .expect("failed to execute knr disasm");
    assert!(
        disasm_output.status.success(),
        "knr disasm failed: {}",
        String::from_utf8_lossy(&disasm_output.stderr)
    );

    let roundtrip_content = fs::read_to_string(ssa_temp.path()).expect("read roundtrip ssa");
    assert_eq!(
        roundtrip_content.trim(),
        original_content.trim(),
        "Canonical 15 lines roundtrip must be identical"
    );
}

#[test]
fn test_disasm_stdout_when_no_output_flag() {
    let src_file = fixture_path("valid/01_minimal.ssa");
    let bin_temp = TempFile::new("minimal.knrbin");

    let _ = knr_cmd()
        .args(["asm", src_file.to_str().unwrap(), "-o", bin_temp.path().to_str().unwrap()])
        .output()
        .expect("knr asm");

    let output = knr_cmd()
        .args(["disasm", bin_temp.path().to_str().unwrap()])
        .output()
        .expect("knr disasm to stdout");
    assert!(output.status.success(), "knr disasm to stdout must exit 0");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("fn main() -> i64"), "Disassembly stdout should contain fn main");
    assert!(stdout.contains("const_i64 42"), "Disassembly stdout should contain const_i64");
}

// ============================================================================
// 5. Subcommand: knr build (--target wasm & --target native)
// ============================================================================

#[test]
fn test_build_wasm_target() {
    let src_file = fixture_path("valid/01_minimal.ssa");
    let wasm_temp = TempFile::new("minimal.wasm");

    let output = knr_cmd()
        .args([
            "build",
            src_file.to_str().unwrap(),
            "--target",
            "wasm",
            "-o",
            wasm_temp.path().to_str().unwrap(),
        ])
        .output()
        .expect("failed to execute knr build wasm");
    assert!(
        output.status.success(),
        "knr build --target wasm must exit 0, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let wasm_bytes = fs::read(wasm_temp.path()).expect("read generated wasm");
    assert!(wasm_bytes.len() >= 8, "WASM binary too short");
    assert_eq!(
        &wasm_bytes[0..4],
        &[0x00, 0x61, 0x73, 0x6D],
        "WASM magic must be \\0asm"
    );
    assert_eq!(
        &wasm_bytes[4..8],
        &[0x01, 0x00, 0x00, 0x00],
        "WASM version must be 1"
    );
}

#[test]
fn test_build_native_target_and_execute() {
    let src_file = fixture_path("valid/01_minimal.ssa");
    let native_temp = TempFile::new("minimal_exe");

    let output = knr_cmd()
        .args([
            "build",
            src_file.to_str().unwrap(),
            "--target",
            "native",
            "-o",
            native_temp.path().to_str().unwrap(),
        ])
        .output()
        .expect("failed to execute knr build native");
    assert!(
        output.status.success(),
        "knr build --target native must exit 0, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let bin_bytes = fs::read(native_temp.path()).expect("read generated native executable");
    assert!(bin_bytes.len() > 64, "Native binary too small");
    assert_eq!(
        &bin_bytes[0..4],
        &[0x7F, b'E', b'L', b'F'],
        "Native binary must start with ELF magic \\x7fELF"
    );

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(native_temp.path()).unwrap().permissions();
        perms.set_mode(0o755);
        let _ = fs::set_permissions(native_temp.path(), perms);

        let run_output = Command::new(native_temp.path())
            .output()
            .expect("failed to execute compiled native binary");
        assert_eq!(
            run_output.status.code(),
            Some(42),
            "Compiled native binary must exit with status 42"
        );
    }
}

#[test]
fn test_build_native_target_arithmetic() {
    let src_file = fixture_path("valid/cli_native_arith.ssa");
    let native_temp = TempFile::new("native_arith_exe");

    let output = knr_cmd()
        .args([
            "build",
            src_file.to_str().unwrap(),
            "--target",
            "native",
            "-o",
            native_temp.path().to_str().unwrap(),
        ])
        .output()
        .expect("failed to execute knr build native on arithmetic fixture");
    assert!(
        output.status.success(),
        "knr build --target native arithmetic must exit 0, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    #[cfg(unix)]
    {
        let run_output = Command::new(native_temp.path())
            .output()
            .expect("failed to execute compiled native binary");
        assert_eq!(
            run_output.status.code(),
            Some(42),
            "Compiled native arithmetic binary must exit with status 42"
        );
    }
}

#[test]
fn test_cli_struct_asm_disasm_and_run() {
    let ssa_content = r#"schema Point {
    0: x i64,
    1: y i64,
}

fn main() -> i64 {
bb0:
    %0 = struct_new Point
    %1 = const_i64 35
    %2 = struct_set %0 0 %1
    %3 = const_i64 7
    %4 = struct_set %2 1 %3
    %5 = struct_get %4 0
    %6 = struct_get %4 1
    %7 = add %5 %6
    free %4
    ret %7
}
"#;
    let ssa_temp = TempFile::new("struct_cli_test.ssa");
    std::fs::write(ssa_temp.path(), ssa_content).expect("write ssa");

    // 1. knr check
    let check_out = knr_cmd()
        .args(["check", ssa_temp.path().to_str().unwrap()])
        .output()
        .expect("knr check failed");
    assert!(check_out.status.success(), "knr check on struct ssa must succeed");

    // 2. knr run
    let run_out = knr_cmd()
        .args(["run", ssa_temp.path().to_str().unwrap()])
        .output()
        .expect("knr run failed");
    assert!(run_out.status.success(), "knr run on struct ssa must succeed");
    let run_val = String::from_utf8_lossy(&run_out.stdout).trim().to_string();
    assert_eq!(run_val, "42", "knr run should compute 35 + 7 = 42");

    // 3. knr asm
    let bin_temp = TempFile::new("struct_cli_test.knrbin");
    let asm_out = knr_cmd()
        .args([
            "asm",
            ssa_temp.path().to_str().unwrap(),
            "-o",
            bin_temp.path().to_str().unwrap(),
        ])
        .output()
        .expect("knr asm failed");
    assert!(asm_out.status.success(), "knr asm on struct ssa must succeed");

    // 4. knr disasm
    let disasm_out = knr_cmd()
        .args(["disasm", bin_temp.path().to_str().unwrap()])
        .output()
        .expect("knr disasm failed");
    assert!(disasm_out.status.success(), "knr disasm on struct knrbin must succeed");
    let disasm_text = String::from_utf8_lossy(&disasm_out.stdout);
    assert!(disasm_text.contains("schema Point"), "Disassembly must contain schema Point: {disasm_text}");
    assert!(disasm_text.contains("struct_new Point"), "Disassembly must contain struct_new Point: {disasm_text}");
}

// ============================================================================
// 6. Subcommand: knr update & install.sh
// ============================================================================

#[test]
fn test_update_semver_and_help() {
    use knr_cli::commands::update::{is_newer_version, parse_semver};

    assert_eq!(parse_semver("v0.1.0"), Some((0, 1, 0)));
    assert_eq!(parse_semver("1.2.3-rc1"), Some((1, 2, 3)));
    assert!(!is_newer_version("0.1.0", "v0.1.0"));
    assert!(is_newer_version("0.1.0", "v0.2.0"));
    assert!(is_newer_version("0.1.0", "v1.0.0"));
    assert!(!is_newer_version("0.2.0", "v0.1.9"));

    let out = knr_cmd()
        .args(["update", "--help"])
        .output()
        .expect("knr update --help");
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("--check"));
    assert!(stdout.contains("--force"));
    assert!(stdout.contains("--version"));
}

#[test]
fn test_update_check_and_self_replacement_with_install_sh() {
    let target = knr_cli::commands::update::current_target_triple().expect("supported target");
    let pid = std::process::id();
    let work_dir = std::env::temp_dir().join(format!("knr_update_e2e_{pid}"));
    let _ = fs::remove_dir_all(&work_dir);
    fs::create_dir_all(&work_dir).expect("create work_dir");

    // 1. Test `knr update --check` when already up to date (v0.1.0)
    let json_same = work_dir.join("release_same.json");
    fs::write(
        &json_same,
        r#"{"tag_name": "v0.1.0", "html_url": "https://github.com/solarpush/knr/releases/tag/v0.1.0", "assets": []}"#,
    )
    .unwrap();

    let out_same = knr_cmd()
        .args(["update", "--check"])
        .env("KNR_RELEASE_API_URL", format!("file://{}", json_same.display()))
        .output()
        .expect("knr update --check same");
    assert!(out_same.status.success());
    let stdout_same = String::from_utf8_lossy(&out_same.stdout);
    assert!(
        stdout_same.contains("already up to date"),
        "Expected 'already up to date', got: {stdout_same}"
    );

    // 2. Build a mock release archive `knr-<target>.tar.gz` containing a mock `knr` binary (v9.9.9)
    let stage_dir = work_dir.join("stage");
    fs::create_dir_all(&stage_dir).unwrap();
    let mock_bin = stage_dir.join("knr");
    fs::write(&mock_bin, "#!/usr/bin/env sh\necho \"knr 9.9.9\"\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&mock_bin).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&mock_bin, perms).unwrap();
    }

    let archive_name = format!("knr-{target}.tar.gz");
    let archive_path = work_dir.join(&archive_name);
    let tar_status = Command::new("tar")
        .args(["-czf", archive_path.to_str().unwrap(), "-C", stage_dir.to_str().unwrap(), "knr"])
        .status()
        .expect("tar -czf");
    assert!(tar_status.success());

    // Create mock release JSON for v9.9.9 pointing to the local tarball
    let json_newer = work_dir.join("release_newer.json");
    let release_json_content = format!(
        r#"{{
            "tag_name": "v9.9.9",
            "html_url": "https://github.com/solarpush/knr/releases/tag/v9.9.9",
            "assets": [
                {{
                    "name": "{archive_name}",
                    "browser_download_url": "file://{}"
                }}
            ]
        }}"#,
        archive_path.display()
    );
    fs::write(&json_newer, release_json_content).unwrap();

    // 3. Test `knr update` replacing a target binary
    let target_bin = work_dir.join("installed_knr");
    fs::write(&target_bin, "old binary").unwrap();

    let out_update = knr_cmd()
        .arg("update")
        .env("KNR_RELEASE_API_URL", format!("file://{}", json_newer.display()))
        .env("KNR_UPDATE_BIN_PATH", &target_bin)
        .output()
        .expect("knr update");
    assert!(
        out_update.status.success(),
        "knr update failed: {}",
        String::from_utf8_lossy(&out_update.stderr)
    );

    let ver_out = Command::new(&target_bin)
        .arg("--version")
        .output()
        .expect("run updated binary");
    assert_eq!(String::from_utf8_lossy(&ver_out.stdout).trim(), "knr 9.9.9");

    // 4. Test `install.sh` end-to-end with local release URL
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let repo_root = manifest_dir.parent().unwrap().parent().unwrap();
    let install_script = repo_root.join("install.sh");
    let install_dir = work_dir.join("bin_from_installer");

    let installer_out = Command::new("sh")
        .arg(&install_script)
        .env("KNR_RELEASE_API_URL", format!("file://{}", json_newer.display()))
        .env("KNR_RELEASE_BASE_URL", format!("file://{}", work_dir.display()))
        .env("KNR_INSTALL_DIR", &install_dir)
        .output()
        .expect("run install.sh");
    assert!(
        installer_out.status.success(),
        "install.sh failed: {}\n{}",
        String::from_utf8_lossy(&installer_out.stdout),
        String::from_utf8_lossy(&installer_out.stderr)
    );

    let installed_bin = install_dir.join("knr");
    assert!(installed_bin.is_file(), "install.sh must install knr binary");
    let installed_ver = Command::new(&installed_bin)
        .arg("--version")
        .output()
        .expect("run installed knr");
    assert_eq!(String::from_utf8_lossy(&installed_ver.stdout).trim(), "knr 9.9.9");

    let _ = fs::remove_dir_all(&work_dir);
}

