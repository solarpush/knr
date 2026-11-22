use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::args::BuildTarget;
use crate::c_runtime::{KNR_ENTRY_SHIM_C, KNR_RT_C};
use crate::error::CliError;

fn compile_shaders_for_build(
    file: &Path,
    is_source: bool,
    module: &knr_core::Module,
) -> Result<Vec<knr_codegen::CompiledShader>, CliError> {
    let source_map = if is_source && module.functions.iter().any(|f| f.is_shader) {
        knr_core::lower::lower_program_from_file_with_source_map(file)
            .ok()
            .map(|(_, sm)| sm)
    } else {
        None
    };
    let wgsl_backend = knr_codegen::WgslBackend::new();
    match source_map.as_ref() {
        Some(sm) => wgsl_backend.compile_all_shaders_with_source_map(module, sm),
        None => wgsl_backend.compile_all_shaders(module),
    }
    .map_err(|err| {
        eprintln!("{err}");
        CliError::Codegen(err)
    })
}

pub fn execute(file: &Path, target: BuildTarget, output: &Path, gui: bool) -> Result<(), CliError> {
    let is_source = knr_core::is_source_file(file);
    let module = if is_source {
        let src = std::fs::read_to_string(file).map_err(|e| {
            eprintln!("{e}");
            CliError::Other(e.to_string())
        })?;
        let _prog = knr_core::syntax::parse_tsx(&src).map_err(|err| {
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

            // Compile all `@shader` declarations to naga-validated WGSL + WebGL2 GLSL ES 3.00
            // using SourceMap parameter names so `resolution` and `dt` auto-bind cleanly!
            let compiled_shaders = compile_shaders_for_build(file, is_source, &module)?;
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
            let style_field_count = module
                .get_schema("Style")
                .map(|s| s.fields.len())
                .unwrap_or(0);
            let tick_arity = module
                .get_function("tick")
                .map(|f| f.params.len())
                .unwrap_or(0);
            let on_resize_fn = module.get_function("on_resize");
            let on_resize_arity = on_resize_fn.map(|f| f.params.len()).unwrap_or(0);
            let on_resize_w_i64 = on_resize_fn
                .and_then(|f| f.params.get(1))
                .is_some_and(|(_, ty)| *ty == knr_core::Type::I64);
            let on_resize_h_i64 = on_resize_fn
                .and_then(|f| f.params.get(2))
                .is_some_and(|(_, ty)| *ty == knr_core::Type::I64);

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

            let title = file
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("knr app");
            let wasm_b64 = base64_encode(&wasm_bytes);
            let index_html = generate_web_host_html(&WebHostConfig {
                title,
                wasm_file: &wasm_file_name,
                wasm_b64: &wasm_b64,
                shaders_json: &shaders_json,
                is_widget_v2,
                style_field_count,
                tick_arity,
                on_resize_arity,
                on_resize_w_i64,
                on_resize_h_i64,
            });
            fs::write(&html_path, index_html).map_err(CliError::Io)?;
            Ok(())
        }
        BuildTarget::Native => {
            if gui {
                if module.get_function("init").is_none() || module.get_function("view").is_none() {
                    return Err(CliError::Other(
                        "Native GUI build (--gui) requires both `init()` and `view(state)` functions"
                            .to_string(),
                    ));
                }

                let compiled_shaders = compile_shaders_for_build(file, is_source, &module)?;
                let mut shader_map = std::collections::BTreeMap::new();
                for cs in compiled_shaders {
                    shader_map.insert(cs.name, cs.wgsl);
                }
                let shaders_json =
                    serde_json::to_string(&shader_map).unwrap_or_else(|_| "{}".to_string());

                let rust_rt_lib = find_rust_runtime_staticlib().ok_or_else(|| {
                    CliError::Linker(
                        "libknr_runtime.a is required for --target native --gui; build knr-runtime first"
                            .to_string(),
                    )
                })?;

                let mut backend = knr_codegen::CraneliftBackend::new().map_err(|err| {
                    eprintln!("{err}");
                    CliError::Codegen(err)
                })?;
                let obj_bytes = backend.compile_object(&module).map_err(|err| {
                    eprintln!("{err}");
                    CliError::Codegen(err)
                })?;

                let compiler = find_c_compiler().ok_or_else(|| {
                    CliError::Linker(
                        "No C compiler (cc, gcc, clang) found in PATH for native linking"
                            .to_string(),
                    )
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

                let title = file
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("knr native app");
                let gui_shim_c = generate_native_gui_shim_c(&module, title, &shaders_json);
                let shim_path = temp_dir.join("gui_entry_shim.c");
                fs::write(&shim_path, gui_shim_c).map_err(CliError::Io)?;

                let mut cmd = Command::new(compiler);
                cmd.arg("-O2")
                    .arg(&obj_path)
                    .arg(&shim_path)
                    .arg(&rust_rt_lib)
                    .arg("-o")
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
                        "Metal",
                        "QuartzCore",
                    ] {
                        cmd.arg("-framework").arg(fw);
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

                return Ok(());
            }

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

fn escape_c_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 16);
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 || (c as u32) == 0x7f => {
                use std::fmt::Write as _;
                let _ = write!(out, "\\{:03o}", c as u32);
            }
            c => out.push(c),
        }
    }
    out
}

fn generate_native_gui_shim_c(
    module: &knr_core::Module,
    title: &str,
    shaders_json: &str,
) -> String {
    let has_update = module.get_function("update").is_some();
    let tick_arity = module
        .get_function("tick")
        .map(|f| f.params.len())
        .unwrap_or(0);
    let has_on_slider = module.get_function("on_slider").is_some();
    let has_on_input = module.get_function("on_input").is_some();

    let mut externs =
        String::from("extern int64_t init(void);\nextern int64_t view(int64_t);\n");
    if has_update {
        externs.push_str("extern int64_t update(int64_t, const char*);\n");
    }
    if tick_arity == 2 {
        externs.push_str("extern int64_t tick(int64_t, double);\n");
    } else if tick_arity >= 3 {
        externs.push_str("extern int64_t tick(int64_t, double, double);\n");
    }
    if has_on_slider {
        externs.push_str("extern int64_t on_slider(int64_t, const char*, double);\n");
    }
    if has_on_input {
        externs.push_str("extern int64_t on_input(int64_t, const char*, const char*);\n");
    }

    let update_arg = if has_update { "update" } else { "NULL" };
    let tick2_arg = if tick_arity == 2 { "tick" } else { "NULL" };
    let tick3_arg = if tick_arity >= 3 { "tick" } else { "NULL" };
    let slider_arg = if has_on_slider { "on_slider" } else { "NULL" };
    let input_arg = if has_on_input { "on_input" } else { "NULL" };

    let escaped_title = escape_c_string(title);
    let escaped_shaders = escape_c_string(shaders_json);

    format!(
        r#"#include <stddef.h>
#include <stdint.h>

typedef int64_t (*knr_init_fn)(void);
typedef int64_t (*knr_view_fn)(int64_t);
typedef int64_t (*knr_update_fn)(int64_t, const char*);
typedef int64_t (*knr_tick2_fn)(int64_t, double);
typedef int64_t (*knr_tick3_fn)(int64_t, double, double);
typedef int64_t (*knr_on_slider_fn)(int64_t, const char*, double);
typedef int64_t (*knr_on_input_fn)(int64_t, const char*, const char*);

extern int64_t rt_gui_launch_native(
    const char* title,
    uint32_t width,
    uint32_t height,
    knr_init_fn init_fn,
    knr_view_fn view_fn,
    knr_update_fn update_fn,
    knr_tick2_fn tick2_fn,
    knr_tick3_fn tick3_fn,
    knr_on_slider_fn on_slider_fn,
    knr_on_input_fn on_input_fn,
    const char* shaders_json
);

{externs}
static const char* KNR_GUI_TITLE = "{escaped_title}";
static const char* KNR_SHADERS_JSON = "{escaped_shaders}";

int main(void) {{
    return (int)rt_gui_launch_native(
        KNR_GUI_TITLE,
        1120u,
        800u,
        init,
        view,
        {update_arg},
        {tick2_arg},
        {tick3_arg},
        {slider_arg},
        {input_arg},
        KNR_SHADERS_JSON
    );
}}
"#
    )
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

struct WebHostConfig<'a> {
    title: &'a str,
    wasm_file: &'a str,
    wasm_b64: &'a str,
    shaders_json: &'a str,
    is_widget_v2: bool,
    style_field_count: usize,
    tick_arity: usize,
    on_resize_arity: usize,
    on_resize_w_i64: bool,
    on_resize_h_i64: bool,
}

/// Generates a minimal, Flutter-style Unified Single-Canvas + WebGPU/WebGL2 Web Host (ZERO HTML DOM UI!).
/// - Single fullscreen `<canvas id="knr-surface">` + hidden transparent `<input>` IME bridge
/// - Zero-import WASM execution with `__knr_heap` arena reset on every frame (zero memory growth)
/// - 8-field `Widget` + 45-field `Style` (including `fontFamily`, `textAlign`, `lineHeight`, `colSpan`, `letterSpacing`)
/// - 2-pass Flexbox + Bento Grid (`colSpan`) + Stack + True `z_index` Paint & Hit-Testing
/// - Full 67 Crisp Vector Icons + Keyboard Focus Ring + Global `on_key` / `on_resize` + H/V Scrollbars
/// - Full 16-Uniform WebGPU + WebGL2 GLSL ES 3.00 Shader Compositing + Toggleable Zero-GC Telemetry HUD (`Alt+Shift+D`)
fn generate_web_host_html(cfg: &WebHostConfig<'_>) -> String {
    let escaped_title = knr_runtime::ui::escape_html_text(cfg.title);
    let safe_shaders_json = cfg.shaders_json.replace("</script", "<\\/script");
    let wasm_url_json =
        serde_json::to_string(cfg.wasm_file).unwrap_or_else(|_| "\"app.wasm\"".into());

    WEB_HOST_TEMPLATE
        .replace("__KNR_TITLE__", &escaped_title)
        .replace("__KNR_WASM_URL__", &wasm_url_json)
        .replace("__KNR_WASM_B64__", cfg.wasm_b64)
        .replace(
            "__KNR_IS_WIDGET_V2__",
            if cfg.is_widget_v2 { "true" } else { "false" },
        )
        .replace("__KNR_STYLE_FIELDS__", &cfg.style_field_count.to_string())
        .replace("__KNR_TICK_ARITY__", &cfg.tick_arity.to_string())
        .replace(
            "__KNR_ON_RESIZE_ARITY__",
            &cfg.on_resize_arity.to_string(),
        )
        .replace(
            "__KNR_ON_RESIZE_W_I64__",
            if cfg.on_resize_w_i64 { "true" } else { "false" },
        )
        .replace(
            "__KNR_ON_RESIZE_H_I64__",
            if cfg.on_resize_h_i64 { "true" } else { "false" },
        )
        .replace("__KNR_SHADERS_JSON__", &safe_shaders_json)
}

const WEB_HOST_TEMPLATE: &str = r##"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8" />
<meta name="viewport" content="width=device-width, initial-scale=1, maximum-scale=1" />
<title>__KNR_TITLE__ — knr</title>
<link rel="preconnect" href="https://fonts.googleapis.com" />
<link rel="preconnect" href="https://fonts.gstatic.com" crossorigin />
<link rel="stylesheet" href="https://fonts.googleapis.com/css2?family=Inter:wght@400;500;600;700;800&family=JetBrains+Mono:wght@400;500;600&display=swap" />
<style>
  *, *::before, *::after { box-sizing: border-box; margin: 0; padding: 0; }
  html, body { width: 100%; height: 100%; overflow: hidden; background: #050811; user-select: none; -webkit-user-select: none; touch-action: none; }
  #knr-surface { display: block; width: 100vw; height: 100vh; }
  #knr-ime-input {
    position: fixed; left: -9999px; top: -9999px; width: 1px; height: 1px;
    opacity: 0; border: none; outline: none; background: transparent;
    color: transparent; caret-color: transparent; font-size: 16px;
    pointer-events: none; z-index: 10;
  }
</style>
</head>
<body>
<canvas id="knr-surface"></canvas>
<input id="knr-ime-input" type="text" autocomplete="off" spellcheck="false" aria-label="Text input" />
<script data-knr-web-host="v3-canvas" data-knr-renderer="v4-canvas">
(async function() {
  const WASM_URL = __KNR_WASM_URL__;
  const WASM_B64 = "__KNR_WASM_B64__";
  const IS_WIDGET_V2 = __KNR_IS_WIDGET_V2__;
  const STYLE_FIELDS = __KNR_STYLE_FIELDS__;
  const TICK_ARITY = __KNR_TICK_ARITY__;
  const ON_RESIZE_ARITY = __KNR_ON_RESIZE_ARITY__;
  const ON_RESIZE_W_I64 = __KNR_ON_RESIZE_W_I64__;
  const ON_RESIZE_H_I64 = __KNR_ON_RESIZE_H_I64__;
  const KNR_SHADERS = __KNR_SHADERS_JSON__;

  const SANS_FONT = '"Inter", -apple-system, BlinkMacSystemFont, "SF Pro Text", "Segoe UI", system-ui, sans-serif';
  const MONO_FONT = '"JetBrains Mono", "Adwaita Mono", ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace';

  const surface = document.getElementById("knr-surface");
  const ctx = surface.getContext("2d", { alpha: false });
  const imeInput = document.getElementById("knr-ime-input");
  const decoder = new TextDecoder();
  const encoder = new TextEncoder();

  // ========================================================================
  // 1. Zero-Import WASM Loader & 45-Field Linear Memory Protocol
  // ========================================================================
  async function loadWasm() {
    if (location.protocol && location.protocol.startsWith("http")) {
      try {
        const res = await fetch(WASM_URL);
        if (res.ok) {
          const buf = await res.arrayBuffer();
          return (await WebAssembly.instantiate(buf, {})).instance.exports;
        }
      } catch (_) {}
    }
    const bin = atob(WASM_B64);
    const bytes = new Uint8Array(bin.length);
    for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i);
    return (await WebAssembly.instantiate(bytes.buffer, {})).instance.exports;
  }

  function readStr(mem, ptr) {
    if (!ptr || ptr === 0n) return "";
    const p = Number(ptr);
    const dv = new DataView(mem.buffer);
    const len = Number(dv.getBigUint64(p - 8, true));
    return decoder.decode(new Uint8Array(mem.buffer, p, len));
  }

  function allocStr(exports, str) {
    const bytes = encoder.encode(str);
    const raw = Number(exports.__knr_alloc(bytes.length + 9));
    const dv = new DataView(exports.memory.buffer);
    dv.setBigUint64(raw, BigInt(bytes.length), true);
    const u8 = new Uint8Array(exports.memory.buffer);
    u8.set(bytes, raw + 8);
    u8[raw + 8 + bytes.length] = 0;
    return BigInt(raw + 8);
  }

  function defaultStyle() {
    return {
      w: 0, h: 0, minW: 0, maxW: 0,
      bg: "", color: "#f8fafc", border: "", borderW: 1,
      radii: [0, 0, 0, 0], pad: [0, 0, 0, 0],
      gapX: 0, gapY: 0, fontSize: 15, fontWeight: 400,
      align: "stretch", justify: "start", gridCols: 0,
      shadowX: 0, shadowY: 0, shadowBlur: 0, shadowColor: "",
      backdropBlur: 0, opacity: 1,
      tx: 0, ty: 0, scale: 1, rotateDeg: 0,
      hoverBg: "", hoverBorder: "", hoverScale: 1, transitionMs: 0,
      overflow: "visible", zIndex: 0, grow: 0,
      fontFamily: "sans", textAlign: "left", lineHeight: 0, colSpan: 1, letterSpacing: 0,
    };
  }

  function readStyleV2(mem, ptr) {
    if (!ptr || ptr === 0n) return defaultStyle();
    const p = Number(ptr);
    const dv = new DataView(mem.buffer);
    const f = (i) => dv.getFloat64(p + i * 8, true);
    const s = (i) => readStr(mem, dv.getBigInt64(p + i * 8, true));
    const n = (i) => Number(dv.getBigInt64(p + i * 8, true));
    const opacityRaw = f(28);
    const scaleRaw = f(31);
    const hoverScaleRaw = f(35);
    return {
      w: f(0), h: f(1), minW: f(2), maxW: f(3),
      bg: s(4), color: s(5) || "#f8fafc", border: s(6), borderW: f(7) > 0 ? f(7) : 1,
      radii: [Math.max(0, f(8)), Math.max(0, f(9)), Math.max(0, f(10)), Math.max(0, f(11))],
      pad: [Math.max(0, f(12)), Math.max(0, f(13)), Math.max(0, f(14)), Math.max(0, f(15))],
      gapX: Math.max(0, f(16)), gapY: Math.max(0, f(17)),
      fontSize: f(18) > 0 ? f(18) : 15,
      fontWeight: n(19) > 0 ? n(19) : 400,
      align: s(20) || "stretch",
      justify: s(21) || "start",
      gridCols: n(22),
      shadowX: f(23), shadowY: f(24), shadowBlur: Math.max(0, f(25)), shadowColor: s(26),
      backdropBlur: Math.max(0, f(27)),
      opacity: opacityRaw > 0 ? Math.min(1, opacityRaw) : 1,
      tx: f(29), ty: f(30),
      scale: scaleRaw > 0 ? scaleRaw : 1,
      rotateDeg: f(32),
      hoverBg: s(33), hoverBorder: s(34),
      hoverScale: hoverScaleRaw > 0 ? hoverScaleRaw : 1,
      transitionMs: Math.max(0, f(36)),
      overflow: s(37) || "visible",
      zIndex: n(38),
      grow: Math.max(0, f(39)),
      fontFamily: STYLE_FIELDS >= 41 ? (s(40) || "sans") : "sans",
      textAlign: STYLE_FIELDS >= 42 ? (s(41) || "left") : "left",
      lineHeight: STYLE_FIELDS >= 43 ? Math.max(0, f(42)) : 0,
      colSpan: STYLE_FIELDS >= 44 ? Math.max(1, n(43)) : 1,
      letterSpacing: STYLE_FIELDS >= 45 ? f(44) : 0,
    };
  }

  function normalizeKind(raw) {
    const k = (raw || "col").toLowerCase();
    if (k === "zstack") return "stack";
    if (k === "box") return "card";
    if (k === "btn") return "button";
    if (k === "pill") return "badge";
    if (k === "h1" || k === "h2" || k === "p" || k === "span") return "text";
    if (k === "textinput" || k === "text_input") return "input";
    if (k === "range") return "slider";
    if (k === "switch" || k === "checkbox") return "toggle";
    if (k === "scrollview") return "scroll";
    if (k === "img") return "image";
    if (k === "shaderview" || k === "canvas") return "shader";
    if (k === "hr") return "divider";
    if (k === "progressbar") return "progress";
    return k;
  }

  function readWidgetV2(mem, ptr) {
    const p = Number(ptr);
    const dv = new DataView(mem.buffer);
    const key = readStr(mem, dv.getBigInt64(p + 0, true));
    const kind = normalizeKind(readStr(mem, dv.getBigInt64(p + 8, true)));
    const text = readStr(mem, dv.getBigInt64(p + 16, true));
    const action = readStr(mem, dv.getBigInt64(p + 24, true));
    const value = dv.getFloat64(p + 32, true);
    const uPtr = Number(dv.getBigInt64(p + 40, true));
    const uniforms = [];
    if (uPtr > 0) {
      const uLen = Number(dv.getBigUint64(uPtr + 0, true));
      const uData = Number(dv.getBigUint64(uPtr + 16, true));
      for (let i = 0; i < uLen && i < 16; i++) uniforms.push(dv.getFloat64(uData + i * 8, true));
    }
    const style = readStyleV2(mem, dv.getBigInt64(p + 48, true));
    const cPtr = Number(dv.getBigInt64(p + 56, true));
    const children = [];
    if (cPtr > 0) {
      const cLen = Number(dv.getBigUint64(cPtr + 0, true));
      const cData = Number(dv.getBigUint64(cPtr + 16, true));
      for (let i = 0; i < cLen; i++) {
        const cp = dv.getBigInt64(cData + i * 8, true);
        if (cp !== 0n) children.push(readWidgetV2(mem, cp));
      }
    }
    return { key, kind, text, action, value, uniforms, style, children };
  }

  function readUiNodeV1(mem, ptr) {
    const p = Number(ptr);
    const dv = new DataView(mem.buffer);
    const kind = normalizeKind(readStr(mem, dv.getBigInt64(p + 0, true)));
    const text = readStr(mem, dv.getBigInt64(p + 8, true));
    const action = readStr(mem, dv.getBigInt64(p + 16, true));
    const bg = readStr(mem, dv.getBigInt64(p + 24, true));
    const color = readStr(mem, dv.getBigInt64(p + 32, true)) || "#f8fafc";
    const border = readStr(mem, dv.getBigInt64(p + 40, true));
    const radius = Math.max(0, Number(dv.getBigInt64(p + 48, true)));
    const pad = Math.max(0, Number(dv.getBigInt64(p + 56, true)));
    const gap = Math.max(0, Number(dv.getBigInt64(p + 64, true)));
    const fsRaw = Number(dv.getBigInt64(p + 72, true));
    const fontSize = fsRaw > 0 ? fsRaw : 16;
    const vecPtr = Number(dv.getBigInt64(p + 80, true));
    const children = [];
    if (vecPtr > 0) {
      const len = Number(dv.getBigUint64(vecPtr + 0, true));
      const dataPtr = Number(dv.getBigUint64(vecPtr + 16, true));
      for (let i = 0; i < len; i++) {
        const cp = dv.getBigInt64(dataPtr + i * 8, true);
        if (cp !== 0n) children.push(readUiNodeV1(mem, cp));
      }
    }
    const style = defaultStyle();
    style.bg = bg; style.color = color; style.border = border;
    style.radii = [radius, radius, radius, radius];
    style.pad = [pad, pad, pad, pad];
    style.gapX = gap; style.gapY = gap; style.fontSize = fontSize;
    return { key: action, kind, text, action, value: 0, uniforms: [], style, children };
  }

  // ========================================================================
  // 2. Color & Gradient Helpers
  // ========================================================================
  function parseHexRgba(hex) {
    if (!hex || typeof hex !== "string") return null;
    const s = hex.trim();
    if (!s.startsWith("#")) return null;
    const h = s.slice(1);
    if (h.length === 3) {
      return [parseInt(h[0], 16) * 17, parseInt(h[1], 16) * 17, parseInt(h[2], 16) * 17, 1];
    }
    if (h.length === 6) {
      return [parseInt(h.slice(0, 2), 16), parseInt(h.slice(2, 4), 16), parseInt(h.slice(4, 6), 16), 1];
    }
    if (h.length === 8) {
      return [
        parseInt(h.slice(0, 2), 16),
        parseInt(h.slice(2, 4), 16),
        parseInt(h.slice(4, 6), 16),
        parseInt(h.slice(6, 8), 16) / 255,
      ];
    }
    return null;
  }

  function rgbaStr(c, alphaMul = 1) {
    return `rgba(${Math.round(c[0])},${Math.round(c[1])},${Math.round(c[2])},${(c[3] * alphaMul).toFixed(4)})`;
  }

  function makeCanvasFill(ctx, spec, rect, brighten = false) {
    if (!spec) return null;
    const s = spec.trim();
    if (!s) return null;
    const solid = parseHexRgba(s);
    if (solid) {
      if (brighten) {
        return rgbaStr([Math.min(255, solid[0] + 18), Math.min(255, solid[1] + 18), Math.min(255, solid[2] + 18), solid[3]]);
      }
      return rgbaStr(solid);
    }
    if (s.startsWith("linear(") && s.endsWith(")")) {
      const parts = s.slice(7, -1).split(",").map(x => x.trim());
      if (parts.length >= 3) {
        const angle = parseFloat(parts[0].replace(/deg$/i, "")) || 135;
        const rad = (angle - 90) * Math.PI / 180;
        const dx = Math.cos(rad), dy = Math.sin(rad);
        const cx = rect.x + rect.w * 0.5, cy = rect.y + rect.h * 0.5;
        const half = (Math.abs(rect.w * dx) + Math.abs(rect.h * dy)) * 0.5;
        const grad = ctx.createLinearGradient(cx - dx * half, cy - dy * half, cx + dx * half, cy + dy * half);
        const c0 = parseHexRgba(parts[1]) || [18, 26, 46, 1];
        const c1 = parseHexRgba(parts[2]) || [12, 18, 34, 1];
        grad.addColorStop(0, rgbaStr(brighten ? [Math.min(255, c0[0] + 14), Math.min(255, c0[1] + 14), Math.min(255, c0[2] + 14), c0[3]] : c0));
        grad.addColorStop(1, rgbaStr(brighten ? [Math.min(255, c1[0] + 14), Math.min(255, c1[1] + 14), Math.min(255, c1[2] + 14), c1[3]] : c1));
        return grad;
      }
    }
    if (s.startsWith("radial(") && s.endsWith(")")) {
      const parts = s.slice(7, -1).split(",").map(x => x.trim());
      if (parts.length >= 2) {
        const cx = rect.x + rect.w * 0.5, cy = rect.y + rect.h * 0.5;
        const r = Math.max(1, Math.max(rect.w, rect.h) * 0.6);
        const grad = ctx.createRadialGradient(cx, cy, 0, cx, cy, r);
        const c0 = parseHexRgba(parts[0]) || [56, 189, 248, 0.35];
        const c1 = parseHexRgba(parts[1]) || [10, 18, 33, 1];
        grad.addColorStop(0, rgbaStr(c0));
        grad.addColorStop(1, rgbaStr(c1));
        return grad;
      }
    }
    return s;
  }

  function roundRectPath(ctx, x, y, w, h, radii) {
    const maxR = Math.max(0, Math.min(w * 0.5, h * 0.5));
    const r0 = Math.min(maxR, Math.max(0, radii[0] || 0));
    const r1 = Math.min(maxR, Math.max(0, radii[1] || 0));
    const r2 = Math.min(maxR, Math.max(0, radii[2] || 0));
    const r3 = Math.min(maxR, Math.max(0, radii[3] || 0));
    ctx.beginPath();
    ctx.moveTo(x + r0, y);
    ctx.lineTo(x + w - r1, y);
    if (r1 > 0.2) ctx.quadraticCurveTo(x + w, y, x + w, y + r1);
    ctx.lineTo(x + w, y + h - r2);
    if (r2 > 0.2) ctx.quadraticCurveTo(x + w, y + h, x + w - r2, y + h);
    ctx.lineTo(x + r3, y + h);
    if (r3 > 0.2) ctx.quadraticCurveTo(x, y + h, x, y + h - r3);
    ctx.lineTo(x, y + r0);
    if (r0 > 0.2) ctx.quadraticCurveTo(x, y, x + r0, y);
    ctx.closePath();
  }

  // ========================================================================
  // 3. Full 67 Crisp Vector Icons (24x24 Lucide-Style Path2D Matching renderer.rs 1:1)
  // ========================================================================
  const ICON_PATHS = {
    sparkles: ["M12 3l1.6 4.4L18 9l-4.4 1.6L12 15l-1.6-4.4L6 9l4.4-1.6L12 3z", "M19 15l.8 2.2L22 18l-2.2.8L19 21l-.8-2.2L16 18l2.2-.8L19 15z"],
    sparkle: ["M12 3l1.8 5.2L19 10l-5.2 1.8L12 17l-1.8-5.2L5 10l5.2-1.8L12 3z"],
    zap: ["M13 2L4 14h7l-1 8 9-12h-7l1-8z"],
    bolt: ["M13 2L4 14h7l-1 8 9-12h-7l1-8z"],
    shield: ["M12 22s8-4 8-10V5l-8-3-8 3v7c0 6 8 10 8 10z"],
    "shield-check": ["M12 22s8-4 8-10V5l-8-3-8 3v7c0 6 8 10 8 10z", "M9 12l2 2 4-4"],
    cpu: ["M6 6h12v12H6z", "M9 9h6v6H9z", "M9 2v4M15 2v4M9 18v4M15 18v4M2 9h4M2 15h4M18 9h4M18 15h4"],
    code: ["M16 18l6-6-6-6", "M8 6l-6 6 6 6", "M13.8 4.5l-3.6 15"],
    "code-2": ["M18 16l4-4-4-4", "M6 8l-4 4 4 4", "M14.5 4l-5 16"],
    terminal: ["M4 17l6-5-6-5", "M12 19h8"],
    play: ["M6 4l14 8-14 8V4z"],
    pause: ["M6 4h4v16H6z", "M14 4h4v16h-4z"],
    refresh: ["M21 12a9 9 0 1 1-2.64-6.36", "M21 3v6h-6"],
    "rotate-cw": ["M21 12a9 9 0 1 1-2.64-6.36", "M21 3v6h-6"],
    plus: ["M12 5v14", "M5 12h14"],
    minus: ["M5 12h14"],
    check: ["M20 6L9 17l-5-5"],
    "check-circle": ["M22 11.08V12a10 10 0 1 1-5.93-9.14", "M22 4L12 14.01l-3-3"],
    x: ["M18 6L6 18", "M6 6l12 12"],
    close: ["M18 6L6 18", "M6 6l12 12"],
    "arrow-right": ["M5 12h14", "M13 5l7 7-7 7"],
    "arrow-left": ["M19 12H5", "M11 19l-7-7 7-7"],
    "arrow-up": ["M12 19V5", "M5 12l7-7 7 7"],
    "arrow-down": ["M12 5v14", "M19 12l-7 7-7-7"],
    "chevron-left": ["M15 18l-6-6 6-6"],
    "chevron-right": ["M9 18l6-6-6-6"],
    "chevron-up": ["M18 15l-6-6-6 6"],
    "chevron-down": ["M6 9l6 6 6-6"],
    layers: ["M12 2L2 7l10 5 10-5-10-5z", "M2 17l10 5 10-5", "M2 12l10 5 10-5"],
    box: ["M21 16V8a2 2 0 0 0-1-1.73l-7-4a2 2 0 0 0-2 0l-7 4A2 2 0 0 0 3 8v8a2 2 0 0 0 1 1.73l7 4a2 2 0 0 0 2 0l7-4A2 2 0 0 0 21 16z", "M3.27 6.96L12 12.01l8.73-5.05", "M12 22.08V12"],
    cube: ["M21 16V8l-9-5-9 5v8l9 5 9-5z", "M3.27 6.96L12 12l8.73-5.04", "M12 22V12"],
    globe: ["M12 2a10 10 0 1 0 0 20 10 10 0 0 0 0-20z", "M2 12h20", "M12 2a15.3 15.3 0 0 1 4 10 15.3 15.3 0 0 1-4 10 15.3 15.3 0 0 1-4-10 15.3 15.3 0 0 1 4-10z"],
    sliders: ["M4 21v-7M4 10V3M12 21v-9M12 8V3M20 21v-5M20 12V3M1 14h6M9 8h6M17 16h6"],
    settings: ["M12 15a3 3 0 1 0 0-6 3 3 0 0 0 0 6z", "M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 1 1-4 0v-.09a1.65 1.65 0 0 0-1-1.51 1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 1 1 0-4h.09a1.65 1.65 0 0 0 1.51-1 1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 1 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 1 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z"],
    search: ["M11 19a8 8 0 1 0 0-16 8 8 0 0 0 0 16z", "M21 21l-4.35-4.35"],
    trash: ["M3.5 6h17", "M9 6V3.8h6V6", "M5.5 6l1.1 13.8a1.6 1.6 0 0 0 1.6 1.4h7.6a1.6 1.6 0 0 0 1.6-1.4L18.5 6", "M10 10.2v7", "M14 10.2v7"],
    edit: ["M16.2 3.8l4 4-11.4 11.4-5 1 1-5 11.4-11.4z", "M13.8 6.2l4 4"],
    download: ["M12 3v12", "M7.5 10.8L12 15.3l4.5-4.5", "M4 16.5v3a1.5 1.5 0 0 0 1.5 1.5h13a1.5 1.5 0 0 0 1.5-1.5v-3"],
    upload: ["M12 15.5v-12", "M7.5 7.8L12 3.3l4.5 4.5", "M4 16.5v3a1.5 1.5 0 0 0 1.5 1.5h13a1.5 1.5 0 0 0 1.5-1.5v-3"],
    eye: ["M1 12s4-8 11-8 11 8 11 8-4 8-11 8-11-8-11-8z", "M12 9a3 3 0 1 0 0 6 3 3 0 0 0 0-6z"],
    lock: ["M5 11h14v10H5z", "M7 11V7a5 5 0 0 1 10 0v4"],
    unlock: ["M5 11h14v10H5z", "M8 11V7.2a4.1 4.1 0 0 1 8.2-.8"],
    flame: ["M12 2c1 4 5 6 5 11a5 5 0 1 1-10 0c0-3 2-5 3-7 0 2 1 3 2 3 0-3 0-5 0-7z"],
    star: ["M12 2l3.09 6.26L22 9.27l-5 4.87 1.18 6.88L12 17.77l-6.18 3.25L7 14.14 2 9.27l6.91-1.01L12 2z"],
    heart: ["M20.84 4.61a5.5 5.5 0 0 0-7.78 0L12 5.67l-1.06-1.06a5.5 5.5 0 0 0-7.78 7.78l1.06 1.06L12 21.23l7.78-7.78 1.06-1.06a5.5 5.5 0 0 0 0-7.78z"],
    bookmark: ["M6 4.2A1.4 1.4 0 0 1 7.5 2.8h9a1.4 1.4 0 0 1 1.5 1.4V21l-6-4.2L6 21V4.2z"],
    filter: ["M3 4.2h18l-7 8.4v6.8l-4 1.8v-8.6L3 4.2z"],
    external: ["M14.5 3.5h6v6", "M10.5 13.5l10-10", "M18.5 13.5V19a1.5 1.5 0 0 1-1.6 1.5H5.1A1.5 1.5 0 0 1 3.5 19V7.1a1.5 1.5 0 0 1 1.6-1.6h5.4"],
    sun: ["M12 17a5 5 0 1 0 0-10 5 5 0 0 0 0 10z", "M12 1v2M12 21v2M4.22 4.22l1.42 1.42M18.36 18.36l1.42 1.42M1 12h2M21 12h2M4.22 19.78l1.42-1.42M18.36 5.64l1.42-1.42"],
    moon: ["M21 12.79A9 9 0 1 1 11.21 3 7 7 0 0 0 21 12.79z"],
    wifi: ["M2.6 9.2a13.4 13.4 0 0 1 18.8 0", "M5.6 12.6a9.2 9.2 0 0 1 12.8 0", "M8.8 15.8a4.6 4.6 0 0 1 6.4 0", "M12 18.1a1.1 1.1 0 1 0 0 2.2 1.1 1.1 0 1 0 0-2.2z"],
    bell: ["M18.2 14.5V9.6a6.2 6.2 0 1 0-12.4 0v4.9l-2 2.7h16.4l-2-2.7z", "M10 19.8a2.2 2.2 0 0 0 4 0"],
    mail: ["M3 5.2h18a1.5 1.5 0 0 1 0 13.8H3a1.5 1.5 0 0 1 0-13.8z", "M3.6 6.2L12 12.8l8.4-6.6"],
    user: ["M12 3.8a3.8 3.8 0 1 0 0 7.6 3.8 3.8 0 1 0 0-7.6z", "M4.4 20.6c0-4.8 3.4-6.6 7.6-6.6s7.6 1.8 7.6 6.6"],
    users: ["M9.2 4.8a3.2 3.2 0 1 0 0 6.4 3.2 3.2 0 1 0 0-6.4z", "M2.8 20.2c0-4.2 2.8-5.8 6.4-5.8s6.4 1.6 6.4 5.8", "M16.4 6.2a2.4 2.4 0 1 0 0 4.8 2.4 2.4 0 1 0 0-4.8z", "M16.5 14.6c2.7.2 4.7 1.8 4.7 5.6"],
    monitor: ["M2 3h20v14H2z", "M8 21h8", "M12 17v4"],
    rocket: ["M4.5 16.5c-1.5 1.26-2 5-2 5s3.74-.5 5-2c.71-.84.7-2.13-.09-2.91a2.18 2.18 0 0 0-2.91-.09z", "M12 15l-3-3a22 22 0 0 1 2-3.95A12.88 12.88 0 0 1 22 2c0 2.72-.78 7.5-6 11a22.35 22.35 0 0 1-4 2z"],
    info: ["M12 22a10 10 0 1 0 0-20 10 10 0 0 0 0 20z", "M12 16v-4.5", "M12 7a.9.9 0 1 0 0 1.8.9.9 0 1 0 0-1.8z"],
    alert: ["M10.29 3.86L1.82 18a2 2 0 0 0 1.71 3h16.94a2 2 0 0 0 1.71-3L13.71 3.86a2 2 0 0 0-3.42 0z", "M12 9v4.5", "M12 16.2a.9.9 0 1 0 0 1.8.9.9 0 1 0 0-1.8z"],
    calendar: ["M3.2 5.2h17.6v15.8H3.2z", "M3.2 10.2h17.6", "M8 2.8v4", "M16 2.8v4", "M8.2 13.4a.8.8 0 1 0 0 1.6.8.8 0 1 0 0-1.6z", "M12 13.4a.8.8 0 1 0 0 1.6.8.8 0 1 0 0-1.6z", "M15.8 13.4a.8.8 0 1 0 0 1.6.8.8 0 1 0 0-1.6z"],
    clock: ["M12 2.8a9.2 9.2 0 1 0 0 18.4 9.2 9.2 0 1 0 0-18.4z", "M12 6.6V12l4.2 2.2"],
    chart: ["M3.5 4v16h17", "M8.2 17.8v-4.8", "M12.8 17.8V9.2", "M17.4 17.8V5.6"],
    database: ["M4 5.8a8 2.8 0 1 0 16 0 8 2.8 0 1 0-16 0z", "M4 5.8v12.4c0 1.55 3.58 2.8 8 2.8s8-1.25 8-2.8V5.8", "M4 12c0 1.55 3.58 2.8 8 2.8s8-1.25 8-2.8"],
    git: ["M12 2.8L21.2 12 12 21.2 2.8 12 12 2.8z", "M9.5 7.2l5.3 5.3", "M11.4 9.2v6.6", "M11.4 14.9a.9.9 0 1 0 0 1.8.9.9 0 1 0 0-1.8z", "M14.8 11.6a.9.9 0 1 0 0 1.8.9.9 0 1 0 0-1.8z"],
    "git-branch": ["M6 3v12", "M18 9a3 3 0 1 0 0-6 3 3 0 0 0 0 6z", "M6 21a3 3 0 1 0 0-6 3 3 0 0 0 0 6z", "M18 9a9 9 0 0 1-9 9"],
    copy: ["M9 9h13v13H9z", "M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"],
    file: ["M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z", "M14 2v6h6"],
    folder: ["M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z"],
    home: ["M3 9l9-7 9 7v11a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z", "M9 22V12h6v10"],
    menu: ["M3 12h18", "M3 6h18", "M3 18h18"],
    more: ["M5.5 10.7a1.3 1.3 0 1 0 0 2.6 1.3 1.3 0 1 0 0-2.6z", "M12 10.7a1.3 1.3 0 1 0 0 2.6 1.3 1.3 0 1 0 0-2.6z", "M18.5 10.7a1.3 1.3 0 1 0 0 2.6 1.3 1.3 0 1 0 0-2.6z"],
    send: ["M21 3.2l-6.8 17.6-3.6-7.4L3.2 9.8 21 3.2z", "M21 3.2l-10.4 10.2"],
    inbox: ["M5.5 4.5h13L21 13v5.8a1.6 1.6 0 0 1-1.6 1.6H4.6A1.6 1.6 0 0 1 3 18.8V13l2.5-8.5z", "M3.2 13h5.2l1.4 2.8h4.4L15.6 13h5.2"],
    tag: ["M11.6 3.4H4.8a1.4 1.4 0 0 0-1.4 1.4v6.8l8.8 8.8a1.8 1.8 0 0 0 2.5 0l5.7-5.7a1.8 1.8 0 0 0 0-2.5l-8.8-8.8z", "M7.6 6.6a1 1 0 1 0 0 2 1 1 0 1 0 0-2z"],
    gpu: ["M4 6h16v12H4z", "M9 12a3 3 0 1 0 6 0 3 3 0 0 0-6 0z", "M2 10h2M2 14h2M20 10h2M20 14h2"],
    palette: ["M12 2a10 10 0 1 0 0 20 2.5 2.5 0 0 0 2.5-2.5c0-.6-.2-1.1-.6-1.5-.4-.4-.6-.9-.6-1.5A2.5 2.5 0 0 1 15.8 14H18a4 4 0 0 0 4-4c0-4.4-4.5-8-10-8z"],
    gauge: ["M12 14l4-4", "M3.34 19a10 10 0 1 1 17.32 0"],
    command: ["M18 3a3 3 0 0 0-3 3v12a3 3 0 0 0 3 3 3 3 0 0 0 3-3 3 3 0 0 0-3-3H6a3 3 0 0 0-3 3 3 3 0 0 0 3 3 3 3 0 0 0 3-3V6a3 3 0 0 0-3-3 3 3 0 0 0-3 3 3 3 0 0 0 3 3h12a3 3 0 0 0 3-3 3 3 0 0 0-3-3z"],
    activity: ["M22 12h-4l-3 9L9 3l-3 9H2"],
    diamond: ["M12 2.5L21.2 12 12 21.5 2.8 12 12 2.5z", "M12 7.5L16.5 12 12 16.5 7.5 12 12 7.5z"],
  };
  const ICON_CACHE = new Map();
  function getIconPaths(name) {
    const key = (name || "").toLowerCase().trim().replace(/_/g, "-");
    if (ICON_CACHE.has(key)) return ICON_CACHE.get(key);
    const defs = ICON_PATHS[key] || ["M12 4.2a7.8 7.8 0 1 0 0 15.6 7.8 7.8 0 0 0 0-15.6z", "M12 10.2a1.8 1.8 0 1 0 0 3.6 1.8 1.8 0 0 0 0-3.6z"];
    const paths = defs.map(d => new Path2D(d));
    ICON_CACHE.set(key, paths);
    return paths;
  }

  function drawVectorIcon(ctx, name, x, y, size, color) {
    const paths = getIconPaths(name);
    const scale = Math.max(8, size) / 24;
    ctx.save();
    ctx.translate(x, y);
    ctx.scale(scale, scale);
    ctx.strokeStyle = color || "#94a3b8";
    ctx.lineWidth = 1.9;
    ctx.lineCap = "round";
    ctx.lineJoin = "round";
    for (const p of paths) ctx.stroke(p);
    ctx.restore();
  }

  // ========================================================================
  // 4. Syntax-Highlighted <Code> Tokenizer (Matching renderer.rs Palette 1:1)
  // ========================================================================
  const CODE_KEYWORDS = new Set([
    "import", "from", "export", "type", "interface", "enum", "function", "fn",
    "const", "let", "return", "if", "else", "while", "for", "switch", "case",
    "default", "break", "continue", "readonly", "true", "false", "as", "match",
    "schema", "shader", "syscall", "effects",
  ]);
  const CODE_TYPES = new Set([
    "i64", "f64", "bool", "string", "str", "void", "Widget", "Style", "Vec2", "Vec3", "Color",
    "Column", "Row", "Grid", "Stack", "Card", "Scroll", "Text", "Code", "Badge",
    "Button", "Input", "Slider", "Toggle", "Shader", "Icon", "Image", "Divider", "Progress", "Spacer",
  ]);

  function tokenizeCodeLine(line) {
    const out = [];
    let i = 0;
    const n = line.length;
    while (i < n) {
      const ch = line[i];
      if (ch === "/" && line[i + 1] === "/") {
        out.push({ text: line.slice(i), color: "#64748b", italic: true });
        break;
      }
      if (ch === '"' || ch === "'" || ch === "`") {
        const q = ch;
        let j = i + 1;
        while (j < n && line[j] !== q) {
          if (line[j] === "\\") j++;
          j++;
        }
        if (j < n) j++;
        out.push({ text: line.slice(i, j), color: "#34d399" });
        i = j;
        continue;
      }
      if (ch === "@" && /[a-zA-Z_]/.test(line[i + 1] || "")) {
        let j = i + 1;
        while (j < n && /[a-zA-Z0-9_]/.test(line[j])) j++;
        out.push({ text: line.slice(i, j), color: "#f472b6" });
        i = j;
        continue;
      }
      if (ch === "#" && /[0-9a-fA-F]/.test(line[i + 1] || "")) {
        let j = i + 1;
        while (j < n && /[0-9a-fA-F]/.test(line[j])) j++;
        out.push({ text: line.slice(i, j), color: "#34d399" });
        i = j;
        continue;
      }
      if (/[0-9]/.test(ch)) {
        let j = i + 1;
        while (j < n && /[0-9._xXa-fA-F]/.test(line[j])) j++;
        out.push({ text: line.slice(i, j), color: "#fbbf24" });
        i = j;
        continue;
      }
      if (/[a-zA-Z_]/.test(ch)) {
        let j = i + 1;
        while (j < n && /[a-zA-Z0-9_]/.test(line[j])) j++;
        const word = line.slice(i, j);
        let color = "#e2e8f0";
        if (CODE_KEYWORDS.has(word)) color = "#c084fc";
        else if (CODE_TYPES.has(word) || (word[0] >= "A" && word[0] <= "Z")) color = "#38bdf8";
        else if (line[j] === "(") color = "#60a5fa";
        out.push({ text: word, color });
        i = j;
        continue;
      }
      if (/\s/.test(ch)) {
        let j = i + 1;
        while (j < n && /\s/.test(line[j])) j++;
        out.push({ text: line.slice(i, j), color: "#e2e8f0" });
        i = j;
        continue;
      }
      out.push({ text: ch, color: "#94a3b8" });
      i++;
    }
    return out;
  }

  // ========================================================================
  // 5. Typography Measurement & Multi-Line Wrapping
  // ========================================================================
  function isMonoStyle(st, kind) {
    return kind === "code" || st.fontFamily === "mono" || st.fontWeight === 450;
  }

  function fontSpec(st, kind, italic = false) {
    const fs = Math.max(8, st.fontSize || 15);
    let fw = st.fontWeight || 400;
    if (fw === 450) fw = 400;
    if (fw < 100) fw = (kind === "button" || kind === "badge") ? 600 : 400;
    const fam = isMonoStyle(st, kind) ? MONO_FONT : SANS_FONT;
    return `${italic ? "italic " : ""}${fw} ${fs}px ${fam}`;
  }

  function applyFont(ctx, st, kind, italic = false) {
    ctx.font = fontSpec(st, kind, italic);
    if ("letterSpacing" in ctx) {
      ctx.letterSpacing = st.letterSpacing ? `${st.letterSpacing}px` : "0px";
    }
  }

  function measureTextW(text, st, kind) {
    if (!text) return 0;
    applyFont(ctx, st, kind);
    let w = ctx.measureText(text).width;
    if (!("letterSpacing" in ctx) && st.letterSpacing && text.length > 1) {
      w += st.letterSpacing * (text.length - 1);
    }
    return Math.ceil(w);
  }

  function computeLineHeight(st, kind) {
    const fs = Math.max(8, st.fontSize || 15);
    if (st.lineHeight > 0) {
      return st.lineHeight < 4.5 ? Math.ceil(fs * st.lineHeight) : Math.ceil(st.lineHeight);
    }
    if (kind === "code") return Math.ceil(fs * 1.55);
    if (fs >= 28) return Math.ceil(fs * 1.18);
    if (fs >= 20) return Math.ceil(fs * 1.28);
    return Math.ceil(fs * 1.45);
  }

  function wrapLines(text, maxW, st, kind) {
    const fs = Math.max(8, st.fontSize || 15);
    const baseH = Math.ceil(fs * 1.22);
    const lineH = computeLineHeight(st, kind);
    if (!text) return { lines: [], maxW: 0, totalH: baseH, lineH };
    const limit = Math.max(20, maxW);
    const lines = [];
    let maxLW = 0;

    for (const para of text.split("\n")) {
      if (para === "") {
        lines.push("");
        continue;
      }
      const fullW = measureTextW(para, st, kind);
      if (fullW <= limit) {
        maxLW = Math.max(maxLW, fullW);
        lines.push(para);
        continue;
      }
      if (kind === "code") {
        // Preserve leading indentation and break cleanly on char boundary
        let chunk = "";
        for (const ch of para) {
          const next = chunk + ch;
          if (measureTextW(next, st, kind) <= limit || chunk.length === 0) {
            chunk = next;
          } else {
            maxLW = Math.max(maxLW, measureTextW(chunk, st, kind));
            lines.push(chunk);
            chunk = ch;
          }
        }
        if (chunk) {
          maxLW = Math.max(maxLW, measureTextW(chunk, st, kind));
          lines.push(chunk);
        }
        continue;
      }
      let cur = "";
      for (const word of para.split(/\s+/)) {
        if (!word) continue;
        const cand = cur ? `${cur} ${word}` : word;
        if (measureTextW(cand, st, kind) <= limit) {
          cur = cand;
        } else {
          if (cur) {
            maxLW = Math.max(maxLW, measureTextW(cur, st, kind));
            lines.push(cur);
          }
          if (measureTextW(word, st, kind) <= limit) {
            cur = word;
          } else {
            let chunk = "";
            for (const ch of word) {
              const next = chunk + ch;
              if (measureTextW(next, st, kind) <= limit || chunk.length === 0) {
                chunk = next;
              } else {
                maxLW = Math.max(maxLW, measureTextW(chunk, st, kind));
                lines.push(chunk);
                chunk = ch;
              }
            }
            cur = chunk;
          }
        }
      }
      if (cur) {
        maxLW = Math.max(maxLW, measureTextW(cur, st, kind));
        lines.push(cur);
      }
    }
    if (lines.length === 0) lines.push("");
    const totalH = lines.length === 1 ? baseH : baseH + (lines.length - 1) * lineH;
    return { lines, maxW: maxLW, totalH, lineH };
  }

  function fitInputTail(text, maxW, st) {
    const limit = Math.max(20, maxW);
    if (measureTextW(text, st, "input") <= limit) return text;
    const chars = Array.from(text);
    let out = "";
    for (let i = chars.length - 1; i >= 0; i--) {
      const cand = chars[i] + out;
      if (measureTextW(cand, st, "input") <= limit) out = cand;
      else break;
    }
    return out;
  }

  // ========================================================================
  // 6. Unified 2-Pass Flexbox + Bento Grid + Stack Layout Engine
  // ========================================================================
  function effectiveGridCols(requested, availW) {
    const req = Math.max(1, requested || 1);
    if (req >= 2 && availW < 640) return 1;
    if (req >= 3 && availW < 900) return 2;
    return req;
  }

  function packBentoRows(children, cols) {
    const rows = [];
    let cur = [];
    let used = 0;
    for (const ch of children) {
      const span = Math.min(cols, Math.max(1, (ch.style && ch.style.colSpan) || 1));
      if (used > 0 && used + span > cols) {
        rows.push(cur);
        cur = [];
        used = 0;
      }
      cur.push({ child: ch, col: used, span });
      used += span;
      if (used >= cols) {
        rows.push(cur);
        cur = [];
        used = 0;
      }
    }
    if (cur.length > 0) rows.push(cur);
    return rows;
  }

  function effectivePad(st, kind) {
    const p = st.pad || [0, 0, 0, 0];
    if ((kind === "button" || kind === "badge") && p[0] > 0 && Math.abs(p[0] - p[1]) < 0.01 && Math.abs(p[0] - p[3]) < 0.01) {
      const pv = Math.max(6, Math.round(p[0] * 0.65));
      const ph = Math.max(12, Math.round(p[0] * 1.3));
      return [pv, ph, pv, ph];
    }
    if (kind === "button" && p[0] === 0 && p[1] === 0 && p[2] === 0 && p[3] === 0) return [8, 14, 8, 14];
    if (kind === "badge" && p[0] === 0 && p[1] === 0 && p[2] === 0 && p[3] === 0) return [5, 11, 5, 11];
    return p;
  }

  function measureIntrinsic(node, maxW) {
    const st = node.style || defaultStyle();
    const [pt, pr, pb, pl] = effectivePad(st, node.kind);
    const gx = st.gapX || 0;
    const gy = st.gapY || 0;
    let avail = maxW;
    if (st.maxW > 0) avail = Math.min(avail, st.maxW);
    if (st.w > 0) avail = st.w;

    let rawW = 0, rawH = 0;
    const k = node.kind;

    if (k === "text" || k === "code") {
      const innerAvail = Math.max(20, avail - pl - pr);
      const wr = wrapLines(node.text, innerAvail, st, k);
      rawW = k === "code" ? avail : Math.min(avail, wr.maxW + pl + pr);
      rawH = wr.totalH + pt + pb;
    } else if (k === "icon") {
      const sz = Math.max(10, st.fontSize || 18);
      rawW = sz + pl + pr;
      rawH = sz + pt + pb;
    } else if (k === "button") {
      if (node.children.length > 0) {
        const bgap = gx > 0 ? gx : 8;
        let sumW = 0, maxH = 0;
        const innerMax = Math.max(20, avail - pl - pr);
        for (let i = 0; i < node.children.length; i++) {
          const [cw, ch] = measureIntrinsic(node.children[i], innerMax);
          sumW += cw + (i > 0 ? bgap : 0);
          maxH = Math.max(maxH, ch);
        }
        rawW = Math.min(avail, sumW + pl + pr);
        rawH = Math.max(24, maxH + pt + pb);
      } else {
        const tw = measureTextW(node.text, st, "button");
        const th = Math.ceil((st.fontSize || 14) * 1.22);
        rawW = tw + pl + pr;
        rawH = Math.max(28, th + pt + pb);
      }
    } else if (k === "badge") {
      const tw = measureTextW(node.text, st, "badge");
      const th = Math.ceil((st.fontSize || 12) * 1.22);
      rawW = tw + pl + pr;
      rawH = th + pt + pb;
    } else if (k === "toggle") {
      const tw = measureTextW(node.text, st, "toggle");
      const th = Math.ceil((st.fontSize || 13) * 1.22);
      rawW = tw + 52 + pl + pr;
      rawH = Math.max(26, th) + pt + pb;
    } else if (k === "input") {
      const th = Math.ceil((st.fontSize || 14) * 1.25);
      rawW = Math.max(160, avail);
      rawH = Math.max(38, th + pt + pb);
    } else if (k === "slider") {
      rawW = Math.max(140, avail);
      rawH = 30;
    } else if (k === "shader" || k === "image") {
      rawW = st.w > 0 ? st.w : avail;
      rawH = st.h > 0 ? st.h : 240;
    } else if (k === "spacer") {
      rawW = Math.max(8, pl || st.w || 8);
      rawH = Math.max(8, pt || st.h || 8);
    } else if (k === "divider") {
      rawW = avail;
      rawH = Math.max(1, st.h || (st.borderW > 1 ? st.borderW : 1));
    } else if (k === "progress") {
      rawW = avail;
      rawH = st.h > 0 ? st.h : 8;
    } else if (k === "col" || k === "card" || k === "scroll") {
      const innerMax = Math.max(24, avail - pl - pr);
      let maxCW = 0, totalH = 0;
      if (node.text) {
        const wr = wrapLines(node.text, innerMax, st, "text");
        maxCW = Math.max(maxCW, wr.maxW);
        totalH += wr.totalH + (node.children.length ? gy : 0);
      }
      for (let i = 0; i < node.children.length; i++) {
        const [cw, ch] = measureIntrinsic(node.children[i], innerMax);
        maxCW = Math.max(maxCW, cw);
        totalH += ch + (i + 1 < node.children.length ? gy : 0);
      }
      rawW = st.w > 0 ? st.w : avail;
      rawH = totalH + pt + pb;
    } else if (k === "grid") {
      const cols = effectiveGridCols(st.gridCols, avail);
      const innerW = Math.max(24, avail - pl - pr);
      const colW = Math.max(20, (innerW - gx * (cols - 1)) / cols);
      const rows = packBentoRows(node.children, cols);
      let totalH = 0;
      for (let r = 0; r < rows.length; r++) {
        let rowH = 0;
        for (const item of rows[r]) {
          const itemW = colW * item.span + gx * (item.span - 1);
          const [, ch] = measureIntrinsic(item.child, itemW);
          rowH = Math.max(rowH, ch);
        }
        totalH += rowH + (r > 0 ? gy : 0);
      }
      rawW = avail;
      rawH = totalH + pt + pb;
    } else if (k === "stack") {
      const innerMax = Math.max(24, avail - pl - pr);
      let maxWC = 0, maxHC = 0;
      for (const ch of node.children) {
        const [cw, chH] = measureIntrinsic(ch, innerMax);
        maxWC = Math.max(maxWC, cw);
        maxHC = Math.max(maxHC, chH);
      }
      rawW = Math.max(maxWC + pl + pr, st.w < 0 ? avail : 0) || avail;
      rawH = maxHC + pt + pb;
    } else if (k === "row") {
      const innerMax = Math.max(24, avail - pl - pr);
      let rowX = 0, rowH = 0, totalH = 0, maxRW = 0;
      for (const ch of node.children) {
        const [cw, chH] = measureIntrinsic(ch, innerMax);
        if (rowX > 0 && rowX + gx + cw > innerMax + 0.5) {
          maxRW = Math.max(maxRW, rowX);
          totalH += rowH + gy;
          rowX = cw;
          rowH = chH;
        } else {
          if (rowX > 0) rowX += gx;
          rowX += cw;
          rowH = Math.max(rowH, chH);
        }
      }
      maxRW = Math.max(maxRW, rowX);
      totalH += rowH;
      rawW = Math.min(avail, maxRW + pl + pr);
      rawH = totalH + pt + pb;
    }

    let finalW = st.w > 0 ? st.w : (st.w < 0 ? avail : Math.max(rawW, st.minW || 0));
    if (st.maxW > 0) finalW = Math.min(finalW, st.maxW);
    const finalH = st.h > 0 ? st.h : rawH;
    return [finalW, finalH];
  }

  function shiftSubtree(laid, dx, dy) {
    if (Math.abs(dx) < 0.01 && Math.abs(dy) < 0.01) return;
    laid.rect.x += dx;
    laid.rect.y += dy;
    for (const c of laid.children) shiftSubtree(c, dx, dy);
  }

  function shiftHitBoxes(hitBoxes, startIdx, endIdx, dx, dy) {
    if (Math.abs(dx) < 0.01 && Math.abs(dy) < 0.01) return;
    for (let j = startIdx; j < endIdx && j < hitBoxes.length; j++) {
      hitBoxes[j].rect.x += dx;
      hitBoxes[j].rect.y += dy;
      hitBoxes[j].hitRect.x += dx;
      hitBoxes[j].hitRect.y += dy;
    }
  }

  function isScrollableY(node) {
    const ov = ((node.style && node.style.overflow) || "visible").toLowerCase();
    return (node.kind === "scroll" && ov !== "scroll-x") || ov === "scroll" || ov === "scroll-xy";
  }

  function isScrollableX(node) {
    const ov = ((node.style && node.style.overflow) || "visible").toLowerCase();
    return ov === "scroll-x" || ov === "scroll-xy";
  }

  function isScrollable(node) {
    return isScrollableY(node) || isScrollableX(node);
  }

  function clipsChildren(node) {
    const ov = ((node.style && node.style.overflow) || "visible").toLowerCase();
    return isScrollable(node) || ov === "hidden";
  }

  function getScrollKey(node, path) {
    if (node.key) return node.key;
    if (node.action) return node.action;
    return path;
  }

  function intersectRect(a, b) {
    const x0 = Math.max(a.x, b.x);
    const y0 = Math.max(a.y, b.y);
    const x1 = Math.min(a.x + a.w, b.x + b.w);
    const y1 = Math.min(a.y + a.h, b.y + b.h);
    if (x1 <= x0 || y1 <= y0) return null;
    return { x: x0, y: y0, w: x1 - x0, h: y1 - y0 };
  }

  function childrenNeedZSort(kids) {
    for (let i = 1; i < kids.length; i++) {
      const prevZ = (kids[i - 1].node.style && kids[i - 1].node.style.zIndex) || 0;
      const curZ = (kids[i].node.style && kids[i].node.style.zIndex) || 0;
      if (prevZ > curZ) return true;
    }
    return false;
  }

  function sortedChildIndicesByZ(kids) {
    return kids.map((_, i) => i).sort((a, b) => {
      const za = (kids[a].node.style && kids[a].node.style.zIndex) || 0;
      const zb = (kids[b].node.style && kids[b].node.style.zIndex) || 0;
      return za !== zb ? za - zb : a - b;
    });
  }

  function reorderHitBoxesByZ(hitBoxes, startIdx, laidKids, childHbRanges) {
    if (!childrenNeedZSort(laidKids) || startIdx >= hitBoxes.length) return;
    const order = sortedChildIndicesByZ(laidKids);
    const reordered = [];
    for (const idx of order) {
      const [s, e] = childHbRanges[idx];
      for (let j = s; j < e && j < hitBoxes.length; j++) reordered.push(hitBoxes[j]);
    }
    if (reordered.length === hitBoxes.length - startIdx) {
      for (let j = 0; j < reordered.length; j++) hitBoxes[startIdx + j] = reordered[j];
    }
  }

  function computeContentExtent(x, y, boxW, boxH, pr, pb, laidKids) {
    let maxR = x;
    let maxB = y;
    for (const c of laidKids) {
      maxR = Math.max(maxR, c.rect.x + c.rect.w);
      maxB = Math.max(maxB, c.rect.y + c.rect.h);
    }
    return {
      contentW: Math.max(boxW, (maxR - x) + pr),
      contentH: Math.max(boxH, (maxB - y) + pb),
    };
  }

  function finalizeContainerLayout(node, rect, contentW, contentH, path, laidKids, hitBoxes, childrenHbStart, childHbRanges) {
    reorderHitBoxesByZ(hitBoxes, childrenHbStart, laidKids, childHbRanges);
    const scrollKey = getScrollKey(node, path);
    let localScrollY = 0;
    let localScrollX = 0;
    if (isScrollableY(node)) {
      const maxSY = Math.max(0, contentH - rect.h);
      localScrollY = Math.min(maxSY, Math.max(0, nestedScrollY.get(scrollKey) || 0));
      nestedScrollY.set(scrollKey, localScrollY);
    }
    if (isScrollableX(node)) {
      const maxSX = Math.max(0, contentW - rect.w);
      localScrollX = Math.min(maxSX, Math.max(0, nestedScrollX.get(scrollKey) || 0));
      nestedScrollX.set(scrollKey, localScrollX);
    }
    if (localScrollX > 0.01 || localScrollY > 0.01) {
      for (const ch of laidKids) shiftSubtree(ch, -localScrollX, -localScrollY);
      shiftHitBoxes(hitBoxes, childrenHbStart, hitBoxes.length, -localScrollX, -localScrollY);
    }
    if (clipsChildren(node)) {
      for (let j = hitBoxes.length - 1; j >= childrenHbStart; j--) {
        const clipped = intersectRect(hitBoxes[j].hitRect, rect);
        if (clipped) hitBoxes[j].hitRect = clipped;
        else hitBoxes.splice(j, 1);
      }
    }
    return { scrollKey, scrollX: localScrollX, scrollY: localScrollY, contentW, contentH };
  }

  function layoutNode(node, x, y, availW, minH, hitBoxes, focusables, path = "root") {
    const st = node.style || defaultStyle();
    const [pt, pr, pb, pl] = effectivePad(st, node.kind);
    const gx = st.gapX || 0;
    const gy = st.gapY || 0;
    let boxW = availW;
    if (st.maxW > 0) boxW = Math.min(boxW, st.maxW);
    if (st.w > 0) boxW = st.w;

    const k = node.kind;
    const isFocusableKind = k === "input" || k === "button" || k === "toggle" || k === "slider";
    const pushHit = (rect) => {
      if (node.action) {
        const hb = {
          rect: { ...rect },
          hitRect: { ...rect },
          action: node.action,
          kind: k,
          value: node.value,
          text: node.text,
          key: node.key,
        };
        hitBoxes.push(hb);
        if (isFocusableKind) focusables.push(hb);
        return hb;
      }
      return null;
    };

    // Button with child elements (e.g. <Button><Icon /><Text /></Button>)
    if (k === "button" && node.children.length > 0) {
      const bgap = gx > 0 ? gx : 8;
      const innerMax = Math.max(20, boxW - pl - pr);
      const childDims = node.children.map(c => measureIntrinsic(c, innerMax));
      const sumW = childDims.reduce((a, d) => a + d[0], 0) + bgap * Math.max(0, node.children.length - 1);
      const maxCH = childDims.reduce((a, d) => Math.max(a, d[1]), 0);
      const w = st.w > 0 ? st.w : (st.w < 0 ? boxW : Math.min(boxW, Math.max(sumW + pl + pr, st.minW || 0)));
      const contentH = Math.max(24, maxCH + pt + pb);
      const h = st.h > 0 ? st.h : (minH ? Math.max(contentH, minH) : contentH);
      const rect = { x, y, w, h };
      pushHit(rect);
      const childrenHbStart = hitBoxes.length;
      const childHbRanges = [];
      let cx = x + (w - sumW) * 0.5;
      const laidKids = [];
      for (let i = 0; i < node.children.length; i++) {
        const [cw, ch] = childDims[i];
        const cy = y + (h - ch) * 0.5;
        const hbStart = hitBoxes.length;
        const laid = layoutNode(node.children[i], cx, cy, cw, ch, hitBoxes, focusables, `${path}/${i}`);
        childHbRanges.push([hbStart, hitBoxes.length]);
        laidKids.push(laid);
        cx += laid.rect.w + bgap;
      }
      reorderHitBoxesByZ(hitBoxes, childrenHbStart, laidKids, childHbRanges);
      return { rect, node, wrappedLines: [], lineHeight: computeLineHeight(st, k), children: laidKids, contentW: w, contentH: h, scrollX: 0, scrollY: 0, scrollKey: getScrollKey(node, path) };
    }

    // Leaf widgets
    if (
      k === "text" || k === "code" || k === "icon" || k === "button" || k === "badge" ||
      k === "input" || k === "slider" || k === "toggle" || k === "image" || k === "shader" ||
      k === "spacer" || k === "divider" || k === "progress"
    ) {
      const [iw, ih] = measureIntrinsic(node, boxW);
      let w = iw;
      if (k === "input" || k === "slider" || k === "shader" || k === "divider" || k === "progress" || k === "code") {
        w = st.w > 0 ? st.w : boxW;
      } else if (k === "text") {
        w = st.textAlign === "center" || st.textAlign === "right" ? boxW : Math.min(iw, boxW);
      }
      const h = minH ? Math.max(ih, minH) : ih;
      const rect = { x, y, w, h };
      let wrappedLines = [];
      let lineHeight = computeLineHeight(st, k);

      if ((k === "text" || k === "code") && node.text) {
        const wr = wrapLines(node.text, Math.max(20, w - pl - pr), st, k);
        wrappedLines = wr.lines;
        lineHeight = wr.lineH;
      } else if (k === "input" && node.text) {
        wrappedLines = [fitInputTail(node.text, Math.max(20, w - Math.max(12, pl) - Math.max(12, pr)), st)];
      } else if (k !== "divider" && k !== "progress" && k !== "icon" && k !== "shader" && k !== "image" && node.text) {
        wrappedLines = [node.text];
      }
      pushHit(rect);
      return { rect, node, wrappedLines, lineHeight, children: [], contentW: w, contentH: h, scrollX: 0, scrollY: 0, scrollKey: "" };
    }

    // Vertical container: col / card / scroll
    if (k === "col" || k === "card" || k === "scroll") {
      const selfHb = pushHit({ x, y, w: boxW, h: 0 });
      const childrenHbStart = hitBoxes.length;
      const innerW = Math.max(20, boxW - pl - pr);
      let curY = y + pt;
      let wrappedLines = [];
      let lineHeight = computeLineHeight(st, "text");
      if (node.text) {
        const wr = wrapLines(node.text, innerW, st, "text");
        wrappedLines = wr.lines;
        lineHeight = wr.lineH;
        curY += wr.totalH + (node.children.length ? gy : 0);
      }
      const laidKids = [];
      const childHbRanges = [];
      for (let i = 0; i < node.children.length; i++) {
        const child = node.children[i];
        const [cw] = measureIntrinsic(child, innerW);
        const childW = (st.align === "center" || st.align === "end" || st.align === "start")
          ? Math.min(cw, innerW) : innerW;
        let cx = x + pl;
        if (st.align === "center") cx = x + pl + Math.max(0, (innerW - childW) * 0.5);
        else if (st.align === "end") cx = x + pl + Math.max(0, innerW - childW);
        const hbStart = hitBoxes.length;
        const laid = layoutNode(child, cx, curY, childW, null, hitBoxes, focusables, `${path}/${i}`);
        const hbEnd = hitBoxes.length;
        curY += laid.rect.h + (i + 1 < node.children.length ? gy : 0);
        laidKids.push(laid);
        childHbRanges.push([hbStart, hbEnd]);
      }
      const rawContentH = (curY - y) + pb;
      const h = st.h > 0 ? st.h : (minH ? Math.max(rawContentH, minH) : rawContentH);
      const extraH = Math.max(0, h - rawContentH);
      if (extraH > 0.5 && laidKids.length > 0) {
        if (st.justify === "center" || st.justify === "end") {
          const dy = st.justify === "center" ? extraH * 0.5 : extraH;
          for (let i = 0; i < laidKids.length; i++) {
            shiftSubtree(laidKids[i], 0, dy);
            const [hs, he] = childHbRanges[i];
            shiftHitBoxes(hitBoxes, hs, he, 0, dy);
          }
        } else if (st.justify === "between" && laidKids.length > 1) {
          const step = extraH / (laidKids.length - 1);
          for (let i = 1; i < laidKids.length; i++) {
            const dy = step * i;
            shiftSubtree(laidKids[i], 0, dy);
            const [hs, he] = childHbRanges[i];
            shiftHitBoxes(hitBoxes, hs, he, 0, dy);
          }
        }
      }
      const rect = { x, y, w: boxW, h };
      if (selfHb) { selfHb.rect = { ...rect }; selfHb.hitRect = { ...rect }; }
      const ext = computeContentExtent(x, y, boxW, Math.max(h, rawContentH), pr, pb, laidKids);
      const meta = finalizeContainerLayout(node, rect, ext.contentW, Math.max(ext.contentH, rawContentH), path, laidKids, hitBoxes, childrenHbStart, childHbRanges);
      return { rect, node, wrappedLines, lineHeight, children: laidKids, ...meta };
    }

    // Bento Grid with colSpan & equal row heights
    if (k === "grid") {
      const selfHb = pushHit({ x, y, w: boxW, h: 0 });
      const childrenHbStart = hitBoxes.length;
      const cols = effectiveGridCols(st.gridCols, boxW);
      const innerW = Math.max(20, boxW - pl - pr);
      const colW = Math.max(20, (innerW - gx * (cols - 1)) / cols);
      const rows = packBentoRows(node.children, cols);
      let curY = y + pt;
      const laidKids = [];
      const childHbRanges = [];
      let childIdx = 0;
      for (let r = 0; r < rows.length; r++) {
        let rowH = 0;
        for (const item of rows[r]) {
          const itemW = colW * item.span + gx * (item.span - 1);
          const [, ch] = measureIntrinsic(item.child, itemW);
          rowH = Math.max(rowH, ch);
        }
        for (const item of rows[r]) {
          const itemX = x + pl + item.col * (colW + gx);
          const itemW = colW * item.span + gx * (item.span - 1);
          const hbStart = hitBoxes.length;
          laidKids.push(layoutNode(item.child, itemX, curY, itemW, rowH, hitBoxes, focusables, `${path}/${childIdx++}`));
          childHbRanges.push([hbStart, hitBoxes.length]);
        }
        curY += rowH + (r + 1 < rows.length ? gy : 0);
      }
      const rawContentH = (curY - y) + pb;
      const h = st.h > 0 ? st.h : (minH ? Math.max(rawContentH, minH) : rawContentH);
      const rect = { x, y, w: boxW, h };
      if (selfHb) { selfHb.rect = { ...rect }; selfHb.hitRect = { ...rect }; }
      const ext = computeContentExtent(x, y, boxW, Math.max(h, rawContentH), pr, pb, laidKids);
      const meta = finalizeContainerLayout(node, rect, ext.contentW, Math.max(ext.contentH, rawContentH), path, laidKids, hitBoxes, childrenHbStart, childHbRanges);
      return { rect, node, wrappedLines: [], lineHeight: computeLineHeight(st, k), children: laidKids, ...meta };
    }

    // Stack / ZStack
    if (k === "stack") {
      const selfHb = pushHit({ x, y, w: boxW, h: 0 });
      const childrenHbStart = hitBoxes.length;
      const innerW = Math.max(20, boxW - pl - pr);
      let maxH = 0;
      const laidKids = [];
      const childHbRanges = [];
      for (let i = 0; i < node.children.length; i++) {
        const hbStart = hitBoxes.length;
        const laid = layoutNode(node.children[i], x + pl, y + pt, innerW, null, hitBoxes, focusables, `${path}/${i}`);
        childHbRanges.push([hbStart, hitBoxes.length]);
        maxH = Math.max(maxH, laid.rect.h);
        laidKids.push(laid);
      }
      const rawContentH = maxH + pt + pb;
      const h = st.h > 0 ? st.h : (minH ? Math.max(rawContentH, minH) : rawContentH);
      if (st.align === "center") {
        for (let i = 0; i < laidKids.length; i++) {
          const dy = Math.max(0, (h - pt - pb - laidKids[i].rect.h) * 0.5);
          shiftSubtree(laidKids[i], 0, dy);
          const [hs, he] = childHbRanges[i];
          shiftHitBoxes(hitBoxes, hs, he, 0, dy);
        }
      }
      const rect = { x, y, w: boxW, h };
      if (selfHb) { selfHb.rect = { ...rect }; selfHb.hitRect = { ...rect }; }
      const ext = computeContentExtent(x, y, boxW, Math.max(h, rawContentH), pr, pb, laidKids);
      const meta = finalizeContainerLayout(node, rect, ext.contentW, Math.max(ext.contentH, rawContentH), path, laidKids, hitBoxes, childrenHbStart, childHbRanges);
      return { rect, node, wrappedLines: [], lineHeight: computeLineHeight(st, k), children: laidKids, ...meta };
    }

    // Row (with wrap, scroll-x strip mode, grow, justify, and per-line cross-axis align)
    const selfHb = pushHit({ x, y, w: boxW, h: 0 });
    const childrenHbStart = hitBoxes.length;
    const innerW = Math.max(20, boxW - pl - pr);
    const allowScrollX = isScrollableX(node);
    const totalGrow = allowScrollX ? 0 : node.children.reduce((a, c) => a + Math.max(0, (c.style && c.style.grow) || 0), 0);
    const intrinsic = node.children.map(c => measureIntrinsic(c, innerW));
    const fixedSum = node.children.reduce((a, c, idx) => a + (((c.style && c.style.grow) || 0) > 0 ? 0 : intrinsic[idx][0]), 0);
    const gapsTotal = gx * Math.max(0, node.children.length - 1);
    const remGrow = Math.max(0, innerW - fixedSum - gapsTotal);

    // Group into wrapped lines when totalGrow == 0 and not horizontally scrollable
    const lines = [];
    let curLine = [];
    let lineW = 0;
    for (let i = 0; i < node.children.length; i++) {
      const c = node.children[i];
      const [iw] = intrinsic[i];
      const cw = (totalGrow > 0 && c.style.grow > 0)
        ? remGrow * (c.style.grow / totalGrow)
        : (allowScrollX ? iw : Math.min(iw, innerW));
      if (!allowScrollX && totalGrow === 0 && curLine.length > 0 && lineW + gx + cw > innerW + 0.5) {
        lines.push({ items: curLine, rawW: lineW });
        curLine = [];
        lineW = 0;
      }
      if (curLine.length > 0) lineW += gx;
      curLine.push({ child: c, w: cw, idx: i });
      lineW += cw;
    }
    if (curLine.length > 0) lines.push({ items: curLine, rawW: lineW });

    let curY = y + pt;
    const laidKids = [];
    const childHbRanges = [];
    for (let li = 0; li < lines.length; li++) {
      const ln = lines[li];
      const count = ln.items.length;
      const itemsSumW = ln.items.reduce((a, it) => a + it.w, 0);
      let startX = x + pl;
      let itemGap = gx;
      if (totalGrow === 0 && !allowScrollX) {
        if (st.justify === "between" && count > 1) {
          itemGap = Math.max(gx, (innerW - itemsSumW) / (count - 1));
        } else if (st.justify === "around" && count > 0) {
          const space = Math.max(0, innerW - itemsSumW) / count;
          startX += space * 0.5;
          itemGap = space;
        } else if (st.justify === "center") {
          startX += Math.max(0, (innerW - ln.rawW) * 0.5);
        } else if (st.justify === "end") {
          startX += Math.max(0, innerW - ln.rawW);
        }
      }
      let cx = startX;
      let lineH = 0;
      const lineLaid = [];
      const lineRanges = [];
      for (const it of ln.items) {
        const hbStart = hitBoxes.length;
        const laid = layoutNode(it.child, cx, curY, it.w, null, hitBoxes, focusables, `${path}/${it.idx}`);
        const hbEnd = hitBoxes.length;
        cx += laid.rect.w + itemGap;
        lineH = Math.max(lineH, laid.rect.h);
        lineLaid.push(laid);
        lineRanges.push([hbStart, hbEnd]);
      }
      // Cross-axis vertical alignment on this row line
      const crossAlign = st.align || "center";
      for (let kIdx = 0; kIdx < lineLaid.length; kIdx++) {
        const laid = lineLaid[kIdx];
        const [hs, he] = lineRanges[kIdx];
        if (crossAlign === "center") {
          const dy = Math.max(0, (lineH - laid.rect.h) * 0.5);
          shiftSubtree(laid, 0, dy);
          shiftHitBoxes(hitBoxes, hs, he, 0, dy);
        } else if (crossAlign === "end") {
          const dy = Math.max(0, lineH - laid.rect.h);
          shiftSubtree(laid, 0, dy);
          shiftHitBoxes(hitBoxes, hs, he, 0, dy);
        }
        laidKids.push(laid);
        childHbRanges.push([hs, he]);
      }
      curY += lineH + (li + 1 < lines.length ? gy : 0);
    }

    const rawContentH = (curY - y) + pb;
    const h = st.h > 0 ? st.h : (minH ? Math.max(rawContentH, minH) : rawContentH);
    const rect = { x, y, w: boxW, h };
    if (selfHb) { selfHb.rect = { ...rect }; selfHb.hitRect = { ...rect }; }
    const ext = computeContentExtent(x, y, boxW, Math.max(h, rawContentH), pr, pb, laidKids);
    const meta = finalizeContainerLayout(node, rect, ext.contentW, Math.max(ext.contentH, rawContentH), path, laidKids, hitBoxes, childrenHbStart, childHbRanges);
    return { rect, node, wrappedLines: [], lineHeight: computeLineHeight(st, k), children: laidKids, ...meta };
  }

  // ========================================================================
  // 7. Offscreen WebGPU + Automatic WebGL2 Shader Runtime (128-byte Uniforms)
  // ========================================================================
  let gpuDevicePromise = null;
  async function getWebGpuDevice() {
    if (!navigator.gpu) return null;
    if (!gpuDevicePromise) {
      gpuDevicePromise = (async () => {
        try {
          const adapter = await navigator.gpu.requestAdapter();
          return adapter ? await adapter.requestDevice() : null;
        } catch (_) { return null; }
      })();
    }
    return gpuDevicePromise;
  }

  function initOffscreenWebGl2(canvas, shaderName) {
    const spec = KNR_SHADERS[shaderName];
    if (!spec || !spec.glsl_vertex || !spec.glsl_fragment) return null;
    const gl = canvas.getContext("webgl2", { alpha: true, antialias: true, preserveDrawingBuffer: true });
    if (!gl) return null;
    const compile = (type, src) => {
      const sh = gl.createShader(type);
      gl.shaderSource(sh, src);
      gl.compileShader(sh);
      if (!gl.getShaderParameter(sh, gl.COMPILE_STATUS)) {
        console.error("knr WebGL2 shader error:", gl.getShaderInfoLog(sh));
        return null;
      }
      return sh;
    };
    const vs = compile(gl.VERTEX_SHADER, spec.glsl_vertex);
    const fs = compile(gl.FRAGMENT_SHADER, spec.glsl_fragment);
    if (!vs || !fs) return null;
    const prog = gl.createProgram();
    gl.attachShader(prog, vs);
    gl.attachShader(prog, fs);
    gl.linkProgram(prog);
    if (!gl.getProgramParameter(prog, gl.LINK_STATUS)) {
      console.error("knr WebGL2 link error:", gl.getProgramInfoLog(prog));
      return null;
    }
    const blockCount = gl.getProgramParameter(prog, gl.ACTIVE_UNIFORM_BLOCKS);
    for (let i = 0; i < blockCount; i++) gl.uniformBlockBinding(prog, i, 0);
    const ubo = gl.createBuffer();
    gl.bindBuffer(gl.UNIFORM_BUFFER, ubo);
    gl.bufferData(gl.UNIFORM_BUFFER, 128, gl.DYNAMIC_DRAW);
    gl.bindBufferBase(gl.UNIFORM_BUFFER, 0, ubo);
    return { kind: "webgl2", shaderName, gl, prog, ubo };
  }

  const shaderPool = new Map();
  async function ensureOffscreenShader(slotKey, shaderName) {
    if (!shaderName || !KNR_SHADERS[shaderName]) return null;
    let entry = shaderPool.get(slotKey);
    if (entry && entry.shaderName === shaderName) return entry;
    if (entry && entry.initName === shaderName) return entry;

    const canvas = document.createElement("canvas");
    entry = { canvas, shaderName: null, initName: shaderName, gpu: null };
    shaderPool.set(slotKey, entry);

    const spec = KNR_SHADERS[shaderName];
    const device = await getWebGpuDevice();
    if (device && spec.wgsl) {
      try {
        const wgpuCtx = canvas.getContext("webgpu");
        if (wgpuCtx) {
          const fmt = navigator.gpu.getPreferredCanvasFormat();
          wgpuCtx.configure({ device, format: fmt, alphaMode: "premultiplied" });
          const mod = device.createShaderModule({ code: spec.wgsl });
          const pipe = device.createRenderPipeline({
            layout: "auto",
            vertex: { module: mod, entryPoint: "vs_main" },
            fragment: { module: mod, entryPoint: "fs_main", targets: [{ format: fmt }] },
            primitive: { topology: "triangle-list" },
          });
          const ubuf = device.createBuffer({ size: 128, usage: GPUBufferUsage.UNIFORM | GPUBufferUsage.COPY_DST });
          const bg = device.createBindGroup({
            layout: pipe.getBindGroupLayout(0),
            entries: [{ binding: 0, resource: { buffer: ubuf } }],
          });
          entry.shaderName = shaderName;
          entry.gpu = { kind: "webgpu", device, ctx: wgpuCtx, pipe, ubuf, bg };
          return entry;
        }
      } catch (err) {
        console.warn("knr WebGPU fallback to WebGL2:", err);
      }
    }
    const glState = initOffscreenWebGl2(canvas, shaderName);
    if (glState) {
      entry.shaderName = shaderName;
      entry.gpu = glState;
    }
    return entry;
  }

  const uData = new Float32Array(32);
  function renderShaderIntoRect(ctx, slotKey, shaderName, rect, radii, uniforms, elapsed, dt, dpr, scrollY) {
    const entry = shaderPool.get(slotKey);
    if (!entry || entry.initName !== shaderName) {
      ensureOffscreenShader(slotKey, shaderName);
    }
    const gpu = entry && entry.gpu;
    const pw = Math.max(1, Math.round(rect.w * dpr));
    const ph = Math.max(1, Math.round(rect.h * dpr));

    ctx.save();
    roundRectPath(ctx, rect.x, rect.y, rect.w, rect.h, radii);
    ctx.clip();

    if (gpu && entry.shaderName === shaderName) {
      const c = entry.canvas;
      if (c.width !== pw || c.height !== ph) { c.width = pw; c.height = ph; }
      uData.fill(0);
      uData[0] = rect.w || pw;
      uData[1] = rect.h || ph;
      uData[2] = elapsed;
      uData[3] = dt;
      const screenTop = rect.y - scrollY;
      uData[4] = rect.w > 0 ? Math.min(1, Math.max(0, (pointerX - rect.x) / rect.w)) : 0.5;
      uData[5] = rect.h > 0 ? Math.min(1, Math.max(0, (pointerY - screenTop) / rect.h)) : 0.5;
      uData[6] = pointerIsDown ? 1.0 : 0.0;
      uData[7] = dpr;
      uData[8] = radii[0] || 0; uData[9] = radii[1] || 0; uData[10] = radii[2] || 0; uData[11] = radii[3] || 0;
      for (let i = 0; i < uniforms.length && i < 16; i++) {
        const v = Number(uniforms[i]);
        uData[12 + i] = Number.isFinite(v) ? v : 0;
      }

      if (gpu.kind === "webgpu") {
        gpu.device.queue.writeBuffer(gpu.ubuf, 0, uData);
        const enc = gpu.device.createCommandEncoder();
        const pass = enc.beginRenderPass({
          colorAttachments: [{
            view: gpu.ctx.getCurrentTexture().createView(),
            clearValue: { r: 0.03, g: 0.05, b: 0.11, a: 1 },
            loadOp: "clear", storeOp: "store",
          }],
        });
        pass.setPipeline(gpu.pipe);
        pass.setBindGroup(0, gpu.bg);
        pass.draw(6);
        pass.end();
        gpu.device.queue.submit([enc.finish()]);
      } else if (gpu.kind === "webgl2") {
        const gl = gpu.gl;
        gl.viewport(0, 0, pw, ph);
        gl.clearColor(0.03, 0.05, 0.11, 1);
        gl.clear(gl.COLOR_BUFFER_BIT);
        gl.useProgram(gpu.prog);
        gl.bindBuffer(gl.UNIFORM_BUFFER, gpu.ubo);
        gl.bufferSubData(gl.UNIFORM_BUFFER, 0, uData);
        gl.bindBufferBase(gl.UNIFORM_BUFFER, 0, gpu.ubo);
        gl.drawArrays(gl.TRIANGLES, 0, 6);
      }
      ctx.drawImage(c, rect.x, rect.y, rect.w, rect.h);
    } else {
      // Smooth procedural fallback while GPU pipeline warms up
      const g = ctx.createLinearGradient(rect.x, rect.y, rect.x + rect.w, rect.y + rect.h);
      const wave = 0.5 + 0.5 * Math.sin(elapsed * 1.4);
      g.addColorStop(0, `rgba(${Math.round(14 + 34 * wave)}, 34, 78, 1)`);
      g.addColorStop(0.5, `rgba(24, ${Math.round(92 + 54 * wave)}, 168, 1)`);
      g.addColorStop(1, "#0b1120");
      ctx.fillStyle = g;
      ctx.fillRect(rect.x, rect.y, rect.w, rect.h);
    }
    ctx.restore();
  }

  // Image asset cache
  const imageCache = new Map();
  function getImageEntry(src) {
    if (!src) return null;
    let entry = imageCache.get(src);
    if (!entry) {
      const img = new Image();
      img.crossOrigin = "anonymous";
      entry = { img, loaded: false, failed: false };
      img.onload = () => { entry.loaded = true; needsPaint = true; };
      img.onerror = () => { entry.failed = true; needsPaint = true; };
      img.src = src;
      imageCache.set(src, entry);
    }
    return entry;
  }

  // ========================================================================
  // 8. Unified Canvas2D Surface Painter (Pixel-Identical to Native Engine!)
  // ========================================================================
  let shaderVisitCounter = 0;

  function paintNode(ctx, laid, elapsed, dt, dpr, scrollY, viewportH) {
    const node = laid.node;
    const st = node.style || defaultStyle();
    const rect = laid.rect;
    const k = node.kind;
    if (st.opacity <= 0.004 || rect.w <= 0 || rect.h <= 0) return;

    // Viewport vertical culling
    const screenY = rect.y - scrollY;
    if (screenY + rect.h < -80 || screenY > viewportH + 80) return;

    const isHovered = Boolean(node.action && hoveredAction === node.action);
    const isFocused = Boolean(node.action && focusedInput && focusedInput.action === node.action);
    const isKeyFocused = Boolean(
      focusedWidgetKey &&
      (k === "input" || k === "button" || k === "toggle" || k === "slider") &&
      ((node.key && node.key === focusedWidgetKey) || (node.action && node.action === focusedWidgetKey))
    );

    ctx.save();
    if (st.opacity < 0.999) ctx.globalAlpha *= st.opacity;

    // Local transforms (translate, scale, hoverScale, rotateDeg)
    const activeScale = (isHovered && st.hoverScale && Math.abs(st.hoverScale - 1) > 0.002)
      ? st.scale * st.hoverScale : st.scale;
    const hasTx = Math.abs(st.tx) > 0.01 || Math.abs(st.ty) > 0.01;
    const hasScale = Math.abs(activeScale - 1) > 0.002;
    const hasRot = Math.abs(st.rotateDeg) > 0.01;
    if (hasTx || hasScale || hasRot) {
      const cx = rect.x + rect.w * 0.5;
      const cy = rect.y + rect.h * 0.5;
      ctx.translate(cx + st.tx, cy + st.ty);
      if (hasRot) ctx.rotate(st.rotateDeg * Math.PI / 180);
      if (hasScale) ctx.scale(activeScale, activeScale);
      ctx.translate(-cx, -cy);
    }

    let radii = st.radii || [0, 0, 0, 0];
    if (radii[0] === 0 && radii[1] === 0 && radii[2] === 0 && radii[3] === 0) {
      if (k === "progress") radii = [rect.h * 0.5, rect.h * 0.5, rect.h * 0.5, rect.h * 0.5];
      else if (k === "divider") radii = [1, 1, 1, 1];
    }

    // 1. Soft drop shadow
    if (st.shadowBlur > 0 && st.shadowColor) {
      const sc = parseHexRgba(st.shadowColor);
      if (sc) {
        ctx.save();
        ctx.shadowColor = rgbaStr(sc);
        ctx.shadowBlur = st.shadowBlur;
        ctx.shadowOffsetX = st.shadowX;
        ctx.shadowOffsetY = st.shadowY;
        const fill = makeCanvasFill(ctx, (isHovered && st.hoverBg) ? st.hoverBg : st.bg, rect);
        if (fill) {
          roundRectPath(ctx, rect.x, rect.y, rect.w, rect.h, radii);
          ctx.fillStyle = fill;
          ctx.fill();
        }
        ctx.restore();
      }
    }

    // 2. Background surface fill
    const activeBg = (isHovered && st.hoverBg) ? st.hoverBg : st.bg;
    if (activeBg && k !== "divider") {
      const fill = makeCanvasFill(ctx, activeBg, rect, isHovered && !st.hoverBg && (k === "button" || k === "card" || k === "toggle"));
      if (fill) {
        roundRectPath(ctx, rect.x, rect.y, rect.w, rect.h, radii);
        ctx.fillStyle = fill;
        ctx.fill();
      }
    } else if (k === "input") {
      roundRectPath(ctx, rect.x, rect.y, rect.w, rect.h, radii);
      ctx.fillStyle = "rgba(8, 13, 26, 0.94)";
      ctx.fill();
    }

    // Subtle 1px top specular highlight on elevated Card & Code surfaces
    if ((k === "card" || k === "code") && activeBg) {
      ctx.save();
      roundRectPath(ctx, rect.x, rect.y, rect.w, rect.h, radii);
      ctx.clip();
      const specGrad = ctx.createLinearGradient(rect.x, rect.y, rect.x + rect.w, rect.y);
      specGrad.addColorStop(0, "rgba(255,255,255,0.01)");
      specGrad.addColorStop(0.5, "rgba(255,255,255,0.09)");
      specGrad.addColorStop(1, "rgba(255,255,255,0.01)");
      ctx.fillStyle = specGrad;
      ctx.fillRect(rect.x, rect.y + 0.5, rect.w, 1);
      ctx.restore();
    }

    // 3. Specialized widgets
    if (k === "shader") {
      const slotKey = `${node.key || node.text}#${shaderVisitCounter++}`;
      renderShaderIntoRect(ctx, slotKey, node.text, rect, radii, node.uniforms || [], elapsed, dt, dpr, scrollY);
    } else if (k === "image" && node.text) {
      const entry = getImageEntry(node.text);
      ctx.save();
      roundRectPath(ctx, rect.x, rect.y, rect.w, rect.h, radii);
      ctx.clip();
      if (entry && entry.loaded && entry.img.naturalWidth > 0 && entry.img.naturalHeight > 0) {
        const iw = entry.img.naturalWidth;
        const ih = entry.img.naturalHeight;
        const scale = Math.max(rect.w / iw, rect.h / ih);
        const dw = iw * scale;
        const dh = ih * scale;
        ctx.drawImage(entry.img, rect.x + (rect.w - dw) * 0.5, rect.y + (rect.h - dh) * 0.5, dw, dh);
      } else {
        ctx.fillStyle = (entry && entry.failed) ? "rgba(38, 19, 28, 0.85)" : "rgba(15, 23, 42, 0.72)";
        ctx.fillRect(rect.x, rect.y, rect.w, rect.h);
      }
      ctx.restore();
    } else if (k === "icon") {
      const [pt, , , pl] = effectivePad(st, k);
      const sz = Math.max(10, st.fontSize || 18);
      drawVectorIcon(ctx, node.text, rect.x + pl, rect.y + pt, sz, st.color || "#94a3b8");
    } else if (k === "divider") {
      roundRectPath(ctx, rect.x, rect.y, rect.w, rect.h, radii);
      ctx.fillStyle = makeCanvasFill(ctx, st.bg || st.border || "#334155aa", rect) || "rgba(51,65,85,0.65)";
      ctx.fill();
    } else if (k === "progress") {
      if (!activeBg) {
        roundRectPath(ctx, rect.x, rect.y, rect.w, rect.h, radii);
        ctx.fillStyle = "#1e293b";
        ctx.fill();
      }
      const t = Math.min(1, Math.max(0, node.value || 0));
      if (t > 0.001) {
        const fw = Math.min(rect.w, Math.max(rect.h, rect.w * t));
        const fillRect = { x: rect.x, y: rect.y, w: fw, h: rect.h };
        roundRectPath(ctx, fillRect.x, fillRect.y, fillRect.w, fillRect.h, radii);
        const customFill = (st.color && st.color !== "#f8fafc") ? makeCanvasFill(ctx, st.color, fillRect) : null;
        if (customFill) {
          ctx.fillStyle = customFill;
        } else {
          const g = ctx.createLinearGradient(rect.x, rect.y, rect.x + fw, rect.y);
          g.addColorStop(0, "#38bdf8");
          g.addColorStop(1, "#818cf8");
          ctx.fillStyle = g;
        }
        ctx.fill();
      }
    } else if (k === "slider") {
      const t = Math.min(1, Math.max(0, node.value || 0));
      const trackH = 7;
      const trackY = rect.y + (rect.h - trackH) * 0.5;
      roundRectPath(ctx, rect.x, trackY, rect.w, trackH, [4, 4, 4, 4]);
      ctx.fillStyle = "rgba(30, 41, 59, 0.92)";
      ctx.fill();

      const fillW = Math.min(rect.w, Math.max(trackH, rect.w * t));
      const g = ctx.createLinearGradient(rect.x, trackY, rect.x + rect.w, trackY);
      g.addColorStop(0, "#38bdf8");
      g.addColorStop(1, "#818cf8");
      roundRectPath(ctx, rect.x, trackY, fillW, trackH, [4, 4, 4, 4]);
      ctx.fillStyle = g;
      ctx.fill();

      const thumbR = isHovered || (activeSlider && activeSlider.action === node.action) ? 10 : 8.5;
      const thumbCX = rect.x + thumbR + Math.max(0, rect.w - thumbR * 2) * t;
      const thumbCY = rect.y + rect.h * 0.5;
      ctx.save();
      ctx.shadowColor = "rgba(56, 189, 248, 0.65)";
      ctx.shadowBlur = 12;
      ctx.beginPath();
      ctx.arc(thumbCX, thumbCY, thumbR, 0, Math.PI * 2);
      ctx.fillStyle = "#f8fafc";
      ctx.fill();
      ctx.lineWidth = 2.2;
      ctx.strokeStyle = "#38bdf8";
      ctx.stroke();
      ctx.restore();
    } else if (k === "toggle") {
      const checked = (node.value || 0) > 0.5;
      const [, , , pl] = effectivePad(st, k);
      const pillW = 40, pillH = 22;
      const pillX = rect.x + pl;
      const pillY = rect.y + (rect.h - pillH) * 0.5;
      roundRectPath(ctx, pillX, pillY, pillW, pillH, [11, 11, 11, 11]);
      ctx.fillStyle = checked ? "#38bdf8" : "#334155";
      ctx.fill();
      const knobX = checked ? pillX + pillW - 19 : pillX + 3;
      ctx.beginPath();
      ctx.arc(knobX + 8, pillY + 11, 8, 0, Math.PI * 2);
      ctx.fillStyle = "#f8fafc";
      ctx.fill();
    }

    // 4. Border stroke (+ focus ring on Input)
    if (k !== "divider") {
      const activeBorder = isFocused ? "#38bdf8" : ((isHovered && st.hoverBorder) ? st.hoverBorder : st.border);
      if (activeBorder) {
        const bc = parseHexRgba(activeBorder);
        const bw = isFocused ? 2.0 : (isHovered ? Math.max(1.4, st.borderW) : Math.max(1, st.borderW));
        const inset = bw * 0.5;
        roundRectPath(ctx, rect.x + inset, rect.y + inset, Math.max(0, rect.w - bw), Math.max(0, rect.h - bw),
          radii.map(r => Math.max(0, r - inset)));
        ctx.strokeStyle = bc ? rgbaStr(bc) : activeBorder;
        ctx.lineWidth = bw;
        ctx.stroke();
      } else if (isHovered && k === "button") {
        roundRectPath(ctx, rect.x + 0.75, rect.y + 0.75, Math.max(0, rect.w - 1.5), Math.max(0, rect.h - 1.5), radii);
        ctx.strokeStyle = "rgba(255, 255, 255, 0.28)";
        ctx.lineWidth = 1.4;
        ctx.stroke();
      }
    }

    // 4b. Crisp 2px rounded `#38bdf8` accessibility focus ring on keyboard-focused widget
    if (isKeyFocused) {
      const ringRadii = radii.map(r => r > 0 ? r + 2.5 : 6);
      roundRectPath(ctx, rect.x - 2.5, rect.y - 2.5, rect.w + 5, rect.h + 5, ringRadii);
      ctx.strokeStyle = "#38bdf8";
      ctx.lineWidth = 2.0;
      ctx.stroke();
    }

    // 5. Text & Syntax-Highlighted <Code> rendering
    if (k === "code") {
      const [pt, pr, , pl] = effectivePad(st, k);
      if (node.action) {
        ctx.save();
        ctx.font = `600 10px ${MONO_FONT}`;
        const label = node.action.toUpperCase();
        ctx.fillStyle = "rgba(148, 163, 184, 0.55)";
        ctx.textBaseline = "top";
        ctx.textAlign = "right";
        ctx.fillText(label, rect.x + rect.w - Math.max(10, pr * 0.75), rect.y + Math.max(8, pt * 0.55));
        ctx.restore();
      }
      ctx.save();
      roundRectPath(ctx, rect.x, rect.y, rect.w, rect.h, radii);
      ctx.clip();
      ctx.textBaseline = "top";
      ctx.textAlign = "left";
      for (let i = 0; i < laid.wrappedLines.length; i++) {
        const line = laid.wrappedLines[i];
        const ly = rect.y + pt + i * laid.lineHeight;
        let cx = rect.x + pl;
        const toks = tokenizeCodeLine(line);
        for (const tok of toks) {
          applyFont(ctx, st, "code", Boolean(tok.italic));
          ctx.fillStyle = tok.color;
          ctx.fillText(tok.text, cx, ly);
          cx += ctx.measureText(tok.text).width;
        }
      }
      ctx.restore();
    } else if (
      laid.wrappedLines.length > 0 &&
      k !== "shader" && k !== "image" && k !== "slider" && k !== "divider" && k !== "progress" && k !== "icon"
    ) {
      const [pt, pr, , pl] = effectivePad(st, k);
      applyFont(ctx, st, k);
      ctx.fillStyle = st.color || "#f8fafc";
      ctx.textBaseline = "top";
      const fs = Math.max(8, st.fontSize || 15);
      const textH = Math.ceil(fs * 1.16);

      if (k === "button" || k === "badge") {
        ctx.textAlign = "center";
        ctx.textBaseline = "middle";
        ctx.fillText(laid.wrappedLines[0], rect.x + rect.w * 0.5, rect.y + rect.h * 0.5 + 0.5);
      } else if (k === "input") {
        ctx.textAlign = "left";
        ctx.textBaseline = "middle";
        const tx = rect.x + Math.max(12, pl);
        const cy = rect.y + rect.h * 0.5 + 0.5;
        const lineText = laid.wrappedLines[0] || "";
        if (isFocused && document.activeElement === imeInput) {
          const selS = Math.min(lineText.length, Math.max(0, imeInput.selectionStart || 0));
          const selE = Math.min(lineText.length, Math.max(selS, imeInput.selectionEnd || 0));
          if (selE > selS) {
            const x1 = tx + measureTextW(lineText.slice(0, selS), st, "input");
            const x2 = tx + measureTextW(lineText.slice(0, selE), st, "input");
            ctx.fillStyle = "rgba(56, 189, 248, 0.28)";
            ctx.fillRect(x1, rect.y + (rect.h - textH) * 0.5 - 1, Math.max(2, x2 - x1), textH + 2);
          }
        }
        ctx.fillStyle = st.color || "#f8fafc";
        ctx.fillText(lineText, tx, cy);
        if (isFocused && Math.floor(elapsed * 2) % 2 === 0) {
          const caretIdx = document.activeElement === imeInput
            ? Math.min(lineText.length, Math.max(0, imeInput.selectionEnd || 0))
            : lineText.length;
          const tw = measureTextW(lineText.slice(0, caretIdx), st, "input");
          ctx.fillStyle = "#38bdf8";
          ctx.fillRect(tx + tw + 1, rect.y + (rect.h - textH) * 0.5, 2, textH);
        }
      } else if (k === "toggle") {
        ctx.textAlign = "left";
        ctx.textBaseline = "middle";
        ctx.fillText(laid.wrappedLines[0], rect.x + pl + 50, rect.y + rect.h * 0.5 + 0.5);
      } else {
        const ta = st.textAlign || "left";
        ctx.textAlign = ta === "center" ? "center" : (ta === "right" ? "right" : "left");
        const baseX = ta === "center" ? (rect.x + rect.w * 0.5) : (ta === "right" ? (rect.x + rect.w - pr) : (rect.x + pl));
        for (let i = 0; i < laid.wrappedLines.length; i++) {
          const ln = laid.wrappedLines[i];
          if (ln) ctx.fillText(ln, baseX, rect.y + pt + i * laid.lineHeight);
        }
      }
    } else if (k === "input" && isFocused && Math.floor(elapsed * 2) % 2 === 0) {
      const [, , , pl] = effectivePad(st, k);
      const fs = Math.max(8, st.fontSize || 15);
      const caretH = Math.ceil(fs * 1.18);
      ctx.fillStyle = "#38bdf8";
      ctx.fillRect(rect.x + Math.max(12, pl), rect.y + (rect.h - caretH) * 0.5, 2, caretH);
    }

    // 6. Children (ascending z_index stable sort, overflow clip, H/V scrollbars)
    if (laid.children.length > 0) {
      const canScrollY = isScrollableY(node);
      const canScrollX = isScrollableX(node);
      const maxScrollY = canScrollY ? Math.max(0, (laid.contentH || rect.h) - rect.h) : 0;
      const maxScrollX = canScrollX ? Math.max(0, (laid.contentW || rect.w) - rect.w) : 0;
      const scrollKey = laid.scrollKey || "";
      if ((maxScrollY > 1 || maxScrollX > 1) && scrollKey) {
        scrollRegions.push({ key: scrollKey, rect, maxScrollX, maxScrollY });
      }
      const clipKids = clipsChildren(node);
      if (clipKids) {
        ctx.save();
        roundRectPath(ctx, rect.x, rect.y, rect.w, rect.h, radii);
        ctx.clip();
      }
      if (childrenNeedZSort(laid.children)) {
        for (const idx of sortedChildIndicesByZ(laid.children)) {
          paintNode(ctx, laid.children[idx], elapsed, dt, dpr, scrollY, viewportH);
        }
      } else {
        for (const child of laid.children) {
          paintNode(ctx, child, elapsed, dt, dpr, scrollY, viewportH);
        }
      }
      if (clipKids) ctx.restore();

      // 7. Sleek rounded vertical & horizontal scrollbar thumbs
      const hasV = canScrollY && maxScrollY > 2 && rect.h > 24 && rect.w > 16;
      const hasH = canScrollX && maxScrollX > 2 && rect.w > 24 && rect.h > 16;
      if (hasV || hasH) {
        const isActive = Boolean(activeScrollKey && activeScrollKey === scrollKey) || isHovered;
        const thumbColor = isActive ? "rgba(56, 189, 248, 0.55)" : "rgba(148, 163, 184, 0.34)";
        if (hasV) {
          const corner = hasH ? 8 : 0;
          const trackH = Math.max(4, rect.h - 8 - corner);
          const maxT = Math.max(4, trackH - 4);
          const minT = Math.min(20, maxT);
          const thumbH = Math.min(maxT, Math.max(minT, (rect.h / (laid.contentH || rect.h)) * trackH));
          const ratio = Math.min(1, Math.max(0, (laid.scrollY || 0) / Math.max(1, maxScrollY)));
          const thumbY = rect.y + 4 + ratio * Math.max(0, trackH - thumbH);
          roundRectPath(ctx, rect.x + rect.w - 7, thumbY, 5, thumbH, [2.5, 2.5, 2.5, 2.5]);
          ctx.fillStyle = thumbColor;
          ctx.fill();
        }
        if (hasH) {
          const corner = hasV ? 8 : 0;
          const trackW = Math.max(4, rect.w - 8 - corner);
          const maxT = Math.max(4, trackW - 4);
          const minT = Math.min(20, maxT);
          const thumbW = Math.min(maxT, Math.max(minT, (rect.w / (laid.contentW || rect.w)) * trackW));
          const ratio = Math.min(1, Math.max(0, (laid.scrollX || 0) / Math.max(1, maxScrollX)));
          const thumbX = rect.x + 4 + ratio * Math.max(0, trackW - thumbW);
          roundRectPath(ctx, thumbX, rect.y + rect.h - 7, thumbW, 5, [2.5, 2.5, 2.5, 2.5]);
          ctx.fillStyle = thumbColor;
          ctx.fill();
        }
      }
    }

    ctx.restore();
  }

  function paintScrollbar(ctx, viewportW, viewportH, scrollY, contentH) {
    if (contentH <= viewportH + 2 || viewportH <= 32) return;
    const maxThumb = Math.max(16, viewportH - 16);
    const thumbH = Math.min(maxThumb, Math.max(32, (viewportH / contentH) * viewportH));
    const maxScroll = Math.max(1, contentH - viewportH);
    const ratio = Math.min(1, Math.max(0, scrollY / maxScroll));
    const thumbY = 6 + ratio * Math.max(0, viewportH - thumbH - 12);
    const trackX = viewportW - 9;

    roundRectPath(ctx, trackX, 6, 5, viewportH - 12, [2.5, 2.5, 2.5, 2.5]);
    ctx.fillStyle = "rgba(15, 23, 42, 0.30)";
    ctx.fill();

    roundRectPath(ctx, trackX, thumbY, 5, thumbH, [2.5, 2.5, 2.5, 2.5]);
    ctx.fillStyle = draggingScrollbar ? "rgba(56, 189, 248, 0.55)" : "rgba(148, 163, 184, 0.34)";
    ctx.fill();
  }

  function activeRendererLabel() {
    let sawWebGl2 = false;
    for (const entry of shaderPool.values()) {
      if (entry && entry.gpu) {
        if (entry.gpu.kind === "webgpu") return "WebGPU";
        if (entry.gpu.kind === "webgl2") sawWebGl2 = true;
      }
    }
    return sawWebGl2 ? "WebGL2" : "Canvas2D";
  }

  function paintTelemetryHud(ctx, viewportW, viewportH) {
    if (!showTelemetryHud) return;
    const fps = Math.round(1000 / Math.max(1, smoothedFrameMs));
    const capKb = lastHeapCapacityBytes > 0
      ? (lastHeapCapacityBytes / 1024).toFixed(0)
      : (exports && exports.memory ? (exports.memory.buffer.byteLength / 1024).toFixed(0) : "0");
    const label = `${fps} FPS  ·  ${smoothedFrameMs.toFixed(1)} ms  ·  ${lastHeapFrameBytes} B/frame  ·  ${capKb} KB  ·  ${activeRendererLabel()}`;

    ctx.save();
    ctx.font = `600 11px ${MONO_FONT}`;
    const textW = Math.ceil(ctx.measureText(label).width);
    const pillW = textW + 30;
    const pillH = 26;
    const x = Math.max(10, viewportW - pillW - 14);
    const y = Math.max(10, viewportH - pillH - 14);

    ctx.shadowColor = "rgba(2, 6, 23, 0.55)";
    ctx.shadowBlur = 14;
    ctx.shadowOffsetY = 4;
    roundRectPath(ctx, x, y, pillW, pillH, [9, 9, 9, 9]);
    ctx.fillStyle = "rgba(9, 14, 28, 0.88)";
    ctx.fill();
    ctx.shadowBlur = 0;
    ctx.shadowOffsetY = 0;
    ctx.strokeStyle = "rgba(56, 189, 248, 0.45)";
    ctx.lineWidth = 1;
    ctx.stroke();

    ctx.beginPath();
    ctx.arc(x + 11, y + pillH * 0.5, 3.2, 0, Math.PI * 2);
    ctx.fillStyle = "#38bdf8";
    ctx.fill();

    ctx.fillStyle = "#e2e8f0";
    ctx.textAlign = "left";
    ctx.textBaseline = "middle";
    ctx.fillText(label, x + 20, y + pillH * 0.5 + 0.5);
    ctx.restore();
  }

  // ========================================================================
  // 9. App Lifecycle, Interaction, Scroll, Slider Drag, Keyboard & IME Bridge
  // ========================================================================
  const exports = await loadWasm();
  if (typeof exports.init !== "function" || typeof exports.view !== "function") return;

  let state = exports.init();
  let currentTree = null;
  let currentLayout = null;
  let hitBoxes = [];
  let focusableWidgets = [];
  let contentHeight = 0;
  let viewportW = window.innerWidth || 1280;
  let viewportH = window.innerHeight || 800;
  let dpr = Math.min(3, window.devicePixelRatio || 1);

  let scrollY = 0;
  let targetScrollY = 0;
  const nestedScrollY = new Map();
  const nestedScrollX = new Map();
  let scrollRegions = [];
  let activeScrollKey = null;
  let activeScrollTimer = null;
  let pointerX = 0, pointerY = 0;
  let pointerIsDown = false;
  let hoveredAction = null;
  let activeSlider = null;
  let focusedInput = null;
  let focusedControl = null;
  let focusedWidgetKey = null;
  let draggingScrollbar = false;
  let scrollDragStartY = 0;
  let scrollDragStartScroll = 0;
  let touchDragging = false;
  let touchStartY = 0;
  let touchStartScroll = 0;
  let touchMoved = false;
  let needsLayout = true;
  let needsPaint = true;

  // Zero-GC Telemetry HUD state (`Alt+Shift+D`)
  let showTelemetryHud = false;
  let smoothedFrameMs = 16.6;
  let lastHeapFrameBytes = 0;
  let lastHeapCapacityBytes = typeof exports.__knr_heap_capacity === "function"
    ? Number(exports.__knr_heap_capacity())
    : (exports.memory ? exports.memory.buffer.byteLength : 0);
  window.__knr_toggle_hud = () => { showTelemetryHud = !showTelemetryHud; needsPaint = true; };

  let resizeDispatchTimer = null;
  function dispatchResize(w, h) {
    if (typeof exports.on_resize === "function" && ON_RESIZE_ARITY >= 3) {
      const wArg = ON_RESIZE_W_I64 ? BigInt(Math.round(w)) : w;
      const hArg = ON_RESIZE_H_I64 ? BigInt(Math.round(h)) : h;
      state = exports.on_resize(state, wArg, hArg);
      evaluateView();
      runEffectHook(`resize:${Math.round(w)}x${Math.round(h)}`);
    }
  }

  function resizeSurface(notifyWasm = false) {
    viewportW = Math.max(1, window.innerWidth || 1280);
    viewportH = Math.max(1, window.innerHeight || 800);
    dpr = Math.min(3, window.devicePixelRatio || 1);
    surface.width = Math.round(viewportW * dpr);
    surface.height = Math.round(viewportH * dpr);
    surface.style.width = viewportW + "px";
    surface.style.height = viewportH + "px";
    needsLayout = true;
    needsPaint = true;
    if (notifyWasm && typeof exports.on_resize === "function" && ON_RESIZE_ARITY >= 3) {
      if (resizeDispatchTimer) clearTimeout(resizeDispatchTimer);
      resizeDispatchTimer = setTimeout(() => {
        resizeDispatchTimer = null;
        dispatchResize(viewportW, viewportH);
      }, 60);
    }
  }

  function evaluateView() {
    const usedBefore = typeof exports.__knr_heap_used === "function" ? Number(exports.__knr_heap_used()) : 0;
    const heapMark = exports.__knr_heap ? exports.__knr_heap.value : 0;
    const treePtr = exports.view(state);
    const usedAfter = typeof exports.__knr_heap_used === "function" ? Number(exports.__knr_heap_used()) : 0;
    lastHeapFrameBytes = Math.max(0, usedAfter - usedBefore);
    if (typeof exports.__knr_heap_capacity === "function") {
      lastHeapCapacityBytes = Number(exports.__knr_heap_capacity());
    }
    currentTree = IS_WIDGET_V2
      ? readWidgetV2(exports.memory, treePtr)
      : readUiNodeV1(exports.memory, treePtr);
    if (heapMark > 0 && exports.__knr_heap) exports.__knr_heap.value = heapMark;
    needsLayout = true;
    needsPaint = true;
  }

  function recomputeLayout() {
    if (!currentTree) return;
    hitBoxes = [];
    focusableWidgets = [];
    currentLayout = layoutNode(currentTree, 0, 0, viewportW, viewportH, hitBoxes, focusableWidgets, "root");
    contentHeight = Math.max(viewportH, currentLayout.rect.h);
    const maxScroll = Math.max(0, contentHeight - viewportH);
    scrollY = Math.min(maxScroll, Math.max(0, scrollY));
    targetScrollY = Math.min(maxScroll, Math.max(0, targetScrollY));
    syncImeInputPosition();
    if (focusedControl) {
      const freshCtrl = hitBoxes.find(h => h.action === focusedControl.action);
      if (freshCtrl) focusedControl = freshCtrl;
      else { focusedControl = null; if (!focusedInput) focusedWidgetKey = null; }
    }
    needsLayout = false;
  }

  function hitTestAt(clientX, clientY) {
    const wy = clientY + scrollY;
    for (let i = hitBoxes.length - 1; i >= 0; i--) {
      const hb = hitBoxes[i];
      const r = hb.hitRect || hb.rect;
      if (clientX >= r.x && clientX <= r.x + r.w && wy >= r.y && wy <= r.y + r.h) {
        return hb;
      }
    }
    return null;
  }

  function syncImeInputPosition() {
    if (!focusedInput) {
      imeInput.style.left = "-9999px";
      imeInput.style.top = "-9999px";
      imeInput.style.width = "1px";
      imeInput.style.height = "1px";
      imeInput.style.pointerEvents = "none";
      return;
    }
    const fresh = hitBoxes.find(h => h.kind === "input" && h.action === focusedInput.action);
    if (!fresh) {
      focusedInput = null;
      if (!focusedControl) focusedWidgetKey = null;
      imeInput.blur();
      syncImeInputPosition();
      return;
    }
    focusedInput = fresh;
    imeInput.style.left = `${fresh.rect.x}px`;
    imeInput.style.top = `${fresh.rect.y - scrollY}px`;
    imeInput.style.width = `${Math.max(40, fresh.rect.w)}px`;
    imeInput.style.height = `${Math.max(24, fresh.rect.h)}px`;
    imeInput.style.pointerEvents = "auto";
    if (document.activeElement !== imeInput && imeInput.value !== fresh.text) {
      imeInput.value = fresh.text;
    }
  }

  function readCmd(mem, ptr) {
    if (!ptr || ptr === 0n) return null;
    const p = Number(ptr);
    const dv = new DataView(mem.buffer);
    const kind = readStr(mem, dv.getBigInt64(p + 0, true));
    if (!kind || kind === "none") return null;
    return {
      kind,
      target: readStr(mem, dv.getBigInt64(p + 8, true)),
      payload: readStr(mem, dv.getBigInt64(p + 16, true)),
      onResult: readStr(mem, dv.getBigInt64(p + 24, true)),
      delayMs: dv.getFloat64(p + 32, true),
    };
  }

  function dispatchInputOrAction(action, value) {
    if (!action || typeof exports.__knr_alloc !== "function") return;
    if (typeof exports.on_input === "function") {
      state = exports.on_input(state, allocStr(exports, action), allocStr(exports, value));
      evaluateView();
      runEffectHook(action);
    } else {
      dispatchAction(`${action}:${value}`);
    }
  }

  function runEffectHook(action) {
    if (typeof exports.effect !== "function" || typeof exports.__knr_alloc !== "function") return;
    const mark = exports.__knr_heap ? exports.__knr_heap.value : 0;
    const cmdPtr = exports.effect(state, allocStr(exports, action));
    const cmd = readCmd(exports.memory, cmdPtr);
    if (mark > 0 && exports.__knr_heap) exports.__knr_heap.value = mark;
    if (!cmd) return;
    if (cmd.kind === "timer" && cmd.onResult) {
      setTimeout(() => dispatchAction(cmd.onResult), Math.max(0, cmd.delayMs || 0));
    } else if (cmd.kind === "http_get" && cmd.target) {
      fetch(cmd.target).then(r => r.text()).then(txt => dispatchInputOrAction(cmd.onResult, txt)).catch(err => dispatchInputOrAction(cmd.onResult, `error:${err.message || err}`));
    } else if (cmd.kind === "http_post" && cmd.target) {
      fetch(cmd.target, { method: "POST", body: cmd.payload || "" }).then(r => r.text()).then(txt => dispatchInputOrAction(cmd.onResult, txt)).catch(err => dispatchInputOrAction(cmd.onResult, `error:${err.message || err}`));
    } else if (cmd.kind === "storage_save" && cmd.target) {
      try { localStorage.setItem("knr:" + cmd.target, cmd.payload || ""); } catch (_) {}
      if (cmd.onResult) dispatchAction(cmd.onResult);
    } else if (cmd.kind === "storage_load" && cmd.target) {
      let val = "";
      try { val = localStorage.getItem("knr:" + cmd.target) || ""; } catch (_) {}
      if (cmd.onResult) dispatchInputOrAction(cmd.onResult, val);
    }
  }

  function dispatchAction(action) {
    if (typeof exports.update === "function" && typeof exports.__knr_alloc === "function") {
      state = exports.update(state, allocStr(exports, action));
      evaluateView();
      runEffectHook(action);
    }
  }
  window.__knr_dispatch = dispatchAction;

  function dispatchSliderValue(hb, value) {
    const t = Math.min(1, Math.max(0, value));
    if (typeof exports.on_slider === "function" && typeof exports.__knr_alloc === "function") {
      state = exports.on_slider(state, allocStr(exports, hb.action), t);
      evaluateView();
      runEffectHook(hb.action);
    } else {
      dispatchAction(`${hb.action}:${t.toFixed(3)}`);
    }
  }

  function dispatchSlider(hb, clientX) {
    const t = Math.min(1, Math.max(0, (clientX - hb.rect.x) / Math.max(1, hb.rect.w)));
    dispatchSliderValue(hb, t);
  }

  function formatKeyCombo(e) {
    let raw = e.key || "";
    if (!raw || raw === "Shift" || raw === "Control" || raw === "Alt" || raw === "Meta" || raw === "Dead" || raw === "Unidentified") {
      return "";
    }
    if (raw === " " || e.code === "Space") raw = "Space";
    else if (raw === "Esc") raw = "Escape";
    const ctrl = Boolean(e.ctrlKey || e.metaKey);
    const alt = Boolean(e.altKey);
    const shift = Boolean(e.shiftKey);
    const normalized = (raw.length === 1 && (ctrl || alt)) ? raw.toUpperCase() : raw;
    const parts = [];
    if (ctrl) parts.push("Ctrl");
    if (alt) parts.push("Alt");
    const isNamed = normalized.length > 1;
    if (shift && (ctrl || alt || isNamed)) parts.push("Shift");
    parts.push(normalized);
    return parts.join("+");
  }

  function dispatchKey(combo) {
    if (!combo) return;
    if (typeof exports.on_key === "function" && typeof exports.__knr_alloc === "function") {
      state = exports.on_key(state, allocStr(exports, combo));
      evaluateView();
      runEffectHook(`key:${combo}`);
    }
  }

  function getUniqueFocusables() {
    const out = [];
    const seen = new Set();
    for (const hb of focusableWidgets) {
      if (!hb.action || seen.has(hb.action)) continue;
      seen.add(hb.action);
      out.push(hb);
    }
    return out;
  }

  function focusWidgetByKeyboard(hb) {
    if (!hb) return;
    focusedWidgetKey = hb.key || hb.action;
    if (hb.kind === "input") {
      focusedControl = null;
      focusedInput = hb;
      imeInput.value = hb.text || "";
      syncImeInputPosition();
      imeInput.focus();
      imeInput.select();
    } else {
      focusedInput = null;
      focusedControl = hb;
      imeInput.blur();
      syncImeInputPosition();
    }
    // Keep focused widget visible inside viewport
    if (hb.rect.y < scrollY + 12) {
      targetScrollY = Math.max(0, hb.rect.y - 24);
    } else if (hb.rect.y + hb.rect.h > scrollY + viewportH - 12) {
      const maxScroll = Math.max(0, contentHeight - viewportH);
      targetScrollY = Math.min(maxScroll, hb.rect.y + hb.rect.h - viewportH + 24);
    }
    needsPaint = true;
  }

  function cycleTabFocus(backwards) {
    const focusables = getUniqueFocusables();
    if (focusables.length === 0) return false;
    const curAct = (focusedInput && focusedInput.action) || (focusedControl && focusedControl.action) || null;
    const curIdx = curAct ? focusables.findIndex(h => h.action === curAct) : -1;
    let nextIdx;
    if (curIdx < 0) nextIdx = backwards ? focusables.length - 1 : 0;
    else nextIdx = (curIdx + (backwards ? -1 : 1) + focusables.length) % focusables.length;
    focusWidgetByKeyboard(focusables[nextIdx]);
    return true;
  }

  imeInput.addEventListener("input", () => {
    if (!focusedInput || typeof exports.__knr_alloc !== "function") return;
    if (typeof exports.on_input === "function") {
      state = exports.on_input(state, allocStr(exports, focusedInput.action), allocStr(exports, imeInput.value));
      evaluateView();
    } else {
      dispatchAction(`${focusedInput.action}:${imeInput.value}`);
    }
  });
  imeInput.addEventListener("keyup", () => { needsPaint = true; });
  imeInput.addEventListener("select", () => { needsPaint = true; });
  imeInput.addEventListener("blur", () => {
    if (focusedInput) {
      focusedInput = null;
      if (!focusedControl) focusedWidgetKey = null;
      syncImeInputPosition();
      needsPaint = true;
    }
  });

  window.addEventListener("keydown", (e) => {
    // Toggleable Zero-GC Telemetry HUD (`Alt+Shift+D`)
    if (e.altKey && e.shiftKey && (e.key === "D" || e.key === "d" || e.code === "KeyD")) {
      e.preventDefault();
      showTelemetryHud = !showTelemetryHud;
      needsPaint = true;
      return;
    }

    const isImeActive = Boolean(focusedInput && document.activeElement === imeInput);

    // Escape always clears focus and forwards to on_key
    if (e.key === "Escape") {
      if (focusedInput || focusedControl || focusedWidgetKey) {
        focusedInput = null;
        focusedControl = null;
        focusedWidgetKey = null;
        imeInput.blur();
        syncImeInputPosition();
        needsPaint = true;
      }
      const combo = formatKeyCombo(e);
      if (combo) dispatchKey(combo);
      return;
    }

    // Tab / Shift+Tab cycles focus across ALL focusable widgets (input, button, toggle, slider)
    if (e.key === "Tab") {
      e.preventDefault();
      if (!cycleTabFocus(e.shiftKey)) {
        const combo = formatKeyCombo(e);
        if (combo) dispatchKey(combo);
      }
      return;
    }

    // When an <Input> is actively focused
    if (isImeActive) {
      if (e.key === "Enter") {
        e.preventDefault();
        dispatchAction(`${focusedInput.action}_submit`);
        return;
      }
      if (e.ctrlKey || e.metaKey || e.altKey) {
        const lower = (e.key || "").toLowerCase();
        const isNativeEdit = !e.altKey && (lower === "a" || lower === "c" || lower === "v" || lower === "x" || lower === "z" || lower === "y");
        if (!isNativeEdit) {
          const combo = formatKeyCombo(e);
          if (combo && typeof exports.on_key === "function") {
            e.preventDefault();
            dispatchKey(combo);
          }
        }
        return;
      }
      needsPaint = true;
      return;
    }

    // Keyboard activation & adjustment on focused non-input controls
    if (focusedControl && !e.ctrlKey && !e.metaKey && !e.altKey) {
      const fresh = hitBoxes.find(h => h.action === focusedControl.action) || focusedControl;
      if ((e.key === "Enter" || e.key === " " || e.code === "Space") && (fresh.kind === "button" || fresh.kind === "toggle")) {
        e.preventDefault();
        dispatchAction(fresh.action);
        return;
      }
      if ((e.key === "ArrowLeft" || e.key === "ArrowRight") && fresh.kind === "slider") {
        e.preventDefault();
        const step = e.shiftKey ? 0.01 : 0.05;
        const cur = Math.min(1, Math.max(0, fresh.value || 0));
        const nextVal = Math.min(1, Math.max(0, cur + (e.key === "ArrowRight" ? step : -step)));
        dispatchSliderValue(fresh, nextVal);
        return;
      }
    }

    // Global on_key dispatch when no <Input> is consuming typed text
    const combo = formatKeyCombo(e);
    if (combo && typeof exports.on_key === "function") {
      if (e.ctrlKey || e.metaKey || e.altKey || combo === "Space" || combo.startsWith("Arrow")) {
        e.preventDefault();
      }
      dispatchKey(combo);
    }
  });

  surface.addEventListener("pointermove", (e) => {
    pointerX = e.clientX;
    pointerY = e.clientY;
    if (draggingScrollbar) {
      const maxScroll = Math.max(0, contentHeight - viewportH);
      const dy = e.clientY - scrollDragStartY;
      targetScrollY = scrollY = Math.min(maxScroll, Math.max(0, scrollDragStartScroll + (dy / Math.max(1, viewportH)) * contentHeight));
      syncImeInputPosition();
      needsPaint = true;
      return;
    }
    if (activeSlider) {
      dispatchSlider(activeSlider, e.clientX);
      return;
    }
    if (touchDragging) {
      const dy = touchStartY - e.clientY;
      if (Math.abs(dy) > 4) touchMoved = true;
      const maxScroll = Math.max(0, contentHeight - viewportH);
      targetScrollY = scrollY = Math.min(maxScroll, Math.max(0, touchStartScroll + dy));
      syncImeInputPosition();
      needsPaint = true;
      return;
    }
    const hb = hitTestAt(e.clientX, e.clientY);
    const nextHover = hb ? hb.action : null;
    if (nextHover !== hoveredAction) {
      hoveredAction = nextHover;
      needsPaint = true;
    }
    if (!hb) surface.style.cursor = "default";
    else if (hb.kind === "input") surface.style.cursor = "text";
    else if (hb.kind === "slider") surface.style.cursor = "ew-resize";
    else surface.style.cursor = "pointer";
  });

  surface.addEventListener("pointerdown", (e) => {
    pointerX = e.clientX;
    pointerY = e.clientY;
    pointerIsDown = true;
    if (contentHeight > viewportH + 2 && e.clientX >= viewportW - 16) {
      draggingScrollbar = true;
      scrollDragStartY = e.clientY;
      scrollDragStartScroll = scrollY;
      surface.setPointerCapture(e.pointerId);
      needsPaint = true;
      return;
    }
    const hb = hitTestAt(e.clientX, e.clientY);
    if (hb && hb.kind === "slider") {
      activeSlider = hb;
      focusedControl = null;
      focusedWidgetKey = null;
      surface.setPointerCapture(e.pointerId);
      dispatchSlider(hb, e.clientX);
      return;
    }
    if (e.pointerType === "touch") {
      touchDragging = true;
      touchMoved = false;
      touchStartY = e.clientY;
      touchStartScroll = scrollY;
      surface.setPointerCapture(e.pointerId);
    }
  });

  surface.addEventListener("pointerup", (e) => {
    pointerIsDown = false;
    if (draggingScrollbar) { draggingScrollbar = false; needsPaint = true; return; }
    if (activeSlider) { activeSlider = null; needsPaint = true; return; }
    const wasTouchScroll = touchDragging && touchMoved;
    touchDragging = false;
    if (wasTouchScroll) return;

    const hb = hitTestAt(e.clientX, e.clientY);
    if (hb && hb.kind === "input") {
      focusedControl = null;
      focusedInput = hb;
      focusedWidgetKey = hb.key || hb.action;
      imeInput.value = hb.text || "";
      syncImeInputPosition();
      imeInput.focus();
      needsPaint = true;
      return;
    }
    if (focusedInput || focusedControl || focusedWidgetKey) {
      focusedInput = null;
      focusedControl = null;
      focusedWidgetKey = null;
      imeInput.blur();
      syncImeInputPosition();
      needsPaint = true;
    }
    if (hb && hb.action && hb.kind !== "slider") {
      dispatchAction(hb.action);
    }
  });

  function markScrollRegionActive(key) {
    activeScrollKey = key;
    if (activeScrollTimer) clearTimeout(activeScrollTimer);
    activeScrollTimer = setTimeout(() => {
      activeScrollKey = null;
      activeScrollTimer = null;
      needsPaint = true;
    }, 650);
  }

  surface.addEventListener("wheel", (e) => {
    const wy = e.clientY + scrollY;
    const dx = (e.shiftKey && Math.abs(e.deltaX) < 0.01) ? e.deltaY : e.deltaX;
    const dy = (e.shiftKey && Math.abs(e.deltaX) < 0.01) ? 0 : e.deltaY;
    let consumed = false;

    for (let i = scrollRegions.length - 1; i >= 0; i--) {
      const sr = scrollRegions[i];
      if (
        e.clientX >= sr.rect.x && e.clientX <= sr.rect.x + sr.rect.w &&
        wy >= sr.rect.y && wy <= sr.rect.y + sr.rect.h
      ) {
        if (Math.abs(dx) > 0.01 && sr.maxScrollX > 0) {
          const curX = nestedScrollX.get(sr.key) || 0;
          const nextX = Math.min(sr.maxScrollX, Math.max(0, curX + dx));
          if (Math.abs(nextX - curX) > 0.25) {
            nestedScrollX.set(sr.key, nextX);
            markScrollRegionActive(sr.key);
            consumed = true;
          }
        }
        if (Math.abs(dy) > 0.01 && sr.maxScrollY > 0) {
          const curY = nestedScrollY.get(sr.key) || 0;
          const nextY = Math.min(sr.maxScrollY, Math.max(0, curY + dy));
          if (Math.abs(nextY - curY) > 0.25) {
            nestedScrollY.set(sr.key, nextY);
            markScrollRegionActive(sr.key);
            consumed = true;
          }
        }
        if (consumed) {
          e.preventDefault();
          needsLayout = true;
          needsPaint = true;
          return;
        }
      }
    }
    const maxScroll = Math.max(0, contentHeight - viewportH);
    if (maxScroll <= 0 || Math.abs(dy) < 0.01) return;
    e.preventDefault();
    targetScrollY = Math.min(maxScroll, Math.max(0, targetScrollY + dy));
    needsPaint = true;
  }, { passive: false });

  window.addEventListener("resize", () => resizeSurface(true));
  if (document.fonts && document.fonts.ready) {
    document.fonts.ready.then(() => { needsLayout = true; needsPaint = true; });
  }

  resizeSurface(false);
  dispatchResize(viewportW, viewportH);
  evaluateView();
  recomputeLayout();

  const hasTick = typeof exports.tick === "function" && TICK_ARITY >= 2;
  const hasShaders = Object.keys(KNR_SHADERS).length > 0;
  let startMs = performance.now();
  let lastMs = startMs;

  function frame(now) {
    const rawFrameMs = Math.max(0.1, now - lastMs);
    smoothedFrameMs = smoothedFrameMs * 0.88 + Math.min(100, rawFrameMs) * 0.12;
    const dt = Math.min(0.05, Math.max(0.0001, rawFrameMs / 1000));
    const elapsed = (now - startMs) / 1000;
    lastMs = now;

    if (hasTick) {
      state = TICK_ARITY >= 3 ? exports.tick(state, dt, elapsed) : exports.tick(state, dt);
      evaluateView();
    }

    if (Math.abs(targetScrollY - scrollY) > 0.25) {
      scrollY += (targetScrollY - scrollY) * Math.min(1, dt * 16);
      syncImeInputPosition();
      needsPaint = true;
    } else if (scrollY !== targetScrollY) {
      scrollY = targetScrollY;
      syncImeInputPosition();
      needsPaint = true;
    }

    if (needsLayout) recomputeLayout();

    if (needsPaint || hasShaders || focusedInput || showTelemetryHud) {
      needsPaint = false;
      shaderVisitCounter = 0;
      scrollRegions = [];
      ctx.save();
      ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
      const rootBg = (currentLayout && currentLayout.node.style && currentLayout.node.style.bg) || "#050811";
      const bgFill = makeCanvasFill(ctx, rootBg, { x: 0, y: 0, w: viewportW, h: viewportH });
      ctx.fillStyle = bgFill || "#050811";
      ctx.fillRect(0, 0, viewportW, viewportH);

      if (currentLayout) {
        ctx.save();
        ctx.translate(0, -scrollY);
        paintNode(ctx, currentLayout, elapsed, dt, dpr, scrollY, viewportH);
        ctx.restore();
      }
      paintScrollbar(ctx, viewportW, viewportH, scrollY, contentHeight);
      paintTelemetryHud(ctx, viewportW, viewportH);
      ctx.restore();
    }

    requestAnimationFrame(frame);
  }
  requestAnimationFrame(frame);
})();
</script>
</body>
</html>
"##;
