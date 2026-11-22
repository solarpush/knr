use std::fs;
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

use knr_core::ExploreFilter;
use knr_interp::Value as VmValue;
use knr_runtime::mock::MockPlatform;
use knr_verify::Diagnostic;
use serde_json::{json, Value};

use crate::args::{RenderStep, VERSION};
use crate::commands::bench::run_benchmark;
use crate::commands::lsp::{apply_lsp_text_edits, compute_all_quickfixes_for_source};
use crate::commands::render::render_and_audit_for_mcp;
use crate::commands::skill::SKILL_MARKDOWN;
use crate::error::CliError;

const MCP_PROTOCOL_VERSION: &str = "2024-11-05";
const MCP_SERVER_NAME: &str = "knr-mcp";
const MAX_CONTENT_LENGTH: usize = 32 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FramingMode {
    LineDelimited,
    ContentLength,
}

/// Runs the native Model Context Protocol (MCP 2024-11-05) JSON-RPC 2.0 server over stdio.
/// Transparently handles BOTH newline-delimited JSON-RPC (standard MCP stdio transport)
/// and `Content-Length` framed JSON-RPC messages.
pub fn execute() -> Result<(), CliError> {
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut reader = stdin.lock();
    let mut writer = stdout.lock();

    loop {
        let (raw, mode) = match read_mcp_message(&mut reader) {
            Ok(Some(msg)) => msg,
            Ok(None) => break,
            Err(err) if err.kind() == io::ErrorKind::BrokenPipe => break,
            Err(err) => return Err(CliError::Io(err)),
        };

        let response_opt = match serde_json::from_str::<Value>(&raw) {
            Ok(msg) => handle_mcp_message(&msg),
            Err(err) => Some(json_rpc_error(
                Value::Null,
                -32700,
                &format!("Parse error: {err}"),
            )),
        };

        if let Some(response) = response_opt {
            if let Err(err) = write_mcp_message(&mut writer, &response, mode) {
                if err.kind() == io::ErrorKind::BrokenPipe {
                    return Ok(());
                }
                return Err(CliError::Io(err));
            }
        }
    }

    Ok(())
}

fn read_mcp_message<R: BufRead>(reader: &mut R) -> io::Result<Option<(String, FramingMode)>> {
    let mut first_line = String::new();
    loop {
        first_line.clear();
        let n = reader.read_line(&mut first_line)?;
        if n == 0 {
            return Ok(None);
        }
        let trimmed = first_line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.starts_with('{') || trimmed.starts_with('[') {
            return Ok(Some((trimmed.to_string(), FramingMode::LineDelimited)));
        }
        // Otherwise treat as Content-Length header framing
        let mut content_length: Option<usize> = parse_content_length_header(trimmed);
        let mut header_line = String::new();
        loop {
            header_line.clear();
            let hn = reader.read_line(&mut header_line)?;
            if hn == 0 {
                return Ok(None);
            }
            let ht = header_line.trim();
            if ht.is_empty() {
                break;
            }
            if let Some(len) = parse_content_length_header(ht) {
                content_length = Some(len);
            }
        }

        let Some(len) = content_length else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Missing Content-Length header in framed MCP message",
            ));
        };
        if len > MAX_CONTENT_LENGTH {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("MCP Content-Length {len} exceeds maximum limit"),
            ));
        }
        let mut buf = vec![0u8; len];
        reader.read_exact(&mut buf)?;
        let body = String::from_utf8(buf)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        return Ok(Some((body, FramingMode::ContentLength)));
    }
}

fn parse_content_length_header(line: &str) -> Option<usize> {
    let (k, v) = line.split_once(':')?;
    if k.trim().eq_ignore_ascii_case("content-length") {
        v.trim().parse::<usize>().ok()
    } else {
        None
    }
}

fn write_mcp_message<W: Write>(
    writer: &mut W,
    msg: &Value,
    mode: FramingMode,
) -> io::Result<()> {
    let body = serde_json::to_string(msg)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    match mode {
        FramingMode::LineDelimited => {
            writer.write_all(body.as_bytes())?;
            writer.write_all(b"\n")?;
        }
        FramingMode::ContentLength => {
            write!(writer, "Content-Length: {}\r\n\r\n", body.len())?;
            writer.write_all(body.as_bytes())?;
        }
    }
    writer.flush()
}

/// Pure, deterministic MCP JSON-RPC 2.0 message dispatcher for unit testing and stdio loop.
pub fn handle_mcp_message(msg: &Value) -> Option<Value> {
    let id = msg.get("id").cloned();
    let Some(method) = msg.get("method").and_then(Value::as_str) else {
        return id.map(|req_id| json_rpc_error(req_id, -32600, "Invalid Request: missing 'method'"));
    };
    let params = msg.get("params");

    match method {
        "initialize" => {
            let req_id = id?;
            Some(json_rpc_result(
                req_id,
                json!({
                    "protocolVersion": MCP_PROTOCOL_VERSION,
                    "capabilities": {
                        "tools": {}
                    },
                    "serverInfo": {
                        "name": MCP_SERVER_NAME,
                        "version": VERSION
                    }
                }),
            ))
        }
        "notifications/initialized" | "initialized" | "notifications/cancelled" => None,
        "ping" => {
            let req_id = id?;
            Some(json_rpc_result(req_id, json!({})))
        }
        "tools/list" => {
            let req_id = id?;
            Some(json_rpc_result(
                req_id,
                json!({
                    "tools": tool_definitions()
                }),
            ))
        }
        "tools/call" => {
            let req_id = id?;
            let result = dispatch_tool_call(params);
            Some(json_rpc_result(req_id, result))
        }
        other => id.map(|req_id| {
            json_rpc_error(req_id, -32601, &format!("Method not found: '{other}'"))
        }),
    }
}

fn json_rpc_result(id: Value, result: Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": result
    })
}

fn json_rpc_error(id: Value, code: i64, message: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {
            "code": code,
            "message": message
        }
    })
}

fn tool_text_ok(text: String) -> Value {
    json!({
        "content": [{ "type": "text", "text": text }],
        "isError": false
    })
}

fn tool_text_err(message: impl Into<String>) -> Value {
    json!({
        "content": [{ "type": "text", "text": message.into() }],
        "isError": true
    })
}

fn tool_definitions() -> Value {
    json!([
        {
            "name": "knr_check",
            "description": "Parse, statically verify linear ownership/types/effects, and validate all @shader kernels (WGSL + WebGL2 GLSL) in a .tsk, .knr, or .ssa file.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "file": {
                        "type": "string",
                        "description": "Path to the .tsk, .knr, or .ssa file to check."
                    }
                },
                "required": ["file"],
                "additionalProperties": false
            }
        },
        {
            "name": "knr_explore",
            "description": "Explore project structure, schemas, call graph, effects, and orphan symbols via ProjectGraph.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Path to a .tsk/.knr/.ssa file or project directory."
                    },
                    "format": {
                        "type": "string",
                        "enum": ["ai", "json", "mermaid"],
                        "description": "Output format (default: 'ai')."
                    },
                    "symbol": {
                        "type": "string",
                        "description": "Filter by schema or function symbol name."
                    },
                    "callers": {
                        "type": "string",
                        "description": "Show direct and transitive callers of function."
                    },
                    "callees": {
                        "type": "string",
                        "description": "Show direct and transitive callees of function."
                    },
                    "effects": {
                        "type": "string",
                        "description": "Filter functions declaring or invoking an effect (ui, net, hw, async)."
                    },
                    "orphans": {
                        "type": "boolean",
                        "description": "Return only orphan schemas and uncalled functions."
                    }
                },
                "required": ["path"],
                "additionalProperties": false
            }
        },
        {
            "name": "knr_test",
            "description": "Discover and deterministically execute all zero-argument test_* functions in a file or directory using the reference VM with mocked effects.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Path to a .tsk/.knr/.ssa file or directory."
                    },
                    "gas": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "Maximum VM gas steps per test (default: 1000000)."
                    }
                },
                "required": ["path"],
                "additionalProperties": false
            }
        },
        {
            "name": "knr_render_audit",
            "description": "Render a headless HiDPI UI frame (with real @shader previews), simulate clicks/inputs/sliders, run the automated visual layout/glyph/contrast/image/shader auditor, and optionally return a base64 PNG image.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "file": {
                        "type": "string",
                        "description": "Path to the .tsk, .knr, or .ssa UI file."
                    },
                    "width": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 8192,
                        "description": "Logical viewport width in pixels (default: 1280)."
                    },
                    "height": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 8192,
                        "description": "Logical viewport height in pixels (default: 800)."
                    },
                    "scale": {
                        "type": "number",
                        "minimum": 0.25,
                        "maximum": 4.0,
                        "description": "HiDPI scale factor (default: 2.0)."
                    },
                    "output": {
                        "type": "string",
                        "description": "Optional PNG output path on disk."
                    },
                    "clicks": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Sequence of widget keys or actions to click."
                    },
                    "inputs": {
                        "type": "object",
                        "additionalProperties": { "type": "string" },
                        "description": "Map of <Input key> -> text value to type before clicks."
                    },
                    "sliders": {
                        "type": "object",
                        "additionalProperties": { "type": "number" },
                        "description": "Map of <Slider key> -> float value in [0.0, 1.0]."
                    },
                    "include_png_base64": {
                        "type": "boolean",
                        "description": "When true, attaches a base64-encoded image/png MCP content block."
                    }
                },
                "required": ["file"],
                "additionalProperties": false
            }
        },
        {
            "name": "knr_fmt",
            "description": "Canonically format a .tsk or .knr source file or check whether formatting changes are needed.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "file": {
                        "type": "string",
                        "description": "Path to the .tsk or .knr file."
                    },
                    "write": {
                        "type": "boolean",
                        "description": "Write formatted source back to disk (default: false)."
                    }
                },
                "required": ["file"],
                "additionalProperties": false
            }
        },
        {
            "name": "knr_fix",
            "description": "Compute or apply deterministic compiler-verified QuickFixes (readonly borrows, @effects, missing std/ui imports, BorrowEscape) to a .tsk file.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "file": {
                        "type": "string",
                        "description": "Path to the .tsk or .knr file."
                    },
                    "write": {
                        "type": "boolean",
                        "description": "Apply QuickFixes directly to the file on disk (default: false)."
                    }
                },
                "required": ["file"],
                "additionalProperties": false
            }
        },
        {
            "name": "knr_bench",
            "description": "Benchmark deterministic VM gas/steps, Cranelift JIT compile & native execution speed, and .knrbin/.wasm artifact sizes.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "file": {
                        "type": "string",
                        "description": "Path to the .tsk, .knr, or .ssa file."
                    },
                    "fn_name": {
                        "type": "string",
                        "description": "Optional target function name (defaults to main / view(init()) / init)."
                    },
                    "iters": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 100000,
                        "description": "Number of measured iterations (default: 20)."
                    }
                },
                "required": ["file"],
                "additionalProperties": false
            }
        },
        {
            "name": "knr_get_skill",
            "description": "Return the official knr AI Agent language reference, Strict TSX (.tsk) UI guide, and mandatory verification workflow (SKILL.md).",
            "inputSchema": {
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }
        }
    ])
}

fn dispatch_tool_call(params: Option<&Value>) -> Value {
    let Some(params_obj) = params.and_then(Value::as_object) else {
        return tool_text_err("Missing or invalid 'params' object for tools/call");
    };
    let Some(name) = params_obj.get("name").and_then(Value::as_str) else {
        return tool_text_err("Missing required string field 'name' in tools/call params");
    };
    let empty_args = serde_json::Map::new();
    let args = params_obj
        .get("arguments")
        .and_then(Value::as_object)
        .unwrap_or(&empty_args);

    match name {
        "knr_check" => call_knr_check(args),
        "knr_explore" => call_knr_explore(args),
        "knr_test" => call_knr_test(args),
        "knr_render_audit" => call_knr_render_audit(args),
        "knr_fmt" => call_knr_fmt(args),
        "knr_fix" => call_knr_fix(args),
        "knr_bench" => call_knr_bench(args),
        "knr_get_skill" => tool_text_ok(SKILL_MARKDOWN.to_string()),
        other => tool_text_err(format!("Unknown MCP tool '{other}'")),
    }
}

fn required_str<'a>(args: &'a serde_json::Map<String, Value>, key: &str) -> Result<&'a str, Value> {
    args.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| tool_text_err(format!("Missing required string argument '{key}'")))
}

fn call_knr_check(args: &serde_json::Map<String, Value>) -> Value {
    let file_str = match required_str(args, "file") {
        Ok(s) => s,
        Err(e) => return e,
    };
    let path = Path::new(file_str);
    if !path.is_file() {
        return tool_text_err(format!("File not found: '{file_str}'"));
    }

    let module_res = if knr_core::is_source_file(path) {
        let src = match fs::read_to_string(path) {
            Ok(s) => s,
            Err(e) => return tool_text_err(format!("Failed to read '{file_str}': {e}")),
        };
        if let Err(err) = knr_core::syntax::parse_tsx(&src) {
            let diag = Diagnostic::new("ParseError", "<parse>", 1, err.to_string());
            let payload = json!({
                "ok": false,
                "file": file_str,
                "diagnostics": [diag],
                "shader_count": 0
            });
            return tool_text_ok(
                serde_json::to_string_pretty(&payload).unwrap_or_else(|_| payload.to_string()),
            );
        }
        knr_verify::verify_program_from_file(path)
    } else {
        match knr_core::load_module_from_path(path) {
            Ok(m) => match knr_verify::verify_module(&m) {
                Ok(()) => Ok(m),
                Err(diags) => Err(diags),
            },
            Err(err) => {
                let diag = Diagnostic::new("ParseError", "<parse>", 1, err.to_string());
                let payload = json!({
                    "ok": false,
                    "file": file_str,
                    "diagnostics": [diag],
                    "shader_count": 0
                });
                return tool_text_ok(
                    serde_json::to_string_pretty(&payload).unwrap_or_else(|_| payload.to_string()),
                );
            }
        }
    };

    match module_res {
        Ok(module) => match knr_codegen::WgslBackend::new().compile_all_shaders(&module) {
            Ok(shaders) => {
                let payload = json!({
                    "ok": true,
                    "file": file_str,
                    "diagnostics": [],
                    "shader_count": shaders.len()
                });
                tool_text_ok(
                    serde_json::to_string_pretty(&payload).unwrap_or_else(|_| payload.to_string()),
                )
            }
            Err(err) => {
                let diag = Diagnostic::new("ShaderCompileError", "<shader>", 1, err.to_string())
                    .with_fix_hint(
                        "Ensure @shader function uses pure GPU arithmetic/structs and returns Color/Vec4/Vec3 or f64",
                    );
                let payload = json!({
                    "ok": false,
                    "file": file_str,
                    "diagnostics": [diag],
                    "shader_count": 0
                });
                tool_text_ok(
                    serde_json::to_string_pretty(&payload).unwrap_or_else(|_| payload.to_string()),
                )
            }
        },
        Err(diagnostics) => {
            let payload = json!({
                "ok": false,
                "file": file_str,
                "diagnostics": diagnostics,
                "shader_count": 0
            });
            tool_text_ok(
                serde_json::to_string_pretty(&payload).unwrap_or_else(|_| payload.to_string()),
            )
        }
    }
}

fn call_knr_explore(args: &serde_json::Map<String, Value>) -> Value {
    let path_str = match required_str(args, "path") {
        Ok(s) => s,
        Err(e) => return e,
    };
    let path = Path::new(path_str);
    if !path.exists() {
        return tool_text_err(format!("Path does not exist: '{path_str}'"));
    }

    let format = args
        .get("format")
        .and_then(Value::as_str)
        .unwrap_or("ai")
        .to_ascii_lowercase();

    let graph = match knr_core::explore_path(path) {
        Ok(g) => g,
        Err(err) => return tool_text_err(format!("Failed to explore '{path_str}': {err}")),
    };

    let filter = ExploreFilter {
        symbol: args.get("symbol").and_then(Value::as_str).map(String::from),
        callers: args.get("callers").and_then(Value::as_str).map(String::from),
        callees: args.get("callees").and_then(Value::as_str).map(String::from),
        effects: args.get("effects").and_then(Value::as_str).map(String::from),
        orphans_only: args
            .get("orphans")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    };

    let rendered = match format.as_str() {
        "ai" => graph.format_ai(&filter),
        "json" => graph.format_json(&filter),
        "mermaid" => graph.format_mermaid(&filter),
        other => {
            return tool_text_err(format!(
                "Invalid format '{other}': expected 'ai', 'json', or 'mermaid'"
            ));
        }
    };

    tool_text_ok(rendered)
}

fn call_knr_test(args: &serde_json::Map<String, Value>) -> Value {
    let path_str = match required_str(args, "path") {
        Ok(s) => s,
        Err(e) => return e,
    };
    let path = Path::new(path_str);
    if !path.exists() {
        return tool_text_err(format!("Path does not exist: '{path_str}'"));
    }
    let gas = args
        .get("gas")
        .and_then(Value::as_u64)
        .unwrap_or(1_000_000);

    let files = match collect_test_source_files(path) {
        Ok(f) => f,
        Err(err) => return tool_text_err(err),
    };
    if files.is_empty() {
        return tool_text_err(format!(
            "No .tsk, .knr, or .ssa files found under '{path_str}'"
        ));
    }

    let mut results = Vec::new();
    let mut passed_count = 0usize;
    let mut failed_count = 0usize;

    for file in &files {
        let file_label = file.display().to_string();
        let module = if knr_core::is_source_file(file) {
            let src = match fs::read_to_string(file) {
                Ok(s) => s,
                Err(e) => return tool_text_err(format!("Failed to read '{file_label}': {e}")),
            };
            if let Err(err) = knr_core::syntax::parse_tsx(&src) {
                return tool_text_err(format!("Parse error in '{file_label}': {err}"));
            }
            match knr_verify::verify_program_from_file(file) {
                Ok(m) => m,
                Err(diags) => {
                    return tool_text_err(format!(
                        "Verification failed for '{file_label}' with {} diagnostic(s)",
                        diags.len()
                    ));
                }
            }
        } else {
            let m = match knr_core::load_module_from_path(file) {
                Ok(m) => m,
                Err(err) => return tool_text_err(format!("Parse error in '{file_label}': {err}")),
            };
            if let Err(diags) = knr_verify::verify_module(&m) {
                return tool_text_err(format!(
                    "Verification failed for '{file_label}' with {} diagnostic(s)",
                    diags.len()
                ));
            }
            m
        };

        let test_fns: Vec<String> = module
            .functions
            .iter()
            .filter(|f| f.name.starts_with("test_") && f.params.is_empty())
            .map(|f| f.name.clone())
            .collect();

        for fn_name in test_fns {
            let start = Instant::now();
            let mut mock = MockPlatform::new();
            let exec_res =
                knr_interp::execute_module(&module, &fn_name, vec![], &mut mock, gas, false);
            let duration_us = start.elapsed().as_micros();

            match exec_res {
                Ok((VmValue::Bool(true), _)) => {
                    passed_count += 1;
                    results.push(json!({
                        "file": file_label,
                        "name": fn_name,
                        "passed": true,
                        "duration_us": duration_us
                    }));
                }
                Ok((VmValue::I64(n), _)) if n != 0 => {
                    passed_count += 1;
                    results.push(json!({
                        "file": file_label,
                        "name": fn_name,
                        "passed": true,
                        "duration_us": duration_us
                    }));
                }
                Ok((val, _)) => {
                    failed_count += 1;
                    results.push(json!({
                        "file": file_label,
                        "name": fn_name,
                        "passed": false,
                        "duration_us": duration_us,
                        "error": format!("Assertion failed (returned {val})")
                    }));
                }
                Err(vm_err) => {
                    failed_count += 1;
                    results.push(json!({
                        "file": file_label,
                        "name": fn_name,
                        "passed": false,
                        "duration_us": duration_us,
                        "error": vm_err.to_string()
                    }));
                }
            }
        }
    }

    let total = passed_count + failed_count;
    let report = json!({
        "status": if failed_count == 0 { "ok" } else { "failed" },
        "total": total,
        "passed": passed_count,
        "failed": failed_count,
        "results": results
    });
    tool_text_ok(serde_json::to_string_pretty(&report).unwrap_or_else(|_| report.to_string()))
}

fn collect_test_source_files(path: &Path) -> Result<Vec<PathBuf>, String> {
    if path.is_file() {
        return Ok(vec![path.to_path_buf()]);
    }
    let mut out = Vec::new();
    visit_test_dir(path, &mut out)?;
    out.sort();
    Ok(out)
}

fn visit_test_dir(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
    let entries = fs::read_dir(dir).map_err(|e| format!("Failed to read '{}': {e}", dir.display()))?;
    for entry in entries {
        let entry = entry.map_err(|e| e.to_string())?;
        let p = entry.path();
        if p.is_dir() {
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if !name.starts_with('.') && name != "target" && name != "node_modules" {
                visit_test_dir(&p, out)?;
            }
        } else if knr_core::is_source_file(&p) || p.to_string_lossy().ends_with(".ssa") {
            out.push(p);
        }
    }
    Ok(())
}

fn call_knr_render_audit(args: &serde_json::Map<String, Value>) -> Value {
    let file_str = match required_str(args, "file") {
        Ok(s) => s,
        Err(e) => return e,
    };
    let path = Path::new(file_str);
    if !path.is_file() {
        return tool_text_err(format!("File not found: '{file_str}'"));
    }

    let width = args
        .get("width")
        .and_then(Value::as_u64)
        .unwrap_or(1280)
        .clamp(1, 8192) as u32;
    let height = args
        .get("height")
        .and_then(Value::as_u64)
        .unwrap_or(800)
        .clamp(1, 8192) as u32;
    let scale = args
        .get("scale")
        .and_then(Value::as_f64)
        .unwrap_or(2.0)
        .clamp(0.25, 4.0) as f32;
    let output = args.get("output").and_then(Value::as_str).map(PathBuf::from);
    let include_png_base64 = args
        .get("include_png_base64")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    let mut steps = Vec::new();
    if let Some(inputs_obj) = args.get("inputs").and_then(Value::as_object) {
        let mut keys: Vec<&String> = inputs_obj.keys().collect();
        keys.sort();
        for k in keys {
            if let Some(v) = inputs_obj.get(k).and_then(Value::as_str) {
                steps.push(RenderStep::Input {
                    key: k.clone(),
                    value: v.to_string(),
                });
            }
        }
    }
    if let Some(sliders_obj) = args.get("sliders").and_then(Value::as_object) {
        let mut keys: Vec<&String> = sliders_obj.keys().collect();
        keys.sort();
        for k in keys {
            if let Some(v) = sliders_obj.get(k).and_then(Value::as_f64) {
                steps.push(RenderStep::Slider {
                    key: k.clone(),
                    value: v.clamp(0.0, 1.0),
                });
            }
        }
    }
    if let Some(clicks_arr) = args.get("clicks").and_then(Value::as_array) {
        for item in clicks_arr {
            if let Some(key) = item.as_str() {
                steps.push(RenderStep::Click(key.to_string()));
            }
        }
    }

    match render_and_audit_for_mcp(
        path,
        width,
        height,
        scale,
        1.2,
        &steps,
        output.as_deref(),
        2_000_000,
    ) {
        Ok((report_json, pixmap)) => {
            let text = serde_json::to_string_pretty(&report_json)
                .unwrap_or_else(|_| report_json.to_string());
            let mut content = vec![json!({
                "type": "text",
                "text": text
            })];
            if include_png_base64 {
                match pixmap.encode_png() {
                    Ok(png_bytes) => {
                        let b64 = base64_encode(&png_bytes);
                        content.push(json!({
                            "type": "image",
                            "data": b64,
                            "mimeType": "image/png"
                        }));
                    }
                    Err(e) => {
                        return tool_text_err(format!("Failed to encode PNG: {e}"));
                    }
                }
            }
            json!({
                "content": content,
                "isError": false
            })
        }
        Err(err) => tool_text_err(format!("Render/audit failed: {err}")),
    }
}

fn call_knr_fmt(args: &serde_json::Map<String, Value>) -> Value {
    let file_str = match required_str(args, "file") {
        Ok(s) => s,
        Err(e) => return e,
    };
    let write = args.get("write").and_then(Value::as_bool).unwrap_or(false);
    let path = Path::new(file_str);
    if !path.is_file() {
        return tool_text_err(format!("File not found: '{file_str}'"));
    }

    let orig = match fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => return tool_text_err(format!("Failed to read '{file_str}': {e}")),
    };
    let formatted = match knr_core::format_tsk(&orig) {
        Ok(f) => f,
        Err(err) => return tool_text_err(format!("Format error in '{file_str}': {err}")),
    };

    let changed = formatted != orig;
    let written = if write && changed {
        if let Err(e) = fs::write(path, &formatted) {
            return tool_text_err(format!("Failed to write '{file_str}': {e}"));
        }
        true
    } else {
        false
    };

    let payload = json!({
        "ok": true,
        "file": file_str,
        "changed": changed,
        "written": written,
        "formatted_source": if changed && !written { Some(formatted) } else { None }
    });
    tool_text_ok(serde_json::to_string_pretty(&payload).unwrap_or_else(|_| payload.to_string()))
}

fn call_knr_fix(args: &serde_json::Map<String, Value>) -> Value {
    let file_str = match required_str(args, "file") {
        Ok(s) => s,
        Err(e) => return e,
    };
    let write = args.get("write").and_then(Value::as_bool).unwrap_or(false);
    let path = Path::new(file_str);
    if !path.is_file() {
        return tool_text_err(format!("File not found: '{file_str}'"));
    }

    let abs = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let uri = format!("file://{}", abs.display());
    let original = match fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => return tool_text_err(format!("Failed to read '{file_str}': {e}")),
    };

    let mut current = original.clone();
    let mut applied_fixes: Vec<Value> = Vec::new();

    for _pass in 0..8 {
        let actions = compute_all_quickfixes_for_source(&uri, &current);
        let Some(first) = actions.into_iter().next() else {
            break;
        };
        let title = first["title"].as_str().unwrap_or("QuickFix").to_string();
        let edits = first
            .pointer("/edit/changes")
            .and_then(|m| m.as_object())
            .and_then(|m| m.get(&uri).or_else(|| m.values().next()))
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        if edits.is_empty() {
            break;
        }
        let next = apply_lsp_text_edits(&current, &edits);
        if next == current {
            break;
        }
        applied_fixes.push(json!({
            "title": title,
            "edit_count": edits.len(),
        }));
        current = next;
    }

    let changed = current != original;
    let written = if write && changed {
        if let Err(e) = fs::write(path, &current) {
            return tool_text_err(format!("Failed to write '{file_str}': {e}"));
        }
        true
    } else {
        false
    };

    let remaining_fixable = compute_all_quickfixes_for_source(&uri, &current).len();
    let payload = json!({
        "ok": true,
        "file": file_str,
        "changed": changed,
        "written": written,
        "applied_count": applied_fixes.len(),
        "applied_fixes": applied_fixes,
        "remaining_fixable": remaining_fixable,
        "patched_source": if changed && !written { Some(current) } else { None }
    });
    tool_text_ok(serde_json::to_string_pretty(&payload).unwrap_or_else(|_| payload.to_string()))
}

fn call_knr_bench(args: &serde_json::Map<String, Value>) -> Value {
    let file_str = match required_str(args, "file") {
        Ok(s) => s,
        Err(e) => return e,
    };
    let fn_name = args.get("fn_name").and_then(Value::as_str);
    let iters = args
        .get("iters")
        .and_then(Value::as_u64)
        .unwrap_or(20) as usize;

    match run_benchmark(Path::new(file_str), fn_name, iters, 2_000_000) {
        Ok(report) => {
            let pretty = serde_json::to_string_pretty(&report)
                .unwrap_or_else(|_| "{}".to_string());
            tool_text_ok(pretty)
        }
        Err(err) => tool_text_err(format!("Benchmark failed: {err}")),
    }
}

fn base64_encode(input: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = if chunk.len() > 1 { chunk[1] as u32 } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] as u32 } else { 0 };
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[((triple >> 18) & 0x3f) as usize] as char);
        out.push(TABLE[((triple >> 12) & 0x3f) as usize] as char);
        if chunk.len() > 1 {
            out.push(TABLE[((triple >> 6) & 0x3f) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(TABLE[(triple & 0x3f) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}
