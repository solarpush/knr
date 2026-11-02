use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::ast::{Effect, FieldDef, Function, Op, RegId, Schema, Type};
use crate::error::ParseError;
use crate::parser::{
    parse_import_directive, parse_schema_name_peek, parse_ssa_with_role_and_env,
    prescan_fn_header, strip_comment, FileRole,
};

/// Represents one source file in the project topology.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileNode {
    /// Relative display path (e.g. "site/components/page.ssa").
    pub path: String,
    /// Schema or Code role.
    pub role: FileRole,
    /// Relative display paths of imported files.
    pub imports: Vec<String>,
    /// Sorted list of schema names declared in this file.
    pub schemas: Vec<String>,
    /// Sorted list of function names declared in this file.
    pub functions: Vec<String>,
}

/// Captures a schema usage site.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SchemaUsage {
    /// Enclosing function where the usage occurs.
    pub function: String,
    /// Relative path of the file where the usage occurs.
    pub file: String,
    /// Line number of the instruction or declaration.
    pub line: usize,
    /// Kind of usage: "struct_new", "struct_get#<idx>", "struct_set#<idx>", "param", "ret", "vec_new".
    pub op_kind: String,
}

/// Captures a schema definition and cross-references.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SchemaNode {
    /// Name of the schema.
    pub name: String,
    /// Relative file path where declared.
    pub file: String,
    /// 1-based line number of the `schema <name>` header.
    pub line: usize,
    /// Field declarations (index, name, ty).
    pub fields: Vec<FieldDef>,
    /// Cross-reference usages across all functions.
    pub usages: Vec<SchemaUsage>,
}

/// A direct call site in a function body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallSite {
    /// Target function name.
    pub callee: String,
    /// Line number of the `call` instruction.
    pub line: usize,
}

/// An incoming caller invoking a function.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallerSite {
    /// Enclosing caller function name.
    pub caller: String,
    /// File where the call instruction is located.
    pub file: String,
    /// Line number of the `call` instruction.
    pub line: usize,
}

/// A syscall invocation site.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyscallSite {
    /// Effect capability: Net, Ui, Hw, Async.
    pub effect: Effect,
    /// Opcode string: e.g. "ui.render", "net.listen", "net.wait_req".
    pub op: String,
    /// Line number of the `syscall` instruction.
    pub line: usize,
}

/// Function node with call graph, effects, struct ops, and UI actions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FunctionNode {
    /// Function name.
    pub name: String,
    /// Relative file path where declared.
    pub file: String,
    /// 1-based line number of the `fn <name>` header.
    pub line: usize,
    /// Parameters: register ID and type.
    pub params: Vec<(RegId, Type)>,
    /// Declared return type.
    pub ret_ty: Type,
    /// Declared effects: Ui, Net, Hw, Async.
    pub effects: Vec<Effect>,
    /// Callee sites directly invoked by this function.
    pub calls: Vec<CallSite>,
    /// Callers directly invoking this function across the project.
    pub called_by: Vec<CallerSite>,
    /// Syscalls directly invoked by this function.
    pub syscalls: Vec<SyscallSite>,
    /// Deduplicated summary of struct operations: "new <Schema>", "get <Schema>#<idx>", "set <Schema>#<idx>".
    pub struct_ops: Vec<String>,
    /// String constants set on `Leaf.action` or compared as HTTP routes.
    pub ui_actions: Vec<String>,
}

/// Dead code analysis report.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrphanReport {
    /// Schema names with 0 usages in any function and 0 references in any other schema field.
    pub unused_schemas: Vec<String>,
    /// Function names with 0 incoming calls, excluding entry points "main" and "knr_dispatch".
    pub uncalled_functions: Vec<String>,
}

impl OrphanReport {
    pub fn is_empty(&self) -> bool {
        self.unused_schemas.is_empty() && self.uncalled_functions.is_empty()
    }
}

/// The root container holding the complete structural graph.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectGraph {
    /// All discovered files with topology metadata.
    pub files: Vec<FileNode>,
    /// All schemas with field definitions and usages.
    pub schemas: Vec<SchemaNode>,
    /// All functions with call graph, effects, struct ops, and UI actions.
    pub functions: Vec<FunctionNode>,
    /// Dead code analysis report.
    pub orphans: OrphanReport,
}

/// Supported output formats for `knr explore`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ExploreOutputFormat {
    #[default]
    Ai,
    Json,
    Mermaid,
}

/// Query filter options for inspecting a `ProjectGraph`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExploreFilter {
    /// Matches a schema name or function name, displaying its definition
    /// and all cross-references (usages, callers, callees).
    pub symbol: Option<String>,

    /// Computes and displays direct and transitive callers of the specified function.
    pub callers: Option<String>,

    /// Computes and displays direct and transitive callees plus syscalls of the specified function.
    pub callees: Option<String>,

    /// Filters functions declaring or invoking the specified effect.
    pub effects: Option<String>,

    /// Restricts output solely to orphan schemas and uncalled functions.
    pub orphans_only: bool,
}

impl ExploreFilter {
    pub fn is_empty(&self) -> bool {
        self.symbol.is_none()
            && self.callers.is_none()
            && self.callees.is_none()
            && self.effects.is_none()
            && !self.orphans_only
    }
}

/// Transitive caller entry with origin location and depth.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransitiveCaller {
    pub caller: String,
    pub file: String,
    pub line: usize,
    pub depth: usize,
}

/// Callee edge with caller, callee, call site, definition location, and depth.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CalleeEdge {
    pub caller: String,
    pub callee: String,
    pub call_line: usize,
    pub def_file: String,
    pub def_line: usize,
    pub depth: usize,
}

/// Syscall occurrence within a callee traversal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyscallOccurrence {
    pub in_fn: String,
    pub effect: String,
    pub op: String,
    pub file: String,
    pub line: usize,
    pub depth: usize,
}

/// Callee traversal report containing all reachable callees and syscalls.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CalleeTraversalReport {
    pub target: String,
    pub callees: Vec<CalleeEdge>,
    pub syscalls: Vec<SyscallOccurrence>,
}

// ============================================================================
// Public Entrypoint: explore_path
// ============================================================================

/// Explores a single SSA/schema file or an entire project directory.
pub fn explore_path(path: &Path) -> Result<ProjectGraph, ParseError> {
    if !path.exists() {
        return Err(ParseError::FileNotFound {
            line: 1,
            path: path.display().to_string(),
        });
    }

    let canonical = fs::canonicalize(path).map_err(|e| ParseError::IoError {
        path: path.display().to_string(),
        msg: e.to_string(),
    })?;

    if canonical.is_file() {
        explore_file(&canonical, path)
    } else if canonical.is_dir() {
        explore_directory(&canonical, path)
    } else {
        Err(ParseError::IoError {
            path: path.display().to_string(),
            msg: "Path is neither a regular file nor a directory".to_string(),
        })
    }
}

// ============================================================================
// File Mode: explore_file
// ============================================================================

fn explore_file(canonical: &Path, root_hint: &Path) -> Result<ProjectGraph, ParseError> {
    let file_str = canonical.to_string_lossy();
    if !(file_str.ends_with(".ssa.schema") || file_str.ends_with(".ssa")) {
        return Err(ParseError::InvalidSyntax {
            line: 1,
            msg: format!(
                "File '{}' has invalid extension; expected '.ssa' or '.ssa.schema'",
                root_hint.display()
            ),
        });
    }

    let mut state = FileExplorerState::new(root_hint);
    state.visit_file(canonical, 1)?;
    state.into_graph()
}

struct FileExplorerState<'a> {
    root_hint: &'a Path,
    visited: HashSet<PathBuf>,
    active_stack: Vec<PathBuf>,
    file_order: Vec<PathBuf>,
    raw_files: HashMap<PathBuf, RawFileInfo>,
}

struct RawFileInfo {
    display_path: String,
    role: FileRole,
    imports: Vec<String>,
    schemas: Vec<Schema>,
    functions: Vec<Function>,
    schema_lines: HashMap<String, usize>,
    fn_lines: HashMap<String, usize>,
}

impl<'a> FileExplorerState<'a> {
    fn new(root_hint: &'a Path) -> Self {
        Self {
            root_hint,
            visited: HashSet::new(),
            active_stack: Vec::new(),
            file_order: Vec::new(),
            raw_files: HashMap::new(),
        }
    }

    fn visit_file(&mut self, canonical: &Path, import_line: usize) -> Result<(), ParseError> {
        // 1. Cycle detection
        if let Some(pos) = self.active_stack.iter().position(|p| p == canonical) {
            let cycle = self.active_stack[pos..]
                .iter()
                .chain(std::iter::once(&canonical.to_path_buf()))
                .map(|p| p.file_name().unwrap_or_default().to_string_lossy().to_string())
                .collect::<Vec<_>>();
            return Err(ParseError::CircularImport { cycle });
        }

        // 2. Diamond import deduplication
        if self.visited.contains(canonical) {
            return Ok(());
        }

        let file_str = canonical.to_string_lossy();
        let role = if file_str.ends_with(".ssa.schema") {
            FileRole::Schema
        } else if file_str.ends_with(".ssa") {
            FileRole::Code
        } else {
            return Err(ParseError::InvalidSyntax {
                line: import_line,
                msg: format!(
                    "File '{}' has invalid extension; expected '.ssa' or '.ssa.schema'",
                    canonical.display()
                ),
            });
        };

        let content = fs::read_to_string(canonical).map_err(|e| ParseError::IoError {
            path: canonical.display().to_string(),
            msg: e.to_string(),
        })?;

        self.active_stack.push(canonical.to_path_buf());

        // Scan imports
        let base_dir = canonical.parent().unwrap_or(Path::new("."));
        let mut seen_decl = false;
        let mut file_imports = Vec::new();
        for (i, line) in content.lines().enumerate() {
            let stripped = strip_comment(line);
            let trimmed = stripped.trim();
            if trimmed.is_empty() {
                continue;
            }
            if trimmed.starts_with("import ") || trimmed == "import" {
                if seen_decl {
                    return Err(ParseError::ImportAfterDeclaration { line: i + 1 });
                }
                let imp_path = parse_import_directive(trimmed, i + 1)?;
                file_imports.push((imp_path, i + 1));
            } else if trimmed.starts_with("schema ") || trimmed.starts_with("fn ") {
                seen_decl = true;
            }
        }

        let mut resolved_import_paths = Vec::new();

        // Recursively visit dependencies first
        for (rel_path, line) in &file_imports {
            if role == FileRole::Schema && !rel_path.ends_with(".ssa.schema") {
                return Err(ParseError::InvalidConstructInSchemaFile {
                    line: *line,
                    msg: format!(
                        "Schema files may only import other '.ssa.schema' files, but found '{rel_path}'"
                    ),
                });
            }
            let target_path = base_dir.join(rel_path);
            let target_canonical = match fs::canonicalize(&target_path) {
                Ok(c) => c,
                Err(e) => {
                    if e.kind() == std::io::ErrorKind::NotFound {
                        return Err(ParseError::FileNotFound {
                            line: *line,
                            path: target_path.display().to_string(),
                        });
                    } else {
                        return Err(ParseError::IoError {
                            path: target_path.display().to_string(),
                            msg: e.to_string(),
                        });
                    }
                }
            };
            resolved_import_paths.push(clean_display_path(&target_canonical, self.root_hint));
            self.visit_file(&target_canonical, *line)?;
        }

        // Collect existing schemas and functions from visited dependencies
        let mut accumulated_schemas = Vec::new();
        let mut accumulated_functions = Vec::new();
        for path in &self.file_order {
            if let Some(info) = self.raw_files.get(path) {
                accumulated_schemas.extend(info.schemas.clone());
                accumulated_functions.extend(info.functions.clone());
            }
        }

        let parsed_module = parse_ssa_with_role_and_env(
            &content,
            role,
            &accumulated_schemas,
            &accumulated_functions,
        )?;

        let (schema_lines, fn_lines) = prescan_decl_lines(&content);
        let display_path = clean_display_path(canonical, self.root_hint);

        let info = RawFileInfo {
            display_path,
            role,
            imports: resolved_import_paths,
            schemas: parsed_module.schemas,
            functions: parsed_module.functions,
            schema_lines,
            fn_lines,
        };

        self.raw_files.insert(canonical.to_path_buf(), info);
        self.file_order.push(canonical.to_path_buf());
        self.visited.insert(canonical.to_path_buf());
        self.active_stack.pop();

        Ok(())
    }

    fn into_graph(self) -> Result<ProjectGraph, ParseError> {
        let mut raw_list = Vec::new();
        for path in &self.file_order {
            if let Some(info) = self.raw_files.get(path) {
                raw_list.push(info);
            }
        }
        build_project_graph_from_raw(&raw_list)
    }
}

// ============================================================================
// Directory Mode: explore_directory
// ============================================================================

fn explore_directory(canonical_dir: &Path, root_hint: &Path) -> Result<ProjectGraph, ParseError> {
    let mut discovered = Vec::new();
    find_ssa_files_recursive(canonical_dir, &mut discovered)?;

    // Separate schema and code files
    let mut schema_files = Vec::new();
    let mut code_files = Vec::new();

    for path in discovered {
        let name = path.to_string_lossy();
        if name.ends_with(".ssa.schema") {
            schema_files.push(path);
        } else if name.ends_with(".ssa") {
            code_files.push(path);
        }
    }

    schema_files.sort();
    code_files.sort();

    let mut raw_files = Vec::new();
    let mut accumulated_schemas = Vec::new();
    let mut schema_origins: HashMap<String, PathBuf> = HashMap::new();

    // 1. Parse all schema files first
    for path in &schema_files {
        let content = fs::read_to_string(path).map_err(|e| ParseError::IoError {
            path: path.display().to_string(),
            msg: e.to_string(),
        })?;

        let parsed = parse_ssa_with_role_and_env(
            &content,
            FileRole::Schema,
            &accumulated_schemas,
            &[],
        )?;

        for s in &parsed.schemas {
            if let Some(prev) = schema_origins.get(&s.name) {
                return Err(ParseError::DuplicateSymbol {
                    symbol: s.name.clone(),
                    first_file: prev.display().to_string(),
                    second_file: path.display().to_string(),
                });
            }
            schema_origins.insert(s.name.clone(), path.clone());
            accumulated_schemas.push(s.clone());
        }

        let imports = resolve_file_imports(&content, path, root_hint);
        let (schema_lines, fn_lines) = prescan_decl_lines(&content);
        let display_path = clean_display_path(path, root_hint);

        raw_files.push(RawFileInfo {
            display_path,
            role: FileRole::Schema,
            imports,
            schemas: parsed.schemas,
            functions: parsed.functions,
            schema_lines,
            fn_lines,
        });
    }

    // 2. Parse all code files
    let mut accumulated_functions = Vec::new();
    for path in &code_files {
        let content = fs::read_to_string(path).map_err(|e| ParseError::IoError {
            path: path.display().to_string(),
            msg: e.to_string(),
        })?;

        let parsed = parse_ssa_with_role_and_env(
            &content,
            FileRole::Code,
            &accumulated_schemas,
            &accumulated_functions,
        )?;

        accumulated_functions.extend(parsed.functions.clone());

        let imports = resolve_file_imports(&content, path, root_hint);
        let (schema_lines, fn_lines) = prescan_decl_lines(&content);
        let display_path = clean_display_path(path, root_hint);

        raw_files.push(RawFileInfo {
            display_path,
            role: FileRole::Code,
            imports,
            schemas: parsed.schemas,
            functions: parsed.functions,
            schema_lines,
            fn_lines,
        });
    }

    let refs: Vec<&RawFileInfo> = raw_files.iter().collect();
    build_project_graph_from_raw(&refs)
}

fn find_ssa_files_recursive(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), ParseError> {
    let entries = fs::read_dir(dir).map_err(|e| ParseError::IoError {
        path: dir.display().to_string(),
        msg: e.to_string(),
    })?;

    for entry in entries {
        let entry = entry.map_err(|e| ParseError::IoError {
            path: dir.display().to_string(),
            msg: e.to_string(),
        })?;
        let path = entry.path();
        let file_name = entry.file_name();
        let name_str = file_name.to_string_lossy();

        if name_str.starts_with('.') || name_str == "target" {
            continue;
        }

        if path.is_dir() {
            find_ssa_files_recursive(&path, out)?;
        } else if path.is_file()
            && (name_str.ends_with(".ssa") || name_str.ends_with(".ssa.schema"))
        {
            if let Ok(c) = fs::canonicalize(&path) {
                out.push(c);
            } else {
                out.push(path);
            }
        }
    }

    Ok(())
}

fn resolve_file_imports(content: &str, file_path: &Path, root_hint: &Path) -> Vec<String> {
    let base_dir = file_path.parent().unwrap_or(Path::new("."));
    let mut out = Vec::new();
    for (i, line) in content.lines().enumerate() {
        let stripped = strip_comment(line).trim();
        if stripped.starts_with("import ") || stripped == "import" {
            if let Ok(rel) = parse_import_directive(stripped, i + 1) {
                let target = base_dir.join(&rel);
                if let Ok(c) = fs::canonicalize(&target) {
                    out.push(clean_display_path(&c, root_hint));
                } else {
                    out.push(rel);
                }
            }
        } else if stripped.starts_with("schema ") || stripped.starts_with("fn ") {
            break;
        }
    }
    out
}

// ============================================================================
// Core Graph Construction & Semantic Extraction
// ============================================================================

fn build_project_graph_from_raw(raw_files: &[&RawFileInfo]) -> Result<ProjectGraph, ParseError> {
    let mut files = Vec::new();
    let mut schemas = Vec::new();
    let mut functions = Vec::new();

    // 1. Build FileNodes and collect raw symbols
    for raw in raw_files {
        let schema_names: Vec<String> = raw.schemas.iter().map(|s| s.name.clone()).collect();
        let fn_names: Vec<String> = raw.functions.iter().map(|f| f.name.clone()).collect();

        files.push(FileNode {
            path: raw.display_path.clone(),
            role: raw.role,
            imports: raw.imports.clone(),
            schemas: schema_names,
            functions: fn_names,
        });

        for s in &raw.schemas {
            let line = raw.schema_lines.get(&s.name).copied().unwrap_or(1);
            schemas.push(SchemaNode {
                name: s.name.clone(),
                file: raw.display_path.clone(),
                line,
                fields: s.fields.clone(),
                usages: Vec::new(),
            });
        }
    }

    // 2. Build FunctionNodes with intra-procedural analysis
    for raw in raw_files {
        for f in &raw.functions {
            let line = raw.fn_lines.get(&f.name).copied().unwrap_or(1);
            let mut calls = Vec::new();
            let mut syscalls = Vec::new();
            let mut struct_ops = Vec::new();
            let mut ui_actions = Vec::new();

            let mut reg_types: HashMap<RegId, Type> = HashMap::new();
            for (reg, ty) in &f.params {
                reg_types.insert(*reg, ty.clone());
            }

            let mut reg_str_consts: HashMap<RegId, String> = HashMap::new();

            for block in &f.blocks {
                for inst in &block.instructions {
                    if let Some(dst) = inst.dst {
                        reg_types.insert(dst, inst.ty.clone());
                    }

                    match &inst.op {
                        Op::ConstStr(s) => {
                            if let Some(dst) = inst.dst {
                                reg_str_consts.insert(dst, s.clone());
                            }
                            if s.starts_with("/api/") {
                                ui_actions.push(s.clone());
                            }
                        }
                        Op::Call(callee, _) => {
                            calls.push(CallSite {
                                callee: callee.clone(),
                                line: inst.line,
                            });
                        }
                        Op::Syscall(eff, op, _) => {
                            syscalls.push(SyscallSite {
                                effect: *eff,
                                op: op.clone(),
                                line: inst.line,
                            });
                        }
                        Op::StructNew(name) => {
                            if let Some(dst) = inst.dst {
                                reg_types.insert(dst, Type::Struct(name.clone()));
                            }
                            struct_ops.push(format!("new {name}"));
                        }
                        Op::StructGet(reg, idx) => {
                            if let Some(Type::Struct(name)) = reg_types.get(reg) {
                                struct_ops.push(format!("get {name}#{idx}"));
                            }
                        }
                        Op::StructSet(reg, idx, val_reg) => {
                            let schema_name = match reg_types.get(reg) {
                                Some(Type::Struct(name)) => Some(name.clone()),
                                _ => None,
                            };
                            if let Some(name) = schema_name {
                                if let Some(dst) = inst.dst {
                                    reg_types.insert(dst, Type::Struct(name.clone()));
                                }
                                struct_ops.push(format!("set {name}#{idx}"));
                            }
                            if let Some(val_str) = reg_str_consts.get(val_reg) {
                                if *idx == 3 || val_str.starts_with("/api/") {
                                    ui_actions.push(val_str.clone());
                                }
                            }
                        }
                        Op::CmpEq(a, b) => {
                            if let Some(s) = reg_str_consts.get(a) {
                                if s.starts_with('/') {
                                    ui_actions.push(s.clone());
                                }
                            }
                            if let Some(s) = reg_str_consts.get(b) {
                                if s.starts_with('/') {
                                    ui_actions.push(s.clone());
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }

            functions.push(FunctionNode {
                name: f.name.clone(),
                file: raw.display_path.clone(),
                line,
                params: f.params.clone(),
                ret_ty: f.ret_ty.clone(),
                effects: f.effects.clone(),
                calls,
                called_by: Vec::new(),
                syscalls,
                struct_ops: deduplicate_preserving_order(struct_ops),
                ui_actions: deduplicate_preserving_order(ui_actions),
            });
        }
    }

    // 3. Compute called_by for each function
    let caller_edges: Vec<(String, String, usize, String)> = functions
        .iter()
        .flat_map(|caller| {
            caller.calls.iter().map(move |call| {
                (
                    caller.name.clone(),
                    caller.file.clone(),
                    call.line,
                    call.callee.clone(),
                )
            })
        })
        .collect();

    for func in &mut functions {
        for (caller_name, caller_file, call_line, callee_name) in &caller_edges {
            if &func.name == callee_name {
                func.called_by.push(CallerSite {
                    caller: caller_name.clone(),
                    file: caller_file.clone(),
                    line: *call_line,
                });
            }
        }
        func.called_by.sort_by(|a, b| {
            a.file
                .cmp(&b.file)
                .then_with(|| a.line.cmp(&b.line))
                .then_with(|| a.caller.cmp(&b.caller))
        });
        func.called_by.dedup();
    }

    // 4. Compute schema usages
    let mut schema_usages_map: HashMap<String, Vec<SchemaUsage>> = HashMap::new();

    for raw in raw_files {
        for f in &raw.functions {
            // Check param types
            for (_, pty) in &f.params {
                let mut refs = HashSet::new();
                extract_struct_references(pty, &mut refs);
                for schema_name in refs {
                    schema_usages_map.entry(schema_name).or_default().push(SchemaUsage {
                        function: f.name.clone(),
                        file: raw.display_path.clone(),
                        line: raw.fn_lines.get(&f.name).copied().unwrap_or(1),
                        op_kind: "param".to_string(),
                    });
                }
            }

            // Check ret_ty
            let mut ret_refs = HashSet::new();
            extract_struct_references(&f.ret_ty, &mut ret_refs);
            for schema_name in ret_refs {
                schema_usages_map.entry(schema_name).or_default().push(SchemaUsage {
                    function: f.name.clone(),
                    file: raw.display_path.clone(),
                    line: raw.fn_lines.get(&f.name).copied().unwrap_or(1),
                    op_kind: "ret".to_string(),
                });
            }

            // Check instructions
            let mut reg_types: HashMap<RegId, Type> = HashMap::new();
            for (reg, ty) in &f.params {
                reg_types.insert(*reg, ty.clone());
            }

            for block in &f.blocks {
                for inst in &block.instructions {
                    if let Some(dst) = inst.dst {
                        reg_types.insert(dst, inst.ty.clone());
                    }

                    match &inst.op {
                        Op::StructNew(name) => {
                            if let Some(dst) = inst.dst {
                                reg_types.insert(dst, Type::Struct(name.clone()));
                            }
                            schema_usages_map.entry(name.clone()).or_default().push(SchemaUsage {
                                function: f.name.clone(),
                                file: raw.display_path.clone(),
                                line: inst.line,
                                op_kind: "struct_new".to_string(),
                            });
                        }
                        Op::StructGet(reg, idx) => {
                            if let Some(Type::Struct(name)) = reg_types.get(reg) {
                                schema_usages_map.entry(name.clone()).or_default().push(SchemaUsage {
                                    function: f.name.clone(),
                                    file: raw.display_path.clone(),
                                    line: inst.line,
                                    op_kind: format!("struct_get#{idx}"),
                                });
                            }
                        }
                        Op::StructSet(reg, idx, _) => {
                            let schema_name = match reg_types.get(reg) {
                                Some(Type::Struct(name)) => Some(name.clone()),
                                _ => None,
                            };
                            if let Some(name) = schema_name {
                                if let Some(dst) = inst.dst {
                                    reg_types.insert(dst, Type::Struct(name.clone()));
                                }
                                schema_usages_map.entry(name).or_default().push(SchemaUsage {
                                    function: f.name.clone(),
                                    file: raw.display_path.clone(),
                                    line: inst.line,
                                    op_kind: format!("struct_set#{idx}"),
                                });
                            }
                        }
                        Op::VecNew(ty) => {
                            let mut refs = HashSet::new();
                            extract_struct_references(ty, &mut refs);
                            for schema_name in refs {
                                schema_usages_map.entry(schema_name).or_default().push(SchemaUsage {
                                    function: f.name.clone(),
                                    file: raw.display_path.clone(),
                                    line: inst.line,
                                    op_kind: "vec_new".to_string(),
                                });
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    for schema in &mut schemas {
        if let Some(mut usages) = schema_usages_map.remove(&schema.name) {
            usages.sort_by(|a, b| {
                a.file
                    .cmp(&b.file)
                    .then_with(|| a.line.cmp(&b.line))
                    .then_with(|| a.function.cmp(&b.function))
                    .then_with(|| a.op_kind.cmp(&b.op_kind))
            });
            usages.dedup();
            schema.usages = usages;
        }
    }

    // 5. Compute Orphans
    let orphans = detect_orphans(&schemas, &functions);

    // 6. Canonical sorting for determinism
    files.sort_by(|a, b| a.path.cmp(&b.path));
    schemas.sort_by(|a, b| a.name.cmp(&b.name));
    functions.sort_by(|a, b| a.file.cmp(&b.file).then_with(|| a.line.cmp(&b.line)).then_with(|| a.name.cmp(&b.name)));

    Ok(ProjectGraph {
        files,
        schemas,
        functions,
        orphans,
    })
}

fn detect_orphans(schemas: &[SchemaNode], functions: &[FunctionNode]) -> OrphanReport {
    let mut schema_field_refs = HashSet::new();
    for schema in schemas {
        for field in &schema.fields {
            extract_struct_references(&field.ty, &mut schema_field_refs);
        }
    }

    let mut unused_schemas: Vec<String> = schemas
        .iter()
        .filter(|s| s.usages.is_empty() && !schema_field_refs.contains(&s.name))
        .map(|s| s.name.clone())
        .collect();
    unused_schemas.sort();
    unused_schemas.dedup();

    let mut uncalled_functions: Vec<String> = functions
        .iter()
        .filter(|f| f.name != "main" && f.name != "knr_dispatch" && f.called_by.is_empty())
        .map(|f| f.name.clone())
        .collect();
    uncalled_functions.sort();
    uncalled_functions.dedup();

    OrphanReport {
        unused_schemas,
        uncalled_functions,
    }
}

fn extract_struct_references(ty: &Type, out: &mut HashSet<String>) {
    match ty {
        Type::Struct(name) => {
            out.insert(name.clone());
        }
        Type::Vec(inner) => extract_struct_references(inner, out),
        Type::Map(k, v) => {
            extract_struct_references(k, out);
            extract_struct_references(v, out);
        }
        Type::Chan(inner) => extract_struct_references(inner, out),
        _ => {}
    }
}

fn deduplicate_preserving_order(items: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for item in items {
        if seen.insert(item.clone()) {
            out.push(item);
        }
    }
    out
}

fn prescan_decl_lines(content: &str) -> (HashMap<String, usize>, HashMap<String, usize>) {
    let mut schema_lines = HashMap::new();
    let mut fn_lines = HashMap::new();
    for (idx, line) in content.lines().enumerate() {
        let stripped = strip_comment(line).trim();
        let line_num = idx + 1;
        if stripped.starts_with("schema ") || stripped.starts_with("schema\t") {
            if let Some(name) = parse_schema_name_peek(stripped) {
                schema_lines.insert(name, line_num);
            }
        } else if stripped.starts_with("fn ") || stripped.starts_with("fn\t") {
            if let Some((name, _)) = prescan_fn_header(stripped, line_num) {
                fn_lines.insert(name, line_num);
            }
        }
    }
    (schema_lines, fn_lines)
}

fn clean_display_path(canonical: &Path, root_hint: &Path) -> String {
    if let Ok(cwd) = std::env::current_dir() {
        if let Ok(canonical_cwd) = fs::canonicalize(&cwd) {
            if let Ok(rel) = canonical.strip_prefix(&canonical_cwd) {
                let s = rel.to_string_lossy().replace('\\', "/");
                return if s.is_empty() { ".".to_string() } else { s };
            }
        }
    }

    let base = if root_hint.is_dir() {
        root_hint
    } else {
        root_hint.parent().unwrap_or(root_hint)
    };

    if let Ok(canonical_base) = fs::canonicalize(base) {
        if let Some(parent) = canonical_base.parent() {
            if let Ok(rel) = canonical.strip_prefix(parent) {
                let s = rel.to_string_lossy().replace('\\', "/");
                return if s.is_empty() { ".".to_string() } else { s };
            }
        }
        if let Ok(rel) = canonical.strip_prefix(&canonical_base) {
            let s = rel.to_string_lossy().replace('\\', "/");
            return if s.is_empty() { ".".to_string() } else { s };
        }
    }

    if let Ok(rel) = canonical.strip_prefix(root_hint) {
        let s = rel.to_string_lossy().replace('\\', "/");
        return if s.is_empty() { ".".to_string() } else { s };
    }

    canonical.to_string_lossy().replace('\\', "/")
}

// ============================================================================
// Methods on ProjectGraph: Query & Formatters
// ============================================================================

impl ProjectGraph {
    /// Formats the project graph or filtered subset in the specified output format.
    pub fn format(&self, format: ExploreOutputFormat, filter: &ExploreFilter) -> String {
        match format {
            ExploreOutputFormat::Ai => self.format_ai(filter),
            ExploreOutputFormat::Json => self.format_json(filter),
            ExploreOutputFormat::Mermaid => self.format_mermaid(filter),
        }
    }

    /// Resolves all direct and transitive callers reaching `target_fn` with cycle protection.
    pub fn get_transitive_callers(&self, target_fn: &str) -> Vec<TransitiveCaller> {
        let mut results = Vec::new();
        let mut visited = HashSet::new();
        let mut queue = VecDeque::new();

        // 1. Seed queue with direct callers from all matching FunctionNodes
        for func in &self.functions {
            if func.name == target_fn {
                for caller_site in &func.called_by {
                    queue.push_back((caller_site.clone(), 1));
                }
            }
        }

        // 2. BFS Traversal
        while let Some((caller_site, depth)) = queue.pop_front() {
            let visit_key = format!("{}@{}:{}", caller_site.caller, caller_site.file, caller_site.line);
            if visited.contains(&visit_key) {
                continue;
            }
            visited.insert(visit_key);

            results.push(TransitiveCaller {
                caller: caller_site.caller.clone(),
                file: caller_site.file.clone(),
                line: caller_site.line,
                depth,
            });

            for f in &self.functions {
                if f.name == caller_site.caller && f.file == caller_site.file {
                    for upstream in &f.called_by {
                        let up_key = format!("{}@{}:{}", upstream.caller, upstream.file, upstream.line);
                        if !visited.contains(&up_key) {
                            queue.push_back((upstream.clone(), depth + 1));
                        }
                    }
                }
            }
        }

        results.sort_by(|a, b| {
            a.depth
                .cmp(&b.depth)
                .then_with(|| a.caller.cmp(&b.caller))
                .then_with(|| a.file.cmp(&b.file))
                .then_with(|| a.line.cmp(&b.line))
        });

        results
    }

    /// Resolves all direct and transitive callees and syscalls of `target_fn` with cycle protection.
    pub fn get_transitive_callees(&self, target_fn: &str) -> CalleeTraversalReport {
        let mut callees = Vec::new();
        let mut syscalls = Vec::new();
        let mut visited_calls = HashSet::new();
        let mut visited_fns = HashSet::new();
        let mut queue = VecDeque::new();

        let root_funcs: Vec<&FunctionNode> = self
            .functions
            .iter()
            .filter(|f| f.name == target_fn)
            .collect();

        for func in &root_funcs {
            visited_fns.insert(func.name.clone());

            for s in &func.syscalls {
                syscalls.push(SyscallOccurrence {
                    in_fn: func.name.clone(),
                    effect: s.effect.to_string(),
                    op: s.op.clone(),
                    file: func.file.clone(),
                    line: s.line,
                    depth: 0,
                });
            }

            for call in &func.calls {
                queue.push_back((func.name.clone(), call.callee.clone(), call.line, 1));
            }
        }

        while let Some((caller_name, callee_name, call_line, depth)) = queue.pop_front() {
            let edge_key = (caller_name.clone(), callee_name.clone(), call_line);
            if visited_calls.contains(&edge_key) {
                continue;
            }
            visited_calls.insert(edge_key);

            let callee_defs: Vec<&FunctionNode> = self
                .functions
                .iter()
                .filter(|f| f.name == callee_name)
                .collect();

            let (def_file, def_line) = if let Some(first) = callee_defs.first() {
                (first.file.clone(), first.line)
            } else {
                ("<external>".to_string(), 0)
            };

            callees.push(CalleeEdge {
                caller: caller_name,
                callee: callee_name.clone(),
                call_line,
                def_file,
                def_line,
                depth,
            });

            for def in &callee_defs {
                for s in &def.syscalls {
                    syscalls.push(SyscallOccurrence {
                        in_fn: def.name.clone(),
                        effect: s.effect.to_string(),
                        op: s.op.clone(),
                        file: def.file.clone(),
                        line: s.line,
                        depth,
                    });
                }
            }

            if !visited_fns.contains(&callee_name) {
                visited_fns.insert(callee_name.clone());
                for def in &callee_defs {
                    for next_call in &def.calls {
                        queue.push_back((
                            def.name.clone(),
                            next_call.callee.clone(),
                            next_call.line,
                            depth + 1,
                        ));
                    }
                }
            }
        }

        callees.sort_by(|a, b| {
            a.depth
                .cmp(&b.depth)
                .then_with(|| a.caller.cmp(&b.caller))
                .then_with(|| a.callee.cmp(&b.callee))
                .then_with(|| a.call_line.cmp(&b.call_line))
        });

        syscalls.sort_by(|a, b| {
            a.depth
                .cmp(&b.depth)
                .then_with(|| a.in_fn.cmp(&b.in_fn))
                .then_with(|| a.line.cmp(&b.line))
                .then_with(|| a.op.cmp(&b.op))
        });

        CalleeTraversalReport {
            target: target_fn.to_string(),
            callees,
            syscalls,
        }
    }

    /// Formats the project graph in AI token-dense format.
    pub fn format_ai(&self, filter: &ExploreFilter) -> String {
        let mut out = String::new();

        // 1. If orphans only
        if filter.orphans_only {
            self.format_ai_orphans(&mut out);
            return out;
        }

        // 2. If callers query
        if let Some(target) = &filter.callers {
            let callers = self.get_transitive_callers(target);
            out.push_str("[CALL GRAPH & SIGNATURES]\n");
            out.push_str(&format!("  CALLERS OF {target}:\n"));
            if callers.is_empty() {
                out.push_str("    (no callers found)\n");
            } else {
                for c in &callers {
                    out.push_str(&format!(
                        "    depth {}: {}:{} in {}\n",
                        c.depth, c.file, c.line, c.caller
                    ));
                }
            }
            return out;
        }

        // 3. If callees query
        if let Some(target) = &filter.callees {
            let report = self.get_transitive_callees(target);
            out.push_str("[CALL GRAPH & SIGNATURES]\n");
            out.push_str(&format!("  CALLEES OF {target}:\n"));
            if report.callees.is_empty() {
                out.push_str("    (no callees)\n");
            } else {
                for c in &report.callees {
                    out.push_str(&format!(
                        "    depth {}: {} (line {} in {}, defined in {}:{})\n",
                        c.depth, c.callee, c.call_line, c.caller, c.def_file, c.def_line
                    ));
                }
            }
            out.push_str(&format!("  SYSCALLS OF {target}:\n"));
            if report.syscalls.is_empty() {
                out.push_str("    (no syscalls)\n");
            } else {
                for s in &report.syscalls {
                    out.push_str(&format!(
                        "    depth {}: {}:{} (line {} in {}, {})\n",
                        s.depth, s.effect, s.op, s.line, s.in_fn, s.file
                    ));
                }
            }
            return out;
        }

        // 4. If symbol query
        if let Some(sym) = &filter.symbol {
            let matching_schemas: Vec<&SchemaNode> =
                self.schemas.iter().filter(|s| &s.name == sym).collect();
            let matching_functions: Vec<&FunctionNode> =
                self.functions.iter().filter(|f| &f.name == sym).collect();

            if matching_schemas.is_empty() && matching_functions.is_empty() {
                return format!("Symbol '{sym}' not found in project.\n");
            }

            if !matching_schemas.is_empty() {
                out.push_str("[SCHEMAS]\n");
                for s in matching_schemas {
                    self.format_ai_single_schema(s, &mut out);
                }
            }

            if !matching_functions.is_empty() {
                out.push_str("[CALL GRAPH & SIGNATURES]\n");
                for f in matching_functions {
                    self.format_ai_single_function(f, &mut out);
                }
            }
            return out;
        }

        // 5. If effects query
        if let Some(eff) = &filter.effects {
            let matching_funcs: Vec<&FunctionNode> = self
                .functions
                .iter()
                .filter(|f| matches_effect_filter(f, eff))
                .collect();

            if matching_funcs.is_empty() {
                return format!("No functions found with effect '{eff}'.\n");
            }

            out.push_str("[CALL GRAPH & SIGNATURES]\n");
            for f in matching_funcs {
                self.format_ai_single_function(f, &mut out);
            }
            return out;
        }

        // 6. Default view: all 4 sections
        // [TOPOLOGY & IMPORTS]
        out.push_str("[TOPOLOGY & IMPORTS]\n");
        for file in &self.files {
            out.push_str(&format!("  FILE {} ({:?})\n", file.path, file.role));
            if !file.imports.is_empty() {
                out.push_str(&format!("    imports: {}\n", file.imports.join(", ")));
            }
            if !file.schemas.is_empty() {
                out.push_str(&format!("    schemas: {}\n", file.schemas.join(", ")));
            }
            if !file.functions.is_empty() {
                out.push_str(&format!("    functions: {}\n", file.functions.join(", ")));
            }
        }

        // [SCHEMAS]
        if !self.schemas.is_empty() {
            out.push_str("\n[SCHEMAS]\n");
            for schema in &self.schemas {
                self.format_ai_single_schema(schema, &mut out);
            }
        }

        // [CALL GRAPH & SIGNATURES]
        if !self.functions.is_empty() {
            out.push_str("\n[CALL GRAPH & SIGNATURES]\n");
            for func in &self.functions {
                self.format_ai_single_function(func, &mut out);
            }
        }

        // [ORPHANS]
        out.push('\n');
        self.format_ai_orphans(&mut out);

        out
    }

    fn format_ai_single_schema(&self, schema: &SchemaNode, out: &mut String) {
        out.push_str(&format!("  SCHEMA {} ({}:{})\n", schema.name, schema.file, schema.line));
        if schema.fields.is_empty() {
            out.push_str("    fields: (none)\n");
        } else {
            let field_strs: Vec<String> = schema
                .fields
                .iter()
                .map(|f| {
                    let name = f.name.as_deref().unwrap_or("_");
                    format!("{}: {} {}", f.index, name, f.ty)
                })
                .collect();
            out.push_str(&format!("    fields: {}\n", field_strs.join(", ")));
        }

        if schema.usages.is_empty() {
            out.push_str("    usages: (none)\n");
        } else {
            out.push_str(&format!("    usages ({}):\n", schema.usages.len()));
            for u in &schema.usages {
                out.push_str(&format!("      {}:{} in {} ({})\n", u.file, u.line, u.function, u.op_kind));
            }
        }
    }

    fn format_ai_single_function(&self, func: &FunctionNode, out: &mut String) {
        let param_strs: Vec<String> = func
            .params
            .iter()
            .map(|(reg, ty)| format!("%{reg}: {ty}"))
            .collect();
        let eff_str = if func.effects.is_empty() {
            String::new()
        } else {
            let eff_list: Vec<String> = func.effects.iter().map(|e| e.to_string()).collect();
            format!(" effects({})", eff_list.join(", "))
        };

        out.push_str(&format!(
            "  FN {}({}) -> {}{} ({}:{})\n",
            func.name,
            param_strs.join(", "),
            func.ret_ty,
            eff_str,
            func.file,
            func.line
        ));

        if !func.called_by.is_empty() {
            out.push_str(&format!("    called_by ({}):\n", func.called_by.len()));
            for c in &func.called_by {
                out.push_str(&format!("      {}:{} in {}\n", c.file, c.line, c.caller));
            }
        }

        if !func.calls.is_empty() {
            out.push_str(&format!("    calls ({}):\n", func.calls.len()));
            for c in &func.calls {
                out.push_str(&format!("      {} (line {})\n", c.callee, c.line));
            }
        }

        if !func.syscalls.is_empty() {
            out.push_str(&format!("    syscalls ({}):\n", func.syscalls.len()));
            for s in &func.syscalls {
                out.push_str(&format!("      {}:{} (line {})\n", s.effect, s.op, s.line));
            }
        }

        if !func.struct_ops.is_empty() {
            out.push_str(&format!("    struct_ops: {}\n", func.struct_ops.join(", ")));
        }

        if !func.ui_actions.is_empty() {
            let acts: Vec<String> = func.ui_actions.iter().map(|a| format!("\"{a}\"")).collect();
            out.push_str(&format!("    ui_actions: {}\n", acts.join(", ")));
        }
    }

    fn format_ai_orphans(&self, out: &mut String) {
        out.push_str("[ORPHANS]\n");
        out.push_str(&format!("  unused_schemas ({}):\n", self.orphans.unused_schemas.len()));
        if self.orphans.unused_schemas.is_empty() {
            out.push_str("    (none)\n");
        } else {
            for s in &self.orphans.unused_schemas {
                out.push_str(&format!("    - {s}\n"));
            }
        }

        out.push_str(&format!("  uncalled_functions ({}):\n", self.orphans.uncalled_functions.len()));
        if self.orphans.uncalled_functions.is_empty() {
            out.push_str("    (none)\n");
        } else {
            for f in &self.orphans.uncalled_functions {
                out.push_str(&format!("    - {f}\n"));
            }
        }
    }

    /// Formats the project graph in deterministic pretty JSON.
    pub fn format_json(&self, filter: &ExploreFilter) -> String {
        if filter.orphans_only {
            let val = serde_json::json!({
                "orphans": &self.orphans
            });
            return serde_json::to_string_pretty(&val).unwrap_or_default();
        }

        if let Some(target) = &filter.callers {
            let callers = self.get_transitive_callers(target);
            let val = serde_json::json!({
                "target": target,
                "callers": callers
            });
            return serde_json::to_string_pretty(&val).unwrap_or_default();
        }

        if let Some(target) = &filter.callees {
            let report = self.get_transitive_callees(target);
            return serde_json::to_string_pretty(&report).unwrap_or_default();
        }

        if let Some(sym) = &filter.symbol {
            let matching_schemas: Vec<&SchemaNode> =
                self.schemas.iter().filter(|s| &s.name == sym).collect();
            let matching_functions: Vec<&FunctionNode> =
                self.functions.iter().filter(|f| &f.name == sym).collect();
            let val = serde_json::json!({
                "symbol": sym,
                "schemas": matching_schemas,
                "functions": matching_functions
            });
            return serde_json::to_string_pretty(&val).unwrap_or_default();
        }

        if let Some(eff) = &filter.effects {
            let matching_funcs: Vec<&FunctionNode> = self
                .functions
                .iter()
                .filter(|f| matches_effect_filter(f, eff))
                .collect();
            let val = serde_json::json!({
                "effect": eff,
                "functions": matching_funcs
            });
            return serde_json::to_string_pretty(&val).unwrap_or_default();
        }

        serde_json::to_string_pretty(self).unwrap_or_default()
    }

    /// Formats the project graph in valid Mermaid flowchart TD syntax.
    pub fn format_mermaid(&self, filter: &ExploreFilter) -> String {
        let mut out = String::from("flowchart TD\n");

        if filter.orphans_only {
            out.push_str("  subgraph Orphans[\"Project Orphans\"]\n");
            for s in &self.orphans.unused_schemas {
                let id = sanitize_mermaid_id(s);
                out.push_str(&format!("    orphan_schema_{id}[\"schema {s} (0 usages)\"]\n"));
            }
            for f in &self.orphans.uncalled_functions {
                let id = sanitize_mermaid_id(f);
                out.push_str(&format!("    orphan_fn_{id}[\"fn {f} (0 callers)\"]\n"));
            }
            out.push_str("  end\n");
            return out;
        }

        // Subgraphs for files
        for file in &self.files {
            let file_id = sanitize_mermaid_id(&file.path);
            out.push_str(&format!("  subgraph file_{file_id}[\"{}\"]\n", file.path));

            for s_name in &file.schemas {
                let s_id = sanitize_mermaid_id(s_name);
                out.push_str(&format!("    schema_{file_id}_{s_id}[\"schema {s_name}\"]\n"));
            }

            for f_name in &file.functions {
                let f_id = sanitize_mermaid_id(f_name);
                let ret_ty = self
                    .functions
                    .iter()
                    .find(|f| f.name == *f_name && f.file == file.path)
                    .map(|f| format!("{}", f.ret_ty))
                    .unwrap_or_else(|| "void".to_string());
                out.push_str(&format!("    fn_{file_id}_{f_id}[\"{f_name}() -> {ret_ty}\"]\n"));
            }

            out.push_str("  end\n\n");
        }

        // Function call edges
        let mut rendered_edges = HashSet::new();

        if let Some(target) = &filter.callers {
            let callers = self.get_transitive_callers(target);
            for c in &callers {
                let caller_file_id = sanitize_mermaid_id(&c.file);
                let caller_fn_id = sanitize_mermaid_id(&c.caller);
                // Edge from caller to target
                out.push_str(&format!(
                    "  fn_{caller_file_id}_{caller_fn_id} --> target_{}[\"{target}\"]\n",
                    sanitize_mermaid_id(target)
                ));
            }
            return out;
        }

        if let Some(target) = &filter.callees {
            let report = self.get_transitive_callees(target);
            for edge in &report.callees {
                let caller_fn_id = sanitize_mermaid_id(&edge.caller);
                let callee_file_id = sanitize_mermaid_id(&edge.def_file);
                let callee_fn_id = sanitize_mermaid_id(&edge.callee);
                out.push_str(&format!(
                    "  call_{caller_fn_id} --> fn_{callee_file_id}_{callee_fn_id}\n"
                ));
            }
            return out;
        }

        for func in &self.functions {
            let caller_file_id = sanitize_mermaid_id(&func.file);
            let caller_fn_id = sanitize_mermaid_id(&func.name);

            for call in &func.calls {
                // Find callee function node
                let callee_node = self.functions.iter().find(|f| f.name == call.callee);
                if let Some(callee) = callee_node {
                    let callee_file_id = sanitize_mermaid_id(&callee.file);
                    let callee_fn_id = sanitize_mermaid_id(&callee.name);
                    let edge = format!("  fn_{caller_file_id}_{caller_fn_id} --> fn_{callee_file_id}_{callee_fn_id}\n");
                    if rendered_edges.insert(edge.clone()) {
                        out.push_str(&edge);
                    }
                } else {
                    let callee_id = sanitize_mermaid_id(&call.callee);
                    let edge = format!("  fn_{caller_file_id}_{caller_fn_id} --> fn_ext_{callee_id}[\"{}\"]\n", call.callee);
                    if rendered_edges.insert(edge.clone()) {
                        out.push_str(&edge);
                    }
                }
            }
        }

        out
    }
}

pub fn matches_effect_filter(func: &FunctionNode, filter_eff: &str) -> bool {
    let declared_match = func
        .effects
        .iter()
        .any(|e| e.to_string().eq_ignore_ascii_case(filter_eff));

    let invoked_match = func
        .syscalls
        .iter()
        .any(|s| s.effect.to_string().eq_ignore_ascii_case(filter_eff));

    declared_match || invoked_match
}

pub fn sanitize_mermaid_id(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
}
