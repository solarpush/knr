use std::path::Path;

use knr_core::lower::{
    lower_program_from_file_with_source_map, lower_program_with_schemas_and_source_map, SourceMap,
};
use knr_core::syntax::Program;
use knr_core::{Module, Schema};

use crate::diagnostic::Diagnostic;
use crate::{effects, verify_function_with_env, verify_module_schemas};

/// Verifies a `.knr` file on disk (resolving all relative `.knr`, `.ssa`, and `.ssa.schema` imports)
/// and returns the verified lowered `Module` on success.
pub fn verify_program_from_file(path: &Path) -> Result<Module, Vec<Diagnostic>> {
    let (module, source_map) = match lower_program_from_file_with_source_map(path) {
        Ok(res) => res,
        Err(err) => return Err(vec![Diagnostic::from_lower_error(err)]),
    };
    verify_lowered_module(module, &source_map)
}

/// Verifies a high-level `.knr` program AST:
/// 1. Lowers high-level declarations, expressions, and statements into an SSA Module.
/// 2. If lowering fails, translates the LowerError into a structured Diagnostic pointing to the source span.
/// 3. Runs all four static verification passes (SSA dominance, linear ownership, static types, effects)
///    plus module schema verification on the lowered Module.
/// 4. Translates any IR-level diagnostic line numbers, register violations, and variable names back
///    to the original `.knr` source locations using the lowering SourceMap.
pub fn verify_program(prog: &Program) -> Result<(), Vec<Diagnostic>> {
    verify_program_with_schemas(prog, &[])
}

/// Verifies a high-level `.knr` program AST with externally supplied schemas.
pub fn verify_program_with_schemas(
    prog: &Program,
    extra_schemas: &[Schema],
) -> Result<(), Vec<Diagnostic>> {
    // 1. Lower program with source map tracking
    let (module, source_map) = match lower_program_with_schemas_and_source_map(prog, extra_schemas, None) {
        Ok(res) => res,
        Err(err) => {
            return Err(vec![Diagnostic::from_lower_error(err)]);
        }
    };
    verify_lowered_module(module, &source_map).map(|_| ())
}

fn verify_lowered_module(module: Module, source_map: &SourceMap) -> Result<Module, Vec<Diagnostic>> {
    // 2. Run standard module verification passes
    let mut diagnostics = Vec::new();

    // 2a. Schema validation (name uniqueness, cycle detection, field indices)
    if let Err(schema_diags) = verify_module_schemas(&module) {
        diagnostics.extend(schema_diags);
    }

    // 2b. Per-function static passes (SSA, linear, types, effects)
    for func in &module.functions {
        if let Err(diag) = verify_function_with_env(func, &module.schemas, &module.functions) {
            diagnostics.push(diag);
        }
    }

    // 2c. Module-level effect isolation
    if let Err(diag) = effects::verify_module_effects(&module) {
        diagnostics.push(diag);
    }

    // 3. Enrich diagnostics with high-level source map locations & variable names
    if diagnostics.is_empty() {
        Ok(module)
    } else {
        for diag in &mut diagnostics {
            enrich_diagnostic_with_source_map(diag, source_map);
        }
        Err(diagnostics)
    }
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
