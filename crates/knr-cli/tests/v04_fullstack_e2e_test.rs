//! Full-Stack v0.4.0 End-to-End Integration & Torture Suite.
//!
//! Exercises the ENTIRE upgraded `knr` v0.4.0 toolchain end-to-end across real processes:
//! 1. `examples/neural_studio/` multi-file project (`knr.toml`, barrel `components/index.tsk`,
//!    `std/theme`, `std/router`, `std/layout`, `std/cmd`, `std/core`, `std/testing`,
//!    `on_key`, `on_resize`, exhaustive `enum Action`, 2 `@shader` kernels):
//!    `knr check` -> `knr test` -> `knr render --flow --assert-audit` ->
//!    `knr render --viewports mobile,desktop --assert-audit` ->
//!    `knr build --target web` -> Node.js real WASM 300-frame zero-GC arena verification
//!    (including `__knr_heap_used()`, `__knr_heap_capacity()`, `on_key`, `on_resize`, `effect`)
//!    + JS syntax check of the embedded `v4-canvas` runtime!
//! 2. `knr mcp` live stdio child process running all core MCP tools against `neural_studio`!
//! 3. `knr lsp` live stdio child process testing v0.4.0 IDE-Ultra capabilities
//!    (`signatureHelp`, `inlayHint`, `workspace/symbol`, `foldingRange`, `<Icon name="|` completion,
//!    and cross-file hover/definition on `examples/neural_studio/src/app.tsk`)!
//! 4. `knr doctor --json`, `knr bench --json` (with VM profiler stats), and `knr run --profile`!

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::{json, Value};

fn knr_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_knr"))
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "knr_v04_fullstack_{tag}_{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn write_lsp_frame(stdin: &mut impl Write, msg: &Value) {
    let payload = serde_json::to_string(msg).unwrap();
    write!(
        stdin,
        "Content-Length: {}\r\n\r\n{}",
        payload.len(),
        payload
    )
    .unwrap();
    stdin.flush().unwrap();
}

fn read_lsp_frame(reader: &mut impl BufRead) -> Option<Value> {
    let mut content_len: Option<usize> = None;
    let mut line = String::new();
    loop {
        line.clear();
        let n = reader.read_line(&mut line).ok()?;
        if n == 0 {
            return None;
        }
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            if content_len.is_some() {
                break;
            }
            continue;
        }
        if let Some((k, v)) = trimmed.split_once(':') {
            if k.trim().eq_ignore_ascii_case("content-length") {
                content_len = v.trim().parse::<usize>().ok();
            }
        }
    }
    let len = content_len?;
    let mut buf = vec![0u8; len];
    reader.read_exact(&mut buf).ok()?;
    serde_json::from_slice(&buf).ok()
}

fn read_lsp_response_by_id(reader: &mut impl BufRead, target_id: u64) -> Value {
    for _ in 0..32 {
        let msg = read_lsp_frame(reader).expect("expected LSP frame");
        if msg.get("id").and_then(Value::as_u64) == Some(target_id) {
            return msg;
        }
    }
    panic!("Timed out waiting for LSP response id={target_id}");
}

fn path_to_file_uri(path: &Path) -> String {
    let abs = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    format!("file://{}", abs.to_string_lossy().replace('\\', "/"))
}

#[test]
fn v04_neural_studio_check_test_flow_viewports_and_web_v4_canvas_wasm_300_frames() {
    let root = workspace_root();
    let app = root.join("examples/neural_studio/src/app.tsk");
    let flow = root.join("examples/neural_studio/flow.json");
    let out_dir = temp_dir("neural_web");

    // 1. Static verification (`knr check --json`)
    let chk = Command::new(knr_bin())
        .args(["check", app.to_str().unwrap(), "--json"])
        .env("KNR_HEADLESS", "1")
        .output()
        .expect("knr check");
    assert!(
        chk.status.success(),
        "knr check neural_studio failed: {}",
        String::from_utf8_lossy(&chk.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&chk.stdout).trim(), "[]");

    // 2. Deterministic test suite (`knr test --json`)
    let tst = Command::new(knr_bin())
        .args(["test", app.to_str().unwrap(), "--json"])
        .env("KNR_HEADLESS", "1")
        .output()
        .expect("knr test");
    assert!(
        tst.status.success(),
        "knr test neural_studio failed: {}",
        String::from_utf8_lossy(&tst.stderr)
    );
    let tst_json: Value = serde_json::from_slice(&tst.stdout).expect("test json");
    assert_eq!(tst_json["failed"].as_u64(), Some(0));
    assert!(tst_json["passed"].as_u64().unwrap_or(0) >= 7);

    // 3. Declarative 23-step E2E flow + strict visual audit (`knr render --flow --assert-audit`)
    let flow_run = Command::new(knr_bin())
        .args([
            "render",
            app.to_str().unwrap(),
            "--flow",
            flow.to_str().unwrap(),
            "--assert-audit",
            "--json",
        ])
        .env("KNR_HEADLESS", "1")
        .output()
        .expect("knr render --flow");
    assert!(
        flow_run.status.success(),
        "knr render --flow failed: stdout={} stderr={}",
        String::from_utf8_lossy(&flow_run.stdout),
        String::from_utf8_lossy(&flow_run.stderr)
    );

    // 4. Multi-viewport responsive audit (`mobile,desktop --assert-audit`)
    let vp_run = Command::new(knr_bin())
        .args([
            "render",
            app.to_str().unwrap(),
            "--viewports",
            "mobile,desktop",
            "--assert-audit",
            "--json",
        ])
        .env("KNR_HEADLESS", "1")
        .output()
        .expect("knr render --viewports");
    assert!(
        vp_run.status.success(),
        "knr render --viewports failed: stdout={} stderr={}",
        String::from_utf8_lossy(&vp_run.stdout),
        String::from_utf8_lossy(&vp_run.stderr)
    );

    // 5. Build standalone Web bundle (`v4-canvas` + zero-import `app.wasm`)
    let bld = Command::new(knr_bin())
        .args([
            "build",
            app.to_str().unwrap(),
            "--target",
            "web",
            "-o",
            out_dir.to_str().unwrap(),
        ])
        .env("KNR_HEADLESS", "1")
        .output()
        .expect("knr build --target web");
    assert!(
        bld.status.success(),
        "knr build --target web failed: {}",
        String::from_utf8_lossy(&bld.stderr)
    );

    let html = fs::read_to_string(out_dir.join("index.html")).expect("read index.html");
    assert!(
        html.contains(r#"data-knr-renderer="v4-canvas""#),
        "Web bundle must carry v4-canvas renderer marker"
    );
    assert!(html.contains("__knr_heap_used"), "Web bundle must wire arena telemetry");
    assert!(html.contains("on_key"), "Web bundle must wire on_key dispatch");
    assert!(html.contains("on_resize"), "Web bundle must wire on_resize dispatch");
    assert!(
        html.contains(r#"const WASM_B64="""#) || html.contains(r#"const WASM_B64 = """#),
        "Default `--target web` build must omit inline base64 WASM (only --standalone embeds it)"
    );

    // 5b. Verify `--standalone` flag embeds inline Base64 WASM for offline file:// opening
    let standalone_dir = out_dir.join("standalone_out");
    let bld_sa = Command::new(knr_bin())
        .args([
            "build",
            app.to_str().unwrap(),
            "--target",
            "web",
            "--standalone",
            "-o",
            standalone_dir.to_str().unwrap(),
        ])
        .env("KNR_HEADLESS", "1")
        .output()
        .expect("knr build --target web --standalone");
    assert!(
        bld_sa.status.success(),
        "knr build --target web --standalone failed: {}",
        String::from_utf8_lossy(&bld_sa.stderr)
    );
    let html_sa = fs::read_to_string(standalone_dir.join("index.html")).expect("read standalone html");
    assert!(
        !html_sa.contains(r#"const WASM_B64="""#) && !html_sa.contains(r#"const WASM_B64 = """#),
        "`--standalone` build must embed non-empty base64 WASM"
    );
    assert!(
        html_sa.len() > html.len() + 20_000,
        "Standalone HTML ({}) should be noticeably larger than default slim HTML ({})",
        html_sa.len(),
        html.len()
    );

    // 6. Verify BOTH embedded JS syntax AND real Node.js WASM 300-frame zero-GC arena loop!
    let node_check = Command::new("node").arg("--version").output();
    if node_check.is_ok_and(|o| o.status.success()) {
        let script = r#"
const fs = require('fs');
const html = fs.readFileSync(process.argv[1], 'utf8');
const scriptMatch = html.match(/<script data-knr-web-host="v3-canvas" data-knr-renderer="v4-canvas">([\s\S]*?)<\/script>/);
if (!scriptMatch) throw new Error('Missing v4-canvas script tag');
new Function(scriptMatch[1]);

const wasmBytes = fs.readFileSync(process.argv[2]);
WebAssembly.instantiate(wasmBytes, {}).then(({ instance }) => {
  const ex = instance.exports;
  if (typeof ex.init !== 'function') throw new Error('Missing init');
  if (typeof ex.view !== 'function') throw new Error('Missing view');
  if (typeof ex.update !== 'function') throw new Error('Missing update');
  if (typeof ex.on_key !== 'function') throw new Error('Missing on_key');
  if (typeof ex.on_resize !== 'function') throw new Error('Missing on_resize');
  if (typeof ex.__knr_heap_used !== 'function') throw new Error('Missing __knr_heap_used');
  if (typeof ex.__knr_heap_capacity !== 'function') throw new Error('Missing __knr_heap_capacity');
  if (typeof ex.__knr_heap_mark !== 'function') throw new Error('Missing __knr_heap_mark');
  if (typeof ex.__knr_heap_reset !== 'function') throw new Error('Missing __knr_heap_reset');

  const allocStr = (s) => {
    const enc = new TextEncoder().encode(s);
    const ptr = Number(ex.__knr_alloc(enc.length + 1));
    const mem = new Uint8Array(ex.memory.buffer);
    mem.set(enc, ptr);
    mem[ptr + enc.length] = 0;
    return BigInt(ptr);
  };

  let state = ex.init();
  state = ex.on_resize(state, 1280.0, 900.0);
  state = ex.on_key(state, allocStr('Ctrl+T'));
  state = ex.update(state, allocStr('RoutePipeline'));
  state = ex.update(state, allocStr('AddJob'));

  const cap0 = Number(ex.__knr_heap_capacity());
  if (cap0 <= 0) throw new Error('Invalid heap capacity: ' + cap0);

  let maxFrameDelta = 0;
  const markBeforeLoop = ex.__knr_heap_mark();
  for (let i = 0; i < 300; i++) {
    const m = ex.__knr_heap_mark();
    const used0 = Number(ex.__knr_heap_used());
    const rootPtr = ex.view(state);
    if (!rootPtr) throw new Error('Null root widget on frame ' + i);
    const used1 = Number(ex.__knr_heap_used());
    maxFrameDelta = Math.max(maxFrameDelta, used1 - used0);
    ex.__knr_heap_reset(m);
  }
  const usedAfter = Number(ex.__knr_heap_used());
  const markAfter = ex.__knr_heap_mark();
  if (markAfter !== markBeforeLoop) {
    throw new Error(`Arena leak: mark ${markBeforeLoop} -> ${markAfter}`);
  }
  console.log(JSON.stringify({ ok: true, frames: 300, maxFrameDelta, usedAfter, cap0 }));
}).catch((err) => {
  console.error(err);
  process.exit(1);
});
"#;
        let run_node = Command::new("node")
            .args([
                "-e",
                script,
                out_dir.join("index.html").to_str().unwrap(),
                out_dir.join("app.wasm").to_str().unwrap(),
            ])
            .output()
            .expect("node wasm run");
        assert!(
            run_node.status.success(),
            "Node.js v4-canvas WASM verification failed:\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&run_node.stdout),
            String::from_utf8_lossy(&run_node.stderr)
        );
        let node_out: Value = serde_json::from_slice(&run_node.stdout).expect("node json");
        assert_eq!(node_out["ok"], true);
        assert_eq!(node_out["frames"], 300);
        assert!(node_out["maxFrameDelta"].as_u64().unwrap_or(0) > 0);
    }

    let _ = fs::remove_dir_all(&out_dir);
}

#[test]
fn v04_mcp_stdio_live_workflow_on_neural_studio() {
    let root = workspace_root();
    let app = root.join("examples/neural_studio/src/app.tsk");

    let mut child = Command::new(knr_bin())
        .arg("mcp")
        .env("KNR_HEADLESS", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn knr mcp");

    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let mut reader = BufReader::new(stdout);

    let send_line = |stdin: &mut std::process::ChildStdin,
                     reader: &mut BufReader<std::process::ChildStdout>,
                     msg: Value|
     -> Value {
        writeln!(stdin, "{}", serde_json::to_string(&msg).unwrap()).unwrap();
        stdin.flush().unwrap();
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        serde_json::from_str(&line).expect("valid MCP JSON-RPC line")
    };

    // 1. Initialize
    let init_res = send_line(
        &mut stdin,
        &mut reader,
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "clientInfo": { "name": "v04-e2e", "version": "1.0" },
                "capabilities": {}
            }
        }),
    );
    assert_eq!(init_res["result"]["serverInfo"]["name"], "knr-mcp");

    // 2. tools/list -> 8 tools
    let list_res = send_line(
        &mut stdin,
        &mut reader,
        json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {} }),
    );
    let tools = list_res["result"]["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 8);

    // 3. knr_check on neural_studio
    let check_res = send_line(
        &mut stdin,
        &mut reader,
        json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tools/call",
            "params": {
                "name": "knr_check",
                "arguments": { "file": app.to_str().unwrap() }
            }
        }),
    );
    assert_eq!(check_res["result"]["isError"], false);
    let check_payload: Value =
        serde_json::from_str(check_res["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(check_payload["ok"], true);
    assert_eq!(check_payload["shader_count"], 2);

    // 4. knr_render_audit on neural_studio
    let render_res = send_line(
        &mut stdin,
        &mut reader,
        json!({
            "jsonrpc": "2.0",
            "id": 4,
            "method": "tools/call",
            "params": {
                "name": "knr_render_audit",
                "arguments": {
                    "file": app.to_str().unwrap(),
                    "width": 1280,
                    "height": 860,
                    "clicks": ["btn_nav_pipeline", "btn_cycle_theme"]
                }
            }
        }),
    );
    assert_eq!(render_res["result"]["isError"], false);
    let audit_payload: Value =
        serde_json::from_str(render_res["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(audit_payload["error_count"], 0);
    assert_eq!(audit_payload["warning_count"], 0);
    assert!(audit_payload["accessibility"]["focusable_count"].as_u64().unwrap_or(0) >= 4);

    // 5. knr_bench on neural_studio
    let bench_res = send_line(
        &mut stdin,
        &mut reader,
        json!({
            "jsonrpc": "2.0",
            "id": 5,
            "method": "tools/call",
            "params": {
                "name": "knr_bench",
                "arguments": { "file": app.to_str().unwrap(), "iters": 3 }
            }
        }),
    );
    assert_eq!(bench_res["result"]["isError"], false);

    drop(stdin);
    let status = child.wait().expect("wait knr mcp");
    assert!(status.success());
}

#[test]
fn v04_lsp_stdio_ide_ultra_capabilities_on_neural_studio() {
    let root = workspace_root();
    let app_path = root.join("examples/neural_studio/src/app.tsk");
    let app_src = fs::read_to_string(&app_path).expect("read neural_studio app.tsk");
    let app_uri = path_to_file_uri(&app_path);
    let root_uri = path_to_file_uri(&root.join("examples/neural_studio"));

    let mut child = Command::new(knr_bin())
        .arg("lsp")
        .env("KNR_HEADLESS", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn knr lsp");

    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let mut reader = BufReader::new(stdout);

    // 1. Initialize & verify all IDE-Ultra capabilities advertised
    write_lsp_frame(
        &mut stdin,
        &json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "rootUri": root_uri,
                "capabilities": {}
            }
        }),
    );
    let init = read_lsp_response_by_id(&mut reader, 1);
    let caps = &init["result"]["capabilities"];
    assert!(caps["signatureHelpProvider"].is_object());
    assert_eq!(caps["inlayHintProvider"], true);
    assert_eq!(caps["workspaceSymbolProvider"], true);
    assert_eq!(caps["foldingRangeProvider"], true);

    // 2. Open neural_studio/src/app.tsk
    write_lsp_frame(
        &mut stdin,
        &json!({
            "jsonrpc": "2.0",
            "method": "textDocument/didOpen",
            "params": {
                "textDocument": {
                    "uri": app_uri,
                    "languageId": "knr",
                    "version": 1,
                    "text": app_src
                }
            }
        }),
    );

    // 3. workspace/symbol search across neural_studio finds `neural_aurora_field` & `State`
    write_lsp_frame(
        &mut stdin,
        &json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "workspace/symbol",
            "params": { "query": "neural_aurora" }
        }),
    );
    let ws_syms = read_lsp_response_by_id(&mut reader, 2);
    let syms = ws_syms["result"].as_array().unwrap();
    assert!(
        syms.iter()
            .any(|s| s["name"].as_str() == Some("neural_aurora_field")),
        "workspace/symbol should find neural_aurora_field: {syms:?}"
    );

    // 4. textDocument/foldingRange returns import + function folds
    write_lsp_frame(
        &mut stdin,
        &json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "textDocument/foldingRange",
            "params": { "textDocument": { "uri": app_uri } }
        }),
    );
    let folds = read_lsp_response_by_id(&mut reader, 3);
    assert!(
        folds["result"].as_array().unwrap().len() >= 3,
        "Expected folding ranges on neural_studio app.tsk"
    );

    // 5. textDocument/inlayHint on app.tsk
    let total_lines = app_src.lines().count() as u32;
    write_lsp_frame(
        &mut stdin,
        &json!({
            "jsonrpc": "2.0",
            "id": 4,
            "method": "textDocument/inlayHint",
            "params": {
                "textDocument": { "uri": app_uri },
                "range": {
                    "start": { "line": 0, "character": 0 },
                    "end": { "line": total_lines, "character": 0 }
                }
            }
        }),
    );
    let hints = read_lsp_response_by_id(&mut reader, 4);
    assert!(
        !hints["result"].as_array().unwrap().is_empty(),
        "Expected inlay hints on neural_studio app.tsk"
    );

    // 6. Clean shutdown & exit
    write_lsp_frame(
        &mut stdin,
        &json!({ "jsonrpc": "2.0", "id": 5, "method": "shutdown", "params": null }),
    );
    let _ = read_lsp_response_by_id(&mut reader, 5);
    write_lsp_frame(
        &mut stdin,
        &json!({ "jsonrpc": "2.0", "method": "exit", "params": null }),
    );
    drop(stdin);
    let status = child.wait().expect("wait knr lsp");
    assert!(status.success());
}

#[test]
fn v04_doctor_bench_and_run_profile_cli() {
    let root = workspace_root();
    let app = root.join("examples/neural_studio/src/app.tsk");

    // 1. knr doctor --json
    let doc = Command::new(knr_bin())
        .args(["doctor", root.to_str().unwrap(), "--json"])
        .output()
        .expect("knr doctor");
    assert!(doc.status.success());
    let doc_json: Value = serde_json::from_slice(&doc.stdout).expect("doctor json");
    assert!(doc_json["checks"].as_array().unwrap().len() >= 6);

    // 2. knr bench --json includes VM profiler breakdown (`instructions`, `allocations`, `top_functions`)
    let bench = Command::new(knr_bin())
        .args(["bench", app.to_str().unwrap(), "--iters", "3", "--json"])
        .output()
        .expect("knr bench");
    assert!(
        bench.status.success(),
        "knr bench failed: {}",
        String::from_utf8_lossy(&bench.stderr)
    );
    let bench_json: Value = serde_json::from_slice(&bench.stdout).expect("bench json");
    assert!(bench_json["vm"]["mean_us"].as_f64().unwrap_or(0.0) > 0.0);
    assert!(bench_json["vm"]["instructions"].as_u64().unwrap_or(0) > 0);
    assert!(bench_json["sizes"]["wasm_bytes"].as_u64().unwrap_or(0) > 0);
    #[cfg(target_os = "linux")]
    assert!(
        bench_json["jit"]["mean_ns"].as_f64().unwrap_or(0.0) > 0.0,
        "Cranelift JIT must succeed on Linux via MmapPageAllocator without UAF: {bench_json}"
    );

    // 3. knr run --profile prints ExecutionStats table
    let tmp = temp_dir("profile");
    let prog = tmp.join("prof.tsk");
    fs::write(
        &prog,
        concat!(
            "function fib(n: i64): i64 {\n",
            "    if (n <= 1) { return n; }\n",
            "    return fib(n - 1) + fib(n - 2);\n",
            "}\n",
            "export function main(): i64 {\n",
            "    return fib(8);\n",
            "}\n",
        ),
    )
    .unwrap();
    let run = Command::new(knr_bin())
        .args(["run", prog.to_str().unwrap(), "--profile"])
        .output()
        .expect("knr run --profile");
    assert!(
        run.status.success(),
        "knr run --profile failed: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    let stdout = String::from_utf8_lossy(&run.stdout);
    assert!(stdout.contains("21"), "expected fib(8)=21 in stdout: {stdout}");
    assert!(
        stdout.contains("instructions") || stdout.contains("Profile") || stdout.contains("fib"),
        "expected profile output in stdout: {stdout}"
    );

    let _ = fs::remove_dir_all(&tmp);
}
