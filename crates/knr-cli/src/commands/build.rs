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
            let index_html = generate_web_host_html(title, &wasm_file_name, &wasm_b64, &ssr_html);
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

/// Generates a clean, 100% generic Web/WASM host HTML document with SSR initial markup
/// and a tiny (~80-line) zero-dependency JS runtime reading `UiNode` straight from WASM linear memory.
fn generate_web_host_html(title: &str, wasm_file: &str, wasm_b64: &str, ssr_html: &str) -> String {
    let escaped_title = knr_runtime::ui::escape_html_text(title);
    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8" />
<meta name="viewport" content="width=device-width, initial-scale=1" />
<title>{escaped_title} — knr</title>
<style>
  *, *::before, *::after {{ box-sizing: border-box; margin: 0; padding: 0; }}
  html, body {{ min-height: 100vh; background: #0b1120; color: #f8fafc; font-family: Inter, system-ui, -apple-system, sans-serif; }}
  button:hover {{ filter: brightness(1.12); }}
  button:active {{ transform: scale(0.98); }}
</style>
</head>
<body>
<div id="knr-root">{ssr_html}</div>
<script data-knr-web-host="v1">
(async function() {{
  const WASM_URL = {wasm_url_json};
  const WASM_B64 = "{wasm_b64}";
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

  function readUiNode(mem, ptr) {{
    const p = Number(ptr);
    const dv = new DataView(mem.buffer);
    const kind = readStr(mem, dv.getBigInt64(p + 0, true));
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
        if (childPtr !== 0n) children.push(readUiNode(mem, childPtr));
      }}
    }}
    return {{ kind, text, action, bg, color, border, radius, pad, gap, fontSize, children }};
  }}

  function buildDom(node) {{
    const tag = node.kind === "button" ? "button" : (node.kind === "badge" ? "span" : "div");
    const el = document.createElement(tag);
    const s = el.style;
    if (node.kind === "col" || node.kind === "card") {{
      s.display = "flex"; s.flexDirection = "column";
    }} else if (node.kind === "row") {{
      s.display = "flex"; s.flexDirection = "row"; s.flexWrap = "wrap"; s.alignItems = "center";
    }} else if (node.kind === "button") {{
      s.display = "inline-flex"; s.alignItems = "center"; s.justifyContent = "center";
      s.cursor = "pointer"; s.fontWeight = "600"; s.fontFamily = "inherit";
      if (!node.border) s.border = "none";
    }} else if (node.kind === "badge") {{
      s.display = "inline-flex"; s.alignItems = "center"; s.alignSelf = "flex-start"; s.fontWeight = "600";
    }}
    if (node.bg) s.background = node.bg;
    if (node.color) s.color = node.color;
    if (node.border) s.border = "1px solid " + node.border;
    if (node.radius > 0) s.borderRadius = node.radius + "px";
    if (node.pad > 0) {{
      if (node.kind === "button" || node.kind === "badge") {{
        s.padding = Math.max(6, Math.round(node.pad * 0.6)) + "px " + Math.max(12, Math.round(node.pad * 1.3)) + "px";
      }} else {{
        s.padding = node.pad + "px";
      }}
    }}
    if (node.gap > 0) s.gap = node.gap + "px";
    if (node.fontSize > 0) s.fontSize = node.fontSize + "px";
    if (node.action) el.setAttribute("data-knr-action", node.action);
    if (node.text) el.textContent = node.text;
    for (const ch of node.children) el.appendChild(buildDom(ch));
    return el;
  }}

  const exports = await loadWasm();
  if (typeof exports.init !== "function" || typeof exports.view !== "function") return;
  const root = document.getElementById("knr-root");
  let state = exports.init();

  function render() {{
    const treePtr = exports.view(state);
    const tree = readUiNode(exports.memory, treePtr);
    root.replaceChildren(buildDom(tree));
  }}

  window.__knr_dispatch = function(action) {{
    if (typeof exports.update === "function" && typeof exports.__knr_alloc === "function") {{
      const actPtr = allocStr(exports, action);
      state = exports.update(state, actPtr);
      render();
    }}
  }};

  root.addEventListener("click", function(e) {{
    const btn = e.target && e.target.closest ? e.target.closest("[data-knr-action]") : null;
    if (!btn) return;
    const act = btn.getAttribute("data-knr-action");
    if (act) window.__knr_dispatch(act);
  }});

  render();
}})();
</script>
</body>
</html>
"#,
        wasm_url_json = serde_json::to_string(wasm_file).unwrap_or_else(|_| "\"app.wasm\"".into())
    )
}
