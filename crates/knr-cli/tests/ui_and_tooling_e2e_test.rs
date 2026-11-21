use std::fs;
use std::path::PathBuf;
use std::process::Command;

use knr_interp::Value;
use knr_runtime::gui::{hit_test, pixmap_fnv_hash, UiNode, UiRenderer};
use knr_runtime::mock::MockPlatform;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

fn knr_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_knr"))
}

#[test]
fn test_milestone_d_and_e_counter_app_check_test_explore_gui_and_web_wasm() {
    let root = repo_root();
    let app_knr = root.join("examples/counter_app.knr");

    // ------------------------------------------------------------------
    // 1. knr check examples/counter_app.knr (multi-file import "./ui.knr")
    // ------------------------------------------------------------------
    let check_out = Command::new(knr_bin())
        .args(["check", app_knr.to_str().unwrap()])
        .output()
        .expect("knr check counter_app.knr");
    assert!(
        check_out.status.success(),
        "knr check failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&check_out.stdout),
        String::from_utf8_lossy(&check_out.stderr)
    );

    // ------------------------------------------------------------------
    // 2. knr test examples/counter_app.knr --json
    // ------------------------------------------------------------------
    let test_out = Command::new(knr_bin())
        .args(["test", app_knr.to_str().unwrap(), "--json"])
        .output()
        .expect("knr test counter_app.knr --json");
    assert!(
        test_out.status.success(),
        "knr test failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&test_out.stdout),
        String::from_utf8_lossy(&test_out.stderr)
    );
    let report: serde_json::Value =
        serde_json::from_slice(&test_out.stdout).expect("valid JSON test report");
    assert_eq!(report["status"], "ok");
    assert_eq!(report["passed"], 3);
    assert_eq!(report["failed"], 0);

    // ------------------------------------------------------------------
    // 3. knr explore examples/counter_app.knr --json
    // ------------------------------------------------------------------
    let explore_out = Command::new(knr_bin())
        .args(["explore", app_knr.to_str().unwrap(), "--json"])
        .output()
        .expect("knr explore counter_app.knr --json");
    assert!(
        explore_out.status.success(),
        "knr explore failed: {}",
        String::from_utf8_lossy(&explore_out.stderr)
    );
    let explore_json: serde_json::Value =
        serde_json::from_slice(&explore_out.stdout).expect("valid JSON explore report");
    let explore_str = explore_json.to_string();
    assert!(explore_str.contains("update"), "explore must list fn update");
    assert!(explore_str.contains("view"), "explore must list fn view");

    // ------------------------------------------------------------------
    // 4. Native Desktop UI Pipeline: Flexbox layout + tiny-skia + ab_glyph + Hit-Testing
    // ------------------------------------------------------------------
    let module = knr_verify::verify_program_from_file(&app_knr).expect("verify counter_app.knr");
    let mut mock = MockPlatform::new();
    let (mut state, _) =
        knr_interp::execute_module(&module, "init", vec![], &mut mock, 1_000_000, false)
            .expect("init()");
    let (view_val_0, _) = knr_interp::execute_module(
        &module,
        "view",
        vec![state.clone()],
        &mut mock,
        1_000_000,
        false,
    )
    .expect("view(&state)");
    let ui_0 = UiNode::from_value(&view_val_0).expect("UiNode::from_value");
    let renderer = UiRenderer::new();
    let (pix_0, hit_boxes_0) = renderer.render_ui(&ui_0, 820, 560, None);
    let hash_0 = pixmap_fnv_hash(&pix_0);

    // Verify all 6 interactive action buttons are laid out with valid bounding boxes
    for expected_action in ["dec", "inc", "reset", "step_1", "step_5", "step_10"] {
        assert!(
            hit_boxes_0
                .iter()
                .any(|hb| hb.action == expected_action && hb.rect.w > 20.0 && hb.rect.h > 15.0),
            "Missing or zero-sized hitbox for '{expected_action}': {hit_boxes_0:#?}"
        );
    }

    // Simulate real pointer clicks via hit_test -> update -> view -> re-render
    for click_action in ["step_5", "inc", "inc", "dec"] {
        let (_, hbs) = renderer.render_ui(
            &UiNode::from_value(
                &knr_interp::execute_module(
                    &module,
                    "view",
                    vec![state.clone()],
                    &mut mock,
                    1_000_000,
                    false,
                )
                .unwrap()
                .0,
            )
            .unwrap(),
            820,
            560,
            None,
        );
        let target = hbs
            .iter()
            .find(|hb| hb.action == click_action)
            .unwrap_or_else(|| panic!("hitbox {click_action} not found"));
        let cx = target.rect.x + target.rect.w * 0.5;
        let cy = target.rect.y + target.rect.h * 0.5;
        assert_eq!(hit_test(&hbs, cx, cy), Some(click_action));

        let (next_state, _) = knr_interp::execute_module(
            &module,
            "update",
            vec![state, Value::Str(click_action.to_string())],
            &mut mock,
            1_000_000,
            false,
        )
        .expect("update()");
        state = next_state;
    }

    let (view_val_1, _) = knr_interp::execute_module(
        &module,
        "view",
        vec![state.clone()],
        &mut mock,
        1_000_000,
        false,
    )
    .expect("view after clicks");
    let ui_1 = UiNode::from_value(&view_val_1).expect("UiNode after clicks");
    let (pix_1, _) = renderer.render_ui(&ui_1, 820, 560, Some("inc"));
    let hash_1 = pixmap_fnv_hash(&pix_1);
    assert_ne!(
        hash_0, hash_1,
        "Rendered pixmap must visually update after MVU clicks"
    );
    let html_1 = ui_1.render_html();
    assert!(html_1.contains("Count: 5"), "Expected 'Count: 5' in rendered UI HTML: {html_1}");

    // Also run `knr run examples/counter_app.knr --gui` in headless CI mode
    let gui_out = Command::new(knr_bin())
        .env("KNR_HEADLESS", "1")
        .args(["run", app_knr.to_str().unwrap(), "--gui"])
        .output()
        .expect("knr run --gui headless");
    assert!(
        gui_out.status.success(),
        "knr run --gui failed: {}",
        String::from_utf8_lossy(&gui_out.stderr)
    );
    assert!(
        String::from_utf8_lossy(&gui_out.stdout).contains("rendered MVU UI snapshot"),
        "Expected MVU snapshot message, got: {}",
        String::from_utf8_lossy(&gui_out.stdout)
    );

    // ------------------------------------------------------------------
    // 5. Generic Web/WASM Export: `knr build --target web` + LIVE WASM execution in Node.js!
    // ------------------------------------------------------------------
    let dist_dir = std::env::temp_dir().join(format!("knr_web_export_test_{}", std::process::id()));
    let _ = fs::remove_dir_all(&dist_dir);

    let build_out = Command::new(knr_bin())
        .args([
            "build",
            app_knr.to_str().unwrap(),
            "--target",
            "web",
            "-o",
            dist_dir.to_str().unwrap(),
        ])
        .output()
        .expect("knr build --target web");
    assert!(
        build_out.status.success(),
        "knr build --target web failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&build_out.stdout),
        String::from_utf8_lossy(&build_out.stderr)
    );

    let wasm_path = dist_dir.join("app.wasm");
    let html_path = dist_dir.join("index.html");
    assert!(wasm_path.is_file(), "dist/app.wasm must be generated");
    assert!(html_path.is_file(), "dist/index.html must be generated");

    let html_content = fs::read_to_string(&html_path).expect("read index.html");
    assert!(html_content.contains("Count: 0"), "SSR HTML must include initial 'Count: 0'");
    assert!(html_content.contains("data-knr-web-host=\"v1\""));
    assert!(!html_content.contains("site.wasm"), "Zero hardcoded site.wasm hacks");
    assert!(!html_content.contains("/nav/"), "Zero hardcoded /nav/ hacks");

    // Execute the exported zero-import app.wasm in Node.js using the exact linear-memory UiNode protocol!
    let node_script = format!(
        r#"
const fs = require('fs');
const bytes = fs.readFileSync({wasm_path_json});
const dec = new TextDecoder();
const enc = new TextEncoder();

function readStr(mem, ptr) {{
  if (!ptr || ptr === 0n) return "";
  const p = Number(ptr);
  const dv = new DataView(mem.buffer);
  const len = Number(dv.getBigUint64(p - 8, true));
  return dec.decode(new Uint8Array(mem.buffer, p, len));
}}

function allocStr(exports, str) {{
  const b = enc.encode(str);
  const raw = Number(exports.__knr_alloc(b.length + 9));
  const dv = new DataView(exports.memory.buffer);
  dv.setBigUint64(raw, BigInt(b.length), true);
  const u8 = new Uint8Array(exports.memory.buffer);
  u8.set(b, raw + 8);
  u8[raw + 8 + b.length] = 0;
  return BigInt(raw + 8);
}}

function readUiNode(mem, ptr) {{
  const p = Number(ptr);
  const dv = new DataView(mem.buffer);
  const kind = readStr(mem, dv.getBigInt64(p + 0, true));
  const text = readStr(mem, dv.getBigInt64(p + 8, true));
  const action = readStr(mem, dv.getBigInt64(p + 16, true));
  const vecPtr = Number(dv.getBigInt64(p + 80, true));
  const children = [];
  if (vecPtr > 0) {{
    const len = Number(dv.getBigUint64(vecPtr + 0, true));
    const dataPtr = Number(dv.getBigUint64(vecPtr + 16, true));
    for (let i = 0; i < len; i++) {{
      const cp = dv.getBigInt64(dataPtr + i * 8, true);
      if (cp !== 0n) children.push(readUiNode(mem, cp));
    }}
  }}
  return {{ kind, text, action, children }};
}}

function collectTexts(node, out) {{
  if (node.text) out.push(node.text);
  for (const c of node.children) collectTexts(c, out);
  return out;
}}

WebAssembly.instantiate(bytes, {{}}).then(({{ instance }}) => {{
  const e = instance.exports;
  let st = e.init();
  const t0 = collectTexts(readUiNode(e.memory, e.view(st)), []);
  st = e.update(st, allocStr(e, "step_10"));
  st = e.update(st, allocStr(e, "inc"));
  st = e.update(st, allocStr(e, "inc"));
  st = e.update(st, allocStr(e, "dec"));
  const t1 = collectTexts(readUiNode(e.memory, e.view(st)), []);
  console.log(JSON.stringify({{ t0, t1 }}));
}}).catch(err => {{ console.error(err); process.exit(1); }});
"#,
        wasm_path_json = serde_json::to_string(wasm_path.to_str().unwrap()).unwrap()
    );

    if let Ok(node_out) = Command::new("node").arg("-e").arg(&node_script).output() {
        assert!(
            node_out.status.success(),
            "Node.js WASM MVU execution failed:\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&node_out.stdout),
            String::from_utf8_lossy(&node_out.stderr)
        );
        let parsed: serde_json::Value =
            serde_json::from_slice(&node_out.stdout).expect("parse node WASM MVU JSON");
        let t0 = parsed["t0"].to_string();
        let t1 = parsed["t1"].to_string();
        assert!(t0.contains("Count: 0"), "Initial WASM UI tree must say Count: 0, got {t0}");
        assert!(t1.contains("Count: 10"), "Updated WASM UI tree must say Count: 10, got {t1}");
        assert!(t1.contains("Decremented by -10"), "Updated WASM UI status mismatch: {t1}");
    }

    let _ = fs::remove_dir_all(&dist_dir);
}

#[test]
fn test_knr_test_command_reports_failure_cleanly() {
    let tmp = std::env::temp_dir().join(format!("knr_failing_test_{}.knr", std::process::id()));
    fs::write(
        &tmp,
        r#"
        fn test_ok() -> bool {
            return 2 + 2 == 4;
        }
        fn test_boom() -> bool {
            return 2 + 2 == 5;
        }
        "#,
    )
    .unwrap();

    let out = Command::new(knr_bin())
        .args(["test", tmp.to_str().unwrap(), "--json"])
        .output()
        .expect("knr test failing");
    let _ = fs::remove_file(&tmp);

    assert!(!out.status.success(), "knr test must exit non-zero on failing test");
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).expect("JSON report");
    assert_eq!(report["status"], "failed");
    assert_eq!(report["passed"], 1);
    assert_eq!(report["failed"], 1);
}
