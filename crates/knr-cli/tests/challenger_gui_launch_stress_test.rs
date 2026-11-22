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
        let file_name = format!("knr_challenger_gui_{pid}_{id}_{suffix}");
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

// -----------------------------------------------------------------------------
// 1. Complex Control-Flow Graph Stress Tests
// -----------------------------------------------------------------------------

#[test]
fn test_gui_complex_cfg_loop_cycle_ssa() {
    let temp = TempFile::new("loop_cycle.ssa");
    fs::write(
        temp.path(),
        r#"
fn main() -> i64 {
bb0:
    %0 = const_i64 10
    br bb1
bb1:
    %1 = const_i64 20
    %2 = cmp_lt %0 %1
    br_if %2 bb2 bb3
bb2:
    %3 = const_i64 42
    ret %3
bb3:
    br bb1
}
"#,
    )
    .expect("write loop_cycle.ssa");

    let out = knr_cmd()
        .args(["run", temp.path().to_str().unwrap(), "--gui"])
        .output()
        .expect("execute knr run loop_cycle.ssa --gui");

    assert!(
        out.status.success(),
        "Complex loop SSA with cycle edge must run with --gui successfully, stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("[GUI] Headless environment detected: rendered off-screen canvas snapshot"),
        "Must render off-screen snapshot for loop CFG"
    );
}

#[test]
fn test_gui_complex_cfg_nested_branches_and_early_returns() {
    let temp = TempFile::new("multi_ret.ssa");
    fs::write(
        temp.path(),
        r#"
fn main() -> i64 {
bb0:
    %0 = const_i64 42
    %1 = const_i64 100
    %2 = cmp_lt %0 %1
    br_if %2 bb1 bb2
bb1:
    %3 = const_i64 10
    %4 = cmp_lt %3 %0
    br_if %4 bb3 bb4
bb2:
    %5 = const_i64 -1
    ret %5
bb3:
    %6 = const_i64 777
    ret %6
bb4:
    %7 = const_i64 888
    ret %7
}
"#,
    )
    .expect("write multi_ret.ssa");

    let out = knr_cmd()
        .args(["run", temp.path().to_str().unwrap(), "--gui"])
        .output()
        .expect("execute knr run multi_ret.ssa --gui");

    assert!(
        out.status.success(),
        "Nested branches and multiple returns must succeed, stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("[GUI] Headless environment detected: rendered off-screen canvas snapshot"));
}

#[test]
fn test_gui_complex_cfg_while_loop_with_early_return_knr() {
    let temp = TempFile::new("while_early_ret.tsk");
    fs::write(
        temp.path(),
        r#"
export function main(): i64 {
    const target: i64 = 0;
    let i: i64 = 0;
    while (i < 100) {
        if (i === target) {
            return 42;
        }
        i = i + 1;
    }
    return -1;
}
"#,
    )
    .expect("write while_early_ret.tsk");

    let out = knr_cmd()
        .args(["run", temp.path().to_str().unwrap(), "--gui"])
        .output()
        .expect("execute knr run while_early_ret.knr --gui");

    assert!(
        out.status.success(),
        "knr while loop with early return must succeed under --gui, stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("[GUI] Headless environment detected: rendered off-screen canvas snapshot"));
}

#[test]
fn test_gui_complex_cfg_recursive_calls() {
    let temp = TempFile::new("recursive_fact.ssa");
    fs::write(
        temp.path(),
        r#"
fn fact(%0: i64) -> i64 {
bb0:
    %1 = const_i64 1
    %2 = cmp_lt %0 %1
    br_if %2 bb1 bb2
bb1:
    %3 = const_i64 1
    ret %3
bb2:
    %4 = const_i64 1
    %5 = sub %0 %4
    %6 = call fact %5
    %7 = mul %0 %6
    ret %7
}

fn main() -> i64 {
bb0:
    %0 = const_i64 4
    %1 = call fact %0
    ret %1
}
"#,
    )
    .expect("write recursive_fact.ssa");

    let out = knr_cmd()
        .args(["run", temp.path().to_str().unwrap(), "--gui"])
        .output()
        .expect("execute knr run recursive_fact.ssa --gui");

    assert!(
        out.status.success(),
        "Recursive call graph must succeed under --gui, stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("[GUI] Headless environment detected: rendered off-screen canvas snapshot"));
}

#[test]
fn test_gui_complex_cfg_unreachable_block() {
    let temp = TempFile::new("unreachable.ssa");
    fs::write(
        temp.path(),
        r#"
fn main() -> i64 {
bb0:
    %0 = const_i64 123
    ret %0
bb1:
    %1 = const_i64 999
    ret %1
}
"#,
    )
    .expect("write unreachable.ssa");

    let out = knr_cmd()
        .args(["run", temp.path().to_str().unwrap(), "--gui"])
        .output()
        .expect("execute knr run unreachable.ssa --gui");

    assert!(
        out.status.success(),
        "Unreachable block in CFG must not crash graph layout or rendering, stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("[GUI] Headless environment detected: rendered off-screen canvas snapshot"));
}

// -----------------------------------------------------------------------------
// 2. Zero Basic Blocks and Empty Function Edge Cases
// -----------------------------------------------------------------------------

#[test]
fn test_gui_zero_basic_blocks_rejected() {
    let temp = TempFile::new("zero_blocks.ssa");
    fs::write(
        temp.path(),
        r#"
fn no_blocks() -> void {
}
"#,
    )
    .expect("write zero_blocks.ssa");

    let out = knr_cmd()
        .args(["run", temp.path().to_str().unwrap(), "--gui"])
        .output()
        .expect("execute knr run zero_blocks.ssa --gui");

    // Zero basic blocks violates SSA verification (missing entry block bb0)
    assert!(
        !out.status.success(),
        "SSA function with zero basic blocks must fail verification gracefully without panic"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("MissingEntryBlock"),
        "stderr should inform about MissingEntryBlock, got: {stderr}"
    );
}

#[test]
fn test_gui_empty_function_knr() {
    let temp = TempFile::new("empty_fn.tsk");
    fs::write(
        temp.path(),
        r#"
export function main(): void {
}
"#,
    )
    .expect("write empty_fn.tsk");

    let out = knr_cmd()
        .args(["run", temp.path().to_str().unwrap(), "--gui"])
        .output()
        .expect("execute knr run empty_fn.tsk --gui");

    assert!(
        out.status.success(),
        "Empty .tsk function must lower to valid SSA and render GUI, stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("[GUI] Headless environment detected: rendered off-screen canvas snapshot"));
}

#[test]
fn test_gui_file_with_no_functions() {
    let temp = TempFile::new("no_fn.tsk");
    fs::write(
        temp.path(),
        r#"
export type EmptyRecord = {
    id: i64;
};
"#,
    )
    .expect("write no_fn.tsk");

    let out = knr_cmd()
        .args(["run", temp.path().to_str().unwrap(), "--gui"])
        .output()
        .expect("execute knr run no_fn.tsk --gui");

    assert!(
        !out.status.success(),
        "Module with no functions must fail cleanly"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("no functions"),
        "stderr must report no functions to execute, got: {stderr}"
    );
}

// -----------------------------------------------------------------------------
// 3. Extreme Gas Limits Stress Tests
// -----------------------------------------------------------------------------

#[test]
fn test_gui_extreme_gas_zero() {
    let temp = TempFile::new("gas_zero.tsk");
    fs::write(
        temp.path(),
        r#"
export function main(): i64 {
    const x: i64 = 42;
    return x;
}
"#,
    )
    .expect("write gas_zero.tsk");

    let out = knr_cmd()
        .args([
            "run",
            temp.path().to_str().unwrap(),
            "--gui",
            "--gas",
            "0",
        ])
        .output()
        .expect("execute knr run with --gas 0");

    assert!(
        !out.status.success(),
        "--gas 0 must fail due to gas exhaustion"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.to_lowercase().contains("gas") || stderr.contains("exhaust"),
        "Error message must indicate gas exhaustion, got: {stderr}"
    );
}

#[test]
fn test_gui_extreme_gas_one_exhaustion() {
    let temp = TempFile::new("gas_one.tsk");
    fs::write(
        temp.path(),
        r#"
export function main(): i64 {
    const a: i64 = 1;
    const b: i64 = 2;
    return a + b;
}
"#,
    )
    .expect("write gas_one.tsk");

    let out = knr_cmd()
        .args([
            "run",
            temp.path().to_str().unwrap(),
            "--gui",
            "--gas",
            "1",
        ])
        .output()
        .expect("execute knr run with --gas 1");

    assert!(
        !out.status.success(),
        "--gas 1 on multi-step program must exhaust gas"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.to_lowercase().contains("gas") || stderr.contains("exhaust"),
        "Error message must indicate gas exhaustion, got: {stderr}"
    );
}

#[test]
fn test_gui_extreme_gas_one_minimal_program_success() {
    // A function with 0 instructions and 1 terminator (ret %0) takes exactly 1 step.
    // With --gas 1, it should consume 1 gas and succeed!
    let temp = TempFile::new("gas_one_exact.ssa");
    fs::write(
        temp.path(),
        r#"
fn main(%0: i64) -> i64 {
bb0:
    ret %0
}
"#,
    )
    .expect("write gas_one_exact.ssa");

    let out = knr_cmd()
        .args([
            "run",
            temp.path().to_str().unwrap(),
            "--gui",
            "--gas",
            "1",
        ])
        .output()
        .expect("execute knr run with --gas 1 on minimal program");

    assert!(
        out.status.success(),
        "--gas 1 must succeed for single-terminator program, stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("[GUI] Headless environment detected: rendered off-screen canvas snapshot"));
}

#[test]
fn test_gui_unlimited_gas_loop_exhaustion() {
    // cli_loop_gas.ssa is an infinite loop. With --gas 50, it exhausts steps safely without hanging.
    let temp = TempFile::new("loop_gas.ssa");
    fs::write(
        temp.path(),
        r#"
fn main() -> i64 {
bb0:
    %0 = const_i64 0
    br bb1
bb1:
    %1 = const_i64 1
    %2 = add %0 %1
    br bb1
}
"#,
    )
    .expect("write loop_gas.ssa");

    let out = knr_cmd()
        .args([
            "run",
            temp.path().to_str().unwrap(),
            "--gui",
            "--gas",
            "50",
        ])
        .output()
        .expect("execute knr run loop_gas.ssa --gas 50");

    assert!(
        !out.status.success(),
        "Infinite loop with gas 50 must exhaust gas"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("gas"));
}

#[test]
fn test_gui_unlimited_gas_with_recursion() {
    // Tests --unlimited-gas on a recursive function
    let temp = TempFile::new("unlimited_fact.ssa");
    fs::write(
        temp.path(),
        r#"
fn fact(%0: i64) -> i64 {
bb0:
    %1 = const_i64 1
    %2 = cmp_lt %0 %1
    br_if %2 bb1 bb2
bb1:
    %3 = const_i64 1
    ret %3
bb2:
    %4 = const_i64 1
    %5 = sub %0 %4
    %6 = call fact %5
    %7 = mul %0 %6
    ret %7
}

fn main() -> i64 {
bb0:
    %0 = const_i64 6
    %1 = call fact %0
    ret %1
}
"#,
    )
    .expect("write unlimited_fact.ssa");

    let out = knr_cmd()
        .args([
            "run",
            temp.path().to_str().unwrap(),
            "--gui",
            "--unlimited-gas",
        ])
        .output()
        .expect("execute knr run with --unlimited-gas");

    assert!(
        out.status.success(),
        "--unlimited-gas must allow execution and GUI rendering, stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("[GUI] Headless environment detected: rendered off-screen canvas snapshot"));
}

// -----------------------------------------------------------------------------
// 4. Simultaneous --gui and --trace
// -----------------------------------------------------------------------------

#[test]
fn test_gui_simultaneous_trace_output() {
    let temp = TempFile::new("simul_trace.tsk");
    fs::write(
        temp.path(),
        r#"
export function main(): i64 {
    const x: i64 = 10;
    const y: i64 = 20;
    const z: i64 = x + y;
    return z;
}
"#,
    )
    .expect("write simul_trace.tsk");

    let out = knr_cmd()
        .args([
            "run",
            temp.path().to_str().unwrap(),
            "--gui",
            "--trace",
        ])
        .output()
        .expect("execute knr run with --gui --trace");

    assert!(
        out.status.success(),
        "--gui --trace must succeed, stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("[GUI] Headless environment detected: rendered off-screen canvas snapshot"),
        "stdout must contain headless snapshot notice"
    );

    let json_start = stdout.find("\n[").or_else(|| stdout.find('[')).expect("must contain JSON array");
    let json_text = stdout[json_start..].trim();
    let trace_value: serde_json::Value = serde_json::from_str(json_text)
        .expect("trace output must be valid parseable JSON");

    assert!(trace_value.is_array());
    let arr = trace_value.as_array().unwrap();
    assert!(arr.len() >= 3, "trace should have records for const x, y, z, return");
    let first = &arr[0];
    assert!(first.get("step").is_some());
    assert!(first.get("opcode").is_some());
}

#[test]
fn test_gui_short_flag_g_with_trace() {
    let temp = TempFile::new("short_g_trace.tsk");
    fs::write(
        temp.path(),
        r#"
export function main(): i64 {
    return 999;
}
"#,
    )
    .expect("write short_g_trace.tsk");

    let out = knr_cmd()
        .args([
            "run",
            temp.path().to_str().unwrap(),
            "-g",
            "--trace",
        ])
        .output()
        .expect("execute knr run with -g --trace");

    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("[GUI] Headless environment detected: rendered off-screen canvas snapshot"));
    let json_start = stdout.find("\n[").or_else(|| stdout.find('[')).expect("must contain JSON array");
    let json_text = stdout[json_start..].trim();
    let trace_value: serde_json::Value = serde_json::from_str(json_text).unwrap();
    assert!(trace_value.is_array());
}

// -----------------------------------------------------------------------------
// 5. Fallback to First Function (No main)
// -----------------------------------------------------------------------------

#[test]
fn test_gui_fallback_single_function_no_main() {
    let temp = TempFile::new("no_main_single.tsk");
    fs::write(
        temp.path(),
        r#"
export function calculate_metric(factor: i64): i64 {
    const base: i64 = 100;
    return base + factor;
}
"#,
    )
    .expect("write no_main_single.tsk");

    let out = knr_cmd()
        .args(["run", temp.path().to_str().unwrap(), "--gui"])
        .output()
        .expect("execute knr run no_main_single.tsk --gui");

    assert!(
        out.status.success(),
        "Fallback to first function without main must succeed under --gui, stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("[GUI] Headless environment detected: rendered off-screen canvas snapshot"));
}

#[test]
fn test_gui_fallback_multiple_functions_no_main() {
    let temp = TempFile::new("no_main_multi.tsk");
    fs::write(
        temp.path(),
        r#"
export function first_entry(x: i64): i64 {
    const doubled: i64 = x * 2;
    return doubled;
}

export function secondary_fn(y: i64): i64 {
    return y + 10;
}
"#,
    )
    .expect("write no_main_multi.tsk");

    let out = knr_cmd()
        .args(["run", temp.path().to_str().unwrap(), "--gui"])
        .output()
        .expect("execute knr run no_main_multi.tsk --gui");

    assert!(
        out.status.success(),
        "Fallback with multiple functions must execute first function and add second to graph, stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("[GUI] Headless environment detected: rendered off-screen canvas snapshot"));
}
