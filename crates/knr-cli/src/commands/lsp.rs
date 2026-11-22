use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Write as _;
use std::io::{self, BufRead, Write};
use std::path::{Component, Path, PathBuf};

use knr_core::lower::SourceMap;
use knr_core::syntax::{
    ArrowBody, BinaryOp, Block, ConstDef, Declaration, DestructPattern, EnumDef, Expr, ExprKind,
    FunctionDef, ImportKind, JsxChild, MatchPattern, Program, Span, Stmt, StmtKind, StructDef,
    TemplatePart, TypeExpr, TypeExprKind, UnaryOp,
};
use knr_core::ParseError;
use knr_verify::Diagnostic;
use serde_json::{json, Value};

use crate::error::CliError;

const SERVER_NAME: &str = "knr-lsp";
const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");
const MAX_CONTENT_LENGTH: usize = 32 * 1024 * 1024;

// LSP CompletionItemKind constants (LSP 3.17)
const COMPLETION_KIND_METHOD: u32 = 2;
const COMPLETION_KIND_FUNCTION: u32 = 3;
const COMPLETION_KIND_FIELD: u32 = 5;
const COMPLETION_KIND_CLASS: u32 = 7;
const COMPLETION_KIND_MODULE: u32 = 9;
const COMPLETION_KIND_PROPERTY: u32 = 10;
const COMPLETION_KIND_VALUE: u32 = 12;
const COMPLETION_KIND_ENUM: u32 = 13;
const COMPLETION_KIND_KEYWORD: u32 = 14;
const COMPLETION_KIND_SNIPPET: u32 = 15;
const COMPLETION_KIND_CONSTANT: u32 = 21;
const COMPLETION_KIND_STRUCT: u32 = 22;

// LSP InsertTextFormat constants (LSP 3.17)
const INSERT_FORMAT_PLAIN: u32 = 1;
const INSERT_FORMAT_SNIPPET: u32 = 2;

// LSP InlayHintKind constants (LSP 3.17)
const INLAY_HINT_KIND_TYPE: u32 = 1;
const INLAY_HINT_KIND_PARAMETER: u32 = 2;

// LSP SymbolKind constants (LSP 3.17)
const SYMBOL_KIND_FIELD: u32 = 8;
const SYMBOL_KIND_ENUM: u32 = 10;
const SYMBOL_KIND_FUNCTION: u32 = 12;
const SYMBOL_KIND_CONSTANT: u32 = 14;
const SYMBOL_KIND_ENUM_MEMBER: u32 = 22;
const SYMBOL_KIND_STRUCT: u32 = 23;

// LSP DiagnosticSeverity constants (LSP 3.17)
const SEVERITY_ERROR: u32 = 1;

/// Persistent state for a `.knr` Language Server session.
#[derive(Debug, Default)]
pub struct LspState {
    pub documents: HashMap<String, String>,
    programs: HashMap<String, Program>,
    workspace_roots: Vec<PathBuf>,
    pub shutdown_requested: bool,
    pub exit_requested: bool,
}

impl LspState {
    pub fn new() -> Self {
        Self::default()
    }
}

/// Runs the JSON-RPC 2.0 Language Server Protocol loop over stdio.
pub fn execute() -> Result<(), CliError> {
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut reader = stdin.lock();
    let mut writer = stdout.lock();
    let mut state = LspState::new();

    loop {
        let raw = match read_message(&mut reader) {
            Ok(Some(msg)) => msg,
            Ok(None) => break,
            Err(err) if err.kind() == io::ErrorKind::BrokenPipe => break,
            Err(err) => return Err(CliError::Io(err)),
        };

        let outgoing = match serde_json::from_str::<Value>(&raw) {
            Ok(msg) => handle_message(&mut state, &msg),
            Err(err) => vec![json_rpc_error(
                Value::Null,
                -32700,
                &format!("Parse error: {err}"),
            )],
        };

        for response in outgoing {
            if let Err(err) = write_message(&mut writer, &response) {
                if err.kind() == io::ErrorKind::BrokenPipe {
                    return Ok(());
                }
                return Err(CliError::Io(err));
            }
        }

        if state.exit_requested {
            break;
        }
    }

    Ok(())
}

/// Pure, deterministic LSP message dispatcher returning 0, 1, or more JSON-RPC responses or notifications.
pub fn handle_message(state: &mut LspState, msg: &Value) -> Vec<Value> {
    let Some(method) = msg.get("method").and_then(Value::as_str) else {
        return Vec::new();
    };
    let id = msg.get("id").cloned();
    let params = msg.get("params");

    match method {
        "initialize" => {
            let Some(req_id) = id else {
                return Vec::new();
            };
            if let Some(p) = params {
                state.workspace_roots.clear();
                if let Some(folders) = p.get("workspaceFolders").and_then(Value::as_array) {
                    for folder in folders {
                        if let Some(uri) = folder.get("uri").and_then(Value::as_str) {
                            if let Some(path) = uri_to_path(uri) {
                                let norm = normalize_path(&path);
                                if !state.workspace_roots.contains(&norm) {
                                    state.workspace_roots.push(norm);
                                }
                            }
                        }
                    }
                }
                if let Some(root_uri) = p.get("rootUri").and_then(Value::as_str) {
                    if let Some(path) = uri_to_path(root_uri) {
                        let norm = normalize_path(&path);
                        if !state.workspace_roots.contains(&norm) {
                            state.workspace_roots.push(norm);
                        }
                    }
                } else if let Some(root_path) = p.get("rootPath").and_then(Value::as_str) {
                    let norm = normalize_path(Path::new(root_path));
                    if !state.workspace_roots.contains(&norm) {
                        state.workspace_roots.push(norm);
                    }
                }
            }
            vec![json_rpc_result(
                req_id,
                json!({
                    "capabilities": {
                        "textDocumentSync": 1,
                        "hoverProvider": true,
                        "completionProvider": {
                            "triggerCharacters": ["<", ".", ":", "\"", "/", "{", "@"]
                        },
                        "signatureHelpProvider": {
                            "triggerCharacters": ["(", ","]
                        },
                        "inlayHintProvider": true,
                        "documentSymbolProvider": true,
                        "workspaceSymbolProvider": true,
                        "foldingRangeProvider": true,
                        "definitionProvider": true,
                        "referencesProvider": true,
                        "renameProvider": {
                            "prepareProvider": true
                        },
                        "codeActionProvider": {
                            "codeActionKinds": ["quickfix"]
                        },
                        "documentFormattingProvider": true
                    },
                    "serverInfo": {
                        "name": SERVER_NAME,
                        "version": SERVER_VERSION
                    }
                }),
            )]
        }
        "initialized" => Vec::new(),
        "shutdown" => {
            state.shutdown_requested = true;
            match id {
                Some(req_id) => vec![json_rpc_result(req_id, Value::Null)],
                None => Vec::new(),
            }
        }
        "exit" => {
            state.exit_requested = true;
            Vec::new()
        }
        "textDocument/didOpen" => handle_did_open(state, params),
        "textDocument/didChange" => handle_did_change(state, params),
        "textDocument/didSave" => handle_did_save(state, params),
        "textDocument/didClose" => handle_did_close(state, params),
        "textDocument/hover" => {
            let Some(req_id) = id else {
                return Vec::new();
            };
            vec![json_rpc_result(req_id, handle_hover(state, params))]
        }
        "textDocument/completion" => {
            let Some(req_id) = id else {
                return Vec::new();
            };
            vec![json_rpc_result(req_id, handle_completion(state, params))]
        }
        "textDocument/signatureHelp" => {
            let Some(req_id) = id else {
                return Vec::new();
            };
            vec![json_rpc_result(req_id, handle_signature_help(state, params))]
        }
        "textDocument/inlayHint" => {
            let Some(req_id) = id else {
                return Vec::new();
            };
            vec![json_rpc_result(req_id, handle_inlay_hint(state, params))]
        }
        "textDocument/documentSymbol" => {
            let Some(req_id) = id else {
                return Vec::new();
            };
            vec![json_rpc_result(
                req_id,
                handle_document_symbol(state, params),
            )]
        }
        "workspace/symbol" => {
            let Some(req_id) = id else {
                return Vec::new();
            };
            vec![json_rpc_result(
                req_id,
                handle_workspace_symbol(state, params),
            )]
        }
        "textDocument/foldingRange" => {
            let Some(req_id) = id else {
                return Vec::new();
            };
            vec![json_rpc_result(
                req_id,
                handle_folding_range(state, params),
            )]
        }
        "textDocument/definition" => {
            let Some(req_id) = id else {
                return Vec::new();
            };
            vec![json_rpc_result(req_id, handle_definition(state, params))]
        }
        "textDocument/references" => {
            let Some(req_id) = id else {
                return Vec::new();
            };
            vec![json_rpc_result(req_id, handle_references(state, params))]
        }
        "textDocument/prepareRename" => {
            let Some(req_id) = id else {
                return Vec::new();
            };
            vec![json_rpc_result(
                req_id,
                handle_prepare_rename(state, params),
            )]
        }
        "textDocument/rename" => {
            let Some(req_id) = id else {
                return Vec::new();
            };
            vec![json_rpc_result(req_id, handle_rename(state, params))]
        }
        "textDocument/codeAction" => {
            let Some(req_id) = id else {
                return Vec::new();
            };
            vec![json_rpc_result(req_id, handle_code_action(state, params))]
        }
        "textDocument/formatting" => {
            let Some(req_id) = id else {
                return Vec::new();
            };
            vec![json_rpc_result(req_id, handle_formatting(state, params))]
        }
        _ => match id {
            Some(req_id) => vec![json_rpc_error(
                req_id,
                -32601,
                &format!("Method not found: {method}"),
            )],
            None => Vec::new(),
        },
    }
}

// ============================================================================
// JSON-RPC 2.0 Framing Helpers
// ============================================================================

fn read_message<R: BufRead>(reader: &mut R) -> io::Result<Option<String>> {
    let mut content_length: Option<usize> = None;
    let mut header_line = String::new();
    let mut saw_any_header = false;

    loop {
        header_line.clear();
        let bytes_read = reader.read_line(&mut header_line)?;
        if bytes_read == 0 {
            return Ok(None);
        }

        let trimmed = header_line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            if saw_any_header {
                break;
            }
            continue;
        }

        saw_any_header = true;
        if let Some((name, value)) = trimmed.split_once(':') {
            if name.trim().eq_ignore_ascii_case("content-length") {
                let len = value.trim().parse::<usize>().map_err(|err| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("Invalid Content-Length header: {err}"),
                    )
                })?;
                if len > MAX_CONTENT_LENGTH {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("Content-Length {len} exceeds maximum allowed size"),
                    ));
                }
                content_length = Some(len);
            }
        }
    }

    let len = content_length.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "Missing Content-Length header in LSP frame",
        )
    })?;

    let mut body = vec![0u8; len];
    match reader.read_exact(&mut body) {
        Ok(()) => {}
        Err(err) if err.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(err) => return Err(err),
    }

    String::from_utf8(body)
        .map(Some)
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, format!("Invalid UTF-8: {err}")))
}

fn write_message<W: Write>(writer: &mut W, msg: &Value) -> io::Result<()> {
    let payload = serde_json::to_string(msg)
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err.to_string()))?;
    write!(
        writer,
        "Content-Length: {}\r\n\r\n{}",
        payload.len(),
        payload
    )?;
    writer.flush()
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

fn publish_diagnostics(uri: &str, diagnostics: Vec<Value>) -> Value {
    json!({
        "jsonrpc": "2.0",
        "method": "textDocument/publishDiagnostics",
        "params": {
            "uri": uri,
            "diagnostics": diagnostics
        }
    })
}

// ============================================================================
// Document Synchronization & Live Analysis
// ============================================================================

fn build_overlays(state: &LspState) -> HashMap<PathBuf, String> {
    let mut overlays = HashMap::with_capacity(state.documents.len() * 2);
    for (doc_uri, doc_text) in &state.documents {
        if let Some(path) = uri_to_path(doc_uri) {
            let norm = normalize_path(&path);
            overlays.insert(norm.clone(), doc_text.clone());
            if let Ok(canon) = path.canonicalize() {
                if canon != norm {
                    overlays.insert(canon, doc_text.clone());
                }
            }
        }
    }
    overlays
}

fn collect_dependent_diagnostics(
    state: &mut LspState,
    changed_uri: &str,
    overlays: &HashMap<PathBuf, String>,
) -> Vec<Value> {
    let mut other_uris: Vec<String> = state
        .documents
        .keys()
        .filter(|u| u.as_str() != changed_uri)
        .cloned()
        .collect();
    other_uris.sort();

    let mut out = Vec::new();
    for other_uri in other_uris {
        let Some(other_text) = state.documents.get(&other_uri).cloned() else {
            continue;
        };
        let imports_changed = program_for_uri(state, &other_uri).is_some_and(|p| {
            document_imports_uri_recursive(state, &other_uri, &p, changed_uri)
        });
        if imports_changed {
            let (prog, diags) =
                analyze_document_with_overlays(&other_uri, &other_text, overlays);
            if let Some(p) = prog {
                state.programs.insert(other_uri.clone(), p);
            }
            out.push(publish_diagnostics(&other_uri, diags));
        }
    }
    out
}

fn handle_did_open(state: &mut LspState, params: Option<&Value>) -> Vec<Value> {
    let Some(params) = params else {
        return Vec::new();
    };
    let Some(uri) = params.pointer("/textDocument/uri").and_then(Value::as_str) else {
        return Vec::new();
    };
    let Some(text) = params.pointer("/textDocument/text").and_then(Value::as_str) else {
        return Vec::new();
    };

    let owned = text.to_string();
    state.documents.insert(uri.to_string(), owned.clone());
    let overlays = build_overlays(state);
    let (prog, diagnostics) = analyze_document_with_overlays(uri, &owned, &overlays);
    if let Some(p) = prog {
        state.programs.insert(uri.to_string(), p);
    } else {
        state.programs.remove(uri);
    }
    vec![publish_diagnostics(uri, diagnostics)]
}

fn handle_did_change(state: &mut LspState, params: Option<&Value>) -> Vec<Value> {
    let Some(params) = params else {
        return Vec::new();
    };
    let Some(uri) = params.pointer("/textDocument/uri").and_then(Value::as_str) else {
        return Vec::new();
    };
    let Some(changes) = params.get("contentChanges").and_then(Value::as_array) else {
        return Vec::new();
    };
    let Some(new_text) = changes
        .iter()
        .rev()
        .find_map(|c| c.get("text").and_then(Value::as_str))
    else {
        return Vec::new();
    };

    let owned = new_text.to_string();
    state.documents.insert(uri.to_string(), owned.clone());
    let overlays = build_overlays(state);
    let (prog, diagnostics) = analyze_document_with_overlays(uri, &owned, &overlays);
    if let Some(p) = prog {
        state.programs.insert(uri.to_string(), p);
    }
    let mut out = vec![publish_diagnostics(uri, diagnostics)];
    out.extend(collect_dependent_diagnostics(state, uri, &overlays));
    out
}

fn handle_did_save(state: &mut LspState, params: Option<&Value>) -> Vec<Value> {
    let Some(params) = params else {
        return Vec::new();
    };
    let Some(uri) = params.pointer("/textDocument/uri").and_then(Value::as_str) else {
        return Vec::new();
    };

    let text = if let Some(saved_text) = params.get("text").and_then(Value::as_str) {
        let owned = saved_text.to_string();
        state.documents.insert(uri.to_string(), owned.clone());
        owned
    } else if let Some(cached) = state.documents.get(uri) {
        cached.clone()
    } else if let Some(path) = uri_to_path(uri) {
        match std::fs::read_to_string(path) {
            Ok(disk_text) => {
                state.documents.insert(uri.to_string(), disk_text.clone());
                disk_text
            }
            Err(_) => return Vec::new(),
        }
    } else {
        return Vec::new();
    };

    let overlays = build_overlays(state);
    let (prog, diagnostics) = analyze_document_with_overlays(uri, &text, &overlays);
    if let Some(p) = prog {
        state.programs.insert(uri.to_string(), p);
    }
    let mut out = vec![publish_diagnostics(uri, diagnostics)];
    out.extend(collect_dependent_diagnostics(state, uri, &overlays));
    out
}

fn handle_did_close(state: &mut LspState, params: Option<&Value>) -> Vec<Value> {
    let Some(uri) = params
        .and_then(|p| p.pointer("/textDocument/uri"))
        .and_then(Value::as_str)
    else {
        return Vec::new();
    };
    state.documents.remove(uri);
    state.programs.remove(uri);
    vec![publish_diagnostics(uri, Vec::new())]
}

fn is_ssa_document(uri: &str) -> bool {
    if let Some(path) = uri_to_path(uri) {
        if knr_core::is_source_file(&path) {
            return false;
        }
        let s = path.to_string_lossy();
        return s.ends_with(".ssa") || s.ends_with(".ssa.schema");
    }
    uri.ends_with(".ssa") || uri.ends_with(".ssa.schema")
}

#[derive(Debug, Clone)]
struct AnalyzedDiagnostic {
    lsp_diag: Value,
    code: String,
    message: String,
    function: Option<String>,
    source_variable: Option<String>,
    violated_register: Option<String>,
    line_1based: usize,
    range: Value,
}

impl AnalyzedDiagnostic {
    fn from_knr_diag(text: &str, diag: &Diagnostic) -> Self {
        let lsp_diag = knr_diagnostic_to_lsp(text, diag);
        let range = lsp_diag["range"].clone();
        let message = lsp_diag["message"].as_str().unwrap_or("").to_string();
        let function = if diag.function.is_empty() || diag.function == "<program>" {
            None
        } else {
            Some(diag.function.clone())
        };
        Self {
            lsp_diag,
            code: diag.error_kind.clone(),
            message,
            function,
            source_variable: diag.source_variable.clone(),
            violated_register: diag.violated_register.clone(),
            line_1based: diag.line,
            range,
        }
    }

    fn from_lsp_value(val: &Value) -> Option<Self> {
        let range = val.get("range")?.clone();
        let code = val
            .get("code")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let message = val
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let line_1based = range
            .pointer("/start/line")
            .and_then(Value::as_u64)
            .unwrap_or(0) as usize
            + 1;
        Some(Self {
            lsp_diag: val.clone(),
            code,
            message,
            function: None,
            source_variable: None,
            violated_register: None,
            line_1based,
            range,
        })
    }
}

fn analyze_document(uri: &str, text: &str) -> (Option<Program>, Vec<Value>) {
    let empty = HashMap::new();
    analyze_document_with_overlays(uri, text, &empty)
}

fn analyze_document_with_overlays(
    uri: &str,
    text: &str,
    overlays: &HashMap<PathBuf, String>,
) -> (Option<Program>, Vec<Value>) {
    let (prog, analyzed) = analyze_document_detailed_with_overlays(uri, text, overlays);
    let lsp_diags = analyzed.into_iter().map(|d| d.lsp_diag).collect();
    (prog, lsp_diags)
}

fn analyze_document_detailed_with_overlays(
    uri: &str,
    text: &str,
    overlays: &HashMap<PathBuf, String>,
) -> (Option<Program>, Vec<AnalyzedDiagnostic>) {
    // Handle raw SSA IR files (`.ssa` / `.ssa.schema`) gracefully alongside `.tsk` / `.knr`
    if is_ssa_document(uri) {
        let module = match knr_core::parse_ssa(text) {
            Ok(m) => m,
            Err(err) => {
                let lsp = parse_error_to_lsp(text, &err);
                return (
                    None,
                    AnalyzedDiagnostic::from_lsp_value(&lsp)
                        .into_iter()
                        .collect(),
                );
            }
        };
        if let Err(verify_diags) = knr_verify::verify_module(&module) {
            let analyzed = verify_diags
                .iter()
                .map(|d| AnalyzedDiagnostic::from_knr_diag(text, d))
                .collect();
            return (None, analyzed);
        }
        return (None, Vec::new());
    }

    // 1. Parse Strict TypeScript / TSX (`.tsk`, `.knr`) source into AST
    let prog = match knr_core::syntax::parse_tsx(text) {
        Ok(p) => p,
        Err(err) => {
            let lsp = parse_error_to_lsp(text, &err);
            return (
                None,
                AnalyzedDiagnostic::from_lsp_value(&lsp)
                    .into_iter()
                    .collect(),
            );
        }
    };

    // 2. Lower AST to canonical SSA IR (resolving relative & stdlib imports with live unsaved overlays)
    let parent_dir = uri_to_path(uri).and_then(|p| p.parent().map(PathBuf::from));
    let (module, source_map) = match knr_core::lower::lower_program_with_overlays_and_source_map(
        &prog,
        &[],
        parent_dir,
        overlays,
    ) {
        Ok(res) => res,
        Err(lower_err) => {
            let diag = Diagnostic::from_lower_error(lower_err);
            return (
                Some(prog),
                vec![AnalyzedDiagnostic::from_knr_diag(text, &diag)],
            );
        }
    };

    // 3. Static verification (schemas, effects, linear ownership, SSA dominance, types, shader purity)
    if let Err(mut verify_diags) = knr_verify::verify_module(&module) {
        let mut analyzed = Vec::with_capacity(verify_diags.len());
        for diag in &mut verify_diags {
            enrich_diagnostic_with_source_map(diag, &source_map);
            // Refine `ShaderEffectForbidden` from default line 1 to the actual `@effects(...)` decorator line
            if diag.error_kind == "ShaderEffectForbidden" && diag.line <= 1 {
                if let Some(f) = find_function_by_name(&prog, &diag.function) {
                    if let Some((eff_start, eff_end)) = find_effects_decorator_bytes(text, f) {
                        let (s_line, _) = byte_offset_to_lsp_pos(text, eff_start);
                        diag.line = (s_line as usize) + 1;
                        diag.span = Some((eff_start, eff_end));
                    } else {
                        diag.line = f.name.span.line;
                        diag.column = Some(f.name.span.col);
                        diag.span = Some((f.name.span.start, f.name.span.end));
                    }
                }
            }
            analyzed.push(AnalyzedDiagnostic::from_knr_diag(text, diag));
        }
        return (Some(prog), analyzed);
    }

    // 4. Validate every `@shader` function compiles cleanly to naga-verified WGSL + WebGL2 GLSL ES 3.00
    if let Err(shader_err) = knr_codegen::WgslBackend::new().compile_all_shaders(&module) {
        let err_msg = shader_err.to_string();
        let target_shader = prog
            .declarations
            .iter()
            .filter_map(|decl| match decl {
                Declaration::Function(f) if f.is_shader => Some(f),
                _ => None,
            })
            .find(|f| err_msg.contains(&f.name.name))
            .or_else(|| {
                prog.declarations.iter().find_map(|decl| match decl {
                    Declaration::Function(f) if f.is_shader => Some(f),
                    _ => None,
                })
            });

        let (fn_name, line, span) = match target_shader {
            Some(f) => (f.name.name.as_str(), f.name.span.line, Some(f.name.span)),
            None => ("<shader>", 1, None),
        };

        let mut diag = Diagnostic::new("ShaderCompileError", fn_name, line, err_msg).with_fix_hint(
            "Ensure @shader function uses pure GPU arithmetic/structs and returns Color/Vec4/Vec3 or f64",
        );
        if let Some(s) = span {
            diag = diag.with_column(s.col).with_span(s.start, s.end);
        }
        return (
            Some(prog),
            vec![AnalyzedDiagnostic::from_knr_diag(text, &diag)],
        );
    }

    (Some(prog), Vec::new())
}

fn enrich_diagnostic_with_source_map(diag: &mut Diagnostic, source_map: &SourceMap) {
    if let Some(ref reg_str) = diag.violated_register {
        if let Some(stripped) = reg_str.strip_prefix('%') {
            if let Ok(reg_id) = stripped.parse::<u32>() {
                if let Some(var_name) = source_map.get_var(&diag.function, reg_id) {
                    diag.source_variable = Some(var_name.to_string());
                }
                if let Some(span) = source_map.get_span(&diag.function, reg_id) {
                    diag.column = Some(span.col);
                    diag.span = Some((span.start, span.end));
                }
            }
        }
    }
    if diag.column.is_none() {
        if let Some(span) = source_map.get_line_span(diag.line) {
            diag.column = Some(span.col);
            diag.span = Some((span.start, span.end));
        }
    }
}

fn parse_error_to_lsp(text: &str, err: &ParseError) -> Value {
    let (error_kind, line, fix_hint) = parse_error_details(err);
    let col = extract_column_from_parse_error(err);
    let range = line_col_to_lsp_range(text, line, col, None);
    let message = format!("[{error_kind}] {err}\nHint: {fix_hint}");
    json!({
        "range": range,
        "severity": SEVERITY_ERROR,
        "code": error_kind,
        "source": "knr",
        "message": message
    })
}

fn knr_diagnostic_to_lsp(text: &str, diag: &Diagnostic) -> Value {
    let range = line_col_to_lsp_range(text, diag.line, diag.column, diag.span);

    let mut message = if diag.instruction.is_empty() {
        match &diag.fix_hint {
            Some(hint) => format!("[{}] {hint}", diag.error_kind),
            None => diag.error_kind.clone(),
        }
    } else {
        let mut msg = format!("[{}] {}", diag.error_kind, diag.instruction);
        if let Some(ref var) = diag.source_variable {
            let _ = write!(msg, " (variable `{var}`)");
        } else if let Some(ref reg) = diag.violated_register {
            let _ = write!(msg, " (register {reg})");
        }
        if let Some(consumed) = diag.consumed_at_line {
            let _ = write!(msg, " (consumed at line {consumed})");
        }
        if let Some(ref hint) = diag.fix_hint {
            let _ = write!(msg, "\nHint: {hint}");
        }
        msg
    };

    if message.is_empty() {
        message = diag.to_string();
    }

    json!({
        "range": range,
        "severity": SEVERITY_ERROR,
        "code": diag.error_kind,
        "source": "knr",
        "message": message
    })
}

fn parse_error_details(err: &ParseError) -> (&'static str, usize, String) {
    match err {
        ParseError::UnexpectedEof { line } => (
            "UnexpectedEof",
            *line,
            "Ensure all blocks, braces, and statements are closed".to_string(),
        ),
        ParseError::InvalidSyntax { line, msg } => {
            ("InvalidSyntax", *line, format!("Fix syntax error: {msg}"))
        }
        ParseError::UnknownOpcode { line, opcode } => (
            "UnknownOpcode",
            *line,
            format!("Check spelling of opcode '{opcode}'"),
        ),
        ParseError::InvalidRegister { line, token } => (
            "InvalidRegister",
            *line,
            format!("Register token '{token}' must use %<digits> format"),
        ),
        ParseError::InvalidBlockLabel { line, token } => (
            "InvalidBlockLabel",
            *line,
            format!("Block label '{token}' must use bb<digits>: format"),
        ),
        ParseError::InvalidType { line, token } => (
            "InvalidType",
            *line,
            format!(
                "Type '{token}' is invalid; use i64, f64, bool, str, buf, vec[T], map[K, V], chan[T], or a schema name"
            ),
        ),
        ParseError::InvalidEffect { line, token } => (
            "InvalidEffect",
            *line,
            format!("Effect '{token}' is invalid; declare Net, Ui, Hw, or Async"),
        ),
        ParseError::UnterminatedString { line } => (
            "UnterminatedString",
            *line,
            "Add closing quotation mark '\"'".to_string(),
        ),
        ParseError::InvalidLiteral {
            line,
            token: _,
            expected,
        } => (
            "InvalidLiteral",
            *line,
            format!("Provide a valid literal: {expected}"),
        ),
        ParseError::MissingTerminator { line, block_id } => (
            "MissingTerminator",
            *line,
            format!("Add a terminator at the end of block bb{block_id}"),
        ),
        ParseError::MissingClosingBrace { line } => (
            "MissingClosingBrace",
            *line,
            "Add closing brace '}'".to_string(),
        ),
        ParseError::InlineSchemaInCodeFile {
            line,
            name: _,
            fix_hint,
        } => ("InlineSchemaInCodeFile", *line, fix_hint.clone()),
        ParseError::InvalidConstructInSchemaFile { line, msg } => (
            "InvalidConstructInSchemaFile",
            *line,
            format!("Schema files may only contain 'import' and 'schema' declarations: {msg}"),
        ),
        ParseError::ImportAfterDeclaration { line } => (
            "ImportAfterDeclaration",
            *line,
            "Move all 'import' directives to the top of the file".to_string(),
        ),
        ParseError::CircularImport { cycle } => (
            "CircularImport",
            1,
            format!("Break circular dependency cycle: {}", cycle.join(" -> ")),
        ),
        ParseError::DuplicateSymbol {
            symbol,
            first_file,
            second_file,
        } => (
            "DuplicateSymbol",
            1,
            format!(
                "Rename or remove duplicate symbol '{symbol}' defined in '{first_file}' and '{second_file}'"
            ),
        ),
        ParseError::FileNotFound { line, path } => (
            "FileNotFound",
            *line,
            format!("Check that imported file '{path}' exists"),
        ),
        ParseError::IoError { path, msg } => (
            "IoError",
            1,
            format!("Unable to read '{path}': {msg}"),
        ),
    }
}

fn extract_column_from_parse_error(err: &ParseError) -> Option<usize> {
    if let ParseError::InvalidSyntax { msg, .. } = err {
        if let Some(rest) = msg.strip_prefix("col ") {
            if let Some(colon_pos) = rest.find(':') {
                return rest[..colon_pos].trim().parse::<usize>().ok();
            }
        }
    }
    None
}

// ============================================================================
// Hover Provider (`textDocument/hover`)
// ============================================================================

fn handle_hover(state: &LspState, params: Option<&Value>) -> Value {
    let Some(params) = params else {
        return Value::Null;
    };
    let Some(uri) = params.pointer("/textDocument/uri").and_then(Value::as_str) else {
        return Value::Null;
    };
    let Some(line) = params
        .pointer("/position/line")
        .and_then(Value::as_u64)
        .map(|v| v as u32)
    else {
        return Value::Null;
    };
    let Some(character) = params
        .pointer("/position/character")
        .and_then(Value::as_u64)
        .map(|v| v as u32)
    else {
        return Value::Null;
    };

    let Some(text) = state.documents.get(uri) else {
        return Value::Null;
    };

    // 0. Hovering over a module path string in `import ... from "./state.tsk"` / `export ... from ...`
    if let Some((imp_path, str_range)) = import_string_at_position(text, line, character) {
        if let Some(markdown) = hover_for_import_module(state, uri, &imp_path) {
            return make_hover_response(&markdown, str_range);
        }
    }

    let Some((ident, ident_start_byte, _ident_end_byte, range)) =
        symbol_at_position(text, line, character)
    else {
        return Value::Null;
    };

    let prog = program_for_uri(state, uri);

    // 0b. Namespace member hover: `ns.member` where `import * as ns from "./mod.tsk"`
    if let (Some(ref program), Some(qualifier)) = (&prog, dot_qualifier_before(text, ident_start_byte))
    {
        if let Some(ns_mod) = resolve_namespace_target_in_program(state, uri, program, &qualifier) {
            if let Some(markdown) = lookup_program_declaration_hover(&ns_mod.prog, &ident) {
                let with_origin = format!("{markdown}\n\n*Imported from `{}`*", ns_mod.import_spec);
                return make_hover_response(&with_origin, range);
            }
        }
    }

    // Resolve `import { orig as alias } from "..."`
    let resolved_ident = prog
        .as_ref()
        .map(|p| resolve_alias_in_program(p, &ident))
        .unwrap_or_else(|| ident.clone());

    // 1. Check top-level declarations in the current document
    if let Some(ref program) = prog {
        if let Some(markdown) = lookup_program_declaration_hover(program, &resolved_ident) {
            return make_hover_response(&markdown, range);
        }
    }

    // 1b. Check top-level declarations in imported `.tsk` / `.knr`, package, and `std/*` modules
    let mut imported = prog
        .as_ref()
        .map(|p| load_imported_programs_recursive(state, uri, p))
        .unwrap_or_default();
    if let Some(ref program) = prog {
        imported.extend(load_imported_stdlib_programs(program));
    }
    for imp in &imported {
        if let Some(markdown) = lookup_program_declaration_hover(&imp.prog, &resolved_ident) {
            let alias_note = if resolved_ident != ident {
                format!("\n\n*Aliased as `{ident}` — imported from `{}`*", imp.import_spec)
            } else {
                format!("\n\n*Imported from `{}`*", imp.import_spec)
            };
            return make_hover_response(&format!("{markdown}{alias_note}"), range);
        }
    }

    // 2. Check built-in math/UI/linear intrinsics, keywords, and standard UI schemas
    if let Some(markdown) = builtin_hover_doc(&resolved_ident) {
        return make_hover_response(&markdown, range);
    }

    // 2b. Check v0.4.0 functional array methods (`.map`, `.filter`, `.reduce`, ..., `.length`)
    if dot_qualifier_before(text, ident_start_byte).is_some() {
        if let Some(markdown) = array_method_hover_doc(&resolved_ident) {
            return make_hover_response(&markdown, range);
        }
    }

    // 3. Check Style and Widget schema fields
    if let Some(markdown) = ui_field_hover_doc(&resolved_ident) {
        return make_hover_response(&markdown, range);
    }

    // 4. Check struct fields and enum variants declared in the current document
    if let Some(ref program) = prog {
        if let Some(markdown) = lookup_program_member_hover(program, &resolved_ident) {
            return make_hover_response(&markdown, range);
        }
    }

    // 4b. Check struct fields and enum variants declared in imported modules (local, package, std/*)
    for imp in &imported {
        if let Some(markdown) = lookup_program_member_hover(&imp.prog, &resolved_ident) {
            let with_origin = format!("{markdown}\n\n*Imported from `{}`*", imp.import_spec);
            return make_hover_response(&with_origin, range);
        }
    }

    // 4c. Fallback to functional array method docs even without leading dot (e.g. hovering standalone)
    if let Some(markdown) = array_method_hover_doc(&resolved_ident) {
        return make_hover_response(&markdown, range);
    }

    Value::Null
}

fn stdlib_module_description(imp_path: &str) -> &'static str {
    let normalized = imp_path
        .strip_suffix(".tsk")
        .or_else(|| imp_path.strip_suffix(".knr"))
        .unwrap_or(imp_path);
    match normalized {
        "std/ui" => "Built-in `knr` declarative UI primitives, 45-field `Style`, `Widget`, `Vec2`, and `Color`.",
        "std/math" => "Built-in `knr` GPU-pure 2D/3D vector and SDF math library.",
        "std/anim" => "Built-in `knr` pure easing curves and `smooth_damp` animation helpers.",
        "std/core" => "Built-in `knr` generic algebraic core types (`Option<T>`, `Result<T, E>`).",
        "std/cmd" => "Built-in `knr` declarative async MVU `Cmd` effect constructors.",
        "std/sys" => "Built-in `knr` capability-isolated filesystem (`@effects(\"hw\")`), JSON, and HTTP/1.1 (`@effects(\"net\")`) library.",
        "std/theme" => "Built-in `knr` design tokens & curated themes (`Theme`, `theme_obsidian`, `theme_light`, `theme_nord`, `theme_solar`).",
        "std/router" => "Built-in `knr` zero-GC client-side router (`Route`, `router_init`, `router_push`, `router_back`, `router_replace`, `router_is`, `router_can_back`).",
        "std/layout" => "Built-in `knr` responsive breakpoint & layout helpers (`is_compact`, `is_medium`, `is_wide`, `responsive_cols`, `responsive_pad`).",
        "std/testing" => "Built-in `knr` deterministic unit testing assertions (`assert_eq_i64`, `assert_near`, `assert_str_eq`, `assert_true`, `assert_false`, `assert_gt`, `assert_lt`).",
        _ => "Built-in `knr` standard library module.",
    }
}

fn hover_for_import_module(state: &LspState, current_uri: &str, imp_path: &str) -> Option<String> {
    if let Some(std_src) = knr_core::syntax::stdlib::resolve_builtin_std(imp_path) {
        let mut names = Vec::new();
        if let Ok(prog) = knr_core::syntax::parse_tsx(std_src) {
            for decl in &prog.declarations {
                names.push(declaration_summary_bullet(decl));
            }
        }
        let summary = if names.is_empty() {
            String::new()
        } else {
            let preview: Vec<String> = names.into_iter().take(16).collect();
            format!("\n\nExports:\n{}", preview.join("\n"))
        };
        let desc = stdlib_module_description(imp_path);
        return Some(format!(
            "```tsx\nmodule \"{imp_path}\"\n```\n\n{desc}{summary}"
        ));
    }

    let (imp_uri, imp_text) = resolve_local_import_targets(state, current_uri, imp_path)?;
    let display_file = uri_to_path(&imp_uri)
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
        .unwrap_or_else(|| imp_path.to_string());
    let is_package_dep =
        !imp_path.starts_with("./") && !imp_path.starts_with("../") && !imp_path.starts_with('/');
    let kind_label = if is_package_dep {
        format!("Package module `{imp_path}` (`{display_file}`)")
    } else {
        format!("Local module `{display_file}`")
    };
    let mut bullets = Vec::new();
    if let Ok(prog) = knr_core::syntax::parse_tsx(&imp_text) {
        for decl in &prog.declarations {
            bullets.push(declaration_summary_bullet(decl));
        }
    }
    let body = if bullets.is_empty() {
        format!("{kind_label}.")
    } else {
        let total = bullets.len();
        let shown: Vec<String> = bullets.into_iter().take(14).collect();
        let more = if total > 14 {
            format!("\n- *…and {} more*", total - 14)
        } else {
            String::new()
        };
        format!(
            "{kind_label} ({total} declaration{}):\n{}{more}",
            if total == 1 { "" } else { "s" },
            shown.join("\n")
        )
    };
    Some(format!("```tsx\nimport \"{imp_path}\"\n```\n\n{body}"))
}

fn declaration_summary_bullet(decl: &Declaration) -> String {
    match decl {
        Declaration::Function(f) if f.is_shader => format!("- `@shader` `function {}`", f.name.name),
        Declaration::Function(f) => format!("- `function {}`", f.name.name),
        Declaration::Struct(s) => format!("- `type {}` ({} fields)", s.name.name, s.fields.len()),
        Declaration::Enum(e) => format!("- `enum {}` ({} variants)", e.name.name, e.variants.len()),
        Declaration::Const(c) => format!("- `const {}`", c.name.name),
    }
}

fn make_hover_response(markdown: &str, range: Value) -> Value {
    json!({
        "contents": {
            "kind": "markdown",
            "value": markdown
        },
        "range": range
    })
}

fn lookup_program_declaration_hover(prog: &Program, ident: &str) -> Option<String> {
    for decl in &prog.declarations {
        match decl {
            Declaration::Function(f) if f.name.name == ident => {
                let sig = format_function_signature(f);
                let note = if f.is_shader {
                    "\n\nGPU-pure `@shader` kernel compiled to WGSL (Desktop `wgpu` / WebGPU) and GLSL ES 3.00 (WebGL2)."
                } else if !f.effects.is_empty() {
                    "\n\nEffectful function with statically verified `@effects(...)` capability boundaries."
                } else {
                    "\n\nPure statically verified function."
                };
                return Some(format!("```tsx\n{sig}\n```{note}"));
            }
            Declaration::Struct(s) if s.name.name == ident => {
                let formatted = format_struct_def(s);
                return Some(format!(
                    "```tsx\n{formatted}\n```\n\nNominal type with {} field(s).",
                    s.fields.len()
                ));
            }
            Declaration::Enum(e) if e.name.name == ident => {
                let formatted = format_enum_def(e);
                return Some(format!(
                    "```tsx\n{formatted}\n```\n\nTagged union with {} variant(s).",
                    e.variants.len()
                ));
            }
            Declaration::Const(c) if c.name.name == ident => {
                let sig = format_const_def(c);
                return Some(format!("```tsx\n{sig}\n```\n\nCompile-time module constant."));
            }
            _ => {}
        }
    }
    None
}

fn lookup_program_member_hover(prog: &Program, ident: &str) -> Option<String> {
    for decl in &prog.declarations {
        match decl {
            Declaration::Struct(s) => {
                for (i, field) in s.fields.iter().enumerate() {
                    if field.name.name == ident {
                        let idx = field.index.unwrap_or(i as u32);
                        let ty = format_type_expr(&field.ty);
                        return Some(format!(
                            "```tsx\n{}.{}: {ty}\n```\n\nField slot `{idx}` of type `{}`.",
                            s.name.name, field.name.name, s.name.name
                        ));
                    }
                }
            }
            Declaration::Enum(e) => {
                for variant in &e.variants {
                    if variant.name.name == ident {
                        let payload = if variant.fields.is_empty() {
                            String::new()
                        } else {
                            let tys: Vec<String> =
                                variant.fields.iter().map(format_type_expr).collect();
                            format!("({})", tys.join(", "))
                        };
                        return Some(format!(
                            "```tsx\n{}.{}{payload}\n```\n\nVariant of enum `{}`.",
                            e.name.name, variant.name.name, e.name.name
                        ));
                    }
                }
            }
            Declaration::Function(_) | Declaration::Const(_) => {}
        }
    }
    None
}

fn builtin_hover_doc(ident: &str) -> Option<String> {
    let doc = match ident {
        // Math intrinsics (CPU + GPU shaders)
        "sin" => "```knr\nfn sin(x: f64) -> f64\n```\n\nSine of `x` in radians. GPU-pure (valid inside `shader fn`).",
        "cos" => "```knr\nfn cos(x: f64) -> f64\n```\n\nCosine of `x` in radians. GPU-pure (valid inside `shader fn`).",
        "tan" => "```knr\nfn tan(x: f64) -> f64\n```\n\nTangent of `x` in radians. GPU-pure (valid inside `shader fn`).",
        "sqrt" => "```knr\nfn sqrt(x: f64) -> f64\n```\n\nSquare root of `x`. GPU-pure (valid inside `shader fn`).",
        "abs" => "```knr\nfn abs(x: f64) -> f64\n```\n\nAbsolute value of `x`. GPU-pure (valid inside `shader fn`).",
        "floor" => "```knr\nfn floor(x: f64) -> f64\n```\n\nLargest integer value less than or equal to `x`. GPU-pure.",
        "ceil" => "```knr\nfn ceil(x: f64) -> f64\n```\n\nSmallest integer value greater than or equal to `x`. GPU-pure.",
        "fract" => "```knr\nfn fract(x: f64) -> f64\n```\n\nFractional part `x - floor(x)`. GPU-pure.",
        "pow" => "```knr\nfn pow(base: f64, exp: f64) -> f64\n```\n\nRaises `base` to the power `exp`. GPU-pure.",
        "atan2" => "```knr\nfn atan2(y: f64, x: f64) -> f64\n```\n\nFour-quadrant arctangent of `y / x` in radians (`[-pi, pi]`). GPU-pure.",
        "min" => "```knr\nfn min(a: f64, b: f64) -> f64\n```\n\nMinimum of `a` and `b`. GPU-pure.",
        "max" => "```knr\nfn max(a: f64, b: f64) -> f64\n```\n\nMaximum of `a` and `b`. GPU-pure.",
        "clamp" => "```knr\nfn clamp(x: f64, lo: f64, hi: f64) -> f64\n```\n\nConstrains `x` to the closed interval `[lo, hi]`. GPU-pure.",
        "lerp" | "mix" => "```knr\nfn lerp(a: f64, b: f64, t: f64) -> f64\n```\n\nLinear interpolation `a + (b - a) * t` (`mix` is an alias). GPU-pure.",
        "smoothstep" => "```knr\nfn smoothstep(edge0: f64, edge1: f64, x: f64) -> f64\n```\n\nSmooth Hermite interpolation from `0.0` to `1.0` across `[edge0, edge1]`. GPU-pure.",

        // Conversions & Strings
        "to_f64" => "```knr\nfn to_f64(x: i64) -> f64\n```\n\nConverts an `i64` integer to `f64`.",
        "to_i64" => "```knr\nfn to_i64(x: f64) -> i64\n```\n\nTruncates an `f64` floating-point number to `i64`.",
        "to_str" => "```knr\nfn to_str(x: i64 | f64 | bool | str) -> str\n```\n\nFormats a primitive value as a UTF-8 string.",
        "str_len" => "```knr\nfn str_len(s: str) -> i64\n```\n\nReturns the byte length of string `s`.",

        // Linear Collections, Buffers & Channels
        "vec_new" => "```knr\nfn vec_new() -> vec[T]\n```\n\nAllocates an empty linear vector `vec[T]`. Element type `T` is inferred from surrounding annotation or return type.",
        "vec_push" => "```knr\nfn vec_push(v: vec[T], item: T) -> vec[T]\n```\n\nConsumes linear vector `v` (and `item` if linear), appends `item`, and returns the updated vector.",
        "vec_get" => "```knr\nfn vec_get(v: &vec[T], index: i64) -> T\n```\n\nReads the element at `index` from vector `v` without consuming `v`.",
        "vec_len" => "```knr\nfn vec_len(v: &vec[T]) -> i64\n```\n\nReturns the number of elements in vector `v`.",
        "map_new" => "```knr\nfn map_new() -> map[K, V]\n```\n\nAllocates an empty linear hash map `map[K, V]`.",
        "map_set" => "```knr\nfn map_set(m: map[K, V], key: K, value: V) -> map[K, V]\n```\n\nConsumes `m`, inserts `(key, value)`, and returns the updated map.",
        "map_get" => "```knr\nfn map_get(m: &map[K, V], key: K) -> V\n```\n\nLooks up `key` in map `m`.",
        "buf_alloc" => "```knr\nfn buf_alloc(size: i64) -> buf\n```\n\nAllocates a linear heap byte buffer of `size` bytes. Must be consumed or passed to `free`.",
        "buf_read" => "```knr\nfn buf_read(b: &buf, offset: i64) -> i64\n```\n\nReads an unsigned byte (`0..=255`) from buffer `b` at `offset`.",
        "buf_write" => "```knr\nfn buf_write(b: buf, offset: i64, value: i64) -> buf\n```\n\nConsumes `b`, writes byte `value` at `offset`, and returns the updated buffer.",
        "chan_new" => "```knr\nfn chan_new(capacity: i64) -> chan[T] effects(Async)\n```\n\nAllocates a bounded linear channel with `capacity` slots. Requires `effects(Async)`.",
        "chan_send" => "```knr\nfn chan_send(ch: &chan[T], value: T) -> bool\n```\n\nSends `value` on channel `ch` (consumes `value` if linear).",
        "chan_recv" => "```knr\nfn chan_recv(ch: &chan[T]) -> T\n```\n\nReceives the next value from channel `ch`.",
        "free" => "```knr\nfn free(resource: T)\n```\n\nExplicitly consumes and releases a linear resource (`buf`, `vec[T]`, `map[K, V]`, `chan[T]`).",

        // Language Keywords & Special Constructs
        "syscall" => "```tsx\nsyscall(\"net\" | \"ui\" | \"hw\" | \"async\", \"op\", payload)\n```\n\nInvokes a host runtime system call under capability effect (`net`, `ui`, `hw`, or `async`). The enclosing function must declare `@effects(...)`.",
        "shader" => "```tsx\n@shader\nexport function name(uv: Vec2, time: f64, ...): Color\n```\n\nDeclares a GPU-pure fragment shader kernel. Verified by `knr-verify` and compiled via `naga` to WGSL (Desktop `wgpu` + WebGPU) and GLSL ES 3.00 (WebGL2).",
        "effects" => "```tsx\n@effects(\"net\", \"ui\", \"hw\", \"async\")\n```\n\nCapability decorator declaring which side-effect domains a function (or its transitive callees) may invoke.",

        // Standard UI & Shader Schemas
        "Style" => "```tsx\nexport type Style = {\n    w: f64; h: f64; min_w: f64; max_w: f64;\n    bg: string; color: string; border: string; border_w: f64;\n    r_tl: f64; r_tr: f64; r_br: f64; r_bl: f64;\n    pad_t: f64; pad_r: f64; pad_b: f64; pad_l: f64;\n    gap_x: f64; gap_y: f64; font_size: f64; font_weight: i64;\n    align: string; justify: string; grid_cols: i64;\n    shadow_x: f64; shadow_y: f64; shadow_blur: f64; shadow_color: string;\n    backdrop_blur: f64; opacity: f64;\n    translate_x: f64; translate_y: f64; scale: f64; rotate_deg: f64;\n    hover_bg: string; hover_border: string; hover_scale: f64; transition_ms: f64;\n    overflow: string; z_index: i64; grow: f64;\n    font_family: string; text_align: string; line_height: f64; col_span: i64; letter_spacing: f64;\n};\n```\n\n45-field statically typed UI style schema rendered pixel-identically on Native GPU Desktop (`wgpu`), Headless PNG (`knr render`), and Unified Canvas+WebGPU Web (`--target web`).",
        "Widget" => "```tsx\nexport type Widget = {\n    key: string; kind: string; text: string; action: string;\n    value: f64; uniforms: f64[]; style: Style; children: Widget[];\n};\n```\n\nDeclarative MVU UI tree node produced by JSX (`<Column>`, `<Row>`, `<Grid>`, `<Stack>`, `<Card>`, `<Text>`, `<Code>`, `<Badge>`, `<Button>`, `<Input>`, `<Slider>`, `<Toggle>`, `<Shader>`, `<Icon>`, `<Image>`, `<Divider>`, `<Progress>`, `<Spacer>`).",
        "Vec2" => "```tsx\nexport type Vec2 = { x: f64; y: f64; };\n```\n\n2D floating-point coordinate vector (`uv.x`, `uv.y` in `[0.0, 1.0]`).",
        "Color" => "```tsx\nexport type Color = { r: f64; g: f64; b: f64; a: f64; };\n```\n\nNormalized RGBA color (`0.0..=1.0`) returned by `@shader` kernels.",
        "Theme" => "```tsx\nexport type Theme = { bg: string; surface: string; surface_alt: string; border: string; text: string; text_muted: string; accent: string; accent_alt: string; success: string; warning: string; danger: string; radius_sm: f64; radius_md: f64; radius_lg: f64; };\n```\n\nDesign token theme schema from `std/theme` (`theme_obsidian`, `theme_light`, `theme_nord`, `theme_solar`).",
        "Route" => "```tsx\nexport type Route = { current: string; previous: string; depth: i64; };\n```\n\nZero-GC client-side router state from `std/router`.",
        "Option" => "```tsx\nexport enum Option<T> { Some(T), None }\n```\n\nGeneric optional value from `std/core` (supports `??` nullish coalescing).",
        "Result" => "```tsx\nexport enum Result<T, E> { Ok(T), Err(E) }\n```\n\nGeneric fallible result type from `std/core` (supports `??` nullish coalescing).",
        "Cmd" => "```tsx\nexport type Cmd = { kind: string; target: string; payload: string; on_result: string; delay_ms: f64; };\n```\n\nDeclarative MVU async side-effect command from `std/cmd`.",
        _ => return None,
    };
    Some(doc.to_string())
}

struct ArrayMethodSpec {
    name: &'static str,
    is_property: bool,
    signature: &'static str,
    params: &'static [&'static str],
    insert_snippet: &'static str,
    doc: &'static str,
}

const ARRAY_METHODS: &[ArrayMethodSpec] = &[
    ArrayMethodSpec {
        name: "map",
        is_property: false,
        signature: "map<U>(fn: (item: T, index?: i64) => U): U[]",
        params: &["fn: (item: T, index?: i64) => U"],
        insert_snippet: "map((${1:item}) => ${2:item})",
        doc: "Transforms each element of the array using `fn` and returns a new array.",
    },
    ArrayMethodSpec {
        name: "filter",
        is_property: false,
        signature: "filter(predicate: (item: T, index?: i64) => bool): T[]",
        params: &["predicate: (item: T, index?: i64) => bool"],
        insert_snippet: "filter((${1:item}) => ${2:true})",
        doc: "Returns a new array containing only elements for which `predicate` returns `true`.",
    },
    ArrayMethodSpec {
        name: "reduce",
        is_property: false,
        signature: "reduce<U>(fn: (acc: U, item: T, index?: i64) => U, initial: U): U",
        params: &["fn: (acc: U, item: T, index?: i64) => U", "initial: U"],
        insert_snippet: "reduce((${1:acc}, ${2:item}) => ${3:acc}, ${4:0})",
        doc: "Folds all elements of the array into a single accumulated value starting from `initial`.",
    },
    ArrayMethodSpec {
        name: "some",
        is_property: false,
        signature: "some(predicate: (item: T, index?: i64) => bool): bool",
        params: &["predicate: (item: T, index?: i64) => bool"],
        insert_snippet: "some((${1:item}) => ${2:true})",
        doc: "Returns `true` if at least one element satisfies `predicate` (short-circuits on first match).",
    },
    ArrayMethodSpec {
        name: "every",
        is_property: false,
        signature: "every(predicate: (item: T, index?: i64) => bool): bool",
        params: &["predicate: (item: T, index?: i64) => bool"],
        insert_snippet: "every((${1:item}) => ${2:true})",
        doc: "Returns `true` if every element satisfies `predicate` (short-circuits on first mismatch).",
    },
    ArrayMethodSpec {
        name: "count",
        is_property: false,
        signature: "count(predicate: (item: T, index?: i64) => bool): i64",
        params: &["predicate: (item: T, index?: i64) => bool"],
        insert_snippet: "count((${1:item}) => ${2:true})",
        doc: "Counts the number of elements in the array satisfying `predicate`.",
    },
    ArrayMethodSpec {
        name: "findIndex",
        is_property: false,
        signature: "findIndex(predicate: (item: T, index?: i64) => bool): i64",
        params: &["predicate: (item: T, index?: i64) => bool"],
        insert_snippet: "findIndex((${1:item}) => ${2:true})",
        doc: "Returns the 0-based index of the first element matching `predicate`, or `-1` if none match.",
    },
    ArrayMethodSpec {
        name: "includes",
        is_property: false,
        signature: "includes(value: T): bool",
        params: &["value: T"],
        insert_snippet: "includes(${1:value})",
        doc: "Returns `true` if the array contains an element equal to `value`.",
    },
    ArrayMethodSpec {
        name: "indexOf",
        is_property: false,
        signature: "indexOf(value: T): i64",
        params: &["value: T"],
        insert_snippet: "indexOf(${1:value})",
        doc: "Returns the first index of `value` in the array, or `-1` if not present.",
    },
    ArrayMethodSpec {
        name: "flatMap",
        is_property: false,
        signature: "flatMap<U>(fn: (item: T, index?: i64) => U[]): U[]",
        params: &["fn: (item: T, index?: i64) => U[]"],
        insert_snippet: "flatMap((${1:item}) => ${2:[]})",
        doc: "Maps each element to an array via `fn` and flattens the result by one level.",
    },
    ArrayMethodSpec {
        name: "slice",
        is_property: false,
        signature: "slice(start: i64, end?: i64): T[]",
        params: &["start: i64", "end?: i64"],
        insert_snippet: "slice(${1:0}, ${2:end})",
        doc: "Returns a shallow slice of the array from `start` up to `end` (clamped to bounds).",
    },
    ArrayMethodSpec {
        name: "concat",
        is_property: false,
        signature: "concat(other: T[]): T[]",
        params: &["other: T[]"],
        insert_snippet: "concat(${1:other})",
        doc: "Returns a new array concatenating the receiver with `other`.",
    },
    ArrayMethodSpec {
        name: "reverse",
        is_property: false,
        signature: "reverse(): T[]",
        params: &[],
        insert_snippet: "reverse()",
        doc: "Returns a new array with all elements in reversed order.",
    },
    ArrayMethodSpec {
        name: "push",
        is_property: false,
        signature: "push(item: T): T[]",
        params: &["item: T"],
        insert_snippet: "push(${1:item})",
        doc: "Appends `item` to a linear array (`vec_push` shorthand).",
    },
    ArrayMethodSpec {
        name: "length",
        is_property: true,
        signature: "length: i64",
        params: &[],
        insert_snippet: "length",
        doc: "Returns the number of elements in an array (`vec_len`) or bytes in a string (`str_len`).",
    },
];

fn array_method_hover_doc(ident: &str) -> Option<String> {
    ARRAY_METHODS.iter().find(|m| m.name == ident).map(|m| {
        let prefix = if m.is_property {
            "(array/string property)"
        } else {
            "(array method)"
        };
        format!(
            "```tsx\n{prefix} Array<T>.{}\n```\n\n{}",
            m.signature, m.doc
        )
    })
}

fn ui_field_hover_doc(ident: &str) -> Option<String> {
    for c in JSX_COMPONENTS {
        if c.tag == ident {
            return Some(format!("```tsx\n<{}>\n```\n\n{}\n\n{}", c.tag, c.detail, c.doc));
        }
    }
    for &(name, ty, desc) in JSX_PROPS {
        if name == ident {
            return Some(format!("```tsx\n{name}: {ty}\n```\n\nJSX prop — {desc}"));
        }
    }
    for &(name, ty, desc) in STYLE_FIELDS {
        if name == ident {
            return Some(format!("```tsx\nStyle.{name}: {ty}\n```\n\n{desc}"));
        }
    }
    for &(name, ty, desc) in WIDGET_FIELDS {
        if name == ident {
            return Some(format!("```tsx\nWidget.{name}: {ty}\n```\n\n{desc}"));
        }
    }
    None
}

// ============================================================================
// Completion Provider (`textDocument/completion`)
// ============================================================================

struct BuiltinSpec {
    name: &'static str,
    detail: &'static str,
    insert_snippet: &'static str,
    doc: &'static str,
}

const BUILTIN_FUNCTIONS: &[BuiltinSpec] = &[
    BuiltinSpec {
        name: "sin",
        detail: "fn(x: f64) -> f64",
        insert_snippet: "sin($1)",
        doc: "Sine of x in radians (CPU + GPU shader).",
    },
    BuiltinSpec {
        name: "cos",
        detail: "fn(x: f64) -> f64",
        insert_snippet: "cos($1)",
        doc: "Cosine of x in radians (CPU + GPU shader).",
    },
    BuiltinSpec {
        name: "tan",
        detail: "fn(x: f64) -> f64",
        insert_snippet: "tan($1)",
        doc: "Tangent of x in radians (CPU + GPU shader).",
    },
    BuiltinSpec {
        name: "sqrt",
        detail: "fn(x: f64) -> f64",
        insert_snippet: "sqrt($1)",
        doc: "Square root of x (CPU + GPU shader).",
    },
    BuiltinSpec {
        name: "abs",
        detail: "fn(x: f64) -> f64",
        insert_snippet: "abs($1)",
        doc: "Absolute value of x (CPU + GPU shader).",
    },
    BuiltinSpec {
        name: "floor",
        detail: "fn(x: f64) -> f64",
        insert_snippet: "floor($1)",
        doc: "Round down to nearest integer (CPU + GPU shader).",
    },
    BuiltinSpec {
        name: "ceil",
        detail: "fn(x: f64) -> f64",
        insert_snippet: "ceil($1)",
        doc: "Round up to nearest integer (CPU + GPU shader).",
    },
    BuiltinSpec {
        name: "fract",
        detail: "fn(x: f64) -> f64",
        insert_snippet: "fract($1)",
        doc: "Fractional part x - floor(x) (CPU + GPU shader).",
    },
    BuiltinSpec {
        name: "pow",
        detail: "fn(base: f64, exp: f64) -> f64",
        insert_snippet: "pow($1, $2)",
        doc: "Raise base to exponent (CPU + GPU shader).",
    },
    BuiltinSpec {
        name: "atan2",
        detail: "fn(y: f64, x: f64) -> f64",
        insert_snippet: "atan2($1, $2)",
        doc: "Four-quadrant arctangent of y/x in radians (CPU + GPU shader).",
    },
    BuiltinSpec {
        name: "min",
        detail: "fn(a: f64, b: f64) -> f64",
        insert_snippet: "min($1, $2)",
        doc: "Minimum of a and b (CPU + GPU shader).",
    },
    BuiltinSpec {
        name: "max",
        detail: "fn(a: f64, b: f64) -> f64",
        insert_snippet: "max($1, $2)",
        doc: "Maximum of a and b (CPU + GPU shader).",
    },
    BuiltinSpec {
        name: "clamp",
        detail: "fn(x: f64, lo: f64, hi: f64) -> f64",
        insert_snippet: "clamp($1, $2, $3)",
        doc: "Clamp x to [lo, hi] (CPU + GPU shader).",
    },
    BuiltinSpec {
        name: "lerp",
        detail: "fn(a: f64, b: f64, t: f64) -> f64",
        insert_snippet: "lerp($1, $2, $3)",
        doc: "Linear interpolation a + (b - a) * t (CPU + GPU shader).",
    },
    BuiltinSpec {
        name: "mix",
        detail: "fn(a: f64, b: f64, t: f64) -> f64",
        insert_snippet: "mix($1, $2, $3)",
        doc: "Linear interpolation alias for lerp(a, b, t).",
    },
    BuiltinSpec {
        name: "smoothstep",
        detail: "fn(edge0: f64, edge1: f64, x: f64) -> f64",
        insert_snippet: "smoothstep($1, $2, $3)",
        doc: "Smooth Hermite interpolation between edge0 and edge1.",
    },
    BuiltinSpec {
        name: "to_f64",
        detail: "fn(x: i64) -> f64",
        insert_snippet: "to_f64($1)",
        doc: "Convert i64 to f64.",
    },
    BuiltinSpec {
        name: "to_i64",
        detail: "fn(x: f64) -> i64",
        insert_snippet: "to_i64($1)",
        doc: "Convert f64 to i64.",
    },
    BuiltinSpec {
        name: "to_str",
        detail: "fn(x: i64 | f64 | bool | str) -> str",
        insert_snippet: "to_str($1)",
        doc: "Convert primitive value to string.",
    },
    BuiltinSpec {
        name: "str_len",
        detail: "fn(s: str) -> i64",
        insert_snippet: "str_len($1)",
        doc: "Return byte length of string.",
    },
    BuiltinSpec {
        name: "vec_new",
        detail: "fn() -> vec[T]",
        insert_snippet: "vec_new()",
        doc: "Create an empty linear vector.",
    },
    BuiltinSpec {
        name: "vec_push",
        detail: "fn(v: vec[T], item: T) -> vec[T]",
        insert_snippet: "vec_push($1, $2)",
        doc: "Append item to linear vector v and return updated vector.",
    },
    BuiltinSpec {
        name: "vec_get",
        detail: "fn(v: &vec[T], index: i64) -> T",
        insert_snippet: "vec_get($1, $2)",
        doc: "Read element at index from vector v.",
    },
    BuiltinSpec {
        name: "vec_len",
        detail: "fn(v: &vec[T]) -> i64",
        insert_snippet: "vec_len($1)",
        doc: "Return number of elements in vector v.",
    },
    BuiltinSpec {
        name: "map_new",
        detail: "fn() -> map[K, V]",
        insert_snippet: "map_new()",
        doc: "Create an empty linear map.",
    },
    BuiltinSpec {
        name: "map_set",
        detail: "fn(m: map[K, V], k: K, v: V) -> map[K, V]",
        insert_snippet: "map_set($1, $2, $3)",
        doc: "Insert key/value into linear map and return updated map.",
    },
    BuiltinSpec {
        name: "map_get",
        detail: "fn(m: &map[K, V], k: K) -> V",
        insert_snippet: "map_get($1, $2)",
        doc: "Lookup value for key in map.",
    },
    BuiltinSpec {
        name: "buf_alloc",
        detail: "fn(size: i64) -> buf",
        insert_snippet: "buf_alloc($1)",
        doc: "Allocate linear byte buffer of size bytes.",
    },
    BuiltinSpec {
        name: "buf_read",
        detail: "fn(b: &buf, offset: i64) -> i64",
        insert_snippet: "buf_read($1, $2)",
        doc: "Read byte from buffer at offset.",
    },
    BuiltinSpec {
        name: "buf_write",
        detail: "fn(b: buf, offset: i64, val: i64) -> buf",
        insert_snippet: "buf_write($1, $2, $3)",
        doc: "Write byte to linear buffer and return updated buffer.",
    },
    BuiltinSpec {
        name: "free",
        detail: "fn(x: T)",
        insert_snippet: "free($1)",
        doc: "Explicitly release a linear value.",
    },
];

/// All 45 fields of `type Style` from `std/ui`.
const STYLE_FIELDS: &[(&str, &str, &str)] = &[
    ("w", "f64", "Fixed width in logical pixels (0.0 = auto, -1.0 = 100%)."),
    ("h", "f64", "Fixed height in logical pixels (0.0 = auto, -1.0 = 100%)."),
    ("min_w", "f64", "Minimum width constraint in logical pixels."),
    ("max_w", "f64", "Maximum width constraint in logical pixels."),
    ("bg", "string", "Background fill color (`#RGB`, `#RRGGBB`, `#RRGGBBAA`, `linear(...)`, or `radial(...)`)."),
    ("color", "string", "Foreground text/icon color (`#RRGGBB` / `#RRGGBBAA`)."),
    ("border", "string", "Border stroke color (`#RRGGBB` / `#RRGGBBAA`)."),
    ("border_w", "f64", "Border stroke width in logical pixels."),
    ("r_tl", "f64", "Top-left corner radius in logical pixels."),
    ("r_tr", "f64", "Top-right corner radius in logical pixels."),
    ("r_br", "f64", "Bottom-right corner radius in logical pixels."),
    ("r_bl", "f64", "Bottom-left corner radius in logical pixels."),
    ("pad_t", "f64", "Top inner padding in logical pixels."),
    ("pad_r", "f64", "Right inner padding in logical pixels."),
    ("pad_b", "f64", "Bottom inner padding in logical pixels."),
    ("pad_l", "f64", "Left inner padding in logical pixels."),
    ("gap_x", "f64", "Horizontal spacing between children in logical pixels."),
    ("gap_y", "f64", "Vertical spacing between children in logical pixels."),
    ("font_size", "f64", "Font or icon size in logical pixels."),
    ("font_weight", "i64", "Font weight (400 regular, 500 medium, 600 semibold, 700 bold, 800 extrabold)."),
    ("align", "string", "Cross-axis alignment (`\"start\"`, `\"center\"`, `\"end\"`, `\"stretch\"`)."),
    ("justify", "string", "Main-axis justification (`\"start\"`, `\"center\"`, `\"end\"`, `\"between\"`, `\"around\"`)."),
    ("grid_cols", "i64", "Number of columns when widget kind is `\"grid\"`."),
    ("shadow_x", "f64", "Drop shadow horizontal offset in pixels."),
    ("shadow_y", "f64", "Drop shadow vertical offset in pixels."),
    ("shadow_blur", "f64", "Drop shadow blur radius in pixels."),
    ("shadow_color", "string", "Drop shadow color (`#RRGGBBAA`)."),
    ("backdrop_blur", "f64", "Glassmorphism backdrop blur radius in logical pixels."),
    ("opacity", "f64", "Element opacity in `[0.0, 1.0]`."),
    ("translate_x", "f64", "Horizontal transform translation in pixels."),
    ("translate_y", "f64", "Vertical transform translation in pixels."),
    ("scale", "f64", "Uniform transform scale factor (default `1.0`)."),
    ("rotate_deg", "f64", "2D rotation angle in degrees."),
    ("hover_bg", "string", "Background color while pointer hovers the widget."),
    ("hover_border", "string", "Border color while pointer hovers the widget."),
    ("hover_scale", "f64", "Transform scale factor while pointer hovers the widget."),
    ("transition_ms", "f64", "Animated style transition duration in milliseconds."),
    ("overflow", "string", "Overflow mode (`\"visible\"`, `\"hidden\"`, `\"scroll\"`)."),
    ("z_index", "i64", "Layer stacking order (higher renders on top)."),
    ("grow", "f64", "Flex grow factor inside a `row` or `col` container."),
    ("font_family", "string", "Font family (`\"sans\"` for Inter/Sans or `\"mono\"` for JetBrains Mono)."),
    ("text_align", "string", "Horizontal text alignment (`\"left\"`, `\"center\"`, `\"right\"`)."),
    ("line_height", "f64", "Line-height multiplier (e.g. `1.5`, or `0.0` for automatic scaling)."),
    ("col_span", "i64", "Bento `<Grid>` column span for this child (default `1`)."),
    ("letter_spacing", "f64", "Horizontal letter tracking adjustment in logical pixels."),
];

/// All 8 fields of `type Widget` from `std/ui`.
const WIDGET_FIELDS: &[(&str, &str, &str)] = &[
    ("key", "string", "Stable unique identity key for diffing and animations."),
    ("kind", "string", "Widget kind (`\"col\"`, `\"row\"`, `\"grid\"`, `\"stack\"`, `\"card\"`, `\"text\"`, `\"code\"`, `\"badge\"`, `\"button\"`, `\"input\"`, `\"slider\"`, `\"toggle\"`, `\"shader\"`, `\"icon\"`, `\"divider\"`, `\"progress\"`, `\"spacer\"`)."),
    ("text", "string", "Label, text content, icon name, or `@shader` function name."),
    ("action", "string", "Action identifier dispatched to `update`, `on_slider`, or `on_input` (or `lang` on `<Code>`)."),
    ("value", "f64", "Numeric widget value (slider position `0.0..=1.0`, progress `0.0..=1.0`, toggle `0.0`/`1.0`)."),
    ("uniforms", "f64[]", "Custom float uniform parameters forwarded to GPU `@shader`."),
    ("style", "Style", "45-field statically typed visual & layout `Style`."),
    ("children", "Widget[]", "Nested child `Widget` nodes."),
];

struct JsxComponentSpec {
    tag: &'static str,
    detail: &'static str,
    snippet: &'static str,
    doc: &'static str,
}

const JSX_COMPONENTS: &[JsxComponentSpec] = &[
    JsxComponentSpec {
        tag: "Column",
        detail: "Vertical Flexbox container (`kind: \"col\"`)",
        snippet: "Column p={${1:24}} gap={${2:16}}>\n    $0\n</Column>",
        doc: "Arranges children vertically with padding, gap, alignment (`align`), and scrolling.",
    },
    JsxComponentSpec {
        tag: "Row",
        detail: "Horizontal Flexbox container (`kind: \"row\"`)",
        snippet: "Row gap={${1:12}} align=\"${2:center}\">\n    $0\n</Row>",
        doc: "Arranges children horizontally with wrapping, `grow`, `justify`, and cross-axis `align`.",
    },
    JsxComponentSpec {
        tag: "Grid",
        detail: "Responsive Bento Grid container (`kind: \"grid\"`)",
        snippet: "Grid cols={${1:3}} gap={${2:16}}>\n    $0\n</Grid>",
        doc: "Responsive multi-column Bento grid supporting child `span`/`colSpan` and equal row heights.",
    },
    JsxComponentSpec {
        tag: "Stack",
        detail: "Layered overlay container (`kind: \"stack\"`)",
        snippet: "Stack>\n    $0\n</Stack>",
        doc: "Overlaps children in the same bounding box ordered by `z` / `zIndex`.",
    },
    JsxComponentSpec {
        tag: "Card",
        detail: "Elevated glassmorphism card (`kind: \"card\"`)",
        snippet: "Card p={${1:22}} gap={${2:12}} radius={${3:16}}>\n    $0\n</Card>",
        doc: "Glass card surface with rounded SDF fill, drop shadow, subtle specular highlight, and border.",
    },
    JsxComponentSpec {
        tag: "Scroll",
        detail: "Scrollable vertical viewport (`kind: \"scroll\"`)",
        snippet: "Scroll gap={${1:16}}>\n    $0\n</Scroll>",
        doc: "Vertical scrolling container.",
    },
    JsxComponentSpec {
        tag: "Text",
        detail: "Wrapped multi-line typography (`kind: \"text\"`)",
        snippet: "Text size={${1:15}} color=\"${2:#e2e8f0}\">$0</Text>",
        doc: "Anti-aliased text node supporting `size`, `weight`, `font` (`\"sans\"`/`\"mono\"`), `textAlign`, `lineHeight`, and `letterSpacing`.",
    },
    JsxComponentSpec {
        tag: "Code",
        detail: "Syntax-highlighted monospace code block (`kind: \"code\"`)",
        snippet: "Code lang=\"${1:tsx}\">{`$0`}</Code>",
        doc: "Renders syntax-highlighted monospace source code identically on Native GPU, Headless PNG, and Canvas Web.",
    },
    JsxComponentSpec {
        tag: "Badge",
        detail: "Pill status badge (`kind: \"badge\"`)",
        snippet: "Badge bg=\"${1:#0c1930}\" color=\"${2:#38bdf8}\">$0</Badge>",
        doc: "Compact rounded pill label.",
    },
    JsxComponentSpec {
        tag: "Button",
        detail: "Interactive clickable button (`kind: \"button\"`)",
        snippet: "Button onClick=\"${1:action}\" bg=\"${2:#0ea5e9}\">$0</Button>",
        doc: "Interactive button dispatching `onClick` to `update(state, action)`. Accepts text or `<Icon>` + `<Text>` children.",
    },
    JsxComponentSpec {
        tag: "Shader",
        detail: "Live GPU `@shader` viewport (`kind: \"shader\"`)",
        snippet: "Shader fn={${1:fx}} h={${2:240}} radius={${3:12}} uniforms={[${4}]} />",
        doc: "Executes a verified `@shader` function on `wgpu` (Desktop), WebGPU/WebGL2 (Web), and Cranelift JIT (`knr render`).",
    },
    JsxComponentSpec {
        tag: "Icon",
        detail: "Crisp 24x24 analytical vector icon (`kind: \"icon\"`)",
        snippet: "Icon name=\"${1:sparkles}\" size={${2:18}} color=\"${3:#38bdf8}\" />",
        doc: "Anti-aliased built-in analytical vector icon supporting 67 icons (`search`, `sparkles`, `database`, `git`, `zap`, `cpu`, `layers`, `shield`, `terminal`, `code`, `play`, `pause`, `refresh`, `box`, `cube`, `check`, `x`, `alert`, `arrow-right`, `sliders`, `eye`, `globe`, `lock`, `rocket`, `command`, `git-branch`, `palette`, `activity`, `plus`, `minus`, `copy`, `sun`, `moon`, `diamond`, and more).",
    },
    JsxComponentSpec {
        tag: "Slider",
        detail: "Interactive continuous slider (`kind: \"slider\"`)",
        snippet: "Slider onChange=\"${1:slider_param}\" value={${2:0.5}} />",
        doc: "Draggable `0.0..=1.0` range slider dispatching to `on_slider(state, action, val)`.",
    },
    JsxComponentSpec {
        tag: "Toggle",
        detail: "Interactive boolean switch (`kind: \"toggle\"`)",
        snippet: "Toggle onToggle=\"${1:toggle_flag}\" checked={${2:true}}>${3:Enabled}</Toggle>",
        doc: "Boolean switch pill dispatching `onToggle` to `update(state, action)`.",
    },
    JsxComponentSpec {
        tag: "Input",
        detail: "Interactive single-line text input (`kind: \"input\"`)",
        snippet: "Input onInput=\"${1:input_field}\" value={${2:s.query}} />",
        doc: "Text input dispatching edits to `on_input(state, action, text)`.",
    },
    JsxComponentSpec {
        tag: "Progress",
        detail: "Gradient progress indicator bar (`kind: \"progress\"`)",
        snippet: "Progress value={${1:0.75}} h={${2:8}} />",
        doc: "Horizontal progress bar with `value` in `0.0..=1.0`.",
    },
    JsxComponentSpec {
        tag: "Divider",
        detail: "Horizontal hairline separator (`kind: \"divider\"`)",
        snippet: "Divider />",
        doc: "Subtle 1px horizontal divider rule.",
    },
    JsxComponentSpec {
        tag: "Spacer",
        detail: "Vertical/horizontal empty spacing block (`kind: \"spacer\"`)",
        snippet: "Spacer size={${1:16}} />",
        doc: "Inserts explicit empty spacing (`size` or `h`).",
    },
    JsxComponentSpec {
        tag: "Image",
        detail: "Raster image surface (`kind: \"image\"`)",
        snippet: "Image src=\"${1:./asset.png}\" h={${2:200}} radius={${3:12}} />",
        doc: "Displays a PNG/JPEG/WebP image clipped to rounded corners.",
    },
];

const JSX_PROPS: &[(&str, &str, &str)] = &[
    ("p", "f64", "Uniform padding on all 4 sides."),
    ("px", "f64", "Horizontal left + right padding."),
    ("py", "f64", "Vertical top + bottom padding."),
    ("pt", "f64", "Top padding (`pad_t`)."),
    ("pr", "f64", "Right padding (`pad_r`)."),
    ("pb", "f64", "Bottom padding (`pad_b`)."),
    ("pl", "f64", "Left padding (`pad_l`)."),
    ("gap", "f64", "Uniform horizontal and vertical gap between children."),
    ("gapX", "f64", "Horizontal gap (`gap_x`)."),
    ("gapY", "f64", "Vertical gap (`gap_y`)."),
    ("w", "f64 | \"full\" | \"auto\"", "Width in pixels or `\"full\"` (`-1.0`)."),
    ("h", "f64 | \"full\" | \"auto\"", "Height in pixels or `\"full\"` (`-1.0`)."),
    ("minW", "f64", "Minimum width (`min_w`)."),
    ("maxW", "f64", "Maximum width (`max_w`)."),
    ("grow", "f64", "Flex grow weight inside `<Row>` / `<Column>`."),
    ("cols", "i64", "Number of columns on `<Grid>` (`grid_cols`)."),
    ("span", "i64", "Number of Bento columns spanned inside `<Grid>` (`col_span`)."),
    ("colSpan", "i64", "Alias for `span`: Bento `<Grid>` column span (`col_span`)."),
    ("size", "f64", "Font size on `<Text>`/`<Code>` or icon size on `<Icon>` (`font_size`)."),
    ("weight", "\"normal\" | \"medium\" | \"semibold\" | \"bold\" | i64", "Font weight (`font_weight`)."),
    ("font", "\"sans\" | \"mono\"", "Font family (`font_family`)."),
    ("textAlign", "\"left\" | \"center\" | \"right\"", "Horizontal text alignment (`text_align`)."),
    ("lineHeight", "f64", "Line-height multiplier (`line_height`)."),
    ("letterSpacing", "f64", "Extra character tracking in px (`letter_spacing`)."),
    ("radius", "f64", "Uniform corner radius across all 4 corners."),
    ("onClick", "string", "Action string dispatched to `update(state, action)` on click."),
    ("onChange", "string", "Action string dispatched to `on_slider(state, action, value)`."),
    ("onInput", "string", "Action string dispatched to `on_input(state, action, text)`."),
    ("onToggle", "string", "Action string dispatched to `update(state, action)` on toggle."),
    ("checked", "bool", "Boolean checked state on `<Toggle>`."),
    ("fn", "@shader function", "GPU `@shader` function reference for `<Shader fn={...} />`."),
    ("name", "string", "Vector icon name for `<Icon name=\"...\" />`."),
    ("lang", "string", "Language tag for `<Code lang=\"tsx\">`."),
    ("hoverBg", "string", "Hover background fill (`hover_bg`)."),
    ("hoverBorder", "string", "Hover border color (`hover_border`)."),
    ("hoverScale", "f64", "Hover scale factor (`hover_scale`)."),
    ("blur", "f64", "Backdrop blur radius (`backdrop_blur`)."),
    ("shadowBlur", "f64", "Drop shadow blur radius (`shadow_blur`)."),
    ("shadowColor", "string", "Drop shadow color (`shadow_color`)."),
];

struct KeywordSnippet {
    label: &'static str,
    kind: u32,
    detail: &'static str,
    insert_text: &'static str,
    insert_format: u32,
    doc: &'static str,
}

const KEYWORD_SNIPPETS: &[KeywordSnippet] = &[
    KeywordSnippet {
        label: "@shader",
        kind: COMPLETION_KIND_SNIPPET,
        detail: "Declare a GPU-pure @shader function",
        insert_text: "@shader\nexport function ${1:fx}(uv: Vec2, time: f64): Color {\n    const w = 0.5 + 0.5 * sin(uv.x * 6.0 + time);\n    return { r: w, g: 0.4, b: 0.9, a: 1.0 };\n}",
        insert_format: INSERT_FORMAT_SNIPPET,
        doc: "Declares a GPU-pure fragment shader compiled to WGSL (Desktop wgpu + WebGPU) and GLSL ES 3.00 (WebGL2).",
    },
    KeywordSnippet {
        label: "@effects(\"net\")",
        kind: COMPLETION_KIND_SNIPPET,
        detail: "Declare network capability decorator",
        insert_text: "@effects(\"net\")",
        insert_format: INSERT_FORMAT_PLAIN,
        doc: "Grants the function permission to invoke `net` syscalls.",
    },
    KeywordSnippet {
        label: "@effects(\"ui\")",
        kind: COMPLETION_KIND_SNIPPET,
        detail: "Declare UI capability decorator",
        insert_text: "@effects(\"ui\")",
        insert_format: INSERT_FORMAT_PLAIN,
        doc: "Grants the function permission to invoke `ui` syscalls.",
    },
    KeywordSnippet {
        label: "@effects(\"hw\")",
        kind: COMPLETION_KIND_SNIPPET,
        detail: "Declare hardware/filesystem capability decorator",
        insert_text: "@effects(\"hw\")",
        insert_format: INSERT_FORMAT_PLAIN,
        doc: "Grants the function permission to invoke `hw` syscalls (`std/sys`).",
    },
    KeywordSnippet {
        label: "@effects(\"async\")",
        kind: COMPLETION_KIND_SNIPPET,
        detail: "Declare async/channel capability decorator",
        insert_text: "@effects(\"async\")",
        insert_format: INSERT_FORMAT_PLAIN,
        doc: "Grants the function permission to create channels and spawn tasks.",
    },
    KeywordSnippet {
        label: "export function",
        kind: COMPLETION_KIND_SNIPPET,
        detail: "Declare an exported Strict TypeScript function",
        insert_text: "export function ${1:name}(${2:params}): ${3:i64} {\n    $0\n}",
        insert_format: INSERT_FORMAT_SNIPPET,
        doc: "Declares a statically verified Strict TypeScript / TSX function.",
    },
    KeywordSnippet {
        label: "export type",
        kind: COMPLETION_KIND_SNIPPET,
        detail: "Declare a nominal record type",
        insert_text: "export type ${1:State} = {\n    ${2:count}: ${3:i64};\n};",
        insert_format: INSERT_FORMAT_SNIPPET,
        doc: "Declares a statically typed nominal struct schema.",
    },
    KeywordSnippet {
        label: "export enum",
        kind: COMPLETION_KIND_SNIPPET,
        detail: "Declare a tagged union enum",
        insert_text: "export enum ${1:Action} {\n    ${2:Inc}(${3:i64}),\n    ${4:Reset},\n}",
        insert_format: INSERT_FORMAT_SNIPPET,
        doc: "Declares a tagged union enum lowered to a verified tagged schema.",
    },
    KeywordSnippet {
        label: "import { ... } from \"std/ui\";",
        kind: COMPLETION_KIND_MODULE,
        detail: "Import JSX UI components, Style (45 fields), Widget & anim helpers",
        insert_text: "import { Column, Row, Grid, Card, Text, Code, Badge, Button, Shader, Icon } from \"std/ui\";",
        insert_format: INSERT_FORMAT_PLAIN,
        doc: "Imports `Vec2`, `Color`, 45-field `Style`, `Widget`, and JSX UI primitives.",
    },
    KeywordSnippet {
        label: "import { ... } from \"std/math\";",
        kind: COMPLETION_KIND_MODULE,
        detail: "Import standard GPU-compatible 2D/3D math helpers",
        insert_text: "import { Vec2, Vec3, smin, vec3_norm, pi, tau } from \"std/math\";",
        insert_format: INSERT_FORMAT_PLAIN,
        doc: "Imports standard 2D/3D vector and SDF math helpers.",
    },
    KeywordSnippet {
        label: "import { ... } from \"std/sys\";",
        kind: COMPLETION_KIND_MODULE,
        detail: "Import filesystem, JSON & HTTP client helpers",
        insert_text: "import { fs_read, fs_write, json_get, http_get } from \"std/sys\";",
        insert_format: INSERT_FORMAT_PLAIN,
        doc: "Imports `@effects(\"hw\")` and `@effects(\"net\")` system helpers.",
    },
    KeywordSnippet {
        label: "import { ... } from \"std/theme\";",
        kind: COMPLETION_KIND_MODULE,
        detail: "Import curated design tokens & themes",
        insert_text: "import { Theme, theme_obsidian, theme_light, theme_nord, theme_solar } from \"std/theme\";",
        insert_format: INSERT_FORMAT_PLAIN,
        doc: "Imports `Theme` and curated dark/light/nord/solar color palettes.",
    },
    KeywordSnippet {
        label: "import { ... } from \"std/router\";",
        kind: COMPLETION_KIND_MODULE,
        detail: "Import zero-GC client-side router helpers",
        insert_text: "import { Route, router_init, router_push, router_back, router_is } from \"std/router\";",
        insert_format: INSERT_FORMAT_PLAIN,
        doc: "Imports `Route` and zero-GC MVU navigation helpers.",
    },
    KeywordSnippet {
        label: "import { ... } from \"std/layout\";",
        kind: COMPLETION_KIND_MODULE,
        detail: "Import responsive breakpoint & layout helpers",
        insert_text: "import { is_compact, is_medium, is_wide, responsive_cols, responsive_pad } from \"std/layout\";",
        insert_format: INSERT_FORMAT_PLAIN,
        doc: "Imports responsive viewport breakpoint and Bento column helpers.",
    },
    KeywordSnippet {
        label: "import { ... } from \"std/testing\";",
        kind: COMPLETION_KIND_MODULE,
        detail: "Import deterministic unit test assertions",
        insert_text: "import { assert_eq_i64, assert_near, assert_str_eq, assert_true } from \"std/testing\";",
        insert_format: INSERT_FORMAT_PLAIN,
        doc: "Imports pure unit test assertion helpers for `knr test`.",
    },
    KeywordSnippet {
        label: "import \"std/ui\";",
        kind: COMPLETION_KIND_MODULE,
        detail: "Import all standard UI & styling primitives",
        insert_text: "import \"std/ui\";",
        insert_format: INSERT_FORMAT_PLAIN,
        doc: "Imports `Vec2`, `Color`, 45-field `Style`, `Widget`, and animation helpers.",
    },
    KeywordSnippet {
        label: "while",
        kind: COMPLETION_KIND_KEYWORD,
        detail: "While loop",
        insert_text: "while (${1:cond}) {\n    $0\n}",
        insert_format: INSERT_FORMAT_SNIPPET,
        doc: "Structured loop lowered to SSA block-argument headers.",
    },
    KeywordSnippet {
        label: "if",
        kind: COMPLETION_KIND_KEYWORD,
        detail: "Conditional branch",
        insert_text: "if (${1:cond}) {\n    $0\n}",
        insert_format: INSERT_FORMAT_SNIPPET,
        doc: "Conditional statement.",
    },
    KeywordSnippet {
        label: "else",
        kind: COMPLETION_KIND_KEYWORD,
        detail: "Alternative branch",
        insert_text: "else {\n    $0\n}",
        insert_format: INSERT_FORMAT_SNIPPET,
        doc: "Alternative branch for `if`.",
    },
    KeywordSnippet {
        label: "return",
        kind: COMPLETION_KIND_KEYWORD,
        detail: "Return from function",
        insert_text: "return $0;",
        insert_format: INSERT_FORMAT_SNIPPET,
        doc: "Returns a value from the enclosing function.",
    },
    KeywordSnippet {
        label: "readonly",
        kind: COMPLETION_KIND_KEYWORD,
        detail: "Second-class borrow parameter modifier",
        insert_text: "readonly ",
        insert_format: INSERT_FORMAT_PLAIN,
        doc: "Borrows a linear value read-only without consuming ownership (zero lifetimes).",
    },
    KeywordSnippet {
        label: "syscall",
        kind: COMPLETION_KIND_SNIPPET,
        detail: "Invoke an effectful runtime syscall",
        insert_text: "syscall(\"${1:net}\", \"${2:op}\", ${3:payload})",
        insert_format: INSERT_FORMAT_SNIPPET,
        doc: "Performs a capability-checked runtime syscall.",
    },
];

fn handle_completion(state: &LspState, params: Option<&Value>) -> Value {
    let uri = params
        .and_then(|p| p.pointer("/textDocument/uri"))
        .and_then(Value::as_str);
    let trigger = params
        .and_then(|p| p.pointer("/context/triggerCharacter"))
        .and_then(Value::as_str);
    let pos_line = params
        .and_then(|p| p.pointer("/position/line"))
        .and_then(Value::as_u64)
        .map(|v| v as u32);
    let pos_col = params
        .and_then(|p| p.pointer("/position/character"))
        .and_then(Value::as_u64)
        .map(|v| v as u32);

    let doc_text = uri.and_then(|u| state.documents.get(u)).map(String::as_str);

    // Special Context A: Inside an import/export module path string (`from "./|"`, `import "./|"`, `from "std/|"`)
    if let (Some(u), Some(text), Some(line), Some(col)) = (uri, doc_text, pos_line, pos_col) {
        if let Some(partial_path) = import_path_completion_context(text, line, col) {
            return Value::Array(complete_import_paths(state, u, &partial_path));
        }

        // Special Context B: Inside `import { | } from "./state.tsk"` or `export { | } from "..."`
        if let Some((target_module, already_named)) =
            import_named_list_completion_context(text, line, col)
        {
            let named_items =
                complete_named_import_symbols(state, u, &target_module, &already_named);
            if !named_items.is_empty() {
                return Value::Array(named_items);
            }
        }

        // Special Context C: Inside `<Icon name="|" />` -> suggest all 67 analytical vector icons at priority "0"
        if let Some(partial_icon) = icon_name_completion_context(text, line, col) {
            return Value::Array(complete_icon_names(&partial_icon));
        }
    }

    let line_prefix = match (doc_text, pos_line, pos_col) {
        (Some(text), Some(line), Some(col)) => text.split('\n').nth(line as usize).map(|l| {
            let trimmed = l.trim_end_matches('\r');
            let byte_idx = utf16_col_to_byte_idx(trimmed, col);
            trimmed[..byte_idx].to_string()
        }),
        _ => None,
    };

    let prefer_fields = trigger == Some(".")
        || line_prefix
            .as_deref()
            .is_some_and(|p| p.ends_with('.'));
    let prefer_imports = trigger == Some("\"")
        || line_prefix
            .as_deref()
            .is_some_and(|p| p.contains("import"));
    let prefer_jsx_tags = trigger == Some("<")
        || line_prefix
            .as_deref()
            .is_some_and(|p| p.ends_with('<'));
    let prefer_decorators = trigger == Some("@")
        || line_prefix
            .as_deref()
            .is_some_and(|p| p.ends_with('@'));
    let in_jsx_open_tag = line_prefix.as_deref().is_some_and(|p| {
        if let Some(lt) = p.rfind('<') {
            let after = &p[lt + 1..];
            !after.contains('>')
                && after.chars().next().is_some_and(|c| c.is_ascii_uppercase())
                && after.contains(' ')
        } else {
            false
        }
    });

    let mut items: Vec<Value> = Vec::new();
    let mut seen: HashSet<(String, u32)> = HashSet::new();

    // 0. If completing `ns.` where `import * as ns from "./mod.tsk"`, prioritize that module's exports
    if let (Some(u), Some(prefix)) = (uri, line_prefix.as_deref()) {
        if let Some(ns_ident) = trailing_dot_qualifier(prefix) {
            if let Some(prog) = program_for_uri(state, u) {
                if let Some(ns_mod) = resolve_namespace_target_in_program(state, u, &prog, &ns_ident)
                {
                    push_program_declaration_completions(
                        &ns_mod.prog,
                        Some(&ns_mod.import_spec),
                        false,
                        false,
                        true,
                        &mut seen,
                        &mut items,
                    );
                }
            }
        }
    }

    // 1. Document-local declarations (function, @shader, type, enum, const) + their fields
    if let Some(u) = uri {
        let cur_prog = program_for_uri(state, u);
        if let Some(ref prog) = cur_prog {
            push_program_declaration_completions(
                prog,
                None,
                prefer_fields,
                prefer_jsx_tags,
                false,
                &mut seen,
                &mut items,
            );
        }

        // 1a. Cross-file imported module declarations, JSX components, enum variants, and struct fields
        //     (tolerant fallback scans `import ... from "..."` even when current line is mid-edit like `s.`)
        let mut imported = match (cur_prog.as_ref(), doc_text) {
            (Some(prog), _) => load_imported_programs_recursive(state, u, prog),
            (None, Some(text)) => load_imported_programs_tolerant(state, u, text),
            (None, None) => Vec::new(),
        };
        match (cur_prog.as_ref(), doc_text) {
            (Some(prog), _) => imported.extend(load_imported_stdlib_programs(prog)),
            (None, Some(text)) => imported.extend(load_imported_stdlib_programs_tolerant(text)),
            (None, None) => {}
        }
        for imp in imported {
            push_program_declaration_completions(
                &imp.prog,
                Some(&imp.import_spec),
                prefer_fields,
                prefer_jsx_tags,
                false,
                &mut seen,
                &mut items,
            );
        }
    }

    // 1a-ii. v0.4.0 Functional Array Methods (`.map`, `.filter`, `.reduce`, ..., `.push`, `.length`)
    for method in ARRAY_METHODS {
        let kind = if method.is_property {
            COMPLETION_KIND_PROPERTY
        } else {
            COMPLETION_KIND_METHOD
        };
        let key = (method.name.to_string(), kind);
        if seen.insert(key) {
            let sort = if prefer_fields { "0" } else { "3" };
            let insert_fmt = if method.is_property {
                INSERT_FORMAT_PLAIN
            } else {
                INSERT_FORMAT_SNIPPET
            };
            items.push(json!({
                "label": method.name,
                "kind": kind,
                "detail": format!("Array<T>.{}", method.signature),
                "documentation": method.doc,
                "insertText": method.insert_snippet,
                "insertTextFormat": insert_fmt,
                "sortText": format!("{sort}_arr_{}", method.name)
            }));
        }
    }

    // 1b. Built-in JSX UI components (`<Column>`, `<Row>`, `<Grid>`, `<Card>`, `<Text>`, `<Code>`, `<Shader>`, `<Icon>`, ...)
    for comp in JSX_COMPONENTS {
        let key = (comp.tag.to_string(), COMPLETION_KIND_CLASS);
        if seen.insert(key) {
            let sort = if prefer_jsx_tags { "0" } else { "2" };
            items.push(json!({
                "label": comp.tag,
                "kind": COMPLETION_KIND_CLASS,
                "detail": comp.detail,
                "documentation": comp.doc,
                "insertText": comp.snippet,
                "insertTextFormat": INSERT_FORMAT_SNIPPET,
                "sortText": format!("{sort}_jsx_{}", comp.tag)
            }));
        }
    }

    // 1c. Built-in JSX props (`p`, `gap`, `cols`, `span`, `colSpan`, `onClick`, `fn`, `lang`, `name`, ...)
    for &(prop, ty, doc) in JSX_PROPS {
        let key = (prop.to_string(), COMPLETION_KIND_PROPERTY);
        if seen.insert(key) {
            let sort = if in_jsx_open_tag { "0" } else { "3" };
            items.push(json!({
                "label": prop,
                "kind": COMPLETION_KIND_PROPERTY,
                "detail": format!("JSX prop {prop}: {ty}"),
                "documentation": doc,
                "sortText": format!("{sort}_prop_{prop}")
            }));
        }
    }

    // 2. Keywords, decorators (`@shader`, `@effects(...)`), and stdlib snippets
    for kw in KEYWORD_SNIPPETS {
        let key = (kw.label.to_string(), kw.kind);
        if seen.insert(key) {
            let sort = if (prefer_imports && kw.label.starts_with("import"))
                || (prefer_decorators && kw.label.starts_with('@'))
            {
                "0"
            } else {
                "4"
            };
            items.push(json!({
                "label": kw.label,
                "kind": kw.kind,
                "detail": kw.detail,
                "documentation": kw.doc,
                "insertText": kw.insert_text,
                "insertTextFormat": kw.insert_format,
                "sortText": format!("{sort}_{}", kw.label)
            }));
        }
    }

    // 3. Math & runtime builtins
    for b in BUILTIN_FUNCTIONS {
        let key = (b.name.to_string(), COMPLETION_KIND_FUNCTION);
        if seen.insert(key) {
            let sort = if prefer_fields { "4" } else { "2" };
            items.push(json!({
                "label": b.name,
                "kind": COMPLETION_KIND_FUNCTION,
                "detail": b.detail,
                "documentation": b.doc,
                "insertText": b.insert_snippet,
                "insertTextFormat": INSERT_FORMAT_SNIPPET,
                "sortText": format!("{sort}_{}", b.name)
            }));
        }
    }

    // 4. All 45 `Style` fields
    for &(field, ty, doc) in STYLE_FIELDS {
        let key = (field.to_string(), COMPLETION_KIND_FIELD);
        if seen.insert(key) {
            let sort = if prefer_fields { "0" } else { "3" };
            items.push(json!({
                "label": field,
                "kind": COMPLETION_KIND_FIELD,
                "detail": format!("Style.{field}: {ty}"),
                "documentation": doc,
                "sortText": format!("{sort}_style_{field}")
            }));
        }
    }

    // 5. All 8 `Widget` fields
    for &(field, ty, doc) in WIDGET_FIELDS {
        let key = (field.to_string(), COMPLETION_KIND_FIELD);
        if seen.insert(key) {
            let sort = if prefer_fields { "0" } else { "3" };
            items.push(json!({
                "label": field,
                "kind": COMPLETION_KIND_FIELD,
                "detail": format!("Widget.{field}: {ty}"),
                "documentation": doc,
                "sortText": format!("{sort}_widget_{field}")
            }));
        }
    }

    // 6. Standard UI & stdlib schemas if not already declared in this file
    for (name, detail) in [
        ("Style", "45-field UI styling type"),
        ("Widget", "Declarative MVU UI widget type"),
        ("Vec2", "2D floating-point vector type"),
        ("Color", "Normalized RGBA color type"),
        ("Theme", "Design token color theme (std/theme)"),
        ("Route", "Zero-GC client-side router state (std/router)"),
        ("Option", "Generic Option<T> algebraic enum (std/core)"),
        ("Result", "Generic Result<T, E> algebraic enum (std/core)"),
        ("Cmd", "Declarative MVU async side-effect command (std/cmd)"),
    ] {
        let key = (name.to_string(), COMPLETION_KIND_STRUCT);
        if seen.insert(key) {
            items.push(json!({
                "label": name,
                "kind": COMPLETION_KIND_STRUCT,
                "detail": detail,
                "sortText": format!("3_{name}")
            }));
        }
    }

    Value::Array(items)
}

// ============================================================================
// Document Symbols Provider (`textDocument/documentSymbol`)
// ============================================================================

fn handle_document_symbol(state: &LspState, params: Option<&Value>) -> Value {
    let Some(uri) = params
        .and_then(|p| p.pointer("/textDocument/uri"))
        .and_then(Value::as_str)
    else {
        return Value::Array(Vec::new());
    };
    let Some(text) = state.documents.get(uri) else {
        return Value::Array(Vec::new());
    };
    let Some(prog) = program_for_uri(state, uri) else {
        return Value::Array(Vec::new());
    };

    let mut symbols = Vec::with_capacity(prog.declarations.len());
    for decl in &prog.declarations {
        match decl {
            Declaration::Struct(s) => {
                let range = span_to_lsp_range(text, s.span);
                let selection_range =
                    clamp_selection_range(span_to_lsp_range(text, s.name.span), &range);
                let children: Vec<Value> = s
                    .fields
                    .iter()
                    .enumerate()
                    .map(|(i, field)| {
                        let idx = field.index.unwrap_or(i as u32);
                        let f_range =
                            clamp_selection_range(span_to_lsp_range(text, field.span), &range);
                        let f_sel = clamp_selection_range(
                            span_to_lsp_range(text, field.name.span),
                            &f_range,
                        );
                        json!({
                            "name": field.name.name,
                            "detail": format!("{idx}: {}", format_type_expr(&field.ty)),
                            "kind": SYMBOL_KIND_FIELD,
                            "range": f_range,
                            "selectionRange": f_sel
                        })
                    })
                    .collect();

                symbols.push(json!({
                    "name": s.name.name,
                    "detail": format!("schema ({} fields)", s.fields.len()),
                    "kind": SYMBOL_KIND_STRUCT,
                    "range": range,
                    "selectionRange": selection_range,
                    "children": children
                }));
            }
            Declaration::Enum(e) => {
                let range = span_to_lsp_range(text, e.span);
                let selection_range =
                    clamp_selection_range(span_to_lsp_range(text, e.name.span), &range);
                let children: Vec<Value> = e
                    .variants
                    .iter()
                    .map(|variant| {
                        let v_range =
                            clamp_selection_range(span_to_lsp_range(text, variant.span), &range);
                        let v_sel = clamp_selection_range(
                            span_to_lsp_range(text, variant.name.span),
                            &v_range,
                        );
                        let detail = if variant.fields.is_empty() {
                            String::new()
                        } else {
                            let tys: Vec<String> =
                                variant.fields.iter().map(format_type_expr).collect();
                            format!("({})", tys.join(", "))
                        };
                        json!({
                            "name": variant.name.name,
                            "detail": detail,
                            "kind": SYMBOL_KIND_ENUM_MEMBER,
                            "range": v_range,
                            "selectionRange": v_sel
                        })
                    })
                    .collect();

                symbols.push(json!({
                    "name": e.name.name,
                    "detail": format!("enum ({} variants)", e.variants.len()),
                    "kind": SYMBOL_KIND_ENUM,
                    "range": range,
                    "selectionRange": selection_range,
                    "children": children
                }));
            }
            Declaration::Function(f) => {
                let range = span_to_lsp_range(text, f.span);
                let selection_range =
                    clamp_selection_range(span_to_lsp_range(text, f.name.span), &range);
                symbols.push(json!({
                    "name": f.name.name,
                    "detail": format_function_signature(f),
                    "kind": SYMBOL_KIND_FUNCTION,
                    "range": range,
                    "selectionRange": selection_range
                }));
            }
            Declaration::Const(c) => {
                let range = span_to_lsp_range(text, c.span);
                let selection_range =
                    clamp_selection_range(span_to_lsp_range(text, c.name.span), &range);
                symbols.push(json!({
                    "name": c.name.name,
                    "detail": format_const_def(c),
                    "kind": SYMBOL_KIND_CONSTANT,
                    "range": range,
                    "selectionRange": selection_range
                }));
            }
        }
    }

    Value::Array(symbols)
}

// ============================================================================
// Signature Help Provider (`textDocument/signatureHelp`)
// ============================================================================

#[derive(Debug, Clone)]
struct SignatureSpec {
    label: String,
    doc: String,
    params: Vec<String>,
}

#[derive(Debug, Clone)]
struct CallSiteContext {
    /// `Some("")` means method call on an arbitrary receiver (e.g. `arr.map(`);
    /// `Some("ns")` means `ns.fn(`; `None` means unqualified call `fn(`.
    qualifier: Option<String>,
    callee: String,
    active_param: usize,
}

#[derive(Debug, Clone, Copy)]
enum CallDelimiter {
    Paren { open_byte: usize, comma_count: usize },
    Bracket,
    Brace,
}

fn handle_signature_help(state: &LspState, params: Option<&Value>) -> Value {
    let Some((uri, line, character)) = extract_text_doc_position(params) else {
        return Value::Null;
    };
    let Some(text) = state.documents.get(uri) else {
        return Value::Null;
    };
    let Some(ctx) = signature_help_call_context(text, line, character) else {
        return Value::Null;
    };
    let Some(spec) = resolve_signature_help(state, uri, text, &ctx) else {
        return Value::Null;
    };

    let active_param = if spec.params.is_empty() {
        0
    } else {
        ctx.active_param.min(spec.params.len() - 1)
    };
    let lsp_params: Vec<Value> = spec
        .params
        .iter()
        .map(|p| json!({ "label": p }))
        .collect();

    json!({
        "signatures": [{
            "label": spec.label,
            "documentation": {
                "kind": "markdown",
                "value": spec.doc
            },
            "parameters": lsp_params
        }],
        "activeSignature": 0,
        "activeParameter": active_param
    })
}

/// Scans `text` up to `(line, character)` with lexical awareness (skipping comments, strings,
/// and nested `(...)`, `[...]`, `{...}`) to identify the innermost open call site and exact
/// 0-based active parameter index counted from top-level commas.
fn signature_help_call_context(text: &str, line: u32, character: u32) -> Option<CallSiteContext> {
    let line_starts = compute_line_starts(text);
    let line_start = *line_starts.get(line as usize)?;
    let line_raw_end = line_starts
        .get(line as usize + 1)
        .map(|&next| next.saturating_sub(1))
        .unwrap_or(text.len());
    let line_str = text[line_start..line_raw_end].trim_end_matches('\r');
    let col_byte = utf16_col_to_byte_idx(line_str, character);
    let cursor_byte = (line_start + col_byte).min(text.len());

    let bytes = text.as_bytes();
    let mut i = 0usize;
    let mut modes: Vec<ScanMode> = vec![ScanMode::Normal {
        brace_depth: 0,
        in_template_expr: false,
    }];
    let mut stack: Vec<CallDelimiter> = Vec::new();

    while i < cursor_byte {
        let mode = *modes.last().unwrap_or(&ScanMode::Normal {
            brace_depth: 0,
            in_template_expr: false,
        });
        match mode {
            ScanMode::TemplateText => {
                let b = bytes[i];
                if b == b'\\' {
                    i = (i + 2).min(cursor_byte);
                    continue;
                }
                if b == b'`' {
                    modes.pop();
                    i += 1;
                    continue;
                }
                if b == b'$' && i + 1 < cursor_byte && bytes[i + 1] == b'{' {
                    modes.push(ScanMode::Normal {
                        brace_depth: 0,
                        in_template_expr: true,
                    });
                    stack.push(CallDelimiter::Brace);
                    i += 2;
                    continue;
                }
                i += 1;
            }
            ScanMode::Normal {
                brace_depth,
                in_template_expr,
            } => {
                let b = bytes[i];
                // Line comment `// ...`
                if b == b'/' && i + 1 < cursor_byte && bytes[i + 1] == b'/' {
                    i += 2;
                    while i < cursor_byte && bytes[i] != b'\n' {
                        i += 1;
                    }
                    continue;
                }
                // Block comment `/* ... */`
                if b == b'/' && i + 1 < cursor_byte && bytes[i + 1] == b'*' {
                    i += 2;
                    while i + 1 < cursor_byte && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                        i += 1;
                    }
                    i = (i + 2).min(cursor_byte);
                    continue;
                }
                // Quoted string literal
                if b == b'"' || b == b'\'' {
                    let q = b;
                    i += 1;
                    while i < cursor_byte {
                        if bytes[i] == b'\\' {
                            i = (i + 2).min(cursor_byte);
                            continue;
                        }
                        if bytes[i] == q {
                            i += 1;
                            break;
                        }
                        i += 1;
                    }
                    continue;
                }
                // Template literal
                if b == b'`' {
                    modes.push(ScanMode::TemplateText);
                    i += 1;
                    continue;
                }

                match b {
                    b'(' => {
                        stack.push(CallDelimiter::Paren {
                            open_byte: i,
                            comma_count: 0,
                        });
                    }
                    b')' => {
                        if matches!(stack.last(), Some(CallDelimiter::Paren { .. })) {
                            stack.pop();
                        }
                    }
                    b'[' => stack.push(CallDelimiter::Bracket),
                    b']' => {
                        if matches!(stack.last(), Some(CallDelimiter::Bracket)) {
                            stack.pop();
                        }
                    }
                    b'{' => {
                        if in_template_expr {
                            if let Some(ScanMode::Normal { brace_depth: d, .. }) = modes.last_mut() {
                                *d += 1;
                            }
                        }
                        stack.push(CallDelimiter::Brace);
                    }
                    b'}' => {
                        if matches!(stack.last(), Some(CallDelimiter::Brace)) {
                            stack.pop();
                        }
                        if in_template_expr {
                            if brace_depth == 0 {
                                modes.pop();
                            } else if let Some(ScanMode::Normal { brace_depth: d, .. }) =
                                modes.last_mut()
                            {
                                *d -= 1;
                            }
                        }
                    }
                    b',' => {
                        if let Some(CallDelimiter::Paren { comma_count, .. }) = stack.last_mut() {
                            *comma_count += 1;
                        }
                    }
                    _ => {}
                }
                i += 1;
            }
        }
    }

    // Find the innermost open `(` that has a valid callable identifier directly before it
    for delim in stack.iter().rev() {
        let CallDelimiter::Paren {
            open_byte,
            comma_count,
        } = *delim
        else {
            continue;
        };

        let Some((qualifier, callee)) = extract_callee_before_paren(text, open_byte) else {
            continue;
        };
        return Some(CallSiteContext {
            qualifier,
            callee,
            active_param: comma_count,
        });
    }

    None
}

fn extract_callee_before_paren(text: &str, open_paren_byte: usize) -> Option<(Option<String>, String)> {
    let mut end = text.get(..open_paren_byte)?.trim_end().len();
    if end == 0 {
        return None;
    }

    // Skip optional generic type argument list `foo<i64, string>(`
    if text.as_bytes().get(end - 1) == Some(&b'>') {
        let mut depth = 0i32;
        let mut idx = end;
        let mut found_open = None;
        while idx > 0 {
            idx -= 1;
            match text.as_bytes()[idx] {
                b'>' => depth += 1,
                b'<' => {
                    depth -= 1;
                    if depth == 0 {
                        found_open = Some(idx);
                        break;
                    }
                }
                b';' | b'{' | b'}' | b'\n' => break,
                _ => {}
            }
        }
        if let Some(open_lt) = found_open {
            end = text[..open_lt].trim_end().len();
        }
    }

    let before = &text[..end];
    let mut start = end;
    for (idx, c) in before.char_indices().rev() {
        if is_ident_char(c) {
            start = idx;
        } else {
            break;
        }
    }
    if start == end {
        return None;
    }

    let callee = &before[start..end];
    if !callee
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
    {
        return None;
    }
    if matches!(
        callee,
        "if" | "while"
            | "for"
            | "switch"
            | "return"
            | "match"
            | "catch"
            | "function"
            | "fn"
            | "effects"
            | "effect"
    ) {
        return None;
    }

    let prefix_before_callee = before[..start].trim_end();
    // Skip function declaration headers (`function foo(a: i64, |)`)
    if let Some(prev_word) = trailing_identifier(prefix_before_callee) {
        if matches!(prev_word, "function" | "fn") {
            return None;
        }
    }

    if let Some(before_dot) = prefix_before_callee.strip_suffix('.') {
        let qual = trailing_identifier(before_dot.trim_end()).unwrap_or("");
        return Some((Some(qual.to_string()), callee.to_string()));
    }

    Some((None, callee.to_string()))
}

fn trailing_identifier(s: &str) -> Option<&str> {
    let trimmed = s.trim_end();
    let end = trimmed.len();
    let mut start = end;
    for (idx, c) in trimmed.char_indices().rev() {
        if is_ident_char(c) {
            start = idx;
        } else {
            break;
        }
    }
    if start < end {
        let word = &trimmed[start..end];
        if word
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        {
            return Some(word);
        }
    }
    None
}

fn signature_from_fn_def(f: &FunctionDef, origin_module: Option<&str>) -> SignatureSpec {
    let params: Vec<String> = f
        .params
        .iter()
        .map(|p| {
            let borrow = if p.is_borrowed { "readonly " } else { "" };
            format!("{}: {}{}", p.name.name, borrow, format_type_expr(&p.ty))
        })
        .collect();
    let ret = format_type_expr(&f.ret_ty);
    let ret_part = if ret == "void" {
        String::new()
    } else {
        format!(": {ret}")
    };
    let shader_prefix = if f.is_shader { "@shader " } else { "" };
    let label = format!("{shader_prefix}function {}({}){ret_part}", f.name.name, params.join(", "));
    let mut doc = if f.is_shader {
        "GPU-pure `@shader` kernel.".to_string()
    } else if !f.effects.is_empty() {
        let effs: Vec<String> = f
            .effects
            .iter()
            .map(|e| format!("`\"{}\"`", e.name.to_ascii_lowercase()))
            .collect();
        format!("Effectful function requiring `@effects({})`.", effs.join(", "))
    } else {
        "Pure statically verified function.".to_string()
    };
    if let Some(orig) = origin_module {
        let _ = write!(doc, "\n\n*Imported from `{orig}`*");
    }
    SignatureSpec { label, doc, params }
}

fn resolve_signature_help(
    state: &LspState,
    uri: &str,
    text: &str,
    ctx: &CallSiteContext,
) -> Option<SignatureSpec> {
    let prog = program_for_uri(state, uri);

    // Qualified calls: `ns.fn(...)` or `arr.map(...)`
    if let Some(ref qual) = ctx.qualifier {
        if !qual.is_empty() {
            let ns_mod = prog
                .as_ref()
                .and_then(|p| resolve_namespace_target_in_program(state, uri, p, qual))
                .or_else(|| resolve_namespace_tolerant(state, uri, text, qual));
            if let Some(ns) = ns_mod {
                if let Some(f) = find_function_by_name(&ns.prog, &ctx.callee) {
                    return Some(signature_from_fn_def(f, Some(&ns.import_spec)));
                }
            }
        }
        if let Some(method) = ARRAY_METHODS
            .iter()
            .find(|m| !m.is_property && m.name == ctx.callee)
        {
            return Some(SignatureSpec {
                label: format!("Array<T>.{}", method.signature),
                doc: method.doc.to_string(),
                params: method.params.iter().map(|s| (*s).to_string()).collect(),
            });
        }
        return None;
    }

    // Resolve `import { orig as alias } from "..."`
    let resolved_name = prog
        .as_ref()
        .map(|p| resolve_alias_in_program(p, &ctx.callee))
        .unwrap_or_else(|| resolve_alias_tolerant(text, &ctx.callee));

    // 1. Current document declarations (with tolerant fallback when mid-call doesn't parse cleanly)
    if let Some(ref p) = prog {
        if let Some(f) = find_function_by_name(p, &resolved_name) {
            return Some(signature_from_fn_def(f, None));
        }
    }
    if let Some(sig) = find_local_function_signature_tolerant(text, &resolved_name) {
        return Some(sig);
    }

    // 2. Cross-file imported modules (local, knr.toml packages, and `std/*` stdlib)
    let mut imported = match prog.as_ref() {
        Some(p) => load_imported_programs_recursive(state, uri, p),
        None => load_imported_programs_tolerant(state, uri, text),
    };
    match prog.as_ref() {
        Some(p) => imported.extend(load_imported_stdlib_programs(p)),
        None => imported.extend(load_imported_stdlib_programs_tolerant(text)),
    }
    for imp in &imported {
        if let Some(f) = find_function_by_name(&imp.prog, &resolved_name) {
            return Some(signature_from_fn_def(f, Some(&imp.import_spec)));
        }
    }

    // 3. Built-in intrinsics (`sin`, `clamp`, `lerp`, `smoothstep`, `vec_push`, `map_set`, `syscall`, ...)
    lookup_builtin_signature(&resolved_name)
}

fn resolve_alias_tolerant(text: &str, ident: &str) -> String {
    for raw in text.lines() {
        let trimmed = raw.trim();
        if !(trimmed.starts_with("import ") || trimmed.contains(" as ")) {
            continue;
        }
        let tokens: Vec<&str> = trimmed
            .split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
            .filter(|s| !s.is_empty())
            .collect();
        for w in tokens.windows(3) {
            if w[1] == "as" && w[2] == ident {
                return w[0].to_string();
            }
        }
    }
    ident.to_string()
}

fn resolve_namespace_tolerant(
    state: &LspState,
    uri: &str,
    text: &str,
    qualifier: &str,
) -> Option<ImportedModule> {
    let needle = format!("* as {qualifier}");
    for raw in text.lines() {
        let trimmed = raw.trim();
        if !trimmed.starts_with("import ") || !trimmed.contains(&needle) {
            continue;
        }
        let Some(from_pos) = trimmed.find(" from ") else {
            continue;
        };
        let after = trimmed[from_pos + 6..].trim_start();
        let Some(&q) = after.as_bytes().first() else {
            continue;
        };
        if q != b'"' && q != b'\'' {
            continue;
        }
        let body = &after[1..];
        let Some(end_q) = body.find(q as char) else {
            continue;
        };
        let spec = &body[..end_q];
        if let Some(std_src) = knr_core::syntax::stdlib::resolve_builtin_std(spec) {
            let std_prog = knr_core::syntax::parse_tsx(std_src).ok()?;
            return Some(ImportedModule {
                uri: format!("knr-std://{spec}"),
                import_spec: spec.to_string(),
                text: std_src.to_string(),
                prog: std_prog,
            });
        }
        if let Some((imp_uri, imp_text)) = resolve_local_import_targets(state, uri, spec) {
            if let Ok(imp_prog) = knr_core::syntax::parse_tsx(&imp_text) {
                return Some(ImportedModule {
                    uri: imp_uri,
                    import_spec: spec.to_string(),
                    text: imp_text,
                    prog: imp_prog,
                });
            }
        }
    }
    None
}

fn find_local_function_signature_tolerant(text: &str, callee: &str) -> Option<SignatureSpec> {
    let needle = format!("function {callee}(");
    let mut search_from = 0usize;
    while let Some(rel) = text[search_from..].find(&needle) {
        let start = search_from + rel;
        let before_ok = start == 0
            || !text[..start]
                .chars()
                .next_back()
                .is_some_and(is_ident_char);
        if before_ok {
            let open_paren = start + needle.len() - 1;
            let rest = &text[open_paren + 1..];
            if let Some(close_rel) = rest.find(')') {
                let raw_params = &rest[..close_rel];
                let params: Vec<String> = raw_params
                    .split(',')
                    .map(|p| p.split_whitespace().collect::<Vec<_>>().join(" "))
                    .filter(|p| !p.is_empty())
                    .collect();
                let after_close = rest[close_rel + 1..].trim_start();
                let ret_part = if let Some(after_colon) = after_close.strip_prefix(':') {
                    let ret_ty = after_colon
                        .split(['{', '\n'])
                        .next()
                        .unwrap_or("")
                        .trim();
                    if ret_ty.is_empty() || ret_ty == "void" {
                        String::new()
                    } else {
                        format!(": {ret_ty}")
                    }
                } else {
                    String::new()
                };
                return Some(SignatureSpec {
                    label: format!("function {callee}({}){ret_part}", params.join(", ")),
                    doc: "Local statically verified function.".to_string(),
                    params,
                });
            }
        }
        search_from = start + needle.len();
    }
    None
}

struct BuiltinSigSpec {
    name: &'static str,
    label: &'static str,
    params: &'static [&'static str],
    ret_ty: Option<&'static str>,
    doc: &'static str,
}

const BUILTIN_SIGNATURES: &[BuiltinSigSpec] = &[
    BuiltinSigSpec {
        name: "sin",
        label: "function sin(x: f64): f64",
        params: &["x: f64"],
        ret_ty: Some("f64"),
        doc: "Sine of `x` in radians (CPU + GPU `@shader`).",
    },
    BuiltinSigSpec {
        name: "cos",
        label: "function cos(x: f64): f64",
        params: &["x: f64"],
        ret_ty: Some("f64"),
        doc: "Cosine of `x` in radians (CPU + GPU `@shader`).",
    },
    BuiltinSigSpec {
        name: "tan",
        label: "function tan(x: f64): f64",
        params: &["x: f64"],
        ret_ty: Some("f64"),
        doc: "Tangent of `x` in radians (CPU + GPU `@shader`).",
    },
    BuiltinSigSpec {
        name: "sqrt",
        label: "function sqrt(x: f64): f64",
        params: &["x: f64"],
        ret_ty: Some("f64"),
        doc: "Square root of `x` (CPU + GPU `@shader`).",
    },
    BuiltinSigSpec {
        name: "abs",
        label: "function abs(x: f64): f64",
        params: &["x: f64"],
        ret_ty: Some("f64"),
        doc: "Absolute value of `x` (CPU + GPU `@shader`).",
    },
    BuiltinSigSpec {
        name: "floor",
        label: "function floor(x: f64): f64",
        params: &["x: f64"],
        ret_ty: Some("f64"),
        doc: "Largest integer less than or equal to `x` (CPU + GPU `@shader`).",
    },
    BuiltinSigSpec {
        name: "ceil",
        label: "function ceil(x: f64): f64",
        params: &["x: f64"],
        ret_ty: Some("f64"),
        doc: "Smallest integer greater than or equal to `x` (CPU + GPU `@shader`).",
    },
    BuiltinSigSpec {
        name: "fract",
        label: "function fract(x: f64): f64",
        params: &["x: f64"],
        ret_ty: Some("f64"),
        doc: "Fractional part `x - floor(x)` (CPU + GPU `@shader`).",
    },
    BuiltinSigSpec {
        name: "pow",
        label: "function pow(base: f64, exp: f64): f64",
        params: &["base: f64", "exp: f64"],
        ret_ty: Some("f64"),
        doc: "Raises `base` to power `exp` (CPU + GPU `@shader`).",
    },
    BuiltinSigSpec {
        name: "atan2",
        label: "function atan2(y: f64, x: f64): f64",
        params: &["y: f64", "x: f64"],
        ret_ty: Some("f64"),
        doc: "Four-quadrant arctangent of `y / x` in radians (CPU + GPU `@shader`).",
    },
    BuiltinSigSpec {
        name: "min",
        label: "function min(a: f64, b: f64): f64",
        params: &["a: f64", "b: f64"],
        ret_ty: Some("f64"),
        doc: "Minimum of `a` and `b` (CPU + GPU `@shader`).",
    },
    BuiltinSigSpec {
        name: "max",
        label: "function max(a: f64, b: f64): f64",
        params: &["a: f64", "b: f64"],
        ret_ty: Some("f64"),
        doc: "Maximum of `a` and `b` (CPU + GPU `@shader`).",
    },
    BuiltinSigSpec {
        name: "clamp",
        label: "function clamp(x: f64, lo: f64, hi: f64): f64",
        params: &["x: f64", "lo: f64", "hi: f64"],
        ret_ty: Some("f64"),
        doc: "Constrains `x` to the closed interval `[lo, hi]` (CPU + GPU `@shader`).",
    },
    BuiltinSigSpec {
        name: "lerp",
        label: "function lerp(a: f64, b: f64, t: f64): f64",
        params: &["a: f64", "b: f64", "t: f64"],
        ret_ty: Some("f64"),
        doc: "Linear interpolation `a + (b - a) * t` (CPU + GPU `@shader`).",
    },
    BuiltinSigSpec {
        name: "mix",
        label: "function mix(a: f64, b: f64, t: f64): f64",
        params: &["a: f64", "b: f64", "t: f64"],
        ret_ty: Some("f64"),
        doc: "Linear interpolation alias for `lerp(a, b, t)` (CPU + GPU `@shader`).",
    },
    BuiltinSigSpec {
        name: "smoothstep",
        label: "function smoothstep(edge0: f64, edge1: f64, x: f64): f64",
        params: &["edge0: f64", "edge1: f64", "x: f64"],
        ret_ty: Some("f64"),
        doc: "Smooth Hermite interpolation from `0.0` to `1.0` across `[edge0, edge1]` (CPU + GPU `@shader`).",
    },
    BuiltinSigSpec {
        name: "to_f64",
        label: "function to_f64(x: i64): f64",
        params: &["x: i64"],
        ret_ty: Some("f64"),
        doc: "Converts an `i64` integer to `f64`.",
    },
    BuiltinSigSpec {
        name: "toFloat",
        label: "function toFloat(x: i64): f64",
        params: &["x: i64"],
        ret_ty: Some("f64"),
        doc: "Converts an `i64` integer to `f64`.",
    },
    BuiltinSigSpec {
        name: "to_i64",
        label: "function to_i64(x: f64): i64",
        params: &["x: f64"],
        ret_ty: Some("i64"),
        doc: "Truncates an `f64` floating-point number to `i64`.",
    },
    BuiltinSigSpec {
        name: "toInt",
        label: "function toInt(x: f64): i64",
        params: &["x: f64"],
        ret_ty: Some("i64"),
        doc: "Truncates an `f64` floating-point number to `i64`.",
    },
    BuiltinSigSpec {
        name: "to_str",
        label: "function to_str(x: i64 | f64 | bool | string): string",
        params: &["x: i64 | f64 | bool | string"],
        ret_ty: Some("string"),
        doc: "Formats a primitive value as a UTF-8 string.",
    },
    BuiltinSigSpec {
        name: "toString",
        label: "function toString(x: i64 | f64 | bool | string): string",
        params: &["x: i64 | f64 | bool | string"],
        ret_ty: Some("string"),
        doc: "Formats a primitive value as a UTF-8 string.",
    },
    BuiltinSigSpec {
        name: "str_len",
        label: "function str_len(s: string): i64",
        params: &["s: string"],
        ret_ty: Some("i64"),
        doc: "Returns the byte length of string `s`.",
    },
    BuiltinSigSpec {
        name: "vec_new",
        label: "function vec_new(): T[]",
        params: &[],
        ret_ty: None,
        doc: "Allocates an empty linear vector.",
    },
    BuiltinSigSpec {
        name: "vec_push",
        label: "function vec_push(v: T[], item: T): T[]",
        params: &["v: T[]", "item: T"],
        ret_ty: None,
        doc: "Consumes linear vector `v`, appends `item`, and returns the updated vector.",
    },
    BuiltinSigSpec {
        name: "vec_get",
        label: "function vec_get(v: readonly T[], index: i64): T",
        params: &["v: readonly T[]", "index: i64"],
        ret_ty: None,
        doc: "Reads the element at `index` from vector `v` without consuming `v`.",
    },
    BuiltinSigSpec {
        name: "vec_len",
        label: "function vec_len(v: readonly T[]): i64",
        params: &["v: readonly T[]"],
        ret_ty: Some("i64"),
        doc: "Returns the number of elements in vector `v`.",
    },
    BuiltinSigSpec {
        name: "map_new",
        label: "function map_new(): Map<K, V>",
        params: &[],
        ret_ty: None,
        doc: "Allocates an empty linear hash map.",
    },
    BuiltinSigSpec {
        name: "map_set",
        label: "function map_set(m: Map<K, V>, key: K, value: V): Map<K, V>",
        params: &["m: Map<K, V>", "key: K", "value: V"],
        ret_ty: None,
        doc: "Consumes `m`, inserts `(key, value)`, and returns the updated map.",
    },
    BuiltinSigSpec {
        name: "map_get",
        label: "function map_get(m: readonly Map<K, V>, key: K): V",
        params: &["m: readonly Map<K, V>", "key: K"],
        ret_ty: None,
        doc: "Looks up `key` in map `m`.",
    },
    BuiltinSigSpec {
        name: "buf_alloc",
        label: "function buf_alloc(size: i64): buf",
        params: &["size: i64"],
        ret_ty: Some("buf"),
        doc: "Allocates a linear heap byte buffer of `size` bytes.",
    },
    BuiltinSigSpec {
        name: "buf_read",
        label: "function buf_read(b: readonly buf, offset: i64): i64",
        params: &["b: readonly buf", "offset: i64"],
        ret_ty: Some("i64"),
        doc: "Reads an unsigned byte (`0..=255`) from buffer `b` at `offset`.",
    },
    BuiltinSigSpec {
        name: "buf_write",
        label: "function buf_write(b: buf, offset: i64, value: i64): buf",
        params: &["b: buf", "offset: i64", "value: i64"],
        ret_ty: Some("buf"),
        doc: "Consumes `b`, writes byte `value` at `offset`, and returns the updated buffer.",
    },
    BuiltinSigSpec {
        name: "chan_new",
        label: "function chan_new(capacity: i64): Channel<T>",
        params: &["capacity: i64"],
        ret_ty: None,
        doc: "Allocates a bounded linear channel with `capacity` slots (`@effects(\"async\")`).",
    },
    BuiltinSigSpec {
        name: "chan_send",
        label: "function chan_send(ch: readonly Channel<T>, value: T): bool",
        params: &["ch: readonly Channel<T>", "value: T"],
        ret_ty: Some("bool"),
        doc: "Sends `value` on channel `ch`.",
    },
    BuiltinSigSpec {
        name: "chan_recv",
        label: "function chan_recv(ch: readonly Channel<T>): T",
        params: &["ch: readonly Channel<T>"],
        ret_ty: None,
        doc: "Receives the next value from channel `ch`.",
    },
    BuiltinSigSpec {
        name: "free",
        label: "function free(resource: T)",
        params: &["resource: T"],
        ret_ty: None,
        doc: "Explicitly consumes and releases a linear resource.",
    },
    BuiltinSigSpec {
        name: "syscall",
        label: "syscall(effect: \"net\" | \"ui\" | \"hw\" | \"async\", op: string, payload: T): R",
        params: &[
            "effect: \"net\" | \"ui\" | \"hw\" | \"async\"",
            "op: string",
            "payload: T",
        ],
        ret_ty: None,
        doc: "Invokes a capability-checked host runtime syscall. Enclosing function must declare `@effects(...)`.",
    },
];

fn lookup_builtin_signature(name: &str) -> Option<SignatureSpec> {
    BUILTIN_SIGNATURES.iter().find(|b| b.name == name).map(|b| SignatureSpec {
        label: b.label.to_string(),
        doc: b.doc.to_string(),
        params: b.params.iter().map(|p| (*p).to_string()).collect(),
    })
}

// ============================================================================
// Inlay Hints Provider (`textDocument/inlayHint`)
// ============================================================================

#[derive(Debug, Clone)]
struct InlayFnSig {
    param_names: Vec<String>,
    ret_ty: Option<String>,
}

#[derive(Debug, Default)]
struct InlayTypeEnv {
    fn_sigs: HashMap<String, InlayFnSig>,
    ns_fn_sigs: HashMap<(String, String), InlayFnSig>,
    struct_fields: HashMap<String, HashMap<String, String>>,
    const_types: HashMap<String, String>,
}

impl InlayTypeEnv {
    fn register_program(&mut self, prog: &Program, ns_prefix: Option<&str>) {
        for decl in &prog.declarations {
            match decl {
                Declaration::Function(f) => {
                    let ret = format_type_expr(&f.ret_ty);
                    let sig = InlayFnSig {
                        param_names: f.params.iter().map(|p| p.name.name.clone()).collect(),
                        ret_ty: if ret == "void" { None } else { Some(ret) },
                    };
                    if let Some(ns) = ns_prefix {
                        self.ns_fn_sigs
                            .insert((ns.to_string(), f.name.name.clone()), sig);
                    } else {
                        self.fn_sigs.insert(f.name.name.clone(), sig);
                    }
                }
                Declaration::Struct(s) => {
                    let entry = self
                        .struct_fields
                        .entry(s.name.name.clone())
                        .or_default();
                    for field in &s.fields {
                        entry.insert(field.name.name.clone(), format_type_expr(&field.ty));
                    }
                }
                Declaration::Const(c) => {
                    if let Some(ref ty) = c.ty {
                        self.const_types
                            .insert(c.name.name.clone(), format_type_expr(ty));
                    }
                }
                Declaration::Enum(_) => {}
            }
        }
    }
}

#[derive(Debug, Clone)]
struct RawInlayHint {
    line: u32,
    character: u32,
    label: String,
    kind: u32,
    padding_right: bool,
}

fn handle_inlay_hint(state: &LspState, params: Option<&Value>) -> Value {
    let Some(params) = params else {
        return Value::Array(Vec::new());
    };
    let Some(uri) = params.pointer("/textDocument/uri").and_then(Value::as_str) else {
        return Value::Array(Vec::new());
    };
    let Some(text) = state.documents.get(uri) else {
        return Value::Array(Vec::new());
    };
    let Some(prog) = program_for_uri(state, uri) else {
        return Value::Array(Vec::new());
    };

    let req_bounds = params.get("range").map(extract_range_bounds);

    let mut env = InlayTypeEnv::default();
    // 1. Seed built-in intrinsics
    for b in BUILTIN_SIGNATURES {
        let param_names: Vec<String> = b
            .params
            .iter()
            .filter_map(|p| p.split(':').next().map(|s| s.trim().to_string()))
            .collect();
        env.fn_sigs.insert(
            b.name.to_string(),
            InlayFnSig {
                param_names,
                ret_ty: b.ret_ty.map(str::to_string),
            },
        );
    }
    // 2. Seed imported local, package, and `std/*` modules
    let mut imported = load_imported_programs_recursive(state, uri, &prog);
    imported.extend(load_imported_stdlib_programs(&prog));
    for imp in &imported {
        env.register_program(&imp.prog, None);
    }
    // 3. Register `as` aliases & `* as ns` namespace imports
    for imp in &prog.imports {
        match &imp.kind {
            ImportKind::Named(items) => {
                for (orig, alias) in items {
                    if let Some(al) = alias {
                        if let Some(sig) = env.fn_sigs.get(&orig.name).cloned() {
                            env.fn_sigs.insert(al.name.clone(), sig);
                        }
                    }
                }
            }
            ImportKind::Default { named, .. } => {
                for (orig, alias) in named {
                    if let Some(al) = alias {
                        if let Some(sig) = env.fn_sigs.get(&orig.name).cloned() {
                            env.fn_sigs.insert(al.name.clone(), sig);
                        }
                    }
                }
            }
            ImportKind::Namespace(ns_alias) => {
                if let Some(ns_mod) =
                    resolve_namespace_target_in_program(state, uri, &prog, &ns_alias.name)
                {
                    env.register_program(&ns_mod.prog, Some(&ns_alias.name));
                }
            }
            ImportKind::SideEffect => {}
        }
    }
    // 4. Register current document declarations (highest precedence)
    env.register_program(&prog, None);

    let mut raw_hints: Vec<RawInlayHint> = Vec::new();
    for decl in &prog.declarations {
        if let Declaration::Function(f) = decl {
            let mut locals: HashMap<String, String> = HashMap::new();
            for p in &f.params {
                locals.insert(p.name.name.clone(), format_type_expr(&p.ty));
            }
            collect_inlay_hints_in_block(text, &f.body, &env, &mut locals, &mut raw_hints);
        }
    }

    // Filter to requested range, sort deterministically, and deduplicate
    raw_hints.retain(|h| match req_bounds {
        Some((start, end)) => (h.line, h.character) >= start && (h.line, h.character) <= end,
        None => true,
    });
    raw_hints.sort_by(|a, b| {
        a.line
            .cmp(&b.line)
            .then_with(|| a.character.cmp(&b.character))
            .then_with(|| a.kind.cmp(&b.kind))
            .then_with(|| a.label.cmp(&b.label))
    });
    raw_hints.dedup_by(|a, b| a.line == b.line && a.character == b.character && a.label == b.label);

    let lsp_hints: Vec<Value> = raw_hints
        .into_iter()
        .map(|h| {
            json!({
                "position": { "line": h.line, "character": h.character },
                "label": h.label,
                "kind": h.kind,
                "paddingLeft": false,
                "paddingRight": h.padding_right
            })
        })
        .collect();
    Value::Array(lsp_hints)
}

fn collect_inlay_hints_in_block(
    text: &str,
    block: &Block,
    env: &InlayTypeEnv,
    locals: &mut HashMap<String, String>,
    out: &mut Vec<RawInlayHint>,
) {
    for stmt in &block.stmts {
        collect_inlay_hints_in_stmt(text, stmt, env, locals, out);
    }
    if let Some(ref tail) = block.tail_expr {
        collect_inlay_hints_in_expr(text, tail, env, locals, out);
    }
}

fn collect_inlay_hints_in_stmt(
    text: &str,
    stmt: &Stmt,
    env: &InlayTypeEnv,
    locals: &mut HashMap<String, String>,
    out: &mut Vec<RawInlayHint>,
) {
    match &stmt.kind {
        StmtKind::Let {
            name,
            ty,
            init,
            destruct,
            ..
        } => {
            collect_inlay_hints_in_expr(text, init, env, locals, out);
            let inferred = infer_expr_type(init, locals, env);
            if let Some(pat) = destruct {
                let src_ty = ty
                    .as_ref()
                    .map(format_type_expr)
                    .or(inferred);
                match pat {
                    DestructPattern::Object(fields) => {
                        if let Some(ref struct_name) = src_ty {
                            if let Some(field_map) = env.struct_fields.get(struct_name) {
                                for (f_name, alias) in fields {
                                    let local_name = alias.as_ref().unwrap_or(f_name);
                                    if let Some(f_ty) = field_map.get(&f_name.name) {
                                        locals.insert(local_name.name.clone(), f_ty.clone());
                                    }
                                }
                            }
                        }
                    }
                    DestructPattern::Array(elems) => {
                        if let Some(elem_ty) = src_ty.as_deref().and_then(|t| t.strip_suffix("[]")) {
                            for el in elems {
                                locals.insert(el.name.clone(), elem_ty.to_string());
                            }
                        }
                    }
                }
            } else {
                if ty.is_none() && name.name != "_" {
                    if let Some(ref inferred_ty) = inferred {
                        if name.span.end <= text.len() && name.span.end > 0 {
                            let (line, character) = byte_offset_to_lsp_pos(text, name.span.end);
                            out.push(RawInlayHint {
                                line,
                                character,
                                label: format!(": {inferred_ty}"),
                                kind: INLAY_HINT_KIND_TYPE,
                                padding_right: false,
                            });
                        }
                    }
                }
                if let Some(final_ty) = ty.as_ref().map(format_type_expr).or(inferred) {
                    locals.insert(name.name.clone(), final_ty);
                }
            }
        }
        StmtKind::Assign { target, value } => {
            collect_inlay_hints_in_expr(text, target, env, locals, out);
            collect_inlay_hints_in_expr(text, value, env, locals, out);
        }
        StmtKind::Return { value: Some(expr) }
        | StmtKind::Expr(expr)
        | StmtKind::Syscall { payload: expr, .. } => {
            collect_inlay_hints_in_expr(text, expr, env, locals, out);
        }
        StmtKind::If {
            cond,
            then_block,
            else_block,
        } => {
            collect_inlay_hints_in_expr(text, cond, env, locals, out);
            let mut then_locals = locals.clone();
            collect_inlay_hints_in_block(text, then_block, env, &mut then_locals, out);
            if let Some(eb) = else_block {
                let mut else_locals = locals.clone();
                collect_inlay_hints_in_block(text, eb, env, &mut else_locals, out);
            }
        }
        StmtKind::While { cond, body } => {
            collect_inlay_hints_in_expr(text, cond, env, locals, out);
            let mut body_locals = locals.clone();
            collect_inlay_hints_in_block(text, body, env, &mut body_locals, out);
        }
        StmtKind::Block(inner) => {
            let mut inner_locals = locals.clone();
            collect_inlay_hints_in_block(text, inner, env, &mut inner_locals, out);
        }
        StmtKind::Return { value: None } | StmtKind::Break | StmtKind::Continue => {}
    }
}

fn arg_matches_param_name(arg: &Expr, param_name: &str) -> bool {
    match &arg.kind {
        ExprKind::Variable(v) => v.name == param_name,
        ExprKind::Borrow(inner) => arg_matches_param_name(inner, param_name),
        _ => false,
    }
}

fn emit_call_param_hints(
    text: &str,
    sig: &InlayFnSig,
    args: &[Expr],
    out: &mut Vec<RawInlayHint>,
) {
    if args.len() < 2 {
        return;
    }
    for (arg, param_name) in args.iter().zip(sig.param_names.iter()) {
        if arg_matches_param_name(arg, param_name) {
            continue;
        }
        if arg.span.start <= text.len() && (arg.span.start > 0 || arg.span.end > 0) {
            let (line, character) = byte_offset_to_lsp_pos(text, arg.span.start);
            out.push(RawInlayHint {
                line,
                character,
                label: format!("{param_name}:"),
                kind: INLAY_HINT_KIND_PARAMETER,
                padding_right: true,
            });
        }
    }
}

fn collect_inlay_hints_in_expr(
    text: &str,
    expr: &Expr,
    env: &InlayTypeEnv,
    locals: &HashMap<String, String>,
    out: &mut Vec<RawInlayHint>,
) {
    match &expr.kind {
        ExprKind::Call { callee, args, .. } => {
            if let Some(sig) = env.fn_sigs.get(&callee.name) {
                emit_call_param_hints(text, sig, args, out);
            }
            for arg in args {
                collect_inlay_hints_in_expr(text, arg, env, locals, out);
            }
        }
        ExprKind::MethodCall {
            receiver,
            method,
            args,
        } => {
            if let ExprKind::Variable(ns) = &receiver.kind {
                if let Some(sig) = env.ns_fn_sigs.get(&(ns.name.clone(), method.name.clone())) {
                    emit_call_param_hints(text, sig, args, out);
                }
            }
            collect_inlay_hints_in_expr(text, receiver, env, locals, out);
            for arg in args {
                collect_inlay_hints_in_expr(text, arg, env, locals, out);
            }
        }
        ExprKind::EnumLit { args, .. } | ExprKind::ArrayLit(args) => {
            for arg in args {
                collect_inlay_hints_in_expr(text, arg, env, locals, out);
            }
        }
        ExprKind::StructLit { fields, spread, .. } => {
            for f in fields {
                collect_inlay_hints_in_expr(text, &f.value, env, locals, out);
            }
            if let Some(ref sp) = spread {
                collect_inlay_hints_in_expr(text, sp, env, locals, out);
            }
        }
        ExprKind::Binary { lhs, rhs, .. } => {
            collect_inlay_hints_in_expr(text, lhs, env, locals, out);
            collect_inlay_hints_in_expr(text, rhs, env, locals, out);
        }
        ExprKind::Unary { operand, .. }
        | ExprKind::Borrow(operand)
        | ExprKind::FieldAccess {
            target: operand, ..
        }
        | ExprKind::Syscall {
            payload: operand, ..
        } => {
            collect_inlay_hints_in_expr(text, operand, env, locals, out);
        }
        ExprKind::Index { target, index } => {
            collect_inlay_hints_in_expr(text, target, env, locals, out);
            collect_inlay_hints_in_expr(text, index, env, locals, out);
        }
        ExprKind::TemplateStr(parts) => {
            for part in parts {
                if let TemplatePart::Expr(e) = part {
                    collect_inlay_hints_in_expr(text, e, env, locals, out);
                }
            }
        }
        ExprKind::Jsx {
            props, children, ..
        } => {
            for prop in props {
                if let Some(ref val) = prop.value {
                    collect_inlay_hints_in_expr(text, val, env, locals, out);
                }
            }
            for child in children {
                if let JsxChild::Expr(e) | JsxChild::Element(e) = child {
                    collect_inlay_hints_in_expr(text, e, env, locals, out);
                }
            }
        }
        ExprKind::If {
            cond,
            then_branch,
            else_branch,
        } => {
            collect_inlay_hints_in_expr(text, cond, env, locals, out);
            let mut then_locals = locals.clone();
            collect_inlay_hints_in_block(text, then_branch, env, &mut then_locals, out);
            let mut else_locals = locals.clone();
            collect_inlay_hints_in_block(text, else_branch, env, &mut else_locals, out);
        }
        ExprKind::Match { scrutinee, arms } => {
            collect_inlay_hints_in_expr(text, scrutinee, env, locals, out);
            for arm in arms {
                collect_inlay_hints_in_expr(text, &arm.body, env, locals, out);
            }
        }
        ExprKind::Block(blk) => {
            let mut blk_locals = locals.clone();
            collect_inlay_hints_in_block(text, blk, env, &mut blk_locals, out);
        }
        ExprKind::Arrow { body, .. } => match body {
            ArrowBody::Expr(e) => collect_inlay_hints_in_expr(text, e, env, locals, out),
            ArrowBody::Block(blk) => {
                let mut blk_locals = locals.clone();
                collect_inlay_hints_in_block(text, blk, env, &mut blk_locals, out);
            }
        },
        ExprKind::Int(_)
        | ExprKind::Float(_)
        | ExprKind::Bool(_)
        | ExprKind::Str(_)
        | ExprKind::ColorHex(_)
        | ExprKind::Variable(_) => {}
    }
}

fn infer_expr_type(
    expr: &Expr,
    locals: &HashMap<String, String>,
    env: &InlayTypeEnv,
) -> Option<String> {
    match &expr.kind {
        ExprKind::Int(_) => Some("i64".to_string()),
        ExprKind::Float(_) => Some("f64".to_string()),
        ExprKind::Bool(_) => Some("bool".to_string()),
        ExprKind::Str(_) | ExprKind::ColorHex(_) | ExprKind::TemplateStr(_) => {
            Some("string".to_string())
        }
        ExprKind::Jsx { .. } => Some("Widget".to_string()),
        ExprKind::Variable(v) => locals
            .get(&v.name)
            .or_else(|| env.const_types.get(&v.name))
            .cloned(),
        ExprKind::Borrow(inner) => infer_expr_type(inner, locals, env),
        ExprKind::Call { callee, .. } => env.fn_sigs.get(&callee.name).and_then(|s| s.ret_ty.clone()),
        ExprKind::MethodCall {
            receiver,
            method,
            ..
        } => {
            if let ExprKind::Variable(ns) = &receiver.kind {
                if let Some(sig) = env.ns_fn_sigs.get(&(ns.name.clone(), method.name.clone())) {
                    return sig.ret_ty.clone();
                }
            }
            match method.name.as_str() {
                "some" | "every" | "includes" => Some("bool".to_string()),
                "count" | "findIndex" | "indexOf" => Some("i64".to_string()),
                "filter" | "slice" | "concat" | "reverse" | "push" => {
                    infer_expr_type(receiver, locals, env)
                }
                _ => None,
            }
        }
        ExprKind::StructLit { name, spread, .. } => {
            if let Some(n) = name {
                Some(n.name.clone())
            } else if let Some(ref sp) = spread {
                infer_expr_type(sp, locals, env)
            } else {
                None
            }
        }
        ExprKind::EnumLit {
            enum_name,
            type_args,
            ..
        } => {
            if type_args.is_empty() {
                Some(enum_name.name.clone())
            } else {
                let args: Vec<String> = type_args.iter().map(format_type_expr).collect();
                Some(format!("{}<{}>", enum_name.name, args.join(", ")))
            }
        }
        ExprKind::FieldAccess { target, field } => {
            if field.name == "length" {
                return Some("i64".to_string());
            }
            let target_ty = infer_expr_type(target, locals, env)?;
            env.struct_fields
                .get(&target_ty)
                .and_then(|fields| fields.get(&field.name))
                .cloned()
        }
        ExprKind::Index { target, .. } => {
            let target_ty = infer_expr_type(target, locals, env)?;
            target_ty.strip_suffix("[]").map(str::to_string)
        }
        ExprKind::ArrayLit(elems) => {
            let first = elems.first()?;
            let elem_ty = infer_expr_type(first, locals, env)?;
            Some(format!("{elem_ty}[]"))
        }
        ExprKind::Unary { op, operand } => match op {
            UnaryOp::Not => Some("bool".to_string()),
            UnaryOp::Neg => infer_expr_type(operand, locals, env),
        },
        ExprKind::Binary { op, lhs, rhs } => match op {
            BinaryOp::Eq
            | BinaryOp::NotEq
            | BinaryOp::Lt
            | BinaryOp::LtEq
            | BinaryOp::Gt
            | BinaryOp::GtEq
            | BinaryOp::And
            | BinaryOp::Or => Some("bool".to_string()),
            BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul | BinaryOp::Div | BinaryOp::Rem => {
                infer_expr_type(lhs, locals, env).or_else(|| infer_expr_type(rhs, locals, env))
            }
            BinaryOp::NullishCoalesce => infer_expr_type(rhs, locals, env),
        },
        ExprKind::If {
            then_branch,
            else_branch,
            ..
        } => then_branch
            .tail_expr
            .as_deref()
            .and_then(|e| infer_expr_type(e, locals, env))
            .or_else(|| {
                else_branch
                    .tail_expr
                    .as_deref()
                    .and_then(|e| infer_expr_type(e, locals, env))
            }),
        ExprKind::Block(blk) => blk
            .tail_expr
            .as_deref()
            .and_then(|e| infer_expr_type(e, locals, env)),
        ExprKind::Match { arms, .. } => arms
            .first()
            .and_then(|a| infer_expr_type(&a.body, locals, env)),
        ExprKind::Arrow { .. } | ExprKind::Syscall { .. } => None,
    }
}

// ============================================================================
// Workspace Symbol Provider (`workspace/symbol`)
// ============================================================================

fn handle_workspace_symbol(state: &LspState, params: Option<&Value>) -> Value {
    let query = params
        .and_then(|p| p.get("query"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();

    let mut files: BTreeMap<PathBuf, (String, String)> = BTreeMap::new();
    let mut non_file_docs: Vec<(String, String)> = Vec::new();

    // 1. All open documents in `state.documents` take precedence over disk copies
    for (doc_uri, doc_text) in &state.documents {
        if is_ssa_document(doc_uri) {
            continue;
        }
        if let Some(p) = uri_to_path(doc_uri) {
            files.insert(normalize_path(&p), (doc_uri.clone(), doc_text.clone()));
        } else {
            non_file_docs.push((doc_uri.clone(), doc_text.clone()));
        }
    }

    // 2. Discover project `.tsk`/`.knr` files around every open document & initialized workspace roots
    let mut disk_candidates: Vec<PathBuf> = Vec::new();
    for doc_uri in state.documents.keys() {
        disk_candidates.extend(discover_project_source_files(doc_uri));
    }
    for root in &state.workspace_roots {
        collect_source_files_bounded(root, 0, 4, 128, &mut disk_candidates);
    }
    disk_candidates.sort();
    disk_candidates.dedup();

    for disk_path in disk_candidates {
        let norm = normalize_path(&disk_path);
        if files.contains_key(&norm) {
            continue;
        }
        if let Ok(disk_text) = std::fs::read_to_string(&norm) {
            let disk_uri = path_to_uri(&norm);
            files.insert(norm, (disk_uri, disk_text));
        }
    }

    let mut matches: Vec<(u8, String, String, Value)> = Vec::new();

    let mut visit_doc = |doc_uri: &str, doc_text: &str| {
        let Ok(prog) = knr_core::syntax::parse_tsx(doc_text) else {
            return;
        };
        let container_name = uri_to_path(doc_uri)
            .and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
            .unwrap_or_default();

        for decl in &prog.declarations {
            let (name, kind, span) = match decl {
                Declaration::Function(f) => (&f.name.name, SYMBOL_KIND_FUNCTION, f.name.span),
                Declaration::Struct(s) => (&s.name.name, SYMBOL_KIND_STRUCT, s.name.span),
                Declaration::Enum(e) => (&e.name.name, SYMBOL_KIND_ENUM, e.name.span),
                Declaration::Const(c) => (&c.name.name, SYMBOL_KIND_CONSTANT, c.name.span),
            };
            let Some(rank) = workspace_symbol_match_rank(query, name) else {
                continue;
            };
            let range = span_to_lsp_range(doc_text, span);
            matches.push((
                rank,
                name.to_ascii_lowercase(),
                doc_uri.to_string(),
                json!({
                    "name": name,
                    "kind": kind,
                    "location": {
                        "uri": doc_uri,
                        "range": range
                    },
                    "containerName": container_name
                }),
            ));
        }
    };

    for (doc_uri, doc_text) in files.values() {
        visit_doc(doc_uri, doc_text);
    }
    for (doc_uri, doc_text) in &non_file_docs {
        visit_doc(doc_uri, doc_text);
    }

    matches.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then_with(|| a.1.cmp(&b.1))
            .then_with(|| a.2.cmp(&b.2))
    });

    Value::Array(matches.into_iter().map(|(_, _, _, v)| v).collect())
}

fn workspace_symbol_match_rank(query: &str, candidate: &str) -> Option<u8> {
    if query.is_empty() {
        return Some(0);
    }
    let q = query.to_ascii_lowercase();
    let c = candidate.to_ascii_lowercase();
    if c == q {
        return Some(0);
    }
    if c.starts_with(&q) {
        return Some(1);
    }
    if c.contains(&q) {
        return Some(2);
    }
    // Case-insensitive fuzzy subsequence match
    let mut q_chars = q.chars().peekable();
    for ch in c.chars() {
        if q_chars.peek() == Some(&ch) {
            q_chars.next();
            if q_chars.peek().is_none() {
                return Some(3);
            }
        }
    }
    None
}

// ============================================================================
// Folding Range Provider (`textDocument/foldingRange`)
// ============================================================================

fn handle_folding_range(state: &LspState, params: Option<&Value>) -> Value {
    let Some(uri) = params
        .and_then(|p| p.pointer("/textDocument/uri"))
        .and_then(Value::as_str)
    else {
        return Value::Array(Vec::new());
    };
    let Some(text) = state.documents.get(uri) else {
        return Value::Array(Vec::new());
    };

    let mut ranges: Vec<(u32, u32, &'static str)> = Vec::new();

    // 1. Block comments (`/* ... */`) and contiguous line-comment (`//`) headers
    collect_comment_folding_ranges(text, &mut ranges);

    if let Some(prog) = program_for_uri(state, uri) {
        // 2. Contiguous import blocks & multi-line `import { ... } from "..."` statements
        if !prog.imports.is_empty() {
            let mut imp_spans: Vec<(u32, u32)> = Vec::with_capacity(prog.imports.len());
            for imp in &prog.imports {
                let sl = byte_offset_to_lsp_pos(text, imp.span.start).0;
                let el = byte_offset_to_lsp_pos(text, imp.span.end).0;
                if el > sl {
                    ranges.push((sl, el, "imports"));
                }
                imp_spans.push((sl, el));
            }
            let lines: Vec<&str> = text.split('\n').collect();
            let mut group_start = imp_spans[0].0;
            let mut group_end = imp_spans[0].1;
            let mut group_count = 1usize;
            for &(sl, el) in &imp_spans[1..] {
                let only_ws_or_comments = ((group_end + 1) as usize..sl as usize).all(|li| {
                    let t = lines.get(li).copied().unwrap_or("").trim();
                    t.is_empty() || t.starts_with("//")
                });
                if sl <= group_end + 1 || only_ws_or_comments {
                    group_end = group_end.max(el);
                    group_count += 1;
                } else {
                    if group_count >= 2 && group_end > group_start {
                        ranges.push((group_start, group_end, "imports"));
                    }
                    group_start = sl;
                    group_end = el;
                    group_count = 1;
                }
            }
            if group_count >= 2 && group_end > group_start {
                ranges.push((group_start, group_end, "imports"));
            }
        }

        // 3. Multi-line `type`, `enum`, `function` (`@shader` included), and `const` declarations
        for decl in &prog.declarations {
            let span = match decl {
                Declaration::Struct(s) => s.span,
                Declaration::Enum(e) => e.span,
                Declaration::Function(f) => f.span,
                Declaration::Const(c) => c.span,
            };
            let sl = byte_offset_to_lsp_pos(text, span.start).0;
            let el = byte_offset_to_lsp_pos(text, span.end).0;
            if el > sl {
                ranges.push((sl, el, "region"));
            }
        }
    } else {
        // Tolerant fallback when file is temporarily mid-edit
        collect_brace_folding_ranges_fallback(text, &mut ranges);
    }

    ranges.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
    ranges.dedup_by(|a, b| a.0 == b.0 && a.1 == b.1);

    let lsp_ranges: Vec<Value> = ranges
        .into_iter()
        .map(|(start_line, end_line, kind)| {
            json!({
                "startLine": start_line,
                "endLine": end_line,
                "kind": kind
            })
        })
        .collect();
    Value::Array(lsp_ranges)
}

fn collect_comment_folding_ranges(text: &str, out: &mut Vec<(u32, u32, &'static str)>) {
    // 1. Contiguous full-line `//` comment runs of 2+ lines
    let mut run_start: Option<u32> = None;
    let mut run_end: u32 = 0;
    for (idx, raw) in text.split('\n').enumerate() {
        let line_no = idx as u32;
        if raw.trim_start().starts_with("//") {
            if run_start.is_none() {
                run_start = Some(line_no);
            }
            run_end = line_no;
        } else if let Some(start) = run_start.take() {
            if run_end > start {
                out.push((start, run_end, "comment"));
            }
        }
    }
    if let Some(start) = run_start {
        if run_end > start {
            out.push((start, run_end, "comment"));
        }
    }

    // 2. Multi-line block comments `/* ... */`
    let bytes = text.as_bytes();
    let len = bytes.len();
    let mut i = 0usize;
    while i < len {
        let b = bytes[i];
        if b == b'"' || b == b'\'' || b == b'`' {
            let q = b;
            i += 1;
            while i < len {
                if bytes[i] == b'\\' {
                    i = (i + 2).min(len);
                    continue;
                }
                if bytes[i] == q {
                    i += 1;
                    break;
                }
                i += 1;
            }
            continue;
        }
        if b == b'/' && i + 1 < len && bytes[i + 1] == b'/' {
            i += 2;
            while i < len && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if b == b'/' && i + 1 < len && bytes[i + 1] == b'*' {
            let start_byte = i;
            i += 2;
            while i + 1 < len && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                i += 1;
            }
            let end_byte = (i + 2).min(len);
            let sl = byte_offset_to_lsp_pos(text, start_byte).0;
            let el = byte_offset_to_lsp_pos(text, end_byte).0;
            if el > sl {
                out.push((sl, el, "comment"));
            }
            i = end_byte;
            continue;
        }
        i += 1;
    }
}

fn collect_brace_folding_ranges_fallback(text: &str, out: &mut Vec<(u32, u32, &'static str)>) {
    let bytes = text.as_bytes();
    let len = bytes.len();
    let mut i = 0usize;
    let mut line = 0u32;
    let mut brace_stack: Vec<u32> = Vec::new();
    while i < len {
        let b = bytes[i];
        if b == b'\n' {
            line += 1;
            i += 1;
            continue;
        }
        if b == b'/' && i + 1 < len && bytes[i + 1] == b'/' {
            i += 2;
            while i < len && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if b == b'/' && i + 1 < len && bytes[i + 1] == b'*' {
            i += 2;
            while i + 1 < len && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                if bytes[i] == b'\n' {
                    line += 1;
                }
                i += 1;
            }
            i = (i + 2).min(len);
            continue;
        }
        if b == b'"' || b == b'\'' || b == b'`' {
            let q = b;
            i += 1;
            while i < len {
                if bytes[i] == b'\\' {
                    i = (i + 2).min(len);
                    continue;
                }
                if bytes[i] == b'\n' {
                    line += 1;
                }
                if bytes[i] == q {
                    i += 1;
                    break;
                }
                i += 1;
            }
            continue;
        }
        if b == b'{' {
            brace_stack.push(line);
        } else if b == b'}' {
            if let Some(sl) = brace_stack.pop() {
                if line > sl {
                    out.push((sl, line, "region"));
                }
            }
        }
        i += 1;
    }
}

// ============================================================================
// Go to Definition Provider (`textDocument/definition`)
// ============================================================================

#[derive(Debug, Clone, PartialEq, Eq)]
enum ResolvedDef {
    Local {
        fn_span: Span,
        uri: String,
        range: Value,
    },
    TopLevel {
        uri: String,
        range: Value,
    },
    Member {
        uri: String,
        range: Value,
    },
}

impl ResolvedDef {
    fn uri(&self) -> &str {
        match self {
            ResolvedDef::Local { uri, .. }
            | ResolvedDef::TopLevel { uri, .. }
            | ResolvedDef::Member { uri, .. } => uri,
        }
    }

    fn range(&self) -> &Value {
        match self {
            ResolvedDef::Local { range, .. }
            | ResolvedDef::TopLevel { range, .. }
            | ResolvedDef::Member { range, .. } => range,
        }
    }

    fn to_location(&self) -> Value {
        json!({
            "uri": self.uri(),
            "range": self.range()
        })
    }
}

fn handle_definition(state: &LspState, params: Option<&Value>) -> Value {
    let Some((uri, line, character)) = extract_text_doc_position(params) else {
        return Value::Null;
    };
    let Some(text) = state.documents.get(uri) else {
        return Value::Null;
    };

    // 0. Ctrl+Click directly on a module path string in `import ... from "./state.tsk"` / `export ... from ...`
    if let Some((imp_path, _str_range)) = import_string_at_position(text, line, character) {
        if let Some((imp_uri, _imp_text)) = resolve_local_import_targets(state, uri, &imp_path) {
            return json!({
                "uri": imp_uri,
                "range": make_lsp_range(0, 0, 0, 0)
            });
        }
    }

    let Some((ident, ident_start_byte, _ident_end_byte, _range)) =
        symbol_at_position(text, line, character)
    else {
        return Value::Null;
    };

    match resolve_definition(state, uri, text, &ident, ident_start_byte) {
        Some(def) => def.to_location(),
        None => Value::Null,
    }
}

fn resolve_definition(
    state: &LspState,
    uri: &str,
    text: &str,
    ident: &str,
    cursor_byte: usize,
) -> Option<ResolvedDef> {
    let prog = program_for_uri(state, uri)?;
    let imported = load_imported_programs_recursive(state, uri, &prog);

    // Detect `Qualifier.ident` member/variant/namespace access right before the cursor token
    if let Some(qualifier) = dot_qualifier_before(text, cursor_byte) {
        // 0. Namespace import: `import * as state from "./state.tsk"` -> `state.init`
        if let Some(ns_mod) = resolve_namespace_target_in_program(state, uri, &prog, &qualifier) {
            if let Some(span) = find_top_level_decl_in_program(&ns_mod.prog, ident) {
                return Some(ResolvedDef::TopLevel {
                    uri: ns_mod.uri,
                    range: span_to_lsp_range(&ns_mod.text, span),
                });
            }
        }
        // 1. Check enum variant on matching enum in current document
        if let Some(span) = find_enum_variant_in_program(&prog, Some(&qualifier), ident) {
            return Some(ResolvedDef::Member {
                uri: uri.to_string(),
                range: span_to_lsp_range(text, span),
            });
        }
        // 2. Check enum variant in imported files (recursive)
        for imp in &imported {
            if let Some(span) = find_enum_variant_in_program(&imp.prog, Some(&qualifier), ident) {
                return Some(ResolvedDef::Member {
                    uri: imp.uri.clone(),
                    range: span_to_lsp_range(&imp.text, span),
                });
            }
        }
        // 3. Check struct field in current document
        if let Some(span) = find_struct_field_in_program(&prog, ident) {
            return Some(ResolvedDef::Member {
                uri: uri.to_string(),
                range: span_to_lsp_range(text, span),
            });
        }
        // 4. Check struct field in imported files (recursive)
        for imp in &imported {
            if let Some(span) = find_struct_field_in_program(&imp.prog, ident) {
                return Some(ResolvedDef::Member {
                    uri: imp.uri.clone(),
                    range: span_to_lsp_range(&imp.text, span),
                });
            }
        }
    }

    // 1. Enclosing function's parameters & local `let`/`const`/match bindings first
    if let Some(enclosing_fn) = find_enclosing_function_at_byte(&prog, cursor_byte) {
        if let Some(local_span) = find_local_def_in_fn(enclosing_fn, ident, cursor_byte) {
            return Some(ResolvedDef::Local {
                fn_span: enclosing_fn.span,
                uri: uri.to_string(),
                range: span_to_lsp_range(text, local_span),
            });
        }
    }

    // Resolve `import { orig as alias } from "..."`
    let resolved_ident = resolve_alias_in_program(&prog, ident);

    // 2. Current document's top-level declarations (`Function`, `Struct`, `Enum`, `Const`)
    if let Some(decl_span) = find_top_level_decl_in_program(&prog, &resolved_ident) {
        return Some(ResolvedDef::TopLevel {
            uri: uri.to_string(),
            range: span_to_lsp_range(text, decl_span),
        });
    }

    // 2b. Enum variants declared in the current document (e.g. inside `switch`/`match` cases)
    if let Some(variant_span) = find_enum_variant_in_program(&prog, None, &resolved_ident) {
        return Some(ResolvedDef::Member {
            uri: uri.to_string(),
            range: span_to_lsp_range(text, variant_span),
        });
    }

    // 3. Imported local `.tsk` / `.knr` files resolved recursively relative to the current document
    for imp in &imported {
        if let Some(decl_span) = find_top_level_decl_in_program(&imp.prog, &resolved_ident) {
            return Some(ResolvedDef::TopLevel {
                uri: imp.uri.clone(),
                range: span_to_lsp_range(&imp.text, decl_span),
            });
        }
    }
    for imp in &imported {
        if let Some(variant_span) = find_enum_variant_in_program(&imp.prog, None, &resolved_ident) {
            return Some(ResolvedDef::Member {
                uri: imp.uri.clone(),
                range: span_to_lsp_range(&imp.text, variant_span),
            });
        }
    }

    None
}

fn find_top_level_decl_in_program(prog: &Program, ident: &str) -> Option<Span> {
    for decl in &prog.declarations {
        match decl {
            Declaration::Function(f) if f.name.name == ident => return Some(f.name.span),
            Declaration::Struct(s) if s.name.name == ident => return Some(s.name.span),
            Declaration::Enum(e) if e.name.name == ident => return Some(e.name.span),
            Declaration::Const(c) if c.name.name == ident => return Some(c.name.span),
            _ => {}
        }
    }
    None
}

fn find_enum_variant_in_program(
    prog: &Program,
    enum_qualifier: Option<&str>,
    variant_name: &str,
) -> Option<Span> {
    for decl in &prog.declarations {
        if let Declaration::Enum(e) = decl {
            if let Some(qual) = enum_qualifier {
                if e.name.name != qual {
                    continue;
                }
            }
            for v in &e.variants {
                if v.name.name == variant_name {
                    return Some(v.name.span);
                }
            }
        }
    }
    None
}

fn find_struct_field_in_program(prog: &Program, field_name: &str) -> Option<Span> {
    for decl in &prog.declarations {
        if let Declaration::Struct(s) = decl {
            for field in &s.fields {
                if field.name.name == field_name {
                    return Some(field.name.span);
                }
            }
        }
    }
    None
}

fn find_enclosing_function_at_byte(prog: &Program, byte_offset: usize) -> Option<&FunctionDef> {
    for decl in &prog.declarations {
        if let Declaration::Function(f) = decl {
            if f.span.start <= byte_offset && byte_offset <= f.span.end {
                return Some(f);
            }
        }
    }
    None
}

fn find_local_def_in_fn(f: &FunctionDef, ident: &str, cursor_offset: usize) -> Option<Span> {
    let mut found: Option<Span> = None;
    for param in &f.params {
        if param.name.name == ident {
            found = Some(param.name.span);
        }
    }
    if let Some(body_span) = find_local_def_in_block(&f.body, ident, cursor_offset) {
        found = Some(body_span);
    }
    found
}

fn span_contains(span: Span, offset: usize) -> bool {
    span.start <= offset && offset <= span.end
}

fn find_local_def_in_block(block: &Block, ident: &str, cursor_offset: usize) -> Option<Span> {
    let mut found: Option<Span> = None;
    for stmt in &block.stmts {
        if let Some(inner) = find_local_def_in_stmt(stmt, ident, cursor_offset, &mut found) {
            return Some(inner);
        }
    }
    if let Some(ref tail) = block.tail_expr {
        if span_contains(tail.span, cursor_offset) {
            if let Some(inner) = find_local_def_in_expr(tail, ident, cursor_offset) {
                return Some(inner);
            }
        }
    }
    found
}

fn find_local_def_in_stmt(
    stmt: &Stmt,
    ident: &str,
    cursor_offset: usize,
    active_binding: &mut Option<Span>,
) -> Option<Span> {
    match &stmt.kind {
        StmtKind::Let {
            name,
            init,
            destruct,
            ..
        } => {
            if let Some(pat) = destruct {
                // 1. Cursor directly on one of the destructured bindings
                match pat {
                    DestructPattern::Object(fields) => {
                        for (field_name, alias) in fields {
                            let binding = alias.as_ref().unwrap_or(field_name);
                            if span_contains(binding.span, cursor_offset) && binding.name == ident {
                                return Some(binding.span);
                            }
                        }
                    }
                    DestructPattern::Array(elems) => {
                        for elem in elems {
                            if span_contains(elem.span, cursor_offset) && elem.name == ident {
                                return Some(elem.span);
                            }
                        }
                    }
                }
                // 2. Cursor inside the initializer expression (before bindings take effect)
                if span_contains(init.span, cursor_offset) {
                    return find_local_def_in_expr(init, ident, cursor_offset);
                }
                // 3. Subsequent statements see all destructured local bindings
                match pat {
                    DestructPattern::Object(fields) => {
                        for (field_name, alias) in fields {
                            let binding = alias.as_ref().unwrap_or(field_name);
                            if binding.name == ident && binding.span.start <= cursor_offset {
                                *active_binding = Some(binding.span);
                            }
                        }
                    }
                    DestructPattern::Array(elems) => {
                        for elem in elems {
                            if elem.name == ident && elem.span.start <= cursor_offset {
                                *active_binding = Some(elem.span);
                            }
                        }
                    }
                }
            } else {
                if span_contains(name.span, cursor_offset) && name.name == ident {
                    return Some(name.span);
                }
                if span_contains(init.span, cursor_offset) {
                    if let Some(inner) = find_local_def_in_expr(init, ident, cursor_offset) {
                        return Some(inner);
                    }
                }
                if name.name == ident
                    && name.span.start <= cursor_offset
                    && !span_contains(init.span, cursor_offset)
                {
                    *active_binding = Some(name.span);
                }
            }
        }
        StmtKind::Assign { target, value } => {
            if span_contains(target.span, cursor_offset) {
                if let Some(inner) = find_local_def_in_expr(target, ident, cursor_offset) {
                    return Some(inner);
                }
            }
            if span_contains(value.span, cursor_offset) {
                if let Some(inner) = find_local_def_in_expr(value, ident, cursor_offset) {
                    return Some(inner);
                }
            }
        }
        StmtKind::Return { value: Some(expr) }
        | StmtKind::Expr(expr)
        | StmtKind::Syscall { payload: expr, .. } => {
            if span_contains(expr.span, cursor_offset) {
                if let Some(inner) = find_local_def_in_expr(expr, ident, cursor_offset) {
                    return Some(inner);
                }
            }
        }
        StmtKind::If {
            cond,
            then_block,
            else_block,
        } => {
            if span_contains(cond.span, cursor_offset) {
                if let Some(inner) = find_local_def_in_expr(cond, ident, cursor_offset) {
                    return Some(inner);
                }
            }
            if span_contains(then_block.span, cursor_offset) {
                if let Some(inner) = find_local_def_in_block(then_block, ident, cursor_offset) {
                    return Some(inner);
                }
            }
            if let Some(eb) = else_block {
                if span_contains(eb.span, cursor_offset) {
                    if let Some(inner) = find_local_def_in_block(eb, ident, cursor_offset) {
                        return Some(inner);
                    }
                }
            }
        }
        StmtKind::While { cond, body } => {
            if span_contains(cond.span, cursor_offset) {
                if let Some(inner) = find_local_def_in_expr(cond, ident, cursor_offset) {
                    return Some(inner);
                }
            }
            if span_contains(body.span, cursor_offset) {
                if let Some(inner) = find_local_def_in_block(body, ident, cursor_offset) {
                    return Some(inner);
                }
            }
        }
        StmtKind::Block(inner_blk) => {
            if span_contains(inner_blk.span, cursor_offset) {
                if let Some(inner) = find_local_def_in_block(inner_blk, ident, cursor_offset) {
                    return Some(inner);
                }
            }
        }
        StmtKind::Return { value: None } | StmtKind::Break | StmtKind::Continue => {}
    }
    None
}

fn find_local_def_in_expr(expr: &Expr, ident: &str, cursor_offset: usize) -> Option<Span> {
    match &expr.kind {
        ExprKind::Block(blk) => find_local_def_in_block(blk, ident, cursor_offset),
        ExprKind::If {
            cond,
            then_branch,
            else_branch,
        } => {
            if span_contains(cond.span, cursor_offset) {
                if let Some(s) = find_local_def_in_expr(cond, ident, cursor_offset) {
                    return Some(s);
                }
            }
            if span_contains(then_branch.span, cursor_offset) {
                if let Some(s) = find_local_def_in_block(then_branch, ident, cursor_offset) {
                    return Some(s);
                }
            }
            if span_contains(else_branch.span, cursor_offset) {
                if let Some(s) = find_local_def_in_block(else_branch, ident, cursor_offset) {
                    return Some(s);
                }
            }
            None
        }
        ExprKind::Match { scrutinee, arms } => {
            if span_contains(scrutinee.span, cursor_offset) {
                if let Some(s) = find_local_def_in_expr(scrutinee, ident, cursor_offset) {
                    return Some(s);
                }
            }
            for arm in arms {
                if span_contains(arm.span, cursor_offset) {
                    let mut pat_binding = None;
                    if let MatchPattern::Variant { bindings, .. } = &arm.pattern {
                        for b in bindings {
                            if b.name == ident {
                                pat_binding = Some(b.span);
                            }
                        }
                    }
                    if let Some(inner) = find_local_def_in_expr(&arm.body, ident, cursor_offset) {
                        return Some(inner);
                    }
                    if pat_binding.is_some() {
                        return pat_binding;
                    }
                }
            }
            None
        }
        ExprKind::Jsx {
            props, children, ..
        } => {
            for prop in props {
                if let Some(ref val) = prop.value {
                    if span_contains(val.span, cursor_offset) {
                        if let Some(s) = find_local_def_in_expr(val, ident, cursor_offset) {
                            return Some(s);
                        }
                    }
                }
            }
            for child in children {
                match child {
                    JsxChild::Expr(e) | JsxChild::Element(e) => {
                        if span_contains(e.span, cursor_offset) {
                            if let Some(s) = find_local_def_in_expr(e, ident, cursor_offset) {
                                return Some(s);
                            }
                        }
                    }
                    JsxChild::Text(_, _) => {}
                }
            }
            None
        }
        ExprKind::Call { args, .. } | ExprKind::EnumLit { args, .. } | ExprKind::ArrayLit(args) => {
            for arg in args {
                if span_contains(arg.span, cursor_offset) {
                    if let Some(s) = find_local_def_in_expr(arg, ident, cursor_offset) {
                        return Some(s);
                    }
                }
            }
            None
        }
        ExprKind::StructLit { fields, spread, .. } => {
            for field in fields {
                if span_contains(field.value.span, cursor_offset) {
                    if let Some(s) = find_local_def_in_expr(&field.value, ident, cursor_offset) {
                        return Some(s);
                    }
                }
            }
            if let Some(ref sp) = spread {
                if span_contains(sp.span, cursor_offset) {
                    return find_local_def_in_expr(sp, ident, cursor_offset);
                }
            }
            None
        }
        ExprKind::Binary { lhs, rhs, .. } => {
            if span_contains(lhs.span, cursor_offset) {
                if let Some(s) = find_local_def_in_expr(lhs, ident, cursor_offset) {
                    return Some(s);
                }
            }
            if span_contains(rhs.span, cursor_offset) {
                return find_local_def_in_expr(rhs, ident, cursor_offset);
            }
            None
        }
        ExprKind::Unary { operand, .. }
        | ExprKind::Borrow(operand)
        | ExprKind::FieldAccess {
            target: operand, ..
        }
        | ExprKind::Syscall {
            payload: operand, ..
        } => {
            if span_contains(operand.span, cursor_offset) {
                return find_local_def_in_expr(operand, ident, cursor_offset);
            }
            None
        }
        ExprKind::Index { target, index } => {
            if span_contains(target.span, cursor_offset) {
                if let Some(s) = find_local_def_in_expr(target, ident, cursor_offset) {
                    return Some(s);
                }
            }
            if span_contains(index.span, cursor_offset) {
                return find_local_def_in_expr(index, ident, cursor_offset);
            }
            None
        }
        ExprKind::TemplateStr(parts) => {
            for part in parts {
                if let TemplatePart::Expr(e) = part {
                    if span_contains(e.span, cursor_offset) {
                        if let Some(s) = find_local_def_in_expr(e, ident, cursor_offset) {
                            return Some(s);
                        }
                    }
                }
            }
            None
        }
        ExprKind::Arrow { params, body } => {
            let mut param_binding = None;
            for p in params {
                if p.name == ident {
                    param_binding = Some(p.span);
                }
            }
            let body_hit = match body {
                ArrowBody::Expr(e) if span_contains(e.span, cursor_offset) => {
                    find_local_def_in_expr(e, ident, cursor_offset)
                }
                ArrowBody::Block(blk) if span_contains(blk.span, cursor_offset) => {
                    find_local_def_in_block(blk, ident, cursor_offset)
                }
                _ => None,
            };
            body_hit.or(param_binding)
        }
        ExprKind::MethodCall { receiver, args, .. } => {
            if span_contains(receiver.span, cursor_offset) {
                if let Some(s) = find_local_def_in_expr(receiver, ident, cursor_offset) {
                    return Some(s);
                }
            }
            for arg in args {
                if span_contains(arg.span, cursor_offset) {
                    if let Some(s) = find_local_def_in_expr(arg, ident, cursor_offset) {
                        return Some(s);
                    }
                }
            }
            None
        }
        ExprKind::Int(_)
        | ExprKind::Float(_)
        | ExprKind::Bool(_)
        | ExprKind::Str(_)
        | ExprKind::ColorHex(_)
        | ExprKind::Variable(_) => None,
    }
}

#[derive(Debug, Clone)]
struct ImportedModule {
    uri: String,
    import_spec: String,
    text: String,
    prog: Program,
}

fn resolve_local_import_targets(
    state: &LspState,
    current_uri: &str,
    imp_path: &str,
) -> Option<(String, String)> {
    if imp_path.starts_with("std/") {
        return None;
    }
    let current_path = uri_to_path(current_uri)?;
    let base_dir = current_path.parent()?;

    // 0. Non-relative package imports resolved via `knr.toml` `[dependencies]` / workspace members
    if !imp_path.starts_with("./") && !imp_path.starts_with("../") && !imp_path.starts_with('/') {
        let overlays = build_overlays(state);
        if let Some(pkg_path) =
            knr_core::manifest::resolve_package_import_with_overlays(imp_path, base_dir, &overlays)
                .or_else(|| knr_core::manifest::resolve_package_import(imp_path, base_dir))
        {
            let norm = normalize_path(&pkg_path);
            let pkg_uri = path_to_uri(&norm);
            if let Some(open_text) = state.documents.get(&pkg_uri) {
                return Some((pkg_uri, open_text.clone()));
            }
            for (doc_uri, doc_text) in &state.documents {
                if let Some(doc_path) = uri_to_path(doc_uri) {
                    if normalize_path(&doc_path) == norm {
                        return Some((doc_uri.clone(), doc_text.clone()));
                    }
                }
            }
            if norm.is_file() {
                if let Ok(disk_text) = std::fs::read_to_string(&norm) {
                    return Some((pkg_uri, disk_text));
                }
            }
        }
    }

    let mut variants = vec![imp_path.to_string()];
    if !imp_path.ends_with(".tsk") && !imp_path.ends_with(".knr") {
        let trimmed = imp_path.trim_end_matches('/');
        variants.push(format!("{trimmed}.tsk"));
        variants.push(format!("{trimmed}.knr"));
        variants.push(format!("{trimmed}/index.tsk"));
        variants.push(format!("{trimmed}/index.knr"));
    }

    for variant in &variants {
        let candidate = normalize_path(&base_dir.join(variant));
        let candidate_uri = path_to_uri(&candidate);

        // 1. Prefer open document in `state.documents` (handles unsaved edits & in-memory sessions)
        if let Some(open_text) = state.documents.get(&candidate_uri) {
            return Some((candidate_uri, open_text.clone()));
        }
        for (doc_uri, doc_text) in &state.documents {
            if let Some(doc_path) = uri_to_path(doc_uri) {
                if normalize_path(&doc_path) == candidate {
                    return Some((doc_uri.clone(), doc_text.clone()));
                }
            }
        }

        // 2. Fall back to reading local `.tsk`/`.knr` file from disk
        if candidate.is_file() {
            if let Ok(disk_text) = std::fs::read_to_string(&candidate) {
                return Some((candidate_uri, disk_text));
            }
        }
    }
    None
}

fn load_imported_stdlib_programs(prog: &Program) -> Vec<ImportedModule> {
    let mut results = Vec::new();
    let mut seen = HashSet::new();
    for imp in &prog.imports {
        if imp.path.starts_with("std/") && seen.insert(imp.path.clone()) {
            if let Some(std_src) = knr_core::syntax::stdlib::resolve_builtin_std(&imp.path) {
                if let Ok(std_prog) = knr_core::syntax::parse_tsx(std_src) {
                    results.push(ImportedModule {
                        uri: format!("knr-std://{}", imp.path),
                        import_spec: imp.path.clone(),
                        text: std_src.to_string(),
                        prog: std_prog,
                    });
                }
            }
        }
    }
    results
}

fn load_imported_stdlib_programs_tolerant(text: &str) -> Vec<ImportedModule> {
    let mut results = Vec::new();
    let mut seen = HashSet::new();
    for spec in extract_all_import_paths_tolerant(text) {
        if spec.starts_with("std/") && seen.insert(spec.clone()) {
            if let Some(std_src) = knr_core::syntax::stdlib::resolve_builtin_std(&spec) {
                if let Ok(std_prog) = knr_core::syntax::parse_tsx(std_src) {
                    results.push(ImportedModule {
                        uri: format!("knr-std://{spec}"),
                        import_spec: spec,
                        text: std_src.to_string(),
                        prog: std_prog,
                    });
                }
            }
        }
    }
    results
}

fn extract_all_import_paths_tolerant(text: &str) -> Vec<String> {
    let mut paths = Vec::new();
    for raw in text.lines() {
        let trimmed = raw.trim();
        if trimmed.starts_with("//") {
            continue;
        }
        let after_kw = if let Some(rest) = trimmed.strip_prefix("import ") {
            if let Some(pos) = rest.find(" from ") {
                rest[pos + 6..].trim_start()
            } else {
                rest.trim_start()
            }
        } else if let Some(pos) = trimmed.find("} from ") {
            trimmed[pos + 7..].trim_start()
        } else if let Some(rest) = trimmed.strip_prefix("export ") {
            if let Some(pos) = rest.find(" from ") {
                rest[pos + 6..].trim_start()
            } else {
                continue;
            }
        } else {
            continue;
        };

        let Some(first) = after_kw.as_bytes().first().copied() else {
            continue;
        };
        if first == b'"' || first == b'\'' {
            let body = &after_kw[1..];
            if let Some(end) = body.find(first as char) {
                let spec = &body[..end];
                if !spec.is_empty() {
                    paths.push(spec.to_string());
                }
            }
        }
    }
    paths
}

fn load_imported_programs_recursive(
    state: &LspState,
    uri: &str,
    prog: &Program,
) -> Vec<ImportedModule> {
    const MAX_IMPORTED_MODULES: usize = 32;
    let mut results = Vec::new();
    let mut seen_uris = HashSet::new();
    seen_uris.insert(uri.to_string());
    if let Some(cur_path) = uri_to_path(uri) {
        seen_uris.insert(path_to_uri(&normalize_path(&cur_path)));
    }

    let mut queue: std::collections::VecDeque<(String, Program)> = std::collections::VecDeque::new();
    queue.push_back((uri.to_string(), prog.clone()));

    while let Some((cur_uri, cur_prog)) = queue.pop_front() {
        for imp in &cur_prog.imports {
            if results.len() >= MAX_IMPORTED_MODULES {
                return results;
            }
            if let Some((imp_uri, imp_text)) = resolve_local_import_targets(state, &cur_uri, &imp.path)
            {
                let norm_key = uri_to_path(&imp_uri)
                    .map(|p| path_to_uri(&normalize_path(&p)))
                    .unwrap_or_else(|| imp_uri.clone());
                if !seen_uris.insert(norm_key) {
                    continue;
                }
                if let Ok(imp_prog) = knr_core::syntax::parse_tsx(&imp_text) {
                    queue.push_back((imp_uri.clone(), imp_prog.clone()));
                    results.push(ImportedModule {
                        uri: imp_uri,
                        import_spec: imp.path.clone(),
                        text: imp_text,
                        prog: imp_prog,
                    });
                }
            }
        }
    }
    results
}

fn load_imported_programs_tolerant(
    state: &LspState,
    uri: &str,
    text: &str,
) -> Vec<ImportedModule> {
    const MAX_IMPORTED_MODULES: usize = 32;
    let mut results = Vec::new();
    let mut seen_uris = HashSet::new();
    seen_uris.insert(uri.to_string());
    if let Some(cur_path) = uri_to_path(uri) {
        seen_uris.insert(path_to_uri(&normalize_path(&cur_path)));
    }

    let mut queue: std::collections::VecDeque<(String, Program)> = std::collections::VecDeque::new();
    for imp_path in extract_import_paths_tolerant(text) {
        if results.len() >= MAX_IMPORTED_MODULES {
            return results;
        }
        if let Some((imp_uri, imp_text)) = resolve_local_import_targets(state, uri, &imp_path) {
            let norm_key = uri_to_path(&imp_uri)
                .map(|p| path_to_uri(&normalize_path(&p)))
                .unwrap_or_else(|| imp_uri.clone());
            if !seen_uris.insert(norm_key) {
                continue;
            }
            if let Ok(imp_prog) = knr_core::syntax::parse_tsx(&imp_text) {
                queue.push_back((imp_uri.clone(), imp_prog.clone()));
                results.push(ImportedModule {
                    uri: imp_uri,
                    import_spec: imp_path,
                    text: imp_text,
                    prog: imp_prog,
                });
            }
        }
    }

    while let Some((cur_uri, cur_prog)) = queue.pop_front() {
        for imp in &cur_prog.imports {
            if results.len() >= MAX_IMPORTED_MODULES {
                return results;
            }
            if let Some((imp_uri, imp_text)) =
                resolve_local_import_targets(state, &cur_uri, &imp.path)
            {
                let norm_key = uri_to_path(&imp_uri)
                    .map(|p| path_to_uri(&normalize_path(&p)))
                    .unwrap_or_else(|| imp_uri.clone());
                if !seen_uris.insert(norm_key) {
                    continue;
                }
                if let Ok(imp_prog) = knr_core::syntax::parse_tsx(&imp_text) {
                    queue.push_back((imp_uri.clone(), imp_prog.clone()));
                    results.push(ImportedModule {
                        uri: imp_uri,
                        import_spec: imp.path.clone(),
                        text: imp_text,
                        prog: imp_prog,
                    });
                }
            }
        }
    }
    results
}

fn extract_import_paths_tolerant(text: &str) -> Vec<String> {
    let mut paths = Vec::new();
    for raw in text.lines() {
        let trimmed = raw.trim();
        if trimmed.starts_with("//") {
            continue;
        }
        let after_kw = if let Some(rest) = trimmed.strip_prefix("import ") {
            if let Some(pos) = rest.find(" from ") {
                rest[pos + 6..].trim_start()
            } else {
                rest.trim_start()
            }
        } else if let Some(pos) = trimmed.find("} from ") {
            trimmed[pos + 7..].trim_start()
        } else if let Some(rest) = trimmed.strip_prefix("export ") {
            if let Some(pos) = rest.find(" from ") {
                rest[pos + 6..].trim_start()
            } else {
                continue;
            }
        } else {
            continue;
        };

        let Some(first) = after_kw.as_bytes().first().copied() else {
            continue;
        };
        if first == b'"' || first == b'\'' {
            let body = &after_kw[1..];
            if let Some(end) = body.find(first as char) {
                let spec = &body[..end];
                if !spec.is_empty() && !spec.starts_with("std/") {
                    paths.push(spec.to_string());
                }
            }
        }
    }
    paths
}

fn resolve_alias_in_program(prog: &Program, ident: &str) -> String {
    for imp in &prog.imports {
        match &imp.kind {
            ImportKind::Named(items) => {
                for (orig, alias) in items {
                    if let Some(al) = alias {
                        if al.name == ident {
                            return orig.name.clone();
                        }
                    }
                }
            }
            ImportKind::Default { named, .. } => {
                for (orig, alias) in named {
                    if let Some(al) = alias {
                        if al.name == ident {
                            return orig.name.clone();
                        }
                    }
                }
            }
            ImportKind::SideEffect | ImportKind::Namespace(_) => {}
        }
    }
    ident.to_string()
}

fn resolve_namespace_target_in_program(
    state: &LspState,
    uri: &str,
    prog: &Program,
    qualifier: &str,
) -> Option<ImportedModule> {
    for imp in &prog.imports {
        if let ImportKind::Namespace(alias) = &imp.kind {
            if alias.name == qualifier {
                if let Some(std_src) = knr_core::syntax::stdlib::resolve_builtin_std(&imp.path) {
                    let std_prog = knr_core::syntax::parse_tsx(std_src).ok()?;
                    return Some(ImportedModule {
                        uri: format!("knr-std://{}", imp.path),
                        import_spec: imp.path.clone(),
                        text: std_src.to_string(),
                        prog: std_prog,
                    });
                }
                let (imp_uri, imp_text) = resolve_local_import_targets(state, uri, &imp.path)?;
                let imp_prog = knr_core::syntax::parse_tsx(&imp_text).ok()?;
                return Some(ImportedModule {
                    uri: imp_uri,
                    import_spec: imp.path.clone(),
                    text: imp_text,
                    prog: imp_prog,
                });
            }
        }
    }
    None
}

fn uris_match(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    match (uri_to_path(a), uri_to_path(b)) {
        (Some(pa), Some(pb)) => normalize_path(&pa) == normalize_path(&pb),
        _ => false,
    }
}

fn document_imports_uri_recursive(
    state: &LspState,
    doc_uri: &str,
    doc_prog: &Program,
    target_uri: &str,
) -> bool {
    load_imported_programs_recursive(state, doc_uri, doc_prog)
        .iter()
        .any(|imp| uris_match(&imp.uri, target_uri))
}

fn discover_project_source_files(uri: &str) -> Vec<PathBuf> {
    const MAX_FILES: usize = 64;
    let Some(cur_path) = uri_to_path(uri) else {
        return Vec::new();
    };
    let Some(cur_dir) = cur_path.parent() else {
        return Vec::new();
    };
    if !cur_dir.is_dir() {
        return Vec::new();
    }

    // If inside a subdirectory like `site/components/`, include its parent when parent also has `.tsk`/`.knr` or `knr.toml`
    let scan_root = cur_dir
        .parent()
        .filter(|parent| {
            parent.join("knr.toml").is_file()
                || std::fs::read_dir(parent)
                    .ok()
                    .is_some_and(|rd| {
                        rd.flatten().any(|e| {
                            let p = e.path();
                            p.is_file() && knr_core::is_source_file(&p)
                        })
                    })
        })
        .unwrap_or(cur_dir);

    let mut files = Vec::new();
    collect_source_files_bounded(scan_root, 0, 3, MAX_FILES, &mut files);
    files.sort();
    files
}

fn collect_source_files_bounded(
    dir: &Path,
    depth: usize,
    max_depth: usize,
    limit: usize,
    out: &mut Vec<PathBuf>,
) {
    if depth > max_depth || out.len() >= limit {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut paths: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
    paths.sort();
    for path in paths {
        if out.len() >= limit {
            break;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if name.starts_with('.') || matches!(name, "target" | "node_modules" | "dist" | "build") {
            continue;
        }
        if path.is_dir() {
            collect_source_files_bounded(&path, depth + 1, max_depth, limit, out);
        } else if path.is_file() && knr_core::is_source_file(&path) {
            out.push(normalize_path(&path));
        }
    }
}

fn relative_import_spec(from_dir: &Path, target_file: &Path) -> Option<String> {
    let from_norm = normalize_path(from_dir);
    let target_norm = normalize_path(target_file);
    let from_comps: Vec<_> = from_norm.components().collect();
    let target_comps: Vec<_> = target_norm.components().collect();

    let mut common = 0usize;
    while common < from_comps.len()
        && common < target_comps.len()
        && from_comps[common] == target_comps[common]
    {
        common += 1;
    }
    let up = from_comps.len().saturating_sub(common);
    let mut parts: Vec<String> = Vec::new();
    if up == 0 {
        parts.push(".".to_string());
    } else {
        for _ in 0..up {
            parts.push("..".to_string());
        }
    }
    for comp in &target_comps[common..] {
        parts.push(comp.as_os_str().to_string_lossy().to_string());
    }
    Some(parts.join("/"))
}

/// Detects whether `(line, character)` lies inside a quoted string on an `import` or `export ... from` statement.
/// Returns `(raw_string_content, lsp_range_of_content_including_quotes)`.
fn import_string_at_position(text: &str, line: u32, character: u32) -> Option<(String, Value)> {
    let line_str = text.split('\n').nth(line as usize)?.trim_end_matches('\r');
    let trimmed = line_str.trim_start();
    // Accept single-line `import ... from "..."`, `import "..."`, `export ... from "..."`, and multiline closing `} from "..."`
    let looks_like_import_line = trimmed.starts_with("import ")
        || trimmed.starts_with("export ")
        || trimmed.contains("from \"")
        || trimmed.contains("from '");
    if !looks_like_import_line {
        return None;
    }

    let cursor_byte = utf16_col_to_byte_idx(line_str, character);
    let bytes = line_str.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        let q = bytes[i];
        if q == b'"' || q == b'\'' {
            let start_q = i;
            i += 1;
            let content_start = i;
            while i < bytes.len() && bytes[i] != q {
                if bytes[i] == b'\\' && i + 1 < bytes.len() {
                    i += 2;
                    continue;
                }
                i += 1;
            }
            let content_end = i;
            let end_q = (i + 1).min(bytes.len());
            if cursor_byte >= start_q && cursor_byte <= end_q {
                let raw = line_str[content_start..content_end].to_string();
                let start_u16: u32 = line_str[..start_q].chars().map(|c| c.len_utf16() as u32).sum();
                let end_u16: u32 = line_str[..end_q].chars().map(|c| c.len_utf16() as u32).sum();
                return Some((raw, make_lsp_range(line, start_u16, line, end_u16)));
            }
            i = end_q;
            continue;
        }
        i += 1;
    }
    None
}

/// If cursor sits inside the opening module path string of an import/re-export statement (`from "./|"` or `import "./|"`),
/// returns the text typed so far inside the quotes.
fn import_path_completion_context(text: &str, line: u32, character: u32) -> Option<String> {
    let line_str = text.split('\n').nth(line as usize)?.trim_end_matches('\r');
    let cursor_byte = utf16_col_to_byte_idx(line_str, character);
    let prefix = &line_str[..cursor_byte];

    // Find the last unmatched quote (`"` or `'`) in `prefix`
    let mut active_quote: Option<(usize, u8)> = None;
    let bytes = prefix.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        let b = bytes[i];
        if let Some((_, q)) = active_quote {
            if b == b'\\' {
                i += 2;
                continue;
            }
            if b == q {
                active_quote = None;
            }
        } else if b == b'"' || b == b'\'' {
            active_quote = Some((i, b));
        }
        i += 1;
    }

    let (quote_idx, _) = active_quote?;
    let before_quote = prefix[..quote_idx].trim_end();
    if before_quote.ends_with("from") || before_quote == "import" {
        return Some(prefix[quote_idx + 1..].to_string());
    }
    None
}

/// If cursor sits inside `{ ... }` of an `import { ... } from "<path>"` or `export { ... } from "<path>"` statement
/// (single-line or multiline), returns `(target_module_path, set_of_already_imported_names)`.
fn import_named_list_completion_context(
    text: &str,
    line: u32,
    character: u32,
) -> Option<(String, HashSet<String>)> {
    let line_starts = compute_line_starts(text);
    let line_start = *line_starts.get(line as usize)?;
    let line_raw_end = line_starts
        .get(line as usize + 1)
        .map(|&next| next.saturating_sub(1))
        .unwrap_or(text.len());
    let line_str = text[line_start..line_raw_end].trim_end_matches('\r');
    let col_byte = utf16_col_to_byte_idx(line_str, character);
    let cursor_byte = line_start + col_byte;

    // Look back up to 512 bytes for an unmatched `{` belonging to an `import` / `export` statement
    let back_start = cursor_byte.saturating_sub(512);
    let before = &text[back_start..cursor_byte];
    let open_rel = before.rfind('{')?;
    if before[open_rel + 1..].contains('}') {
        return None;
    }
    let open_abs = back_start + open_rel;
    let stmt_head = text[back_start..open_abs].trim_end();
    if !(stmt_head.ends_with("import")
        || stmt_head.ends_with("import type")
        || stmt_head.ends_with("export")
        || stmt_head.ends_with("export type"))
    {
        return None;
    }

    // Look ahead from cursor up to 512 bytes for `} from "<path>"`
    let fwd_end = (cursor_byte + 512).min(text.len());
    let after = &text[cursor_byte..fwd_end];
    let close_rel = after.find('}')?;
    let after_brace = after[close_rel + 1..].trim_start();
    let after_from = after_brace.strip_prefix("from")?.trim_start();
    let first_b = *after_from.as_bytes().first()?;
    if first_b != b'"' && first_b != b'\'' {
        return None;
    }
    let rest = &after_from[1..];
    let end_q = rest.find(first_b as char)?;
    let module_path = rest[..end_q].to_string();

    let inside = &text[open_abs + 1..cursor_byte + close_rel];
    let already: HashSet<String> = inside
        .split([',', '\n', '\r', ' ', '\t'])
        .filter(|s| !s.is_empty() && *s != "as" && *s != "type")
        .map(str::to_string)
        .collect();

    Some((module_path, already))
}

fn trailing_dot_qualifier(line_prefix: &str) -> Option<String> {
    let before_dot = line_prefix.trim_end().strip_suffix('.')?.trim_end();
    let mut start = before_dot.len();
    for (idx, c) in before_dot.char_indices().rev() {
        if is_ident_char(c) {
            start = idx;
        } else {
            break;
        }
    }
    if start < before_dot.len() {
        let qual = &before_dot[start..];
        if qual
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        {
            return Some(qual.to_string());
        }
    }
    None
}

fn complete_import_paths(state: &LspState, current_uri: &str, partial: &str) -> Vec<Value> {
    let mut items = Vec::new();
    let mut seen = HashSet::new();

    // 1. Local project `.tsk` / `.knr` modules relative to current file
    if let Some(cur_path) = uri_to_path(current_uri) {
        let cur_norm = normalize_path(&cur_path);
        if let Some(cur_dir) = cur_norm.parent() {
            let mut candidates = discover_project_source_files(current_uri);
            for open_uri in state.documents.keys() {
                if let Some(p) = uri_to_path(open_uri) {
                    let norm = normalize_path(&p);
                    if knr_core::is_source_file(&norm) && !candidates.contains(&norm) {
                        candidates.push(norm);
                    }
                }
            }
            candidates.sort();
            for cand in candidates {
                if cand == cur_norm {
                    continue;
                }
                if let Some(rel) = relative_import_spec(cur_dir, &cand) {
                    if !partial.is_empty() && !rel.starts_with(partial) && !rel.contains(partial) {
                        continue;
                    }
                    if seen.insert(rel.clone()) {
                        let insert_suffix = rel.strip_prefix(partial).unwrap_or(&rel);
                        items.push(json!({
                            "label": rel,
                            "kind": COMPLETION_KIND_MODULE,
                            "detail": "Local knr module",
                            "insertText": insert_suffix,
                            "insertTextFormat": INSERT_FORMAT_PLAIN,
                            "sortText": format!("0_local_{rel}")
                        }));
                    }
                }
            }
        }
    }

    // 1b. `knr.toml` `[dependencies]` / workspace member package imports (e.g. `"ui-kit"`, `"ui-kit/cards"`)
    if let Some(cur_path) = uri_to_path(current_uri) {
        if let Some(cur_dir) = normalize_path(&cur_path).parent() {
            for (pkg_spec, pkg_desc) in collect_manifest_package_completions(cur_dir, partial) {
                if !partial.is_empty()
                    && !pkg_spec.starts_with(partial)
                    && !pkg_spec.contains(partial)
                {
                    continue;
                }
                if seen.insert(pkg_spec.clone()) {
                    let insert_suffix = pkg_spec.strip_prefix(partial).unwrap_or(&pkg_spec);
                    items.push(json!({
                        "label": pkg_spec,
                        "kind": COMPLETION_KIND_MODULE,
                        "detail": pkg_desc,
                        "insertText": insert_suffix,
                        "insertTextFormat": INSERT_FORMAT_PLAIN,
                        "sortText": format!("0_pkg_{pkg_spec}")
                    }));
                }
            }
        }
    }

    // 2. Built-in standard library modules
    for (spec, desc) in [
        ("std/ui", "Declarative UI primitives, Style, Widget, Vec2, Color"),
        ("std/math", "GPU-pure 2D/3D SDF vector math & constants"),
        ("std/anim", "Easing curves & smooth_damp animation helpers"),
        ("std/core", "Generic Option<T> & Result<T, E> core types"),
        ("std/cmd", "Async MVU Cmd effect constructors (timer, HTTP, storage)"),
        ("std/sys", "Effectful filesystem, JSON & HTTP/1.1 helpers"),
        (
            "std/theme",
            "Design tokens & curated color themes (Theme, theme_obsidian, theme_light, theme_nord, theme_solar)",
        ),
        (
            "std/router",
            "Zero-GC client-side router (Route, router_init, router_push, router_back, router_replace, router_is, router_can_back)",
        ),
        (
            "std/layout",
            "Responsive breakpoint & layout helpers (is_compact, is_medium, is_wide, responsive_cols, responsive_pad)",
        ),
        (
            "std/testing",
            "Deterministic unit test assertions (assert_eq_i64, assert_near, assert_str_eq, assert_true, assert_false, assert_gt, assert_lt)",
        ),
    ] {
        if !partial.is_empty() && !spec.starts_with(partial) {
            continue;
        }
        if seen.insert(spec.to_string()) {
            let insert_suffix = spec.strip_prefix(partial).unwrap_or(spec);
            items.push(json!({
                "label": spec,
                "kind": COMPLETION_KIND_MODULE,
                "detail": desc,
                "insertText": insert_suffix,
                "insertTextFormat": INSERT_FORMAT_PLAIN,
                "sortText": format!("1_std_{spec}")
            }));
        }
    }

    items
}

fn collect_manifest_package_completions(base_dir: &Path, partial: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut cur = base_dir.to_path_buf();

    for _ in 0..=6 {
        let manifest_path = cur.join("knr.toml");
        if manifest_path.is_file() {
            if let Ok(manifest) = knr_core::manifest::Manifest::from_file(&manifest_path) {
                match manifest {
                    knr_core::manifest::Manifest::Package(pkg) => {
                        for (dep_name, dep_spec) in &pkg.dependencies {
                            let desc = match dep_spec.path() {
                                Some(p) => format!("Package dependency (knr.toml -> {})", p.display()),
                                None => "Package dependency (knr.toml)".to_string(),
                            };
                            if seen.insert(dep_name.clone()) {
                                out.push((dep_name.clone(), desc));
                            }
                            if let Some(rel_dep) = dep_spec.path() {
                                if partial.starts_with(&format!("{dep_name}/")) {
                                    collect_package_submodule_completions(
                                        dep_name,
                                        &cur.join(rel_dep),
                                        &mut seen,
                                        &mut out,
                                    );
                                }
                            }
                        }
                    }
                    knr_core::manifest::Manifest::Workspace(ws) => {
                        for (dep_name, dep_spec) in &ws.dependencies {
                            let desc = match dep_spec.path() {
                                Some(p) => format!("Workspace dependency (knr.toml -> {})", p.display()),
                                None => "Workspace dependency (knr.toml)".to_string(),
                            };
                            if seen.insert(dep_name.clone()) {
                                out.push((dep_name.clone(), desc));
                            }
                            if let Some(rel_dep) = dep_spec.path() {
                                if partial.starts_with(&format!("{dep_name}/")) {
                                    collect_package_submodule_completions(
                                        dep_name,
                                        &cur.join(rel_dep),
                                        &mut seen,
                                        &mut out,
                                    );
                                }
                            }
                        }
                        if let Ok(members) =
                            knr_core::manifest::resolve_workspace_members(&cur, &ws.members)
                        {
                            for member_dir in members {
                                let m_toml = member_dir.join("knr.toml");
                                if let Ok(knr_core::manifest::Manifest::Package(m_pkg)) =
                                    knr_core::manifest::Manifest::from_file(&m_toml)
                                {
                                    if seen.insert(m_pkg.name.clone()) {
                                        out.push((
                                            m_pkg.name.clone(),
                                            format!("Workspace package ({})", m_pkg.version),
                                        ));
                                    }
                                    if partial.starts_with(&format!("{}/", m_pkg.name)) {
                                        collect_package_submodule_completions(
                                            &m_pkg.name,
                                            &member_dir,
                                            &mut seen,
                                            &mut out,
                                        );
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        if !cur.pop() {
            break;
        }
    }

    out
}

fn collect_package_submodule_completions(
    pkg_name: &str,
    pkg_dir: &Path,
    seen: &mut HashSet<String>,
    out: &mut Vec<(String, String)>,
) {
    let search_root = if pkg_dir.join("src").is_dir() {
        pkg_dir.join("src")
    } else {
        pkg_dir.to_path_buf()
    };
    let mut files = Vec::new();
    collect_source_files_bounded(&search_root, 0, 2, 32, &mut files);
    let norm_root = normalize_path(&search_root);
    for file in files {
        if let Ok(rel) = file.strip_prefix(&norm_root) {
            let rel_str = rel.to_string_lossy().replace('\\', "/");
            let trimmed = rel_str
                .strip_suffix(".tsk")
                .or_else(|| rel_str.strip_suffix(".knr"))
                .unwrap_or(&rel_str);
            let spec = format!("{pkg_name}/{trimmed}");
            if seen.insert(spec.clone()) {
                out.push((spec, format!("Package submodule of `{pkg_name}`")));
            }
        }
    }
}

/// Detects whether cursor sits inside the string value of `name="..."` on an `<Icon ...>` JSX tag.
/// Returns the partial icon prefix typed so far inside the quotes.
fn icon_name_completion_context(text: &str, line: u32, character: u32) -> Option<String> {
    let line_starts = compute_line_starts(text);
    let line_start = *line_starts.get(line as usize)?;
    let line_raw_end = line_starts
        .get(line as usize + 1)
        .map(|&next| next.saturating_sub(1))
        .unwrap_or(text.len());
    let line_str = text[line_start..line_raw_end].trim_end_matches('\r');
    let col_byte = utf16_col_to_byte_idx(line_str, character);
    let prefix = &line_str[..col_byte];

    // Find the last unmatched quote (`"` or `'`) on the current line before cursor
    let mut active_quote: Option<(usize, u8)> = None;
    let bytes = prefix.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        let b = bytes[i];
        if let Some((_, q)) = active_quote {
            if b == b'\\' {
                i += 2;
                continue;
            }
            if b == q {
                active_quote = None;
            }
        } else if b == b'"' || b == b'\'' {
            active_quote = Some((i, b));
        }
        i += 1;
    }

    let (quote_rel, _) = active_quote?;
    let before_quote_line = prefix[..quote_rel].trim_end();
    let before_eq_line = before_quote_line.strip_suffix('=')?.trim_end();
    if !before_eq_line.ends_with("name") {
        return None;
    }
    let name_start_in_line = before_eq_line.len() - "name".len();
    if name_start_in_line > 0
        && before_eq_line[..name_start_in_line]
            .chars()
            .next_back()
            .is_some_and(is_ident_char)
    {
        return None;
    }

    // Walk back up to 512 bytes across potential multiline JSX opening tag for `<Icon `
    let quote_abs = line_start + quote_rel;
    let back_start = quote_abs.saturating_sub(512);
    let window = &text[back_start..quote_abs];
    let lt_rel = window.rfind('<')?;
    let tag_slice = &window[lt_rel + 1..];
    if tag_slice.contains('>') {
        return None;
    }
    let after_icon = tag_slice.strip_prefix("Icon")?;
    if !after_icon
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_whitespace())
    {
        return None;
    }

    Some(prefix[quote_rel + 1..].to_string())
}

fn complete_icon_names(partial: &str) -> Vec<Value> {
    let mut items = Vec::new();
    for &icon in knr_runtime::gui::supported_icon_names() {
        if !partial.is_empty() && !icon.starts_with(partial) && !icon.contains(partial) {
            continue;
        }
        let insert_suffix = icon.strip_prefix(partial).unwrap_or(icon);
        items.push(json!({
            "label": icon,
            "kind": COMPLETION_KIND_VALUE,
            "detail": "Built-in crisp 24x24 analytical vector icon",
            "documentation": {
                "kind": "markdown",
                "value": format!("```tsx\n<Icon name=\"{icon}\" size={{18}} />\n```\n\nCrisp anti-aliased analytical vector icon rendered identically on Native GPU, Headless PNG, and Web.")
            },
            "insertText": insert_suffix,
            "insertTextFormat": INSERT_FORMAT_PLAIN,
            "sortText": format!("0_icon_{icon}")
        }));
    }
    items
}

fn complete_named_import_symbols(
    state: &LspState,
    current_uri: &str,
    module_spec: &str,
    already_named: &HashSet<String>,
) -> Vec<Value> {
    let mut items = Vec::new();
    let mut seen = HashSet::new();

    if module_spec == "std/ui" || module_spec == "std/ui.tsk" {
        for comp in JSX_COMPONENTS {
            if !already_named.contains(comp.tag) && seen.insert(comp.tag.to_string()) {
                items.push(json!({
                    "label": comp.tag,
                    "kind": COMPLETION_KIND_CLASS,
                    "detail": comp.detail,
                    "documentation": comp.doc,
                    "sortText": format!("0_ui_{}", comp.tag)
                }));
            }
        }
    }

    let parsed_prog = if let Some(std_src) = knr_core::syntax::stdlib::resolve_builtin_std(module_spec)
    {
        knr_core::syntax::parse_tsx(std_src).ok()
    } else if let Some((_, text)) = resolve_local_import_targets(state, current_uri, module_spec) {
        knr_core::syntax::parse_tsx(&text).ok()
    } else {
        None
    };

    if let Some(prog) = parsed_prog {
        for decl in &prog.declarations {
            let (name, kind, detail, doc) = match decl {
                Declaration::Function(f) => {
                    let sig = format_function_signature(f);
                    (
                        f.name.name.clone(),
                        COMPLETION_KIND_FUNCTION,
                        sig.clone(),
                        format!("```tsx\n{sig}\n```"),
                    )
                }
                Declaration::Struct(s) => (
                    s.name.name.clone(),
                    COMPLETION_KIND_STRUCT,
                    format!("type {} ({} fields)", s.name.name, s.fields.len()),
                    format!("```tsx\n{}\n```", format_struct_def(s)),
                ),
                Declaration::Enum(e) => (
                    e.name.name.clone(),
                    COMPLETION_KIND_ENUM,
                    format!("enum {} ({} variants)", e.name.name, e.variants.len()),
                    format!("```tsx\n{}\n```", format_enum_def(e)),
                ),
                Declaration::Const(c) => {
                    let sig = format_const_def(c);
                    (
                        c.name.name.clone(),
                        COMPLETION_KIND_CONSTANT,
                        sig.clone(),
                        format!("```tsx\n{sig}\n```"),
                    )
                }
            };
            if already_named.contains(&name) || !seen.insert(name.clone()) {
                continue;
            }
            items.push(json!({
                "label": name,
                "kind": kind,
                "detail": detail,
                "documentation": { "kind": "markdown", "value": doc },
                "sortText": format!("0_sym_{name}")
            }));
        }
    }

    items
}

fn push_program_declaration_completions(
    prog: &Program,
    origin_module: Option<&str>,
    prefer_fields: bool,
    prefer_jsx_tags: bool,
    force_top_priority: bool,
    seen: &mut HashSet<(String, u32)>,
    items: &mut Vec<Value>,
) {
    let origin_note = origin_module
        .map(|m| format!("\n\n*Imported from `{m}`*"))
        .unwrap_or_default();
    let default_sort = if force_top_priority {
        "0"
    } else if prefer_fields {
        "3"
    } else {
        "1"
    };

    for decl in &prog.declarations {
        match decl {
            Declaration::Function(f) => {
                let sig = format_function_signature(f);
                let is_jsx_component = f
                    .name
                    .name
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_ascii_uppercase())
                    && format_type_expr(&f.ret_ty) == "Widget";
                let kind = if is_jsx_component && prefer_jsx_tags {
                    COMPLETION_KIND_CLASS
                } else {
                    COMPLETION_KIND_FUNCTION
                };
                let key = (f.name.name.clone(), kind);
                if seen.insert(key) {
                    let sort = if is_jsx_component && prefer_jsx_tags {
                        "0"
                    } else {
                        default_sort
                    };
                    let doc = if f.is_shader {
                        format!("GPU `@shader` kernel.\n\n```tsx\n{sig}\n```{origin_note}")
                    } else if is_jsx_component {
                        format!("JSX UI component.\n\n```tsx\n{sig}\n```{origin_note}")
                    } else {
                        format!("```tsx\n{sig}\n```{origin_note}")
                    };
                    items.push(json!({
                        "label": f.name.name,
                        "kind": kind,
                        "detail": sig,
                        "documentation": { "kind": "markdown", "value": doc },
                        "sortText": format!("{sort}_{}", f.name.name)
                    }));
                }
            }
            Declaration::Struct(s) => {
                let key = (s.name.name.clone(), COMPLETION_KIND_STRUCT);
                if seen.insert(key) {
                    items.push(json!({
                        "label": s.name.name,
                        "kind": COMPLETION_KIND_STRUCT,
                        "detail": format!("type {} ({} fields)", s.name.name, s.fields.len()),
                        "documentation": {
                            "kind": "markdown",
                            "value": format!("```tsx\n{}\n```{origin_note}", format_struct_def(s))
                        },
                        "sortText": format!("{default_sort}_{}", s.name.name)
                    }));
                }
                for field in &s.fields {
                    let fkey = (field.name.name.clone(), COMPLETION_KIND_FIELD);
                    if seen.insert(fkey) {
                        let sort = if prefer_fields { "0" } else { "2" };
                        let ty = format_type_expr(&field.ty);
                        let detail = match origin_module {
                            Some(m) => format!("{}.{}: {ty} ({m})", s.name.name, field.name.name),
                            None => format!("{}.{}: {ty}", s.name.name, field.name.name),
                        };
                        items.push(json!({
                            "label": field.name.name,
                            "kind": COMPLETION_KIND_FIELD,
                            "detail": detail,
                            "sortText": format!("{sort}_{}", field.name.name)
                        }));
                    }
                }
            }
            Declaration::Enum(e) => {
                let key = (e.name.name.clone(), COMPLETION_KIND_ENUM);
                if seen.insert(key) {
                    items.push(json!({
                        "label": e.name.name,
                        "kind": COMPLETION_KIND_ENUM,
                        "detail": format!("enum {} ({} variants)", e.name.name, e.variants.len()),
                        "documentation": {
                            "kind": "markdown",
                            "value": format!("```tsx\n{}\n```{origin_note}", format_enum_def(e))
                        },
                        "sortText": format!("{default_sort}_{}", e.name.name)
                    }));
                }
                for variant in &e.variants {
                    let vkey = (variant.name.name.clone(), COMPLETION_KIND_CONSTANT);
                    if seen.insert(vkey) {
                        let sort = if prefer_fields { "0" } else { "2" };
                        items.push(json!({
                            "label": variant.name.name,
                            "kind": COMPLETION_KIND_CONSTANT,
                            "detail": format!("{}.{}", e.name.name, variant.name.name),
                            "sortText": format!("{sort}_enum_{}", variant.name.name)
                        }));
                    }
                }
            }
            Declaration::Const(c) => {
                let key = (c.name.name.clone(), COMPLETION_KIND_CONSTANT);
                if seen.insert(key) {
                    let sig = format_const_def(c);
                    items.push(json!({
                        "label": c.name.name,
                        "kind": COMPLETION_KIND_CONSTANT,
                        "detail": sig.clone(),
                        "documentation": {
                            "kind": "markdown",
                            "value": format!("```tsx\n{sig}\n```{origin_note}")
                        },
                        "sortText": format!("{default_sort}_{}", c.name.name)
                    }));
                }
            }
        }
    }
}

// ============================================================================
// Find All References Provider (`textDocument/references`)
// ============================================================================

fn handle_references(state: &LspState, params: Option<&Value>) -> Value {
    let Some((uri, line, character)) = extract_text_doc_position(params) else {
        return Value::Array(Vec::new());
    };
    let include_declaration = params
        .and_then(|p| p.pointer("/context/includeDeclaration"))
        .and_then(Value::as_bool)
        .unwrap_or(true);

    let Some(text) = state.documents.get(uri) else {
        return Value::Array(Vec::new());
    };
    let Some((ident, ident_start_byte, _ident_end_byte, _range)) =
        symbol_at_position(text, line, character)
    else {
        return Value::Array(Vec::new());
    };

    let def = resolve_definition(state, uri, text, &ident, ident_start_byte);
    let occurrences = collect_symbol_occurrences(state, uri, text, &ident, def.as_ref(), false);

    let mut locations = Vec::new();
    for (occ_uri, occ_range) in occurrences {
        if !include_declaration {
            if let Some(ref d) = def {
                if occ_uri == d.uri() && &occ_range == d.range() {
                    continue;
                }
            }
        }
        locations.push(json!({
            "uri": occ_uri,
            "range": occ_range
        }));
    }

    Value::Array(locations)
}

/// Collects non-comment, non-string whole-word occurrences of `ident` in the current document
/// and any related open documents in `state.documents`.
fn collect_symbol_occurrences(
    state: &LspState,
    uri: &str,
    text: &str,
    ident: &str,
    def: Option<&ResolvedDef>,
    require_exported_for_cross_doc: bool,
) -> Vec<(String, Value)> {
    let mut results: Vec<(String, Value)> = Vec::new();

    match def {
        Some(ResolvedDef::Local { fn_span, .. }) => {
            // Restrict local variables/params to the enclosing function body and skip `.field` accesses
            for occ in scan_identifier_occurrences(text, ident) {
                if occ.start >= fn_span.start
                    && occ.end <= fn_span.end
                    && !occ.preceded_by_dot
                {
                    let (sl, sc) = byte_offset_to_lsp_pos(text, occ.start);
                    let (el, ec) = byte_offset_to_lsp_pos(text, occ.end);
                    results.push((uri.to_string(), make_lsp_range(sl, sc, el, ec)));
                }
            }
            return results;
        }
        _ => {
            for occ in scan_identifier_occurrences(text, ident) {
                let (sl, sc) = byte_offset_to_lsp_pos(text, occ.start);
                let (el, ec) = byte_offset_to_lsp_pos(text, occ.end);
                results.push((uri.to_string(), make_lsp_range(sl, sc, el, ec)));
            }
        }
    }

    // Determine whether to include matching open documents in `state.documents`
    let def_uri = def.map(|d| d.uri().to_string());
    let cur_prog = program_for_uri(state, uri);

    let can_cross_doc = if require_exported_for_cross_doc {
        match (&def_uri, &cur_prog) {
            (Some(d_uri), _) if d_uri != uri => true,
            (Some(_), Some(prog)) => is_symbol_exported_in_doc(text, prog, ident),
            _ => false,
        }
    } else {
        true
    };

    if !can_cross_doc {
        return results;
    }

    // Gather candidate cross-file documents: both open buffers in `state.documents` AND sibling project files on disk
    let mut candidate_docs: BTreeMap<String, (String, Option<Program>)> = BTreeMap::new();
    let mut seen_paths: HashSet<PathBuf> = HashSet::new();
    if let Some(cur_path) = uri_to_path(uri) {
        seen_paths.insert(normalize_path(&cur_path));
    }

    for (doc_uri, doc_text) in &state.documents {
        if uris_match(doc_uri, uri) {
            continue;
        }
        if let Some(p) = uri_to_path(doc_uri) {
            seen_paths.insert(normalize_path(&p));
        }
        let prog = program_for_uri(state, doc_uri);
        candidate_docs.insert(doc_uri.clone(), (doc_text.clone(), prog));
    }

    for disk_path in discover_project_source_files(uri) {
        let norm = normalize_path(&disk_path);
        if !seen_paths.insert(norm.clone()) {
            continue;
        }
        let disk_uri = path_to_uri(&norm);
        if uris_match(&disk_uri, uri) {
            continue;
        }
        if let Ok(disk_text) = std::fs::read_to_string(&norm) {
            let disk_prog = knr_core::syntax::parse_tsx(&disk_text).ok();
            candidate_docs.insert(disk_uri, (disk_text, disk_prog));
        }
    }

    for (other_uri, (other_text, other_prog)) in candidate_docs {
        let related = if def_uri.as_deref().is_some_and(|d| uris_match(d, &other_uri)) {
            true
        } else if let Some(ref o_prog) = other_prog {
            let imports_current = document_imports_uri_recursive(state, &other_uri, o_prog, uri);
            let imports_def = def_uri
                .as_deref()
                .is_some_and(|d| document_imports_uri_recursive(state, &other_uri, o_prog, d));
            let current_imports_other = cur_prog
                .as_ref()
                .is_some_and(|cp| document_imports_uri_recursive(state, uri, cp, &other_uri));
            imports_current || imports_def || current_imports_other
        } else {
            false
        };

        if related {
            for occ in scan_identifier_occurrences(&other_text, ident) {
                let (sl, sc) = byte_offset_to_lsp_pos(&other_text, occ.start);
                let (el, ec) = byte_offset_to_lsp_pos(&other_text, occ.end);
                results.push((other_uri.clone(), make_lsp_range(sl, sc, el, ec)));
            }
        }
    }

    results
}

fn is_symbol_exported_in_doc(text: &str, prog: &Program, ident: &str) -> bool {
    for decl in &prog.declarations {
        let (is_exported, decl_span) = match decl {
            Declaration::Function(f) if f.name.name == ident => (f.is_exported, f.span),
            Declaration::Struct(s) if s.name.name == ident => (s.is_exported, s.span),
            Declaration::Enum(e) if e.name.name == ident => (e.is_exported, e.span),
            Declaration::Const(c) if c.name.name == ident => (c.is_exported, c.span),
            _ => continue,
        };
        if is_exported {
            return true;
        }
        let line_idx = decl_span.line.saturating_sub(1);
        if let Some(line_str) = text.split('\n').nth(line_idx) {
            if line_str.trim_start().starts_with("export ")
                || line_str.contains(" export ")
            {
                return true;
            }
        }
    }
    false
}

// ============================================================================
// Rename Symbol Provider (`textDocument/prepareRename` & `textDocument/rename`)
// ============================================================================

fn handle_prepare_rename(state: &LspState, params: Option<&Value>) -> Value {
    let Some((uri, line, character)) = extract_text_doc_position(params) else {
        return Value::Null;
    };
    let Some(text) = state.documents.get(uri) else {
        return Value::Null;
    };
    let Some((ident, start_byte, end_byte, range)) = symbol_at_position(text, line, character)
    else {
        return Value::Null;
    };

    // Reject tokens inside comments or plain string literals
    let in_code = scan_identifier_occurrences(text, &ident)
        .iter()
        .any(|occ| occ.start == start_byte && occ.end == end_byte);
    if !in_code {
        return Value::Null;
    }

    let prog = program_for_uri(state, uri);
    if !is_renameable_identifier(&ident, prog.as_ref()) {
        return Value::Null;
    }

    json!({
        "range": range,
        "placeholder": ident
    })
}

fn handle_rename(state: &LspState, params: Option<&Value>) -> Value {
    let Some(params) = params else {
        return Value::Null;
    };
    let Some((uri, line, character)) = extract_text_doc_position(Some(params)) else {
        return Value::Null;
    };
    let Some(new_name) = params.get("newName").and_then(Value::as_str) else {
        return Value::Null;
    };
    if !is_valid_new_identifier(new_name) {
        return Value::Null;
    }

    let Some(text) = state.documents.get(uri) else {
        return Value::Null;
    };
    let Some((ident, start_byte, end_byte, _range)) = symbol_at_position(text, line, character)
    else {
        return Value::Null;
    };

    // Reject tokens inside comments or plain string literals
    let in_code = scan_identifier_occurrences(text, &ident)
        .iter()
        .any(|occ| occ.start == start_byte && occ.end == end_byte);
    if !in_code {
        return Value::Null;
    }

    let prog = program_for_uri(state, uri);
    if !is_renameable_identifier(&ident, prog.as_ref()) {
        return Value::Null;
    }

    let def = resolve_definition(state, uri, text, &ident, start_byte);
    let occurrences = collect_symbol_occurrences(state, uri, text, &ident, def.as_ref(), true);
    if occurrences.is_empty() {
        return Value::Null;
    }

    let mut changes: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    for (occ_uri, occ_range) in occurrences {
        changes.entry(occ_uri).or_default().push(json!({
            "range": occ_range,
            "newText": new_name
        }));
    }

    json!({
        "changes": changes
    })
}

const LANGUAGE_KEYWORDS: &[&str] = &[
    "import",
    "from",
    "export",
    "type",
    "interface",
    "enum",
    "function",
    "const",
    "let",
    "return",
    "if",
    "else",
    "while",
    "for",
    "switch",
    "case",
    "default",
    "break",
    "continue",
    "readonly",
    "true",
    "false",
    "as",
    "shader",
    "effects",
    "effect",
    "syscall",
    "void",
    "null",
    "undefined",
    "match",
    "fn",
    "struct",
    "mut",
    "pub",
    "in",
    "of",
    "new",
];

const PRIMITIVE_TYPES: &[&str] = &[
    "i64", "int", "f64", "number", "bool", "boolean", "str", "string", "buf", "Buffer", "void",
    "Array", "Vec", "Map", "Channel",
];

fn is_valid_new_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first.is_ascii_alphabetic() || first == '_') {
        return false;
    }
    if !chars.all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return false;
    }
    !LANGUAGE_KEYWORDS.contains(&name) && !PRIMITIVE_TYPES.contains(&name)
}

fn is_renameable_identifier(ident: &str, prog: Option<&Program>) -> bool {
    if !is_valid_new_identifier(ident) {
        return false;
    }
    if JSX_COMPONENTS.iter().any(|c| c.tag == ident) {
        return false;
    }
    if BUILTIN_FUNCTIONS.iter().any(|b| b.name == ident)
        || matches!(ident, "chan_new" | "chan_send" | "chan_recv")
    {
        return false;
    }
    if matches!(ident, "Style" | "Widget" | "Vec2" | "Color") {
        let locally_declared = prog.is_some_and(|p| {
            p.declarations.iter().any(|d| match d {
                Declaration::Struct(s) => s.name.name == ident,
                _ => false,
            })
        });
        if !locally_declared {
            return false;
        }
    }
    true
}

// ============================================================================
// Code Actions / QuickFixes Provider (`textDocument/codeAction`)
// ============================================================================

fn handle_code_action(state: &LspState, params: Option<&Value>) -> Value {
    let Some(params) = params else {
        return Value::Array(Vec::new());
    };
    let Some(uri) = params.pointer("/textDocument/uri").and_then(Value::as_str) else {
        return Value::Array(Vec::new());
    };
    let Some(req_range) = params.get("range") else {
        return Value::Array(Vec::new());
    };

    // Respect `context.only` filter when provided
    if let Some(only) = params.pointer("/context/only").and_then(Value::as_array) {
        let wants_quickfix = only.iter().filter_map(Value::as_str).any(|k| {
            k == "quickfix" || k.starts_with("quickfix.")
        });
        if !wants_quickfix {
            return Value::Array(Vec::new());
        }
    }

    let Some(text) = state.documents.get(uri) else {
        return Value::Array(Vec::new());
    };

    let overlays = build_overlays(state);
    let (parsed_prog, analyzed_diags) = analyze_document_detailed_with_overlays(uri, text, &overlays);
    let prog = parsed_prog.or_else(|| state.programs.get(uri).cloned());

    let mut actions: Vec<Value> = Vec::new();
    let mut seen_titles: HashSet<String> = HashSet::new();

    // 1. Inspect re-analyzed diagnostics overlapping the requested range
    for diag in &analyzed_diags {
        let overlaps = ranges_overlap_by_line(req_range, &diag.range)
            || is_shader_effect_range_match(diag, prog.as_ref(), text, req_range);
        if overlaps {
            for action in build_quickfixes_for_diagnostic(state, uri, text, prog.as_ref(), diag) {
                let title = action["title"].as_str().unwrap_or("").to_string();
                if seen_titles.insert(title) {
                    actions.push(action);
                }
            }
        }
    }

    // 2. Also inspect any diagnostics supplied directly by the client in `context.diagnostics`
    if let Some(ctx_diags) = params
        .pointer("/context/diagnostics")
        .and_then(Value::as_array)
    {
        for raw_diag in ctx_diags {
            if let Some(parsed_diag) = AnalyzedDiagnostic::from_lsp_value(raw_diag) {
                let overlaps = ranges_overlap_by_line(req_range, &parsed_diag.range)
                    || is_shader_effect_range_match(&parsed_diag, prog.as_ref(), text, req_range);
                if overlaps {
                    for action in build_quickfixes_for_diagnostic(
                        state,
                        uri,
                        text,
                        prog.as_ref(),
                        &parsed_diag,
                    ) {
                        let title = action["title"].as_str().unwrap_or("").to_string();
                        if seen_titles.insert(title) {
                            actions.push(action);
                        }
                    }
                }
            }
        }
    }

    Value::Array(actions)
}

/// Formats an entire `.tsk` / `.knr` document via `knr_core::format_tsk` (`textDocument/formatting`).
fn handle_formatting(state: &LspState, params: Option<&Value>) -> Value {
    let Some(uri) = params
        .and_then(|p| p.pointer("/textDocument/uri"))
        .and_then(Value::as_str)
    else {
        return Value::Array(Vec::new());
    };
    if is_ssa_document(uri) {
        return Value::Array(Vec::new());
    }
    let Some(text) = state.documents.get(uri) else {
        return Value::Array(Vec::new());
    };
    let Ok(formatted) = knr_core::format_tsk(text) else {
        return Value::Array(Vec::new());
    };
    if &formatted == text {
        return Value::Array(Vec::new());
    }
    let line_count = text.lines().count();
    let ends_with_nl = text.ends_with('\n');
    let end_line = if ends_with_nl {
        line_count
    } else {
        line_count.saturating_sub(1)
    };
    let end_char = if ends_with_nl {
        0
    } else {
        text.lines()
            .last()
            .map(|l| l.encode_utf16().count())
            .unwrap_or(0)
    };
    json!([
        {
            "range": {
                "start": { "line": 0, "character": 0 },
                "end": { "line": end_line, "character": end_char }
            },
            "newText": formatted
        }
    ])
}

/// Computes all deterministic QuickFix CodeActions for `text` across the entire document.
/// Reused by both `knr lsp` and the standalone `knr fix` CLI command.
pub fn compute_all_quickfixes_for_source(uri: &str, text: &str) -> Vec<Value> {
    let mut state = LspState::new();
    let (prog, _) = analyze_document(uri, text);
    state.documents.insert(uri.to_string(), text.to_string());
    if let Some(p) = prog {
        state.programs.insert(uri.to_string(), p);
    }
    let end_line = text.lines().count() + 1;
    let params = json!({
        "textDocument": { "uri": uri },
        "range": {
            "start": { "line": 0, "character": 0 },
            "end": { "line": end_line, "character": 0 }
        },
        "context": { "diagnostics": [], "only": ["quickfix"] }
    });
    handle_code_action(&state, Some(&params))
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// Applies a slice of LSP `TextEdit` JSON objects cleanly to `src`.
pub fn apply_lsp_text_edits(src: &str, edits: &[Value]) -> String {
    let mut line_starts = vec![0usize];
    for (idx, b) in src.bytes().enumerate() {
        if b == b'\n' {
            line_starts.push(idx + 1);
        }
    }
    let pos_to_byte = |line: usize, ch_utf16: usize| -> usize {
        let Some(&start) = line_starts.get(line) else {
            return src.len();
        };
        let end = line_starts.get(line + 1).copied().unwrap_or(src.len());
        let slice = &src[start..end];
        let mut units = 0usize;
        for (off, c) in slice.char_indices() {
            if units >= ch_utf16 {
                return start + off;
            }
            units += c.len_utf16();
        }
        end
    };

    let mut parsed: Vec<(usize, usize, String)> = Vec::new();
    for edit in edits {
        let Some(sl) = edit.pointer("/range/start/line").and_then(Value::as_u64) else {
            continue;
        };
        let Some(sc) = edit
            .pointer("/range/start/character")
            .and_then(Value::as_u64)
        else {
            continue;
        };
        let Some(el) = edit.pointer("/range/end/line").and_then(Value::as_u64) else {
            continue;
        };
        let Some(ec) = edit.pointer("/range/end/character").and_then(Value::as_u64) else {
            continue;
        };
        let new_text = edit
            .get("newText")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let s_off = pos_to_byte(sl as usize, sc as usize);
        let e_off = pos_to_byte(el as usize, ec as usize).max(s_off);
        parsed.push((s_off, e_off, new_text));
    }

    parsed.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| b.1.cmp(&a.1)));
    let mut out = src.to_string();
    for (s, e, rep) in parsed {
        if s <= e && e <= out.len() {
            out.replace_range(s..e, &rep);
        }
    }
    out
}

fn ranges_overlap_by_line(a: &Value, b: &Value) -> bool {
    let ((a_sl, _), (a_el, _)) = extract_range_bounds(a);
    let ((b_sl, _), (b_el, _)) = extract_range_bounds(b);
    a_sl <= b_el && b_sl <= a_el
}

fn is_shader_effect_range_match(
    diag: &AnalyzedDiagnostic,
    prog: Option<&Program>,
    text: &str,
    req_range: &Value,
) -> bool {
    if diag.code != "ShaderEffectForbidden" {
        return false;
    }
    let Some(prog) = prog else {
        return false;
    };
    let Some(f) = find_shader_effect_target(prog, diag) else {
        return false;
    };
    let fn_header_line = f.span.line.saturating_sub(1) as u32;
    let fn_header_range = make_lsp_range(fn_header_line, 0, fn_header_line, 0);
    if ranges_overlap_by_line(req_range, &fn_header_range) {
        return true;
    }
    if let Some((eff_start, eff_end)) = find_effects_decorator_bytes(text, f) {
        let (sl, sc) = byte_offset_to_lsp_pos(text, eff_start);
        let (el, ec) = byte_offset_to_lsp_pos(text, eff_end);
        let eff_range = make_lsp_range(sl, sc, el, ec);
        if ranges_overlap_by_line(req_range, &eff_range) {
            return true;
        }
    }
    false
}

fn build_quickfixes_for_diagnostic(
    state: &LspState,
    uri: &str,
    text: &str,
    prog: Option<&Program>,
    diag: &AnalyzedDiagnostic,
) -> Vec<Value> {
    let mut fixes = Vec::new();

    // 1. UseAfterMove / linear param consumed -> Borrow parameter `<name>` as `readonly`
    if diag.code == "UseAfterMove" {
        if let Some(action) = quickfix_use_after_move(uri, text, prog, diag) {
            fixes.push(action);
        }
    }

    // 2. UndeclaredEffect -> Add `@effects("<eff>")` to `<fn>` (or append to existing `@effects(...)`)
    if diag.code == "UndeclaredEffect" {
        if let Some(action) = quickfix_undeclared_effect(uri, text, prog, diag) {
            fixes.push(action);
        }
    }

    // 3. UnknownSchema for Style/Widget/Vec2/Color or unknown builtin JSX component -> import std/ui
    if is_missing_std_ui_diagnostic(diag) {
        if let Some(action) = quickfix_import_std_ui(uri, prog, diag) {
            fixes.push(action);
        }
    }

    // 4. ShaderEffectForbidden -> Remove `@effects(...)` from `@shader` function
    if diag.code == "ShaderEffectForbidden" {
        if let Some(action) = quickfix_shader_effect_forbidden(uri, text, prog, diag) {
            fixes.push(action);
        }
    }

    // 5. BorrowEscape (from verifier or lowerer) -> Change parameter `<name>` from `readonly` to owned
    if is_borrow_escape_diagnostic(diag) {
        if let Some(action) = quickfix_borrow_escape(uri, text, prog, diag) {
            fixes.push(action);
        }
    }

    // 6. Auto-import local symbol declared in a sibling `.tsk` / `.knr` file
    if matches!(
        diag.code.as_str(),
        "UnknownSchema" | "UnknownFunction" | "UndefinedVariable" | "LowerError"
    ) {
        for action in quickfix_auto_import_local(state, uri, text, prog, diag) {
            fixes.push(action);
        }
    }

    fixes
}

fn quickfix_auto_import_local(
    state: &LspState,
    uri: &str,
    text: &str,
    prog: Option<&Program>,
    diag: &AnalyzedDiagnostic,
) -> Vec<Value> {
    let Some(missing_sym) = extract_single_quoted_ident(&diag.message) else {
        return Vec::new();
    };
    let Some(cur_path) = uri_to_path(uri) else {
        return Vec::new();
    };
    let cur_norm = normalize_path(&cur_path);
    let Some(cur_dir) = cur_norm.parent() else {
        return Vec::new();
    };

    let mut candidates = discover_project_source_files(uri);
    for open_uri in state.documents.keys() {
        if let Some(p) = uri_to_path(open_uri) {
            let norm = normalize_path(&p);
            if knr_core::is_source_file(&norm) && !candidates.contains(&norm) {
                candidates.push(norm);
            }
        }
    }
    candidates.sort();

    let mut actions = Vec::new();
    for cand in candidates {
        if cand == cur_norm {
            continue;
        }
        let cand_uri = path_to_uri(&cand);
        let cand_text = state
            .documents
            .get(&cand_uri)
            .cloned()
            .or_else(|| std::fs::read_to_string(&cand).ok());
        let Some(cand_text) = cand_text else {
            continue;
        };
        let Ok(cand_prog) = knr_core::syntax::parse_tsx(&cand_text) else {
            continue;
        };
        if find_top_level_decl_in_program(&cand_prog, &missing_sym).is_none() {
            continue;
        }
        let Some(rel_spec) = relative_import_spec(cur_dir, &cand) else {
            continue;
        };

        // Check if current file already has a named import from this module; if so, append to it
        let existing_import = prog.and_then(|p| {
            p.imports.iter().find(|imp| {
                matches!(imp.kind, ImportKind::Named(_))
                    && resolve_local_import_targets(state, uri, &imp.path)
                        .is_some_and(|(ru, _)| uris_match(&ru, &cand_uri))
            })
        });

        if let Some(imp) = existing_import {
            let imp_slice = text.get(imp.span.start..imp.span.end.min(text.len())).unwrap_or("");
            if let Some(brace_rel) = imp_slice.rfind('}') {
                let brace_abs = imp.span.start + brace_rel;
                let before_brace = &text[imp.span.start..brace_abs];
                let insert_pos = before_brace.trim_end().len() + imp.span.start;
                let (il, ic) = byte_offset_to_lsp_pos(text, insert_pos);
                let new_text = if before_brace.trim_end().ends_with('{') {
                    format!(" {missing_sym} ")
                } else {
                    format!(", {missing_sym}")
                };
                let edit = json!({
                    "range": make_lsp_range(il, ic, il, ic),
                    "newText": new_text
                });
                actions.push(make_quickfix_action(
                    format!("Add '{missing_sym}' to import from \"{}\"", imp.path),
                    uri,
                    vec![edit],
                    diag,
                ));
                continue;
            }
        }

        // Otherwise insert a new `import { sym } from "./...";` after the last import or at line 0
        let insert_line = prog
            .and_then(|p| p.imports.last())
            .map(|last| byte_offset_to_lsp_pos(text, last.span.end).0 + 1)
            .unwrap_or(0);
        let edit = json!({
            "range": make_lsp_range(insert_line, 0, insert_line, 0),
            "newText": format!("import {{ {missing_sym} }} from \"{rel_spec}\";\n")
        });
        actions.push(make_quickfix_action(
            format!("Import '{missing_sym}' from \"{rel_spec}\""),
            uri,
            vec![edit],
            diag,
        ));
    }
    actions
}

fn extract_single_quoted_ident(msg: &str) -> Option<String> {
    let mut rest = msg;
    while let Some(start) = rest.find('\'') {
        let after = &rest[start + 1..];
        let Some(end) = after.find('\'') else {
            break;
        };
        let cand = &after[..end];
        if !cand.is_empty()
            && cand
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
            && cand.chars().all(is_ident_char)
        {
            return Some(cand.to_string());
        }
        rest = &after[end + 1..];
    }
    None
}

fn make_quickfix_action(title: String, uri: &str, edits: Vec<Value>, diag: &AnalyzedDiagnostic) -> Value {
    json!({
        "title": title,
        "kind": "quickfix",
        "isPreferred": true,
        "diagnostics": [diag.lsp_diag],
        "edit": {
            "changes": {
                uri: edits
            }
        }
    })
}

fn quickfix_use_after_move(
    uri: &str,
    text: &str,
    prog: Option<&Program>,
    diag: &AnalyzedDiagnostic,
) -> Option<Value> {
    let prog = prog?;
    let func = resolve_diag_function(prog, text, diag)?;

    let var_name = diag
        .source_variable
        .as_deref()
        .filter(|v| !v.starts_with('%'))
        .map(str::to_string)
        .or_else(|| extract_variable_name_from_message(&diag.message));

    let param = if let Some(ref name) = var_name {
        func.params
            .iter()
            .find(|p| p.name.name == *name && !p.is_borrowed)
    } else {
        None
    }
    .or_else(|| {
        let reg_idx = diag
            .violated_register
            .as_deref()
            .and_then(parse_register_index)
            .or_else(|| {
                diag.source_variable
                    .as_deref()
                    .and_then(parse_register_index)
            })
            .or_else(|| extract_register_index_from_message(&diag.message))?;
        func.params.get(reg_idx).filter(|p| !p.is_borrowed)
    })?;

    if param.name.span.end > param.ty.span.start || param.ty.span.start > text.len() {
        return None;
    }

    let (sl, sc) = byte_offset_to_lsp_pos(text, param.name.span.end);
    let (el, ec) = byte_offset_to_lsp_pos(text, param.ty.span.start);
    let edit = json!({
        "range": make_lsp_range(sl, sc, el, ec),
        "newText": ": readonly "
    });

    Some(make_quickfix_action(
        format!("Borrow parameter '{}' as 'readonly'", param.name.name),
        uri,
        vec![edit],
        diag,
    ))
}

fn quickfix_undeclared_effect(
    uri: &str,
    text: &str,
    prog: Option<&Program>,
    diag: &AnalyzedDiagnostic,
) -> Option<Value> {
    let prog = prog?;
    let eff = extract_missing_effect(&diag.message)?;
    let func = resolve_diag_function(prog, text, diag)?;

    let title = format!("Add @effects(\"{eff}\") to '{}'", func.name.name);

    // If the function already has an `@effects(...)` decorator, append to it
    if let Some((eff_start, eff_end)) = find_effects_decorator_bytes(text, func) {
        let dec_slice = &text[eff_start..eff_end];
        if let (Some(open_rel), Some(close_rel)) = (dec_slice.find('('), dec_slice.rfind(')')) {
            let inner = dec_slice[open_rel + 1..close_rel].trim();
            let close_abs = eff_start + close_rel;
            let (cl, cc) = byte_offset_to_lsp_pos(text, close_abs);
            let insert_text = if inner.is_empty() {
                format!("\"{eff}\"")
            } else {
                format!(", \"{eff}\"")
            };
            let edit = json!({
                "range": make_lsp_range(cl, cc, cl, cc),
                "newText": insert_text
            });
            return Some(make_quickfix_action(title, uri, vec![edit], diag));
        }
    }

    // Otherwise insert `@effects("<eff>")\n` right above the function declaration
    let fn_line = func.span.line.saturating_sub(1);
    let line_str = text.split('\n').nth(fn_line).unwrap_or("");
    let indent: String = line_str
        .chars()
        .take_while(|c| *c == ' ' || *c == '\t')
        .collect();
    let insert_line = fn_line as u32;
    let edit = json!({
        "range": make_lsp_range(insert_line, 0, insert_line, 0),
        "newText": format!("{indent}@effects(\"{eff}\")\n")
    });

    Some(make_quickfix_action(title, uri, vec![edit], diag))
}

fn is_missing_std_ui_diagnostic(diag: &AnalyzedDiagnostic) -> bool {
    if diag.code == "UnknownSchema" {
        return mentions_any_word(&diag.message, &["Style", "Widget", "Vec2", "Color"])
            || diag.message.contains("std/ui");
    }
    if diag.code == "UnknownFunction" {
        if JSX_COMPONENTS
            .iter()
            .any(|c| mentions_any_word(&diag.message, &[c.tag]))
        {
            return true;
        }
        return mentions_any_word(&diag.message, &["style_default", "style_card"]);
    }
    false
}

fn quickfix_import_std_ui(
    uri: &str,
    prog: Option<&Program>,
    diag: &AnalyzedDiagnostic,
) -> Option<Value> {
    if let Some(p) = prog {
        if p.imports
            .iter()
            .any(|i| i.path == "std/ui" || i.path == "std/ui.knr" || i.path == "std/ui.tsk")
        {
            return None;
        }
    }

    let edit = json!({
        "range": make_lsp_range(0, 0, 0, 0),
        "newText": "import { Column, Row, Card, Text, Button } from \"std/ui\";\n"
    });
    Some(make_quickfix_action(
        "Import UI primitives from \"std/ui\"".to_string(),
        uri,
        vec![edit],
        diag,
    ))
}

fn quickfix_shader_effect_forbidden(
    uri: &str,
    text: &str,
    prog: Option<&Program>,
    diag: &AnalyzedDiagnostic,
) -> Option<Value> {
    let prog = prog?;
    let func = find_shader_effect_target(prog, diag)?;
    let (eff_start, eff_end) = find_effects_decorator_bytes(text, func)?;

    let (start_line, _) = byte_offset_to_lsp_pos(text, eff_start);
    let (end_line, _) = byte_offset_to_lsp_pos(text, eff_end);

    let lines: Vec<&str> = text.split('\n').collect();
    let delete_range = if start_line == end_line {
        let line_idx = start_line as usize;
        let line_raw = lines.get(line_idx).copied().unwrap_or("");
        let trimmed = line_raw.trim();
        let dec_text = text[eff_start..eff_end].trim();
        if trimmed == dec_text {
            // Decorator occupies its own line -> delete the entire line including newline
            if line_idx + 1 < lines.len() {
                make_lsp_range(start_line, 0, start_line + 1, 0)
            } else {
                let line_utf16: u32 = line_raw
                    .trim_end_matches('\r')
                    .chars()
                    .map(|c| c.len_utf16() as u32)
                    .sum();
                make_lsp_range(start_line, 0, start_line, line_utf16)
            }
        } else {
            // Inline decorator on same line as other code -> delete decorator + optional trailing space
            let mut del_end = eff_end;
            if text.as_bytes().get(del_end) == Some(&b' ') {
                del_end += 1;
            }
            let (sl, sc) = byte_offset_to_lsp_pos(text, eff_start);
            let (el, ec) = byte_offset_to_lsp_pos(text, del_end);
            make_lsp_range(sl, sc, el, ec)
        }
    } else {
        // Multi-line decorator -> delete full spanned lines
        let end_idx = end_line as usize;
        if end_idx + 1 < lines.len() {
            make_lsp_range(start_line, 0, end_line + 1, 0)
        } else {
            let (el, ec) = byte_offset_to_lsp_pos(text, eff_end);
            make_lsp_range(start_line, 0, el, ec)
        }
    };

    let edit = json!({
        "range": delete_range,
        "newText": ""
    });

    Some(make_quickfix_action(
        "Remove @effects(...) from @shader function".to_string(),
        uri,
        vec![edit],
        diag,
    ))
}

fn is_borrow_escape_diagnostic(diag: &AnalyzedDiagnostic) -> bool {
    diag.code == "BorrowEscape"
        || (diag.code == "InvalidConstruct"
            && diag
                .message
                .to_ascii_lowercase()
                .contains("borrowed parameter"))
}

fn quickfix_borrow_escape(
    uri: &str,
    text: &str,
    prog: Option<&Program>,
    diag: &AnalyzedDiagnostic,
) -> Option<Value> {
    let prog = prog?;
    let func = resolve_diag_function(prog, text, diag)?;

    // Resolve the offending `readonly` parameter
    let by_name = diag
        .source_variable
        .as_deref()
        .filter(|v| !v.starts_with('%'))
        .map(str::to_string)
        .or_else(|| extract_variable_name_from_message(&diag.message))
        .and_then(|name| {
            func.params
                .iter()
                .find(|p| p.name.name == name && p.is_borrowed)
        });

    let by_reg = diag
        .violated_register
        .as_deref()
        .and_then(parse_register_index)
        .or_else(|| extract_register_index_from_message(&diag.message))
        .and_then(|idx| func.params.get(idx).filter(|p| p.is_borrowed));

    let by_line_ident = text
        .split('\n')
        .nth(diag.line_1based.saturating_sub(1))
        .and_then(|line_str| {
            func.params
                .iter()
                .find(|p| p.is_borrowed && mentions_any_word(line_str, &[p.name.name.as_str()]))
        });

    let only_borrowed = {
        let borrowed: Vec<_> = func.params.iter().filter(|p| p.is_borrowed).collect();
        if borrowed.len() == 1 {
            Some(borrowed[0])
        } else {
            None
        }
    };

    let param = by_name.or(by_reg).or(by_line_ident).or(only_borrowed)?;

    if param.name.span.end > param.ty.span.start || param.ty.span.start > text.len() {
        return None;
    }

    let between = &text[param.name.span.end..param.ty.span.start];
    let (del_start, del_end) = if let Some(rel) = between.find("readonly") {
        let abs_start = param.name.span.end + rel;
        let mut abs_end = abs_start + "readonly".len();
        while abs_end < param.ty.span.start {
            match text.as_bytes().get(abs_end) {
                Some(b' ') | Some(b'\t') => abs_end += 1,
                _ => break,
            }
        }
        (abs_start, abs_end)
    } else {
        let rel = between.find('&')?;
        let abs_start = param.name.span.end + rel;
        let mut abs_end = abs_start + 1;
        while abs_end < param.ty.span.start {
            match text.as_bytes().get(abs_end) {
                Some(b' ') | Some(b'\t') => abs_end += 1,
                _ => break,
            }
        }
        (abs_start, abs_end)
    };

    let (sl, sc) = byte_offset_to_lsp_pos(text, del_start);
    let (el, ec) = byte_offset_to_lsp_pos(text, del_end);
    let edit = json!({
        "range": make_lsp_range(sl, sc, el, ec),
        "newText": ""
    });

    Some(make_quickfix_action(
        format!("Change parameter '{}' from 'readonly' to owned", param.name.name),
        uri,
        vec![edit],
        diag,
    ))
}

fn find_function_by_name<'a>(prog: &'a Program, name: &str) -> Option<&'a FunctionDef> {
    for decl in &prog.declarations {
        if let Declaration::Function(f) = decl {
            if f.name.name == name {
                return Some(f);
            }
        }
    }
    None
}

fn find_shader_effect_target<'a>(
    prog: &'a Program,
    diag: &AnalyzedDiagnostic,
) -> Option<&'a FunctionDef> {
    if let Some(ref fn_name) = diag.function {
        if let Some(f) = find_function_by_name(prog, fn_name) {
            if f.is_shader && !f.effects.is_empty() {
                return Some(f);
            }
        }
    }
    for decl in &prog.declarations {
        if let Declaration::Function(f) = decl {
            if f.is_shader && !f.effects.is_empty() {
                return Some(f);
            }
        }
    }
    None
}

fn resolve_diag_function<'a>(
    prog: &'a Program,
    text: &str,
    diag: &AnalyzedDiagnostic,
) -> Option<&'a FunctionDef> {
    if let Some(ref fn_name) = diag.function {
        if let Some(f) = find_function_by_name(prog, fn_name) {
            return Some(f);
        }
    }
    if let Some(fn_name) = extract_fn_name_from_message(&diag.message) {
        if let Some(f) = find_function_by_name(prog, &fn_name) {
            return Some(f);
        }
    }
    for decl in &prog.declarations {
        if let Declaration::Function(f) = decl {
            let start_line = f.span.line;
            let end_line = (byte_offset_to_lsp_pos(text, f.span.end).0 as usize) + 1;
            if start_line <= diag.line_1based && diag.line_1based <= end_line {
                return Some(f);
            }
        }
    }
    None
}

/// Finds the byte range `(start, end)` of an `@effects(...)` or `@effect(...)` decorator attached to `f`.
fn find_effects_decorator_bytes(text: &str, f: &FunctionDef) -> Option<(usize, usize)> {
    let fn_line_idx = f.span.line.saturating_sub(1);
    let line_starts = compute_line_starts(text);
    let fn_line_start = *line_starts.get(fn_line_idx)?;
    let fn_byte_end = f.span.start.min(text.len());

    // 1. Check inline decorators on the same line right before `function`
    if fn_line_start <= fn_byte_end {
        if let Some(bounds) = find_effects_call_in_slice(text, fn_line_start, fn_byte_end) {
            return Some(bounds);
        }
    }

    // 2. Scan contiguous decorator lines directly above the function header
    let mut cur_line = fn_line_idx;
    while cur_line > 0 {
        cur_line -= 1;
        let l_start = line_starts[cur_line];
        let l_end = line_starts
            .get(cur_line + 1)
            .copied()
            .unwrap_or(text.len());
        let line_text = text[l_start..l_end].trim();
        if line_text.is_empty() || !line_text.starts_with('@') {
            break;
        }
        if let Some(bounds) = find_effects_call_in_slice(text, l_start, fn_byte_end) {
            return Some(bounds);
        }
    }

    None
}

fn find_effects_call_in_slice(text: &str, slice_start: usize, search_limit: usize) -> Option<(usize, usize)> {
    let limit = search_limit.min(text.len());
    if slice_start >= limit {
        return None;
    }
    let slice = &text[slice_start..limit];
    for needle in ["@effects(", "@effect("] {
        if let Some(rel) = slice.find(needle) {
            let abs_start = slice_start + rel;
            let bytes = text.as_bytes();
            let mut i = abs_start + needle.len();
            let mut in_quote: Option<u8> = None;
            while i < limit {
                let b = bytes[i];
                if let Some(q) = in_quote {
                    if b == b'\\' {
                        i += 2;
                        continue;
                    }
                    if b == q {
                        in_quote = None;
                    }
                } else if b == b'"' || b == b'\'' {
                    in_quote = Some(b);
                } else if b == b')' {
                    return Some((abs_start, i + 1));
                }
                i += 1;
            }
        }
    }
    None
}

fn compute_line_starts(text: &str) -> Vec<usize> {
    let mut starts = vec![0usize];
    for (idx, b) in text.bytes().enumerate() {
        if b == b'\n' {
            starts.push(idx + 1);
        }
    }
    starts
}

fn extract_missing_effect(msg: &str) -> Option<&'static str> {
    let patterns = [
        "undeclared effect '",
        "with effect ",
        "requiring effect ",
        "effects(",
    ];
    for pat in patterns {
        if let Some(pos) = msg.find(pat) {
            let rest = &msg[pos + pat.len()..];
            let token: String = rest
                .chars()
                .take_while(|c| c.is_ascii_alphabetic())
                .collect();
            if let Some(canonical) = normalize_effect_token(&token) {
                return Some(canonical);
            }
        }
    }
    None
}

fn normalize_effect_token(tok: &str) -> Option<&'static str> {
    match tok.to_ascii_lowercase().as_str() {
        "net" => Some("net"),
        "ui" => Some("ui"),
        "hw" => Some("hw"),
        "async" => Some("async"),
        _ => None,
    }
}

fn extract_variable_name_from_message(msg: &str) -> Option<String> {
    for (prefix, delim) in [("variable '", '\''), ("variable `", '`')] {
        if let Some(pos) = msg.find(prefix) {
            let rest = &msg[pos + prefix.len()..];
            if let Some(end) = rest.find(delim) {
                let candidate = &rest[..end];
                if !candidate.is_empty() && !candidate.starts_with('%') {
                    return Some(candidate.to_string());
                }
            }
        }
    }
    None
}

fn extract_fn_name_from_message(msg: &str) -> Option<String> {
    for prefix in ["Function '", "function '", "Component '"] {
        if let Some(pos) = msg.find(prefix) {
            let rest = &msg[pos + prefix.len()..];
            if let Some(end) = rest.find('\'') {
                let candidate = &rest[..end];
                if !candidate.is_empty() {
                    return Some(candidate.to_string());
                }
            }
        }
    }
    None
}

fn parse_register_index(s: &str) -> Option<usize> {
    s.strip_prefix('%')?.parse::<usize>().ok()
}

fn extract_register_index_from_message(msg: &str) -> Option<usize> {
    let bytes = msg.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let start = i + 1;
            let mut end = start;
            while end < bytes.len() && bytes[end].is_ascii_digit() {
                end += 1;
            }
            if end > start {
                if let Ok(idx) = msg[start..end].parse::<usize>() {
                    return Some(idx);
                }
            }
        }
        i += 1;
    }
    None
}

fn mentions_any_word(haystack: &str, needles: &[&str]) -> bool {
    let bytes = haystack.as_bytes();
    for needle in needles {
        let mut search_from = 0;
        while let Some(rel) = haystack[search_from..].find(needle) {
            let start = search_from + rel;
            let end = start + needle.len();
            let before_ok = start == 0
                || !is_ident_char(haystack[..start].chars().next_back().unwrap_or(' '));
            let after_ok = end >= bytes.len()
                || !is_ident_char(haystack[end..].chars().next().unwrap_or(' '));
            if before_ok && after_ok {
                return true;
            }
            search_from = end;
        }
    }
    false
}

// ============================================================================
// Lexical Scanner, Symbol Position & Path Helpers
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct IdentOccurrence {
    start: usize,
    end: usize,
    preceded_by_dot: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScanMode {
    Normal { brace_depth: usize, in_template_expr: bool },
    TemplateText,
}

/// Scans `text` for all whole-word identifier occurrences matching `target`, skipping `//` and
/// `/* */` comments, plain string literals (`"..."`, `'...'`), hex colors (`#rrggbb`), and static
/// template literal text while scanning live code inside `${ ... }` interpolations.
fn scan_identifier_occurrences(text: &str, target: &str) -> Vec<IdentOccurrence> {
    if target.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let bytes = text.as_bytes();
    let len = bytes.len();
    let mut i = 0usize;
    let mut modes: Vec<ScanMode> = vec![ScanMode::Normal {
        brace_depth: 0,
        in_template_expr: false,
    }];
    let mut prev_non_ws: Option<u8> = None;

    while i < len {
        let mode = *modes.last().unwrap_or(&ScanMode::Normal {
            brace_depth: 0,
            in_template_expr: false,
        });

        match mode {
            ScanMode::TemplateText => {
                let b = bytes[i];
                if b == b'\\' {
                    i = (i + 2).min(len);
                    continue;
                }
                if b == b'`' {
                    modes.pop();
                    prev_non_ws = Some(b'`');
                    i += 1;
                    continue;
                }
                if b == b'$' && i + 1 < len && bytes[i + 1] == b'{' {
                    modes.push(ScanMode::Normal {
                        brace_depth: 0,
                        in_template_expr: true,
                    });
                    prev_non_ws = Some(b'{');
                    i += 2;
                    continue;
                }
                i += 1;
            }
            ScanMode::Normal {
                brace_depth,
                in_template_expr,
            } => {
                let b = bytes[i];

                // Whitespace
                if b.is_ascii_whitespace() {
                    i += 1;
                    continue;
                }

                // Line comment `// ...`
                if b == b'/' && i + 1 < len && bytes[i + 1] == b'/' {
                    i += 2;
                    while i < len && bytes[i] != b'\n' {
                        i += 1;
                    }
                    continue;
                }

                // Block comment `/* ... */`
                if b == b'/' && i + 1 < len && bytes[i + 1] == b'*' {
                    i += 2;
                    while i + 1 < len && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                        i += 1;
                    }
                    i = (i + 2).min(len);
                    continue;
                }

                // Plain string literals `"..."` and `'...'`
                if b == b'"' || b == b'\'' {
                    let quote = b;
                    i += 1;
                    while i < len {
                        if bytes[i] == b'\\' {
                            i = (i + 2).min(len);
                            continue;
                        }
                        if bytes[i] == quote {
                            i += 1;
                            break;
                        }
                        i += 1;
                    }
                    prev_non_ws = Some(quote);
                    continue;
                }

                // Template string literal `` `...` ``
                if b == b'`' {
                    modes.push(ScanMode::TemplateText);
                    i += 1;
                    continue;
                }

                // Hex color literal `#RGB`, `#RRGGBB`, `#RRGGBBAA`
                if b == b'#' {
                    i += 1;
                    while i < len && bytes[i].is_ascii_hexdigit() {
                        i += 1;
                    }
                    prev_non_ws = Some(b'#');
                    continue;
                }

                // Braces tracking for template `${ ... }`
                if b == b'{' {
                    if in_template_expr {
                        if let Some(ScanMode::Normal { brace_depth: d, .. }) = modes.last_mut() {
                            *d += 1;
                        }
                    }
                    prev_non_ws = Some(b'{');
                    i += 1;
                    continue;
                }
                if b == b'}' {
                    if in_template_expr {
                        if brace_depth == 0 {
                            modes.pop();
                        } else if let Some(ScanMode::Normal { brace_depth: d, .. }) =
                            modes.last_mut()
                        {
                            *d -= 1;
                        }
                    }
                    prev_non_ws = Some(b'}');
                    i += 1;
                    continue;
                }

                // Numbers (consume trailing alphanumerics so e.g. `1e5` or `0xdead` aren't split)
                if b.is_ascii_digit() {
                    i += 1;
                    while i < len && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                        i += 1;
                    }
                    prev_non_ws = Some(b'0');
                    continue;
                }

                // Identifier token
                if b.is_ascii_alphabetic() || b == b'_' {
                    let start = i;
                    i += 1;
                    while i < len && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                        i += 1;
                    }
                    if &text[start..i] == target {
                        out.push(IdentOccurrence {
                            start,
                            end: i,
                            preceded_by_dot: prev_non_ws == Some(b'.'),
                        });
                    }
                    prev_non_ws = Some(b'a');
                    continue;
                }

                // Any other ASCII punctuation or multi-byte UTF-8 character
                prev_non_ws = Some(b);
                if b < 0x80 {
                    i += 1;
                } else {
                    let ch_len = text[i..].chars().next().map(char::len_utf8).unwrap_or(1);
                    i += ch_len;
                }
            }
        }
    }

    out
}

fn extract_text_doc_position(params: Option<&Value>) -> Option<(&str, u32, u32)> {
    let params = params?;
    let uri = params.pointer("/textDocument/uri").and_then(Value::as_str)?;
    let line = params
        .pointer("/position/line")
        .and_then(Value::as_u64)? as u32;
    let character = params
        .pointer("/position/character")
        .and_then(Value::as_u64)? as u32;
    Some((uri, line, character))
}

/// Extracts `(ident, start_byte_in_doc, end_byte_in_doc, lsp_range)` at `(line, character)`,
/// falling back one character left when the cursor sits directly at the right edge of an identifier.
fn symbol_at_position(
    text: &str,
    line: u32,
    character: u32,
) -> Option<(String, usize, usize, Value)> {
    let line_starts = compute_line_starts(text);
    let line_start = *line_starts.get(line as usize)?;
    let line_raw_end = line_starts
        .get(line as usize + 1)
        .map(|&next| next.saturating_sub(1))
        .unwrap_or(text.len());
    let line_str = text[line_start..line_raw_end].trim_end_matches('\r');

    let mut byte_idx = utf16_col_to_byte_idx(line_str, character);
    let at_ident = byte_idx < line_str.len()
        && line_str[byte_idx..]
            .chars()
            .next()
            .is_some_and(is_ident_char);

    if !at_ident {
        if byte_idx == 0 {
            return None;
        }
        let prev_char = line_str[..byte_idx].chars().next_back()?;
        if !is_ident_char(prev_char) {
            return None;
        }
        byte_idx -= prev_char.len_utf8();
    }

    let mut start_in_line = byte_idx;
    for (idx, c) in line_str[..byte_idx].char_indices().rev() {
        if is_ident_char(c) {
            start_in_line = idx;
        } else {
            break;
        }
    }

    let mut end_in_line = byte_idx;
    for (rel_idx, c) in line_str[byte_idx..].char_indices() {
        if is_ident_char(c) {
            end_in_line = byte_idx + rel_idx + c.len_utf8();
        } else {
            break;
        }
    }

    let ident = &line_str[start_in_line..end_in_line];
    let first = ident.chars().next()?;
    if !(first.is_ascii_alphabetic() || first == '_') {
        return None;
    }

    let start_utf16: u32 = line_str[..start_in_line]
        .chars()
        .map(|c| c.len_utf16() as u32)
        .sum();
    let end_utf16: u32 = line_str[..end_in_line]
        .chars()
        .map(|c| c.len_utf16() as u32)
        .sum();

    Some((
        ident.to_string(),
        line_start + start_in_line,
        line_start + end_in_line,
        make_lsp_range(line, start_utf16, line, end_utf16),
    ))
}

/// If `ident_start_byte` is immediately preceded by `<Qualifier>.`, returns `Some("Qualifier")`.
fn dot_qualifier_before(text: &str, ident_start_byte: usize) -> Option<String> {
    let prefix = text.get(..ident_start_byte)?.trim_end();
    let before_dot = prefix.strip_suffix('.')?.trim_end();
    let end = before_dot.len();
    let mut start = end;
    for (idx, c) in before_dot.char_indices().rev() {
        if is_ident_char(c) {
            start = idx;
        } else {
            break;
        }
    }
    if start < end {
        let qual = &before_dot[start..end];
        if qual
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        {
            return Some(qual.to_string());
        }
    }
    None
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push(comp.as_os_str());
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

fn path_to_uri(path: &Path) -> String {
    let normalized = normalize_path(path);
    let raw = normalized.to_string_lossy().replace('\\', "/");
    let with_leading_slash = if raw.starts_with('/') {
        raw
    } else {
        format!("/{raw}")
    };
    let mut encoded = String::with_capacity(with_leading_slash.len() + 7);
    encoded.push_str("file://");
    for b in with_leading_slash.bytes() {
        let keep = b.is_ascii_alphanumeric()
            || matches!(b, b'/' | b'-' | b'_' | b'.' | b'~' | b':');
        if keep {
            encoded.push(b as char);
        } else {
            let _ = write!(encoded, "%{b:02X}");
        }
    }
    encoded
}

// ============================================================================
// AST Formatting, Position Conversion & URI Utilities
// ============================================================================

fn program_for_uri(state: &LspState, uri: &str) -> Option<Program> {
    if is_ssa_document(uri) {
        return None;
    }
    if let Some(text) = state.documents.get(uri) {
        if let Ok(prog) = knr_core::syntax::parse_tsx(text) {
            return Some(prog);
        }
    }
    state.programs.get(uri).cloned()
}

fn format_type_expr(ty: &TypeExpr) -> String {
    let base = match &ty.kind {
        TypeExprKind::Named(ident) => ident.name.clone(),
        TypeExprKind::Vec(inner) => return format!("{}[]", format_type_expr(inner)),
        TypeExprKind::Map(k, v) => {
            return format!("Map<{}, {}>", format_type_expr(k), format_type_expr(v));
        }
        TypeExprKind::Chan(inner) => return format!("Channel<{}>", format_type_expr(inner)),
        TypeExprKind::Struct(ident) => ident.name.clone(),
    };
    if ty.type_args.is_empty() {
        base
    } else {
        let args: Vec<String> = ty.type_args.iter().map(format_type_expr).collect();
        format!("{base}<{}>", args.join(", "))
    }
}

fn format_function_signature(f: &FunctionDef) -> String {
    let mut prefix = String::new();
    if f.is_shader {
        prefix.push_str("@shader\n");
    }
    if !f.effects.is_empty() {
        let effs: Vec<String> = f
            .effects
            .iter()
            .map(|e| format!("\"{}\"", e.name.to_ascii_lowercase()))
            .collect();
        let _ = writeln!(prefix, "@effects({})", effs.join(", "));
    }
    let params: Vec<String> = f
        .params
        .iter()
        .map(|p| {
            let borrow = if p.is_borrowed { "readonly " } else { "" };
            format!("{}: {}{}", p.name.name, borrow, format_type_expr(&p.ty))
        })
        .collect();
    let ret = format_type_expr(&f.ret_ty);
    let ret_part = if ret == "void" {
        String::new()
    } else {
        format!(": {ret}")
    };
    format!("{prefix}function {}({}){ret_part}", f.name.name, params.join(", "))
}

fn format_struct_def(s: &StructDef) -> String {
    if s.fields.is_empty() {
        return format!("type {} = {{}};", s.name.name);
    }
    let mut out = format!("type {} = {{\n", s.name.name);
    for field in &s.fields {
        let _ = writeln!(out, "    {}: {};", field.name.name, format_type_expr(&field.ty));
    }
    out.push_str("};");
    out
}

fn format_const_def(c: &ConstDef) -> String {
    if let Some(ref ty) = c.ty {
        format!("const {}: {}", c.name.name, format_type_expr(ty))
    } else {
        format!("const {}", c.name.name)
    }
}

fn format_enum_def(e: &EnumDef) -> String {
    if e.variants.is_empty() {
        return format!("enum {} {{}}", e.name.name);
    }
    let mut out = format!("enum {} {{\n", e.name.name);
    for variant in &e.variants {
        if variant.fields.is_empty() {
            let _ = writeln!(out, "    {},", variant.name.name);
        } else {
            let tys: Vec<String> = variant.fields.iter().map(format_type_expr).collect();
            let _ = writeln!(out, "    {}({}),", variant.name.name, tys.join(", "));
        }
    }
    out.push('}');
    out
}

fn is_ident_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '_'
}

fn utf16_col_to_byte_idx(line_str: &str, target_utf16: u32) -> usize {
    let mut utf16 = 0u32;
    for (byte_idx, ch) in line_str.char_indices() {
        if utf16 >= target_utf16 {
            return byte_idx;
        }
        utf16 += ch.len_utf16() as u32;
    }
    line_str.len()
}

fn byte_offset_to_lsp_pos(text: &str, byte_offset: usize) -> (u32, u32) {
    let clamped = byte_offset.min(text.len());
    let mut line = 0u32;
    let mut col_utf16 = 0u32;
    for (idx, ch) in text.char_indices() {
        if idx >= clamped {
            break;
        }
        if ch == '\n' {
            line += 1;
            col_utf16 = 0;
        } else if ch != '\r' {
            col_utf16 += ch.len_utf16() as u32;
        }
    }
    (line, col_utf16)
}

fn make_lsp_range(start_line: u32, start_char: u32, end_line: u32, end_char: u32) -> Value {
    json!({
        "start": { "line": start_line, "character": start_char },
        "end": { "line": end_line, "character": end_char }
    })
}

fn span_to_lsp_range(text: &str, span: Span) -> Value {
    if span.start <= span.end && span.end <= text.len() && (span.start > 0 || span.end > 0) {
        let (s_line, s_col) = byte_offset_to_lsp_pos(text, span.start);
        let (e_line, e_col) = byte_offset_to_lsp_pos(text, span.end);
        return make_lsp_range(s_line, s_col, e_line, e_col);
    }
    line_col_to_lsp_range(text, span.line, Some(span.col), None)
}

fn extract_range_bounds(range: &Value) -> ((u32, u32), (u32, u32)) {
    let sl = range
        .pointer("/start/line")
        .and_then(Value::as_u64)
        .unwrap_or(0) as u32;
    let sc = range
        .pointer("/start/character")
        .and_then(Value::as_u64)
        .unwrap_or(0) as u32;
    let el = range
        .pointer("/end/line")
        .and_then(Value::as_u64)
        .unwrap_or(sl as u64) as u32;
    let ec = range
        .pointer("/end/character")
        .and_then(Value::as_u64)
        .unwrap_or(sc as u64) as u32;
    ((sl, sc), (el, ec))
}

fn clamp_selection_range(selection: Value, enclosing: &Value) -> Value {
    let (sel_start, sel_end) = extract_range_bounds(&selection);
    let (enc_start, enc_end) = extract_range_bounds(enclosing);
    if sel_start >= enc_start && sel_end <= enc_end && sel_start <= sel_end {
        selection
    } else {
        enclosing.clone()
    }
}

fn line_col_to_lsp_range(
    text: &str,
    line_1based: usize,
    col_1based: Option<usize>,
    span: Option<(usize, usize)>,
) -> Value {
    let expected_line = line_1based.saturating_sub(1) as u32;
    if let Some((start, end)) = span {
        if start <= end && end <= text.len() {
            let (s_line, s_col) = byte_offset_to_lsp_pos(text, start);
            let (e_line, e_col) = byte_offset_to_lsp_pos(text, end);
            if s_line == expected_line {
                let adj_e_col = if s_line == e_line && s_col == e_col {
                    s_col + 1
                } else {
                    e_col
                };
                return make_lsp_range(s_line, s_col, e_line, adj_e_col);
            }
        }
    }

    let lines: Vec<&str> = text.split('\n').collect();
    let target_line = line_1based
        .saturating_sub(1)
        .min(lines.len().saturating_sub(1));
    let line_str = lines
        .get(target_line)
        .copied()
        .unwrap_or("")
        .trim_end_matches('\r');
    let line_utf16_len: u32 = line_str.chars().map(|c| c.len_utf16() as u32).sum();

    if let Some(col) = col_1based {
        let start_char_idx = col.saturating_sub(1);
        let start_utf16: u32 = line_str
            .chars()
            .take(start_char_idx)
            .map(|c| c.len_utf16() as u32)
            .sum();
        let token_utf16: u32 = line_str
            .chars()
            .skip(start_char_idx)
            .take_while(|c| is_ident_char(*c))
            .map(|c| c.len_utf16() as u32)
            .sum();
        let end_utf16 = if token_utf16 > 0 {
            start_utf16 + token_utf16
        } else {
            (start_utf16 + 1).min(line_utf16_len)
        };
        make_lsp_range(
            target_line as u32,
            start_utf16,
            target_line as u32,
            end_utf16,
        )
    } else {
        let leading_ws: u32 = line_str
            .chars()
            .take_while(|c| c.is_whitespace())
            .map(|c| c.len_utf16() as u32)
            .sum();
        let start_utf16 = if leading_ws < line_utf16_len {
            leading_ws
        } else {
            0
        };
        make_lsp_range(
            target_line as u32,
            start_utf16,
            target_line as u32,
            line_utf16_len,
        )
    }
}

#[cfg(test)]
fn identifier_at_position(text: &str, line: u32, character: u32) -> Option<(String, Value)> {
    let line_str = text.split('\n').nth(line as usize)?.trim_end_matches('\r');
    let byte_idx = utf16_col_to_byte_idx(line_str, character);

    if byte_idx >= line_str.len() {
        return None;
    }
    let ch = line_str[byte_idx..].chars().next()?;
    if !is_ident_char(ch) {
        return None;
    }

    let mut start_byte = byte_idx;
    for (idx, c) in line_str[..byte_idx].char_indices().rev() {
        if is_ident_char(c) {
            start_byte = idx;
        } else {
            break;
        }
    }

    let mut end_byte = byte_idx;
    for (rel_idx, c) in line_str[byte_idx..].char_indices() {
        if is_ident_char(c) {
            end_byte = byte_idx + rel_idx + c.len_utf8();
        } else {
            break;
        }
    }

    let ident = &line_str[start_byte..end_byte];
    let first = ident.chars().next()?;
    if !(first.is_ascii_alphabetic() || first == '_') {
        return None;
    }

    let start_utf16: u32 = line_str[..start_byte]
        .chars()
        .map(|c| c.len_utf16() as u32)
        .sum();
    let end_utf16: u32 = line_str[..end_byte]
        .chars()
        .map(|c| c.len_utf16() as u32)
        .sum();
    Some((
        ident.to_string(),
        make_lsp_range(line, start_utf16, line, end_utf16),
    ))
}

/// Converts a `file://` URI into a local filesystem `PathBuf`, handling percent-encoding.
fn uri_to_path(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    let raw_path = if let Some(after_localhost) = rest.strip_prefix("localhost/") {
        format!("/{after_localhost}")
    } else if rest.starts_with('/') {
        rest.to_string()
    } else {
        return None;
    };

    let decoded = percent_decode(&raw_path)?;

    #[cfg(windows)]
    let normalized = {
        let bytes = decoded.as_bytes();
        if bytes.len() >= 3
            && bytes[0] == b'/'
            && bytes[1].is_ascii_alphabetic()
            && bytes[2] == b':'
        {
            decoded[1..].to_string()
        } else {
            decoded
        }
    };
    #[cfg(not(windows))]
    let normalized = decoded;

    if normalized.is_empty() {
        None
    } else {
        Some(PathBuf::from(normalized))
    }
}

fn percent_decode(input: &str) -> Option<String> {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(hi), Some(lo)) = (hex_val(bytes[i + 1]), hex_val(bytes[i + 2])) {
                out.push((hi << 4) | lo);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).ok()
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

// ============================================================================
// Unit Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn framing_roundtrip_handles_utf8_and_extra_headers() {
        let payload = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": { "label": "héllo 🚀" }
        });
        let mut wire = Vec::new();
        write_message(&mut wire, &payload).unwrap();

        // Prepend Content-Type header to verify header parsing ignores extra headers
        let body = serde_json::to_string(&payload).unwrap();
        let extra_frame = format!(
            "Content-Type: application/vscode-jsonrpc; charset=utf-8\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body
        );
        let mut combined = wire;
        combined.extend_from_slice(extra_frame.as_bytes());

        let mut cursor = Cursor::new(combined);
        let first = read_message(&mut cursor).unwrap().unwrap();
        let second = read_message(&mut cursor).unwrap().unwrap();
        let eof = read_message(&mut cursor).unwrap();

        assert_eq!(serde_json::from_str::<Value>(&first).unwrap(), payload);
        assert_eq!(serde_json::from_str::<Value>(&second).unwrap(), payload);
        assert!(eof.is_none());
    }

    #[test]
    fn initialize_shutdown_exit_lifecycle() {
        let mut state = LspState::new();
        let init_out = handle_message(
            &mut state,
            &json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {} }),
        );
        assert_eq!(init_out.len(), 1);
        assert_eq!(init_out[0]["id"], 1);
        assert_eq!(init_out[0]["result"]["capabilities"]["textDocumentSync"], 1);
        assert_eq!(init_out[0]["result"]["capabilities"]["hoverProvider"], true);
        assert_eq!(
            init_out[0]["result"]["capabilities"]["documentSymbolProvider"],
            true
        );
        assert_eq!(
            init_out[0]["result"]["serverInfo"]["name"],
            SERVER_NAME
        );

        let initialized_out = handle_message(
            &mut state,
            &json!({ "jsonrpc": "2.0", "method": "initialized", "params": {} }),
        );
        assert!(initialized_out.is_empty());

        let shutdown_out = handle_message(
            &mut state,
            &json!({ "jsonrpc": "2.0", "id": 2, "method": "shutdown" }),
        );
        assert_eq!(shutdown_out.len(), 1);
        assert!(state.shutdown_requested);
        assert!(shutdown_out[0]["result"].is_null());

        let exit_out = handle_message(&mut state, &json!({ "jsonrpc": "2.0", "method": "exit" }));
        assert!(exit_out.is_empty());
        assert!(state.exit_requested);
    }

    #[test]
    fn diagnostics_for_valid_parse_error_and_type_error() {
        let mut state = LspState::new();
        let uri = "file:///tmp/demo.tsk";

        // 1. Valid Strict TS program -> empty diagnostics
        let valid_src = "export function add(a: i64, b: i64): i64 {\n    return a + b;\n}\n";
        let out = handle_message(
            &mut state,
            &json!({
                "jsonrpc": "2.0",
                "method": "textDocument/didOpen",
                "params": { "textDocument": { "uri": uri, "text": valid_src } }
            }),
        );
        assert_eq!(out.len(), 1);
        assert_eq!(out[0]["method"], "textDocument/publishDiagnostics");
        assert!(out[0]["params"]["diagnostics"].as_array().unwrap().is_empty());

        // 2. Parse error -> non-empty diagnostic with 0-based line
        let bad_syntax = "export function broken( {\n    return 1;\n}\n";
        let out = handle_message(
            &mut state,
            &json!({
                "jsonrpc": "2.0",
                "method": "textDocument/didChange",
                "params": {
                    "textDocument": { "uri": uri },
                    "contentChanges": [{ "text": bad_syntax }]
                }
            }),
        );
        let diags = out[0]["params"]["diagnostics"].as_array().unwrap();
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0]["range"]["start"]["line"], 0);
        assert_eq!(diags[0]["severity"], 1);

        // 3. Verification / lowering type error -> diagnostic on offending line
        let type_err = "export function mismatch(): i64 {\n    return \"not an int\";\n}\n";
        let out = handle_message(
            &mut state,
            &json!({
                "jsonrpc": "2.0",
                "method": "textDocument/didChange",
                "params": {
                    "textDocument": { "uri": uri },
                    "contentChanges": [{ "text": type_err }]
                }
            }),
        );
        let diags = out[0]["params"]["diagnostics"].as_array().unwrap();
        assert!(!diags.is_empty());
        assert_eq!(diags[0]["range"]["start"]["line"], 1);

        // 4. Close clears diagnostics
        let out = handle_message(
            &mut state,
            &json!({
                "jsonrpc": "2.0",
                "method": "textDocument/didClose",
                "params": { "textDocument": { "uri": uri } }
            }),
        );
        assert!(out[0]["params"]["diagnostics"].as_array().unwrap().is_empty());
        assert!(!state.documents.contains_key(uri));
    }

    #[test]
    fn hover_completion_and_document_symbols() {
        let mut state = LspState::new();
        let uri = "file:///tmp/shader_demo.tsk";
        let src = concat!(
            "export type Vec2 = { x: f64; y: f64; };\n",
            "export type Color = { r: f64; g: f64; b: f64; a: f64; };\n",
            "export enum Mode { Calm, Hyper(f64) }\n",
            "export const ACCENT: string = \"#38bdf8\";\n",
            "@shader\n",
            "export function aurora_waves(uv: Vec2, time: f64): Color {\n",
            "    const s = smoothstep(0.0, 1.0, sin(uv.x + time));\n",
            "    return { r: s, g: uv.y, b: 0.8, a: 1.0 };\n",
            "}\n",
        );

        let open_out = handle_message(
            &mut state,
            &json!({
                "jsonrpc": "2.0",
                "method": "textDocument/didOpen",
                "params": { "textDocument": { "uri": uri, "text": src } }
            }),
        );
        assert!(
            open_out[0]["params"]["diagnostics"]
                .as_array()
                .unwrap()
                .is_empty(),
            "Valid TSX shader file should produce zero diagnostics, got: {:?}",
            open_out[0]["params"]["diagnostics"]
        );

        // Hover on `aurora_waves` (line 5, char 18)
        let hover_fn = handle_message(
            &mut state,
            &json!({
                "jsonrpc": "2.0",
                "id": 10,
                "method": "textDocument/hover",
                "params": {
                    "textDocument": { "uri": uri },
                    "position": { "line": 5, "character": 18 }
                }
            }),
        );
        let hover_text = hover_fn[0]["result"]["contents"]["value"].as_str().unwrap();
        assert!(hover_text.contains("@shader"));
        assert!(hover_text.contains("function aurora_waves(uv: Vec2, time: f64): Color"));

        // Hover on builtin `smoothstep` (line 6, char 16)
        let hover_builtin = handle_message(
            &mut state,
            &json!({
                "jsonrpc": "2.0",
                "id": 11,
                "method": "textDocument/hover",
                "params": {
                    "textDocument": { "uri": uri },
                    "position": { "line": 6, "character": 16 }
                }
            }),
        );
        let builtin_text = hover_builtin[0]["result"]["contents"]["value"]
            .as_str()
            .unwrap();
        assert!(builtin_text.contains("smoothstep"));

        // Completions include local decls, JSX tags & props, decorators, builtins, 45 Style fields, and Widget fields
        let comp = handle_message(
            &mut state,
            &json!({
                "jsonrpc": "2.0",
                "id": 12,
                "method": "textDocument/completion",
                "params": {
                    "textDocument": { "uri": uri },
                    "position": { "line": 6, "character": 8 }
                }
            }),
        );
        let labels: HashSet<&str> = comp[0]["result"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|item| item.get("label").and_then(Value::as_str))
            .collect();
        for expected in [
            "aurora_waves",
            "Vec2",
            "Mode",
            "ACCENT",
            "@shader",
            "@effects(\"net\")",
            "Column",
            "Row",
            "Grid",
            "Card",
            "Code",
            "Shader",
            "Icon",
            "colSpan",
            "onClick",
            "smoothstep",
            "backdrop_blur",
            "hover_scale",
            "font_family",
            "text_align",
            "line_height",
            "col_span",
            "letter_spacing",
            "uniforms",
            "children",
        ] {
            assert!(labels.contains(expected), "Missing completion: {expected}");
        }

        // Document symbols include Vec2, Color, Mode, ACCENT, and aurora_waves with accurate kinds
        let syms = handle_message(
            &mut state,
            &json!({
                "jsonrpc": "2.0",
                "id": 13,
                "method": "textDocument/documentSymbol",
                "params": { "textDocument": { "uri": uri } }
            }),
        );
        let sym_list = syms[0]["result"].as_array().unwrap();
        assert_eq!(sym_list.len(), 5);
        assert_eq!(sym_list[0]["name"], "Vec2");
        assert_eq!(sym_list[0]["kind"], SYMBOL_KIND_STRUCT);
        assert_eq!(sym_list[2]["name"], "Mode");
        assert_eq!(sym_list[2]["kind"], SYMBOL_KIND_ENUM);
        assert_eq!(sym_list[3]["name"], "ACCENT");
        assert_eq!(sym_list[3]["kind"], SYMBOL_KIND_CONSTANT);
        assert_eq!(sym_list[4]["name"], "aurora_waves");
        assert_eq!(sym_list[4]["kind"], SYMBOL_KIND_FUNCTION);
    }

    #[test]
    fn uri_to_path_decodes_file_uris_safely() {
        assert_eq!(
            uri_to_path("file:///home/dev/my%20app/app.tsk"),
            Some(PathBuf::from("/home/dev/my app/app.tsk"))
        );
        assert_eq!(
            uri_to_path("file://localhost/tmp/test.tsk"),
            Some(PathBuf::from("/tmp/test.tsk"))
        );
        assert_eq!(uri_to_path("untitled:Untitled-1"), None);
    }

    #[test]
    fn identifier_lookup_is_utf8_safe() {
        let src = "const msg = \"🚀 café\"; const alpha = sin(1.0);";
        for col in 0..60 {
            let _ = identifier_at_position(src, 0, col);
        }
        // `sin` starts at UTF-16 column 37
        let (ident, range) = identifier_at_position(src, 0, 37).unwrap();
        assert_eq!(ident, "sin");
        assert_eq!(range["start"]["character"], 37);
        assert_eq!(range["end"]["character"], 40);
    }

    #[test]
    fn shader_compile_error_and_did_save_publish_diagnostics() {
        let mut state = LspState::new();
        let uri = "file:///tmp/bad_shader.tsk";
        let src = concat!(
            "export type Vec2 = { x: f64; y: f64; };\n",
            "@shader\n",
            "export function bad_ret(uv: Vec2, time: f64): i64 {\n",
            "    return 1;\n",
            "}\n",
        );

        let open_out = handle_message(
            &mut state,
            &json!({
                "jsonrpc": "2.0",
                "method": "textDocument/didOpen",
                "params": { "textDocument": { "uri": uri, "text": src } }
            }),
        );
        let diags = open_out[0]["params"]["diagnostics"].as_array().unwrap();
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0]["code"], "ShaderCompileError");
        assert_eq!(diags[0]["range"]["start"]["line"], 2);

        let save_out = handle_message(
            &mut state,
            &json!({
                "jsonrpc": "2.0",
                "method": "textDocument/didSave",
                "params": { "textDocument": { "uri": uri } }
            }),
        );
        assert_eq!(save_out.len(), 1);
        assert_eq!(save_out[0]["method"], "textDocument/publishDiagnostics");
    }

    /// Helper that applies LSP `TextEdit[]` (in reverse offset order) to a source string.
    fn apply_lsp_edits(src: &str, edits: &[Value]) -> String {
        let mut resolved: Vec<(usize, usize, String)> = edits
            .iter()
            .map(|e| {
                let ((sl, sc), (el, ec)) = extract_range_bounds(&e["range"]);
                let s_byte = lsp_pos_to_byte(src, sl, sc);
                let e_byte = lsp_pos_to_byte(src, el, ec);
                let new_text = e["newText"].as_str().unwrap_or("").to_string();
                (s_byte, e_byte, new_text)
            })
            .collect();
        resolved.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| b.1.cmp(&a.1)));
        let mut out = src.to_string();
        for (start, end, new_text) in resolved {
            out.replace_range(start..end, &new_text);
        }
        out
    }

    fn lsp_pos_to_byte(src: &str, line: u32, col_utf16: u32) -> usize {
        let starts = compute_line_starts(src);
        let Some(&line_start) = starts.get(line as usize) else {
            return src.len();
        };
        let line_end = starts
            .get(line as usize + 1)
            .map(|&next| next.saturating_sub(1))
            .unwrap_or(src.len());
        let line_str = src[line_start..line_end].trim_end_matches('\r');
        line_start + utf16_col_to_byte_idx(line_str, col_utf16)
    }

    fn open_doc(state: &mut LspState, uri: &str, text: &str) -> Vec<Value> {
        handle_message(
            state,
            &json!({
                "jsonrpc": "2.0",
                "method": "textDocument/didOpen",
                "params": { "textDocument": { "uri": uri, "text": text } }
            }),
        )
    }

    #[test]
    fn initialize_advertises_ide_pro_capabilities() {
        let mut state = LspState::new();
        let out = handle_message(
            &mut state,
            &json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {} }),
        );
        let caps = &out[0]["result"]["capabilities"];
        assert_eq!(caps["textDocumentSync"], 1);
        assert_eq!(caps["hoverProvider"], true);
        assert!(caps["completionProvider"]["triggerCharacters"].is_array());
        assert_eq!(caps["documentSymbolProvider"], true);
        assert_eq!(caps["definitionProvider"], true);
        assert_eq!(caps["referencesProvider"], true);
        assert_eq!(caps["renameProvider"]["prepareProvider"], true);
        assert_eq!(
            caps["codeActionProvider"]["codeActionKinds"],
            json!(["quickfix"])
        );
    }

    #[test]
    fn definition_local_same_file_jsx_and_cross_file_imports() {
        let mut state = LspState::new();

        // Create temporary directory with cross-file `.tsk` modules on disk
        let tmp_dir = std::env::temp_dir().join(format!(
            "knr_lsp_def_test_{}",
            std::process::id()
        ));
        let comp_dir = tmp_dir.join("components");
        std::fs::create_dir_all(&comp_dir).unwrap();

        let state_path = tmp_dir.join("state.tsk");
        let state_src = concat!(
            "export type State = {\n",
            "    count: i64;\n",
            "};\n",
            "export const STEP: i64 = 1;\n",
        );
        std::fs::write(&state_path, state_src).unwrap();

        let navbar_path = comp_dir.join("navbar.tsk");
        let navbar_src = concat!(
            "import { Row, Text } from \"std/ui\";\n",
            "import { State } from \"../state.tsk\";\n",
            "export function Navbar(s: readonly State): Widget {\n",
            "    return <Row><Text>{`Count: ${s.count}`}</Text></Row>;\n",
            "}\n",
        );
        std::fs::write(&navbar_path, navbar_src).unwrap();

        let app_path = tmp_dir.join("app.tsk");
        let app_src = concat!(
            "import { Column, Text } from \"std/ui\";\n",
            "import { State, STEP } from \"./state.tsk\";\n",
            "import { Navbar } from \"./components/navbar.tsk\";\n",
            "export enum Tab { Home, Lab }\n",
            "export function Hero(s: readonly State): Widget {\n",
            "    const label = \"Hero\";\n",
            "    return <Text>{label}</Text>;\n",
            "}\n",
            "export function view(s: readonly State): Widget {\n",
            "    const bonus = STEP;\n",
            "    const next = s.count + bonus;\n",
            "    return <Column><Navbar s={s} /><Hero s={s} /><Text>{`${next}`}</Text></Column>;\n",
            "}\n",
        );
        std::fs::write(&app_path, app_src).unwrap();

        let app_uri = path_to_uri(&app_path);
        let state_uri = path_to_uri(&state_path);
        let navbar_uri = path_to_uri(&navbar_path);
        open_doc(&mut state, &app_uri, app_src);

        // 1. Local `const bonus` usage on line 10 (`const next = s.count + bonus;`) -> line 9
        let bonus_col = app_src.lines().nth(10).unwrap().find("bonus").unwrap() as u32;
        let def_local = handle_message(
            &mut state,
            &json!({
                "jsonrpc": "2.0",
                "id": 100,
                "method": "textDocument/definition",
                "params": {
                    "textDocument": { "uri": app_uri },
                    "position": { "line": 10, "character": bonus_col + 1 }
                }
            }),
        );
        assert_eq!(def_local[0]["result"]["uri"], app_uri);
        assert_eq!(def_local[0]["result"]["range"]["start"]["line"], 9);

        // 2. Enclosing function parameter `s` inside `view` (line 10: `s.count`) -> line 8
        let s_col = app_src.lines().nth(10).unwrap().find("s.count").unwrap() as u32;
        let def_param = handle_message(
            &mut state,
            &json!({
                "jsonrpc": "2.0",
                "id": 101,
                "method": "textDocument/definition",
                "params": {
                    "textDocument": { "uri": app_uri },
                    "position": { "line": 10, "character": s_col }
                }
            }),
        );
        assert_eq!(def_param[0]["result"]["uri"], app_uri);
        assert_eq!(def_param[0]["result"]["range"]["start"]["line"], 8);

        // 3. Same-file JSX component tag `<Hero s={s} />` on line 11 -> `export function Hero` on line 4
        let hero_col = app_src.lines().nth(11).unwrap().find("Hero").unwrap() as u32;
        let def_jsx = handle_message(
            &mut state,
            &json!({
                "jsonrpc": "2.0",
                "id": 102,
                "method": "textDocument/definition",
                "params": {
                    "textDocument": { "uri": app_uri },
                    "position": { "line": 11, "character": hero_col + 1 }
                }
            }),
        );
        assert_eq!(def_jsx[0]["result"]["uri"], app_uri);
        assert_eq!(def_jsx[0]["result"]["range"]["start"]["line"], 4);

        // 4. Cross-file JSX component `<Navbar s={s} />` on line 11 -> `components/navbar.tsk` line 2
        let navbar_col = app_src.lines().nth(11).unwrap().find("Navbar").unwrap() as u32;
        let def_navbar = handle_message(
            &mut state,
            &json!({
                "jsonrpc": "2.0",
                "id": 103,
                "method": "textDocument/definition",
                "params": {
                    "textDocument": { "uri": app_uri },
                    "position": { "line": 11, "character": navbar_col + 2 }
                }
            }),
        );
        assert_eq!(def_navbar[0]["result"]["uri"], navbar_uri);
        assert_eq!(def_navbar[0]["result"]["range"]["start"]["line"], 2);

        // 5. Cross-file imported type `State` on line 8 (`s: readonly State`) -> `state.tsk` line 0
        let state_col = app_src.lines().nth(8).unwrap().find("State").unwrap() as u32;
        let def_state = handle_message(
            &mut state,
            &json!({
                "jsonrpc": "2.0",
                "id": 104,
                "method": "textDocument/definition",
                "params": {
                    "textDocument": { "uri": app_uri },
                    "position": { "line": 8, "character": state_col + 1 }
                }
            }),
        );
        assert_eq!(def_state[0]["result"]["uri"], state_uri);
        assert_eq!(def_state[0]["result"]["range"]["start"]["line"], 0);

        // 6. Cross-file imported const `STEP` on line 9 -> `state.tsk` line 3
        let step_col = app_src.lines().nth(9).unwrap().find("STEP").unwrap() as u32;
        let def_step = handle_message(
            &mut state,
            &json!({
                "jsonrpc": "2.0",
                "id": 105,
                "method": "textDocument/definition",
                "params": {
                    "textDocument": { "uri": app_uri },
                    "position": { "line": 9, "character": step_col + 1 }
                }
            }),
        );
        assert_eq!(def_step[0]["result"]["uri"], state_uri);
        assert_eq!(def_step[0]["result"]["range"]["start"]["line"], 3);

        let _ = std::fs::remove_dir_all(&tmp_dir);
    }

    #[test]
    fn references_skips_comments_and_strings_and_respects_include_declaration() {
        let mut state = LspState::new();
        let uri = "file:///tmp/refs_demo.tsk";
        let src = concat!(
            "// compute should not match in line comment\n",
            "/* compute should not match in block comment */\n",
            "export function compute(val: i64): i64 {\n",
            "    const note = \"compute inside string\";\n",
            "    const label = `compute static ${val}`;\n",
            "    return val + 1;\n",
            "}\n",
            "export function caller(): i64 {\n",
            "    const a = compute(10);\n",
            "    const b = compute(a);\n",
            "    return b;\n",
            "}\n",
        );
        open_doc(&mut state, uri, src);

        let call_col = src.lines().nth(8).unwrap().find("compute").unwrap() as u32;

        // includeDeclaration = true -> 3 occurrences (lines 2, 8, 9); comments & strings excluded
        let refs_with_decl = handle_message(
            &mut state,
            &json!({
                "jsonrpc": "2.0",
                "id": 200,
                "method": "textDocument/references",
                "params": {
                    "textDocument": { "uri": uri },
                    "position": { "line": 8, "character": call_col + 2 },
                    "context": { "includeDeclaration": true }
                }
            }),
        );
        let locs = refs_with_decl[0]["result"].as_array().unwrap();
        let lines: Vec<u64> = locs
            .iter()
            .map(|l| l["range"]["start"]["line"].as_u64().unwrap())
            .collect();
        assert_eq!(lines, vec![2, 8, 9]);

        // includeDeclaration = false -> 2 occurrences (lines 8, 9)
        let refs_no_decl = handle_message(
            &mut state,
            &json!({
                "jsonrpc": "2.0",
                "id": 201,
                "method": "textDocument/references",
                "params": {
                    "textDocument": { "uri": uri },
                    "position": { "line": 8, "character": call_col + 2 },
                    "context": { "includeDeclaration": false }
                }
            }),
        );
        let locs_no_decl = refs_no_decl[0]["result"].as_array().unwrap();
        let lines_no_decl: Vec<u64> = locs_no_decl
            .iter()
            .map(|l| l["range"]["start"]["line"].as_u64().unwrap())
            .collect();
        assert_eq!(lines_no_decl, vec![8, 9]);

        // Template interpolation `${val}` on line 4 IS found for local param `val`
        let val_col = src.lines().nth(5).unwrap().find("val").unwrap() as u32;
        let val_refs = handle_message(
            &mut state,
            &json!({
                "jsonrpc": "2.0",
                "id": 202,
                "method": "textDocument/references",
                "params": {
                    "textDocument": { "uri": uri },
                    "position": { "line": 5, "character": val_col },
                    "context": { "includeDeclaration": true }
                }
            }),
        );
        let val_lines: Vec<u64> = val_refs[0]["result"]
            .as_array()
            .unwrap()
            .iter()
            .map(|l| l["range"]["start"]["line"].as_u64().unwrap())
            .collect();
        assert_eq!(val_lines, vec![2, 4, 5]);
    }

    #[test]
    fn prepare_rename_and_rename_workspace_edit() {
        let mut state = LspState::new();
        let helper_uri = "file:///tmp/rename_proj/helper.tsk";
        let app_uri = "file:///tmp/rename_proj/app.tsk";

        let helper_src = concat!(
            "// calc in comment stays untouched\n",
            "export function calc(n: i64): i64 {\n",
            "    const tag = \"calc\";\n",
            "    return n * 2;\n",
            "}\n",
        );
        let app_src = concat!(
            "import { Column } from \"std/ui\";\n",
            "import { calc } from \"./helper.tsk\";\n",
            "export function run(): i64 {\n",
            "    return calc(21);\n",
            "}\n",
        );
        open_doc(&mut state, helper_uri, helper_src);
        open_doc(&mut state, app_uri, app_src);

        // 1. prepareRename accepts user identifier `calc`
        let calc_col = helper_src.lines().nth(1).unwrap().find("calc").unwrap() as u32;
        let prep_ok = handle_message(
            &mut state,
            &json!({
                "jsonrpc": "2.0",
                "id": 300,
                "method": "textDocument/prepareRename",
                "params": {
                    "textDocument": { "uri": helper_uri },
                    "position": { "line": 1, "character": calc_col + 1 }
                }
            }),
        );
        assert_eq!(prep_ok[0]["result"]["placeholder"], "calc");
        assert_eq!(prep_ok[0]["result"]["range"]["start"]["line"], 1);

        // 2. prepareRename rejects keyword `return`
        let ret_col = helper_src.lines().nth(3).unwrap().find("return").unwrap() as u32;
        let prep_kw = handle_message(
            &mut state,
            &json!({
                "jsonrpc": "2.0",
                "id": 301,
                "method": "textDocument/prepareRename",
                "params": {
                    "textDocument": { "uri": helper_uri },
                    "position": { "line": 3, "character": ret_col + 1 }
                }
            }),
        );
        assert!(prep_kw[0]["result"].is_null());

        // 3. prepareRename rejects primitive `i64`
        let i64_col = helper_src.lines().nth(1).unwrap().find("i64").unwrap() as u32;
        let prep_prim = handle_message(
            &mut state,
            &json!({
                "jsonrpc": "2.0",
                "id": 302,
                "method": "textDocument/prepareRename",
                "params": {
                    "textDocument": { "uri": helper_uri },
                    "position": { "line": 1, "character": i64_col + 1 }
                }
            }),
        );
        assert!(prep_prim[0]["result"].is_null());

        // 4. prepareRename rejects builtin JSX component `Column`
        let jsx_col = app_src.lines().next().unwrap().find("Column").unwrap() as u32;
        let prep_jsx = handle_message(
            &mut state,
            &json!({
                "jsonrpc": "2.0",
                "id": 303,
                "method": "textDocument/prepareRename",
                "params": {
                    "textDocument": { "uri": app_uri },
                    "position": { "line": 0, "character": jsx_col + 1 }
                }
            }),
        );
        assert!(prep_jsx[0]["result"].is_null());

        // 5. rename rejects invalid newName (`return`, `123bad`)
        for bad_name in ["return", "123bad", ""] {
            let bad_rename = handle_message(
                &mut state,
                &json!({
                    "jsonrpc": "2.0",
                    "id": 304,
                    "method": "textDocument/rename",
                    "params": {
                        "textDocument": { "uri": helper_uri },
                        "position": { "line": 1, "character": calc_col + 1 },
                        "newName": bad_name
                    }
                }),
            );
            assert!(
                bad_rename[0]["result"].is_null(),
                "Expected null rename for invalid newName '{bad_name}'"
            );
        }

        // 6. rename exported symbol `calc` -> `double_val` produces cross-file WorkspaceEdit
        let rename_res = handle_message(
            &mut state,
            &json!({
                "jsonrpc": "2.0",
                "id": 305,
                "method": "textDocument/rename",
                "params": {
                    "textDocument": { "uri": helper_uri },
                    "position": { "line": 1, "character": calc_col + 1 },
                    "newName": "double_val"
                }
            }),
        );
        let changes = rename_res[0]["result"]["changes"].as_object().unwrap();
        let helper_edits = changes.get(helper_uri).unwrap().as_array().unwrap();
        let app_edits = changes.get(app_uri).unwrap().as_array().unwrap();

        assert_eq!(helper_edits.len(), 1);
        assert_eq!(app_edits.len(), 2);

        let updated_helper = apply_lsp_edits(helper_src, helper_edits);
        assert!(updated_helper.contains("// calc in comment stays untouched"));
        assert!(updated_helper.contains("export function double_val(n: i64): i64"));
        assert!(updated_helper.contains("const tag = \"calc\";"));

        let updated_app = apply_lsp_edits(app_src, app_edits);
        assert!(updated_app.contains("import { double_val } from \"./helper.tsk\";"));
        assert!(updated_app.contains("return double_val(21);"));
    }

    #[test]
    fn code_actions_generate_valid_quickfixes_for_knr_diagnostics() {
        // 1. UndeclaredEffect -> insert `@effects("net")` above function & append to existing `@effects(...)`
        {
            let mut state = LspState::new();
            let uri = "file:///tmp/qf_effect.tsk";
            let src = concat!(
                "export function fetch_data(): string {\n",
                "    return syscall(\"net\", \"get\", \"https://example.com\");\n",
                "}\n",
            );
            let open_out = open_doc(&mut state, uri, src);
            let diags = open_out[0]["params"]["diagnostics"].as_array().unwrap();
            assert_eq!(diags.len(), 1);
            assert_eq!(diags[0]["code"], "UndeclaredEffect");

            let actions_out = handle_message(
                &mut state,
                &json!({
                    "jsonrpc": "2.0",
                    "id": 400,
                    "method": "textDocument/codeAction",
                    "params": {
                        "textDocument": { "uri": uri },
                        "range": diags[0]["range"],
                        "context": { "diagnostics": diags, "only": ["quickfix"] }
                    }
                }),
            );
            let actions = actions_out[0]["result"].as_array().unwrap();
            assert_eq!(actions.len(), 1);
            assert_eq!(actions[0]["kind"], "quickfix");
            assert_eq!(actions[0]["title"], "Add @effects(\"net\") to 'fetch_data'");

            let edits = actions[0]["edit"]["changes"][uri].as_array().unwrap();
            let fixed = apply_lsp_edits(src, edits);
            assert!(fixed.starts_with("@effects(\"net\")\nexport function fetch_data"));
            let (_, post_diags) = analyze_document(uri, &fixed);
            assert!(post_diags.is_empty(), "Expected 0 diagnostics after fix, got {post_diags:?}");

            // Also verify appending to an existing `@effects("ui")` decorator
            let src_existing = concat!(
                "@effects(\"ui\")\n",
                "export function fetch_more(): string {\n",
                "    return syscall(\"net\", \"get\", \"https://example.com\");\n",
                "}\n",
            );
            let open2 = open_doc(&mut state, uri, src_existing);
            let diags2 = open2[0]["params"]["diagnostics"].as_array().unwrap();
            let actions2_out = handle_message(
                &mut state,
                &json!({
                    "jsonrpc": "2.0",
                    "id": 401,
                    "method": "textDocument/codeAction",
                    "params": {
                        "textDocument": { "uri": uri },
                        "range": diags2[0]["range"],
                        "context": { "diagnostics": diags2 }
                    }
                }),
            );
            let actions2 = actions2_out[0]["result"].as_array().unwrap();
            assert_eq!(actions2[0]["title"], "Add @effects(\"net\") to 'fetch_more'");
            let fixed2 = apply_lsp_edits(
                src_existing,
                actions2[0]["edit"]["changes"][uri].as_array().unwrap(),
            );
            assert!(fixed2.starts_with("@effects(\"ui\", \"net\")\n"));
            let (_, post_diags2) = analyze_document(uri, &fixed2);
            assert!(post_diags2.is_empty(), "Expected 0 diagnostics after append fix, got {post_diags2:?}");
        }

        // 2. UseAfterMove on non-readonly parameter -> Borrow parameter as `readonly`
        {
            let mut state = LspState::new();
            let uri = "file:///tmp/qf_uam.tsk";
            let src = concat!(
                "export type Bag = { count: i64; };\n",
                "function consume(x: Bag): i64 {\n",
                "    return x.count;\n",
                "}\n",
                "export function inspect_twice(bag: Bag): i64 {\n",
                "    const first = consume(bag);\n",
                "    return first + bag.count;\n",
                "}\n",
            );
            let open_out = open_doc(&mut state, uri, src);
            let diags = open_out[0]["params"]["diagnostics"].as_array().unwrap();
            assert_eq!(diags.len(), 1);
            assert_eq!(diags[0]["code"], "UseAfterMove");

            let actions_out = handle_message(
                &mut state,
                &json!({
                    "jsonrpc": "2.0",
                    "id": 402,
                    "method": "textDocument/codeAction",
                    "params": {
                        "textDocument": { "uri": uri },
                        "range": diags[0]["range"],
                        "context": { "diagnostics": diags }
                    }
                }),
            );
            let actions = actions_out[0]["result"].as_array().unwrap();
            assert_eq!(actions.len(), 1);
            assert_eq!(actions[0]["kind"], "quickfix");
            assert_eq!(actions[0]["title"], "Borrow parameter 'bag' as 'readonly'");

            let edits = actions[0]["edit"]["changes"][uri].as_array().unwrap();
            let fixed = apply_lsp_edits(src, edits);
            assert!(fixed.contains("export function inspect_twice(bag: readonly Bag): i64"));
        }

        // 3. UnknownSchema for UI primitives (`Style`/`Widget`) -> import from "std/ui"
        {
            let mut state = LspState::new();
            let uri = "file:///tmp/qf_ui.tsk";
            let src = concat!(
                "export function view(): Widget {\n",
                "    return <Column p={16}><Text>Hello</Text></Column>;\n",
                "}\n",
            );
            let open_out = open_doc(&mut state, uri, src);
            let diags = open_out[0]["params"]["diagnostics"].as_array().unwrap();
            assert_eq!(diags.len(), 1);
            assert_eq!(diags[0]["code"], "UnknownSchema");

            let actions_out = handle_message(
                &mut state,
                &json!({
                    "jsonrpc": "2.0",
                    "id": 403,
                    "method": "textDocument/codeAction",
                    "params": {
                        "textDocument": { "uri": uri },
                        "range": diags[0]["range"],
                        "context": { "diagnostics": diags }
                    }
                }),
            );
            let actions = actions_out[0]["result"].as_array().unwrap();
            assert_eq!(actions.len(), 1);
            assert_eq!(actions[0]["kind"], "quickfix");
            assert_eq!(actions[0]["title"], "Import UI primitives from \"std/ui\"");

            let edits = actions[0]["edit"]["changes"][uri].as_array().unwrap();
            let fixed = apply_lsp_edits(src, edits);
            assert!(
                fixed.starts_with("import { Column, Row, Card, Text, Button } from \"std/ui\";\n")
            );
            let (_, post_diags) = analyze_document(uri, &fixed);
            assert!(post_diags.is_empty(), "Expected 0 diagnostics after std/ui import fix, got {post_diags:?}");
        }

        // 4. ShaderEffectForbidden -> Remove `@effects(...)` from `@shader` function
        {
            let mut state = LspState::new();
            let uri = "file:///tmp/qf_shader.tsk";
            let src = concat!(
                "export type Vec2 = { x: f64; y: f64; };\n",
                "export type Color = { r: f64; g: f64; b: f64; a: f64; };\n",
                "@shader\n",
                "@effects(\"net\")\n",
                "export function bad_fx(uv: Vec2, time: f64): Color {\n",
                "    return { r: uv.x, g: uv.y, b: 0.5, a: 1.0 };\n",
                "}\n",
            );
            let open_out = open_doc(&mut state, uri, src);
            let diags = open_out[0]["params"]["diagnostics"].as_array().unwrap();
            assert_eq!(diags.len(), 1);
            assert_eq!(diags[0]["code"], "ShaderEffectForbidden");

            let actions_out = handle_message(
                &mut state,
                &json!({
                    "jsonrpc": "2.0",
                    "id": 404,
                    "method": "textDocument/codeAction",
                    "params": {
                        "textDocument": { "uri": uri },
                        "range": diags[0]["range"],
                        "context": { "diagnostics": diags }
                    }
                }),
            );
            let actions = actions_out[0]["result"].as_array().unwrap();
            assert_eq!(actions.len(), 1);
            assert_eq!(actions[0]["title"], "Remove @effects(...) from @shader function");

            let fixed = apply_lsp_edits(src, actions[0]["edit"]["changes"][uri].as_array().unwrap());
            assert!(!fixed.contains("@effects"));
            let (_, post_diags) = analyze_document(uri, &fixed);
            assert!(post_diags.is_empty(), "Expected 0 diagnostics after removing @effects, got {post_diags:?}");
        }

        // 5. BorrowEscape -> Change parameter from `readonly` to owned
        {
            let mut state = LspState::new();
            let uri = "file:///tmp/qf_borrow.tsk";
            let src = concat!(
                "export type State = { count: i64; };\n",
                "export function escape_state(s: readonly State): State {\n",
                "    return s;\n",
                "}\n",
            );
            let open_out = open_doc(&mut state, uri, src);
            let diags = open_out[0]["params"]["diagnostics"].as_array().unwrap();
            assert_eq!(diags.len(), 1);

            let actions_out = handle_message(
                &mut state,
                &json!({
                    "jsonrpc": "2.0",
                    "id": 405,
                    "method": "textDocument/codeAction",
                    "params": {
                        "textDocument": { "uri": uri },
                        "range": diags[0]["range"],
                        "context": { "diagnostics": diags }
                    }
                }),
            );
            let actions = actions_out[0]["result"].as_array().unwrap();
            assert_eq!(actions.len(), 1);
            assert_eq!(actions[0]["title"], "Change parameter 's' from 'readonly' to owned");

            let fixed = apply_lsp_edits(src, actions[0]["edit"]["changes"][uri].as_array().unwrap());
            assert!(fixed.contains("export function escape_state(s: State): State"));
            let (_, post_diags) = analyze_document(uri, &fixed);
            assert!(post_diags.is_empty(), "Expected 0 diagnostics after removing readonly, got {post_diags:?}");
        }
    }

    #[test]
    fn cross_file_lsp_hover_definition_completion_references_overlays_and_auto_import() {
        let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .to_path_buf();
        let site_app_path = workspace_root.join("site/app.tsk");
        let site_state_path = workspace_root.join("site/state.tsk");
        let site_shaders_path = workspace_root.join("site/shaders.tsk");
        let site_navbar_path = workspace_root.join("site/components/navbar.tsk");

        let app_src = std::fs::read_to_string(&site_app_path).expect("read site/app.tsk");
        let app_uri = path_to_uri(&site_app_path);
        let state_uri = path_to_uri(&site_state_path);
        let shaders_uri = path_to_uri(&site_shaders_path);
        let navbar_uri = path_to_uri(&site_navbar_path);

        let mut state = LspState::new();
        let open_res = open_doc(&mut state, &app_uri, &app_src);
        let diags = open_res[0]["params"]["diagnostics"].as_array().unwrap();
        assert!(
            diags.is_empty(),
            "site/app.tsk should have 0 LSP diagnostics, got: {diags:?}"
        );

        // 1. Ctrl+Click directly on `"./state.tsk"` and `"./shaders.tsk"` opens the target files
        let state_imp_line = app_src
            .lines()
            .position(|l| l.contains("\"./state.tsk\""))
            .unwrap() as u32;
        let state_imp_col = app_src
            .lines()
            .nth(state_imp_line as usize)
            .unwrap()
            .find("./state.tsk")
            .unwrap() as u32;
        let def_state_file = handle_message(
            &mut state,
            &json!({
                "jsonrpc": "2.0",
                "id": 500,
                "method": "textDocument/definition",
                "params": {
                    "textDocument": { "uri": app_uri },
                    "position": { "line": state_imp_line, "character": state_imp_col + 3 }
                }
            }),
        );
        assert_eq!(def_state_file[0]["result"]["uri"], state_uri);
        assert_eq!(def_state_file[0]["result"]["range"]["start"]["line"], 0);

        let shaders_imp_line = app_src
            .lines()
            .position(|l| l.contains("\"./shaders.tsk\""))
            .unwrap() as u32;
        let shaders_imp_col = app_src
            .lines()
            .nth(shaders_imp_line as usize)
            .unwrap()
            .find("./shaders.tsk")
            .unwrap() as u32;
        let def_shaders_file = handle_message(
            &mut state,
            &json!({
                "jsonrpc": "2.0",
                "id": 501,
                "method": "textDocument/definition",
                "params": {
                    "textDocument": { "uri": app_uri },
                    "position": { "line": shaders_imp_line, "character": shaders_imp_col + 3 }
                }
            }),
        );
        assert_eq!(def_shaders_file[0]["result"]["uri"], shaders_uri);

        // 2. Hover on imported `State`, `knr_hero_nebula`, `s.active_tab`, and `"./state.tsk"`
        let hover_state_col = app_src
            .lines()
            .nth(state_imp_line as usize)
            .unwrap()
            .find("State")
            .unwrap() as u32;
        let hover_state = handle_message(
            &mut state,
            &json!({
                "jsonrpc": "2.0",
                "id": 502,
                "method": "textDocument/hover",
                "params": {
                    "textDocument": { "uri": app_uri },
                    "position": { "line": state_imp_line, "character": hover_state_col + 1 }
                }
            }),
        );
        let hover_state_md = hover_state[0]["result"]["contents"]["value"].as_str().unwrap();
        assert!(hover_state_md.contains("type State"), "got: {hover_state_md}");
        assert!(hover_state_md.contains("Imported from `./state.tsk`"), "got: {hover_state_md}");

        let nebula_line = app_src
            .lines()
            .position(|l| l.contains("knr_hero_nebula("))
            .unwrap() as u32;
        let nebula_col = app_src
            .lines()
            .nth(nebula_line as usize)
            .unwrap()
            .find("knr_hero_nebula")
            .unwrap() as u32;
        let hover_nebula = handle_message(
            &mut state,
            &json!({
                "jsonrpc": "2.0",
                "id": 503,
                "method": "textDocument/hover",
                "params": {
                    "textDocument": { "uri": app_uri },
                    "position": { "line": nebula_line, "character": nebula_col + 4 }
                }
            }),
        );
        let hover_nebula_md = hover_nebula[0]["result"]["contents"]["value"].as_str().unwrap();
        assert!(hover_nebula_md.contains("@shader"), "got: {hover_nebula_md}");
        assert!(hover_nebula_md.contains("Imported from `./shaders.tsk`"), "got: {hover_nebula_md}");

        let field_line = app_src
            .lines()
            .position(|l| l.contains("s.active_tab === 1"))
            .unwrap() as u32;
        let field_col = app_src
            .lines()
            .nth(field_line as usize)
            .unwrap()
            .find("active_tab")
            .unwrap() as u32;
        let hover_field = handle_message(
            &mut state,
            &json!({
                "jsonrpc": "2.0",
                "id": 504,
                "method": "textDocument/hover",
                "params": {
                    "textDocument": { "uri": app_uri },
                    "position": { "line": field_line, "character": field_col + 2 }
                }
            }),
        );
        let hover_field_md = hover_field[0]["result"]["contents"]["value"].as_str().unwrap();
        assert!(hover_field_md.contains("State.active_tab: i64"), "got: {hover_field_md}");

        let hover_mod_path = handle_message(
            &mut state,
            &json!({
                "jsonrpc": "2.0",
                "id": 505,
                "method": "textDocument/hover",
                "params": {
                    "textDocument": { "uri": app_uri },
                    "position": { "line": state_imp_line, "character": state_imp_col + 3 }
                }
            }),
        );
        let hover_mod_md = hover_mod_path[0]["result"]["contents"]["value"].as_str().unwrap();
        assert!(hover_mod_md.contains("type State"), "got: {hover_mod_md}");
        assert!(hover_mod_md.contains("function init"), "got: {hover_mod_md}");

        // 3. References on `State` from `site/app.tsk` (ONLY app.tsk open!) finds state.tsk & components/navbar.tsk
        let refs_state = handle_message(
            &mut state,
            &json!({
                "jsonrpc": "2.0",
                "id": 506,
                "method": "textDocument/references",
                "params": {
                    "textDocument": { "uri": app_uri },
                    "position": { "line": state_imp_line, "character": hover_state_col + 1 },
                    "context": { "includeDeclaration": true }
                }
            }),
        );
        let ref_uris: HashSet<String> = refs_state[0]["result"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|loc| loc["uri"].as_str().map(str::to_string))
            .collect();
        assert!(ref_uris.contains(&app_uri), "refs missing app.tsk: {ref_uris:?}");
        assert!(ref_uris.contains(&state_uri), "refs missing state.tsk: {ref_uris:?}");
        assert!(ref_uris.contains(&navbar_uri), "refs missing components/navbar.tsk: {ref_uris:?}");

        // 4. Completion inside `import {  } from "./state.tsk"`, inside `from "./"`, and after `s.`
        let scratch_uri = path_to_uri(&workspace_root.join("site/__completion_scratch.tsk"));
        let scratch_src = concat!(
            "import {  } from \"./state.tsk\";\n",
            "import { foo } from \"./\";\n",
            "import { State } from \"./state.tsk\";\n",
            "export function probe(s: readonly State): i64 {\n",
            "    return s.\n",
            "}\n",
        );
        open_doc(&mut state, &scratch_uri, scratch_src);

        let comp_named = handle_message(
            &mut state,
            &json!({
                "jsonrpc": "2.0",
                "id": 507,
                "method": "textDocument/completion",
                "params": {
                    "textDocument": { "uri": scratch_uri },
                    "position": { "line": 0, "character": 9 }
                }
            }),
        );
        let named_labels: HashSet<&str> = comp_named[0]["result"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|i| i["label"].as_str())
            .collect();
        assert!(named_labels.contains("State"), "got: {named_labels:?}");
        assert!(named_labels.contains("init"), "got: {named_labels:?}");
        assert!(named_labels.contains("active_shader_fn_name"), "got: {named_labels:?}");

        let comp_path = handle_message(
            &mut state,
            &json!({
                "jsonrpc": "2.0",
                "id": 508,
                "method": "textDocument/completion",
                "params": {
                    "textDocument": { "uri": scratch_uri },
                    "position": { "line": 1, "character": 23 },
                    "context": { "triggerKind": 2, "triggerCharacter": "/" }
                }
            }),
        );
        let path_labels: HashSet<&str> = comp_path[0]["result"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|i| i["label"].as_str())
            .collect();
        assert!(path_labels.contains("./state.tsk"), "got: {path_labels:?}");
        assert!(path_labels.contains("./shaders.tsk"), "got: {path_labels:?}");
        assert!(path_labels.contains("./components/navbar.tsk"), "got: {path_labels:?}");

        let comp_dot = handle_message(
            &mut state,
            &json!({
                "jsonrpc": "2.0",
                "id": 509,
                "method": "textDocument/completion",
                "params": {
                    "textDocument": { "uri": scratch_uri },
                    "position": { "line": 4, "character": 13 },
                    "context": { "triggerKind": 2, "triggerCharacter": "." }
                }
            }),
        );
        let dot_labels: HashSet<&str> = comp_dot[0]["result"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|i| i["label"].as_str())
            .collect();
        assert!(dot_labels.contains("active_tab"), "got: {dot_labels:?}");
        assert!(dot_labels.contains("headline"), "got: {dot_labels:?}");

        // 5. Live unsaved cross-file editing & Auto-Import QuickFix
        let tmp_dir = std::env::temp_dir().join(format!(
            "knr_lsp_overlay_autoimport_{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&tmp_dir).unwrap();
        let lib_path = tmp_dir.join("lib.tsk");
        let main_path = tmp_dir.join("main.tsk");
        std::fs::write(&lib_path, "export function helper_val(): i64 { return 7; }\n").unwrap();
        std::fs::write(&main_path, "export function run(): i64 { return helper_val(); }\n").unwrap();

        let lib_uri = path_to_uri(&lib_path);
        let main_uri = path_to_uri(&main_path);
        let mut live_state = LspState::new();
        let open_main = open_doc(
            &mut live_state,
            &main_uri,
            "export function run(): i64 { return helper_val(); }\n",
        );
        let main_diags = open_main[0]["params"]["diagnostics"].as_array().unwrap();
        assert_eq!(main_diags.len(), 1);

        // Auto-import QuickFix suggests `Import 'helper_val' from "./lib.tsk"`
        let qf_out = handle_message(
            &mut live_state,
            &json!({
                "jsonrpc": "2.0",
                "id": 510,
                "method": "textDocument/codeAction",
                "params": {
                    "textDocument": { "uri": main_uri },
                    "range": main_diags[0]["range"],
                    "context": { "diagnostics": main_diags, "only": ["quickfix"] }
                }
            }),
        );
        let qf_actions = qf_out[0]["result"].as_array().unwrap();
        assert!(
            qf_actions
                .iter()
                .any(|a| a["title"] == "Import 'helper_val' from \"./lib.tsk\""),
            "Expected auto-import QuickFix, got: {qf_actions:?}"
        );

        // Now open lib.tsk + main.tsk (importing new_fn which is not on disk yet) and add new_fn via unsaved didChange
        open_doc(&mut live_state, &lib_uri, "export function helper_val(): i64 { return 7; }\n");
        let open_main2 = open_doc(
            &mut live_state,
            &main_uri,
            "import { new_fn } from \"./lib.tsk\";\nexport function run(): i64 { return new_fn(); }\n",
        );
        assert!(
            !open_main2[0]["params"]["diagnostics"]
                .as_array()
                .unwrap()
                .is_empty(),
            "Should report diagnostic before new_fn exists"
        );
        let change_out = handle_message(
            &mut live_state,
            &json!({
                "jsonrpc": "2.0",
                "method": "textDocument/didChange",
                "params": {
                    "textDocument": { "uri": lib_uri, "version": 2 },
                    "contentChanges": [{ "text": "export function new_fn(): i64 { return 42; }\n" }]
                }
            }),
        );
        let main_repub = change_out
            .iter()
            .find(|n| n.pointer("/params/uri").and_then(Value::as_str) == Some(main_uri.as_str()))
            .expect("didChange on lib.tsk must republish diagnostics for importing main.tsk");
        assert!(
            main_repub["params"]["diagnostics"].as_array().unwrap().is_empty(),
            "main.tsk diagnostics should clear immediately from unsaved overlay: {main_repub:?}"
        );

        let _ = std::fs::remove_dir_all(&tmp_dir);
    }

    #[test]
    fn initialize_advertises_ide_ultra_v04_capabilities() {
        let mut state = LspState::new();
        let out = handle_message(
            &mut state,
            &json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {} }),
        );
        let caps = &out[0]["result"]["capabilities"];
        assert_eq!(
            caps["signatureHelpProvider"]["triggerCharacters"],
            json!(["(", ","])
        );
        assert_eq!(caps["inlayHintProvider"], true);
        assert_eq!(caps["workspaceSymbolProvider"], true);
        assert_eq!(caps["foldingRangeProvider"], true);
    }

    #[test]
    fn signature_help_local_cross_file_alias_namespace_nested_and_builtins() {
        let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .to_path_buf();

        // 1. Cross-file signatureHelp on `knr_hero_nebula(uv, 1.0, |)` -> activeParameter == 2 (`mouse: Vec2`)
        let scratch_uri = path_to_uri(&workspace_root.join("site/__sig_scratch.tsk"));
        let scratch_src = concat!(
            "import { knr_hero_nebula, knr_sdf_crystal as crystal } from \"./shaders.tsk\";\n",
            "import * as sh from \"./shaders.tsk\";\n",
            "export function local_add(left: i64, right: i64, bias: i64): i64 {\n",
            "    return left + right + bias;\n",
            "}\n",
            "export function probe(uv: Vec2): f64 {\n",
            "    const a = knr_hero_nebula(uv, 1.0, );\n",
            "    const b = local_add(1, local_add(2, 3, 4), );\n",
            "    const c = clamp(0.5, 0.0, );\n",
            "    const d = crystal(uv, );\n",
            "    const e = sh.knr_hero_nebula(uv, 0.5, uv, );\n",
            "    const f = syscall(\"net\", \"get\", );\n",
            "    return 0.0;\n",
            "}\n",
        );

        let mut state = LspState::new();
        open_doc(&mut state, &scratch_uri, scratch_src);

        // 1a. Cross-file `knr_hero_nebula(uv, 1.0, |)` -> activeParameter == 2
        let col_nebula = scratch_src.lines().nth(6).unwrap().find(");").unwrap() as u32;
        let sig_nebula = handle_message(
            &mut state,
            &json!({
                "jsonrpc": "2.0",
                "id": 600,
                "method": "textDocument/signatureHelp",
                "params": {
                    "textDocument": { "uri": scratch_uri },
                    "position": { "line": 6, "character": col_nebula }
                }
            }),
        );
        assert_eq!(sig_nebula[0]["result"]["activeSignature"], 0);
        assert_eq!(sig_nebula[0]["result"]["activeParameter"], 2);
        let nebula_label = sig_nebula[0]["result"]["signatures"][0]["label"]
            .as_str()
            .unwrap();
        assert!(nebula_label.contains("knr_hero_nebula"), "got: {nebula_label}");
        assert_eq!(
            sig_nebula[0]["result"]["signatures"][0]["parameters"][2]["label"],
            "mouse: Vec2"
        );

        // 1b. Local call with nested parens `local_add(1, local_add(2, 3, 4), |)` -> activeParameter == 2 (`bias: i64`)
        let col_nested = scratch_src.lines().nth(7).unwrap().rfind(");").unwrap() as u32;
        let sig_nested = handle_message(
            &mut state,
            &json!({
                "jsonrpc": "2.0",
                "id": 601,
                "method": "textDocument/signatureHelp",
                "params": {
                    "textDocument": { "uri": scratch_uri },
                    "position": { "line": 7, "character": col_nested }
                }
            }),
        );
        assert_eq!(sig_nested[0]["result"]["activeParameter"], 2);
        assert_eq!(
            sig_nested[0]["result"]["signatures"][0]["parameters"][2]["label"],
            "bias: i64"
        );

        // 1c. Builtin `clamp(0.5, 0.0, |)` -> activeParameter == 2 (`hi: f64`)
        let col_clamp = scratch_src.lines().nth(8).unwrap().find(");").unwrap() as u32;
        let sig_clamp = handle_message(
            &mut state,
            &json!({
                "jsonrpc": "2.0",
                "id": 602,
                "method": "textDocument/signatureHelp",
                "params": {
                    "textDocument": { "uri": scratch_uri },
                    "position": { "line": 8, "character": col_clamp }
                }
            }),
        );
        assert_eq!(sig_clamp[0]["result"]["activeParameter"], 2);
        assert_eq!(
            sig_clamp[0]["result"]["signatures"][0]["parameters"][2]["label"],
            "hi: f64"
        );

        // 1d. Aliased cross-file import `crystal(uv, |)` -> activeParameter == 1 (`time: f64`)
        let col_alias = scratch_src.lines().nth(9).unwrap().find(");").unwrap() as u32;
        let sig_alias = handle_message(
            &mut state,
            &json!({
                "jsonrpc": "2.0",
                "id": 603,
                "method": "textDocument/signatureHelp",
                "params": {
                    "textDocument": { "uri": scratch_uri },
                    "position": { "line": 9, "character": col_alias }
                }
            }),
        );
        assert_eq!(sig_alias[0]["result"]["activeParameter"], 1);
        assert!(sig_alias[0]["result"]["signatures"][0]["label"]
            .as_str()
            .unwrap()
            .contains("knr_sdf_crystal"));

        // 1e. Namespace call `sh.knr_hero_nebula(uv, 0.5, uv, |)` -> activeParameter == 3 (`energy: f64`)
        let col_ns = scratch_src.lines().nth(10).unwrap().find(");").unwrap() as u32;
        let sig_ns = handle_message(
            &mut state,
            &json!({
                "jsonrpc": "2.0",
                "id": 604,
                "method": "textDocument/signatureHelp",
                "params": {
                    "textDocument": { "uri": scratch_uri },
                    "position": { "line": 10, "character": col_ns }
                }
            }),
        );
        assert_eq!(sig_ns[0]["result"]["activeParameter"], 3);
        assert_eq!(
            sig_ns[0]["result"]["signatures"][0]["parameters"][3]["label"],
            "energy: f64"
        );

        // 1f. Builtin `syscall("net", "get", |)` -> activeParameter == 2
        let col_sys = scratch_src.lines().nth(11).unwrap().find(");").unwrap() as u32;
        let sig_sys = handle_message(
            &mut state,
            &json!({
                "jsonrpc": "2.0",
                "id": 605,
                "method": "textDocument/signatureHelp",
                "params": {
                    "textDocument": { "uri": scratch_uri },
                    "position": { "line": 11, "character": col_sys }
                }
            }),
        );
        assert_eq!(sig_sys[0]["result"]["activeParameter"], 2);
        assert!(sig_sys[0]["result"]["signatures"][0]["label"]
            .as_str()
            .unwrap()
            .contains("syscall"));
    }

    #[test]
    fn inlay_hints_emit_inferred_types_and_call_parameter_names() {
        let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .to_path_buf();
        let site_app_path = workspace_root.join("site/app.tsk");
        let app_src = std::fs::read_to_string(&site_app_path).expect("read site/app.tsk");
        let app_uri = path_to_uri(&site_app_path);

        let mut state = LspState::new();
        open_doc(&mut state, &app_uri, &app_src);

        let total_lines = app_src.lines().count() as u32;
        let out = handle_message(
            &mut state,
            &json!({
                "jsonrpc": "2.0",
                "id": 610,
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
        let hints = out[0]["result"].as_array().unwrap();
        assert!(!hints.is_empty(), "Expected inlay hints for site/app.tsk");

        // Inferred type hints (kind: 1): `let s = init();` -> `: State`, `const c_nebula = knr_hero_nebula(...)` -> `: Color`, `const v0 = view(s)` -> `: Widget`
        let type_labels: HashSet<&str> = hints
            .iter()
            .filter(|h| h["kind"] == INLAY_HINT_KIND_TYPE)
            .filter_map(|h| h["label"].as_str())
            .collect();
        assert!(type_labels.contains(": State"), "missing : State in {type_labels:?}");
        assert!(type_labels.contains(": Widget"), "missing : Widget in {type_labels:?}");
        assert!(type_labels.contains(": Color"), "missing : Color in {type_labels:?}");
        assert!(type_labels.contains(": bool"), "missing : bool in {type_labels:?}");

        // Parameter name hints (kind: 2) on `knr_hero_nebula(uv1, 1.25, m1, 0.68, 0.12, 0.52)` -> `uv:`, `time:`, `mouse:`, `energy:`, `hue:`, `depth:`
        let nebula_line = app_src
            .lines()
            .position(|l| l.contains("knr_hero_nebula(uv1, 1.25"))
            .unwrap() as u32;
        let nebula_param_hints: Vec<&str> = hints
            .iter()
            .filter(|h| h["kind"] == INLAY_HINT_KIND_PARAMETER && h["position"]["line"] == nebula_line)
            .filter_map(|h| h["label"].as_str())
            .collect();
        assert_eq!(
            nebula_param_hints,
            vec!["uv:", "time:", "mouse:", "energy:", "hue:", "depth:"]
        );

        // Also verify matching variable name skips parameter hint & JSX infers `: Widget`
        let local_uri = "file:///tmp/inlay_local.tsk";
        let local_src = concat!(
            "import { Column, Text } from \"std/ui\";\n",
            "function combine(a: i64, b: i64): i64 { return a + b; }\n",
            "export function demo(a: i64): Widget {\n",
            "    const n = 42;\n",
            "    const f = 3.14;\n",
            "    const flag = true;\n",
            "    const msg = \"hello\";\n",
            "    const sum = combine(a, 10);\n",
            "    const ui = <Column><Text>{msg}</Text></Column>;\n",
            "    return ui;\n",
            "}\n",
        );
        open_doc(&mut state, local_uri, local_src);
        let local_out = handle_message(
            &mut state,
            &json!({
                "jsonrpc": "2.0",
                "id": 611,
                "method": "textDocument/inlayHint",
                "params": {
                    "textDocument": { "uri": local_uri },
                    "range": {
                        "start": { "line": 0, "character": 0 },
                        "end": { "line": 20, "character": 0 }
                    }
                }
            }),
        );
        let local_hints = local_out[0]["result"].as_array().unwrap();
        let local_type_labels: HashSet<&str> = local_hints
            .iter()
            .filter(|h| h["kind"] == INLAY_HINT_KIND_TYPE)
            .filter_map(|h| h["label"].as_str())
            .collect();
        for expected in [": i64", ": f64", ": bool", ": string", ": Widget"] {
            assert!(
                local_type_labels.contains(expected),
                "Expected type hint {expected}, got {local_type_labels:?}"
            );
        }
        // `combine(a, 10)` skips `a:` (exact match) and emits `b:` only!
        let combine_param_hints: Vec<&str> = local_hints
            .iter()
            .filter(|h| h["kind"] == INLAY_HINT_KIND_PARAMETER && h["position"]["line"] == 7)
            .filter_map(|h| h["label"].as_str())
            .collect();
        assert_eq!(combine_param_hints, vec!["b:"]);
    }

    #[test]
    fn workspace_symbol_finds_declarations_across_site_project() {
        let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .to_path_buf();
        let site_app_path = workspace_root.join("site/app.tsk");
        let app_src = std::fs::read_to_string(&site_app_path).expect("read site/app.tsk");
        let app_uri = path_to_uri(&site_app_path);

        let mut state = LspState::new();
        open_doc(&mut state, &app_uri, &app_src);

        // Query "nebula" -> finds `knr_hero_nebula` in `site/shaders.tsk`
        let res_nebula = handle_message(
            &mut state,
            &json!({
                "jsonrpc": "2.0",
                "id": 620,
                "method": "workspace/symbol",
                "params": { "query": "nebula" }
            }),
        );
        let syms_nebula = res_nebula[0]["result"].as_array().unwrap();
        assert!(
            syms_nebula.iter().any(|s| {
                s["name"] == "knr_hero_nebula"
                    && s["kind"] == SYMBOL_KIND_FUNCTION
                    && s["containerName"] == "shaders.tsk"
            }),
            "Expected knr_hero_nebula in shaders.tsk, got: {syms_nebula:?}"
        );

        // Query "State" -> finds `State` struct in `site/state.tsk`
        let res_state = handle_message(
            &mut state,
            &json!({
                "jsonrpc": "2.0",
                "id": 621,
                "method": "workspace/symbol",
                "params": { "query": "State" }
            }),
        );
        let syms_state = res_state[0]["result"].as_array().unwrap();
        assert!(
            syms_state.iter().any(|s| {
                s["name"] == "State"
                    && s["kind"] == SYMBOL_KIND_STRUCT
                    && s["containerName"] == "state.tsk"
            }),
            "Expected State in state.tsk, got: {syms_state:?}"
        );

        // Query "Navbar" -> finds `Navbar` in `site/components/navbar.tsk`
        let res_nav = handle_message(
            &mut state,
            &json!({
                "jsonrpc": "2.0",
                "id": 622,
                "method": "workspace/symbol",
                "params": { "query": "Navbar" }
            }),
        );
        let syms_nav = res_nav[0]["result"].as_array().unwrap();
        assert!(
            syms_nav.iter().any(|s| {
                s["name"] == "Navbar"
                    && s["kind"] == SYMBOL_KIND_FUNCTION
                    && s["containerName"] == "navbar.tsk"
            }),
            "Expected Navbar in navbar.tsk, got: {syms_nav:?}"
        );
    }

    #[test]
    fn folding_range_folds_comments_imports_and_multiline_declarations() {
        let mut state = LspState::new();
        let uri = "file:///tmp/folding_demo.tsk";
        let src = concat!(
            "// Header line 1\n",
            "// Header line 2\n",
            "// Header line 3\n",
            "import { Column, Text } from \"std/ui\";\n",
            "import {\n",
            "    Theme,\n",
            "    theme_obsidian,\n",
            "} from \"std/theme\";\n",
            "\n",
            "export type Config = {\n",
            "    width: i64;\n",
            "    height: i64;\n",
            "};\n",
            "\n",
            "export enum Mode {\n",
            "    Light,\n",
            "    Dark,\n",
            "}\n",
            "\n",
            "export function render_box(cfg: readonly Config): i64 {\n",
            "    return cfg.width + cfg.height;\n",
            "}\n",
        );
        open_doc(&mut state, uri, src);

        let res = handle_message(
            &mut state,
            &json!({
                "jsonrpc": "2.0",
                "id": 630,
                "method": "textDocument/foldingRange",
                "params": { "textDocument": { "uri": uri } }
            }),
        );
        let ranges = res[0]["result"].as_array().unwrap();
        assert!(
            ranges
                .iter()
                .any(|r| r["startLine"] == 0 && r["endLine"] == 2 && r["kind"] == "comment"),
            "Missing header comment fold: {ranges:?}"
        );
        assert!(
            ranges
                .iter()
                .any(|r| r["startLine"] == 3 && r["endLine"] == 7 && r["kind"] == "imports"),
            "Missing contiguous imports fold: {ranges:?}"
        );
        assert!(
            ranges
                .iter()
                .any(|r| r["startLine"] == 4 && r["endLine"] == 7 && r["kind"] == "imports"),
            "Missing multiline import fold: {ranges:?}"
        );
        assert!(
            ranges
                .iter()
                .any(|r| r["startLine"] == 9 && r["endLine"] == 12 && r["kind"] == "region"),
            "Missing multiline type fold: {ranges:?}"
        );
        assert!(
            ranges
                .iter()
                .any(|r| r["startLine"] == 14 && r["endLine"] == 17 && r["kind"] == "region"),
            "Missing multiline enum fold: {ranges:?}"
        );
        assert!(
            ranges
                .iter()
                .any(|r| r["startLine"] == 19 && r["endLine"] == 21 && r["kind"] == "region"),
            "Missing multiline function fold: {ranges:?}"
        );
    }

    #[test]
    fn icon_name_completion_returns_all_67_crisp_icons_at_priority_0() {
        let mut state = LspState::new();
        let uri = "file:///tmp/icon_comp.tsk";
        let src = concat!(
            "import { Icon } from \"std/ui\";\n",
            "export function view(): Widget {\n",
            "    return <Icon name=\"\" size={18} />;\n",
            "}\n",
        );
        open_doc(&mut state, uri, src);

        let col = src.lines().nth(2).unwrap().find("name=\"").unwrap() as u32 + 6;
        let out = handle_message(
            &mut state,
            &json!({
                "jsonrpc": "2.0",
                "id": 640,
                "method": "textDocument/completion",
                "params": {
                    "textDocument": { "uri": uri },
                    "position": { "line": 2, "character": col },
                    "context": { "triggerKind": 2, "triggerCharacter": "\"" }
                }
            }),
        );
        let items = out[0]["result"].as_array().unwrap();
        assert_eq!(
            items.len(),
            knr_runtime::gui::supported_icon_names().len(),
            "Expected all 67 vector icons"
        );
        assert!(items.len() >= 67, "Expected at least 67 icons, got {}", items.len());
        let labels: HashSet<&str> = items
            .iter()
            .filter_map(|i| i["label"].as_str())
            .collect();
        for required in ["search", "sparkles", "database", "git"] {
            assert!(labels.contains(required), "Missing icon '{required}'");
        }
        assert!(
            items
                .iter()
                .all(|i| i["sortText"].as_str().unwrap_or("").starts_with('0')),
            "All icon completions should be priority 0"
        );
    }

    #[test]
    fn stdlib_v04_modules_array_methods_packages_and_destructuring_definition() {
        let mut state = LspState::new();
        let uri = "file:///tmp/v04_std_demo.tsk";
        let src = concat!(
            "import {  } from \"std/theme\";\n",
            "import {  } from \"std/router\";\n",
            "import { foo } from \"std/\";\n",
            "import { Theme, theme_obsidian } from \"std/theme\";\n",
            "import { Route, router_init, router_push } from \"std/router\";\n",
            "export type State = { count: i64; label: string; };\n",
            "export function inspect(s: readonly State, arr: readonly i64[]): i64 {\n",
            "    const { count, label: title } = s;\n",
            "    const [first, second] = arr;\n",
            "    const doubled = arr.map((x) => x * 2);\n",
            "    return count + first + second;\n",
            "}\n",
        );
        open_doc(&mut state, uri, src);

        // 1. `from "std/|"` completes std/theme, std/router, std/layout, std/testing
        let std_col = src.lines().nth(2).unwrap().find("std/").unwrap() as u32 + 4;
        let comp_std = handle_message(
            &mut state,
            &json!({
                "jsonrpc": "2.0",
                "id": 650,
                "method": "textDocument/completion",
                "params": {
                    "textDocument": { "uri": uri },
                    "position": { "line": 2, "character": std_col },
                    "context": { "triggerKind": 2, "triggerCharacter": "/" }
                }
            }),
        );
        let std_labels: HashSet<&str> = comp_std[0]["result"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|i| i["label"].as_str())
            .collect();
        for expected in ["std/theme", "std/router", "std/layout", "std/testing"] {
            assert!(std_labels.contains(expected), "Missing stdlib path {expected}: {std_labels:?}");
        }

        // 2. Named import completion inside `import {  } from "std/theme"` and `"std/router"`
        let comp_theme = handle_message(
            &mut state,
            &json!({
                "jsonrpc": "2.0",
                "id": 651,
                "method": "textDocument/completion",
                "params": {
                    "textDocument": { "uri": uri },
                    "position": { "line": 0, "character": 9 }
                }
            }),
        );
        let theme_syms: HashSet<&str> = comp_theme[0]["result"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|i| i["label"].as_str())
            .collect();
        for expected in ["Theme", "theme_obsidian", "theme_light", "theme_nord", "theme_solar"] {
            assert!(theme_syms.contains(expected), "Missing std/theme export {expected}");
        }

        let comp_router = handle_message(
            &mut state,
            &json!({
                "jsonrpc": "2.0",
                "id": 652,
                "method": "textDocument/completion",
                "params": {
                    "textDocument": { "uri": uri },
                    "position": { "line": 1, "character": 9 }
                }
            }),
        );
        let router_syms: HashSet<&str> = comp_router[0]["result"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|i| i["label"].as_str())
            .collect();
        for expected in ["Route", "router_init", "router_push", "router_back", "router_is", "router_can_back"] {
            assert!(router_syms.contains(expected), "Missing std/router export {expected}");
        }

        // 3. Hover on `"std/theme"` and `"std/router"` module paths + imported `theme_obsidian`
        let theme_path_col = src.lines().nth(3).unwrap().find("std/theme").unwrap() as u32 + 4;
        let hover_theme_mod = handle_message(
            &mut state,
            &json!({
                "jsonrpc": "2.0",
                "id": 653,
                "method": "textDocument/hover",
                "params": {
                    "textDocument": { "uri": uri },
                    "position": { "line": 3, "character": theme_path_col }
                }
            }),
        );
        let theme_mod_md = hover_theme_mod[0]["result"]["contents"]["value"].as_str().unwrap();
        assert!(theme_mod_md.contains("std/theme"));
        assert!(theme_mod_md.contains("theme_obsidian"));

        let router_path_col = src.lines().nth(4).unwrap().find("std/router").unwrap() as u32 + 4;
        let hover_router_mod = handle_message(
            &mut state,
            &json!({
                "jsonrpc": "2.0",
                "id": 654,
                "method": "textDocument/hover",
                "params": {
                    "textDocument": { "uri": uri },
                    "position": { "line": 4, "character": router_path_col }
                }
            }),
        );
        let router_mod_md = hover_router_mod[0]["result"]["contents"]["value"].as_str().unwrap();
        assert!(router_mod_md.contains("std/router"));
        assert!(router_mod_md.contains("router_push"));

        // 4. Array method completions on `.` and hover doc on `.map`
        let map_col = src.lines().nth(9).unwrap().find("map").unwrap() as u32;
        let hover_map = handle_message(
            &mut state,
            &json!({
                "jsonrpc": "2.0",
                "id": 655,
                "method": "textDocument/hover",
                "params": {
                    "textDocument": { "uri": uri },
                    "position": { "line": 9, "character": map_col + 1 }
                }
            }),
        );
        assert!(hover_map[0]["result"]["contents"]["value"]
            .as_str()
            .unwrap()
            .contains("Array<T>.map"));

        let comp_dot = handle_message(
            &mut state,
            &json!({
                "jsonrpc": "2.0",
                "id": 656,
                "method": "textDocument/completion",
                "params": {
                    "textDocument": { "uri": uri },
                    "position": { "line": 9, "character": map_col },
                    "context": { "triggerKind": 2, "triggerCharacter": "." }
                }
            }),
        );
        let dot_labels: HashSet<&str> = comp_dot[0]["result"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|i| i["label"].as_str())
            .collect();
        for m in [
            "map", "filter", "reduce", "some", "every", "count", "findIndex", "includes",
            "indexOf", "flatMap", "slice", "concat", "reverse", "push", "length",
        ] {
            assert!(dot_labels.contains(m), "Missing array method completion '.{m}'");
        }

        // 5. Go-to-Definition on destructured bindings (`const { count } = s; return count;` and `const [first, second] = arr; return second;`)
        let clean_uri = "file:///tmp/destruct_def.tsk";
        let clean_src = concat!(
            "export type State = { count: i64; label: string; };\n",
            "export function read_vals(s: readonly State, arr: readonly i64[]): i64 {\n",
            "    const { count, label: title } = s;\n",
            "    const [first, second] = arr;\n",
            "    const n = str_len(title);\n",
            "    return count + first + second + n;\n",
            "}\n",
        );
        open_doc(&mut state, clean_uri, clean_src);

        let count_use_col = clean_src.lines().nth(5).unwrap().find("count").unwrap() as u32;
        let def_count = handle_message(
            &mut state,
            &json!({
                "jsonrpc": "2.0",
                "id": 657,
                "method": "textDocument/definition",
                "params": {
                    "textDocument": { "uri": clean_uri },
                    "position": { "line": 5, "character": count_use_col + 1 }
                }
            }),
        );
        assert_eq!(def_count[0]["result"]["uri"], clean_uri);
        assert_eq!(def_count[0]["result"]["range"]["start"]["line"], 2);

        let title_use_col = clean_src.lines().nth(4).unwrap().find("title").unwrap() as u32;
        let def_title = handle_message(
            &mut state,
            &json!({
                "jsonrpc": "2.0",
                "id": 658,
                "method": "textDocument/definition",
                "params": {
                    "textDocument": { "uri": clean_uri },
                    "position": { "line": 4, "character": title_use_col + 1 }
                }
            }),
        );
        assert_eq!(def_title[0]["result"]["range"]["start"]["line"], 2);

        let second_use_col = clean_src.lines().nth(5).unwrap().find("second").unwrap() as u32;
        let def_second = handle_message(
            &mut state,
            &json!({
                "jsonrpc": "2.0",
                "id": 659,
                "method": "textDocument/definition",
                "params": {
                    "textDocument": { "uri": clean_uri },
                    "position": { "line": 5, "character": second_use_col + 1 }
                }
            }),
        );
        assert_eq!(def_second[0]["result"]["range"]["start"]["line"], 3);

        // 6. `knr.toml` `[dependencies]` package import Ctrl+Click, Hover & Completion
        let tmp_dir = std::env::temp_dir().join(format!(
            "knr_lsp_pkg_dep_test_{}",
            std::process::id()
        ));
        let pkg_dir = tmp_dir.join("ui-kit");
        let app_dir = tmp_dir.join("app");
        std::fs::create_dir_all(pkg_dir.join("src")).unwrap();
        std::fs::create_dir_all(app_dir.join("src")).unwrap();

        std::fs::write(
            pkg_dir.join("knr.toml"),
            "[package]\nname = \"ui-kit\"\nversion = \"0.1.0\"\nentry = \"src/lib.tsk\"\n",
        )
        .unwrap();
        let pkg_lib = pkg_dir.join("src/lib.tsk");
        std::fs::write(
            &pkg_lib,
            "export function FancyCard(title: string): string { return title; }\n",
        )
        .unwrap();

        std::fs::write(
            app_dir.join("knr.toml"),
            "[package]\nname = \"demo-app\"\nversion = \"0.1.0\"\nentry = \"src/main.tsk\"\n\n[dependencies]\nui-kit = { path = \"../ui-kit\" }\n",
        )
        .unwrap();
        let app_main = app_dir.join("src/main.tsk");
        let app_main_src = concat!(
            "import { FancyCard } from \"ui-kit\";\n",
            "export function run(): string { return FancyCard(\"hi\"); }\n",
        );
        std::fs::write(&app_main, app_main_src).unwrap();

        let main_uri = path_to_uri(&app_main);
        let pkg_uri = path_to_uri(&pkg_lib);
        open_doc(&mut state, &main_uri, app_main_src);

        // Ctrl+Click on `"ui-kit"` opens `ui-kit/src/lib.tsk`
        let pkg_col = app_main_src.lines().next().unwrap().find("ui-kit").unwrap() as u32;
        let def_pkg = handle_message(
            &mut state,
            &json!({
                "jsonrpc": "2.0",
                "id": 660,
                "method": "textDocument/definition",
                "params": {
                    "textDocument": { "uri": main_uri },
                    "position": { "line": 0, "character": pkg_col + 2 }
                }
            }),
        );
        assert_eq!(def_pkg[0]["result"]["uri"], pkg_uri);

        // Hover on `"ui-kit"` shows `FancyCard`
        let hover_pkg = handle_message(
            &mut state,
            &json!({
                "jsonrpc": "2.0",
                "id": 661,
                "method": "textDocument/hover",
                "params": {
                    "textDocument": { "uri": main_uri },
                    "position": { "line": 0, "character": pkg_col + 2 }
                }
            }),
        );
        assert!(hover_pkg[0]["result"]["contents"]["value"]
            .as_str()
            .unwrap()
            .contains("FancyCard"));

        // Import path completion suggests `"ui-kit"` from `knr.toml` `[dependencies]`
        let pkg_comp_uri = path_to_uri(&app_dir.join("src/scratch.tsk"));
        open_doc(&mut state, &pkg_comp_uri, "import {  } from \"ui\";\n");
        let comp_pkg = handle_message(
            &mut state,
            &json!({
                "jsonrpc": "2.0",
                "id": 662,
                "method": "textDocument/completion",
                "params": {
                    "textDocument": { "uri": pkg_comp_uri },
                    "position": { "line": 0, "character": 20 }
                }
            }),
        );
        let pkg_labels: HashSet<&str> = comp_pkg[0]["result"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|i| i["label"].as_str())
            .collect();
        assert!(pkg_labels.contains("ui-kit"), "Expected ui-kit in {pkg_labels:?}");

        let _ = std::fs::remove_dir_all(&tmp_dir);
    }
}
