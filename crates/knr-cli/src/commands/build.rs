use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::args::BuildTarget;
use crate::c_runtime::{KNR_ENTRY_SHIM_C, KNR_RT_C};
use crate::error::CliError;

pub fn execute(file: &Path, target: BuildTarget, output: &Path) -> Result<(), CliError> {
    let module = if file.to_string_lossy().ends_with(".knr") {
        let src = std::fs::read_to_string(file).map_err(|e| {
            eprintln!("{e}");
            CliError::Other(e.to_string())
        })?;
        let _prog = knr_core::syntax::parse_knr(&src).map_err(|err| {
            eprintln!("{err}");
            CliError::Parse(err)
        })?;
        knr_verify::verify_program_from_file(file).map_err(|diagnostics| {
            for diag in &diagnostics {
                eprintln!("{diag}");
            }
            CliError::Verify(diagnostics.len())
        })?
    } else {
        let module = knr_core::load_module_from_path(file).map_err(|err| {
            eprintln!("{err}");
            CliError::Parse(err)
        })?;

        if let Err(diagnostics) = knr_verify::verify_module(&module) {
            for diag in &diagnostics {
                eprintln!("{diag}");
            }
            return Err(CliError::Verify(diagnostics.len()));
        }

        module
    };

    if let Some(parent) = output.parent() {
        if !parent.as_os_str().is_empty() && !parent.exists() {
            fs::create_dir_all(parent).map_err(CliError::Io)?;
        }
    }

    match target {
        BuildTarget::Wasm => {
            let backend = knr_codegen::WasmBackend::new();
            let wasm_bytes = backend.compile(&module).map_err(|err| {
                eprintln!("{err}");
                CliError::Codegen(err)
            })?;
            fs::write(output, wasm_bytes).map_err(CliError::Io)?;
            Ok(())
        }
        BuildTarget::Web => {
            let backend = knr_codegen::WasmBackend::new();
            let wasm_bytes = backend.compile(&module).map_err(|err| {
                eprintln!("{err}");
                CliError::Codegen(err)
            })?;

            // Compile all `shader fn` declarations to naga-validated WGSL + WebGL2 GLSL ES 3.00
            let compiled_shaders = knr_codegen::WgslBackend::new()
                .compile_all_shaders(&module)
                .map_err(|err| {
                    eprintln!("{err}");
                    CliError::Codegen(err)
                })?;
            let mut shader_map = std::collections::BTreeMap::new();
            for cs in compiled_shaders {
                shader_map.insert(
                    cs.name.clone(),
                    serde_json::json!({
                        "wgsl": cs.wgsl,
                        "glsl_vertex": cs.glsl_vertex,
                        "glsl_fragment": cs.glsl_fragment,
                        "custom_param_count": cs.custom_param_count,
                    }),
                );
            }
            let shaders_json =
                serde_json::to_string(&shader_map).unwrap_or_else(|_| "{}".to_string());

            let is_widget_v2 = module
                .get_function("view")
                .is_some_and(|f| f.ret_ty == knr_core::Type::Struct("Widget".to_string()));
            let tick_arity = module
                .get_function("tick")
                .map(|f| f.params.len())
                .unwrap_or(0);

            // Determine output directory and index.html / app.wasm paths
            let (out_dir, html_path, wasm_path, wasm_file_name) =
                if output.extension().and_then(|e| e.to_str()) == Some("html") {
                    let dir = output
                        .parent()
                        .filter(|p| !p.as_os_str().is_empty())
                        .unwrap_or_else(|| Path::new("."))
                        .to_path_buf();
                    let stem = output
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("app");
                    let wname = format!("{stem}.wasm");
                    (dir.clone(), output.to_path_buf(), dir.join(&wname), wname)
                } else {
                    let dir = output.to_path_buf();
                    let wname = "app.wasm".to_string();
                    (dir.clone(), dir.join("index.html"), dir.join(&wname), wname)
                };

            fs::create_dir_all(&out_dir).map_err(CliError::Io)?;
            fs::write(&wasm_path, &wasm_bytes).map_err(CliError::Io)?;

            // Generate SSR initial HTML via VM if module defines `init` and `view`
            let ssr_html = if module.get_function("init").is_some()
                && module.get_function("view").is_some()
            {
                let mut mock = knr_runtime::mock::MockPlatform::new();
                match knr_interp::execute_module(&module, "init", vec![], &mut mock, 1_000_000, false)
                    .and_then(|(st, _)| {
                        knr_interp::execute_module(
                            &module,
                            "view",
                            vec![st],
                            &mut mock,
                            1_000_000,
                            false,
                        )
                    }) {
                    Ok((ui_val, _)) => knr_runtime::gui::UiNode::from_value(&ui_val)
                        .map(|n| n.render_html())
                        .unwrap_or_default(),
                    Err(_) => String::new(),
                }
            } else {
                String::new()
            };

            let title = file
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("knr app");
            let wasm_b64 = base64_encode(&wasm_bytes);
            let index_html = generate_web_host_html(
                title,
                &wasm_file_name,
                &wasm_b64,
                &ssr_html,
                &shaders_json,
                is_widget_v2,
                tick_arity,
            );
            fs::write(&html_path, index_html).map_err(CliError::Io)?;
            Ok(())
        }
        BuildTarget::Native => {
            let mut backend = knr_codegen::CraneliftBackend::new().map_err(|err| {
                eprintln!("{err}");
                CliError::Codegen(err)
            })?;
            let obj_bytes = backend.compile_object(&module).map_err(|err| {
                eprintln!("{err}");
                CliError::Codegen(err)
            })?;

            let timestamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            let temp_dir = std::env::temp_dir().join(format!(
                "knr_build_{}_{}",
                std::process::id(),
                timestamp
            ));
            fs::create_dir_all(&temp_dir).map_err(CliError::Io)?;

            let obj_path = temp_dir.join("module.o");
            fs::write(&obj_path, obj_bytes).map_err(CliError::Io)?;

            let compiler = find_c_compiler().ok_or_else(|| {
                CliError::Linker(
                    "No C compiler (cc, gcc, clang) found in PATH for native linking".to_string(),
                )
            })?;

            let mut cmd = Command::new(compiler);
            cmd.arg("-O2").arg(&obj_path);

            // Prefer linking the real Rust knr-runtime staticlib (`libknr_runtime.a`)
            // so native binaries have full Tokio Net, Ui, Hw, Struct, String, and Channel support!
            if let Some(rust_rt_lib) = find_rust_runtime_staticlib() {
                let shim_path = temp_dir.join("entry_shim.c");
                fs::write(&shim_path, KNR_ENTRY_SHIM_C).map_err(CliError::Io)?;
                cmd.arg(&shim_path).arg(&rust_rt_lib);
            } else {
                let c_rt_path = temp_dir.join("knr_rt.c");
                fs::write(&c_rt_path, KNR_RT_C).map_err(CliError::Io)?;
                cmd.arg(&c_rt_path);
            }

            cmd.arg("-o")
                .arg(output)
                .arg("-lpthread")
                .arg("-ldl")
                .arg("-lm");

            #[cfg(target_os = "macos")]
            {
                for fw in [
                    "AppKit",
                    "CoreFoundation",
                    "CoreGraphics",
                    "Foundation",
                    "QuartzCore",
                ] {
                    cmd.arg("-framework").arg(fw);
                }
            }

            // If the module does not define `main`, wire ENTRY_FN to its first function
            if module.get_function("main").is_none() {
                if let Some(entry_func) = module.functions.first() {
                    cmd.arg(format!("-DENTRY_FN={}", entry_func.name));
                }
            }

            let compile_res = cmd.output();
            let _ = fs::remove_dir_all(&temp_dir);

            let res = compile_res.map_err(CliError::Io)?;
            if !res.status.success() {
                let err_msg = String::from_utf8_lossy(&res.stderr).to_string();
                eprintln!("Linker error: {err_msg}");
                return Err(CliError::Linker(err_msg));
            }

            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if let Ok(metadata) = fs::metadata(output) {
                    let mut perms = metadata.permissions();
                    perms.set_mode(0o755);
                    let _ = fs::set_permissions(output, perms);
                }
            }

            Ok(())
        }
    }
}

fn find_rust_runtime_staticlib() -> Option<PathBuf> {
    let mut candidates = Vec::new();

    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join("libknr_runtime.a"));
            if let Some(parent) = dir.parent() {
                candidates.push(parent.join("libknr_runtime.a"));
            }
        }
    }

    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd.join("target/release/libknr_runtime.a"));
        candidates.push(cwd.join("target/debug/libknr_runtime.a"));
        if let Some(parent) = cwd.parent() {
            candidates.push(parent.join("target/release/libknr_runtime.a"));
            candidates.push(parent.join("target/debug/libknr_runtime.a"));
            if let Some(gp) = parent.parent() {
                candidates.push(gp.join("target/release/libknr_runtime.a"));
                candidates.push(gp.join("target/debug/libknr_runtime.a"));
            }
        }
    }

    // Also inspect CARGO_MANIFEST_DIR at compile time
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    if let Some(ws_root) = manifest_dir.parent().and_then(|p| p.parent()) {
        candidates.push(ws_root.join("target/release/libknr_runtime.a"));
        candidates.push(ws_root.join("target/debug/libknr_runtime.a"));
    }

    candidates.into_iter().find(|p| p.is_file())
}

fn find_c_compiler() -> Option<&'static str> {
    ["cc", "gcc", "clang"]
        .iter()
        .copied()
        .find(|&compiler| Command::new(compiler).arg("--version").output().is_ok())
}

fn base64_encode(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = if chunk.len() > 1 { chunk[1] as u32 } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] as u32 } else { 0 };
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[((n >> 18) & 0x3F) as usize] as char);
        out.push(TABLE[((n >> 12) & 0x3F) as usize] as char);
        out.push(if chunk.len() > 1 { TABLE[((n >> 6) & 0x3F) as usize] as char } else { '=' });
        out.push(if chunk.len() > 2 { TABLE[(n & 0x3F) as usize] as char } else { '=' });
    }
    out
}

/// Generates a clean, 100% generic Web/WASM + WebGPU/WebGL2 host HTML document with:
/// - Pre-rendered SSR initial markup
/// - Zero-dependency generic JS runtime reading BOTH 8-field `Widget`+40-field `Style` AND legacy 11-field `UiNode`
/// - Key-based DOM reconciliation (preserves `<input>` focus & live `<canvas>` GPU pipelines)
/// - 60/120 FPS `requestAnimationFrame` `tick(state, dt, elapsed)` loop
/// - Real WebGPU shader execution with automatic WebGL2 GLSL ES 3.00 fallback!
fn generate_web_host_html(
    title: &str,
    wasm_file: &str,
    wasm_b64: &str,
    ssr_html: &str,
    shaders_json: &str,
    is_widget_v2: bool,
    tick_arity: usize,
) -> String {
    let escaped_title = knr_runtime::ui::escape_html_text(title);
    let safe_shaders_json = shaders_json.replace("</script", "<\\/script");
    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8" />
<meta name="viewport" content="width=device-width, initial-scale=1" />
<title>{escaped_title} — knr</title>
<style>
  *, *::before, *::after {{ box-sizing: border-box; margin: 0; padding: 0; }}
  html, body {{ min-height: 100vh; background: #0b1120; color: #f8fafc; font-family: Inter, system-ui, -apple-system, sans-serif; -webkit-font-smoothing: antialiased; }}
  button {{ transition: filter 120ms ease, transform 120ms ease; }}
  button:hover {{ filter: brightness(1.12); }}
  button:active {{ transform: scale(0.97); }}
  input[type="range"] {{ accent-color: #38bdf8; cursor: pointer; }}
</style>
</head>
<body>
<div id="knr-root">{ssr_html}</div>
<script data-knr-web-host="v2">
(async function() {{
  const WASM_URL = {wasm_url_json};
  const WASM_B64 = "{wasm_b64}";
  const IS_WIDGET_V2 = {is_widget_v2};
  const TICK_ARITY = {tick_arity};
  const KNR_SHADERS = {safe_shaders_json};
  const decoder = new TextDecoder();
  const encoder = new TextEncoder();

  async function loadWasm() {{
    if (location.protocol && location.protocol.startsWith("http")) {{
      try {{
        const res = await fetch(WASM_URL);
        if (res.ok) {{
          const buf = await res.arrayBuffer();
          return (await WebAssembly.instantiate(buf, {{}})).instance.exports;
        }}
      }} catch (_) {{}}
    }}
    const bin = atob(WASM_B64);
    const bytes = new Uint8Array(bin.length);
    for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i);
    return (await WebAssembly.instantiate(bytes.buffer, {{}})).instance.exports;
  }}

  function readStr(mem, ptr) {{
    if (!ptr || ptr === 0n) return "";
    const p = Number(ptr);
    const dv = new DataView(mem.buffer);
    const len = Number(dv.getBigUint64(p - 8, true));
    return decoder.decode(new Uint8Array(mem.buffer, p, len));
  }}

  function allocStr(exports, str) {{
    const bytes = encoder.encode(str);
    const raw = Number(exports.__knr_alloc(bytes.length + 9));
    const dv = new DataView(exports.memory.buffer);
    dv.setBigUint64(raw, BigInt(bytes.length), true);
    const u8 = new Uint8Array(exports.memory.buffer);
    u8.set(bytes, raw + 8);
    u8[raw + 8 + bytes.length] = 0;
    return BigInt(raw + 8);
  }}

  function fillToCss(spec) {{
    if (!spec) return "";
    const s = spec.trim();
    if (s.startsWith("linear(") && s.endsWith(")")) {{
      const p = s.slice(7, -1).split(",").map(x => x.trim());
      if (p.length >= 3) return `linear-gradient(${{p[0].replace(/deg$/,"")}}deg, ${{p[1]}}, ${{p[2]}})`;
    }}
    if (s.startsWith("radial(") && s.endsWith(")")) {{
      const p = s.slice(7, -1).split(",").map(x => x.trim());
      if (p.length >= 2) return `radial-gradient(circle, ${{p[0]}}, ${{p[1]}})`;
    }}
    return s;
  }}

  function readStyleV2(mem, ptr) {{
    if (!ptr || ptr === 0n) return {{}};
    const p = Number(ptr);
    const dv = new DataView(mem.buffer);
    const f = (i) => dv.getFloat64(p + i * 8, true);
    const s = (i) => readStr(mem, dv.getBigInt64(p + i * 8, true));
    const n = (i) => Number(dv.getBigInt64(p + i * 8, true));
    return {{
      w: f(0), h: f(1), minW: f(2), maxW: f(3),
      bg: s(4), color: s(5), border: s(6), borderW: f(7),
      radii: [f(8), f(9), f(10), f(11)],
      pad: [f(12), f(13), f(14), f(15)],
      gapX: f(16), gapY: f(17), fontSize: f(18), fontWeight: n(19),
      align: s(20), justify: s(21), gridCols: n(22),
      shadowX: f(23), shadowY: f(24), shadowBlur: f(25), shadowColor: s(26),
      backdropBlur: f(27), opacity: f(28),
      tx: f(29), ty: f(30), scale: f(31), rotateDeg: f(32),
      hoverBg: s(33), hoverBorder: s(34), hoverScale: f(35), transitionMs: f(36),
      overflow: s(37), zIndex: n(38), grow: f(39),
    }};
  }}

  function readWidgetV2(mem, ptr) {{
    const p = Number(ptr);
    const dv = new DataView(mem.buffer);
    const key = readStr(mem, dv.getBigInt64(p + 0, true));
    const kind = readStr(mem, dv.getBigInt64(p + 8, true)).toLowerCase();
    const text = readStr(mem, dv.getBigInt64(p + 16, true));
    const action = readStr(mem, dv.getBigInt64(p + 24, true));
    const value = dv.getFloat64(p + 32, true);
    const uPtr = Number(dv.getBigInt64(p + 40, true));
    const uniforms = [];
    if (uPtr > 0) {{
      const uLen = Number(dv.getBigUint64(uPtr + 0, true));
      const uData = Number(dv.getBigUint64(uPtr + 16, true));
      for (let i = 0; i < uLen && i < 16; i++) uniforms.push(dv.getFloat64(uData + i * 8, true));
    }}
    const style = readStyleV2(mem, dv.getBigInt64(p + 48, true));
    const cPtr = Number(dv.getBigInt64(p + 56, true));
    const children = [];
    if (cPtr > 0) {{
      const cLen = Number(dv.getBigUint64(cPtr + 0, true));
      const cData = Number(dv.getBigUint64(cPtr + 16, true));
      for (let i = 0; i < cLen; i++) {{
        const cp = dv.getBigInt64(cData + i * 8, true);
        if (cp !== 0n) children.push(readWidgetV2(mem, cp));
      }}
    }}
    return {{ key, kind, text, action, value, uniforms, style, children }};
  }}

  function readUiNodeV1(mem, ptr) {{
    const p = Number(ptr);
    const dv = new DataView(mem.buffer);
    const kind = readStr(mem, dv.getBigInt64(p + 0, true)).toLowerCase();
    const text = readStr(mem, dv.getBigInt64(p + 8, true));
    const action = readStr(mem, dv.getBigInt64(p + 16, true));
    const bg = readStr(mem, dv.getBigInt64(p + 24, true));
    const color = readStr(mem, dv.getBigInt64(p + 32, true));
    const border = readStr(mem, dv.getBigInt64(p + 40, true));
    const radius = Number(dv.getBigInt64(p + 48, true));
    const pad = Number(dv.getBigInt64(p + 56, true));
    const gap = Number(dv.getBigInt64(p + 64, true));
    const fontSize = Number(dv.getBigInt64(p + 72, true));
    const vecPtr = Number(dv.getBigInt64(p + 80, true));
    const children = [];
    if (vecPtr > 0) {{
      const len = Number(dv.getBigUint64(vecPtr + 0, true));
      const dataPtr = Number(dv.getBigUint64(vecPtr + 16, true));
      for (let i = 0; i < len; i++) {{
        const childPtr = dv.getBigInt64(dataPtr + i * 8, true);
        if (childPtr !== 0n) children.push(readUiNodeV1(mem, childPtr));
      }}
    }}
    return {{
      key: action, kind, text, action, value: 0, uniforms: [],
      style: {{
        w: 0, h: 0, minW: 0, maxW: 0, bg, color, border, borderW: 1,
        radii: [radius, radius, radius, radius], pad: [pad, pad, pad, pad],
        gapX: gap, gapY: gap, fontSize, fontWeight: 400, align: "", justify: "", gridCols: 0,
        shadowX: 0, shadowY: 0, shadowBlur: 0, shadowColor: "", backdropBlur: 0, opacity: 1,
        tx: 0, ty: 0, scale: 1, rotateDeg: 0, hoverBg: "", hoverBorder: "", hoverScale: 1,
        transitionMs: 0, overflow: "", zIndex: 0, grow: 0,
      }},
      children,
    }};
  }}

  function tagForKind(kind) {{
    if (kind === "button" || kind === "btn" || kind === "toggle" || kind === "switch") return "button";
    if (kind === "badge" || kind === "pill") return "span";
    if (kind === "input" || kind === "textinput" || kind === "slider" || kind === "range") return "input";
    if (kind === "image" || kind === "img") return "img";
    if (kind === "shader" || kind === "shaderview" || kind === "canvas") return "canvas";
    return "div";
  }}

  function applyStyle(el, node) {{
    const s = el.style;
    const st = node.style || {{}};
    const k = node.kind;
    if (k === "col" || k === "card" || k === "scroll") {{
      s.display = "flex"; s.flexDirection = "column";
      if (k === "scroll") s.overflowY = "auto";
    }} else if (k === "row") {{
      s.display = "flex"; s.flexDirection = "row"; s.flexWrap = "wrap"; s.alignItems = "center";
    }} else if (k === "grid") {{
      s.display = "grid"; s.gridTemplateColumns = `repeat(${{Math.max(1, st.gridCols || 1)}}, minmax(0, 1fr))`;
    }} else if (k === "stack" || k === "zstack") {{
      s.position = "relative"; s.display = "grid";
    }} else if (k === "button" || k === "btn") {{
      s.display = "inline-flex"; s.alignItems = "center"; s.justifyContent = "center";
      s.cursor = "pointer"; s.fontWeight = "600"; s.fontFamily = "inherit";
      if (!st.border) s.border = "none";
    }} else if (k === "toggle" || k === "switch") {{
      s.display = "inline-flex"; s.alignItems = "center"; s.gap = "8px";
      s.cursor = "pointer"; s.fontFamily = "inherit"; s.border = "none";
    }} else if (k === "badge" || k === "pill") {{
      s.display = "inline-flex"; s.alignItems = "center"; s.alignSelf = "flex-start"; s.fontWeight = "600";
    }} else if (k === "input" || k === "textinput") {{
      s.outline = "none"; s.fontFamily = "inherit"; s.width = "100%";
    }} else if (k === "slider" || k === "range") {{
      s.width = "100%";
    }} else if (k === "shader" || k === "shaderview" || k === "canvas") {{
      s.display = "block"; if (!st.w) s.width = "100%"; if (!st.h) s.height = "260px";
    }}

    if (st.w > 0) s.width = st.w + "px"; else if (st.w < 0) s.width = "100%";
    if (st.h > 0) s.height = st.h + "px"; else if (st.h < 0) s.height = "100%";
    if (st.minW > 0) s.minWidth = st.minW + "px";
    if (st.maxW > 0) s.maxWidth = st.maxW + "px";
    if (st.grow > 0) s.flexGrow = String(st.grow);
    if (st.align && st.align !== "stretch") {{
      s.alignItems = st.align === "start" ? "flex-start" : (st.align === "end" ? "flex-end" : st.align);
    }}
    if (st.justify && st.justify !== "start") {{
      const jm = {{ end: "flex-end", center: "center", between: "space-between", around: "space-around" }};
      s.justifyContent = jm[st.justify] || st.justify;
    }}
    if (st.bg) s.background = fillToCss(st.bg);
    if (st.color) s.color = st.color;
    if (st.border) s.border = `${{Math.max(1, st.borderW || 1)}}px solid ${{st.border}}`;
    const r = st.radii || [0,0,0,0];
    if (r[0] || r[1] || r[2] || r[3]) s.borderRadius = `${{r[0]}}px ${{r[1]}}px ${{r[2]}}px ${{r[3]}}px`;
    const p = st.pad || [0,0,0,0];
    if (p[0] || p[1] || p[2] || p[3]) {{
      if ((k === "button" || k === "btn" || k === "badge" || k === "pill") && Math.abs(p[0] - p[1]) < 0.01) {{
        s.padding = `${{Math.max(6, Math.round(p[0] * 0.6))}}px ${{Math.max(12, Math.round(p[0] * 1.3))}}px`;
      }} else {{
        s.padding = `${{p[0]}}px ${{p[1]}}px ${{p[2]}}px ${{p[3]}}px`;
      }}
    }}
    if (st.gapX > 0 || st.gapY > 0) s.gap = `${{st.gapY || st.gapX}}px ${{st.gapX || st.gapY}}px`;
    if (st.fontSize > 0) s.fontSize = st.fontSize + "px";
    if (st.fontWeight >= 600) s.fontWeight = String(st.fontWeight);
    if (st.shadowBlur > 0 && st.shadowColor) s.boxShadow = `${{st.shadowX}}px ${{st.shadowY}}px ${{st.shadowBlur}}px ${{st.shadowColor}}`;
    if (st.backdropBlur > 0) s.backdropFilter = `blur(${{st.backdropBlur}}px)`;
    if (st.opacity > 0 && st.opacity < 0.999) s.opacity = String(st.opacity);
    const xf = [];
    if (Math.abs(st.tx) > 0.01 || Math.abs(st.ty) > 0.01) xf.push(`translate(${{st.tx}}px,${{st.ty}}px)`);
    if (st.scale > 0 && Math.abs(st.scale - 1) > 0.002) xf.push(`scale(${{st.scale}})`);
    if (Math.abs(st.rotateDeg) > 0.01) xf.push(`rotate(${{st.rotateDeg}}deg)`);
    s.transform = xf.join(" ");
    if (st.transitionMs > 0) s.transition = `all ${{Math.round(st.transitionMs)}}ms cubic-bezier(0.22,1,0.36,1)`;
    if (st.overflow === "hidden") s.overflow = "hidden";
    if (st.zIndex) s.zIndex = String(st.zIndex);
  }}

  const activeCanvases = new Set();

  function patchNode(existing, node) {{
    const tag = tagForKind(node.kind);
    const expInputType = (node.kind === "slider" || node.kind === "range") ? "range" : "text";
    let el = existing;
    const sameTag = el && el.tagName && el.tagName.toLowerCase() === tag;
    const sameKey = el && ((el.dataset && el.dataset.knrKey) || "") === (node.key || "");
    const sameInput = tag !== "input" || (el && el.type === expInputType);

    if (!sameTag || !sameKey || !sameInput) {{
      el = document.createElement(tag);
      if (node.key) el.dataset.knrKey = node.key;
      if (tag === "input") {{
        el.type = expInputType;
        if (expInputType === "range") {{ el.min = "0"; el.max = "1"; el.step = "0.005"; }}
      }}
    }}

    if (node.action) el.setAttribute("data-knr-action", node.action);
    else el.removeAttribute("data-knr-action");

    applyStyle(el, node);

    if (tag === "input") {{
      if (expInputType === "range") {{
        if (document.activeElement !== el) el.value = String(node.value);
      }} else {{
        if (document.activeElement !== el && el.value !== node.text) el.value = node.text;
      }}
      return el;
    }}
    if (tag === "img") {{
      if (el.getAttribute("src") !== node.text) el.setAttribute("src", node.text);
      return el;
    }}
    if (tag === "canvas") {{
      el.setAttribute("data-knr-shader", node.text);
      el.__knrShaderName = node.text;
      el.__knrUniforms = node.uniforms || [];
      el.__knrRadii = (node.style && node.style.radii) || [0,0,0,0];
      activeCanvases.add(el);
      ensureShaderCanvas(el);
      return el;
    }}

    if (node.children.length === 0) {{
      const txt = node.kind === "toggle" ? ((node.value > 0.5 ? "● " : "○ ") + node.text) : node.text;
      if (el.textContent !== txt) el.textContent = txt;
    }} else {{
      // Reconcile children in-place to preserve canvas/input identity across 60fps ticks
      const existingKids = Array.from(el.childNodes);
      let idx = 0;
      if (node.text) {{
        let txtNode = existingKids[0];
        if (!txtNode || txtNode.nodeType !== 3) {{
          txtNode = document.createTextNode(node.text);
          el.insertBefore(txtNode, el.firstChild);
        }} else if (txtNode.nodeValue !== node.text) {{
          txtNode.nodeValue = node.text;
        }}
        idx = 1;
      }}
      for (let i = 0; i < node.children.length; i++) {{
        const oldChild = el.childNodes[idx + i];
        const nextChild = patchNode(oldChild, node.children[i]);
        if (nextChild !== oldChild) {{
          if (oldChild) el.replaceChild(nextChild, oldChild);
          else el.appendChild(nextChild);
        }}
      }}
      while (el.childNodes.length > idx + node.children.length) {{
        el.removeChild(el.lastChild);
      }}
    }}
    return el;
  }}

  // ========================================================================
  // Real WebGPU + Automatic WebGL2 Shader Runtime (128-byte KnrUniforms)
  // ========================================================================
  let gpuDevicePromise = null;
  async function getWebGpuDevice() {{
    if (!navigator.gpu) return null;
    if (!gpuDevicePromise) {{
      gpuDevicePromise = (async () => {{
        try {{
          const adapter = await navigator.gpu.requestAdapter();
          return adapter ? await adapter.requestDevice() : null;
        }} catch (_) {{ return null; }}
      }})();
    }}
    return gpuDevicePromise;
  }}

  function initWebGl2(canvas, shaderName) {{
    const spec = KNR_SHADERS[shaderName];
    if (!spec || !spec.glsl_vertex || !spec.glsl_fragment) return null;
    const gl = canvas.getContext("webgl2", {{ alpha: true, antialias: true }});
    if (!gl) return null;
    const compile = (type, src) => {{
      const sh = gl.createShader(type);
      gl.shaderSource(sh, src);
      gl.compileShader(sh);
      if (!gl.getShaderParameter(sh, gl.COMPILE_STATUS)) {{
        console.error("knr WebGL2 shader error:", gl.getShaderInfoLog(sh));
        return null;
      }}
      return sh;
    }};
    const vs = compile(gl.VERTEX_SHADER, spec.glsl_vertex);
    const fs = compile(gl.FRAGMENT_SHADER, spec.glsl_fragment);
    if (!vs || !fs) return null;
    const prog = gl.createProgram();
    gl.attachShader(prog, vs);
    gl.attachShader(prog, fs);
    gl.linkProgram(prog);
    if (!gl.getProgramParameter(prog, gl.LINK_STATUS)) {{
      console.error("knr WebGL2 link error:", gl.getProgramInfoLog(prog));
      return null;
    }}
    const blockCount = gl.getProgramParameter(prog, gl.ACTIVE_UNIFORM_BLOCKS);
    for (let i = 0; i < blockCount; i++) gl.uniformBlockBinding(prog, i, 0);
    const ubo = gl.createBuffer();
    gl.bindBuffer(gl.UNIFORM_BUFFER, ubo);
    gl.bufferData(gl.UNIFORM_BUFFER, 128, gl.DYNAMIC_DRAW);
    gl.bindBufferBase(gl.UNIFORM_BUFFER, 0, ubo);
    return {{ kind: "webgl2", shaderName, gl, prog, ubo }};
  }}

  async function ensureShaderCanvas(canvas) {{
    const name = canvas.__knrShaderName;
    if (!name || !KNR_SHADERS[name]) return;
    if (canvas.__knrGpu && canvas.__knrGpu.shaderName === name) return;
    if (canvas.__knrGpuInit === name) return;
    canvas.__knrGpuInit = name;

    const spec = KNR_SHADERS[name];
    // 1. Primary: WebGPU
    const device = await getWebGpuDevice();
    if (device && spec.wgsl) {{
      try {{
        const ctx = canvas.getContext("webgpu");
        if (ctx) {{
          const fmt = navigator.gpu.getPreferredCanvasFormat();
          ctx.configure({{ device, format: fmt, alphaMode: "premultiplied" }});
          const mod = device.createShaderModule({{ code: spec.wgsl }});
          const pipe = device.createRenderPipeline({{
            layout: "auto",
            vertex: {{ module: mod, entryPoint: "vs_main" }},
            fragment: {{ module: mod, entryPoint: "fs_main", targets: [{{ format: fmt }}] }},
            primitive: {{ topology: "triangle-list" }},
          }});
          const ubuf = device.createBuffer({{ size: 128, usage: GPUBufferUsage.UNIFORM | GPUBufferUsage.COPY_DST }});
          const bg = device.createBindGroup({{
            layout: pipe.getBindGroupLayout(0),
            entries: [{{ binding: 0, resource: {{ buffer: ubuf }} }}],
          }});
          canvas.__knrGpu = {{ kind: "webgpu", shaderName: name, device, ctx, pipe, ubuf, bg }};
          return;
        }}
      }} catch (err) {{
        console.warn("knr WebGPU fallback to WebGL2:", err);
      }}
    }}
    // 2. Universal Fallback: WebGL2 using naga-generated GLSL ES 3.00
    const glState = initWebGl2(canvas, name);
    if (glState) canvas.__knrGpu = glState;
  }}

  let mouseX = 0.5, mouseY = 0.5;
  window.addEventListener("pointermove", (e) => {{ mouseX = e.clientX; mouseY = e.clientY; }});

  const uData = new Float32Array(32);
  function drawShaderCanvases(elapsed, dt) {{
    for (const canvas of activeCanvases) {{
      if (!canvas.isConnected) {{ activeCanvases.delete(canvas); continue; }}
      const gpu = canvas.__knrGpu;
      if (!gpu) continue;
      const rect = canvas.getBoundingClientRect();
      const dpr = window.devicePixelRatio || 1;
      const pw = Math.max(1, Math.round(rect.width * dpr));
      const ph = Math.max(1, Math.round(rect.height * dpr));
      if (canvas.width !== pw || canvas.height !== ph) {{ canvas.width = pw; canvas.height = ph; }}

      uData.fill(0);
      uData[0] = rect.width || pw; uData[1] = rect.height || ph;
      uData[2] = elapsed; uData[3] = dt;
      uData[4] = rect.width > 0 ? Math.min(1, Math.max(0, (mouseX - rect.left) / rect.width)) : 0.5;
      uData[5] = rect.height > 0 ? Math.min(1, Math.max(0, (mouseY - rect.top) / rect.height)) : 0.5;
      const rad = canvas.__knrRadii || [0,0,0,0];
      uData[8] = rad[0]; uData[9] = rad[1]; uData[10] = rad[2]; uData[11] = rad[3];
      const u = canvas.__knrUniforms || [];
      for (let i = 0; i < u.length && i < 16; i++) uData[12 + i] = u[i];

      if (gpu.kind === "webgpu") {{
        gpu.device.queue.writeBuffer(gpu.ubuf, 0, uData);
        const enc = gpu.device.createCommandEncoder();
        const pass = enc.beginRenderPass({{
          colorAttachments: [{{
            view: gpu.ctx.getCurrentTexture().createView(),
            clearValue: {{ r: 0.04, g: 0.07, b: 0.13, a: 1 }},
            loadOp: "clear", storeOp: "store",
          }}],
        }});
        pass.setPipeline(gpu.pipe);
        pass.setBindGroup(0, gpu.bg);
        pass.draw(6);
        pass.end();
        gpu.device.queue.submit([enc.finish()]);
      }} else if (gpu.kind === "webgl2") {{
        const gl = gpu.gl;
        gl.viewport(0, 0, pw, ph);
        gl.useProgram(gpu.prog);
        gl.bindBuffer(gl.UNIFORM_BUFFER, gpu.ubo);
        gl.bufferSubData(gl.UNIFORM_BUFFER, 0, uData);
        gl.bindBufferBase(gl.UNIFORM_BUFFER, 0, gpu.ubo);
        gl.drawArrays(gl.TRIANGLES, 0, 6);
      }}
    }}
  }}

  const exports = await loadWasm();
  if (typeof exports.init !== "function" || typeof exports.view !== "function") return;
  const root = document.getElementById("knr-root");
  let state = exports.init();

  function render() {{
    const treePtr = exports.view(state);
    const tree = IS_WIDGET_V2 ? readWidgetV2(exports.memory, treePtr) : readUiNodeV1(exports.memory, treePtr);
    const next = patchNode(root.firstElementChild, tree);
    if (next !== root.firstElementChild) root.replaceChildren(next);
  }}

  window.__knr_dispatch = function(action) {{
    if (typeof exports.update === "function" && typeof exports.__knr_alloc === "function") {{
      const actPtr = allocStr(exports, action);
      state = exports.update(state, actPtr);
      render();
    }}
  }};

  root.addEventListener("click", function(e) {{
    const el = e.target && e.target.closest ? e.target.closest("[data-knr-action]") : null;
    if (!el || el.tagName === "INPUT") return;
    const act = el.getAttribute("data-knr-action");
    if (act) window.__knr_dispatch(act);
  }});

  root.addEventListener("input", function(e) {{
    const el = e.target;
    if (!el || !el.getAttribute) return;
    const act = el.getAttribute("data-knr-action");
    if (!act || typeof exports.__knr_alloc !== "function") return;
    if (el.type === "range") {{
      const v = parseFloat(el.value);
      if (typeof exports.on_slider === "function") {{
        state = exports.on_slider(state, allocStr(exports, act), v);
        render();
      }} else {{
        window.__knr_dispatch(act + ":" + v.toFixed(3));
      }}
    }} else {{
      if (typeof exports.on_input === "function") {{
        state = exports.on_input(state, allocStr(exports, act), allocStr(exports, el.value));
        render();
      }} else {{
        window.__knr_dispatch(act + ":" + el.value);
      }}
    }}
  }});

  render();

  // 60/120 FPS animation & shader loop
  const hasTick = typeof exports.tick === "function" && TICK_ARITY >= 2;
  if (hasTick || activeCanvases.size > 0 || Object.keys(KNR_SHADERS).length > 0) {{
    let startMs = performance.now();
    let lastMs = startMs;
    function frame(now) {{
      const dt = Math.min(0.05, Math.max(0.0001, (now - lastMs) / 1000));
      const elapsed = (now - startMs) / 1000;
      lastMs = now;
      if (hasTick) {{
        state = TICK_ARITY >= 3 ? exports.tick(state, dt, elapsed) : exports.tick(state, dt);
        render();
      }}
      drawShaderCanvases(elapsed, dt);
      requestAnimationFrame(frame);
    }}
    requestAnimationFrame(frame);
  }}
}})();
</script>
</body>
</html>
"#,
        wasm_url_json = serde_json::to_string(wasm_file).unwrap_or_else(|_| "\"app.wasm\"".into())
    )
}
