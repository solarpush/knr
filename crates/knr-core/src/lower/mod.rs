pub mod context;
pub mod error;
pub mod expr;
pub mod normalize;
pub mod stmt;

pub use context::{EnumMeta, EnumVariantMeta, FnLoweringContext, FnSig, SourceMap};
pub use error::LowerError;
pub use normalize::normalize_function_registers;

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use crate::ast::{BasicBlock, Effect, Function, Module, Schema, Terminator, Type};
use crate::parser::{parse_ssa_with_role_and_env, FileRole};
use crate::syntax::ast::{Declaration, EnumDef, FunctionDef, Program, StructDef};

/// Lowers a high-level AST `Program` into an SSA `Module`.
pub fn lower_program(prog: &Program) -> Result<Module, LowerError> {
    let (module, _) = lower_program_with_schemas_and_source_map(prog, &[], None)?;
    Ok(module)
}

/// Lowers a high-level AST `Program` into an SSA `Module` with source map tracking.
pub fn lower_program_with_source_map(prog: &Program) -> Result<(Module, SourceMap), LowerError> {
    lower_program_with_schemas_and_source_map(prog, &[], None)
}

/// Lowers a high-level AST `Program` with external schemas.
pub fn lower_program_with_schemas(
    prog: &Program,
    extra_schemas: &[Schema],
) -> Result<Module, LowerError> {
    let (module, _) = lower_program_with_schemas_and_source_map(prog, extra_schemas, None)?;
    Ok(module)
}

/// Lowers a high-level `.knr` file from disk, automatically resolving relative imports
/// (`.knr`, `.ssa`, and `.ssa.schema`) with cycle detection and diamond deduplication.
pub fn lower_program_from_file(path: &Path) -> Result<Module, LowerError> {
    let (module, _) = lower_program_from_file_with_source_map(path)?;
    Ok(module)
}

/// Lowers a high-level `.knr` file from disk and returns both the SSA `Module` and `SourceMap`.
pub fn lower_program_from_file_with_source_map(
    path: &Path,
) -> Result<(Module, SourceMap), LowerError> {
    let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let src = fs::read_to_string(&canonical).map_err(|e| LowerError::InvalidConstruct {
        line: 1,
        msg: format!("Failed to read '{}': {e}", canonical.display()),
    })?;
    let prog = crate::syntax::parse_knr(&src).map_err(|e| LowerError::InvalidConstruct {
        line: 1,
        msg: e.to_string(),
    })?;
    let base_dir = canonical.parent().map(|p| p.to_path_buf());
    let mut visited = HashSet::new();
    let mut active_stack = vec![canonical.clone()];
    let res = lower_program_recursive(
        &prog,
        &[],
        &[],
        base_dir.as_deref(),
        &mut visited,
        &mut active_stack,
    )?;
    visited.insert(canonical);
    Ok(res)
}

pub fn lower_program_with_schemas_and_source_map(
    prog: &Program,
    extra_schemas: &[Schema],
    base_dir: Option<PathBuf>,
) -> Result<(Module, SourceMap), LowerError> {
    lower_program_with_env_and_source_map(prog, extra_schemas, &[], base_dir)
}

pub fn lower_program_with_env_and_source_map(
    prog: &Program,
    extra_schemas: &[Schema],
    extra_functions: &[Function],
    base_dir: Option<PathBuf>,
) -> Result<(Module, SourceMap), LowerError> {
    let mut visited = HashSet::new();
    let mut active_stack = Vec::new();
    lower_program_recursive(
        prog,
        extra_schemas,
        extra_functions,
        base_dir.as_deref(),
        &mut visited,
        &mut active_stack,
    )
}

fn lower_program_recursive(
    prog: &Program,
    extra_schemas: &[Schema],
    extra_functions: &[Function],
    base_dir: Option<&Path>,
    visited: &mut HashSet<PathBuf>,
    active_stack: &mut Vec<PathBuf>,
) -> Result<(Module, SourceMap), LowerError> {
    let mut module = Module::new();
    let mut schemas_map: HashMap<String, Schema> = HashMap::new();
    let mut enums_map: HashMap<String, EnumMeta> = HashMap::new();
    let mut fn_sigs: HashMap<String, FnSig> = HashMap::new();
    let mut existing_fn_names: HashSet<String> = HashSet::new();
    let mut overall_source_map = SourceMap::new();

    // 1. Populate extra schemas & functions
    for schema in extra_schemas {
        if schemas_map
            .insert(schema.name.clone(), schema.clone())
            .is_none()
        {
            module.add_schema(schema.clone());
        }
    }
    for f in extra_functions {
        fn_sigs.insert(
            f.name.clone(),
            FnSig {
                param_types: f.params.iter().map(|(_, ty)| ty.clone()).collect(),
                param_borrows: (0..f.params.len())
                    .map(|i| f.is_param_borrowed(i))
                    .collect(),
                ret_ty: f.ret_ty.clone(),
                effects: f.effects.clone(),
            },
        );
    }

    // 2. Recursively resolve import directives (built-in std/*, .ssa.schema, .ssa, .knr)
    for import_decl in &prog.imports {
        module.add_import(&import_decl.path);

        if let Some(builtin_src) = crate::syntax::stdlib::resolve_builtin_std(&import_decl.path) {
            let canonical_key = import_decl
                .path
                .strip_suffix(".knr")
                .unwrap_or(&import_decl.path);
            let canon = PathBuf::from(format!("<builtin:{canonical_key}>"));

            if active_stack.contains(&canon) {
                return Err(LowerError::InvalidConstruct {
                    line: import_decl.span.line,
                    msg: format!("Circular import detected involving '{}'", import_decl.path),
                });
            }
            if !visited.insert(canon.clone()) {
                continue;
            }

            let dep_prog =
                crate::syntax::parse_knr(builtin_src).map_err(|e| LowerError::InvalidConstruct {
                    line: import_decl.span.line,
                    msg: format!("Error parsing built-in '{}': {e}", import_decl.path),
                })?;

            // Collect enum metadata from built-in declarations as well
            for decl in &dep_prog.declarations {
                if let Declaration::Enum(ref enum_def) = decl {
                    let (schema, meta) = convert_enum_def(enum_def)?;
                    if schemas_map.insert(schema.name.clone(), schema.clone()).is_none() {
                        module.add_schema(schema);
                    }
                    enums_map.insert(meta.name.clone(), meta);
                }
            }

            active_stack.push(canon);
            let cur_schemas: Vec<Schema> = module.schemas.clone();
            let cur_funcs: Vec<Function> = module.functions.clone();
            let (dep_mod, dep_sm) = lower_program_recursive(
                &dep_prog,
                &cur_schemas,
                &cur_funcs,
                None,
                visited,
                active_stack,
            )?;
            active_stack.pop();

            for s in dep_mod.schemas {
                if schemas_map.insert(s.name.clone(), s.clone()).is_none() {
                    module.add_schema(s);
                }
            }
            for f in dep_mod.functions {
                fn_sigs.insert(
                    f.name.clone(),
                    FnSig {
                        param_types: f.params.iter().map(|(_, ty)| ty.clone()).collect(),
                        param_borrows: (0..f.params.len())
                            .map(|i| f.is_param_borrowed(i))
                            .collect(),
                        ret_ty: f.ret_ty.clone(),
                        effects: f.effects.clone(),
                    },
                );
                if existing_fn_names.insert(f.name.clone()) {
                    module.add_function(f);
                }
            }
            overall_source_map.reg_to_var.extend(dep_sm.reg_to_var);
            overall_source_map.reg_to_span.extend(dep_sm.reg_to_span);
            continue;
        }

        let resolved_path = resolve_import_path(&import_decl.path, base_dir);
        if let Some(target_path) = resolved_path {
            let canon = target_path
                .canonicalize()
                .unwrap_or_else(|_| target_path.clone());

            if active_stack.contains(&canon) {
                return Err(LowerError::InvalidConstruct {
                    line: import_decl.span.line,
                    msg: format!("Circular import detected involving '{}'", import_decl.path),
                });
            }
            if !visited.insert(canon.clone()) {
                continue;
            }

            let target_str = canon.to_string_lossy().to_string();
            if target_str.ends_with(".knr") {
                let content =
                    fs::read_to_string(&canon).map_err(|e| LowerError::InvalidConstruct {
                        line: import_decl.span.line,
                        msg: format!("Failed to read imported file '{}': {e}", import_decl.path),
                    })?;
                let dep_prog =
                    crate::syntax::parse_knr(&content).map_err(|e| LowerError::InvalidConstruct {
                        line: import_decl.span.line,
                        msg: format!("Error parsing imported '{}': {e}", import_decl.path),
                    })?;

                // Collect enum metadata from dependency declarations as well
                for decl in &dep_prog.declarations {
                    if let Declaration::Enum(ref enum_def) = decl {
                        let (schema, meta) = convert_enum_def(enum_def)?;
                        if schemas_map.insert(schema.name.clone(), schema.clone()).is_none() {
                            module.add_schema(schema);
                        }
                        enums_map.insert(meta.name.clone(), meta);
                    }
                }

                active_stack.push(canon.clone());
                let cur_schemas: Vec<Schema> = module.schemas.clone();
                let cur_funcs: Vec<Function> = module.functions.clone();
                let (dep_mod, dep_sm) = lower_program_recursive(
                    &dep_prog,
                    &cur_schemas,
                    &cur_funcs,
                    canon.parent(),
                    visited,
                    active_stack,
                )?;
                active_stack.pop();

                for s in dep_mod.schemas {
                    if schemas_map.insert(s.name.clone(), s.clone()).is_none() {
                        module.add_schema(s);
                    }
                }
                for f in dep_mod.functions {
                    fn_sigs.insert(
                        f.name.clone(),
                        FnSig {
                            param_types: f.params.iter().map(|(_, ty)| ty.clone()).collect(),
                            param_borrows: (0..f.params.len())
                                .map(|i| f.is_param_borrowed(i))
                                .collect(),
                            ret_ty: f.ret_ty.clone(),
                            effects: f.effects.clone(),
                        },
                    );
                    if existing_fn_names.insert(f.name.clone()) {
                        module.add_function(f);
                    }
                }
                overall_source_map.reg_to_var.extend(dep_sm.reg_to_var);
                overall_source_map.reg_to_span.extend(dep_sm.reg_to_span);
            } else if target_str.ends_with(".schema") || target_str.ends_with(".ssa.schema") {
                if let Ok(content) = fs::read_to_string(&canon) {
                    let cur_schemas: Vec<Schema> = module.schemas.clone();
                    if let Ok(parsed_mod) =
                        parse_ssa_with_role_and_env(&content, FileRole::Schema, &cur_schemas, &[])
                    {
                        for s in parsed_mod.schemas {
                            if schemas_map.insert(s.name.clone(), s.clone()).is_none() {
                                module.add_schema(s);
                            }
                        }
                    }
                }
            } else if target_str.ends_with(".ssa") {
                if let Ok(loaded_mod) = crate::loader::load_module_from_path(&canon) {
                    for s in loaded_mod.schemas {
                        if schemas_map.insert(s.name.clone(), s.clone()).is_none() {
                            module.add_schema(s);
                        }
                    }
                    for f in loaded_mod.functions {
                        fn_sigs.insert(
                            f.name.clone(),
                            FnSig {
                                param_types: f.params.iter().map(|(_, ty)| ty.clone()).collect(),
                                param_borrows: (0..f.params.len())
                                    .map(|i| f.is_param_borrowed(i))
                                    .collect(),
                                ret_ty: f.ret_ty.clone(),
                                effects: f.effects.clone(),
                            },
                        );
                        if existing_fn_names.insert(f.name.clone()) {
                            module.add_function(f);
                        }
                    }
                }
            }
        }
    }

    // 3. Process declared structs/schemas & enums in this Program
    for decl in &prog.declarations {
        match decl {
            Declaration::Struct(ref struct_def) => {
                let schema = convert_struct_def(struct_def)?;
                schemas_map.insert(schema.name.clone(), schema.clone());
                module.add_schema(schema);
            }
            Declaration::Enum(ref enum_def) => {
                let (schema, meta) = convert_enum_def(enum_def)?;
                schemas_map.insert(schema.name.clone(), schema.clone());
                module.add_schema(schema);
                enums_map.insert(meta.name.clone(), meta);
            }
            Declaration::Function(_) => {}
        }
    }

    // 4. Collect function signatures
    for decl in &prog.declarations {
        if let Declaration::Function(ref fn_def) = decl {
            let mut param_types = Vec::new();
            let mut param_borrows = Vec::new();
            for p in &fn_def.params {
                let ty = p
                    .ty
                    .to_core_type()
                    .map_err(|msg| LowerError::InvalidConstruct {
                        line: p.span.line,
                        msg,
                    })?;
                param_types.push(ty);
                param_borrows.push(p.is_borrowed);
            }
            let ret_ty = fn_def
                .ret_ty
                .to_core_type()
                .map_err(|msg| LowerError::InvalidConstruct {
                    line: fn_def.span.line,
                    msg,
                })?;
            let mut effects = Vec::new();
            for eff_ident in &fn_def.effects {
                let eff = match eff_ident.name.as_str() {
                    "Net" => Effect::Net,
                    "Ui" => Effect::Ui,
                    "Hw" => Effect::Hw,
                    "Async" => Effect::Async,
                    _ => {
                        return Err(LowerError::InvalidConstruct {
                            line: eff_ident.span.line,
                            msg: format!("Unknown effect '{}'", eff_ident.name),
                        });
                    }
                };
                effects.push(eff);
            }
            fn_sigs.insert(
                fn_def.name.name.clone(),
                FnSig {
                    param_types,
                    param_borrows,
                    ret_ty,
                    effects,
                },
            );
        }
    }

    // 5. Lower each function in this Program
    for decl in &prog.declarations {
        if let Declaration::Function(ref fn_def) = decl {
            let (func, fn_source_map) =
                lower_function_def(fn_def, &schemas_map, &enums_map, &fn_sigs)?;
            if existing_fn_names.insert(func.name.clone()) {
                module.add_function(func);
            }

            // Merge source map
            overall_source_map.reg_to_var.extend(fn_source_map.reg_to_var);
            overall_source_map
                .reg_to_span
                .extend(fn_source_map.reg_to_span);
            overall_source_map
                .line_spans
                .extend(fn_source_map.line_spans);
        }
    }

    // 6. Resolve call types across module
    module.resolve_call_types();

    Ok((module, overall_source_map))
}

fn convert_struct_def(def: &StructDef) -> Result<Schema, LowerError> {
    let mut schema = Schema::new(&def.name.name);
    for (i, field) in def.fields.iter().enumerate() {
        let idx = field.index.unwrap_or(i as u32);
        let ty = field
            .ty
            .to_core_type()
            .map_err(|msg| LowerError::InvalidConstruct {
                line: field.span.line,
                msg,
            })?;
        schema.add_field(idx, Some(field.name.name.clone()), ty);
    }
    Ok(schema)
}

fn convert_enum_def(def: &EnumDef) -> Result<(Schema, EnumMeta), LowerError> {
    let mut schema = Schema::new(&def.name.name);
    // Field 0 is always the discriminant tag (`__tag: i64`)
    schema.add_field(0, Some("__tag".to_string()), Type::I64);
    let mut next_slot = 1u32;
    let mut variants = Vec::new();

    for (tag_idx, variant) in def.variants.iter().enumerate() {
        let mut field_slots = Vec::new();
        for (f_idx, f_ty_expr) in variant.fields.iter().enumerate() {
            let ty = f_ty_expr
                .to_core_type()
                .map_err(|msg| LowerError::InvalidConstruct {
                    line: variant.span.line,
                    msg,
                })?;
            let slot_name = format!("__v{tag_idx}_{f_idx}");
            schema.add_field(next_slot, Some(slot_name), ty.clone());
            field_slots.push((next_slot, ty));
            next_slot += 1;
        }
        variants.push(EnumVariantMeta {
            name: variant.name.name.clone(),
            tag: tag_idx as i64,
            field_slots,
        });
    }

    Ok((
        schema,
        EnumMeta {
            name: def.name.name.clone(),
            variants,
        },
    ))
}

fn lower_function_def(
    fn_def: &FunctionDef,
    schemas: &HashMap<String, Schema>,
    enums: &HashMap<String, EnumMeta>,
    fn_sigs: &HashMap<String, FnSig>,
) -> Result<(Function, SourceMap), LowerError> {
    let sig = fn_sigs.get(&fn_def.name.name).unwrap();
    let param_types = sig.param_types.clone();
    let param_borrows = sig.param_borrows.clone();
    let ret_ty = sig.ret_ty.clone();
    let effects = sig.effects.clone();

    let mut func = Function::new(&fn_def.name.name, ret_ty.clone());
    func.effects = effects.clone();
    func.is_shader = fn_def.is_shader;

    let mut ctx = FnLoweringContext::new(
        fn_def.name.name.clone(),
        ret_ty.clone(),
        effects,
        param_types.len(),
        schemas,
        enums,
        fn_sigs,
    );

    // Bind parameters: %0, %1, ..., %P-1
    for (i, ((param_def, ty), &is_borrowed)) in fn_def
        .params
        .iter()
        .zip(param_types.iter())
        .zip(param_borrows.iter())
        .enumerate()
    {
        let reg = i as u32;
        if is_borrowed {
            func.add_borrowed_param(reg, ty.clone());
            ctx.bind_borrowed_param(&param_def.name.name, reg, ty.clone(), param_def.span);
        } else {
            func.add_param(reg, ty.clone());
            // For owned linear parameters: local_alloc is false (caller owns; unconsumed leaks unless freed/returned)
            ctx.bind_var(&param_def.name.name, reg, ty.clone(), false, param_def.span);
        }
    }

    // Lower function body (top-level tail expression acts as implicit return)
    ctx.lower_fn_body(&fn_def.body)?;

    // Handle end of function
    if !ctx.is_terminated {
        if ret_ty == Type::Void {
            ctx.lower_return(None, fn_def.span.line)?;
        } else {
            return Err(LowerError::MissingReturn {
                line: fn_def.span.line,
                func: fn_def.name.name.clone(),
                ret_ty: ret_ty.to_string(),
            });
        }
    }

    func.blocks = ctx.completed_blocks;
    if func.blocks.is_empty() {
        func.add_block(BasicBlock::new(0, Terminator::Ret(None)));
    }

    // Normalize registers to guarantee strict monotonic sequential ordering
    normalize_function_registers(&mut func, &mut ctx.source_map);

    Ok((func, ctx.source_map))
}

fn resolve_import_path(rel_path: &str, base_dir: Option<&Path>) -> Option<PathBuf> {
    let mut candidates = Vec::new();

    if let Some(base) = base_dir {
        candidates.push(base.join(rel_path));
    }
    candidates.push(PathBuf::from(rel_path));

    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd.join(rel_path));
        candidates.push(cwd.join("site/components").join(rel_path));
        candidates.push(cwd.join("site").join(rel_path));
        candidates.push(cwd.join("tests/fixtures").join(rel_path));

        if let Some(parent) = cwd.parent() {
            candidates.push(parent.join(rel_path));
            candidates.push(parent.join("site/components").join(rel_path));
            candidates.push(parent.join("site").join(rel_path));
            candidates.push(parent.join("tests/fixtures").join(rel_path));
            if let Some(grandparent) = parent.parent() {
                candidates.push(grandparent.join(rel_path));
                candidates.push(grandparent.join("site/components").join(rel_path));
                candidates.push(grandparent.join("site").join(rel_path));
                candidates.push(grandparent.join("tests/fixtures").join(rel_path));
            }
        }
    }

    for c in candidates {
        if let Ok(canon) = c.canonicalize() {
            if canon.exists() {
                return Some(canon);
            }
        } else if c.exists() {
            return Some(c);
        }
    }
    None
}
