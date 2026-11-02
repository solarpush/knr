#![allow(clippy::result_large_err)]

pub mod diagnostic;
pub mod effects;
pub mod linear;
pub mod ssa;
pub mod types;

pub use diagnostic::Diagnostic;
pub use effects::{verify_effects, verify_module_effects};
pub use linear::verify_linear;
pub use ssa::verify_ssa;
pub use types::{verify_types, verify_types_with_env, verify_types_with_schemas};

use knr_core::{Function, Module, Schema};

/// Verifies a single function against all static analysis passes with module schemas and known functions:
/// 1. Linear ownership state machine (`linear.rs`)
/// 2. SSA structural validation and dominance (`ssa.rs`)
/// 3. Static type checker for all opcodes (`types.rs`)
/// 4. Effect capability isolation (`effects.rs`)
pub fn verify_function_with_env(
    func: &Function,
    schemas: &[Schema],
    functions: &[Function],
) -> Result<(), Diagnostic> {
    effects::verify_effects(func)?;
    linear::verify_linear(func)?;
    ssa::verify_ssa(func)?;
    types::verify_types_with_env(func, schemas, functions)?;
    Ok(())
}

/// Verifies a single function against all static analysis passes with module schemas:
/// 1. Linear ownership state machine (`linear.rs`)
/// 2. SSA structural validation and dominance (`ssa.rs`)
/// 3. Static type checker for all opcodes (`types.rs`)
/// 4. Effect capability isolation (`effects.rs`)
pub fn verify_function_with_schemas(func: &Function, schemas: &[Schema]) -> Result<(), Diagnostic> {
    verify_function_with_env(func, schemas, &[])
}

/// Verifies a single function against all static analysis passes:
/// 1. Linear ownership state machine (`linear.rs`)
/// 2. SSA structural validation and dominance (`ssa.rs`)
/// 3. Static type checker for all opcodes (`types.rs`)
/// 4. Effect capability isolation (`effects.rs`)
pub fn verify_function(func: &Function) -> Result<(), Diagnostic> {
    verify_function_with_schemas(func, &[])
}

/// Verifies all functions and schemas in a module, collecting any emitted diagnostics.
pub fn verify_module(module: &Module) -> Result<(), Vec<Diagnostic>> {
    let mut diagnostics = Vec::new();
    if let Err(schema_diags) = verify_module_schemas(module) {
        diagnostics.extend(schema_diags);
    }
    for func in &module.functions {
        if let Err(diag) = verify_function_with_env(func, &module.schemas, &module.functions) {
            diagnostics.push(diag);
        }
    }
    if let Err(diag) = effects::verify_module_effects(module) {
        diagnostics.push(diag);
    }
    if diagnostics.is_empty() {
        Ok(())
    } else {
        Err(diagnostics)
    }
}

fn is_valid_ident(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {
            chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
        }
        _ => false,
    }
}

/// Validates module-level schema declarations:
/// 1. Schema name uniqueness
/// 2. Field index and field name uniqueness within each schema
/// 3. Field types reference declared schemas
/// 4. Direct recursive schema cycle detection (preventing infinite-size value types)
pub fn verify_module_schemas(module: &Module) -> Result<(), Vec<Diagnostic>> {
    let mut diagnostics = Vec::new();
    let mut seen_schemas = std::collections::HashSet::new();

    for schema in &module.schemas {
        if schema.name.is_empty() || !is_valid_ident(&schema.name) {
            diagnostics.push(
                Diagnostic::new(
                    "InvalidSchemaName",
                    &schema.name,
                    1,
                    format!("schema {}", schema.name),
                )
                .with_fix_hint(format!(
                    "Schema name '{}' is invalid: must be a valid non-empty identifier",
                    schema.name
                )),
            );
        }

        if !seen_schemas.insert(&schema.name) {
            diagnostics.push(
                Diagnostic::new(
                    "DuplicateSchema",
                    "<module>",
                    1,
                    format!("schema {}", schema.name),
                )
                .with_fix_hint(format!("Schema '{}' is defined multiple times", schema.name)),
            );
        }

        let mut seen_indices = std::collections::HashSet::new();
        let mut seen_names = std::collections::HashSet::new();
        for field in &schema.fields {
            if field.index > u16::MAX as u32 {
                diagnostics.push(
                    Diagnostic::new(
                        "FieldIndexOverflow",
                        &schema.name,
                        1,
                        format!("field index {}", field.index),
                    )
                    .with_fix_hint(format!(
                        "Field index {} in schema '{}' exceeds maximum allowed index (65535)",
                        field.index, schema.name
                    )),
                );
            }

            if !seen_indices.insert(field.index) {
                diagnostics.push(
                    Diagnostic::new(
                        "DuplicateField",
                        &schema.name,
                        1,
                        format!("field index {}", field.index),
                    )
                    .with_fix_hint(format!(
                        "Field index {} is defined multiple times in schema '{}'",
                        field.index, schema.name
                    )),
                );
            }
            if let Some(ref name) = field.name {
                if name.is_empty() || !is_valid_ident(name) {
                    diagnostics.push(
                        Diagnostic::new(
                            "InvalidFieldName",
                            &schema.name,
                            1,
                            format!("field {}", name),
                        )
                        .with_fix_hint(format!(
                            "Field name '{}' in schema '{}' is invalid: must be a valid non-empty identifier",
                            name, schema.name
                        )),
                    );
                }
                if !seen_names.insert(name) {
                    diagnostics.push(
                        Diagnostic::new(
                            "DuplicateField",
                            &schema.name,
                            1,
                            format!("field {}", name),
                        )
                        .with_fix_hint(format!(
                            "Field name '{}' is defined multiple times in schema '{}'",
                            name, schema.name
                        )),
                    );
                }
            }

            check_schema_field_type(&field.ty, &schema.name, field.index, &module.schemas, &mut diagnostics);
        }
    }

    for schema in &module.schemas {
        let mut visited = std::collections::HashSet::new();
        if has_schema_cycle(&schema.name, &module.schemas, &mut visited) {
            diagnostics.push(
                Diagnostic::new(
                    "RecursiveSchema",
                    &schema.name,
                    1,
                    format!("schema {}", schema.name),
                )
                .with_fix_hint(format!(
                    "Cyclic direct nesting detected involving schema '{}': structs cannot contain themselves by value without indirection",
                    schema.name
                )),
            );
        }
    }

    if diagnostics.is_empty() {
        Ok(())
    } else {
        Err(diagnostics)
    }
}

fn check_schema_field_type(
    ty: &knr_core::Type,
    schema_name: &str,
    field_idx: u32,
    schemas: &[Schema],
    diagnostics: &mut Vec<Diagnostic>,
) {
    match ty {
        knr_core::Type::Struct(name) => {
            if !schemas.iter().any(|s| s.name == *name) {
                diagnostics.push(
                    Diagnostic::new(
                        "UnknownSchema",
                        schema_name,
                        1,
                        format!("field {field_idx}: struct[{name}]"),
                    )
                    .with_fix_hint(format!(
                        "Field {field_idx} in schema '{schema_name}' references undeclared schema '{name}'"
                    )),
                );
            }
        }
        knr_core::Type::Vec(inner) | knr_core::Type::Chan(inner) => {
            check_schema_field_type(inner, schema_name, field_idx, schemas, diagnostics);
        }
        knr_core::Type::Map(k, v) => {
            check_schema_field_type(k, schema_name, field_idx, schemas, diagnostics);
            check_schema_field_type(v, schema_name, field_idx, schemas, diagnostics);
        }
        _ => {}
    }
}

fn has_schema_cycle<'a>(
    current: &'a str,
    schemas: &'a [Schema],
    visited: &mut std::collections::HashSet<&'a str>,
) -> bool {
    if !visited.insert(current) {
        return true;
    }
    if let Some(schema) = schemas.iter().find(|s| s.name == current) {
        for field in &schema.fields {
            if let knr_core::Type::Struct(nested_name) = &field.ty {
                if has_schema_cycle(nested_name, schemas, visited) {
                    return true;
                }
            }
        }
    }
    visited.remove(current);
    false
}
