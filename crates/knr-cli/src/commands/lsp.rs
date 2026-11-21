use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::io::{self, BufRead, Write};
use std::path::PathBuf;

use knr_core::lower::SourceMap;
use knr_core::syntax::{
    Declaration, EnumDef, FunctionDef, Program, Span, StructDef, TypeExpr, TypeExprKind,
};
use knr_core::ParseError;
use knr_verify::Diagnostic;
use serde_json::{json, Value};

use crate::error::CliError;

const SERVER_NAME: &str = "knr-lsp";
const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");
const MAX_CONTENT_LENGTH: usize = 32 * 1024 * 1024;

// LSP CompletionItemKind constants (LSP 3.17)
const COMPLETION_KIND_FUNCTION: u32 = 3;
const COMPLETION_KIND_FIELD: u32 = 5;
const COMPLETION_KIND_MODULE: u32 = 9;
const COMPLETION_KIND_ENUM: u32 = 13;
const COMPLETION_KIND_KEYWORD: u32 = 14;
const COMPLETION_KIND_SNIPPET: u32 = 15;
const COMPLETION_KIND_STRUCT: u32 = 22;

// LSP InsertTextFormat constants (LSP 3.17)
const INSERT_FORMAT_PLAIN: u32 = 1;
const INSERT_FORMAT_SNIPPET: u32 = 2;

// LSP SymbolKind constants (LSP 3.17)
const SYMBOL_KIND_FIELD: u32 = 8;
const SYMBOL_KIND_ENUM: u32 = 10;
const SYMBOL_KIND_FUNCTION: u32 = 12;
const SYMBOL_KIND_ENUM_MEMBER: u32 = 22;
const SYMBOL_KIND_STRUCT: u32 = 23;

// LSP DiagnosticSeverity constants (LSP 3.17)
const SEVERITY_ERROR: u32 = 1;

/// Persistent state for a `.knr` Language Server session.
#[derive(Debug, Default)]
pub struct LspState {
    pub documents: HashMap<String, String>,
    programs: HashMap<String, Program>,
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
            vec![json_rpc_result(
                req_id,
                json!({
                    "capabilities": {
                        "textDocumentSync": 1,
                        "hoverProvider": true,
                        "completionProvider": {
                            "triggerCharacters": [".", ":", "\""]
                        },
                        "documentSymbolProvider": true
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
        "textDocument/documentSymbol" => {
            let Some(req_id) = id else {
                return Vec::new();
            };
            vec![json_rpc_result(
                req_id,
                handle_document_symbol(state, params),
            )]
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
    let (prog, diagnostics) = analyze_document(uri, &owned);
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
    let (prog, diagnostics) = analyze_document(uri, &owned);
    if let Some(p) = prog {
        state.programs.insert(uri.to_string(), p);
    }
    vec![publish_diagnostics(uri, diagnostics)]
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

    let (prog, diagnostics) = analyze_document(uri, &text);
    if let Some(p) = prog {
        state.programs.insert(uri.to_string(), p);
    }
    vec![publish_diagnostics(uri, diagnostics)]
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

fn analyze_document(uri: &str, text: &str) -> (Option<Program>, Vec<Value>) {
    // 1. Parse high-level `.knr` source into AST
    let prog = match knr_core::syntax::parse_knr(text) {
        Ok(p) => p,
        Err(err) => return (None, vec![parse_error_to_lsp(text, &err)]),
    };

    // 2. Lower AST to canonical SSA IR (resolving relative & stdlib imports when file:// URI has a parent dir)
    let parent_dir = uri_to_path(uri).and_then(|p| p.parent().map(PathBuf::from));
    let (module, source_map) =
        match knr_core::lower::lower_program_with_schemas_and_source_map(&prog, &[], parent_dir) {
            Ok(res) => res,
            Err(lower_err) => {
                let diag = Diagnostic::from_lower_error(lower_err);
                return (Some(prog), vec![knr_diagnostic_to_lsp(text, &diag)]);
            }
        };

    // 3. Static verification (schemas, effects, linear ownership, SSA dominance, types, shader purity)
    if let Err(mut verify_diags) = knr_verify::verify_module(&module) {
        let mut lsp_diags = Vec::with_capacity(verify_diags.len());
        for diag in &mut verify_diags {
            enrich_diagnostic_with_source_map(diag, &source_map);
            lsp_diags.push(knr_diagnostic_to_lsp(text, diag));
        }
        return (Some(prog), lsp_diags);
    }

    // 4. Validate every `shader fn` compiles cleanly to naga-verified WGSL + WebGL2 GLSL ES 3.00
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
            "Ensure shader fn uses pure GPU arithmetic/structs and returns Color/Vec4/Vec3 or f64",
        );
        if let Some(s) = span {
            diag = diag.with_column(s.col).with_span(s.start, s.end);
        }
        return (Some(prog), vec![knr_diagnostic_to_lsp(text, &diag)]);
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
    let Some((ident, range)) = identifier_at_position(text, line, character) else {
        return Value::Null;
    };

    let prog = program_for_uri(state, uri);

    // 1. Check top-level declarations in the parsed AST
    if let Some(ref program) = prog {
        if let Some(markdown) = lookup_program_declaration_hover(program, &ident) {
            return make_hover_response(&markdown, range);
        }
    }

    // 2. Check built-in math/UI/linear intrinsics, keywords, and standard UI schemas
    if let Some(markdown) = builtin_hover_doc(&ident) {
        return make_hover_response(&markdown, range);
    }

    // 3. Check Style and Widget schema fields
    if let Some(markdown) = ui_field_hover_doc(&ident) {
        return make_hover_response(&markdown, range);
    }

    // 4. Check struct fields and enum variants declared in the current document
    if let Some(ref program) = prog {
        if let Some(markdown) = lookup_program_member_hover(program, &ident) {
            return make_hover_response(&markdown, range);
        }
    }

    Value::Null
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
                    "\n\nGPU-pure fragment shader compiled to WGSL (Desktop `wgpu` / WebGPU) and GLSL ES 3.00 (WebGL2)."
                } else if !f.effects.is_empty() {
                    "\n\nEffectful `.knr` function with statically verified capability boundaries."
                } else {
                    "\n\nPure `.knr` function."
                };
                return Some(format!("```knr\n{sig}\n```{note}"));
            }
            Declaration::Struct(s) if s.name.name == ident => {
                let formatted = format_struct_def(s);
                return Some(format!(
                    "```knr\n{formatted}\n```\n\nNominal `.knr` schema with {} field(s).",
                    s.fields.len()
                ));
            }
            Declaration::Enum(e) if e.name.name == ident => {
                let formatted = format_enum_def(e);
                return Some(format!(
                    "```knr\n{formatted}\n```\n\nTagged union with {} variant(s).",
                    e.variants.len()
                ));
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
                            "```knr\n{}.{}: {ty}\n```\n\nField slot `{idx}` of schema `{}`.",
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
                            "```knr\n{}::{}{payload}\n```\n\nVariant of enum `{}`.",
                            e.name.name, variant.name.name, e.name.name
                        ));
                    }
                }
            }
            Declaration::Function(_) => {}
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
        "syscall" => "```knr\nsyscall(Effect, \"op\", payload)\n```\n\nInvokes a host runtime system call under capability `Effect` (`Net`, `Ui`, `Hw`, or `Async`). The enclosing function must declare `effects(Effect)`.",
        "shader" => "```knr\nshader fn name(uv: Vec2, time: f64, ...) -> Color\n```\n\nDeclares a GPU-pure fragment shader kernel. Verified by `knr-verify` and compiled via `naga` to WGSL (Desktop `wgpu` + WebGPU) and GLSL ES 3.00 (WebGL2).",
        "effects" => "```knr\neffects(Net, Ui, Hw, Async)\n```\n\nCapability clause declaring which side-effect domains a function (or its transitive callees) may invoke.",

        // Standard UI & Shader Schemas
        "Style" => "```knr\nschema Style {\n    0: w f64, 1: h f64, 2: min_w f64, 3: max_w f64,\n    4: bg str, 5: color str, 6: border str, 7: border_w f64,\n    8: r_tl f64, 9: r_tr f64, 10: r_br f64, 11: r_bl f64,\n    12: pad_t f64, 13: pad_r f64, 14: pad_b f64, 15: pad_l f64,\n    16: gap_x f64, 17: gap_y f64, 18: font_size f64, 19: font_weight i64,\n    20: align str, 21: justify str, 22: grid_cols i64,\n    23: shadow_x f64, 24: shadow_y f64, 25: shadow_blur f64, 26: shadow_color str,\n    27: backdrop_blur f64, 28: opacity f64,\n    29: translate_x f64, 30: translate_y f64, 31: scale f64, 32: rotate_deg f64,\n    33: hover_bg str, 34: hover_border str, 35: hover_scale f64, 36: transition_ms f64,\n    37: overflow str, 38: z_index i64, 39: grow f64,\n}\n```\n\n40-field statically typed UI style schema rendered identically on Native GPU Desktop (`wgpu`) and Web (`WASM + WebGPU/WebGL2`). Compose overrides with `..style_default()` or `..style_card()`.",
        "Widget" => "```knr\nschema Widget {\n    0: key str,\n    1: kind str,\n    2: text str,\n    3: action str,\n    4: value f64,\n    5: uniforms vec[f64],\n    6: style Style,\n    7: children vec[Widget],\n}\n```\n\nDeclarative MVU UI tree node (`col`, `row`, `grid`, `stack`, `card`, `text`, `badge`, `button`, `input`, `slider`, `toggle`, `shader`).",
        "Vec2" => "```knr\nschema Vec2 {\n    0: x f64,\n    1: y f64,\n}\n```\n\n2D floating-point coordinate vector (`uv.x`, `uv.y` in `[0.0, 1.0]`).",
        "Color" => "```knr\nschema Color {\n    0: r f64,\n    1: g f64,\n    2: b f64,\n    3: a f64,\n}\n```\n\nNormalized RGBA color (`0.0..=1.0`) returned by `shader fn` kernels.",
        _ => return None,
    };
    Some(doc.to_string())
}

fn ui_field_hover_doc(ident: &str) -> Option<String> {
    for &(name, ty, desc) in STYLE_FIELDS {
        if name == ident {
            return Some(format!("```knr\nStyle.{name}: {ty}\n```\n\n{desc}"));
        }
    }
    for &(name, ty, desc) in WIDGET_FIELDS {
        if name == ident {
            return Some(format!("```knr\nWidget.{name}: {ty}\n```\n\n{desc}"));
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

/// All 40 fields of `schema Style` from `std/ui` (`examples/std_ui.knr`).
const STYLE_FIELDS: &[(&str, &str, &str)] = &[
    ("w", "f64", "Fixed width in logical pixels (0.0 = auto)."),
    ("h", "f64", "Fixed height in logical pixels (0.0 = auto)."),
    ("min_w", "f64", "Minimum width constraint in logical pixels."),
    ("max_w", "f64", "Maximum width constraint in logical pixels."),
    ("bg", "str", "Background fill color (`#RGB`, `#RRGGBB`, `#RRGGBBAA`, or gradient string)."),
    ("color", "str", "Foreground text color (`#RRGGBB` / `#RRGGBBAA`)."),
    ("border", "str", "Border stroke color (`#RRGGBB` / `#RRGGBBAA`)."),
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
    ("font_size", "f64", "Font size in logical pixels."),
    ("font_weight", "i64", "Font weight (e.g. 400 regular, 600 semibold, 700 bold)."),
    ("align", "str", "Cross-axis alignment (`\"start\"`, `\"center\"`, `\"end\"`, `\"stretch\"`)."),
    ("justify", "str", "Main-axis justification (`\"start\"`, `\"center\"`, `\"end\"`, `\"between\"`, `\"around\"`)."),
    ("grid_cols", "i64", "Number of columns when widget kind is `\"grid\"`."),
    ("shadow_x", "f64", "Drop shadow horizontal offset in pixels."),
    ("shadow_y", "f64", "Drop shadow vertical offset in pixels."),
    ("shadow_blur", "f64", "Drop shadow blur radius in pixels."),
    ("shadow_color", "str", "Drop shadow color (`#RRGGBBAA`)."),
    ("backdrop_blur", "f64", "Glassmorphism backdrop blur radius in logical pixels."),
    ("opacity", "f64", "Element opacity in `[0.0, 1.0]`."),
    ("translate_x", "f64", "Horizontal transform translation in pixels."),
    ("translate_y", "f64", "Vertical transform translation in pixels."),
    ("scale", "f64", "Uniform transform scale factor (default `1.0`)."),
    ("rotate_deg", "f64", "2D rotation angle in degrees."),
    ("hover_bg", "str", "Background color while pointer hovers the widget."),
    ("hover_border", "str", "Border color while pointer hovers the widget."),
    ("hover_scale", "f64", "Transform scale factor while pointer hovers the widget."),
    ("transition_ms", "f64", "Animated style transition duration in milliseconds."),
    ("overflow", "str", "Overflow mode (`\"visible\"`, `\"hidden\"`, `\"scroll\"`)."),
    ("z_index", "i64", "Layer stacking order (higher renders on top)."),
    ("grow", "f64", "Flex grow factor inside a `row` or `col` container."),
];

/// All 8 fields of `schema Widget` from `std/ui` (`examples/std_ui.knr`).
const WIDGET_FIELDS: &[(&str, &str, &str)] = &[
    ("key", "str", "Stable unique identity key for diffing and animations."),
    ("kind", "str", "Widget kind (`\"col\"`, `\"row\"`, `\"grid\"`, `\"stack\"`, `\"card\"`, `\"text\"`, `\"badge\"`, `\"button\"`, `\"input\"`, `\"slider\"`, `\"toggle\"`, `\"shader\"`)."),
    ("text", "str", "Label, text content, or `shader fn` name (when `kind == \"shader\"`)."),
    ("action", "str", "Action identifier dispatched to `update`, `on_slider`, or `on_input`."),
    ("value", "f64", "Numeric widget value (slider position in `0.0..=1.0`, toggle `0.0`/`1.0`)."),
    ("uniforms", "vec[f64]", "Custom float uniform parameters forwarded to GPU `shader fn`."),
    ("style", "Style", "40-field statically typed visual & layout `Style`."),
    ("children", "vec[Widget]", "Nested child `Widget` nodes."),
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
        label: "fn",
        kind: COMPLETION_KIND_SNIPPET,
        detail: "Declare a function",
        insert_text: "fn ${1:name}(${2:params}) -> ${3:i64} {\n    $0\n}",
        insert_format: INSERT_FORMAT_SNIPPET,
        doc: "Declares a statically verified `.knr` function.",
    },
    KeywordSnippet {
        label: "shader fn",
        kind: COMPLETION_KIND_SNIPPET,
        detail: "Declare a GPU-pure shader function",
        insert_text: "shader fn ${1:name}(uv: Vec2, time: f64) -> Color {\n    return Color { r: uv.x, g: uv.y, b: 0.5 + 0.5 * sin(time), a: 1.0 };\n}",
        insert_format: INSERT_FORMAT_SNIPPET,
        doc: "Declares a GPU-pure fragment shader compiled to WGSL and WebGL2 GLSL.",
    },
    KeywordSnippet {
        label: "schema",
        kind: COMPLETION_KIND_SNIPPET,
        detail: "Declare a nominal schema / struct",
        insert_text: "schema ${1:Name} {\n    0: ${2:field} ${3:i64},\n}",
        insert_format: INSERT_FORMAT_SNIPPET,
        doc: "Declares a statically typed nominal schema with explicit slot indices.",
    },
    KeywordSnippet {
        label: "enum",
        kind: COMPLETION_KIND_SNIPPET,
        detail: "Declare a tagged union enum",
        insert_text: "enum ${1:Msg} {\n    ${2:VariantA},\n    ${3:VariantB}(${4:i64}),\n}",
        insert_format: INSERT_FORMAT_SNIPPET,
        doc: "Declares a tagged union enum lowered to a verified tagged schema.",
    },
    KeywordSnippet {
        label: "import \"std/ui\";",
        kind: COMPLETION_KIND_MODULE,
        detail: "Import standard UI, Style, Widget & animation primitives",
        insert_text: "import \"std/ui\";",
        insert_format: INSERT_FORMAT_PLAIN,
        doc: "Imports `Vec2`, `Color`, `Style` (40 fields), `Widget`, widget constructors, and animation helpers.",
    },
    KeywordSnippet {
        label: "import \"std/math\";",
        kind: COMPLETION_KIND_MODULE,
        detail: "Import standard math helpers",
        insert_text: "import \"std/math\";",
        insert_format: INSERT_FORMAT_PLAIN,
        doc: "Imports standard `.knr` mathematical helper functions and constants.",
    },
    KeywordSnippet {
        label: "while",
        kind: COMPLETION_KIND_KEYWORD,
        detail: "While loop",
        insert_text: "while ${1:cond} {\n    $0\n}",
        insert_format: INSERT_FORMAT_SNIPPET,
        doc: "Structured loop lowered to SSA phi/block-argument headers.",
    },
    KeywordSnippet {
        label: "match",
        kind: COMPLETION_KIND_KEYWORD,
        detail: "Pattern match expression",
        insert_text: "match ${1:expr} {\n    ${2:Pattern} => ${3:result},\n    _ => ${4:default},\n}",
        insert_format: INSERT_FORMAT_SNIPPET,
        doc: "Exhaustive pattern matching on enums and primitives.",
    },
    KeywordSnippet {
        label: "if",
        kind: COMPLETION_KIND_KEYWORD,
        detail: "Conditional branch",
        insert_text: "if ${1:cond} {\n    $0\n}",
        insert_format: INSERT_FORMAT_SNIPPET,
        doc: "Conditional statement or expression.",
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
        label: "effects(Net)",
        kind: COMPLETION_KIND_KEYWORD,
        detail: "Declare network effect capability",
        insert_text: "effects(Net)",
        insert_format: INSERT_FORMAT_PLAIN,
        doc: "Grants the function permission to invoke `Net` syscalls.",
    },
    KeywordSnippet {
        label: "effects(Ui)",
        kind: COMPLETION_KIND_KEYWORD,
        detail: "Declare UI effect capability",
        insert_text: "effects(Ui)",
        insert_format: INSERT_FORMAT_PLAIN,
        doc: "Grants the function permission to invoke `Ui` syscalls.",
    },
    KeywordSnippet {
        label: "effects(Hw)",
        kind: COMPLETION_KIND_KEYWORD,
        detail: "Declare hardware effect capability",
        insert_text: "effects(Hw)",
        insert_format: INSERT_FORMAT_PLAIN,
        doc: "Grants the function permission to invoke `Hw` syscalls.",
    },
    KeywordSnippet {
        label: "effects(Async)",
        kind: COMPLETION_KIND_KEYWORD,
        detail: "Declare async/channel effect capability",
        insert_text: "effects(Async)",
        insert_format: INSERT_FORMAT_PLAIN,
        doc: "Grants the function permission to create channels and invoke `Async` syscalls.",
    },
    KeywordSnippet {
        label: "syscall",
        kind: COMPLETION_KIND_SNIPPET,
        detail: "Invoke an effectful runtime syscall",
        insert_text: "syscall(${1:Ui}, \"${2:op}\", ${3:payload})",
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

    let line_prefix = uri.and_then(|u| {
        let text = state.documents.get(u)?;
        let line = params
            .and_then(|p| p.pointer("/position/line"))
            .and_then(Value::as_u64)? as usize;
        let col = params
            .and_then(|p| p.pointer("/position/character"))
            .and_then(Value::as_u64)? as u32;
        let line_str = text.split('\n').nth(line)?.trim_end_matches('\r');
        let byte_idx = utf16_col_to_byte_idx(line_str, col);
        Some(line_str[..byte_idx].to_string())
    });

    let prefer_fields = trigger == Some(".")
        || line_prefix
            .as_deref()
            .is_some_and(|p| p.ends_with('.'));
    let prefer_imports = trigger == Some("\"")
        || line_prefix
            .as_deref()
            .is_some_and(|p| p.contains("import"));

    let mut items: Vec<Value> = Vec::new();
    let mut seen: HashSet<(String, u32)> = HashSet::new();

    // 1. Document-local declarations (fn, shader fn, schema, enum) + their fields
    if let Some(u) = uri {
        if let Some(prog) = program_for_uri(state, u) {
            for decl in &prog.declarations {
                match decl {
                    Declaration::Function(f) => {
                        let sig = format_function_signature(f);
                        let key = (f.name.name.clone(), COMPLETION_KIND_FUNCTION);
                        if seen.insert(key) {
                            let sort = if prefer_fields { "3" } else { "1" };
                            let doc = if f.is_shader {
                                format!("GPU shader declared in current document.\n\n```knr\n{sig}\n```")
                            } else {
                                format!("Function declared in current document.\n\n```knr\n{sig}\n```")
                            };
                            items.push(json!({
                                "label": f.name.name,
                                "kind": COMPLETION_KIND_FUNCTION,
                                "detail": sig,
                                "documentation": { "kind": "markdown", "value": doc },
                                "sortText": format!("{sort}_{}", f.name.name)
                            }));
                        }
                    }
                    Declaration::Struct(s) => {
                        let key = (s.name.name.clone(), COMPLETION_KIND_STRUCT);
                        if seen.insert(key) {
                            let sort = if prefer_fields { "3" } else { "1" };
                            items.push(json!({
                                "label": s.name.name,
                                "kind": COMPLETION_KIND_STRUCT,
                                "detail": format!("schema {} ({} fields)", s.name.name, s.fields.len()),
                                "documentation": {
                                    "kind": "markdown",
                                    "value": format!("```knr\n{}\n```", format_struct_def(s))
                                },
                                "sortText": format!("{sort}_{}", s.name.name)
                            }));
                        }
                        // Also offer local schema fields when accessing with `.`
                        for field in &s.fields {
                            let fkey = (field.name.name.clone(), COMPLETION_KIND_FIELD);
                            if seen.insert(fkey) {
                                let sort = if prefer_fields { "0" } else { "2" };
                                let ty = format_type_expr(&field.ty);
                                items.push(json!({
                                    "label": field.name.name,
                                    "kind": COMPLETION_KIND_FIELD,
                                    "detail": format!("{}.{}: {ty}", s.name.name, field.name.name),
                                    "sortText": format!("{sort}_{}", field.name.name)
                                }));
                            }
                        }
                    }
                    Declaration::Enum(e) => {
                        let key = (e.name.name.clone(), COMPLETION_KIND_ENUM);
                        if seen.insert(key) {
                            let sort = if prefer_fields { "3" } else { "1" };
                            items.push(json!({
                                "label": e.name.name,
                                "kind": COMPLETION_KIND_ENUM,
                                "detail": format!("enum {} ({} variants)", e.name.name, e.variants.len()),
                                "documentation": {
                                    "kind": "markdown",
                                    "value": format!("```knr\n{}\n```", format_enum_def(e))
                                },
                                "sortText": format!("{sort}_{}", e.name.name)
                            }));
                        }
                    }
                }
            }
        }
    }

    // 2. Keywords & snippets
    for kw in KEYWORD_SNIPPETS {
        let key = (kw.label.to_string(), kw.kind);
        if seen.insert(key) {
            let sort = if prefer_imports && kw.label.starts_with("import") {
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

    // 4. All 40 `Style` fields
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

    // 6. Standard UI schemas if not already declared in this file
    for (name, detail) in [
        ("Style", "40-field UI styling schema"),
        ("Widget", "Declarative MVU UI widget schema"),
        ("Vec2", "2D floating-point vector schema"),
        ("Color", "Normalized RGBA color schema"),
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
        }
    }

    Value::Array(symbols)
}

// ============================================================================
// AST Formatting, Position Conversion & URI Utilities
// ============================================================================

fn program_for_uri(state: &LspState, uri: &str) -> Option<Program> {
    if let Some(text) = state.documents.get(uri) {
        if let Ok(prog) = knr_core::syntax::parse_knr(text) {
            return Some(prog);
        }
    }
    state.programs.get(uri).cloned()
}

fn format_type_expr(ty: &TypeExpr) -> String {
    match &ty.kind {
        TypeExprKind::Named(ident) => ident.name.clone(),
        TypeExprKind::Vec(inner) => format!("vec[{}]", format_type_expr(inner)),
        TypeExprKind::Map(k, v) => {
            format!("map[{}, {}]", format_type_expr(k), format_type_expr(v))
        }
        TypeExprKind::Chan(inner) => format!("chan[{}]", format_type_expr(inner)),
        TypeExprKind::Struct(ident) => format!("struct[{}]", ident.name),
    }
}

fn format_function_signature(f: &FunctionDef) -> String {
    let prefix = if f.is_shader { "shader fn" } else { "fn" };
    let params: Vec<String> = f
        .params
        .iter()
        .map(|p| {
            let borrow = if p.is_borrowed { "&" } else { "" };
            format!("{}: {}{}", p.name.name, borrow, format_type_expr(&p.ty))
        })
        .collect();
    let ret = format_type_expr(&f.ret_ty);
    let ret_part = if ret == "void" {
        String::new()
    } else {
        format!(" -> {ret}")
    };
    let eff_part = if f.effects.is_empty() {
        String::new()
    } else {
        let effs: Vec<&str> = f.effects.iter().map(|e| e.name.as_str()).collect();
        format!(" effects({})", effs.join(", "))
    };
    format!(
        "{prefix} {}({}){ret_part}{eff_part}",
        f.name.name,
        params.join(", ")
    )
}

fn format_struct_def(s: &StructDef) -> String {
    if s.fields.is_empty() {
        return format!("schema {} {{}}", s.name.name);
    }
    let mut out = format!("schema {} {{\n", s.name.name);
    for (i, field) in s.fields.iter().enumerate() {
        let idx = field.index.unwrap_or(i as u32);
        let _ = writeln!(
            out,
            "    {idx}: {} {},",
            field.name.name,
            format_type_expr(&field.ty)
        );
    }
    out.push('}');
    out
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
        let uri = "file:///tmp/demo.knr";

        // 1. Valid program -> empty diagnostics
        let valid_src = "fn add(a: i64, b: i64) -> i64 {\n    return a + b;\n}\n";
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
        let bad_syntax = "fn broken( {\n    return 1;\n}\n";
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
        let type_err = "fn mismatch() -> i64 {\n    return \"not an int\";\n}\n";
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
        let uri = "file:///tmp/shader_demo.knr";
        let src = concat!(
            "schema Vec2 { 0: x f64, 1: y f64 }\n",
            "schema Color { 0: r f64, 1: g f64, 2: b f64, 3: a f64 }\n",
            "enum Mode { Calm, Hyper(f64) }\n",
            "shader fn aurora_waves(uv: Vec2, time: f64) -> Color {\n",
            "    let s = smoothstep(0.0, 1.0, sin(uv.x + time));\n",
            "    return Color { r: s, g: uv.y, b: 0.8, a: 1.0 };\n",
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
            "Valid shader file should produce zero diagnostics, got: {:?}",
            open_out[0]["params"]["diagnostics"]
        );

        // Hover on `aurora_waves` (line 3, char 12)
        let hover_fn = handle_message(
            &mut state,
            &json!({
                "jsonrpc": "2.0",
                "id": 10,
                "method": "textDocument/hover",
                "params": {
                    "textDocument": { "uri": uri },
                    "position": { "line": 3, "character": 12 }
                }
            }),
        );
        let hover_text = hover_fn[0]["result"]["contents"]["value"].as_str().unwrap();
        assert!(hover_text.contains("shader fn aurora_waves(uv: Vec2, time: f64) -> Color"));

        // Hover on builtin `smoothstep` (line 4, char 14)
        let hover_builtin = handle_message(
            &mut state,
            &json!({
                "jsonrpc": "2.0",
                "id": 11,
                "method": "textDocument/hover",
                "params": {
                    "textDocument": { "uri": uri },
                    "position": { "line": 4, "character": 14 }
                }
            }),
        );
        let builtin_text = hover_builtin[0]["result"]["contents"]["value"]
            .as_str()
            .unwrap();
        assert!(builtin_text.contains("smoothstep"));

        // Completions include local decls, keywords, builtins, Style fields, and Widget fields
        let comp = handle_message(
            &mut state,
            &json!({
                "jsonrpc": "2.0",
                "id": 12,
                "method": "textDocument/completion",
                "params": {
                    "textDocument": { "uri": uri },
                    "position": { "line": 4, "character": 8 }
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
            "shader fn",
            "import \"std/ui\";",
            "smoothstep",
            "vec_push",
            "backdrop_blur",
            "hover_scale",
            "uniforms",
            "children",
        ] {
            assert!(labels.contains(expected), "Missing completion: {expected}");
        }

        // Document symbols include Vec2, Color, Mode, and aurora_waves with accurate kinds
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
        assert_eq!(sym_list.len(), 4);
        assert_eq!(sym_list[0]["name"], "Vec2");
        assert_eq!(sym_list[0]["kind"], SYMBOL_KIND_STRUCT);
        assert_eq!(sym_list[2]["name"], "Mode");
        assert_eq!(sym_list[2]["kind"], SYMBOL_KIND_ENUM);
        assert_eq!(sym_list[3]["name"], "aurora_waves");
        assert_eq!(sym_list[3]["kind"], SYMBOL_KIND_FUNCTION);
        assert_eq!(sym_list[3]["range"]["start"]["line"], 3);
        assert_eq!(sym_list[3]["range"]["end"]["line"], 6);
    }

    #[test]
    fn uri_to_path_decodes_file_uris_safely() {
        assert_eq!(
            uri_to_path("file:///home/dev/my%20app/main.knr"),
            Some(PathBuf::from("/home/dev/my app/main.knr"))
        );
        assert_eq!(
            uri_to_path("file://localhost/tmp/test.knr"),
            Some(PathBuf::from("/tmp/test.knr"))
        );
        assert_eq!(uri_to_path("untitled:Untitled-1"), None);
    }

    #[test]
    fn identifier_lookup_is_utf8_safe() {
        let src = "let msg = \"🚀 café\"; let alpha = sin(1.0);";
        // "🚀" is 2 UTF-16 code units; verify arbitrary UTF-16 column lookups never panic
        for col in 0..60 {
            let _ = identifier_at_position(src, 0, col);
        }
        // `sin` spans UTF-16 columns 33..36
        let (ident, range) = identifier_at_position(src, 0, 33).unwrap();
        assert_eq!(ident, "sin");
        assert_eq!(range["start"]["character"], 33);
        assert_eq!(range["end"]["character"], 36);
    }

    #[test]
    fn shader_compile_error_and_did_save_publish_diagnostics() {
        let mut state = LspState::new();
        let uri = "file:///tmp/bad_shader.knr";
        let src = concat!(
            "schema Vec2 { 0: x f64, 1: y f64 }\n",
            "shader fn bad_ret(uv: Vec2, time: f64) -> i64 {\n",
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
        assert_eq!(diags[0]["range"]["start"]["line"], 1);

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
}
