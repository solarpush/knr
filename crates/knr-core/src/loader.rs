use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use crate::ast::{Function, Module, Schema};
use crate::error::ParseError;
use crate::parser::{
    parse_import_directive, parse_ssa_with_role_and_env, strip_comment, FileRole,
};

/// Loads an SSA compilation unit from disk, recursively resolving imports,
/// enforcing file topology rules, deduplicating diamond imports, and detecting cycles.
pub fn load_module_from_path(path: &Path) -> Result<Module, ParseError> {
    let mut loader = LoaderState::new();
    loader.load_file(path, 1)?;
    Ok(loader.into_module())
}

struct LoaderState {
    visited: HashSet<PathBuf>,
    active_stack: Vec<PathBuf>,
    merged_schemas: Vec<Schema>,
    merged_functions: Vec<Function>,
    schema_origins: HashMap<String, PathBuf>,
    function_origins: HashMap<String, PathBuf>,
    root_imports: Vec<String>,
}

impl LoaderState {
    fn new() -> Self {
        Self {
            visited: HashSet::new(),
            active_stack: Vec::new(),
            merged_schemas: Vec::new(),
            merged_functions: Vec::new(),
            schema_origins: HashMap::new(),
            function_origins: HashMap::new(),
            root_imports: Vec::new(),
        }
    }

    fn load_file(&mut self, path: &Path, import_line: usize) -> Result<(), ParseError> {
        let canonical = match fs::canonicalize(path) {
            Ok(c) => c,
            Err(e) => {
                if e.kind() == std::io::ErrorKind::NotFound {
                    return Err(ParseError::FileNotFound {
                        line: import_line,
                        path: path.display().to_string(),
                    });
                } else {
                    return Err(ParseError::IoError {
                        path: path.display().to_string(),
                        msg: e.to_string(),
                    });
                }
            }
        };

        // 1. Cycle detection
        if let Some(pos) = self.active_stack.iter().position(|p| p == &canonical) {
            let cycle = self.active_stack[pos..]
                .iter()
                .chain(std::iter::once(&canonical))
                .map(|p| p.file_name().unwrap_or_default().to_string_lossy().to_string())
                .collect::<Vec<_>>();
            return Err(ParseError::CircularImport { cycle });
        }

        // 2. Diamond import deduplication (idempotent skip)
        if self.visited.contains(&canonical) {
            return Ok(());
        }

        // 3. Determine FileRole from filename extension
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

        let content = match fs::read_to_string(&canonical) {
            Ok(s) => s,
            Err(e) => {
                return Err(ParseError::IoError {
                    path: canonical.display().to_string(),
                    msg: e.to_string(),
                });
            }
        };

        self.active_stack.push(canonical.clone());

        // 4. Pre-scan imports to load dependencies first
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

        if self.active_stack.len() == 1 {
            self.root_imports = file_imports.iter().map(|(p, _)| p.clone()).collect();
        }

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
            self.load_file(&target_path, *line)?;
        }

        // 5. Parse current file with accumulated schemas and functions in context
        let parsed_module =
            parse_ssa_with_role_and_env(&content, role, &self.merged_schemas, &self.merged_functions)?;

        // 6. Collision detection & symbol merging
        for schema in parsed_module.schemas {
            if let Some(first_path) = self.schema_origins.get(&schema.name) {
                return Err(ParseError::DuplicateSymbol {
                    symbol: schema.name.clone(),
                    first_file: first_path.display().to_string(),
                    second_file: canonical.display().to_string(),
                });
            }
            self.schema_origins
                .insert(schema.name.clone(), canonical.clone());
            self.merged_schemas.push(schema);
        }

        for func in parsed_module.functions {
            if let Some(first_path) = self.function_origins.get(&func.name) {
                return Err(ParseError::DuplicateSymbol {
                    symbol: func.name.clone(),
                    first_file: first_path.display().to_string(),
                    second_file: canonical.display().to_string(),
                });
            }
            self.function_origins
                .insert(func.name.clone(), canonical.clone());
            self.merged_functions.push(func);
        }

        self.active_stack.pop();
        self.visited.insert(canonical);
        Ok(())
    }

    fn into_module(self) -> Module {
        let mut module = Module {
            imports: self.root_imports,
            schemas: self.merged_schemas,
            functions: self.merged_functions,
        };
        module.resolve_call_types();
        module
    }
}
