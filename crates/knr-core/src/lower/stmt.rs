use std::collections::HashSet;

use crate::ast::{Effect, Instruction, Op, RegId, Terminator, Type};
use crate::lower::context::{FnLoweringContext, LoopContext};
use crate::lower::error::LowerError;
use crate::syntax::ast::{Block, Expr, ExprKind, Span, Stmt, StmtKind};

impl<'a> FnLoweringContext<'a> {
    /// Lowers a top-level function body block (tail expression returns from the function).
    pub fn lower_fn_body(&mut self, block: &Block) -> Result<(), LowerError> {
        self.enter_scope();
        for stmt in &block.stmts {
            if self.is_terminated {
                break;
            }
            self.lower_stmt(stmt)?;
        }

        if !self.is_terminated {
            if let Some(ref tail_expr) = block.tail_expr {
                if self.fn_ret_ty != Type::Void {
                    self.lower_return(Some(tail_expr), tail_expr.span.line)?;
                } else {
                    self.lower_expr(tail_expr)?;
                }
            }
        }

        if !self.is_terminated {
            self.drop_scope_linear_allocations(block.span.line, None)?;
        }
        self.leave_scope();
        Ok(())
    }

    /// Lowers a statement block (does NOT implicitly return on tail_expr).
    pub fn lower_block(&mut self, block: &Block) -> Result<(), LowerError> {
        self.enter_scope();
        for stmt in &block.stmts {
            if self.is_terminated {
                break;
            }
            self.lower_stmt(stmt)?;
        }

        if !self.is_terminated {
            if let Some(ref tail_expr) = block.tail_expr {
                self.lower_expr(tail_expr)?;
            }
        }

        if !self.is_terminated {
            self.drop_scope_linear_allocations(block.span.line, None)?;
        }
        self.leave_scope();
        Ok(())
    }

    /// Lowers a value-producing block (for `if` expressions, `match` arms, `{ ... }` expressions).
    /// Returns `Ok(None)` if the block terminated early via `return`/`break`/`continue`.
    pub fn lower_block_value(
        &mut self,
        block: &Block,
        expected_ty: Option<&Type>,
    ) -> Result<Option<(RegId, Type)>, LowerError> {
        self.enter_scope();
        for stmt in &block.stmts {
            if self.is_terminated {
                break;
            }
            self.lower_stmt(stmt)?;
        }

        let res = if !self.is_terminated {
            if let Some(ref tail_expr) = block.tail_expr {
                let (r, ty) = self.lower_expr_with_expected(tail_expr, expected_ty)?;
                self.drop_scope_linear_allocations(block.span.line, Some(r))?;
                Some((r, ty))
            } else {
                self.drop_scope_linear_allocations(block.span.line, None)?;
                Some((0, Type::Void))
            }
        } else {
            None
        };

        self.leave_scope();
        Ok(res)
    }

    pub fn lower_stmt(&mut self, stmt: &Stmt) -> Result<(), LowerError> {
        let line = stmt.span.line;
        match &stmt.kind {
            StmtKind::Let { name, ty, init } => {
                let expected_ty = if let Some(t_expr) = ty {
                    Some(
                        t_expr
                            .to_core_type()
                            .map_err(|msg| LowerError::InvalidConstruct { line, msg })?,
                    )
                } else {
                    None
                };

                let (r_val, ty_val) = self.lower_expr_with_expected(init, expected_ty.as_ref())?;

                if let Some(ref exp) = expected_ty {
                    if &ty_val != exp {
                        return Err(LowerError::TypeMismatch {
                            line,
                            expected: exp.to_string(),
                            actual: ty_val.to_string(),
                        });
                    }
                }

                self.bind_var(&name.name, r_val, ty_val, true, name.span);
                Ok(())
            }
            StmtKind::Return { value } => self.lower_return(value.as_ref(), line),
            StmtKind::If {
                cond,
                then_block,
                else_block,
            } => self.lower_if(cond, then_block, else_block.as_ref(), line),
            StmtKind::While { cond, body } => self.lower_while(cond, body, line),
            StmtKind::Break => self.lower_break(line),
            StmtKind::Continue => self.lower_continue(line),
            StmtKind::Syscall {
                effect,
                op,
                payload,
            } => self.lower_syscall_stmt(&effect.name, op, payload, line),
            StmtKind::Assign { target, value } => {
                if let ExprKind::Variable(ref ident) = target.kind {
                    let (_, existing_ty) = self.lookup_var(&ident.name, line)?;
                    let (r_val, ty_val) =
                        self.lower_expr_with_expected(value, Some(&existing_ty))?;
                    if ty_val != existing_ty {
                        return Err(LowerError::TypeMismatch {
                            line,
                            expected: existing_ty.to_string(),
                            actual: ty_val.to_string(),
                        });
                    }
                    self.update_var_binding(&ident.name, r_val, ty_val, true, ident.span)?;
                    Ok(())
                } else {
                    Err(LowerError::InvalidConstruct {
                        line,
                        msg: "Assignment target must be a variable".to_string(),
                    })
                }
            }
            StmtKind::Expr(expr) => {
                self.lower_expr(expr)?;
                Ok(())
            }
            StmtKind::Block(blk) => self.lower_block(blk),
        }
    }

    pub fn lower_return(
        &mut self,
        val_expr: Option<&Expr>,
        line: usize,
    ) -> Result<(), LowerError> {
        if self.is_terminated {
            return Ok(());
        }

        let expected_ret_ty = self.fn_ret_ty.clone();
        let ret_reg = if let Some(expr) = val_expr {
            let (r, ty) = self.lower_expr_with_expected(expr, Some(&expected_ret_ty))?;
            if ty != expected_ret_ty {
                return Err(LowerError::TypeMismatch {
                    line,
                    expected: expected_ret_ty.to_string(),
                    actual: ty.to_string(),
                });
            }
            if self.linear_tracker.is_borrowed(r) {
                return Err(LowerError::InvalidConstruct {
                    line,
                    msg: format!(
                        "Cannot return borrowed parameter from function '{}'",
                        self.fn_name
                    ),
                });
            }
            Some(r)
        } else {
            if expected_ret_ty != Type::Void {
                return Err(LowerError::TypeMismatch {
                    line,
                    expected: expected_ret_ty.to_string(),
                    actual: Type::Void.to_string(),
                });
            }
            None
        };

        // Synthesize auto-drop Op::Free for all unreturned local linear allocations
        let leakables = self.linear_tracker.get_alive_local_allocations(self.schemas);
        for (reg, _) in leakables {
            if Some(reg) != ret_reg {
                self.emit(Instruction::new(None, Type::Void, Op::Free(reg), line));
                self.linear_tracker.mark_moved(reg, line)?;
            }
        }

        self.current_block.terminator = Terminator::Ret(ret_reg);
        self.finalize_current_block();
        self.is_terminated = true;
        Ok(())
    }

    fn lower_break(&mut self, line: usize) -> Result<(), LowerError> {
        let loop_ctx = self.loop_stack.last().cloned().ok_or_else(|| {
            LowerError::InvalidConstruct {
                line,
                msg: "'break' used outside of a loop".to_string(),
            }
        })?;
        let mut args = Vec::with_capacity(loop_ctx.loop_vars.len());
        for var_name in &loop_ctx.loop_vars {
            let (r, _) = self.lookup_var(var_name, line)?;
            args.push(r);
        }
        self.current_block.terminator = if args.is_empty() {
            Terminator::Br(loop_ctx.exit_bb)
        } else {
            Terminator::BrArgs {
                target: loop_ctx.exit_bb,
                args,
            }
        };
        self.finalize_current_block();
        self.is_terminated = true;
        Ok(())
    }

    fn lower_continue(&mut self, line: usize) -> Result<(), LowerError> {
        let loop_ctx = self.loop_stack.last().cloned().ok_or_else(|| {
            LowerError::InvalidConstruct {
                line,
                msg: "'continue' used outside of a loop".to_string(),
            }
        })?;
        let mut args = Vec::with_capacity(loop_ctx.loop_vars.len());
        for var_name in &loop_ctx.loop_vars {
            let (r, _) = self.lookup_var(var_name, line)?;
            args.push(r);
        }
        self.current_block.terminator = if args.is_empty() {
            Terminator::Br(loop_ctx.header_bb)
        } else {
            Terminator::BrArgs {
                target: loop_ctx.header_bb,
                args,
            }
        };
        self.finalize_current_block();
        self.is_terminated = true;
        Ok(())
    }

    fn lower_if(
        &mut self,
        cond_expr: &Expr,
        then_block: &Block,
        else_block: Option<&Block>,
        line: usize,
    ) -> Result<(), LowerError> {
        let (cond_reg, cond_ty) = self.lower_expr(cond_expr)?;
        if cond_ty != Type::Bool {
            return Err(LowerError::TypeMismatch {
                line,
                expected: "bool".to_string(),
                actual: cond_ty.to_string(),
            });
        }

        // Collect outer variables mutated in either branch so we can merge them via SSA block params
        let mut mutated_names = Vec::new();
        collect_assigned_vars_in_block(then_block, &HashSet::new(), &mut mutated_names);
        if let Some(eb) = else_block {
            collect_assigned_vars_in_block(eb, &HashSet::new(), &mut mutated_names);
        }
        let mut_vars: Vec<(String, RegId, Type, bool, Span)> = mutated_names
            .into_iter()
            .filter_map(|name| {
                self.lookup_var_binding(&name).map(|b| {
                    (
                        name,
                        b.reg,
                        b.ty.clone(),
                        b.is_local_alloc,
                        Span::new(0, 0, b.line, 1),
                    )
                })
            })
            .collect();

        let then_bb = self.alloc_block();
        let merge_bb = self.alloc_block();
        let has_else = else_block.is_some();
        let else_bb = if has_else {
            self.alloc_block()
        } else {
            merge_bb
        };

        let pre_if_args: Vec<RegId> = mut_vars.iter().map(|(_, r, _, _, _)| *r).collect();

        if !has_else && !pre_if_args.is_empty() {
            self.current_block.terminator = Terminator::BrIfArgs {
                cond: cond_reg,
                then_blk: then_bb,
                then_args: Vec::new(),
                else_blk: merge_bb,
                else_args: pre_if_args.clone(),
            };
        } else {
            self.current_block.terminator = Terminator::BrIf {
                cond: cond_reg,
                then_blk: then_bb,
                else_blk: else_bb,
            };
        }
        self.finalize_current_block();

        let scopes_snapshot = self.scopes.clone();
        let tracker_snapshot = self.linear_tracker.snapshot();

        // Lower Then branch
        self.start_block(then_bb);
        self.lower_block(then_block)?;
        let then_terminates = self.is_terminated;
        let then_scopes = self.scopes.clone();
        let then_tracker = self.linear_tracker.snapshot();
        let mut then_branch_args = Vec::new();
        if !then_terminates {
            for (name, _, _, _, _) in &mut_vars {
                let (r, _) = self.lookup_var(name, line)?;
                then_branch_args.push(r);
            }
            self.current_block.terminator = if then_branch_args.is_empty() {
                Terminator::Br(merge_bb)
            } else {
                Terminator::BrArgs {
                    target: merge_bb,
                    args: then_branch_args.clone(),
                }
            };
            self.finalize_current_block();
        }

        // Restore pre-if state for Else branch
        self.scopes = scopes_snapshot.clone();
        self.linear_tracker.restore(tracker_snapshot.clone());

        let (else_terminates, else_scopes, else_tracker) = if let Some(else_blk) = else_block {
            self.start_block(else_bb);
            self.lower_block(else_blk)?;
            let terminates = self.is_terminated;
            let es = self.scopes.clone();
            let et = self.linear_tracker.snapshot();
            if !terminates {
                let mut else_branch_args = Vec::new();
                for (name, _, _, _, _) in &mut_vars {
                    let (r, _) = self.lookup_var(name, line)?;
                    else_branch_args.push(r);
                }
                self.current_block.terminator = if else_branch_args.is_empty() {
                    Terminator::Br(merge_bb)
                } else {
                    Terminator::BrArgs {
                        target: merge_bb,
                        args: else_branch_args,
                    }
                };
                self.finalize_current_block();
            }
            (terminates, es, et)
        } else {
            (false, scopes_snapshot.clone(), tracker_snapshot.clone())
        };

        // Reconcile at merge_bb
        if !then_terminates || !else_terminates {
            if !then_terminates && !else_terminates {
                let mut merge_params = Vec::new();
                for (_, _, ty, _, _) in &mut_vars {
                    let pr = self.alloc_reg();
                    merge_params.push((pr, ty.clone()));
                }
                self.start_block_with_params(merge_bb, merge_params.clone());
                self.scopes = scopes_snapshot;
                self.linear_tracker.restore(tracker_snapshot);

                // For non-mutated linear vars, ensure branch agreement
                let mut_reg_set: HashSet<RegId> =
                    mut_vars.iter().map(|(_, r, _, _, _)| *r).collect();
                let mut filtered_base = self.linear_tracker.clone();
                filtered_base.states.retain(|r, _| !mut_reg_set.contains(r));
                filtered_base.merge_branches(
                    &then_tracker,
                    &else_tracker,
                    &self.source_map,
                    &self.fn_name,
                    line,
                )?;
                self.linear_tracker = filtered_base;

                for ((name, _, ty, local_alloc, span), (pr, _)) in
                    mut_vars.iter().zip(merge_params.iter())
                {
                    self.update_var_binding(name, *pr, ty.clone(), *local_alloc, *span)?;
                }
            } else if !then_terminates {
                // Only then reached merge_bb: fix up then's branch if we passed args while else returned
                if !mut_vars.is_empty() {
                    let merge_params: Vec<(RegId, Type)> = mut_vars
                        .iter()
                        .map(|(_, _, ty, _, _)| (self.alloc_reg(), ty.clone()))
                        .collect();
                    self.start_block_with_params(merge_bb, merge_params.clone());
                    self.scopes = then_scopes;
                    self.linear_tracker.restore(then_tracker);
                    for ((name, _, ty, local_alloc, span), (pr, _)) in
                        mut_vars.iter().zip(merge_params.iter())
                    {
                        self.update_var_binding(name, *pr, ty.clone(), *local_alloc, *span)?;
                    }
                } else {
                    self.start_block(merge_bb);
                    self.scopes = then_scopes;
                    self.linear_tracker.restore(then_tracker);
                }
            } else if !mut_vars.is_empty() {
                let merge_params: Vec<(RegId, Type)> = mut_vars
                    .iter()
                    .map(|(_, _, ty, _, _)| (self.alloc_reg(), ty.clone()))
                    .collect();
                self.start_block_with_params(merge_bb, merge_params.clone());
                self.scopes = else_scopes;
                self.linear_tracker.restore(else_tracker);
                for ((name, _, ty, local_alloc, span), (pr, _)) in
                    mut_vars.iter().zip(merge_params.iter())
                {
                    self.update_var_binding(name, *pr, ty.clone(), *local_alloc, *span)?;
                }
            } else {
                self.start_block(merge_bb);
                self.scopes = else_scopes;
                self.linear_tracker.restore(else_tracker);
            }
        } else {
            self.is_terminated = true;
        }

        Ok(())
    }

    fn lower_while(
        &mut self,
        cond_expr: &Expr,
        body: &Block,
        line: usize,
    ) -> Result<(), LowerError> {
        // Identify all outer variables reassigned inside the loop body
        let mut mutated_names = Vec::new();
        collect_assigned_vars_in_block(body, &HashSet::new(), &mut mutated_names);
        let loop_vars_info: Vec<(String, RegId, Type, bool, Span)> = mutated_names
            .into_iter()
            .filter_map(|name| {
                self.lookup_var_binding(&name).map(|b| {
                    (
                        name,
                        b.reg,
                        b.ty.clone(),
                        b.is_local_alloc,
                        Span::new(0, 0, b.line, 1),
                    )
                })
            })
            .collect();

        let loop_header = self.alloc_block();
        let loop_body = self.alloc_block();
        let loop_exit = self.alloc_block();

        let init_args: Vec<RegId> = loop_vars_info.iter().map(|(_, r, _, _, _)| *r).collect();

        // Mark pre-loop linear vars as moved into the loop header
        for (_, r, ty, _, _) in &loop_vars_info {
            if ty.is_linear() {
                self.linear_tracker.mark_moved(*r, line)?;
            }
        }

        // Branch from pre-header into loop_header with initial values
        self.current_block.terminator = if init_args.is_empty() {
            Terminator::Br(loop_header)
        } else {
            Terminator::BrArgs {
                target: loop_header,
                args: init_args,
            }
        };
        self.finalize_current_block();

        // Allocate block parameters for loop_header
        let mut header_params = Vec::with_capacity(loop_vars_info.len());
        for (_, _, ty, _, _) in &loop_vars_info {
            header_params.push((self.alloc_reg(), ty.clone()));
        }
        self.start_block_with_params(loop_header, header_params.clone());

        // Bind each loop variable to its loop_header block parameter
        for ((name, _, ty, local_alloc, span), (preg, _)) in
            loop_vars_info.iter().zip(header_params.iter())
        {
            self.update_var_binding(name, *preg, ty.clone(), *local_alloc, *span)?;
        }

        let header_scopes = self.scopes.clone();
        let header_tracker = self.linear_tracker.snapshot();

        // Evaluate loop condition inside loop_header
        let (cond_reg, cond_ty) = self.lower_expr(cond_expr)?;
        if cond_ty != Type::Bool {
            return Err(LowerError::TypeMismatch {
                line,
                expected: "bool".to_string(),
                actual: cond_ty.to_string(),
            });
        }

        let exit_edge_args: Vec<RegId> = header_params.iter().map(|(r, _)| *r).collect();
        self.current_block.terminator = if exit_edge_args.is_empty() {
            Terminator::BrIf {
                cond: cond_reg,
                then_blk: loop_body,
                else_blk: loop_exit,
            }
        } else {
            Terminator::BrIfArgs {
                cond: cond_reg,
                then_blk: loop_body,
                then_args: Vec::new(),
                else_blk: loop_exit,
                else_args: exit_edge_args,
            }
        };
        self.finalize_current_block();

        // Push loop context for break/continue
        let loop_var_names: Vec<String> = loop_vars_info.iter().map(|(n, ..)| n.clone()).collect();
        self.loop_stack.push(LoopContext {
            header_bb: loop_header,
            exit_bb: loop_exit,
            loop_vars: loop_var_names,
        });

        // Lower loop body
        self.start_block(loop_body);
        self.lower_block(body)?;
        if !self.is_terminated {
            let mut back_args = Vec::with_capacity(loop_vars_info.len());
            for (name, _, _, _, _) in &loop_vars_info {
                let (r, _) = self.lookup_var(name, line)?;
                back_args.push(r);
            }
            self.current_block.terminator = if back_args.is_empty() {
                Terminator::Br(loop_header)
            } else {
                Terminator::BrArgs {
                    target: loop_header,
                    args: back_args,
                }
            };
            self.finalize_current_block();
        }
        self.loop_stack.pop();

        // Start loop_exit with block params receiving the final values of loop_vars
        let mut exit_params = Vec::with_capacity(loop_vars_info.len());
        for (_, _, ty, _, _) in &loop_vars_info {
            exit_params.push((self.alloc_reg(), ty.clone()));
        }
        self.start_block_with_params(loop_exit, exit_params.clone());
        self.scopes = header_scopes;
        self.linear_tracker.restore(header_tracker);

        // Move header linear regs into exit params
        for ((_, _, ty, _, _), (hreg, _)) in loop_vars_info.iter().zip(header_params.iter()) {
            if ty.is_linear() {
                let _ = self.linear_tracker.mark_moved(*hreg, line);
            }
        }
        for ((name, _, ty, local_alloc, span), (ereg, _)) in
            loop_vars_info.iter().zip(exit_params.iter())
        {
            self.update_var_binding(name, *ereg, ty.clone(), *local_alloc, *span)?;
        }

        Ok(())
    }

    pub fn lower_syscall_stmt(
        &mut self,
        effect_name: &str,
        op_name: &str,
        payload_expr: &Expr,
        line: usize,
    ) -> Result<(), LowerError> {
        let eff = parse_effect_name(effect_name, line)?;

        if !self.fn_effects.contains(&eff) {
            return Err(LowerError::UndeclaredEffect {
                line,
                effect: effect_name.to_string(),
                msg: format!(
                    "Function '{}' performs syscall requiring effect {effect_name}",
                    self.fn_name
                ),
            });
        }

        let (payload_reg, payload_ty) = self.lower_expr(payload_expr)?;
        if payload_ty.is_linear() {
            self.mark_consumed(payload_reg, line)?;
        }
        self.emit(Instruction::new(
            None,
            Type::Void,
            Op::Syscall(eff, op_name.to_string(), payload_reg),
            line,
        ));
        Ok(())
    }
}

pub(crate) fn parse_effect_name(effect_name: &str, line: usize) -> Result<Effect, LowerError> {
    match effect_name {
        "Net" => Ok(Effect::Net),
        "Ui" => Ok(Effect::Ui),
        "Hw" => Ok(Effect::Hw),
        "Async" => Ok(Effect::Async),
        _ => Err(LowerError::InvalidConstruct {
            line,
            msg: format!("Unknown effect '{effect_name}'"),
        }),
    }
}

/// Collects variable names assigned via `StmtKind::Assign` inside a block,
/// excluding variables freshly shadowed by `let` in the same or nested scope.
pub(crate) fn collect_assigned_vars_in_block(
    block: &Block,
    shadowed: &HashSet<String>,
    out: &mut Vec<String>,
) {
    let mut local_shadowed = shadowed.clone();
    for stmt in &block.stmts {
        collect_assigned_vars_in_stmt(stmt, &mut local_shadowed, out);
    }
    if let Some(ref tail) = block.tail_expr {
        collect_assigned_vars_in_expr(tail, &local_shadowed, out);
    }
}

fn collect_assigned_vars_in_stmt(
    stmt: &Stmt,
    shadowed: &mut HashSet<String>,
    out: &mut Vec<String>,
) {
    match &stmt.kind {
        StmtKind::Let { name, init, .. } => {
            collect_assigned_vars_in_expr(init, shadowed, out);
            shadowed.insert(name.name.clone());
        }
        StmtKind::Assign { target, value } => {
            collect_assigned_vars_in_expr(value, shadowed, out);
            if let ExprKind::Variable(ref ident) = target.kind {
                if !shadowed.contains(&ident.name) && !out.contains(&ident.name) {
                    out.push(ident.name.clone());
                }
            }
        }
        StmtKind::If {
            cond,
            then_block,
            else_block,
        } => {
            collect_assigned_vars_in_expr(cond, shadowed, out);
            collect_assigned_vars_in_block(then_block, shadowed, out);
            if let Some(eb) = else_block {
                collect_assigned_vars_in_block(eb, shadowed, out);
            }
        }
        StmtKind::While { cond, body } => {
            collect_assigned_vars_in_expr(cond, shadowed, out);
            collect_assigned_vars_in_block(body, shadowed, out);
        }
        StmtKind::Block(blk) => collect_assigned_vars_in_block(blk, shadowed, out),
        StmtKind::Expr(e) => collect_assigned_vars_in_expr(e, shadowed, out),
        StmtKind::Return { value: Some(e) } => collect_assigned_vars_in_expr(e, shadowed, out),
        StmtKind::Syscall { payload, .. } => collect_assigned_vars_in_expr(payload, shadowed, out),
        StmtKind::Return { value: None } | StmtKind::Break | StmtKind::Continue => {}
    }
}

pub(crate) fn collect_assigned_vars_in_expr(
    expr: &Expr,
    shadowed: &HashSet<String>,
    out: &mut Vec<String>,
) {
    match &expr.kind {
        ExprKind::If {
            cond,
            then_branch,
            else_branch,
        } => {
            collect_assigned_vars_in_expr(cond, shadowed, out);
            collect_assigned_vars_in_block(then_branch, shadowed, out);
            collect_assigned_vars_in_block(else_branch, shadowed, out);
        }
        ExprKind::Match { scrutinee, arms } => {
            collect_assigned_vars_in_expr(scrutinee, shadowed, out);
            for arm in arms {
                let mut arm_shadowed = shadowed.clone();
                if let crate::syntax::ast::MatchPattern::Variant { ref bindings, .. } = arm.pattern
                {
                    for b in bindings {
                        arm_shadowed.insert(b.name.clone());
                    }
                }
                collect_assigned_vars_in_expr(&arm.body, &arm_shadowed, out);
            }
        }
        ExprKind::Block(blk) => collect_assigned_vars_in_block(blk, shadowed, out),
        ExprKind::Binary { lhs, rhs, .. } => {
            collect_assigned_vars_in_expr(lhs, shadowed, out);
            collect_assigned_vars_in_expr(rhs, shadowed, out);
        }
        ExprKind::Unary { operand, .. } | ExprKind::Borrow(operand) => {
            collect_assigned_vars_in_expr(operand, shadowed, out);
        }
        ExprKind::Call { args, .. } | ExprKind::EnumLit { args, .. } | ExprKind::ArrayLit(args) => {
            for a in args {
                collect_assigned_vars_in_expr(a, shadowed, out);
            }
        }
        ExprKind::StructLit { fields, .. } => {
            for f in fields {
                collect_assigned_vars_in_expr(&f.value, shadowed, out);
            }
        }
        ExprKind::Syscall { payload, .. } => collect_assigned_vars_in_expr(payload, shadowed, out),
        ExprKind::FieldAccess { target, .. } => collect_assigned_vars_in_expr(target, shadowed, out),
        ExprKind::Index { target, index } => {
            collect_assigned_vars_in_expr(target, shadowed, out);
            collect_assigned_vars_in_expr(index, shadowed, out);
        }
        ExprKind::Int(_)
        | ExprKind::Float(_)
        | ExprKind::Bool(_)
        | ExprKind::Str(_)
        | ExprKind::Variable(_) => {}
    }
}
