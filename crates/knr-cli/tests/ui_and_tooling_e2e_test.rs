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
    assert!(html_content.contains("data-knr-web-host=\"v2\""));
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
fn test_builtin_gpu_wgsl_shaders_pass_naga_validation() {
    for (name, wgsl) in knr_runtime::gui::gpu::BUILTIN_WGSL_SHADERS {
        knr_codegen::wgsl::validate_wgsl_source(wgsl)
            .unwrap_or_else(|e| panic!("Built-in GPU shader '{name}' failed naga validation: {e}"));
    }
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

#[test]
fn test_gpu_shader_studio_full_e2e_check_test_anim_gui_and_web_wasm() {
    use knr_runtime::gui::{
        compute_ui_layout, RenderInteractionState, UiAnimator, UiKind,
    };

    let root = repo_root();
    let studio_knr = root.join("examples/shader_studio.knr");

    // 1. knr check (verifies imports + shader purity + naga WGSL/GLSL compilation!)
    let check_out = Command::new(knr_bin())
        .args(["check", studio_knr.to_str().unwrap()])
        .output()
        .expect("knr check shader_studio.knr");
    assert!(
        check_out.status.success(),
        "knr check shader_studio.knr failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&check_out.stdout),
        String::from_utf8_lossy(&check_out.stderr)
    );

    // 2. knr test --json
    let test_out = Command::new(knr_bin())
        .args(["test", studio_knr.to_str().unwrap(), "--json"])
        .output()
        .expect("knr test shader_studio.knr");
    assert!(
        test_out.status.success(),
        "knr test shader_studio.knr failed:\n{}",
        String::from_utf8_lossy(&test_out.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&test_out.stdout).expect("JSON");
    assert_eq!(report["status"], "ok");
    assert_eq!(report["passed"], 3);
    assert_eq!(report["failed"], 0);

    // 3. Multi-frame headless animation + interactive sliders + TextInput + shader switching
    let module = knr_verify::verify_program_from_file(&studio_knr).expect("verify shader_studio");
    let mut mock = MockPlatform::new();
    let (mut state, _) =
        knr_interp::execute_module(&module, "init", vec![], &mut mock, 1_000_000, false)
            .expect("init()");

    let renderer = UiRenderer::new();
    let mut animator = UiAnimator::new();

    // Frame 0 at t=0.0
    let (v0, _) = knr_interp::execute_module(
        &module,
        "view",
        vec![state.clone()],
        &mut mock,
        1_000_000,
        false,
    )
    .expect("view 0");
    let ui0 = UiNode::from_value(&v0).expect("UiNode v0");
    assert!(ui0.contains_shader(), "shader_studio UI tree must contain a Shader widget");

    let (mut laid0, hbs0) = compute_ui_layout(&ui0, 1040.0, 700.0, &renderer);
    animator.step_and_apply(&mut laid0, Some("shader_aurora"), 0.016);
    let pix0 = renderer.render_layout(
        &laid0,
        1040,
        700,
        &RenderInteractionState {
            hovered_action: Some("shader_aurora"),
            focused_action: None,
            elapsed_sec: 0.0,
        },
    );
    let hash0 = pixmap_fnv_hash(&pix0);

    // Verify all interactive widget kinds are laid out properly (Button, Slider, Input, Toggle)
    assert!(hbs0.iter().any(|h| h.kind == UiKind::Button && h.action == "shader_plasma"));
    assert!(hbs0.iter().any(|h| h.kind == UiKind::Slider && h.action == "slider_speed"));
    assert!(hbs0.iter().any(|h| h.kind == UiKind::Input && h.action == "input_banner"));
    assert!(hbs0.iter().any(|h| h.kind == UiKind::Toggle && h.action == "toggle_pause"));

    // Step tick() + switch shader + move slider + edit banner -> Frame 1 at t=0.75
    state = knr_interp::execute_module(
        &module,
        "tick",
        vec![state, Value::F64(0.75), Value::F64(0.75)],
        &mut mock,
        1_000_000,
        false,
    )
    .unwrap()
    .0;
    state = knr_interp::execute_module(
        &module,
        "update",
        vec![state, Value::Str("shader_plasma".into())],
        &mut mock,
        1_000_000,
        false,
    )
    .unwrap()
    .0;
    state = knr_interp::execute_module(
        &module,
        "on_slider",
        vec![state, Value::Str("slider_speed".into()), Value::F64(0.92)],
        &mut mock,
        1_000_000,
        false,
    )
    .unwrap()
    .0;
    state = knr_interp::execute_module(
        &module,
        "on_input",
        vec![
            state,
            Value::Str("input_banner".into()),
            Value::Str("Live GPU Studio".into()),
        ],
        &mut mock,
        1_000_000,
        false,
    )
    .unwrap()
    .0;

    let (v1, _) = knr_interp::execute_module(
        &module,
        "view",
        vec![state],
        &mut mock,
        1_000_000,
        false,
    )
    .unwrap();
    let ui1 = UiNode::from_value(&v1).unwrap();
    let (mut laid1, _) = compute_ui_layout(&ui1, 1040.0, 700.0, &renderer);
    animator.step_and_apply(&mut laid1, Some("shader_plasma"), 0.016);
    let pix1 = renderer.render_layout(
        &laid1,
        1040,
        700,
        &RenderInteractionState {
            hovered_action: Some("shader_plasma"),
            focused_action: Some("input_banner"),
            elapsed_sec: 0.75,
        },
    );
    let hash1 = pixmap_fnv_hash(&pix1);
    assert_ne!(hash0, hash1, "Animated/updated GPU studio frames must render distinct pixels");

    // 4. Headless `knr run --gui`
    let gui_out = Command::new(knr_bin())
        .env("KNR_HEADLESS", "1")
        .args(["run", studio_knr.to_str().unwrap(), "--gui"])
        .output()
        .expect("knr run shader_studio --gui");
    assert!(gui_out.status.success());

    // 5. `knr build --target web` + live Node.js WASM execution verifying 8-field Widget & 40-field Style!
    let dist_dir = std::env::temp_dir().join(format!("knr_shader_web_test_{}", std::process::id()));
    let _ = fs::remove_dir_all(&dist_dir);

    let build_out = Command::new(knr_bin())
        .args([
            "build",
            studio_knr.to_str().unwrap(),
            "--target",
            "web",
            "-o",
            dist_dir.to_str().unwrap(),
        ])
        .output()
        .expect("knr build shader_studio --target web");
    assert!(
        build_out.status.success(),
        "knr build shader_studio --target web failed:\n{}",
        String::from_utf8_lossy(&build_out.stderr)
    );

    let wasm_path = dist_dir.join("app.wasm");
    let html_path = dist_dir.join("index.html");
    let html_content = fs::read_to_string(&html_path).expect("read index.html");
    assert!(html_content.contains("const IS_WIDGET_V2 = true;"));
    assert!(html_content.contains("aurora_waves"));
    assert!(html_content.contains("plasma_orbit"));
    assert!(html_content.contains("#version 300 es"), "Must embed WebGL2 GLSL ES 3.00 fallback");
    assert!(html_content.contains("@fragment"), "Must embed WebGPU WGSL shaders");

    // Execute exported zero-import WASM in Node.js (init -> tick -> update -> on_slider -> on_input -> view)
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
function readWidget(mem, ptr) {{
  const p = Number(ptr);
  const dv = new DataView(mem.buffer);
  const key = readStr(mem, dv.getBigInt64(p + 0, true));
  const kind = readStr(mem, dv.getBigInt64(p + 8, true));
  const text = readStr(mem, dv.getBigInt64(p + 16, true));
  const action = readStr(mem, dv.getBigInt64(p + 24, true));
  const value = dv.getFloat64(p + 32, true);
  const uPtr = Number(dv.getBigInt64(p + 40, true));
  const uniforms = [];
  if (uPtr > 0) {{
    const uLen = Number(dv.getBigUint64(uPtr, true));
    const uData = Number(dv.getBigUint64(uPtr + 16, true));
    for (let i = 0; i < uLen; i++) uniforms.push(dv.getFloat64(uData + i * 8, true));
  }}
  const cPtr = Number(dv.getBigInt64(p + 56, true));
  const children = [];
  if (cPtr > 0) {{
    const len = Number(dv.getBigUint64(cPtr, true));
    const dataPtr = Number(dv.getBigUint64(cPtr + 16, true));
    for (let i = 0; i < len; i++) {{
      const cp = dv.getBigInt64(dataPtr + i * 8, true);
      if (cp !== 0n) children.push(readWidget(mem, cp));
    }}
  }}
  return {{ key, kind, text, action, value, uniforms, children }};
}}
function findKind(w, k) {{
  if (w.kind === k) return w;
  for (const c of w.children) {{ const r = findKind(c, k); if (r) return r; }}
  return null;
}}
function collectTexts(w, out) {{
  if (w.text) out.push(w.text);
  for (const c of w.children) collectTexts(c, out);
  return out;
}}

WebAssembly.instantiate(bytes, {{}}).then(({{ instance }}) => {{
  const e = instance.exports;
  let st = e.init();
  st = e.tick(st, 0.25, 0.25);
  const w0 = readWidget(e.memory, e.view(st));
  const sh0 = findKind(w0, "shader");
  st = e.update(st, allocStr(e, "shader_plasma"));
  st = e.on_slider(st, allocStr(e, "slider_speed"), 0.92);
  st = e.on_input(st, allocStr(e, "input_banner"), allocStr(e, "WASM Shader Live"));
  const w1 = readWidget(e.memory, e.view(st));
  const sh1 = findKind(w1, "shader");
  const texts = collectTexts(w1, []);
  console.log(JSON.stringify({{ sh0, sh1, texts }}));
}}).catch(err => {{ console.error(err); process.exit(1); }});
"#,
        wasm_path_json = serde_json::to_string(wasm_path.to_str().unwrap()).unwrap()
    );

    if let Ok(node_out) = Command::new("node").arg("-e").arg(&node_script).output() {
        assert!(
            node_out.status.success(),
            "Node.js shader_studio WASM failed:\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&node_out.stdout),
            String::from_utf8_lossy(&node_out.stderr)
        );
        let parsed: serde_json::Value =
            serde_json::from_slice(&node_out.stdout).expect("parse shader_studio WASM JSON");
        assert_eq!(parsed["sh0"]["text"], "aurora_waves");
        assert_eq!(parsed["sh1"]["text"], "plasma_orbit");
        let speed_u = parsed["sh1"]["uniforms"][0].as_f64().unwrap();
        assert!((speed_u - 0.92).abs() < 1e-6, "Expected slider uniform 0.92, got {speed_u}");
        assert!(
            parsed["texts"].to_string().contains("WASM Shader Live"),
            "Expected live TextInput update in WASM view: {}",
            parsed["texts"]
        );
    }

    let _ = fs::remove_dir_all(&dist_dir);
}

#[test]
fn test_stdlib_3d_raymarch_site_revamp_hot_reload_and_lsp_e2e() {
    let root = repo_root();
    let raymarch_knr = root.join("examples/raymarch_3d.knr");
    let site_knr = root.join("site/site.knr");

    // 1. Built-in `import "std/ui";` + `import "std/math";` -> 3D SDF Raymarching demo
    for (label, path, expected_shader) in [
        ("raymarch_3d.knr", &raymarch_knr, "raymarch_scene"),
        ("site.knr", &site_knr, "knr_hero_nebula"),
    ] {
        let chk = Command::new(knr_bin())
            .args(["check", path.to_str().unwrap()])
            .output()
            .unwrap_or_else(|e| panic!("knr check {label}: {e}"));
        assert!(
            chk.status.success(),
            "knr check {label} failed:\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&chk.stdout),
            String::from_utf8_lossy(&chk.stderr)
        );

        let tst = Command::new(knr_bin())
            .args(["test", path.to_str().unwrap(), "--json"])
            .output()
            .unwrap_or_else(|e| panic!("knr test {label}: {e}"));
        assert!(
            tst.status.success(),
            "knr test {label} failed:\n{}",
            String::from_utf8_lossy(&tst.stderr)
        );
        let report: serde_json::Value = serde_json::from_slice(&tst.stdout).expect("JSON report");
        assert_eq!(report["status"], "ok", "{label} test report: {report}");
        assert_eq!(report["passed"], 3, "{label} expected 3 passed tests");
        assert_eq!(report["failed"], 0);

        // Build standalone Web + WebGPU/WebGL2 bundle
        let dist = std::env::temp_dir().join(format!(
            "knr_e2e_web_{}_{}",
            expected_shader,
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dist);
        let bld = Command::new(knr_bin())
            .args(["build", path.to_str().unwrap(), "--target", "web", "-o", dist.to_str().unwrap()])
            .output()
            .expect("knr build --target web");
        assert!(
            bld.status.success(),
            "knr build {label} --target web failed:\n{}",
            String::from_utf8_lossy(&bld.stderr)
        );
        let html = fs::read_to_string(dist.join("index.html")).expect("read index.html");
        assert!(html.contains(expected_shader), "{label} HTML missing shader {expected_shader}");
        assert!(html.contains("#version 300 es"), "{label} HTML missing WebGL2 GLSL ES 3.00");
        assert!(html.contains("@fragment"), "{label} HTML missing WGSL fragment shader");
        let _ = fs::remove_dir_all(&dist);
    }

    // 2. Live GUI Hot-Reload (`knr dev examples/raymarch_3d.knr --gui --once` headless check)
    let dev_gui = Command::new(knr_bin())
        .env("KNR_HEADLESS", "1")
        .args(["dev", raymarch_knr.to_str().unwrap(), "--gui", "--once"])
        .output()
        .expect("knr dev --gui --once");
    assert!(
        dev_gui.status.success(),
        "knr dev --gui --once failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&dev_gui.stdout),
        String::from_utf8_lossy(&dev_gui.stderr)
    );
    assert!(
        String::from_utf8_lossy(&dev_gui.stdout).contains("[DEV-GUI]"),
        "Expected [DEV-GUI] output, got: {}",
        String::from_utf8_lossy(&dev_gui.stdout)
    );

    // 3. Built-in LSP server (`knr lsp` protocol via `handle_message`)
    use knr_cli::commands::lsp::{handle_message, LspState};
    let mut lsp = LspState::new();
    let init_res = handle_message(
        &mut lsp,
        &serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {}
        }),
    );
    assert_eq!(init_res.len(), 1);
    assert_eq!(init_res[0]["result"]["capabilities"]["hoverProvider"], true);

    let uri = "file:///tmp/lsp_test_demo.knr";
    let valid_doc = r#"import "std/ui";

shader fn pulse(uv: Vec2, time: f64) -> Color {
    let v = 0.5 + 0.5 * sin(uv.x * 6.0 + time);
    return Color { r: v, g: 0.4, b: 0.9, a: 1.0 };
}

fn demo() -> i64 {
    return 42;
}
"#;
    let open_msgs = handle_message(
        &mut lsp,
        &serde_json::json!({
            "jsonrpc": "2.0",
            "method": "textDocument/didOpen",
            "params": {
                "textDocument": { "uri": uri, "languageId": "knr", "version": 1, "text": valid_doc }
            }
        }),
    );
    assert_eq!(open_msgs.len(), 1);
    assert_eq!(
        open_msgs[0]["params"]["diagnostics"].as_array().unwrap().len(),
        0,
        "Valid std/ui + shader fn doc must have 0 LSP diagnostics: {:?}",
        open_msgs[0]
    );

    let sym_res = handle_message(
        &mut lsp,
        &serde_json::json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "textDocument/documentSymbol",
            "params": { "textDocument": { "uri": uri } }
        }),
    );
    let syms_str = sym_res[0].to_string();
    assert!(syms_str.contains("pulse") && syms_str.contains("demo"));

    let comp_res = handle_message(
        &mut lsp,
        &serde_json::json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "textDocument/completion",
            "params": { "textDocument": { "uri": uri }, "position": { "line": 3, "character": 12 } }
        }),
    );
    let comp_str = comp_res[0].to_string();
    assert!(comp_str.contains("backdrop_blur") && comp_str.contains("smoothstep"));

    // 4. Verify multi-line text wrapping on site.knr ("Linear Safety" tab): NO line overflows its box!
    use knr_runtime::gui::{compute_ui_layout, LayoutNode, TextMeasurer, UiKind};
    let site_mod = knr_verify::verify_program_from_file(&site_knr).expect("verify site.knr");
    let mut mock = MockPlatform::new();
    let (mut st, _) =
        knr_interp::execute_module(&site_mod, "init", vec![], &mut mock, 1_000_000, false).unwrap();
    st = knr_interp::execute_module(
        &site_mod,
        "update",
        vec![st, Value::Str("tab_safety".into())],
        &mut mock,
        1_000_000,
        false,
    )
    .unwrap()
    .0;
    let (view_val, _) =
        knr_interp::execute_module(&site_mod, "view", vec![st], &mut mock, 1_000_000, false)
            .unwrap();
    let site_ui = UiNode::from_value(&view_val).unwrap();
    let renderer = UiRenderer::new();
    let (laid, _) = compute_ui_layout(&site_ui, 1120.0, 800.0, &renderer);

    fn assert_no_text_overflow(node: &LayoutNode, renderer: &UiRenderer, wrapped_count: &mut usize) {
        if node.node.kind == UiKind::Text && !node.wrapped_lines.is_empty() {
            if node.wrapped_lines.len() > 1 {
                *wrapped_count += 1;
            }
            let [_, pr, _, pl] = node.node.effective_pad();
            let max_allowed = (node.rect.w - pl - pr).max(16.0) + 2.0;
            for line in &node.wrapped_lines {
                let (lw, _) = renderer.measure_text_size(line, node.node.font_size);
                assert!(
                    lw <= max_allowed,
                    "Text line overflowed container ({lw:.1}px > {max_allowed:.1}px): '{line}'"
                );
            }
        }
        for ch in &node.children {
            assert_no_text_overflow(ch, renderer, wrapped_count);
        }
    }

    let mut wrapped_count = 0usize;
    assert_no_text_overflow(&laid, &renderer, &mut wrapped_count);
    assert!(
        wrapped_count >= 4,
        "Expected long descriptions on Linear Safety tab to wrap across multiple lines, got {wrapped_count}"
    );
}

#[test]
fn test_v010_native_gui_binary_std_sys_ui_polish_and_editors_e2e() {
    use knr_runtime::gui::{compute_ui_layout, RenderInteractionState, UiKind};

    let root = repo_root();

    // ------------------------------------------------------------------
    // 1. Editor Extensions (`editors/`) exist & all JSON manifests parse cleanly
    // ------------------------------------------------------------------
    for rel in [
        "editors/README.md",
        "editors/vscode/package.json",
        "editors/vscode/language-configuration.json",
        "editors/vscode/syntaxes/knr.tmLanguage.json",
        "editors/vscode/extension.js",
        "editors/neovim/knr.lua",
        "editors/zed/extension.toml",
        "editors/zed/languages/knr/config.toml",
    ] {
        let p = root.join(rel);
        assert!(p.is_file(), "Missing editor extension file: {rel}");
        if rel.ends_with(".json") {
            let content = fs::read_to_string(&p).expect("read editor JSON");
            let _: serde_json::Value = serde_json::from_str(&content)
                .unwrap_or_else(|e| panic!("Invalid JSON in {rel}: {e}"));
        }
    }

    // ------------------------------------------------------------------
    // 2. System Stdlib (`import "std/sys";`): check, test, VM run & native AOT ELF!
    // ------------------------------------------------------------------
    let sys_demo = root.join("examples/sys_demo.knr");
    let chk = Command::new(knr_bin())
        .args(["check", sys_demo.to_str().unwrap()])
        .output()
        .expect("knr check sys_demo.knr");
    assert!(
        chk.status.success(),
        "knr check sys_demo.knr failed:\n{}",
        String::from_utf8_lossy(&chk.stderr)
    );

    let tst = Command::new(knr_bin())
        .args(["test", sys_demo.to_str().unwrap(), "--json"])
        .output()
        .expect("knr test sys_demo.knr");
    assert!(
        tst.status.success(),
        "knr test sys_demo.knr failed:\n{}",
        String::from_utf8_lossy(&tst.stderr)
    );
    let tst_json: serde_json::Value = serde_json::from_slice(&tst.stdout).expect("JSON");
    assert_eq!(tst_json["status"], "ok");
    assert_eq!(tst_json["passed"], 3);
    assert_eq!(tst_json["failed"], 0);

    let vm_run = Command::new(knr_bin())
        .args(["run", sys_demo.to_str().unwrap()])
        .output()
        .expect("knr run sys_demo.knr");
    assert!(
        vm_run.status.success(),
        "knr run sys_demo.knr failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&vm_run.stdout),
        String::from_utf8_lossy(&vm_run.stderr)
    );
    assert!(
        String::from_utf8_lossy(&vm_run.stdout).contains("8"),
        "Expected sys_demo VM to pass all 8 checks, got: {}",
        String::from_utf8_lossy(&vm_run.stdout)
    );

    let rt_lib_exists = root.join("target/debug/libknr_runtime.a").is_file()
        || root.join("target/release/libknr_runtime.a").is_file();

    if rt_lib_exists {
        // Native AOT binary for `examples/sys_demo.knr` (verifies rt_syscall struct bridging)
        let sys_bin = std::env::temp_dir().join(format!("knr_sys_native_{}", std::process::id()));
        let _ = fs::remove_file(&sys_bin);
        let bld_sys = Command::new(knr_bin())
            .args([
                "build",
                sys_demo.to_str().unwrap(),
                "--target",
                "native",
                "-o",
                sys_bin.to_str().unwrap(),
            ])
            .output()
            .expect("knr build sys_demo --target native");
        assert!(
            bld_sys.status.success(),
            "knr build sys_demo --target native failed:\n{}",
            String::from_utf8_lossy(&bld_sys.stderr)
        );
        let run_sys_native = Command::new(&sys_bin).output().expect("run sys_demo ELF");
        let _ = fs::remove_file(&sys_bin);
        assert_eq!(
            run_sys_native.status.code(),
            Some(8),
            "Expected sys_demo native ELF main() to return 8 (all 8 fs+json checks passed), got {:?}\nstdout: {}\nstderr: {}",
            run_sys_native.status.code(),
            String::from_utf8_lossy(&run_sys_native.stdout),
            String::from_utf8_lossy(&run_sys_native.stderr)
        );

        // ------------------------------------------------------------------
        // 3. Standalone Native Desktop GUI Binary (`knr build --target native --gui`)
        // ------------------------------------------------------------------
        let studio_knr = root.join("examples/shader_studio.knr");
        let gui_bin =
            std::env::temp_dir().join(format!("knr_gui_native_{}", std::process::id()));
        let _ = fs::remove_file(&gui_bin);
        let bld_gui = Command::new(knr_bin())
            .args([
                "build",
                studio_knr.to_str().unwrap(),
                "--target",
                "native",
                "--gui",
                "-o",
                gui_bin.to_str().unwrap(),
            ])
            .output()
            .expect("knr build shader_studio --target native --gui");
        assert!(
            bld_gui.status.success(),
            "knr build --target native --gui failed:\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&bld_gui.stdout),
            String::from_utf8_lossy(&bld_gui.stderr)
        );
        assert!(gui_bin.is_file(), "Native GUI ELF binary must be created");

        let run_gui = Command::new(&gui_bin)
            .env("KNR_HEADLESS", "1")
            .output()
            .expect("run native GUI ELF headless");
        let _ = fs::remove_file(&gui_bin);
        assert!(
            run_gui.status.success(),
            "Native GUI ELF headless run failed:\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&run_gui.stdout),
            String::from_utf8_lossy(&run_gui.stderr)
        );
        assert!(
            String::from_utf8_lossy(&run_gui.stdout)
                .contains("[GUI-NATIVE] Headless rendered snapshot"),
            "Expected [GUI-NATIVE] snapshot line, got: {}",
            String::from_utf8_lossy(&run_gui.stdout)
        );
    }

    // ------------------------------------------------------------------
    // 4. UI Polish: Divider, Progress, Responsive Grid Breakpoints & Scrollbar
    // ------------------------------------------------------------------
    let ui_src = r#"
        import "std/ui";

        fn view() -> Widget {
            let cards: vec[Widget] = vec_new();
            let c1: vec[Widget] = vec_new();
            c1 = vec_push(c1, w_text("t1", "Card 1", 16.0, #f8fafc));
            cards = vec_push(cards, w_card("k1", style_card(), c1));
            let c2: vec[Widget] = vec_new();
            c2 = vec_push(c2, w_text("t2", "Card 2", 16.0, #f8fafc));
            cards = vec_push(cards, w_card("k2", style_card(), c2));
            let c3: vec[Widget] = vec_new();
            c3 = vec_push(c3, w_text("t3", "Card 3", 16.0, #f8fafc));
            cards = vec_push(cards, w_card("k3", style_card(), c3));

            let root_kids: vec[Widget] = vec_new();
            root_kids = vec_push(root_kids, w_grid("g", 3, Style { gap_x: 16.0, gap_y: 16.0, ..style_default() }, cards));
            root_kids = vec_push(root_kids, w_divider("d1", style_default()));
            root_kids = vec_push(root_kids, w_progress("p1", 0.65, Style { h: 12.0, ..style_default() }));
            return w_col("root", Style { bg: #020617, pad_t: 20.0, pad_r: 20.0, pad_b: 20.0, pad_l: 20.0, gap_y: 16.0, ..style_default() }, root_kids);
        }
    "#;
    let prog = knr_core::syntax::parse_knr(ui_src).expect("parse polish UI");
    let module = knr_core::lower::lower_program(&prog).expect("lower polish UI");
    knr_verify::verify_module(&module).expect("verify polish UI");

    let mut mock = MockPlatform::new();
    let (ui_val, _) =
        knr_interp::execute_module(&module, "view", vec![], &mut mock, 1_000_000, false).unwrap();
    let ui_node = UiNode::from_value(&ui_val).unwrap();
    assert_eq!(ui_node.children[1].kind, UiKind::Divider);
    assert_eq!(ui_node.children[2].kind, UiKind::Progress);

    let html = ui_node.render_html();
    assert!(html.contains("65.0%"), "Progress HTML must show 65.0% fill: {html}");

    let renderer = UiRenderer::new();
    // Wide (1120px): all 3 grid cards on the same row (equal y)
    let (laid_wide, _) = compute_ui_layout(&ui_node, 1120.0, 600.0, &renderer);
    let g_wide = &laid_wide.children[0].children;
    assert!((g_wide[0].rect.y - g_wide[1].rect.y).abs() < 1.0);
    assert!((g_wide[0].rect.y - g_wide[2].rect.y).abs() < 1.0);

    // Medium (680px): collapses 3 -> 2 cols (cards 0 & 1 on row 0, card 2 on row 1)
    let (laid_med, _) = compute_ui_layout(&ui_node, 680.0, 600.0, &renderer);
    let g_med = &laid_med.children[0].children;
    assert!((g_med[0].rect.y - g_med[1].rect.y).abs() < 1.0);
    assert!(g_med[2].rect.y > g_med[0].rect.y + 10.0);

    // Narrow (420px): collapses to 1 col (all 3 stacked vertically)
    let (laid_narrow, _) = compute_ui_layout(&ui_node, 420.0, 600.0, &renderer);
    let g_narrow = &laid_narrow.children[0].children;
    assert!(g_narrow[1].rect.y > g_narrow[0].rect.y + 10.0);
    assert!(g_narrow[2].rect.y > g_narrow[1].rect.y + 10.0);

    // Render with and without scrollbar overlay and verify visual difference
    let state = RenderInteractionState::default();
    let pix_plain = renderer.render_layout(&laid_wide, 800, 500, &state);
    let mut pix_scroll = renderer.render_layout(&laid_wide, 800, 500, &state);
    renderer.paint_scrollbar(&mut pix_scroll, 120.0, 1400.0);
    assert_ne!(
        pixmap_fnv_hash(&pix_plain),
        pixmap_fnv_hash(&pix_scroll),
        "Scrollbar indicator must visibly paint when content overflows viewport"
    );
}
