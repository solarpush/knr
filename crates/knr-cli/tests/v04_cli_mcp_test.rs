use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};

use knr_cli::commands::mcp::handle_mcp_message;
use serde_json::{json, Value};

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(suffix: &str) -> Self {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let id = COUNTER.fetch_add(1, Ordering::SeqCst);
        let pid = std::process::id();
        let path = std::env::temp_dir().join(format!("knr_v04_test_{pid}_{id}_{suffix}"));
        let _ = fs::remove_dir_all(&path);
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
    cmd.env("KNR_HEADLESS", "1");
    cmd
}

fn repo_root() -> PathBuf {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest_dir.parent().unwrap().parent().unwrap().to_path_buf()
}

fn example_path(name: &str) -> PathBuf {
    repo_root().join("examples").join(name)
}

// ============================================================================
// 1. Native MCP Server — Unit Dispatcher Tests
// ============================================================================

#[test]
fn test_mcp_unit_lifecycle_and_tools_list() {
    let init_resp = handle_mcp_message(&json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": { "name": "test-client", "version": "1.0.0" }
        }
    }))
    .expect("initialize must return a response");

    assert_eq!(init_resp["jsonrpc"], "2.0");
    assert_eq!(init_resp["id"], 1);
    assert_eq!(init_resp["result"]["protocolVersion"], "2024-11-05");
    assert_eq!(init_resp["result"]["serverInfo"]["name"], "knr-mcp");
    assert!(init_resp["result"]["capabilities"]["tools"].is_object());

    // Notifications must return None
    assert!(handle_mcp_message(&json!({
        "jsonrpc": "2.0",
        "method": "notifications/initialized"
    }))
    .is_none());
    assert!(handle_mcp_message(&json!({
        "jsonrpc": "2.0",
        "method": "initialized"
    }))
    .is_none());

    // Ping
    let ping_resp = handle_mcp_message(&json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "ping"
    }))
    .expect("ping must return a response");
    assert_eq!(ping_resp["result"], json!({}));

    // tools/list returns all 8 tools with inputSchema
    let list_resp = handle_mcp_message(&json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "tools/list"
    }))
    .expect("tools/list must return a response");
    let tools = list_resp["result"]["tools"]
        .as_array()
        .expect("tools must be an array");
    assert_eq!(tools.len(), 8, "Expected exactly 8 MCP tools");

    let expected = [
        "knr_check",
        "knr_explore",
        "knr_test",
        "knr_render_audit",
        "knr_fmt",
        "knr_fix",
        "knr_bench",
        "knr_get_skill",
    ];
    for name in expected {
        let tool = tools
            .iter()
            .find(|t| t["name"] == name)
            .unwrap_or_else(|| panic!("Missing MCP tool '{name}'"));
        assert!(
            tool["inputSchema"]["type"] == "object",
            "Tool '{name}' must have object inputSchema"
        );
    }

    // Bad path & unknown tool return clean isError: true without panicking
    let bad_file = handle_mcp_message(&json!({
        "jsonrpc": "2.0",
        "id": 4,
        "method": "tools/call",
        "params": {
            "name": "knr_check",
            "arguments": { "file": "/nonexistent/path/definitely_missing.tsk" }
        }
    }))
    .expect("bad path must return response");
    assert_eq!(bad_file["result"]["isError"], true);

    let bad_tool = handle_mcp_message(&json!({
        "jsonrpc": "2.0",
        "id": 5,
        "method": "tools/call",
        "params": {
            "name": "knr_does_not_exist",
            "arguments": {}
        }
    }))
    .expect("unknown tool must return response");
    assert_eq!(bad_tool["result"]["isError"], true);
}

#[test]
fn test_mcp_unit_knr_fix_dry_run_and_write() {
    let temp = TempDir::new("mcp_fix");
    let file = temp.path().join("needs_fix.tsk");
    // Missing `std/ui` import — auto-fixable via `compute_all_quickfixes_for_source`
    fs::write(
        &file,
        r#"export function view(): Widget {
    return <Column p={16}><Text>Hello</Text></Column>;
}
"#,
    )
    .unwrap();

    let resp = handle_mcp_message(&json!({
        "jsonrpc": "2.0",
        "id": 10,
        "method": "tools/call",
        "params": {
            "name": "knr_fix",
            "arguments": { "file": file.to_str().unwrap(), "write": true }
        }
    }))
    .expect("knr_fix must return response");
    assert_eq!(resp["result"]["isError"], false);
    let text = resp["result"]["content"][0]["text"].as_str().unwrap();
    let payload: Value = serde_json::from_str(text).expect("knr_fix JSON");
    assert_eq!(payload["ok"], true);
}

// ============================================================================
// 2. Native MCP Server — Real Child-Process Stdio (Line-Delimited & Framed)
// ============================================================================

#[test]
fn test_mcp_child_process_stdio_line_delimited_all_tools() {
    let task_studio = example_path("task_studio.tsk");
    let task_str = task_studio.to_str().unwrap();
    let temp = TempDir::new("mcp_stdio");
    let out_png = temp.path().join("mcp_shot.png");

    let mut child = knr_cmd()
        .arg("mcp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn knr mcp");

    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let mut reader = BufReader::new(stdout);

    let send_and_recv = |stdin: &mut std::process::ChildStdin,
                         reader: &mut BufReader<std::process::ChildStdout>,
                         msg: Value|
     -> Value {
        let line = serde_json::to_string(&msg).unwrap();
        writeln!(stdin, "{line}").unwrap();
        stdin.flush().unwrap();
        let mut resp_line = String::new();
        let n = reader.read_line(&mut resp_line).unwrap();
        assert!(n > 0, "Expected JSON-RPC response line from knr mcp");
        serde_json::from_str(resp_line.trim())
            .unwrap_or_else(|e| panic!("Invalid JSON from knr mcp '{resp_line}': {e}"))
    };

    // 1. initialize
    let init = send_and_recv(
        &mut stdin,
        &mut reader,
        json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {} }),
    );
    assert_eq!(init["result"]["protocolVersion"], "2024-11-05");
    assert_eq!(init["result"]["serverInfo"]["name"], "knr-mcp");

    // 2. notifications/initialized (no response)
    writeln!(
        stdin,
        "{}",
        json!({ "jsonrpc": "2.0", "method": "notifications/initialized" })
    )
    .unwrap();
    stdin.flush().unwrap();

    // 3. tools/list
    let list = send_and_recv(
        &mut stdin,
        &mut reader,
        json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }),
    );
    assert_eq!(list["result"]["tools"].as_array().unwrap().len(), 8);

    // 4. knr_check
    let chk = send_and_recv(
        &mut stdin,
        &mut reader,
        json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tools/call",
            "params": { "name": "knr_check", "arguments": { "file": task_str } }
        }),
    );
    assert_eq!(chk["result"]["isError"], false);
    let chk_body: Value =
        serde_json::from_str(chk["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(chk_body["ok"], true);
    assert_eq!(chk_body["diagnostics"].as_array().unwrap().len(), 0);
    assert!(chk_body["shader_count"].as_u64().unwrap() >= 1);

    // 5. knr_explore
    let exp = send_and_recv(
        &mut stdin,
        &mut reader,
        json!({
            "jsonrpc": "2.0",
            "id": 4,
            "method": "tools/call",
            "params": { "name": "knr_explore", "arguments": { "path": task_str, "format": "json" } }
        }),
    );
    assert_eq!(exp["result"]["isError"], false);
    let exp_text = exp["result"]["content"][0]["text"].as_str().unwrap();
    let exp_json: Value = serde_json::from_str(exp_text).expect("explore json");
    assert!(exp_json.is_object());

    // 6. knr_test
    let tst = send_and_recv(
        &mut stdin,
        &mut reader,
        json!({
            "jsonrpc": "2.0",
            "id": 5,
            "method": "tools/call",
            "params": { "name": "knr_test", "arguments": { "path": task_str } }
        }),
    );
    assert_eq!(tst["result"]["isError"], false);
    let tst_body: Value =
        serde_json::from_str(tst["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(tst_body["status"], "ok");
    assert!(tst_body["passed"].as_u64().unwrap() >= 2);
    assert_eq!(tst_body["failed"], 0);

    // 7. knr_render_audit with inputs + clicks + include_png_base64
    let rnd = send_and_recv(
        &mut stdin,
        &mut reader,
        json!({
            "jsonrpc": "2.0",
            "id": 6,
            "method": "tools/call",
            "params": {
                "name": "knr_render_audit",
                "arguments": {
                    "file": task_str,
                    "width": 1280,
                    "height": 860,
                    "scale": 1.0,
                    "output": out_png.to_str().unwrap(),
                    "inputs": { "task_draft_input": "MCP Audited Task" },
                    "clicks": ["btn_add_task"],
                    "include_png_base64": true
                }
            }
        }),
    );
    assert_eq!(rnd["result"]["isError"], false);
    let content = rnd["result"]["content"].as_array().unwrap();
    assert_eq!(content.len(), 2, "Expected text + image content blocks");
    let rnd_body: Value = serde_json::from_str(content[0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(rnd_body["error_count"], 0);
    assert_eq!(content[1]["type"], "image");
    assert_eq!(content[1]["mimeType"], "image/png");
    assert!(
        content[1]["data"]
            .as_str()
            .unwrap()
            .starts_with("iVBORw0KGgo"),
        "Base64 PNG must start with PNG magic"
    );
    assert!(out_png.is_file(), "output PNG should be written to disk");

    // 8. knr_fmt
    let fmt = send_and_recv(
        &mut stdin,
        &mut reader,
        json!({
            "jsonrpc": "2.0",
            "id": 7,
            "method": "tools/call",
            "params": { "name": "knr_fmt", "arguments": { "file": task_str, "write": false } }
        }),
    );
    assert_eq!(fmt["result"]["isError"], false);
    let fmt_body: Value =
        serde_json::from_str(fmt["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(fmt_body["ok"], true);

    // 9. knr_bench
    let bnc = send_and_recv(
        &mut stdin,
        &mut reader,
        json!({
            "jsonrpc": "2.0",
            "id": 8,
            "method": "tools/call",
            "params": { "name": "knr_bench", "arguments": { "file": task_str, "iters": 5 } }
        }),
    );
    assert_eq!(bnc["result"]["isError"], false);
    let bnc_body: Value =
        serde_json::from_str(bnc["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert!(bnc_body["vm"]["steps"].as_u64().unwrap() > 0);

    // 10. knr_get_skill
    let skl = send_and_recv(
        &mut stdin,
        &mut reader,
        json!({
            "jsonrpc": "2.0",
            "id": 9,
            "method": "tools/call",
            "params": { "name": "knr_get_skill", "arguments": {} }
        }),
    );
    assert_eq!(skl["result"]["isError"], false);
    let skl_text = skl["result"]["content"][0]["text"].as_str().unwrap();
    assert!(skl_text.contains("knr check"));

    drop(stdin);
    let status = child.wait().expect("wait knr mcp");
    assert!(status.success(), "knr mcp should exit cleanly on EOF");
}

#[test]
fn test_mcp_child_process_content_length_framing() {
    let mut child = knr_cmd()
        .arg("mcp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn knr mcp");

    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let mut reader = BufReader::new(stdout);

    let req = serde_json::to_string(&json!({
        "jsonrpc": "2.0",
        "id": 42,
        "method": "ping"
    }))
    .unwrap();
    write!(stdin, "Content-Length: {}\r\n\r\n{}", req.len(), req).unwrap();
    stdin.flush().unwrap();

    // Expect Content-Length framed response back
    let mut header = String::new();
    reader.read_line(&mut header).unwrap();
    assert!(
        header.to_ascii_lowercase().starts_with("content-length:"),
        "Expected Content-Length header, got: {header:?}"
    );
    let len: usize = header.split_once(':').unwrap().1.trim().parse().unwrap();
    let mut blank = String::new();
    reader.read_line(&mut blank).unwrap();
    assert!(blank.trim().is_empty());

    let mut body = vec![0u8; len];
    reader.read_exact(&mut body).unwrap();
    let resp: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(resp["id"], 42);
    assert_eq!(resp["result"], json!({}));

    drop(stdin);
    let status = child.wait().unwrap();
    assert!(status.success());
}

// ============================================================================
// 3. Environment & Project Health Check (`knr doctor`)
// ============================================================================

#[test]
fn test_doctor_json_and_invalid_manifest() {
    let out = knr_cmd()
        .args(["doctor", "--json"])
        .output()
        .expect("run knr doctor --json");
    assert!(
        out.status.success(),
        "knr doctor --json must succeed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let stdout = String::from_utf8_lossy(&out.stdout);
    let report: Value = serde_json::from_str(stdout.trim()).expect("valid doctor JSON");
    assert!(report["version"].is_string());
    assert!(report["executable"].is_string());
    let checks = report["checks"].as_array().expect("checks array");
    let names: Vec<&str> = checks
        .iter()
        .filter_map(|c| c["name"].as_str())
        .collect();
    for required in [
        "knr_cli",
        "c_linker",
        "runtime_staticlib",
        "display_server",
        "nodejs",
        "vscode_extension",
        "project_manifest",
    ] {
        assert!(
            names.contains(&required),
            "knr doctor missing check '{required}': {names:?}"
        );
    }

    // Invalid knr.toml must produce error status and non-zero exit
    let bad_dir = TempDir::new("doctor_bad_toml");
    fs::write(bad_dir.path().join("knr.toml"), "not valid = [ toml").unwrap();
    let bad_out = knr_cmd()
        .args(["doctor", bad_dir.path().to_str().unwrap(), "--json"])
        .output()
        .expect("run knr doctor on invalid manifest");
    assert!(!bad_out.status.success(), "Invalid knr.toml must exit non-zero");
    let bad_report: Value =
        serde_json::from_str(String::from_utf8_lossy(&bad_out.stdout).trim()).unwrap();
    assert_eq!(bad_report["status"], "error");
    assert!(bad_report["error_count"].as_u64().unwrap() >= 1);
}

// ============================================================================
// 4. Deterministic Benchmark Tool (`knr bench`)
// ============================================================================

#[test]
fn test_bench_task_studio_and_pure_numeric_jit() {
    let task_studio = example_path("task_studio.tsk");
    let out = knr_cmd()
        .args(["bench", task_studio.to_str().unwrap(), "--iters", "5", "--json"])
        .output()
        .expect("run knr bench");
    assert!(
        out.status.success(),
        "knr bench task_studio failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let report: Value =
        serde_json::from_str(String::from_utf8_lossy(&out.stdout).trim()).expect("bench JSON");
    assert_eq!(report["target"], "view(init())");
    assert_eq!(report["iters"], 5);
    assert!(report["vm"]["steps"].as_u64().unwrap() > 0);
    assert!(report["vm"]["min_us"].as_f64().unwrap() <= report["vm"]["max_us"].as_f64().unwrap());
    assert!(report["sizes"]["knrbin_bytes"].as_u64().unwrap() > 0);
    assert!(report["sizes"]["wasm_bytes"].as_u64().unwrap() > 0);

    // Pure numeric 0-arg benchmark should run BOTH VM and Cranelift JIT
    let temp = TempDir::new("bench_numeric");
    let num_file = temp.path().join("sum.tsk");
    fs::write(
        &num_file,
        r#"export function main(): i64 {
    let acc: i64 = 0;
    for (let i: i64 = 0; i < 200; i += 1) {
        acc = acc + i;
    }
    return acc;
}
"#,
    )
    .unwrap();

    let num_out = knr_cmd()
        .args(["bench", num_file.to_str().unwrap(), "--iters", "10", "--json"])
        .output()
        .expect("run knr bench numeric");
    assert!(
        num_out.status.success(),
        "knr bench numeric failed: {}",
        String::from_utf8_lossy(&num_out.stderr)
    );
    let num_report: Value =
        serde_json::from_str(String::from_utf8_lossy(&num_out.stdout).trim()).unwrap();
    assert_eq!(num_report["target"], "main()");
    assert!(num_report["vm"]["steps"].as_u64().unwrap() > 200);
    if num_report["jit"].is_object() {
        assert!(num_report["jit"]["compile_ms"].as_f64().unwrap() >= 0.0);
        assert!(num_report["jit"]["mean_ns"].as_f64().unwrap() >= 0.0);
    } else {
        // In W^X-enforced sandboxes where mprotect(RX) is disallowed, bench must fall back gracefully
        let note = num_report["jit_note"]
            .as_str()
            .expect("Expected jit metrics or graceful jit_note fallback");
        assert!(
            note.contains("Cranelift"),
            "Unexpected jit_note fallback: {note}"
        );
    }
}

// ============================================================================
// 5. Upgraded `knr render`: --viewports, --diff-out, and --flow E2E Runner
// ============================================================================

#[test]
fn test_render_multi_viewport_presets_and_custom() {
    let task_studio = example_path("task_studio.tsk");
    let temp = TempDir::new("render_vp");
    let out_base = temp.path().join("shot.png");

    let out = knr_cmd()
        .args([
            "render",
            task_studio.to_str().unwrap(),
            "--viewports",
            "mobile,desktop",
            "--scale",
            "1.0",
            "-o",
            out_base.to_str().unwrap(),
            "--json",
        ])
        .output()
        .expect("knr render --viewports");
    assert!(
        out.status.success(),
        "knr render --viewports failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let report: Value =
        serde_json::from_str(String::from_utf8_lossy(&out.stdout).trim()).expect("vp JSON");
    assert_eq!(report["total_viewports"], 2);
    let vps = report["viewports"].as_array().unwrap();
    assert_eq!(vps[0]["viewport"], "mobile");
    assert_eq!(vps[0]["width"], 390);
    assert_eq!(vps[0]["height"], 844);
    assert_eq!(vps[1]["viewport"], "desktop");
    assert_eq!(vps[1]["width"], 1280);
    assert_eq!(vps[1]["height"], 900);

    assert!(temp.path().join("shot_mobile.png").is_file());
    assert!(temp.path().join("shot_desktop.png").is_file());

    // Custom WxH viewports
    let custom_out = knr_cmd()
        .args([
            "render",
            task_studio.to_str().unwrap(),
            "--viewports",
            "768x1024,1440x900",
            "--scale",
            "1.0",
            "--json",
        ])
        .output()
        .expect("knr render custom viewports");
    assert!(custom_out.status.success());
    let custom_report: Value =
        serde_json::from_str(String::from_utf8_lossy(&custom_out.stdout).trim()).unwrap();
    assert_eq!(custom_report["viewports"][0]["width"], 768);
    assert_eq!(custom_report["viewports"][1]["width"], 1440);
}

#[test]
fn test_render_baseline_visual_diff_png() {
    let task_studio = example_path("task_studio.tsk");
    let temp = TempDir::new("render_diff");
    let base_png = temp.path().join("baseline.png");
    let actual_png = temp.path().join("actual.png");
    let diff_png = temp.path().join("diff.png");

    // 1. Render baseline frame
    let r1 = knr_cmd()
        .args([
            "render",
            task_studio.to_str().unwrap(),
            "--width",
            "1280",
            "--height",
            "860",
            "--scale",
            "1.0",
            "-o",
            base_png.to_str().unwrap(),
        ])
        .output()
        .expect("render baseline");
    assert!(r1.status.success());
    assert!(base_png.is_file());

    // 2. Render changed state with --baseline and --diff-out
    let r2 = knr_cmd()
        .args([
            "render",
            task_studio.to_str().unwrap(),
            "--width",
            "1280",
            "--height",
            "860",
            "--scale",
            "1.0",
            "--input",
            "task_draft_input=Visual Diff Trigger",
            "--click",
            "btn_add_task",
            "-o",
            actual_png.to_str().unwrap(),
            "--baseline",
            base_png.to_str().unwrap(),
            "--diff-out",
            diff_png.to_str().unwrap(),
            "--max-diff",
            "100.0",
            "--json",
        ])
        .output()
        .expect("render diff");
    assert!(
        r2.status.success(),
        "render with --diff-out failed: {}",
        String::from_utf8_lossy(&r2.stderr)
    );

    let report: Value =
        serde_json::from_str(String::from_utf8_lossy(&r2.stdout).trim()).expect("diff JSON");
    assert!(report["baseline_diff_pct"].as_f64().unwrap() > 0.0);
    assert_eq!(report["diff_png"], diff_png.to_str().unwrap());
    assert!(diff_png.is_file(), "Visual diff PNG must be generated");

    // Inspect generated 3-panel diff PNG
    let base_pm = tiny_skia::Pixmap::load_png(&base_png).unwrap();
    let diff_pm = tiny_skia::Pixmap::load_png(&diff_png).unwrap();
    assert!(
        diff_pm.width() >= base_pm.width() * 3,
        "Diff PNG should be 3-panel side-by-side"
    );
    assert_eq!(diff_pm.height(), base_pm.height());
    let has_magenta = diff_pm
        .data()
        .as_chunks::<4>()
        .0
        .iter()
        .any(|px| px[0] == 255 && px[1] == 0 && px[2] == 220 && px[3] == 255);
    assert!(
        has_magenta,
        "Diff PNG must highlight changed pixels in bright magenta"
    );
}

#[test]
fn test_render_declarative_flow_pass_and_fail() {
    let task_studio = example_path("task_studio.tsk");
    let temp = TempDir::new("render_flow");
    let snap_png = temp.path().join("step1.png");
    let pass_flow_path = temp.path().join("pass_flow.json");
    let fail_flow_path = temp.path().join("fail_flow.json");

    let pass_flow = json!({
        "width": 1280,
        "height": 860,
        "scale": 1.0,
        "steps": [
            { "type": "input", "key": "task_draft_input", "value": "Ship v0.4" },
            { "type": "click", "key": "btn_add_task" },
            { "type": "assert_text", "contains": "Ship v0.4" },
            { "type": "assert_key", "key": "btn_add_task" },
            { "type": "assert_audit", "max_errors": 0 },
            { "type": "snapshot", "output": snap_png.to_str().unwrap() }
        ]
    });
    fs::write(&pass_flow_path, serde_json::to_string_pretty(&pass_flow).unwrap()).unwrap();

    let pass_out = knr_cmd()
        .args([
            "render",
            task_studio.to_str().unwrap(),
            "--flow",
            pass_flow_path.to_str().unwrap(),
            "--json",
        ])
        .output()
        .expect("run passing flow");
    assert!(
        pass_out.status.success(),
        "Passing flow must exit 0: {}\n{}",
        String::from_utf8_lossy(&pass_out.stdout),
        String::from_utf8_lossy(&pass_out.stderr)
    );
    let pass_json: Value =
        serde_json::from_str(String::from_utf8_lossy(&pass_out.stdout).trim()).unwrap();
    assert_eq!(pass_json["status"], "pass");
    assert_eq!(pass_json["total_steps"], 6);
    assert_eq!(pass_json["passed_steps"], 6);
    assert_eq!(pass_json["failed_steps"], 0);
    assert!(snap_png.is_file(), "Snapshot step must write PNG");

    // Failing flow assertion must exit non-zero and report failure clearly
    let fail_flow = json!({
        "width": 1280,
        "height": 860,
        "scale": 1.0,
        "steps": [
            { "type": "assert_text", "contains": "THIS_STRING_IS_NEVER_IN_TASK_STUDIO" }
        ]
    });
    fs::write(&fail_flow_path, serde_json::to_string_pretty(&fail_flow).unwrap()).unwrap();

    let fail_out = knr_cmd()
        .args([
            "render",
            task_studio.to_str().unwrap(),
            "--flow",
            fail_flow_path.to_str().unwrap(),
            "--json",
        ])
        .output()
        .expect("run failing flow");
    assert!(
        !fail_out.status.success(),
        "Failing flow assertion must exit non-zero"
    );
    let fail_json: Value =
        serde_json::from_str(String::from_utf8_lossy(&fail_out.stdout).trim()).unwrap();
    assert_eq!(fail_json["status"], "fail");
    assert_eq!(fail_json["failed_steps"], 1);
}

// ============================================================================
// 6. Upgraded `knr new` Modern `.tsk` Templates (`app`, `dashboard`, `shader`)
// ============================================================================

#[test]
fn test_new_modern_templates_check_test_and_render_audit() {
    for tmpl in ["app", "dashboard", "shader"] {
        let temp = TempDir::new(&format!("tmpl_{tmpl}"));
        let project_dir = temp.path().join(format!("my_{tmpl}"));

        let new_out = knr_cmd()
            .args([
                "new",
                project_dir.to_str().unwrap(),
                "--template",
                tmpl,
            ])
            .output()
            .expect("knr new");
        assert!(
            new_out.status.success(),
            "knr new --template {tmpl} failed: {}",
            String::from_utf8_lossy(&new_out.stderr)
        );

        assert!(project_dir.join("knr.toml").is_file());
        let app_entry = project_dir.join("src/app.tsk");
        assert!(app_entry.is_file(), "src/app.tsk must exist for template {tmpl}");

        if tmpl == "app" {
            assert!(project_dir.join("src/state.tsk").is_file());
            assert!(project_dir.join("src/shaders.tsk").is_file());
            assert!(project_dir.join("src/main.tsk").is_file());
        }

        // 1. knr check
        let chk_out = knr_cmd()
            .args(["check", app_entry.to_str().unwrap()])
            .output()
            .expect("knr check template");
        assert!(
            chk_out.status.success(),
            "knr check failed for template {tmpl}: {}",
            String::from_utf8_lossy(&chk_out.stderr)
        );

        // 2. knr test (entire project directory)
        let tst_out = knr_cmd()
            .args(["test", project_dir.to_str().unwrap(), "--json"])
            .output()
            .expect("knr test template");
        assert!(
            tst_out.status.success(),
            "knr test failed for template {tmpl}: {}\n{}",
            String::from_utf8_lossy(&tst_out.stdout),
            String::from_utf8_lossy(&tst_out.stderr)
        );
        let tst_json: Value =
            serde_json::from_str(String::from_utf8_lossy(&tst_out.stdout).trim()).unwrap();
        assert_eq!(tst_json["status"], "ok");
        assert!(
            tst_json["passed"].as_u64().unwrap() >= 1,
            "Template {tmpl} must ship at least one passing test_*"
        );

        // 3. knr render --assert-audit (zero audit errors!)
        let rnd_out = knr_cmd()
            .args([
                "render",
                app_entry.to_str().unwrap(),
                "--width",
                "1280",
                "--height",
                "860",
                "--scale",
                "1.0",
                "--assert-audit",
                "--json",
            ])
            .output()
            .expect("knr render template");
        assert!(
            rnd_out.status.success(),
            "knr render --assert-audit failed for template {tmpl}: {}\n{}",
            String::from_utf8_lossy(&rnd_out.stdout),
            String::from_utf8_lossy(&rnd_out.stderr)
        );
    }
}
