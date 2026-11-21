//! Tier 5 Adversarial Hardening Suite for knr-cli Tooling and Arguments Engine
//!
//! Scope:
//! 1. `args.rs` flag parsing matrix: `--gui`, `-g`, `--watch`, `-w`, `--once`, `--gas`, `--unlimited-gas`
//! 2. `commands/init.rs` & `commands/new.rs`: all 5 templates, workspace manifests, force overwrite
//! 3. `commands/disasm.rs`: schema splitting on binary `.knrbin` and verified re-check of `.ssa`
//! 4. `commands/run.rs`: headless `--gui` execution and error resilience
//! 5. `commands/check.rs`: structured JSON diagnostics and `.knr` graph exploration

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

use knr_cli::args::{parse_args, CliCommand, RunArgs, DevArgs, InitArgs, NewArgs, CheckArgs};

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(suffix: &str) -> Self {
        static COUNTER: AtomicUsize = AtomicUsize::new(700);
        let id = COUNTER.fetch_add(1, Ordering::SeqCst);
        let pid = std::process::id();
        let dir_name = format!("knr_adv_tier5_cli_{pid}_{id}_{suffix}");
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

fn knr_bin() -> Command {
    let bin = env!("CARGO_BIN_EXE_knr");
    let mut cmd = Command::new(bin);
    cmd.env_remove("DISPLAY");
    cmd
}

// =========================================================================
// 1. `args.rs` Parsing Matrix & Flag Validations
// =========================================================================

#[test]
fn test_adv_cli_args_parsing_unit_matrix() {
    // Run args: --gui and -g
    let cmd_run_gui = parse_args(vec!["knr".into(), "run".into(), "main.knr".into(), "--gui".into()]).unwrap();
    assert_eq!(cmd_run_gui, CliCommand::Run(RunArgs {
        file: PathBuf::from("main.knr"),
        gas: 1_000_000,
        trace: false,
        watch: false,
        once: false,
        gui: true,
    }));

    let cmd_run_short_g = parse_args(vec!["knr".into(), "run".into(), "main.knr".into(), "-g".into(), "--gas=50000".into()]).unwrap();
    assert_eq!(cmd_run_short_g, CliCommand::Run(RunArgs {
        file: PathBuf::from("main.knr"),
        gas: 50_000,
        trace: false,
        watch: false,
        once: false,
        gui: true,
    }));

    // Run args: --unlimited-gas and -w --once
    let cmd_unlimited = parse_args(vec!["knr".into(), "run".into(), "app.ssa".into(), "--unlimited-gas".into(), "-w".into(), "--once".into()]).unwrap();
    assert_eq!(cmd_unlimited, CliCommand::Run(RunArgs {
        file: PathBuf::from("app.ssa"),
        gas: u64::MAX,
        trace: false,
        watch: true,
        once: true,
        gui: false,
    }));

    // Dev args: --debounce, --poll, --no-clear, --once
    let cmd_dev = parse_args(vec!["knr".into(), "dev".into(), "my_proj".into(), "--debounce".into(), "50".into(), "--poll=100".into(), "--no-clear".into(), "--once".into()]).unwrap();
    assert_eq!(cmd_dev, CliCommand::Dev(DevArgs {
        path: Some(PathBuf::from("my_proj")),
        debounce_ms: 50,
        poll_ms: 100,
        once: true,
        gas: 1_000_000,
        trace: false,
        clear: false,
        gui: false,
    }));

    // Init args: --template, --name, --workspace, --force
    let cmd_init = parse_args(vec!["knr".into(), "init".into(), "target_dir".into(), "--template=ui".into(), "--name".into(), "my_ui".into(), "--workspace".into(), "--force".into()]).unwrap();
    assert_eq!(cmd_init, CliCommand::Init(InitArgs {
        dir: Some(PathBuf::from("target_dir")),
        template: "ui".to_string(),
        name: Some("my_ui".to_string()),
        workspace: true,
        force: true,
    }));

    // New args
    let cmd_new = parse_args(vec!["knr".into(), "new".into(), "new_ws".into(), "--workspace".into()]).unwrap();
    assert_eq!(cmd_new, CliCommand::New(NewArgs {
        path: PathBuf::from("new_ws"),
        workspace: true,
        template: "minimal".to_string(),
        name: None,
        force: false,
    }));

    // Check args: --json and --graph
    let cmd_check = parse_args(vec!["knr".into(), "check".into(), "test.knr".into(), "--json".into(), "--graph".into()]).unwrap();
    assert_eq!(cmd_check, CliCommand::Check(CheckArgs {
        file: PathBuf::from("test.knr"),
        json: true,
        graph: true,
    }));

    // Rejection of unknown options across subcommands
    assert!(parse_args(vec!["knr".into(), "check".into(), "f.knr".into(), "--bogus".into()]).is_err());
    assert!(parse_args(vec!["knr".into(), "run".into(), "f.knr".into(), "--unknown".into()]).is_err());
    assert!(parse_args(vec!["knr".into(), "dev".into(), "--invalid".into()]).is_err());
    assert!(parse_args(vec!["knr".into(), "init".into(), "--unknown".into()]).is_err());
    assert!(parse_args(vec!["knr".into(), "new".into(), "path".into(), "--bad".into()]).is_err());

    // Rejection of conflicting explore options
    assert!(parse_args(vec!["knr".into(), "explore".into(), ".".into(), "--json".into(), "--format".into(), "mermaid".into()]).is_err());
}

// =========================================================================
// 2. `init` & `new` Templates Matrix and Workspace Management
// =========================================================================

#[test]
fn test_adv_cli_init_templates_matrix() {
    let templates = [
        ("minimal", "0"),
        ("app", "0"),
        ("ui", "800"),
        ("service", "0"),
        ("lib", "40"),
    ];

    for (tmpl, expected_ret) in templates {
        let temp = TempDir::new(&format!("tmpl_{tmpl}"));

        // knr init . --template <tmpl>
        let init_out = knr_bin()
            .args(["init", "--template", tmpl])
            .current_dir(temp.path())
            .output()
            .expect("run knr init");
        assert!(init_out.status.success(), "knr init --template {tmpl} failed: {:?}", init_out);

        // Verify knr.toml exists
        assert!(temp.path().join("knr.toml").is_file());
        assert!(temp.path().join("src/main.knr").is_file());

        // knr check src/main.knr
        let check_out = knr_bin()
            .args(["check", "src/main.knr"])
            .current_dir(temp.path())
            .output()
            .expect("run knr check");
        assert!(check_out.status.success(), "knr check failed for {tmpl}: {:?}", check_out);

        // knr run src/main.knr
        let run_out = knr_bin()
            .args(["run", "src/main.knr"])
            .current_dir(temp.path())
            .output()
            .expect("run knr run");
        assert!(run_out.status.success(), "knr run failed for {tmpl}: {:?}", run_out);
        let stdout = String::from_utf8_lossy(&run_out.stdout);
        assert!(stdout.trim().contains(expected_ret), "Template {tmpl} expected {expected_ret}, got: {stdout}");

        // Attempting to re-init without --force MUST fail
        let reinit_fail = knr_bin()
            .args(["init", "--template", tmpl])
            .current_dir(temp.path())
            .output()
            .expect("run re-init");
        assert!(!reinit_fail.status.success(), "Re-init without --force must fail");

        // Re-init with --force MUST succeed
        let reinit_force = knr_bin()
            .args(["init", "--template", tmpl, "--force"])
            .current_dir(temp.path())
            .output()
            .expect("run re-init --force");
        assert!(reinit_force.status.success(), "Re-init with --force must succeed");
    }
}

#[test]
fn test_adv_cli_new_workspace_lifecycle() {
    let temp = TempDir::new("workspace_parent");
    let ws_path = temp.path().join("my_workspace");

    // knr new <path> --workspace
    let new_out = knr_bin()
        .args(["new", ws_path.to_str().unwrap(), "--workspace"])
        .output()
        .expect("run knr new --workspace");
    assert!(new_out.status.success(), "knr new --workspace failed: {:?}", new_out);

    // Verify workspace layout
    assert!(ws_path.join("knr.toml").is_file());
    assert!(ws_path.join("apps/demo/knr.toml").is_file());
    assert!(ws_path.join("apps/demo/src/main.knr").is_file());

    // Verify workspace knr.toml content
    let ws_toml = fs::read_to_string(ws_path.join("knr.toml")).unwrap();
    assert!(ws_toml.contains("[workspace]"));
    assert!(ws_toml.contains("apps/*"));

    // Check demo package
    let check_out = knr_bin()
        .args(["check", "apps/demo/src/main.knr"])
        .current_dir(&ws_path)
        .output()
        .expect("check demo");
    assert!(check_out.status.success());
}

// =========================================================================
// 3. Disasm Schema Splitting & Verified Re-Check
// =========================================================================

#[test]
fn test_adv_cli_disasm_schema_splitting_and_recheck() {
    let temp = TempDir::new("disasm_test");

    // Source with 2 schemas
    let knr_src = r#"
schema Header {
    0: title str,
    1: version i64,
}

schema Payload {
    0: data str,
    1: count i64,
}

fn main() -> i64 {
    let h = Header {
        title: "Knr",
        version: 1,
    };
    let p = Payload {
        data: "Test",
        count: 42,
    };
    return p.count;
}
"#;
    let knr_file = temp.path().join("main.knr");
    fs::write(&knr_file, knr_src).unwrap();

    let knrbin_file = temp.path().join("main.knrbin");
    let ssa_out_file = temp.path().join("disasm_out/extracted.ssa");
    let schema_expected_file = temp.path().join("disasm_out/extracted.ssa.schema");

    // Step 1: Assemble .knr to .knrbin
    let asm_out = knr_bin()
        .args(["asm", knr_file.to_str().unwrap(), "-o", knrbin_file.to_str().unwrap()])
        .output()
        .expect("run knr asm");
    assert!(asm_out.status.success(), "knr asm failed: {:?}", asm_out);
    assert!(knrbin_file.is_file());

    // Step 2: Disassemble .knrbin to .ssa
    let disasm_out = knr_bin()
        .args(["disasm", knrbin_file.to_str().unwrap(), "-o", ssa_out_file.to_str().unwrap()])
        .output()
        .expect("run knr disasm");
    assert!(disasm_out.status.success(), "knr disasm failed: {:?}", disasm_out);

    // Step 3: Verify schema splitting artifacts
    assert!(ssa_out_file.is_file(), "Disassembled .ssa must exist");
    assert!(schema_expected_file.is_file(), "Accompanying .ssa.schema must exist");

    let ssa_content = fs::read_to_string(&ssa_out_file).unwrap();
    assert!(ssa_content.contains("import \"extracted.ssa.schema\""), "Disassembled .ssa must import schema");
    assert!(!ssa_content.contains("schema Header"), "Disassembled .ssa must not contain inline schema");

    let schema_content = fs::read_to_string(&schema_expected_file).unwrap();
    assert!(schema_content.contains("schema Header"));
    assert!(schema_content.contains("schema Payload"));

    // Step 4: Statically verify disassembled SSA passes knr check without errors!
    let check_out = knr_bin()
        .args(["check", ssa_out_file.to_str().unwrap()])
        .output()
        .expect("check disassembled ssa");
    assert!(check_out.status.success(), "Disassembled SSA must pass knr check cleanly! Stderr: {}", String::from_utf8_lossy(&check_out.stderr));
}

// =========================================================================
// 4. `run --gui` Headless Presentation & Check Diagnostics
// =========================================================================

#[test]
fn test_adv_cli_run_gui_and_diagnostics() {
    let temp = TempDir::new("gui_cli");

    let valid_knr = "fn main() -> i64 { return 42; }";
    let valid_file = temp.path().join("valid.knr");
    fs::write(&valid_file, valid_knr).unwrap();

    // knr run valid.knr --gui
    let run_gui = knr_bin()
        .args(["run", valid_file.to_str().unwrap(), "--gui"])
        .output()
        .expect("run --gui");
    assert!(run_gui.status.success());
    let stdout = String::from_utf8_lossy(&run_gui.stdout);
    assert!(stdout.contains("[GUI] Headless environment detected: rendered off-screen canvas snapshot"));

    // Syntax error .knr: must fail with exit 1 and structured JSON diagnostic
    let bad_knr = "fn main( { return 42; }";
    let bad_file = temp.path().join("bad.knr");
    fs::write(&bad_file, bad_knr).unwrap();

    let check_json = knr_bin()
        .args(["check", bad_file.to_str().unwrap(), "--json"])
        .output()
        .expect("check --json");
    assert!(!check_json.status.success());
    let json_str = String::from_utf8_lossy(&check_json.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&json_str).expect("output must be valid JSON array");
    assert!(parsed.is_array());
    assert!(!parsed.as_array().unwrap().is_empty());

    // Graph exploration: knr check valid.knr --graph
    let check_graph = knr_bin()
        .args(["check", valid_file.to_str().unwrap(), "--graph"])
        .output()
        .expect("check --graph");
    assert!(check_graph.status.success());
    let graph_out = String::from_utf8_lossy(&check_graph.stdout);
    assert!(graph_out.contains("[TOPOLOGY & IMPORTS]") || graph_out.contains("nodes") || graph_out.contains("main"));
}
