use std::collections::{BTreeMap, HashMap, HashSet};

use crate::ast::{BasicBlock, BlockId, Effect, Instruction, Op, RegId, Schema, Terminator, Type};
use crate::lower::error::LowerError;
use crate::syntax::Span;

/// Metadata for a single variant inside an `enum` lowered to a tagged struct schema.
#[derive(Debug, Clone)]
pub struct EnumVariantMeta {
    pub name: String,
    pub tag: i64,
    pub field_slots: Vec<(u32, Type)>,
}

/// Metadata for an `enum` definition lowered to a tagged `Schema`.
#[derive(Debug, Clone)]
pub struct EnumMeta {
    pub name: String,
    pub variants: Vec<EnumVariantMeta>,
}

impl EnumMeta {
    pub fn get_variant(&self, name: &str) -> Option<&EnumVariantMeta> {
        self.variants.iter().find(|v| v.name == name)
    }
}

/// Function signature tracked during lowering (including second-class borrow flags & JSX param names).
#[derive(Debug, Clone)]
pub struct FnSig {
    pub param_names: Vec<String>,
    pub param_types: Vec<Type>,
    pub param_borrows: Vec<bool>,
    pub ret_ty: Type,
    pub effects: Vec<Effect>,
    pub is_shader: bool,
}

impl FnSig {
    pub fn is_param_borrowed(&self, idx: usize) -> bool {
        self.param_borrows.get(idx).copied().unwrap_or(false)
    }
}

/// Active loop context for lowering `break` and `continue` with SSA block arguments.
#[derive(Debug, Clone)]
pub struct LoopContext {
    pub header_bb: BlockId,
    pub exit_bb: BlockId,
    pub loop_vars: Vec<String>,
}

/// Active arrow-function block return context (used by `.map(...)` / `.filter(...)` block bodies).
#[derive(Debug, Clone)]
pub struct ArrowReturnCtx {
    pub merge_bb: BlockId,
    pub expected_ty: Option<Type>,
    pub inferred_ty: Option<Type>,
    pub pre_linear_regs: HashSet<RegId>,
    pub reached_merge: bool,
}

/// Source map tracking lowered registers and spans back to source code.
/// Registers are indexed by (function_name, RegId) to prevent cross-function collisions.
#[derive(Debug, Clone, Default)]
pub struct SourceMap {
    pub reg_to_var: HashMap<(String, RegId), String>,
    pub reg_to_span: HashMap<(String, RegId), Span>,
    pub line_spans: HashMap<usize, Span>,
}

impl SourceMap {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record_var(&mut self, func: &str, reg: RegId, name: impl Into<String>, span: Span) {
        let name_str = name.into();
        self.reg_to_var.insert((func.to_string(), reg), name_str);
        self.reg_to_span.insert((func.to_string(), reg), span);
        self.line_spans.insert(span.line, span);
    }

    pub fn record_span(&mut self, func: &str, reg: RegId, span: Span) {
        self.reg_to_span.insert((func.to_string(), reg), span);
        self.line_spans.insert(span.line, span);
    }

    pub fn get_var(&self, func: &str, reg: RegId) -> Option<&str> {
        self.reg_to_var.get(&(func.to_string(), reg)).map(|s| s.as_str())
    }

    pub fn get_span(&self, func: &str, reg: RegId) -> Option<&Span> {
        self.reg_to_span.get(&(func.to_string(), reg))
    }

    pub fn get_line_span(&self, line: usize) -> Option<&Span> {
        self.line_spans.get(&line)
    }
}

#[derive(Debug, Clone)]
pub struct VarBinding {
    pub reg: RegId,
    pub ty: Type,
    pub is_linear: bool,
    pub is_local_alloc: bool,
    pub is_borrowed: bool,
    pub line: usize,
}

#[derive(Debug, Clone, Default)]
pub struct Scope {
    pub variables: HashMap<String, VarBinding>,
    pub declared_linear_regs: Vec<RegId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinearState {
    Alive {
        ty: Type,
        line: usize,
        local_alloc: bool,
        borrowed: bool,
    },
    Moved {
        consumed_at: usize,
    },
}

#[derive(Debug, Clone, Default)]
pub struct LinearTracker {
    pub states: BTreeMap<RegId, LinearState>,
}

impl LinearTracker {
    pub fn mark_alive(&mut self, reg: RegId, ty: Type, local_alloc: bool, line: usize) {
        if ty.is_linear() {
            self.states.insert(
                reg,
                LinearState::Alive {
                    ty,
                    line,
                    local_alloc,
                    borrowed: false,
                },
            );
        }
    }

    pub fn mark_borrowed(&mut self, reg: RegId, ty: Type, line: usize) {
        if ty.is_linear() {
            self.states.insert(
                reg,
                LinearState::Alive {
                    ty,
                    line,
                    local_alloc: false,
                    borrowed: true,
                },
            );
        }
    }

    pub fn mark_moved(&mut self, reg: RegId, line: usize) -> Result<(), LowerError> {
        if let Some(state) = self.states.get_mut(&reg) {
            match state {
                LinearState::Moved { consumed_at } => {
                    return Err(LowerError::UseAfterMove {
                        line,
                        name: format!("%{reg}"),
                        moved_line: *consumed_at,
                    });
                }
                LinearState::Alive { borrowed: true, .. } => {
                    return Err(LowerError::InvalidConstruct {
                        line,
                        msg: format!(
                            "Cannot move or consume borrowed parameter %{reg}: second-class borrows (&T) are read-only and cannot escape"
                        ),
                    });
                }
                LinearState::Alive { .. } => {
                    *state = LinearState::Moved { consumed_at: line };
                }
            }
        }
        Ok(())
    }

    pub fn is_alive(&self, reg: RegId) -> bool {
        matches!(self.states.get(&reg), Some(LinearState::Alive { .. }))
    }

    pub fn is_borrowed(&self, reg: RegId) -> bool {
        matches!(
            self.states.get(&reg),
            Some(LinearState::Alive { borrowed: true, .. })
        )
    }

    pub fn snapshot(&self) -> LinearTracker {
        self.clone()
    }

    pub fn restore(&mut self, snapshot: LinearTracker) {
        *self = snapshot;
    }

    /// Reconciles linear states across two joining control-flow branches (then and else) at a merge point.
    pub fn merge_branches(
        &mut self,
        then_tracker: &LinearTracker,
        else_tracker: &LinearTracker,
        source_map: &SourceMap,
        fn_name: &str,
        line: usize,
    ) -> Result<(), LowerError> {
        let mut merged = BTreeMap::new();

        for (&reg, base_state) in &self.states {
            let then_st = then_tracker.states.get(&reg).unwrap_or(base_state);
            let else_st = else_tracker.states.get(&reg).unwrap_or(base_state);

            match (then_st, else_st) {
                (
                    LinearState::Alive {
                        ty,
                        local_alloc,
                        borrowed,
                        ..
                    },
                    LinearState::Alive { .. },
                ) => {
                    merged.insert(
                        reg,
                        LinearState::Alive {
                            ty: ty.clone(),
                            line,
                            local_alloc: *local_alloc,
                            borrowed: *borrowed,
                        },
                    );
                }
                (
                    LinearState::Moved { consumed_at: c1 },
                    LinearState::Moved { .. },
                ) => {
                    merged.insert(reg, LinearState::Moved { consumed_at: *c1 });
                }
                (LinearState::Moved { consumed_at, .. }, LinearState::Alive { .. })
                | (LinearState::Alive { .. }, LinearState::Moved { consumed_at, .. }) => {
                    let var_name = source_map
                        .get_var(fn_name, reg)
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| format!("%{reg}"));
                    return Err(LowerError::InvalidConstruct {
                        line,
                        msg: format!(
                            "Linear resource '{var_name}' is consumed in one branch (line {consumed_at}) but not in the other branch across joining if/else",
                        ),
                    });
                }
            }
        }

        self.states = merged;
        Ok(())
    }

    /// Returns unreturned local linear registers that must be auto-dropped.
    /// Declarative UI structures (Struct, Vec, Map) are safely auto-dropped;
    /// raw unmanaged resources (Buf, Chan, or Structs containing Buf/Chan) are not.
    pub fn get_alive_local_allocations(&self, schemas: &HashMap<String, Schema>) -> Vec<(RegId, Type)> {
        self.states
            .iter()
            .filter_map(|(&r, s)| {
                if let LinearState::Alive {
                    ty,
                    local_alloc: true,
                    borrowed: false,
                    ..
                } = s
                {
                    if is_safe_autodrop_type(ty, schemas) {
                        return Some((r, ty.clone()));
                    }
                }
                None
            })
            .collect()
    }
}

pub(crate) fn is_safe_autodrop_type(ty: &Type, schemas: &HashMap<String, Schema>) -> bool {
    if !ty.is_linear() {
        return false;
    }
    match ty {
        Type::Buf | Type::Chan(_) | Type::Str => false,
        Type::Struct(name) => {
            if let Some(schema) = schemas.get(name) {
                !schema.fields.iter().any(|f| field_contains_unmanaged(&f.ty, schemas))
            } else {
                true
            }
        }
        Type::Vec(inner) => !field_contains_unmanaged(inner, schemas),
        Type::Map(k, v) => {
            !field_contains_unmanaged(k, schemas) && !field_contains_unmanaged(v, schemas)
        }
        _ => false,
    }
}

fn field_contains_unmanaged(ty: &Type, schemas: &HashMap<String, Schema>) -> bool {
    let mut visited = std::collections::HashSet::new();
    field_contains_unmanaged_inner(ty, schemas, &mut visited)
}

fn field_contains_unmanaged_inner<'a>(
    ty: &'a Type,
    schemas: &'a HashMap<String, Schema>,
    visited: &mut std::collections::HashSet<&'a str>,
) -> bool {
    match ty {
        Type::Buf | Type::Chan(_) => true,
        Type::Struct(name) => {
            if !visited.insert(name.as_str()) {
                return false;
            }
            let res = if let Some(schema) = schemas.get(name) {
                schema
                    .fields
                    .iter()
                    .any(|f| field_contains_unmanaged_inner(&f.ty, schemas, visited))
            } else {
                false
            };
            visited.remove(name.as_str());
            res
        }
        Type::Vec(inner) => field_contains_unmanaged_inner(inner, schemas, visited),
        Type::Map(k, v) => {
            field_contains_unmanaged_inner(k, schemas, visited)
                || field_contains_unmanaged_inner(v, schemas, visited)
        }
        _ => false,
    }
}

pub struct FnLoweringContext<'a> {
    pub fn_name: String,
    pub fn_ret_ty: Type,
    pub fn_effects: Vec<Effect>,
    pub schemas: &'a HashMap<String, Schema>,
    pub enums: &'a HashMap<String, EnumMeta>,
    pub fn_sigs: &'a HashMap<String, FnSig>,
    pub const_exprs: &'a HashMap<String, crate::syntax::ast::Expr>,
    pub next_reg: RegId,
    pub next_block: BlockId,
    pub scopes: Vec<Scope>,
    pub linear_tracker: LinearTracker,
    pub source_map: SourceMap,
    pub current_block: BasicBlock,
    pub completed_blocks: Vec<BasicBlock>,
    pub is_terminated: bool,
    pub loop_stack: Vec<LoopContext>,
    pub arrow_ret_stack: Vec<ArrowReturnCtx>,
}

impl<'a> FnLoweringContext<'a> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        fn_name: String,
        fn_ret_ty: Type,
        fn_effects: Vec<Effect>,
        param_count: usize,
        schemas: &'a HashMap<String, Schema>,
        enums: &'a HashMap<String, EnumMeta>,
        fn_sigs: &'a HashMap<String, FnSig>,
        const_exprs: &'a HashMap<String, crate::syntax::ast::Expr>,
    ) -> Self {
        Self {
            fn_name,
            fn_ret_ty,
            fn_effects,
            schemas,
            enums,
            fn_sigs,
            const_exprs,
            next_reg: param_count as RegId,
            next_block: 1,
            scopes: vec![Scope::default()],
            linear_tracker: LinearTracker::default(),
            source_map: SourceMap::new(),
            current_block: BasicBlock::new(0, Terminator::Br(0)),
            completed_blocks: Vec::new(),
            is_terminated: false,
            loop_stack: Vec::new(),
            arrow_ret_stack: Vec::new(),
        }
    }

    /// Returns true if `reg` is currently bound to any live variable in any active scope.
    pub fn is_reg_bound_to_var(&self, reg: RegId) -> bool {
        self.scopes
            .iter()
            .any(|s| s.variables.values().any(|b| b.reg == reg))
    }

    pub fn alloc_reg(&mut self) -> RegId {
        let r = self.next_reg;
        self.next_reg += 1;
        r
    }

    pub fn alloc_block(&mut self) -> BlockId {
        let b = self.next_block;
        self.next_block += 1;
        b
    }

    pub fn start_block(&mut self, block_id: BlockId) {
        self.current_block = BasicBlock::new(block_id, Terminator::Br(0));
        self.is_terminated = false;
    }

    pub fn start_block_with_params(&mut self, block_id: BlockId, params: Vec<(RegId, Type)>) {
        self.current_block = BasicBlock::with_params(block_id, params, Terminator::Br(0));
        self.is_terminated = false;
    }

    pub fn enter_scope(&mut self) {
        self.scopes.push(Scope::default());
    }

    pub fn leave_scope(&mut self) {
        self.scopes.pop();
    }

    pub fn emit(&mut self, inst: Instruction) {
        if !self.is_terminated {
            self.current_block.push(inst);
        }
    }

    pub fn record_span(&mut self, reg: RegId, span: Span) {
        self.source_map.record_span(&self.fn_name, reg, span);
    }

    pub fn drop_scope_linear_allocations(
        &mut self,
        line: usize,
        keep_reg: Option<RegId>,
    ) -> Result<(), LowerError> {
        if self.is_terminated {
            return Ok(());
        }

        if let Some(scope) = self.scopes.last() {
            let regs_to_drop = scope.declared_linear_regs.clone();
            for reg in regs_to_drop.into_iter().rev() {
                if Some(reg) == keep_reg {
                    continue;
                }
                if let Some(LinearState::Alive {
                    ty,
                    local_alloc: true,
                    borrowed: false,
                    ..
                }) = self.linear_tracker.states.get(&reg).cloned()
                {
                    if is_safe_autodrop_type(&ty, self.schemas) {
                        self.emit(Instruction::new(None, Type::Void, Op::Free(reg), line));
                        self.linear_tracker.mark_moved(reg, line)?;
                    }
                }
            }
        }
        Ok(())
    }

    pub fn bind_var(
        &mut self,
        name: &str,
        reg: RegId,
        ty: Type,
        is_local_alloc: bool,
        span: Span,
    ) {
        let is_linear = ty.is_linear();
        if is_linear {
            self.linear_tracker
                .mark_alive(reg, ty.clone(), is_local_alloc, span.line);
        }
        self.source_map.record_var(&self.fn_name, reg, name, span);
        if let Some(scope) = self.scopes.last_mut() {
            if is_linear && is_local_alloc {
                scope.declared_linear_regs.push(reg);
            }
            scope.variables.insert(
                name.to_string(),
                VarBinding {
                    reg,
                    ty,
                    is_linear,
                    is_local_alloc,
                    is_borrowed: false,
                    line: span.line,
                },
            );
        }
    }

    pub fn bind_borrowed_param(&mut self, name: &str, reg: RegId, ty: Type, span: Span) {
        let is_linear = ty.is_linear();
        if is_linear {
            self.linear_tracker
                .mark_borrowed(reg, ty.clone(), span.line);
        }
        self.source_map.record_var(&self.fn_name, reg, name, span);
        if let Some(scope) = self.scopes.last_mut() {
            scope.variables.insert(
                name.to_string(),
                VarBinding {
                    reg,
                    ty,
                    is_linear,
                    is_local_alloc: false,
                    is_borrowed: true,
                    line: span.line,
                },
            );
        }
    }

    /// Updates an existing variable binding in the innermost enclosing scope that defines it.
    pub fn update_var_binding(
        &mut self,
        name: &str,
        new_reg: RegId,
        new_ty: Type,
        is_local_alloc: bool,
        span: Span,
    ) -> Result<(), LowerError> {
        let is_linear = new_ty.is_linear();
        if is_linear {
            self.linear_tracker
                .mark_alive(new_reg, new_ty.clone(), is_local_alloc, span.line);
        }
        self.source_map
            .record_var(&self.fn_name, new_reg, name, span);
        for scope in self.scopes.iter_mut().rev() {
            if let Some(binding) = scope.variables.get_mut(name) {
                binding.reg = new_reg;
                binding.ty = new_ty;
                binding.is_linear = is_linear;
                binding.is_local_alloc = is_local_alloc;
                binding.is_borrowed = false;
                binding.line = span.line;
                if is_linear && is_local_alloc {
                    scope.declared_linear_regs.push(new_reg);
                }
                return Ok(());
            }
        }
        Err(LowerError::UndefinedVariable {
            line: span.line,
            name: name.to_string(),
        })
    }

    pub fn lookup_var_binding(&self, name: &str) -> Option<&VarBinding> {
        for scope in self.scopes.iter().rev() {
            if let Some(binding) = scope.variables.get(name) {
                return Some(binding);
            }
        }
        None
    }

    pub fn lookup_var(&self, name: &str, line: usize) -> Result<(RegId, Type), LowerError> {
        for scope in self.scopes.iter().rev() {
            if let Some(binding) = scope.variables.get(name) {
                if binding.is_linear && !self.linear_tracker.is_alive(binding.reg) {
                    let moved_line = match self.linear_tracker.states.get(&binding.reg) {
                        Some(LinearState::Moved { consumed_at }) => *consumed_at,
                        _ => line,
                    };
                    return Err(LowerError::UseAfterMove {
                        line,
                        name: name.to_string(),
                        moved_line,
                    });
                }
                return Ok((binding.reg, binding.ty.clone()));
            }
        }
        Err(LowerError::UndefinedVariable {
            line,
            name: name.to_string(),
        })
    }

    pub fn mark_consumed(&mut self, reg: RegId, line: usize) -> Result<(), LowerError> {
        self.linear_tracker.mark_moved(reg, line)
    }

    pub fn finalize_current_block(&mut self) {
        let mut blk = BasicBlock::with_params(
            self.current_block.id,
            std::mem::take(&mut self.current_block.params),
            self.current_block.terminator.clone(),
        );
        std::mem::swap(&mut blk.instructions, &mut self.current_block.instructions);
        self.completed_blocks.push(blk);
    }
}
