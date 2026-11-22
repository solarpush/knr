pub mod context;
pub mod error;
pub mod expr;
pub mod jsx;
pub mod monomorph;
pub mod normalize;
pub mod stmt;

pub use context::{EnumMeta, EnumVariantMeta, FnLoweringContext, FnSig, SourceMap};
pub use error::LowerError;
pub use monomorph::GenericRegistry;
pub use normalize::normalize_function_registers;

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use crate::ast::{BasicBlock, Effect, Function, Module, Schema, Terminator, Type};
use crate::parser::{parse_ssa_with_role_and_env, FileRole};
use crate::syntax::ast::{
    ArrowBody, BinaryOp, Block, Declaration, DestructPattern, EnumDef, Expr, ExprKind, FunctionDef,
    Ident, ImportDecl, ImportKind, JsxChild, MatchPattern, ParamDef, Program, Span, Stmt, StmtKind,
    StructDef, TemplatePart, TypeExpr, TypeExprKind,
};

/// Shared environment threaded across recursive module imports so that:
/// 1. When file A imports B then C (and C calls functions or uses components/enums/consts/generics from B),
///    C and all its transitive sub-imports immediately see B's loaded symbols without ordering hacks.
/// 2. Any `extra_functions` / `extra_schemas` supplied at the root remain visible to all imported files.
/// 3. Generic templates (`type Pair<A, B>`, `enum Option<T>`, `function first<A, B>`) from `"std/core"`
///    and user modules are available for on-demand monomorphization across the entire import graph.
/// 4. Per-module symbol sets (`module_symbols`) are recorded for named import validation, `as` aliases,
///    namespace imports (`import * as ns from "..."`), and barrel re-exports (`export { ... } from "..."`).
#[derive(Debug, Clone, Default)]
struct TransitiveEnv {
    schemas: HashMap<String, Schema>,
    enums: HashMap<String, EnumMeta>,
    fn_sigs: HashMap<String, FnSig>,
    const_exprs: HashMap<String, Expr>,
    generics: GenericRegistry,
    existing_fns: HashSet<String>,
    module_symbols: HashMap<PathBuf, HashSet<String>>,
}

impl TransitiveEnv {
    fn from_extras(extra_schemas: &[Schema], extra_functions: &[Function]) -> Self {
        let mut env = Self::default();
        for s in extra_schemas {
            env.schemas.entry(s.name.clone()).or_insert_with(|| s.clone());
        }
        for f in extra_functions {
            env.existing_fns.insert(f.name.clone());
            env.fn_sigs.insert(
                f.name.clone(),
                FnSig {
                    param_names: (0..f.params.len()).map(|i| format!("arg{i}")).collect(),
                    param_types: f.params.iter().map(|(_, ty)| ty.clone()).collect(),
                    param_borrows: (0..f.params.len())
                        .map(|i| f.is_param_borrowed(i))
                        .collect(),
                    ret_ty: f.ret_ty.clone(),
                    effects: f.effects.clone(),
                    is_shader: f.is_shader,
                },
            );
        }
        env
    }
}

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

/// Lowers a high-level `.tsk`/`.knr` file from disk, automatically resolving relative imports
/// (`.tsk`, `.knr`, `.ssa`, and `.ssa.schema`) with cycle detection and diamond deduplication.
pub fn lower_program_from_file(path: &Path) -> Result<Module, LowerError> {
    let (module, _) = lower_program_from_file_with_source_map(path)?;
    Ok(module)
}

/// Lowers a high-level `.tsk`/`.knr` file from disk and returns both the SSA `Module` and `SourceMap`.
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
    let mut env = TransitiveEnv::default();
    let empty_overlays = HashMap::new();
    let res = lower_program_recursive(
        &prog,
        Some(&canonical),
        base_dir.as_deref(),
        &empty_overlays,
        &mut visited,
        &mut active_stack,
        &mut env,
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

/// Lowers an in-memory AST `Program` using optional unsaved file overlays (used by `knr lsp`
/// so cross-file edits reflect immediately before saving to disk).
pub fn lower_program_with_overlays_and_source_map(
    prog: &Program,
    extra_schemas: &[Schema],
    base_dir: Option<PathBuf>,
    overlays: &HashMap<PathBuf, String>,
) -> Result<(Module, SourceMap), LowerError> {
    let mut visited = HashSet::new();
    let mut active_stack = Vec::new();
    let mut env = TransitiveEnv::from_extras(extra_schemas, &[]);
    lower_program_recursive(
        prog,
        None,
        base_dir.as_deref(),
        overlays,
        &mut visited,
        &mut active_stack,
        &mut env,
    )
}

pub fn lower_program_with_env_and_source_map(
    prog: &Program,
    extra_schemas: &[Schema],
    extra_functions: &[Function],
    base_dir: Option<PathBuf>,
) -> Result<(Module, SourceMap), LowerError> {
    let mut visited = HashSet::new();
    let mut active_stack = Vec::new();
    let mut env = TransitiveEnv::from_extras(extra_schemas, extra_functions);
    let empty_overlays = HashMap::new();
    lower_program_recursive(
        prog,
        None,
        base_dir.as_deref(),
        &empty_overlays,
        &mut visited,
        &mut active_stack,
        &mut env,
    )
}

fn lower_program_recursive(
    prog: &Program,
    self_key: Option<&Path>,
    base_dir: Option<&Path>,
    overlays: &HashMap<PathBuf, String>,
    visited: &mut HashSet<PathBuf>,
    active_stack: &mut Vec<PathBuf>,
    env: &mut TransitiveEnv,
) -> Result<(Module, SourceMap), LowerError> {
    let mut module = Module::new();
    let mut module_schemas: HashSet<String> = HashSet::new();
    let mut existing_fn_names: HashSet<String> = HashSet::new();
    let mut overall_source_map = SourceMap::new();
    let mut alias_map: HashMap<String, String> = HashMap::new();
    let mut ns_map: HashMap<String, HashSet<String>> = HashMap::new();
    let mut this_module_symbols: HashSet<String> = collect_program_declared_symbols(prog);

    // 1. Seed this module with any pre-loaded schemas already in `env`
    for schema in env.schemas.values() {
        if module_schemas.insert(schema.name.clone()) {
            module.add_schema(schema.clone());
        }
    }

    // 2. Recursively resolve import directives (built-in std/*, .ssa.schema, .ssa, .tsk/.knr)
    for import_decl in &prog.imports {
        module.add_import(&import_decl.path);

        if let Some(builtin_src) = crate::syntax::stdlib::resolve_builtin_std(&import_decl.path) {
            let canonical_key = import_decl
                .path
                .strip_suffix(".tsk")
                .or_else(|| import_decl.path.strip_suffix(".knr"))
                .unwrap_or(&import_decl.path);
            let canon = PathBuf::from(format!("<builtin:{canonical_key}>"));

            if active_stack.contains(&canon) {
                return Err(LowerError::InvalidConstruct {
                    line: import_decl.span.line,
                    msg: format!("Circular import detected involving '{}'", import_decl.path),
                });
            }
            if visited.insert(canon.clone()) {
                let dep_prog = crate::syntax::parse_knr(builtin_src).map_err(|e| {
                    LowerError::InvalidConstruct {
                        line: import_decl.span.line,
                        msg: format!("Error parsing built-in '{}': {e}", import_decl.path),
                    }
                })?;

                active_stack.push(canon.clone());
                let (dep_mod, dep_sm) = lower_program_recursive(
                    &dep_prog,
                    Some(&canon),
                    None,
                    overlays,
                    visited,
                    active_stack,
                    env,
                )?;
                active_stack.pop();

                for s in dep_mod.schemas {
                    env.schemas.entry(s.name.clone()).or_insert_with(|| s.clone());
                    if module_schemas.insert(s.name.clone()) {
                        module.add_schema(s);
                    }
                }
                for f in dep_mod.functions {
                    env.existing_fns.insert(f.name.clone());
                    if existing_fn_names.insert(f.name.clone()) {
                        module.add_function(f);
                    }
                }
                overall_source_map.reg_to_var.extend(dep_sm.reg_to_var);
                overall_source_map.reg_to_span.extend(dep_sm.reg_to_span);
            } else {
                // Already lowered earlier; symbols are already present in `env`.
                for schema in env.schemas.values() {
                    if module_schemas.insert(schema.name.clone()) {
                        module.add_schema(schema.clone());
                    }
                }
            }

            let dep_syms = env
                .module_symbols
                .get(&canon)
                .cloned()
                .unwrap_or_default();
            register_import_bindings(
                import_decl,
                &dep_syms,
                false,
                &mut alias_map,
                &mut ns_map,
                &mut this_module_symbols,
            )?;
            continue;
        }

        let resolved_path = resolve_import_path(&import_decl.path, base_dir, overlays);
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

            let target_str = canon.to_string_lossy().to_string();
            let is_local_tsk = crate::is_source_file(&canon);

            if visited.insert(canon.clone()) {
                if is_local_tsk {
                    let content = if let Some(ov) = lookup_overlay(overlays, &canon, &target_path) {
                        ov.clone()
                    } else {
                        fs::read_to_string(&canon).map_err(|e| LowerError::InvalidConstruct {
                            line: import_decl.span.line,
                            msg: format!(
                                "Failed to read imported file '{}': {e}",
                                import_decl.path
                            ),
                        })?
                    };
                    let dep_prog = crate::syntax::parse_knr(&content).map_err(|e| {
                        LowerError::InvalidConstruct {
                            line: import_decl.span.line,
                            msg: format!("Error parsing imported '{}': {e}", import_decl.path),
                        }
                    })?;

                    active_stack.push(canon.clone());
                    let (dep_mod, dep_sm) = lower_program_recursive(
                        &dep_prog,
                        Some(&canon),
                        canon.parent(),
                        overlays,
                        visited,
                        active_stack,
                        env,
                    )?;
                    active_stack.pop();

                    for s in dep_mod.schemas {
                        env.schemas.entry(s.name.clone()).or_insert_with(|| s.clone());
                        if module_schemas.insert(s.name.clone()) {
                            module.add_schema(s);
                        }
                    }
                    for f in dep_mod.functions {
                        env.existing_fns.insert(f.name.clone());
                        if existing_fn_names.insert(f.name.clone()) {
                            module.add_function(f);
                        }
                    }
                    overall_source_map.reg_to_var.extend(dep_sm.reg_to_var);
                    overall_source_map.reg_to_span.extend(dep_sm.reg_to_span);
                } else if target_str.ends_with(".schema") || target_str.ends_with(".ssa.schema") {
                    if let Ok(content) = fs::read_to_string(&canon) {
                        let cur_schemas: Vec<Schema> = env.schemas.values().cloned().collect();
                        if let Ok(parsed_mod) = parse_ssa_with_role_and_env(
                            &content,
                            FileRole::Schema,
                            &cur_schemas,
                            &[],
                        ) {
                            let mut syms = HashSet::new();
                            for s in parsed_mod.schemas {
                                syms.insert(s.name.clone());
                                env.schemas.entry(s.name.clone()).or_insert_with(|| s.clone());
                                if module_schemas.insert(s.name.clone()) {
                                    module.add_schema(s);
                                }
                            }
                            env.module_symbols.insert(canon.clone(), syms);
                        }
                    }
                } else if target_str.ends_with(".ssa") {
                    if let Ok(loaded_mod) = crate::loader::load_module_from_path(&canon) {
                        let mut syms = HashSet::new();
                        for s in loaded_mod.schemas {
                            syms.insert(s.name.clone());
                            env.schemas.entry(s.name.clone()).or_insert_with(|| s.clone());
                            if module_schemas.insert(s.name.clone()) {
                                module.add_schema(s);
                            }
                        }
                        for f in loaded_mod.functions {
                            syms.insert(f.name.clone());
                            env.existing_fns.insert(f.name.clone());
                            env.fn_sigs.insert(
                                f.name.clone(),
                                FnSig {
                                    param_names: (0..f.params.len())
                                        .map(|i| format!("arg{i}"))
                                        .collect(),
                                    param_types: f
                                        .params
                                        .iter()
                                        .map(|(_, ty)| ty.clone())
                                        .collect(),
                                    param_borrows: (0..f.params.len())
                                        .map(|i| f.is_param_borrowed(i))
                                        .collect(),
                                    ret_ty: f.ret_ty.clone(),
                                    effects: f.effects.clone(),
                                    is_shader: f.is_shader,
                                },
                            );
                            if existing_fn_names.insert(f.name.clone()) {
                                module.add_function(f);
                            }
                        }
                        env.module_symbols.insert(canon.clone(), syms);
                    }
                }
            } else {
                for schema in env.schemas.values() {
                    if module_schemas.insert(schema.name.clone()) {
                        module.add_schema(schema.clone());
                    }
                }
            }

            let dep_syms = env
                .module_symbols
                .get(&canon)
                .cloned()
                .unwrap_or_default();
            register_import_bindings(
                import_decl,
                &dep_syms,
                is_local_tsk,
                &mut alias_map,
                &mut ns_map,
                &mut this_module_symbols,
            )?;
        } else if base_dir.is_some() {
            return Err(LowerError::InvalidConstruct {
                line: import_decl.span.line,
                msg: format!(
                    "File not found: cannot resolve import '{}'",
                    import_decl.path
                ),
            });
        }
    }

    if let Some(key) = self_key {
        env.module_symbols
            .insert(key.to_path_buf(), this_module_symbols);
    }

    // 2b. Rewrite aliased named imports (`import { a as b }`) and namespace imports (`import * as ns`)
    let aliased_prog;
    let effective_prog = if alias_map.is_empty() && ns_map.is_empty() {
        prog
    } else {
        aliased_prog = rewrite_program_imports(prog, &alias_map, &ns_map);
        &aliased_prog
    };

    // 3. On-demand monomorphization of generic structs, enums, and functions
    let existing_schema_names: HashSet<String> = env.schemas.keys().cloned().collect();
    let mut concrete_prog = monomorph::monomorphize_program(
        effective_prog,
        &mut env.generics,
        &existing_schema_names,
        &env.existing_fns,
    )?;

    // 4. Process declared structs/schemas, enums & top-level consts in this Program
    // Keep same-file duplicate declarations in `module.schemas` so `knr-verify` reports `DuplicateSchema`.
    let mut declared_in_this_file: HashSet<String> = HashSet::new();
    for decl in &concrete_prog.declarations {
        match decl {
            Declaration::Struct(ref struct_def) => {
                let schema = convert_struct_def(struct_def)?;
                env.schemas.insert(schema.name.clone(), schema.clone());
                let is_dup_local = !declared_in_this_file.insert(schema.name.clone());
                if is_dup_local || module_schemas.insert(schema.name.clone()) {
                    module.add_schema(schema);
                }
            }
            Declaration::Enum(ref enum_def) => {
                let (schema, meta) = convert_enum_def(enum_def)?;
                env.schemas.insert(schema.name.clone(), schema.clone());
                let is_dup_local = !declared_in_this_file.insert(schema.name.clone());
                if is_dup_local || module_schemas.insert(schema.name.clone()) {
                    module.add_schema(schema);
                }
                env.enums.insert(meta.name.clone(), meta);
            }
            Declaration::Const(ref const_def) => {
                env.const_exprs
                    .insert(const_def.name.name.clone(), const_def.value.clone());
            }
            Declaration::Function(_) => {}
        }
    }

    // 4b. Synthesize exhaustive string->enum `update` bridge if `export function update(s: State, act: SomeEnum): State`
    //     uses a declared enum whose variants are ALL unit (0-field) variants.
    synthesize_typed_update_bridge(&mut concrete_prog, &env.enums)?;

    // 5. Collect function signatures (including real parameter names for JSX components)
    for decl in &concrete_prog.declarations {
        if let Declaration::Function(ref fn_def) = decl {
            let mut param_names = Vec::new();
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
                param_names.push(p.name.name.clone());
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
            env.fn_sigs.insert(
                fn_def.name.name.clone(),
                FnSig {
                    param_names,
                    param_types,
                    param_borrows,
                    ret_ty,
                    effects,
                    is_shader: fn_def.is_shader,
                },
            );
        }
    }

    // 6. Lower each function in this Program
    for decl in &concrete_prog.declarations {
        if let Declaration::Function(ref fn_def) = decl {
            let (func, fn_source_map) = lower_function_def(
                fn_def,
                &env.schemas,
                &env.enums,
                &env.fn_sigs,
                &env.const_exprs,
            )?;
            env.existing_fns.insert(func.name.clone());
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

    // 7. Resolve call types across module
    module.resolve_call_types();

    Ok((module, overall_source_map))
}

/// If the module defines `update(s: State, action: SomeEnum): State` where `SomeEnum` is a
/// declared enum whose variants are ALL unit (0-field) variants, renames the typed function to
/// `__knr_typed_update` and synthesizes an `export function update(s: State, raw: string): State`
/// bridge dispatching each variant name string into `__knr_typed_update` and falling back to `s`.
fn synthesize_typed_update_bridge(
    prog: &mut Program,
    enums: &HashMap<String, EnumMeta>,
) -> Result<(), LowerError> {
    let mut bridge_spec: Option<(usize, ParamDef, TypeExpr, Vec<Ident>, EnumMeta, Span)> = None;

    for (idx, decl) in prog.declarations.iter().enumerate() {
        if let Declaration::Function(fn_def) = decl {
            if fn_def.name.name != "update" || fn_def.params.len() != 2 {
                continue;
            }
            if fn_def.params[0].is_borrowed {
                continue;
            }
            let Ok(p0_ty) = fn_def.params[0].ty.to_core_type() else {
                continue;
            };
            let Ok(ret_ty) = fn_def.ret_ty.to_core_type() else {
                continue;
            };
            if p0_ty != ret_ty {
                continue;
            }
            let Ok(Type::Struct(enum_name)) = fn_def.params[1].ty.to_core_type() else {
                continue;
            };
            let Some(enum_meta) = enums.get(&enum_name) else {
                continue;
            };
            if enum_meta.variants.is_empty()
                || !enum_meta.variants.iter().all(|v| v.field_slots.is_empty())
            {
                continue;
            }
            bridge_spec = Some((
                idx,
                fn_def.params[0].clone(),
                fn_def.ret_ty.clone(),
                fn_def.effects.clone(),
                enum_meta.clone(),
                fn_def.span,
            ));
            break;
        }
    }

    let Some((fn_idx, state_param, ret_ty, effects, enum_meta, span)) = bridge_spec else {
        return Ok(());
    };

    // Rename user's typed `update` to `__knr_typed_update`
    if let Declaration::Function(ref mut fn_def) = prog.declarations[fn_idx] {
        fn_def.name.name = "__knr_typed_update".to_string();
    }

    let s_ident = Ident::new("__knr_s", span);
    let raw_ident = Ident::new("__knr_raw", span);
    let mut stmts = Vec::with_capacity(enum_meta.variants.len() + 1);

    for variant in &enum_meta.variants {
        let cond = Expr {
            kind: ExprKind::Binary {
                op: BinaryOp::Eq,
                lhs: Box::new(Expr {
                    kind: ExprKind::Variable(raw_ident.clone()),
                    span,
                }),
                rhs: Box::new(Expr {
                    kind: ExprKind::Str(variant.name.clone()),
                    span,
                }),
            },
            span,
        };
        let variant_expr = Expr {
            kind: ExprKind::EnumLit {
                enum_name: Ident::new(enum_meta.name.clone(), span),
                type_args: Vec::new(),
                variant: Ident::new(variant.name.clone(), span),
                args: Vec::new(),
            },
            span,
        };
        let call_expr = Expr {
            kind: ExprKind::Call {
                callee: Ident::new("__knr_typed_update", span),
                type_args: Vec::new(),
                args: vec![
                    Expr {
                        kind: ExprKind::Variable(s_ident.clone()),
                        span,
                    },
                    variant_expr,
                ],
            },
            span,
        };
        stmts.push(Stmt {
            kind: StmtKind::If {
                cond,
                then_block: Block {
                    stmts: vec![Stmt {
                        kind: StmtKind::Return {
                            value: Some(call_expr),
                        },
                        span,
                    }],
                    tail_expr: None,
                    span,
                },
                else_block: None,
            },
            span,
        });
    }

    // Fallback: return `s` unchanged on unknown raw action string
    stmts.push(Stmt {
        kind: StmtKind::Return {
            value: Some(Expr {
                kind: ExprKind::Variable(s_ident.clone()),
                span,
            }),
        },
        span,
    });

    let bridge_fn = FunctionDef {
        name: Ident::new("update", span),
        type_params: Vec::new(),
        params: vec![
            ParamDef {
                name: s_ident,
                ty: state_param.ty,
                is_borrowed: false,
                span,
            },
            ParamDef {
                name: raw_ident,
                ty: TypeExpr::new(TypeExprKind::Named(Ident::new("string", span)), span),
                is_borrowed: false,
                span,
            },
        ],
        ret_ty,
        effects,
        is_shader: false,
        is_exported: true,
        body: Block {
            stmts,
            tail_expr: None,
            span,
        },
        span,
    };

    prog.declarations.push(Declaration::Function(bridge_fn));
    Ok(())
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
    const_exprs: &HashMap<String, Expr>,
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
        const_exprs,
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

fn collect_program_declared_symbols(prog: &Program) -> HashSet<String> {
    let mut syms = HashSet::new();
    for decl in &prog.declarations {
        match decl {
            Declaration::Struct(s) => {
                syms.insert(s.name.name.clone());
            }
            Declaration::Enum(e) => {
                syms.insert(e.name.name.clone());
            }
            Declaration::Function(f) => {
                syms.insert(f.name.name.clone());
            }
            Declaration::Const(c) => {
                syms.insert(c.name.name.clone());
            }
        }
    }
    syms
}

fn register_import_bindings(
    import_decl: &ImportDecl,
    dep_syms: &HashSet<String>,
    validate_named: bool,
    alias_map: &mut HashMap<String, String>,
    ns_map: &mut HashMap<String, HashSet<String>>,
    this_module_symbols: &mut HashSet<String>,
) -> Result<(), LowerError> {
    match &import_decl.kind {
        ImportKind::SideEffect => {
            if import_decl.is_reexport {
                this_module_symbols.extend(dep_syms.iter().cloned());
            }
        }
        ImportKind::Namespace(alias) => {
            ns_map.insert(alias.name.clone(), dep_syms.clone());
            if import_decl.is_reexport {
                this_module_symbols.insert(alias.name.clone());
            }
        }
        ImportKind::Named(items) => {
            for (orig, alias) in items {
                if validate_named && !dep_syms.is_empty() && !dep_syms.contains(&orig.name) {
                    return Err(LowerError::InvalidConstruct {
                        line: orig.span.line,
                        msg: format!(
                            "Imported symbol '{}' is not declared in '{}'",
                            orig.name, import_decl.path
                        ),
                    });
                }
                if let Some(al) = alias {
                    if al.name != orig.name {
                        alias_map.insert(al.name.clone(), orig.name.clone());
                    }
                    if import_decl.is_reexport {
                        this_module_symbols.insert(al.name.clone());
                    }
                } else if import_decl.is_reexport {
                    this_module_symbols.insert(orig.name.clone());
                }
            }
        }
        ImportKind::Default { name, named } => {
            if import_decl.is_reexport {
                this_module_symbols.insert(name.name.clone());
            }
            for (orig, alias) in named {
                if validate_named && !dep_syms.is_empty() && !dep_syms.contains(&orig.name) {
                    return Err(LowerError::InvalidConstruct {
                        line: orig.span.line,
                        msg: format!(
                            "Imported symbol '{}' is not declared in '{}'",
                            orig.name, import_decl.path
                        ),
                    });
                }
                if let Some(al) = alias {
                    if al.name != orig.name {
                        alias_map.insert(al.name.clone(), orig.name.clone());
                    }
                    if import_decl.is_reexport {
                        this_module_symbols.insert(al.name.clone());
                    }
                } else if import_decl.is_reexport {
                    this_module_symbols.insert(orig.name.clone());
                }
            }
        }
    }
    Ok(())
}

fn rewrite_program_imports(
    prog: &Program,
    alias_map: &HashMap<String, String>,
    ns_map: &HashMap<String, HashSet<String>>,
) -> Program {
    let mut out = prog.clone();
    for decl in &mut out.declarations {
        match decl {
            Declaration::Struct(s) => {
                for field in &mut s.fields {
                    rewrite_type_expr(&mut field.ty, alias_map);
                }
            }
            Declaration::Enum(e) => {
                for variant in &mut e.variants {
                    for ty in &mut variant.fields {
                        rewrite_type_expr(ty, alias_map);
                    }
                }
            }
            Declaration::Const(c) => {
                if let Some(ref mut ty) = c.ty {
                    rewrite_type_expr(ty, alias_map);
                }
                let locals = HashSet::new();
                rewrite_expr(&mut c.value, alias_map, ns_map, &locals);
            }
            Declaration::Function(f) => {
                let mut locals = HashSet::new();
                for p in &mut f.params {
                    rewrite_type_expr(&mut p.ty, alias_map);
                    locals.insert(p.name.name.clone());
                }
                rewrite_type_expr(&mut f.ret_ty, alias_map);
                rewrite_block(&mut f.body, alias_map, ns_map, &locals);
            }
        }
    }
    out
}

fn rewrite_type_expr(ty: &mut TypeExpr, alias_map: &HashMap<String, String>) {
    match &mut ty.kind {
        TypeExprKind::Named(id) | TypeExprKind::Struct(id) => {
            if let Some(orig) = alias_map.get(&id.name) {
                id.name = orig.clone();
            }
        }
        TypeExprKind::Vec(inner) | TypeExprKind::Chan(inner) => {
            rewrite_type_expr(inner, alias_map);
        }
        TypeExprKind::Map(k, v) => {
            rewrite_type_expr(k, alias_map);
            rewrite_type_expr(v, alias_map);
        }
    }
    for arg in &mut ty.type_args {
        rewrite_type_expr(arg, alias_map);
    }
}

fn rewrite_block(
    block: &mut Block,
    alias_map: &HashMap<String, String>,
    ns_map: &HashMap<String, HashSet<String>>,
    outer_locals: &HashSet<String>,
) {
    let mut locals = outer_locals.clone();
    for stmt in &mut block.stmts {
        rewrite_stmt(stmt, alias_map, ns_map, &mut locals);
    }
    if let Some(ref mut tail) = block.tail_expr {
        rewrite_expr(tail, alias_map, ns_map, &locals);
    }
}

fn rewrite_stmt(
    stmt: &mut Stmt,
    alias_map: &HashMap<String, String>,
    ns_map: &HashMap<String, HashSet<String>>,
    locals: &mut HashSet<String>,
) {
    match &mut stmt.kind {
        StmtKind::Let {
            name,
            ty,
            init,
            destruct,
            ..
        } => {
            if let Some(t) = ty {
                rewrite_type_expr(t, alias_map);
            }
            rewrite_expr(init, alias_map, ns_map, locals);
            match destruct {
                Some(DestructPattern::Object(fields)) => {
                    for (f, alias) in fields {
                        let local_name = alias.as_ref().unwrap_or(f);
                        locals.insert(local_name.name.clone());
                    }
                }
                Some(DestructPattern::Array(elems)) => {
                    for e in elems {
                        locals.insert(e.name.clone());
                    }
                }
                None => {
                    locals.insert(name.name.clone());
                }
            }
        }
        StmtKind::Assign { target, value } => {
            rewrite_expr(target, alias_map, ns_map, locals);
            rewrite_expr(value, alias_map, ns_map, locals);
        }
        StmtKind::Return { value } => {
            if let Some(v) = value {
                rewrite_expr(v, alias_map, ns_map, locals);
            }
        }
        StmtKind::If {
            cond,
            then_block,
            else_block,
        } => {
            rewrite_expr(cond, alias_map, ns_map, locals);
            rewrite_block(then_block, alias_map, ns_map, locals);
            if let Some(eb) = else_block {
                rewrite_block(eb, alias_map, ns_map, locals);
            }
        }
        StmtKind::While { cond, body } => {
            rewrite_expr(cond, alias_map, ns_map, locals);
            rewrite_block(body, alias_map, ns_map, locals);
        }
        StmtKind::Syscall { payload, .. } => {
            rewrite_expr(payload, alias_map, ns_map, locals);
        }
        StmtKind::Expr(e) => {
            rewrite_expr(e, alias_map, ns_map, locals);
        }
        StmtKind::Block(b) => {
            rewrite_block(b, alias_map, ns_map, locals);
        }
        StmtKind::Break | StmtKind::Continue => {}
    }
}

fn rewrite_expr(
    expr: &mut Expr,
    alias_map: &HashMap<String, String>,
    ns_map: &HashMap<String, HashSet<String>>,
    locals: &HashSet<String>,
) {
    match &mut expr.kind {
        ExprKind::Variable(id) => {
            if !locals.contains(&id.name) {
                if let Some(orig) = alias_map.get(&id.name) {
                    id.name = orig.clone();
                }
            }
        }
        ExprKind::Call {
            callee,
            type_args,
            args,
        } => {
            if !locals.contains(&callee.name) {
                if let Some(orig) = alias_map.get(&callee.name) {
                    callee.name = orig.clone();
                }
            }
            for ta in type_args {
                rewrite_type_expr(ta, alias_map);
            }
            for a in args {
                rewrite_expr(a, alias_map, ns_map, locals);
            }
        }
        ExprKind::MethodCall {
            receiver,
            method,
            args,
        } => {
            for a in args.iter_mut() {
                rewrite_expr(a, alias_map, ns_map, locals);
            }
            let ns_hit = if let ExprKind::Variable(ns_id) = &receiver.kind {
                !locals.contains(&ns_id.name)
                    && ns_map
                        .get(&ns_id.name)
                        .is_some_and(|syms| syms.contains(&method.name))
            } else {
                false
            };
            if ns_hit {
                let callee = method.clone();
                let taken_args = std::mem::take(args);
                expr.kind = ExprKind::Call {
                    callee,
                    type_args: Vec::new(),
                    args: taken_args,
                };
            } else {
                rewrite_expr(receiver, alias_map, ns_map, locals);
            }
        }
        ExprKind::FieldAccess { target, field } => {
            let ns_hit = if let ExprKind::Variable(ns_id) = &target.kind {
                !locals.contains(&ns_id.name)
                    && ns_map
                        .get(&ns_id.name)
                        .is_some_and(|syms| syms.contains(&field.name))
            } else {
                false
            };
            if ns_hit {
                expr.kind = ExprKind::Variable(field.clone());
            } else {
                rewrite_expr(target, alias_map, ns_map, locals);
            }
        }
        ExprKind::StructLit {
            name,
            fields,
            spread,
        } => {
            if let Some(id) = name {
                if let Some(orig) = alias_map.get(&id.name) {
                    id.name = orig.clone();
                }
            }
            for f in fields {
                rewrite_expr(&mut f.value, alias_map, ns_map, locals);
            }
            if let Some(sp) = spread {
                rewrite_expr(sp, alias_map, ns_map, locals);
            }
        }
        ExprKind::EnumLit {
            enum_name,
            type_args,
            args,
            ..
        } => {
            if let Some(orig) = alias_map.get(&enum_name.name) {
                enum_name.name = orig.clone();
            }
            for ta in type_args {
                rewrite_type_expr(ta, alias_map);
            }
            for a in args {
                rewrite_expr(a, alias_map, ns_map, locals);
            }
        }
        ExprKind::Jsx {
            tag,
            props,
            children,
        } => {
            if let Some(orig) = alias_map.get(&tag.name) {
                tag.name = orig.clone();
            }
            for p in props {
                if let Some(ref mut v) = p.value {
                    rewrite_expr(v, alias_map, ns_map, locals);
                }
            }
            for ch in children {
                match ch {
                    JsxChild::Expr(e) | JsxChild::Element(e) => {
                        rewrite_expr(e, alias_map, ns_map, locals);
                    }
                    JsxChild::Text(_, _) => {}
                }
            }
        }
        ExprKind::TemplateStr(parts) => {
            for part in parts {
                if let TemplatePart::Expr(e) = part {
                    rewrite_expr(e, alias_map, ns_map, locals);
                }
            }
        }
        ExprKind::ArrayLit(items) => {
            for item in items {
                rewrite_expr(item, alias_map, ns_map, locals);
            }
        }
        ExprKind::Borrow(inner) | ExprKind::Unary { operand: inner, .. } => {
            rewrite_expr(inner, alias_map, ns_map, locals);
        }
        ExprKind::Binary { lhs, rhs, .. } => {
            rewrite_expr(lhs, alias_map, ns_map, locals);
            rewrite_expr(rhs, alias_map, ns_map, locals);
        }
        ExprKind::Syscall { payload, .. } => {
            rewrite_expr(payload, alias_map, ns_map, locals);
        }
        ExprKind::Index { target, index } => {
            rewrite_expr(target, alias_map, ns_map, locals);
            rewrite_expr(index, alias_map, ns_map, locals);
        }
        ExprKind::If {
            cond,
            then_branch,
            else_branch,
        } => {
            rewrite_expr(cond, alias_map, ns_map, locals);
            rewrite_block(then_branch, alias_map, ns_map, locals);
            rewrite_block(else_branch, alias_map, ns_map, locals);
        }
        ExprKind::Match { scrutinee, arms } => {
            rewrite_expr(scrutinee, alias_map, ns_map, locals);
            for arm in arms {
                let mut arm_locals = locals.clone();
                if let MatchPattern::Variant {
                    enum_name,
                    bindings,
                    ..
                } = &mut arm.pattern
                {
                    if let Some(en) = enum_name {
                        if let Some(orig) = alias_map.get(&en.name) {
                            en.name = orig.clone();
                        }
                    }
                    for b in bindings {
                        arm_locals.insert(b.name.clone());
                    }
                }
                rewrite_expr(&mut arm.body, alias_map, ns_map, &arm_locals);
            }
        }
        ExprKind::Block(b) => {
            rewrite_block(b, alias_map, ns_map, locals);
        }
        ExprKind::Arrow { params, body } => {
            let mut arrow_locals = locals.clone();
            for p in params {
                arrow_locals.insert(p.name.clone());
            }
            match body {
                ArrowBody::Expr(e) => rewrite_expr(e, alias_map, ns_map, &arrow_locals),
                ArrowBody::Block(b) => rewrite_block(b, alias_map, ns_map, &arrow_locals),
            }
        }
        ExprKind::Int(_)
        | ExprKind::Float(_)
        | ExprKind::Bool(_)
        | ExprKind::Str(_)
        | ExprKind::ColorHex(_) => {}
    }
}

fn normalize_lexical_path(path: &Path) -> PathBuf {
    use std::path::Component;
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

fn lookup_overlay<'a>(
    overlays: &'a HashMap<PathBuf, String>,
    canon: &Path,
    raw: &Path,
) -> Option<&'a String> {
    if overlays.is_empty() {
        return None;
    }
    if let Some(v) = overlays.get(canon) {
        return Some(v);
    }
    let norm_raw = normalize_lexical_path(raw);
    if let Some(v) = overlays.get(&norm_raw) {
        return Some(v);
    }
    let norm_canon = normalize_lexical_path(canon);
    overlays.get(&norm_canon)
}

fn resolve_import_path(
    rel_path: &str,
    base_dir: Option<&Path>,
    overlays: &HashMap<PathBuf, String>,
) -> Option<PathBuf> {
    let mut rel_variants = vec![rel_path.to_string()];
    if !rel_path.ends_with(".tsk")
        && !rel_path.ends_with(".knr")
        && !rel_path.ends_with(".ssa")
        && !rel_path.ends_with(".ssa.schema")
    {
        let trimmed = rel_path.trim_end_matches('/');
        rel_variants.push(format!("{trimmed}.tsk"));
        rel_variants.push(format!("{trimmed}.knr"));
        rel_variants.push(format!("{trimmed}/index.tsk"));
        rel_variants.push(format!("{trimmed}/index.knr"));
    }

    let mut candidates = Vec::new();
    for rv in &rel_variants {
        if let Some(base) = base_dir {
            candidates.push(base.join(rv));
        }
        candidates.push(PathBuf::from(rv));
    }

    if let Ok(cwd) = std::env::current_dir() {
        for rv in &rel_variants {
            candidates.push(cwd.join(rv));
            candidates.push(cwd.join("site/components").join(rv));
            candidates.push(cwd.join("site").join(rv));
            candidates.push(cwd.join("tests/fixtures/legacy_site/components").join(rv));
            candidates.push(cwd.join("tests/fixtures/legacy_site").join(rv));
            candidates.push(cwd.join("tests/fixtures").join(rv));
        }
        if let Some(parent) = cwd.parent() {
            for rv in &rel_variants {
                candidates.push(parent.join(rv));
                candidates.push(parent.join("site/components").join(rv));
                candidates.push(parent.join("site").join(rv));
                candidates.push(parent.join("tests/fixtures/legacy_site/components").join(rv));
                candidates.push(parent.join("tests/fixtures/legacy_site").join(rv));
                candidates.push(parent.join("tests/fixtures").join(rv));
            }
            if let Some(grandparent) = parent.parent() {
                for rv in &rel_variants {
                    candidates.push(grandparent.join(rv));
                    candidates.push(grandparent.join("site/components").join(rv));
                    candidates.push(grandparent.join("site").join(rv));
                    candidates.push(
                        grandparent
                            .join("tests/fixtures/legacy_site/components")
                            .join(rv),
                    );
                    candidates.push(grandparent.join("tests/fixtures/legacy_site").join(rv));
                    candidates.push(grandparent.join("tests/fixtures").join(rv));
                }
            }
        }
    }

    for c in candidates {
        let norm = normalize_lexical_path(&c);
        if overlays.contains_key(&norm) || overlays.contains_key(&c) {
            return Some(norm);
        }
        if let Ok(canon) = c.canonicalize() {
            if canon.is_file() {
                return Some(canon);
            }
        } else if c.is_file() {
            return Some(c);
        }
    }

    // Package / workspace dependency import resolution via `knr.toml`
    if !rel_path.starts_with("std/")
        && !rel_path.starts_with("./")
        && !rel_path.starts_with("../")
    {
        if let Some(base) = base_dir {
            if let Some(pkg_path) =
                crate::manifest::resolve_package_import_with_overlays(rel_path, base, overlays)
            {
                return Some(pkg_path);
            }
        } else if let Ok(cwd) = std::env::current_dir() {
            if let Some(pkg_path) =
                crate::manifest::resolve_package_import_with_overlays(rel_path, &cwd, overlays)
            {
                return Some(pkg_path);
            }
        }
    }

    None
}

#[cfg(test)]
mod cross_import_tests {
    use super::*;

    #[test]
    fn cross_file_aliases_namespaces_barrels_and_diagnostics() {
        let tmp = std::env::temp_dir().join(format!(
            "knr_core_cross_imports_{}",
            std::process::id()
        ));
        let comp_dir = tmp.join("components");
        fs::create_dir_all(&comp_dir).unwrap();

        fs::write(
            tmp.join("state.tsk"),
            concat!(
                "export type State = {\n",
                "    count: i64;\n",
                "};\n",
                "export const STEP: i64 = 5;\n",
                "export function init(): State {\n",
                "    return { count: 10 };\n",
                "}\n",
                "export function bump(s: State): State {\n",
                "    return { count: s.count + STEP };\n",
                "}\n",
            ),
        )
        .unwrap();

        fs::write(
            comp_dir.join("badge.tsk"),
            concat!(
                "import { Text } from \"std/ui\";\n",
                "import { State } from \"../state\";\n",
                "export function CounterBadge(s: readonly State): Widget {\n",
                "    return <Text>{`Count: ${s.count}`}</Text>;\n",
                "}\n",
            ),
        )
        .unwrap();

        // Barrel file (`components/index.tsk`) using `export { ... } from "..."`
        let barrel_src = "export { CounterBadge } from \"./badge.tsk\";\n";
        fs::write(comp_dir.join("index.tsk"), barrel_src).unwrap();
        assert_eq!(crate::format_tsk(barrel_src).unwrap(), barrel_src);

        // App using: extension-less import, `as` aliases, `* as ns` namespace, and barrel directory import!
        let app_path = tmp.join("app.tsk");
        fs::write(
            &app_path,
            concat!(
                "import { Column } from \"std/ui\";\n",
                "import { State as AppState, init as makeState } from \"./state\";\n",
                "import * as st from \"./state.tsk\";\n",
                "import { CounterBadge } from \"./components\";\n",
                "export function view(s: readonly AppState): Widget {\n",
                "    return <Column><CounterBadge s={s} /></Column>;\n",
                "}\n",
                "export function main(): i64 {\n",
                "    const s0: AppState = makeState();\n",
                "    const s1 = st.bump(s0);\n",
                "    return s1.count + st.STEP;\n",
                "}\n",
            ),
        )
        .unwrap();

        let module = lower_program_from_file(&app_path).expect("alias + namespace + barrel lowers cleanly");
        assert!(module.get_function("main").is_some());
        assert!(module.get_function("CounterBadge").is_some());

        // Missing local file reports crisp `File not found` error
        let missing_path = tmp.join("missing_import.tsk");
        fs::write(
            &missing_path,
            "import { Nope } from \"./does_not_exist.tsk\";\nexport function f(): i64 { return 1; }\n",
        )
        .unwrap();
        let err_missing = lower_program_from_file(&missing_path).unwrap_err();
        assert!(
            err_missing.to_string().contains("File not found"),
            "Expected File not found, got: {err_missing}"
        );

        // Unknown symbol in named import reports crisp error
        let bad_sym_path = tmp.join("bad_sym.tsk");
        fs::write(
            &bad_sym_path,
            "import { State, non_existent_symbol } from \"./state.tsk\";\nexport function f(): i64 { return 1; }\n",
        )
        .unwrap();
        let err_sym = lower_program_from_file(&bad_sym_path).unwrap_err();
        assert!(
            err_sym.to_string().contains("non_existent_symbol"),
            "Expected missing symbol diagnostic, got: {err_sym}"
        );

        // Live in-memory overlays override disk content without writing to disk
        let overlay_prog = crate::syntax::parse_tsx(
            "import { live_only } from \"./state.tsk\";\nexport function f(): i64 { return live_only(); }\n",
        )
        .unwrap();
        let mut overlays = HashMap::new();
        overlays.insert(
            tmp.join("state.tsk").canonicalize().unwrap(),
            "export function live_only(): i64 { return 99; }\n".to_string(),
        );
        let (ov_mod, _) =
            lower_program_with_overlays_and_source_map(&overlay_prog, &[], Some(tmp.clone()), &overlays)
                .expect("overlay lowering succeeds");
        assert!(ov_mod.get_function("live_only").is_some());

        let _ = fs::remove_dir_all(&tmp);
    }
}
