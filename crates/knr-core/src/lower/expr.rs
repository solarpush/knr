use std::collections::HashSet;

use crate::ast::{Effect, Instruction, Op, RegId, Terminator, Type};
use crate::lower::context::{ArrowReturnCtx, FnLoweringContext, LinearState};
use crate::lower::error::LowerError;
use crate::lower::stmt::{
    collect_assigned_vars_in_block, collect_assigned_vars_in_expr, parse_effect_name,
};
use crate::syntax::ast::{
    mangle_generic_name, ArrowBody, BinaryOp, Block, Expr, ExprKind, Ident, MatchArm, MatchPattern,
    Span, UnaryOp,
};

impl<'a> FnLoweringContext<'a> {
    pub fn lower_expr(&mut self, expr: &Expr) -> Result<(RegId, Type), LowerError> {
        self.lower_expr_with_expected(expr, None)
    }

    pub fn lower_expr_with_expected(
        &mut self,
        expr: &Expr,
        expected_ty: Option<&Type>,
    ) -> Result<(RegId, Type), LowerError> {
        let line = expr.span.line;
        match &expr.kind {
            ExprKind::Int(v) => {
                if expected_ty == Some(&Type::F64) {
                    let dst = self.alloc_reg();
                    self.record_span(dst, expr.span);
                    self.emit(Instruction::new(
                        Some(dst),
                        Type::F64,
                        Op::ConstF64(*v as f64),
                        line,
                    ));
                    return Ok((dst, Type::F64));
                }
                let dst = self.alloc_reg();
                self.record_span(dst, expr.span);
                self.emit(Instruction::new(Some(dst), Type::I64, Op::ConstI64(*v), line));
                Ok((dst, Type::I64))
            }
            ExprKind::Float(v) => {
                let dst = self.alloc_reg();
                self.record_span(dst, expr.span);
                self.emit(Instruction::new(Some(dst), Type::F64, Op::ConstF64(*v), line));
                Ok((dst, Type::F64))
            }
            ExprKind::Bool(v) => {
                let dst = self.alloc_reg();
                self.record_span(dst, expr.span);
                self.emit(Instruction::new(Some(dst), Type::Bool, Op::ConstBool(*v), line));
                Ok((dst, Type::Bool))
            }
            ExprKind::Str(s) => {
                let dst = self.alloc_reg();
                self.record_span(dst, expr.span);
                self.emit(Instruction::new(
                    Some(dst),
                    Type::Str,
                    Op::ConstStr(s.clone()),
                    line,
                ));
                Ok((dst, Type::Str))
            }
            ExprKind::ColorHex(hex) => self.lower_color_hex(hex, expected_ty, expr.span),
            ExprKind::TemplateStr(parts) => self.lower_template_str(parts, expr.span),
            ExprKind::Variable(ident) => {
                if let Ok((reg, ty)) = self.lookup_var(&ident.name, line) {
                    self.record_span(reg, expr.span);
                    return Ok((reg, ty));
                }
                if let Some(const_expr) = self.const_exprs.get(&ident.name).cloned() {
                    return self.lower_expr_with_expected(&const_expr, expected_ty);
                }
                let (reg, ty) = self.lookup_var(&ident.name, line)?;
                self.record_span(reg, expr.span);
                Ok((reg, ty))
            }
            ExprKind::Borrow(inner) => {
                // Borrow evaluates inner without moving ownership
                self.lower_expr_with_expected(inner, expected_ty)
            }
            ExprKind::Jsx {
                tag,
                props,
                children,
            } => self.lower_jsx(tag, props, children, expr.span),
            ExprKind::Unary { op, operand } => self.lower_unary(*op, operand, line),
            ExprKind::Binary { op, lhs, rhs } => {
                self.lower_binary(*op, lhs, rhs, expected_ty, line)
            }
            ExprKind::Call {
                callee,
                type_args: _,
                args,
            } => self.lower_call(&callee.name, args, expected_ty, line),
            ExprKind::MethodCall {
                receiver,
                method,
                args,
            } => self.lower_method_call(receiver, &method.name, args, expected_ty, line),
            ExprKind::Arrow { .. } => Err(LowerError::InvalidConstruct {
                line,
                msg: "Arrow function expressions are only supported as callbacks to array methods (.map, .filter, .reduce, .some, .every, .count, .findIndex, .flatMap)".to_string(),
            }),
            ExprKind::Syscall {
                effect,
                op,
                payload,
            } => self.lower_syscall_expr(&effect.name, op, payload, expected_ty, expr.span),
            ExprKind::StructLit {
                name,
                fields,
                spread,
            } => self.lower_struct_lit(
                name.as_ref().map(|id| id.name.as_str()),
                fields,
                spread.as_deref(),
                expected_ty,
                line,
            ),
            ExprKind::EnumLit {
                enum_name,
                type_args,
                variant,
                args,
            } => {
                let resolved_enum = self.resolve_enum_name(
                    &enum_name.name,
                    type_args,
                    expected_ty,
                    line,
                )?;
                self.lower_enum_lit(&resolved_enum, &variant.name, args, line)
            }
            ExprKind::ArrayLit(elements) => self.lower_array_lit(elements, expected_ty, expr.span),
            ExprKind::FieldAccess { target, field } => {
                self.lower_field_access(target, &field.name, expected_ty, line)
            }
            ExprKind::Index { target, index } => self.lower_index(target, index, line),
            ExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => self.lower_if_expr(cond, then_branch, else_branch, expected_ty, line),
            ExprKind::Match { scrutinee, arms } => {
                self.lower_match_expr(scrutinee, arms, expected_ty, line)
            }
            ExprKind::Block(blk) => match self.lower_block_value(blk, expected_ty)? {
                Some(pair) => Ok(pair),
                None => Ok((0, expected_ty.cloned().unwrap_or(Type::Void))),
            },
        }
    }

    fn lower_unary(
        &mut self,
        op: UnaryOp,
        operand: &Expr,
        line: usize,
    ) -> Result<(RegId, Type), LowerError> {
        let (r_val, ty_val) = self.lower_expr(operand)?;
        match op {
            UnaryOp::Neg => {
                if ty_val == Type::I64 {
                    let zero_reg = self.alloc_reg();
                    self.emit(Instruction::new(
                        Some(zero_reg),
                        Type::I64,
                        Op::ConstI64(0),
                        line,
                    ));
                    let dst = self.alloc_reg();
                    self.emit(Instruction::new(
                        Some(dst),
                        Type::I64,
                        Op::Sub(zero_reg, r_val),
                        line,
                    ));
                    Ok((dst, Type::I64))
                } else if ty_val == Type::F64 {
                    let zero_reg = self.alloc_reg();
                    self.emit(Instruction::new(
                        Some(zero_reg),
                        Type::F64,
                        Op::ConstF64(0.0),
                        line,
                    ));
                    let dst = self.alloc_reg();
                    self.emit(Instruction::new(
                        Some(dst),
                        Type::F64,
                        Op::Sub(zero_reg, r_val),
                        line,
                    ));
                    Ok((dst, Type::F64))
                } else {
                    Err(LowerError::TypeMismatch {
                        line,
                        expected: "i64 or f64".to_string(),
                        actual: ty_val.to_string(),
                    })
                }
            }
            UnaryOp::Not => {
                if ty_val != Type::Bool {
                    return Err(LowerError::TypeMismatch {
                        line,
                        expected: "bool".to_string(),
                        actual: ty_val.to_string(),
                    });
                }
                let false_reg = self.alloc_reg();
                self.emit(Instruction::new(
                    Some(false_reg),
                    Type::Bool,
                    Op::ConstBool(false),
                    line,
                ));
                let dst = self.alloc_reg();
                self.emit(Instruction::new(
                    Some(dst),
                    Type::Bool,
                    Op::CmpEq(r_val, false_reg),
                    line,
                ));
                Ok((dst, Type::Bool))
            }
        }
    }

    fn lower_binary(
        &mut self,
        op: BinaryOp,
        lhs: &Expr,
        rhs: &Expr,
        expected_ty: Option<&Type>,
        line: usize,
    ) -> Result<(RegId, Type), LowerError> {
        // Short-circuit logical operators using SSA block parameters
        if matches!(op, BinaryOp::And | BinaryOp::Or) {
            return self.lower_short_circuit(op, lhs, rhs, line);
        }
        // Lazy Option / Result nullish coalescing (`lhs ?? rhs`)
        if op == BinaryOp::NullishCoalesce {
            return self.lower_nullish_coalesce(lhs, rhs, expected_ty, line);
        }

        let (r_lhs, ty_lhs) = self.lower_expr(lhs)?;
        let (r_rhs, ty_rhs) = self.lower_expr(rhs)?;

        match op {
            BinaryOp::Add => {
                if ty_lhs != ty_rhs {
                    return Err(LowerError::TypeMismatch {
                        line,
                        expected: ty_lhs.to_string(),
                        actual: ty_rhs.to_string(),
                    });
                }
                if ty_lhs != Type::I64 && ty_lhs != Type::F64 && ty_lhs != Type::Str {
                    return Err(LowerError::TypeMismatch {
                        line,
                        expected: "i64, f64, or str".to_string(),
                        actual: ty_lhs.to_string(),
                    });
                }
                let dst = self.alloc_reg();
                self.emit(Instruction::new(
                    Some(dst),
                    ty_lhs.clone(),
                    Op::Add(r_lhs, r_rhs),
                    line,
                ));
                Ok((dst, ty_lhs))
            }
            BinaryOp::Sub => self.emit_arith(r_lhs, r_rhs, ty_lhs, ty_rhs, Op::Sub, line),
            BinaryOp::Mul => self.emit_arith(r_lhs, r_rhs, ty_lhs, ty_rhs, Op::Mul, line),
            BinaryOp::Div => self.emit_arith(r_lhs, r_rhs, ty_lhs, ty_rhs, Op::Div, line),
            BinaryOp::Rem => {
                if ty_lhs != Type::I64 || ty_rhs != Type::I64 {
                    return Err(LowerError::TypeMismatch {
                        line,
                        expected: "i64".to_string(),
                        actual: format!("{ty_lhs} and {ty_rhs}"),
                    });
                }
                let dst = self.alloc_reg();
                self.emit(Instruction::new(
                    Some(dst),
                    Type::I64,
                    Op::Rem(r_lhs, r_rhs),
                    line,
                ));
                Ok((dst, Type::I64))
            }
            BinaryOp::Eq => {
                if ty_lhs != ty_rhs {
                    return Err(LowerError::TypeMismatch {
                        line,
                        expected: ty_lhs.to_string(),
                        actual: ty_rhs.to_string(),
                    });
                }
                let dst = self.alloc_reg();
                self.emit(Instruction::new(
                    Some(dst),
                    Type::Bool,
                    Op::CmpEq(r_lhs, r_rhs),
                    line,
                ));
                Ok((dst, Type::Bool))
            }
            BinaryOp::NotEq => {
                if ty_lhs != ty_rhs {
                    return Err(LowerError::TypeMismatch {
                        line,
                        expected: ty_lhs.to_string(),
                        actual: ty_rhs.to_string(),
                    });
                }
                let eq_reg = self.alloc_reg();
                self.emit(Instruction::new(
                    Some(eq_reg),
                    Type::Bool,
                    Op::CmpEq(r_lhs, r_rhs),
                    line,
                ));
                let false_reg = self.alloc_reg();
                self.emit(Instruction::new(
                    Some(false_reg),
                    Type::Bool,
                    Op::ConstBool(false),
                    line,
                ));
                let dst = self.alloc_reg();
                self.emit(Instruction::new(
                    Some(dst),
                    Type::Bool,
                    Op::CmpEq(eq_reg, false_reg),
                    line,
                ));
                Ok((dst, Type::Bool))
            }
            BinaryOp::Lt => {
                if ty_lhs != ty_rhs || (ty_lhs != Type::I64 && ty_lhs != Type::F64) {
                    return Err(LowerError::TypeMismatch {
                        line,
                        expected: "matching i64 or f64".to_string(),
                        actual: format!("{ty_lhs} and {ty_rhs}"),
                    });
                }
                let dst = self.alloc_reg();
                self.emit(Instruction::new(
                    Some(dst),
                    Type::Bool,
                    Op::CmpLt(r_lhs, r_rhs),
                    line,
                ));
                Ok((dst, Type::Bool))
            }
            BinaryOp::Gt => {
                if ty_lhs != ty_rhs || (ty_lhs != Type::I64 && ty_lhs != Type::F64) {
                    return Err(LowerError::TypeMismatch {
                        line,
                        expected: "matching i64 or f64".to_string(),
                        actual: format!("{ty_lhs} and {ty_rhs}"),
                    });
                }
                let dst = self.alloc_reg();
                self.emit(Instruction::new(
                    Some(dst),
                    Type::Bool,
                    Op::CmpLt(r_rhs, r_lhs),
                    line,
                ));
                Ok((dst, Type::Bool))
            }
            BinaryOp::LtEq => {
                if ty_lhs != ty_rhs || (ty_lhs != Type::I64 && ty_lhs != Type::F64) {
                    return Err(LowerError::TypeMismatch {
                        line,
                        expected: "matching i64 or f64".to_string(),
                        actual: format!("{ty_lhs} and {ty_rhs}"),
                    });
                }
                let gt_dst = self.alloc_reg();
                self.emit(Instruction::new(
                    Some(gt_dst),
                    Type::Bool,
                    Op::CmpLt(r_rhs, r_lhs),
                    line,
                ));
                let false_reg = self.alloc_reg();
                self.emit(Instruction::new(
                    Some(false_reg),
                    Type::Bool,
                    Op::ConstBool(false),
                    line,
                ));
                let dst = self.alloc_reg();
                self.emit(Instruction::new(
                    Some(dst),
                    Type::Bool,
                    Op::CmpEq(gt_dst, false_reg),
                    line,
                ));
                Ok((dst, Type::Bool))
            }
            BinaryOp::GtEq => {
                if ty_lhs != ty_rhs || (ty_lhs != Type::I64 && ty_lhs != Type::F64) {
                    return Err(LowerError::TypeMismatch {
                        line,
                        expected: "matching i64 or f64".to_string(),
                        actual: format!("{ty_lhs} and {ty_rhs}"),
                    });
                }
                let lt_dst = self.alloc_reg();
                self.emit(Instruction::new(
                    Some(lt_dst),
                    Type::Bool,
                    Op::CmpLt(r_lhs, r_rhs),
                    line,
                ));
                let false_reg = self.alloc_reg();
                self.emit(Instruction::new(
                    Some(false_reg),
                    Type::Bool,
                    Op::ConstBool(false),
                    line,
                ));
                let dst = self.alloc_reg();
                self.emit(Instruction::new(
                    Some(dst),
                    Type::Bool,
                    Op::CmpEq(lt_dst, false_reg),
                    line,
                ));
                Ok((dst, Type::Bool))
            }
            BinaryOp::And | BinaryOp::Or | BinaryOp::NullishCoalesce => unreachable!(),
        }
    }

    /// Lowers lazy nullish coalescing `lhs ?? rhs` on `Option<T>` (`Some(v)`/`None`) or
    /// `Result<T, E>` (`Ok(v)`/`Err(e)`): checks `__tag === 0`, extracts payload in the
    /// present branch, and evaluates `rhs` lazily ONLY in the `None`/`Err` branch.
    fn lower_nullish_coalesce(
        &mut self,
        lhs: &Expr,
        rhs: &Expr,
        expected_ty: Option<&Type>,
        line: usize,
    ) -> Result<(RegId, Type), LowerError> {
        let (lhs_reg, lhs_ty) = self.lower_expr(lhs)?;
        let (payload_slot, payload_ty) = match &lhs_ty {
            Type::Struct(enum_name) => {
                let em = self.enums.get(enum_name).ok_or_else(|| LowerError::TypeMismatch {
                    line,
                    expected: "Option<T> or Result<T, E>".to_string(),
                    actual: lhs_ty.to_string(),
                })?;
                let is_option = em.variants.len() == 2
                    && em.variants[0].name == "Some"
                    && em.variants[0].field_slots.len() == 1
                    && em.variants[1].name == "None";
                let is_result = em.variants.len() == 2
                    && em.variants[0].name == "Ok"
                    && em.variants[0].field_slots.len() == 1
                    && em.variants[1].name == "Err";
                if is_option || is_result {
                    em.variants[0].field_slots[0].clone()
                } else {
                    return Err(LowerError::TypeMismatch {
                        line,
                        expected: "Option<T> or Result<T, E>".to_string(),
                        actual: lhs_ty.to_string(),
                    });
                }
            }
            _ => {
                return Err(LowerError::TypeMismatch {
                    line,
                    expected: "Option<T> or Result<T, E>".to_string(),
                    actual: lhs_ty.to_string(),
                });
            }
        };

        if let Some(exp) = expected_ty {
            if exp != &payload_ty {
                return Err(LowerError::TypeMismatch {
                    line,
                    expected: exp.to_string(),
                    actual: payload_ty.to_string(),
                });
            }
        }
        if payload_ty.is_linear() && self.linear_tracker.is_borrowed(lhs_reg) {
            return Err(LowerError::InvalidConstruct {
                line,
                msg: "Cannot extract linear payload via '??' from borrowed parameter".to_string(),
            });
        }

        let lhs_is_temp_owned = !self.is_reg_bound_to_var(lhs_reg)
            && matches!(
                self.linear_tracker.states.get(&lhs_reg),
                Some(LinearState::Alive {
                    local_alloc: true,
                    borrowed: false,
                    ..
                })
            );

        let tag_reg = self.alloc_reg();
        self.emit(Instruction::new(
            Some(tag_reg),
            Type::I64,
            Op::StructGet(lhs_reg, 0),
            line,
        ));
        let zero_reg = self.alloc_reg();
        self.emit(Instruction::new(
            Some(zero_reg),
            Type::I64,
            Op::ConstI64(0),
            line,
        ));
        let is_present = self.alloc_reg();
        self.emit(Instruction::new(
            Some(is_present),
            Type::Bool,
            Op::CmpEq(tag_reg, zero_reg),
            line,
        ));

        let some_bb = self.alloc_block();
        let none_bb = self.alloc_block();
        let merge_bb = self.alloc_block();

        self.current_block.terminator = Terminator::BrIf {
            cond: is_present,
            then_blk: some_bb,
            else_blk: none_bb,
        };
        self.finalize_current_block();

        let scopes_snap = self.scopes.clone();
        let tracker_snap = self.linear_tracker.snapshot();

        // Present branch (`Some(v)` / `Ok(v)`): extract payload, skip `rhs` completely
        self.start_block(some_bb);
        let payload_reg = self.alloc_reg();
        self.emit(Instruction::new(
            Some(payload_reg),
            payload_ty.clone(),
            Op::StructGet(lhs_reg, payload_slot),
            line,
        ));
        self.current_block.terminator = Terminator::BrArgs {
            target: merge_bb,
            args: vec![payload_reg],
        };
        self.finalize_current_block();

        // Absent branch (`None` / `Err(e)`): lazily evaluate `rhs`
        self.scopes = scopes_snap.clone();
        self.linear_tracker.restore(tracker_snap.clone());
        self.start_block(none_bb);
        let (rhs_reg, rhs_ty) = self.lower_expr_with_expected(rhs, Some(&payload_ty))?;
        if rhs_ty != payload_ty {
            return Err(LowerError::TypeMismatch {
                line: rhs.span.line,
                expected: payload_ty.to_string(),
                actual: rhs_ty.to_string(),
            });
        }
        if self.linear_tracker.is_borrowed(rhs_reg) {
            return Err(LowerError::InvalidConstruct {
                line: rhs.span.line,
                msg: "Cannot return borrowed parameter from '??' fallback".to_string(),
            });
        }
        if rhs_ty.is_linear() {
            let _ = self.linear_tracker.mark_moved(rhs_reg, line);
        }
        let none_tracker = self.linear_tracker.snapshot();
        if !self.is_terminated {
            self.current_block.terminator = Terminator::BrArgs {
                target: merge_bb,
                args: vec![rhs_reg],
            };
            self.finalize_current_block();
        }

        // Reconcile outer linear state across both branches
        self.scopes = scopes_snap;
        let mut merged_tracker = tracker_snap.clone();
        merged_tracker.merge_branches(
            &tracker_snap,
            &none_tracker,
            &self.source_map,
            &self.fn_name,
            line,
        )?;
        self.linear_tracker = merged_tracker;

        let res_reg = self.alloc_reg();
        self.start_block_with_params(merge_bb, vec![(res_reg, payload_ty.clone())]);
        if payload_ty.is_linear() {
            self.linear_tracker
                .mark_alive(res_reg, payload_ty.clone(), true, line);
        }
        if lhs_is_temp_owned {
            self.emit(Instruction::new(None, Type::Void, Op::Free(lhs_reg), line));
            self.linear_tracker.mark_moved(lhs_reg, line)?;
        }

        Ok((res_reg, payload_ty))
    }

    fn lower_short_circuit(
        &mut self,
        op: BinaryOp,
        lhs: &Expr,
        rhs: &Expr,
        line: usize,
    ) -> Result<(RegId, Type), LowerError> {
        let (r_lhs, ty_lhs) = self.lower_expr(lhs)?;
        if ty_lhs != Type::Bool {
            return Err(LowerError::TypeMismatch {
                line,
                expected: "bool".to_string(),
                actual: ty_lhs.to_string(),
            });
        }
        let rhs_bb = self.alloc_block();
        let merge_bb = self.alloc_block();

        match op {
            BinaryOp::And => {
                // If lhs is true -> eval rhs; else short-circuit false into merge_bb(r_lhs)
                self.current_block.terminator = Terminator::BrIfArgs {
                    cond: r_lhs,
                    then_blk: rhs_bb,
                    then_args: Vec::new(),
                    else_blk: merge_bb,
                    else_args: vec![r_lhs],
                };
            }
            BinaryOp::Or => {
                // If lhs is true -> short-circuit true into merge_bb(r_lhs); else eval rhs
                self.current_block.terminator = Terminator::BrIfArgs {
                    cond: r_lhs,
                    then_blk: merge_bb,
                    then_args: vec![r_lhs],
                    else_blk: rhs_bb,
                    else_args: Vec::new(),
                };
            }
            _ => unreachable!(),
        }
        self.finalize_current_block();

        self.start_block(rhs_bb);
        let (r_rhs, ty_rhs) = self.lower_expr(rhs)?;
        if ty_rhs != Type::Bool {
            return Err(LowerError::TypeMismatch {
                line,
                expected: "bool".to_string(),
                actual: ty_rhs.to_string(),
            });
        }
        self.current_block.terminator = Terminator::BrArgs {
            target: merge_bb,
            args: vec![r_rhs],
        };
        self.finalize_current_block();

        let res_reg = self.alloc_reg();
        self.start_block_with_params(merge_bb, vec![(res_reg, Type::Bool)]);
        Ok((res_reg, Type::Bool))
    }

    fn emit_arith(
        &mut self,
        lhs: RegId,
        rhs: RegId,
        ty_lhs: Type,
        ty_rhs: Type,
        op_ctor: fn(RegId, RegId) -> Op,
        line: usize,
    ) -> Result<(RegId, Type), LowerError> {
        if ty_lhs != ty_rhs {
            return Err(LowerError::TypeMismatch {
                line,
                expected: ty_lhs.to_string(),
                actual: ty_rhs.to_string(),
            });
        }
        if ty_lhs != Type::I64 && ty_lhs != Type::F64 {
            return Err(LowerError::TypeMismatch {
                line,
                expected: "i64 or f64".to_string(),
                actual: ty_lhs.to_string(),
            });
        }
        let dst = self.alloc_reg();
        self.emit(Instruction::new(
            Some(dst),
            ty_lhs.clone(),
            op_ctor(lhs, rhs),
            line,
        ));
        Ok((dst, ty_lhs))
    }

    fn lower_syscall_expr(
        &mut self,
        effect_name: &str,
        op_name: &str,
        payload_expr: &Expr,
        expected_ty: Option<&Type>,
        span: Span,
    ) -> Result<(RegId, Type), LowerError> {
        let line = span.line;
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

        let ret_ty = if let Some(exp) = expected_ty {
            exp.clone()
        } else if eff == Effect::Ui && op_name.contains("render") {
            Type::Str
        } else if eff == Effect::Net && op_name == "net.wait_req" && self.schemas.contains_key("HttpRequest") {
            Type::Struct("HttpRequest".to_string())
        } else if eff == Effect::Net && (op_name == "net.respond" || op_name == "net.close") {
            Type::Bool
        } else if eff == Effect::Hw && (op_name == "hw.simd_xor" || op_name == "hw.mmio_map") {
            Type::Buf
        } else {
            Type::I64
        };

        let dst = self.alloc_reg();
        self.record_span(dst, span);
        self.emit(Instruction::new(
            Some(dst),
            ret_ty.clone(),
            Op::Syscall(eff, op_name.to_string(), payload_reg),
            line,
        ));
        if ret_ty.is_linear() {
            self.linear_tracker
                .mark_alive(dst, ret_ty.clone(), true, line);
        }
        Ok((dst, ret_ty))
    }

    fn lower_call(
        &mut self,
        callee: &str,
        args: &[Expr],
        expected_ty: Option<&Type>,
        line: usize,
    ) -> Result<(RegId, Type), LowerError> {
        // Built-in intrinsics
        match callee {
            "syscall" => {
                self.expect_argc(callee, args, 3, line)?;
                let eff_raw = match &args[0].kind {
                    ExprKind::Str(s) => s.as_str(),
                    _ => {
                        return Err(LowerError::InvalidConstruct {
                            line,
                            msg: "First argument to syscall(effect, op, payload) must be a string literal ('net', 'ui', 'hw', 'async')".to_string(),
                        });
                    }
                };
                let eff_lower = eff_raw.to_ascii_lowercase();
                let eff_canon = match eff_lower.as_str() {
                    "net" => "Net",
                    "ui" => "Ui",
                    "hw" | "fs" | "sys" => "Hw",
                    "async" => "Async",
                    other => other,
                };
                let op_name = match &args[1].kind {
                    ExprKind::Str(s) => s.clone(),
                    _ => {
                        return Err(LowerError::InvalidConstruct {
                            line,
                            msg: "Second argument to syscall(effect, op, payload) must be an op string literal".to_string(),
                        });
                    }
                };
                return self.lower_syscall_expr(eff_canon, &op_name, &args[2], expected_ty, args[0].span);
            }
            "to_str" | "String" => {
                self.expect_argc(callee, args, 1, line)?;
                let (r, ty) = self.lower_expr(&args[0])?;
                if ty == Type::Str {
                    return Ok((r, Type::Str));
                }
                if !matches!(ty, Type::I64 | Type::F64 | Type::Bool) {
                    return Err(LowerError::TypeMismatch {
                        line,
                        expected: "i64, f64, bool, or str".to_string(),
                        actual: ty.to_string(),
                    });
                }
                let dst = self.alloc_reg();
                self.emit(Instruction::new(Some(dst), Type::Str, Op::IntToStr(r), line));
                return Ok((dst, Type::Str));
            }
            "to_f64" | "toFloat" => {
                self.expect_argc(callee, args, 1, line)?;
                let (r, ty) = self.lower_expr(&args[0])?;
                if ty == Type::F64 {
                    return Ok((r, Type::F64));
                }
                if ty != Type::I64 {
                    return Err(LowerError::TypeMismatch {
                        line,
                        expected: "i64".to_string(),
                        actual: ty.to_string(),
                    });
                }
                let dst = self.alloc_reg();
                self.emit(Instruction::new(Some(dst), Type::F64, Op::IntToFloat(r), line));
                return Ok((dst, Type::F64));
            }
            "to_i64" | "toInt" => {
                self.expect_argc(callee, args, 1, line)?;
                let (r, ty) = self.lower_expr(&args[0])?;
                if ty == Type::I64 {
                    return Ok((r, Type::I64));
                }
                if ty != Type::F64 {
                    return Err(LowerError::TypeMismatch {
                        line,
                        expected: "f64".to_string(),
                        actual: ty.to_string(),
                    });
                }
                let dst = self.alloc_reg();
                self.emit(Instruction::new(Some(dst), Type::I64, Op::FloatToInt(r), line));
                return Ok((dst, Type::I64));
            }
            "str_len" => {
                self.expect_argc(callee, args, 1, line)?;
                let (r, ty) = self.lower_expr(&args[0])?;
                if ty != Type::Str {
                    return Err(LowerError::TypeMismatch {
                        line,
                        expected: "str".to_string(),
                        actual: ty.to_string(),
                    });
                }
                let dst = self.alloc_reg();
                self.emit(Instruction::new(Some(dst), Type::I64, Op::StrLen(r), line));
                return Ok((dst, Type::I64));
            }
            "sin" | "cos" | "tan" | "sqrt" | "abs" | "floor" | "ceil" | "fract" => {
                self.expect_argc(callee, args, 1, line)?;
                let (r, ty) = self.lower_expr(&args[0])?;
                if ty != Type::F64 {
                    return Err(LowerError::TypeMismatch {
                        line,
                        expected: "f64".to_string(),
                        actual: ty.to_string(),
                    });
                }
                if callee == "fract" {
                    let fl = self.alloc_reg();
                    self.emit(Instruction::new(Some(fl), Type::F64, Op::Floor(r), line));
                    let dst = self.alloc_reg();
                    self.emit(Instruction::new(Some(dst), Type::F64, Op::Sub(r, fl), line));
                    return Ok((dst, Type::F64));
                }
                let op = match callee {
                    "sin" => Op::Sin(r),
                    "cos" => Op::Cos(r),
                    "tan" => Op::Tan(r),
                    "sqrt" => Op::Sqrt(r),
                    "abs" => Op::Abs(r),
                    "floor" => Op::Floor(r),
                    _ => Op::Ceil(r),
                };
                let dst = self.alloc_reg();
                self.emit(Instruction::new(Some(dst), Type::F64, op, line));
                return Ok((dst, Type::F64));
            }
            "pow" | "atan2" | "min" | "max" => {
                self.expect_argc(callee, args, 2, line)?;
                let (a, ta) = self.lower_expr(&args[0])?;
                let (b, tb) = self.lower_expr(&args[1])?;
                if ta != Type::F64 || tb != Type::F64 {
                    return Err(LowerError::TypeMismatch {
                        line,
                        expected: "(f64, f64)".to_string(),
                        actual: format!("({ta}, {tb})"),
                    });
                }
                let op = match callee {
                    "pow" => Op::Pow(a, b),
                    "atan2" => Op::Atan2(a, b),
                    "min" => Op::Min(a, b),
                    _ => Op::Max(a, b),
                };
                let dst = self.alloc_reg();
                self.emit(Instruction::new(Some(dst), Type::F64, op, line));
                return Ok((dst, Type::F64));
            }
            "clamp" => {
                self.expect_argc(callee, args, 3, line)?;
                let (x, tx) = self.lower_expr(&args[0])?;
                let (lo, tlo) = self.lower_expr(&args[1])?;
                let (hi, thi) = self.lower_expr(&args[2])?;
                if tx != Type::F64 || tlo != Type::F64 || thi != Type::F64 {
                    return Err(LowerError::TypeMismatch {
                        line,
                        expected: "(f64, f64, f64)".to_string(),
                        actual: format!("({tx}, {tlo}, {thi})"),
                    });
                }
                let r1 = self.alloc_reg();
                self.emit(Instruction::new(Some(r1), Type::F64, Op::Max(x, lo), line));
                let dst = self.alloc_reg();
                self.emit(Instruction::new(Some(dst), Type::F64, Op::Min(r1, hi), line));
                return Ok((dst, Type::F64));
            }
            "lerp" | "mix" => {
                self.expect_argc(callee, args, 3, line)?;
                let (a, ta) = self.lower_expr(&args[0])?;
                let (b, tb) = self.lower_expr(&args[1])?;
                let (t, tt) = self.lower_expr(&args[2])?;
                if ta != Type::F64 || tb != Type::F64 || tt != Type::F64 {
                    return Err(LowerError::TypeMismatch {
                        line,
                        expected: "(f64, f64, f64)".to_string(),
                        actual: format!("({ta}, {tb}, {tt})"),
                    });
                }
                let diff = self.alloc_reg();
                self.emit(Instruction::new(Some(diff), Type::F64, Op::Sub(b, a), line));
                let scaled = self.alloc_reg();
                self.emit(Instruction::new(Some(scaled), Type::F64, Op::Mul(diff, t), line));
                let dst = self.alloc_reg();
                self.emit(Instruction::new(Some(dst), Type::F64, Op::Add(a, scaled), line));
                return Ok((dst, Type::F64));
            }
            "smoothstep" => {
                self.expect_argc(callee, args, 3, line)?;
                let (e0, t0) = self.lower_expr(&args[0])?;
                let (e1, t1) = self.lower_expr(&args[1])?;
                let (x, tx) = self.lower_expr(&args[2])?;
                if t0 != Type::F64 || t1 != Type::F64 || tx != Type::F64 {
                    return Err(LowerError::TypeMismatch {
                        line,
                        expected: "(f64, f64, f64)".to_string(),
                        actual: format!("({t0}, {t1}, {tx})"),
                    });
                }
                let num = self.alloc_reg();
                self.emit(Instruction::new(Some(num), Type::F64, Op::Sub(x, e0), line));
                let den = self.alloc_reg();
                self.emit(Instruction::new(Some(den), Type::F64, Op::Sub(e1, e0), line));
                let raw = self.alloc_reg();
                self.emit(Instruction::new(Some(raw), Type::F64, Op::Div(num, den), line));
                let c0 = self.alloc_reg();
                self.emit(Instruction::new(Some(c0), Type::F64, Op::ConstF64(0.0), line));
                let c1 = self.alloc_reg();
                self.emit(Instruction::new(Some(c1), Type::F64, Op::ConstF64(1.0), line));
                let cl1 = self.alloc_reg();
                self.emit(Instruction::new(Some(cl1), Type::F64, Op::Max(raw, c0), line));
                let t = self.alloc_reg();
                self.emit(Instruction::new(Some(t), Type::F64, Op::Min(cl1, c1), line));
                let c2 = self.alloc_reg();
                self.emit(Instruction::new(Some(c2), Type::F64, Op::ConstF64(2.0), line));
                let c3 = self.alloc_reg();
                self.emit(Instruction::new(Some(c3), Type::F64, Op::ConstF64(3.0), line));
                let two_t = self.alloc_reg();
                self.emit(Instruction::new(Some(two_t), Type::F64, Op::Mul(c2, t), line));
                let three_m = self.alloc_reg();
                self.emit(Instruction::new(Some(three_m), Type::F64, Op::Sub(c3, two_t), line));
                let t2 = self.alloc_reg();
                self.emit(Instruction::new(Some(t2), Type::F64, Op::Mul(t, t), line));
                let dst = self.alloc_reg();
                self.emit(Instruction::new(Some(dst), Type::F64, Op::Mul(t2, three_m), line));
                return Ok((dst, Type::F64));
            }
            "buf_alloc" => {
                self.expect_argc(callee, args, 1, line)?;
                let (size_reg, size_ty) = self.lower_expr(&args[0])?;
                if size_ty != Type::I64 {
                    return Err(LowerError::TypeMismatch {
                        line,
                        expected: "i64".to_string(),
                        actual: size_ty.to_string(),
                    });
                }
                let dst = self.alloc_reg();
                self.emit(Instruction::new(
                    Some(dst),
                    Type::Buf,
                    Op::BufAlloc(size_reg),
                    line,
                ));
                self.linear_tracker.mark_alive(dst, Type::Buf, true, line);
                return Ok((dst, Type::Buf));
            }
            "buf_read" => {
                self.expect_argc(callee, args, 2, line)?;
                let (b_reg, b_ty) = self.lower_expr(&args[0])?;
                let (i_reg, i_ty) = self.lower_expr(&args[1])?;
                if b_ty != Type::Buf || i_ty != Type::I64 {
                    return Err(LowerError::TypeMismatch {
                        line,
                        expected: "(buf, i64)".to_string(),
                        actual: format!("({b_ty}, {i_ty})"),
                    });
                }
                let dst = self.alloc_reg();
                self.emit(Instruction::new(
                    Some(dst),
                    Type::I64,
                    Op::BufReadU8(b_reg, i_reg),
                    line,
                ));
                return Ok((dst, Type::I64));
            }
            "buf_write" => {
                self.expect_argc(callee, args, 3, line)?;
                let (b_reg, b_ty) = self.lower_expr(&args[0])?;
                let (i_reg, i_ty) = self.lower_expr(&args[1])?;
                let (v_reg, v_ty) = self.lower_expr(&args[2])?;
                if b_ty != Type::Buf || i_ty != Type::I64 || v_ty != Type::I64 {
                    return Err(LowerError::TypeMismatch {
                        line,
                        expected: "(buf, i64, i64)".to_string(),
                        actual: format!("({b_ty}, {i_ty}, {v_ty})"),
                    });
                }
                let dst = self.alloc_reg();
                self.emit(Instruction::new(
                    Some(dst),
                    Type::Buf,
                    Op::BufWriteU8(b_reg, i_reg, v_reg),
                    line,
                ));
                self.mark_consumed(b_reg, line)?;
                self.linear_tracker.mark_alive(dst, Type::Buf, true, line);
                return Ok((dst, Type::Buf));
            }
            "vec_new" => {
                self.expect_argc(callee, args, 0, line)?;
                let elem_ty = match expected_ty {
                    Some(Type::Vec(inner)) => (**inner).clone(),
                    _ => Type::I64,
                };
                let vec_ty = Type::Vec(Box::new(elem_ty.clone()));
                let dst = self.alloc_reg();
                self.emit(Instruction::new(
                    Some(dst),
                    vec_ty.clone(),
                    Op::VecNew(elem_ty),
                    line,
                ));
                self.linear_tracker
                    .mark_alive(dst, vec_ty.clone(), true, line);
                return Ok((dst, vec_ty));
            }
            "vec_push" => {
                self.expect_argc(callee, args, 2, line)?;
                let (v_reg, v_ty) = self.lower_expr(&args[0])?;
                let elem_ty = match &v_ty {
                    Type::Vec(inner) => (**inner).clone(),
                    _ => {
                        return Err(LowerError::TypeMismatch {
                            line,
                            expected: "vec[...]".to_string(),
                            actual: v_ty.to_string(),
                        });
                    }
                };
                let (item_reg, item_ty) = self.lower_expr_with_expected(&args[1], Some(&elem_ty))?;
                if item_ty != elem_ty {
                    return Err(LowerError::TypeMismatch {
                        line,
                        expected: elem_ty.to_string(),
                        actual: item_ty.to_string(),
                    });
                }
                let dst = self.alloc_reg();
                self.emit(Instruction::new(
                    Some(dst),
                    v_ty.clone(),
                    Op::VecPush(v_reg, item_reg),
                    line,
                ));
                self.mark_consumed(v_reg, line)?;
                if item_ty.is_linear() {
                    self.mark_consumed(item_reg, line)?;
                }
                self.linear_tracker.mark_alive(dst, v_ty.clone(), true, line);
                return Ok((dst, v_ty));
            }
            "vec_get" => {
                self.expect_argc(callee, args, 2, line)?;
                let (v_reg, v_ty) = self.lower_expr(&args[0])?;
                let (i_reg, i_ty) = self.lower_expr(&args[1])?;
                if i_ty != Type::I64 {
                    return Err(LowerError::TypeMismatch {
                        line,
                        expected: "i64".to_string(),
                        actual: i_ty.to_string(),
                    });
                }
                let elem_ty = match v_ty {
                    Type::Vec(inner) => *inner,
                    _ => {
                        return Err(LowerError::TypeMismatch {
                            line,
                            expected: "vec[...]".to_string(),
                            actual: v_ty.to_string(),
                        });
                    }
                };
                let dst = self.alloc_reg();
                self.emit(Instruction::new(
                    Some(dst),
                    elem_ty.clone(),
                    Op::VecGet(v_reg, i_reg),
                    line,
                ));
                return Ok((dst, elem_ty));
            }
            "vec_len" => {
                self.expect_argc(callee, args, 1, line)?;
                let (v_reg, v_ty) = self.lower_expr(&args[0])?;
                if !matches!(v_ty, Type::Vec(_)) {
                    return Err(LowerError::TypeMismatch {
                        line,
                        expected: "vec[...]".to_string(),
                        actual: v_ty.to_string(),
                    });
                }
                let dst = self.alloc_reg();
                self.emit(Instruction::new(
                    Some(dst),
                    Type::I64,
                    Op::VecLen(v_reg),
                    line,
                ));
                return Ok((dst, Type::I64));
            }
            "map_new" => {
                self.expect_argc(callee, args, 0, line)?;
                let (k_ty, v_ty) = match expected_ty {
                    Some(Type::Map(k, v)) => ((**k).clone(), (**v).clone()),
                    _ => (Type::Str, Type::I64),
                };
                let map_ty = Type::Map(Box::new(k_ty.clone()), Box::new(v_ty.clone()));
                let dst = self.alloc_reg();
                self.emit(Instruction::new(
                    Some(dst),
                    map_ty.clone(),
                    Op::MapNew(k_ty, v_ty),
                    line,
                ));
                self.linear_tracker
                    .mark_alive(dst, map_ty.clone(), true, line);
                return Ok((dst, map_ty));
            }
            "map_set" => {
                self.expect_argc(callee, args, 3, line)?;
                let (m_reg, m_ty) = self.lower_expr(&args[0])?;
                let (k_exp, v_exp) = match &m_ty {
                    Type::Map(k, v) => ((**k).clone(), (**v).clone()),
                    _ => {
                        return Err(LowerError::TypeMismatch {
                            line,
                            expected: "map[...]".to_string(),
                            actual: m_ty.to_string(),
                        });
                    }
                };
                let (k_reg, k_ty) = self.lower_expr_with_expected(&args[1], Some(&k_exp))?;
                let (v_reg, v_ty) = self.lower_expr_with_expected(&args[2], Some(&v_exp))?;
                if k_ty != k_exp || v_ty != v_exp {
                    return Err(LowerError::TypeMismatch {
                        line,
                        expected: format!("({k_exp}, {v_exp})"),
                        actual: format!("({k_ty}, {v_ty})"),
                    });
                }
                let dst = self.alloc_reg();
                self.emit(Instruction::new(
                    Some(dst),
                    m_ty.clone(),
                    Op::MapSet(m_reg, k_reg, v_reg),
                    line,
                ));
                self.mark_consumed(m_reg, line)?;
                if k_ty.is_linear() {
                    self.mark_consumed(k_reg, line)?;
                }
                if v_ty.is_linear() {
                    self.mark_consumed(v_reg, line)?;
                }
                self.linear_tracker.mark_alive(dst, m_ty.clone(), true, line);
                return Ok((dst, m_ty));
            }
            "map_get" => {
                self.expect_argc(callee, args, 2, line)?;
                let (m_reg, m_ty) = self.lower_expr(&args[0])?;
                let (k_exp, v_exp) = match &m_ty {
                    Type::Map(k, v) => ((**k).clone(), (**v).clone()),
                    _ => {
                        return Err(LowerError::TypeMismatch {
                            line,
                            expected: "map[...]".to_string(),
                            actual: m_ty.to_string(),
                        });
                    }
                };
                let (k_reg, k_ty) = self.lower_expr_with_expected(&args[1], Some(&k_exp))?;
                if k_ty != k_exp {
                    return Err(LowerError::TypeMismatch {
                        line,
                        expected: k_exp.to_string(),
                        actual: k_ty.to_string(),
                    });
                }
                let dst = self.alloc_reg();
                self.emit(Instruction::new(
                    Some(dst),
                    v_exp.clone(),
                    Op::MapGet(m_reg, k_reg),
                    line,
                ));
                return Ok((dst, v_exp));
            }
            "chan_new" => {
                self.expect_argc(callee, args, 1, line)?;
                if !self.fn_effects.contains(&Effect::Async) {
                    return Err(LowerError::UndeclaredEffect {
                        line,
                        effect: "Async".to_string(),
                        msg: format!("Function '{}' calls chan_new without effects(Async)", self.fn_name),
                    });
                }
                let (cap_reg, cap_ty) = self.lower_expr(&args[0])?;
                if cap_ty != Type::I64 {
                    return Err(LowerError::TypeMismatch {
                        line,
                        expected: "i64".to_string(),
                        actual: cap_ty.to_string(),
                    });
                }
                let elem_ty = match expected_ty {
                    Some(Type::Chan(inner)) => (**inner).clone(),
                    _ => Type::I64,
                };
                let chan_ty = Type::Chan(Box::new(elem_ty.clone()));
                let dst = self.alloc_reg();
                self.emit(Instruction::new(
                    Some(dst),
                    chan_ty.clone(),
                    Op::ChanNew(elem_ty, cap_reg),
                    line,
                ));
                self.linear_tracker
                    .mark_alive(dst, chan_ty.clone(), true, line);
                return Ok((dst, chan_ty));
            }
            "chan_send" => {
                self.expect_argc(callee, args, 2, line)?;
                let (c_reg, c_ty) = self.lower_expr(&args[0])?;
                let elem_ty = match &c_ty {
                    Type::Chan(inner) => (**inner).clone(),
                    _ => {
                        return Err(LowerError::TypeMismatch {
                            line,
                            expected: "chan[...]".to_string(),
                            actual: c_ty.to_string(),
                        });
                    }
                };
                let (v_reg, v_ty) = self.lower_expr_with_expected(&args[1], Some(&elem_ty))?;
                if v_ty != elem_ty {
                    return Err(LowerError::TypeMismatch {
                        line,
                        expected: elem_ty.to_string(),
                        actual: v_ty.to_string(),
                    });
                }
                let dst = self.alloc_reg();
                self.emit(Instruction::new(
                    Some(dst),
                    Type::Bool,
                    Op::ChanSend(c_reg, v_reg),
                    line,
                ));
                if v_ty.is_linear() {
                    self.mark_consumed(v_reg, line)?;
                }
                return Ok((dst, Type::Bool));
            }
            "chan_recv" => {
                self.expect_argc(callee, args, 1, line)?;
                let (c_reg, c_ty) = self.lower_expr(&args[0])?;
                let elem_ty = match c_ty {
                    Type::Chan(inner) => *inner,
                    _ => {
                        return Err(LowerError::TypeMismatch {
                            line,
                            expected: "chan[...]".to_string(),
                            actual: c_ty.to_string(),
                        });
                    }
                };
                let dst = self.alloc_reg();
                self.emit(Instruction::new(
                    Some(dst),
                    elem_ty.clone(),
                    Op::ChanRecv(c_reg),
                    line,
                ));
                if elem_ty.is_linear() {
                    self.linear_tracker
                        .mark_alive(dst, elem_ty.clone(), true, line);
                }
                return Ok((dst, elem_ty));
            }
            "free" => {
                self.expect_argc(callee, args, 1, line)?;
                let (r, ty) = self.lower_expr(&args[0])?;
                self.emit(Instruction::new(None, Type::Void, Op::Free(r), line));
                if ty.is_linear() {
                    self.mark_consumed(r, line)?;
                }
                return Ok((0, Type::Void));
            }
            _ => {}
        }

        // Seamless typed-action dispatch: if a typed `update(s: State, act: Action)` was bridged
        // to `__knr_typed_update` + `update(s: State, raw: string)`, allow callers inside `.tsk`
        // (such as deterministic unit tests) to call BOTH `update(s, "Inc")` AND `update(s, Action.Inc)`.
        if callee == "update" && args.len() == 2 {
            if let Some(typed_sig) = self.fn_sigs.get("__knr_typed_update").cloned() {
                if let Some(bridge_sig) = self.fn_sigs.get("update").cloned() {
                    if bridge_sig.param_types.get(1) == Some(&Type::Str)
                        && typed_sig.param_types.len() == 2
                        && typed_sig.param_types[1] != Type::Str
                    {
                        let is_enum_arg = match &args[1].kind {
                            ExprKind::EnumLit { .. } => true,
                            ExprKind::FieldAccess { target, .. } => {
                                if let ExprKind::Variable(ref v) = target.kind {
                                    self.enums.contains_key(&v.name)
                                } else {
                                    false
                                }
                            }
                            ExprKind::Variable(ref v) => self
                                .lookup_var_binding(&v.name)
                                .is_some_and(|b| b.ty == typed_sig.param_types[1]),
                            _ => false,
                        };
                        if is_enum_arg {
                            return self.lower_call(
                                "__knr_typed_update",
                                args,
                                expected_ty,
                                line,
                            );
                        }
                    }
                }
            }
        }

        let sig = self
            .fn_sigs
            .get(callee)
            .cloned()
            .ok_or_else(|| LowerError::UnknownFunction {
                line,
                name: callee.to_string(),
            })?;

        if args.len() != sig.param_types.len() {
            return Err(LowerError::ArgCountMismatch {
                line,
                func: callee.to_string(),
                expected: sig.param_types.len(),
                actual: args.len(),
            });
        }

        // Transitive effect checking
        for eff in &sig.effects {
            if !self.fn_effects.contains(eff) {
                return Err(LowerError::UndeclaredEffect {
                    line,
                    effect: eff.to_string(),
                    msg: format!(
                        "Function '{}' calls effectful function '{callee}' requiring effect {eff}",
                        self.fn_name
                    ),
                });
            }
        }

        let mut arg_regs = Vec::new();
        for (i, arg_expr) in args.iter().enumerate() {
            let expected = &sig.param_types[i];
            let is_param_borrowed = sig.is_param_borrowed(i);
            let (r_arg, ty_arg) = self.lower_expr_with_expected(arg_expr, Some(expected))?;
            if &ty_arg != expected {
                return Err(LowerError::TypeMismatch {
                    line,
                    expected: expected.to_string(),
                    actual: ty_arg.to_string(),
                });
            }
            // Only consume ownership if the parameter is NOT a second-class borrow (`&T`)
            if ty_arg.is_linear() && !is_param_borrowed {
                self.mark_consumed(r_arg, line)?;
            }
            arg_regs.push(r_arg);
        }

        let dst = if sig.ret_ty != Type::Void {
            Some(self.alloc_reg())
        } else {
            None
        };

        self.emit(Instruction::new(
            dst,
            sig.ret_ty.clone(),
            Op::Call(callee.to_string(), arg_regs),
            line,
        ));

        if let Some(d) = dst {
            if sig.ret_ty.is_linear() {
                self.linear_tracker
                    .mark_alive(d, sig.ret_ty.clone(), true, line);
            }
            Ok((d, sig.ret_ty))
        } else {
            Ok((0, Type::Void))
        }
    }

    fn expect_argc(
        &self,
        callee: &str,
        args: &[Expr],
        expected: usize,
        line: usize,
    ) -> Result<(), LowerError> {
        if args.len() != expected {
            return Err(LowerError::ArgCountMismatch {
                line,
                func: callee.to_string(),
                expected,
                actual: args.len(),
            });
        }
        Ok(())
    }

    pub fn lower_enum_lit(
        &mut self,
        enum_name: &str,
        variant_name: &str,
        args: &[Expr],
        line: usize,
    ) -> Result<(RegId, Type), LowerError> {
        let enum_meta = self
            .enums
            .get(enum_name)
            .cloned()
            .ok_or_else(|| LowerError::UnknownSchema {
                line,
                name: enum_name.to_string(),
            })?;

        let variant = enum_meta
            .get_variant(variant_name)
            .ok_or_else(|| LowerError::UndeclaredField {
                line,
                schema: enum_name.to_string(),
                field: variant_name.to_string(),
            })?;

        if args.len() != variant.field_slots.len() {
            return Err(LowerError::ArgCountMismatch {
                line,
                func: format!("{enum_name}::{variant_name}"),
                expected: variant.field_slots.len(),
                actual: args.len(),
            });
        }

        let struct_ty = Type::Struct(enum_name.to_string());
        let mut cur = self.alloc_reg();
        self.emit(Instruction::new(
            Some(cur),
            struct_ty.clone(),
            Op::StructNew(enum_name.to_string()),
            line,
        ));
        self.linear_tracker
            .mark_alive(cur, struct_ty.clone(), true, line);

        // Set field 0 = variant tag
        let tag_reg = self.alloc_reg();
        self.emit(Instruction::new(
            Some(tag_reg),
            Type::I64,
            Op::ConstI64(variant.tag),
            line,
        ));
        let next = self.alloc_reg();
        self.emit(Instruction::new(
            Some(next),
            struct_ty.clone(),
            Op::StructSet(cur, 0, tag_reg),
            line,
        ));
        self.linear_tracker.mark_moved(cur, line)?;
        self.linear_tracker
            .mark_alive(next, struct_ty.clone(), true, line);
        cur = next;

        // Set variant payload fields
        for (arg_expr, (slot_idx, expected_ty)) in args.iter().zip(variant.field_slots.iter()) {
            let (val_reg, val_ty) = self.lower_expr_with_expected(arg_expr, Some(expected_ty))?;
            if &val_ty != expected_ty {
                return Err(LowerError::TypeMismatch {
                    line: arg_expr.span.line,
                    expected: expected_ty.to_string(),
                    actual: val_ty.to_string(),
                });
            }
            let next = self.alloc_reg();
            self.emit(Instruction::new(
                Some(next),
                struct_ty.clone(),
                Op::StructSet(cur, *slot_idx, val_reg),
                line,
            ));
            self.linear_tracker.mark_moved(cur, line)?;
            if val_ty.is_linear() {
                self.linear_tracker.mark_moved(val_reg, line)?;
            }
            self.linear_tracker
                .mark_alive(next, struct_ty.clone(), true, line);
            cur = next;
        }

        Ok((cur, struct_ty))
    }

    fn lower_if_expr(
        &mut self,
        cond_expr: &Expr,
        then_branch: &Block,
        else_branch: &Block,
        expected_ty: Option<&Type>,
        line: usize,
    ) -> Result<(RegId, Type), LowerError> {
        let (cond_reg, cond_ty) = self.lower_expr(cond_expr)?;
        if cond_ty != Type::Bool {
            return Err(LowerError::TypeMismatch {
                line,
                expected: "bool".to_string(),
                actual: cond_ty.to_string(),
            });
        }

        let then_bb = self.alloc_block();
        let else_bb = self.alloc_block();
        let merge_bb = self.alloc_block();

        self.current_block.terminator = Terminator::BrIf {
            cond: cond_reg,
            then_blk: then_bb,
            else_blk: else_bb,
        };
        self.finalize_current_block();

        let scopes_snap = self.scopes.clone();
        let tracker_snap = self.linear_tracker.snapshot();

        // Collect outer variables mutated in either branch so we can merge them via SSA block params
        let mut mutated_names = Vec::new();
        collect_assigned_vars_in_block(then_branch, &HashSet::new(), &mut mutated_names);
        collect_assigned_vars_in_block(else_branch, &HashSet::new(), &mut mutated_names);
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

        self.start_block(then_bb);
        let then_val = self.lower_block_value(then_branch, expected_ty)?;
        let then_term = self.is_terminated;
        if let Some((r, ref ty)) = then_val {
            if !then_term {
                if ty.is_linear() {
                    let _ = self.linear_tracker.mark_moved(r, line);
                }
                let mut args = Vec::new();
                if *ty != Type::Void {
                    args.push(r);
                }
                for (name, _, _, _, _) in &mut_vars {
                    let (vr, _) = self.lookup_var(name, line)?;
                    args.push(vr);
                }
                self.current_block.terminator = if args.is_empty() {
                    Terminator::Br(merge_bb)
                } else {
                    Terminator::BrArgs {
                        target: merge_bb,
                        args,
                    }
                };
                self.finalize_current_block();
            }
        }
        let then_tracker = self.linear_tracker.snapshot();

        self.scopes = scopes_snap.clone();
        self.linear_tracker.restore(tracker_snap.clone());

        self.start_block(else_bb);
        let else_expected = then_val
            .as_ref()
            .map(|(_, t)| t)
            .or(expected_ty);
        let else_val = self.lower_block_value(else_branch, else_expected)?;
        let else_term = self.is_terminated;
        if let Some((r, ref ty)) = else_val {
            if !else_term {
                if ty.is_linear() {
                    let _ = self.linear_tracker.mark_moved(r, line);
                }
                let mut args = Vec::new();
                if *ty != Type::Void {
                    args.push(r);
                }
                for (name, _, _, _, _) in &mut_vars {
                    let (vr, _) = self.lookup_var(name, line)?;
                    args.push(vr);
                }
                self.current_block.terminator = if args.is_empty() {
                    Terminator::Br(merge_bb)
                } else {
                    Terminator::BrArgs {
                        target: merge_bb,
                        args,
                    }
                };
                self.finalize_current_block();
            }
        }
        let else_tracker = self.linear_tracker.snapshot();

        let res_ty = match (&then_val, &else_val) {
            (Some((_, t1)), Some((_, t2))) => {
                if t1 != t2 {
                    return Err(LowerError::TypeMismatch {
                        line,
                        expected: t1.to_string(),
                        actual: t2.to_string(),
                    });
                }
                t1.clone()
            }
            (Some((_, t)), None) | (None, Some((_, t))) => t.clone(),
            (None, None) => {
                self.is_terminated = true;
                return Ok((0, expected_ty.cloned().unwrap_or(Type::Void)));
            }
        };

        self.scopes = scopes_snap;
        if !then_term && !else_term {
            let mut_reg_set: HashSet<RegId> = mut_vars.iter().map(|(_, r, _, _, _)| *r).collect();
            let mut filtered_base = tracker_snap;
            filtered_base.states.retain(|r, _| !mut_reg_set.contains(r));
            filtered_base.merge_branches(
                &then_tracker,
                &else_tracker,
                &self.source_map,
                &self.fn_name,
                line,
            )?;
            self.linear_tracker = filtered_base;
        } else if !then_term {
            self.linear_tracker.restore(then_tracker);
        } else {
            self.linear_tracker.restore(else_tracker);
        }

        let mut merge_params = Vec::new();
        let res_reg = if res_ty != Type::Void {
            let r = self.alloc_reg();
            merge_params.push((r, res_ty.clone()));
            r
        } else {
            0
        };
        let mut var_merge_regs = Vec::with_capacity(mut_vars.len());
        for (_, _, ty, _, _) in &mut_vars {
            let pr = self.alloc_reg();
            merge_params.push((pr, ty.clone()));
            var_merge_regs.push(pr);
        }
        if merge_params.is_empty() {
            self.start_block(merge_bb);
        } else {
            self.start_block_with_params(merge_bb, merge_params);
        }
        for ((name, _, ty, local_alloc, span), pr) in mut_vars.iter().zip(var_merge_regs.iter()) {
            self.update_var_binding(name, *pr, ty.clone(), *local_alloc, *span)?;
        }
        if res_ty != Type::Void && res_ty.is_linear() {
            self.linear_tracker
                .mark_alive(res_reg, res_ty.clone(), true, line);
        }
        Ok((res_reg, res_ty))
    }

    fn lower_match_expr(
        &mut self,
        scrutinee: &Expr,
        arms: &[MatchArm],
        expected_ty: Option<&Type>,
        line: usize,
    ) -> Result<(RegId, Type), LowerError> {
        if arms.is_empty() {
            return Err(LowerError::InvalidConstruct {
                line,
                msg: "match expression must have at least one arm".to_string(),
            });
        }

        let (scrut_reg, scrut_ty) = self.lower_expr(scrutinee)?;

        // Determine if scrutinee is an enum (Struct present in self.enums)
        let enum_meta = if let Type::Struct(ref name) = scrut_ty {
            self.enums.get(name).cloned()
        } else {
            None
        };

        // Exhaustiveness check for enum matches
        if let Some(ref em) = enum_meta {
            let mut has_wildcard = false;
            let mut matched_variants = HashSet::new();
            for arm in arms {
                match &arm.pattern {
                    MatchPattern::Wildcard => has_wildcard = true,
                    MatchPattern::Variant {
                        enum_name, variant, ..
                    } => {
                        if let Some(ref en) = enum_name {
                            let generic_match = em.name.starts_with(&format!("{}__", en.name));
                            if en.name != em.name && !generic_match {
                                return Err(LowerError::TypeMismatch {
                                    line: arm.span.line,
                                    expected: em.name.clone(),
                                    actual: en.name.clone(),
                                });
                            }
                        }
                        if em.get_variant(&variant.name).is_none() {
                            return Err(LowerError::UndeclaredField {
                                line: arm.span.line,
                                schema: em.name.clone(),
                                field: variant.name.clone(),
                            });
                        }
                        matched_variants.insert(variant.name.clone());
                    }
                    _ => {
                        return Err(LowerError::TypeMismatch {
                            line: arm.span.line,
                            expected: format!("variant of enum '{}'", em.name),
                            actual: "literal pattern".to_string(),
                        });
                    }
                }
            }
            if !has_wildcard {
                let missing: Vec<&str> = em
                    .variants
                    .iter()
                    .filter(|v| !matched_variants.contains(&v.name))
                    .map(|v| v.name.as_str())
                    .collect();
                if !missing.is_empty() {
                    return Err(LowerError::InvalidConstruct {
                        line,
                        msg: format!(
                            "NonExhaustiveMatch: missing variant(s) for enum '{}': {}",
                            em.name,
                            missing.join(", ")
                        ),
                    });
                }
            }
        }

        // For enums, read the discriminant tag from field 0 once
        let cmp_reg = if enum_meta.is_some() {
            let tag_reg = self.alloc_reg();
            self.emit(Instruction::new(
                Some(tag_reg),
                Type::I64,
                Op::StructGet(scrut_reg, 0),
                line,
            ));
            tag_reg
        } else {
            scrut_reg
        };

        let merge_bb = self.alloc_block();
        let scopes_snap = self.scopes.clone();
        let tracker_snap = self.linear_tracker.snapshot();

        // Collect outer variables mutated in any match arm so we pass them through merge_bb params!
        let mut mutated_names = Vec::new();
        for arm in arms {
            let mut arm_shadowed = HashSet::new();
            if let MatchPattern::Variant { ref bindings, .. } = arm.pattern {
                for b in bindings {
                    arm_shadowed.insert(b.name.clone());
                }
            }
            collect_assigned_vars_in_expr(&arm.body, &arm_shadowed, &mut mutated_names);
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

        let mut inferred_res_ty: Option<Type> = expected_ty.cloned();
        let mut any_reaches_merge = false;
        let mut arm_blocks_info: Vec<(usize, crate::ast::BlockId)> = Vec::new();
        let mut arm_trackers = Vec::new();

        // Build dispatch chain block-by-block
        for (idx, arm) in arms.iter().enumerate() {
            let is_last = idx + 1 == arms.len();
            let arm_bb = self.alloc_block();
            let next_check_bb = if is_last { None } else { Some(self.alloc_block()) };

            match &arm.pattern {
                MatchPattern::Wildcard => {
                    self.current_block.terminator = Terminator::Br(arm_bb);
                    self.finalize_current_block();
                }
                _ if is_last => {
                    // Exhaustive last arm jumps directly
                    self.current_block.terminator = Terminator::Br(arm_bb);
                    self.finalize_current_block();
                }
                MatchPattern::Variant { variant, .. } => {
                    let em = enum_meta.as_ref().ok_or_else(|| LowerError::InvalidConstruct {
                        line: arm.span.line,
                        msg: format!("Variant pattern '{}' used on non-enum type {scrut_ty}", variant.name),
                    })?;
                    let vmeta = em.get_variant(&variant.name).unwrap();
                    let tag_const = self.alloc_reg();
                    self.emit(Instruction::new(
                        Some(tag_const),
                        Type::I64,
                        Op::ConstI64(vmeta.tag),
                        arm.span.line,
                    ));
                    let cond_reg = self.alloc_reg();
                    self.emit(Instruction::new(
                        Some(cond_reg),
                        Type::Bool,
                        Op::CmpEq(cmp_reg, tag_const),
                        arm.span.line,
                    ));
                    self.current_block.terminator = Terminator::BrIf {
                        cond: cond_reg,
                        then_blk: arm_bb,
                        else_blk: next_check_bb.unwrap(),
                    };
                    self.finalize_current_block();
                }
                MatchPattern::Int(v) => {
                    let c = self.alloc_reg();
                    self.emit(Instruction::new(
                        Some(c),
                        Type::I64,
                        Op::ConstI64(*v),
                        arm.span.line,
                    ));
                    let cond_reg = self.alloc_reg();
                    self.emit(Instruction::new(
                        Some(cond_reg),
                        Type::Bool,
                        Op::CmpEq(cmp_reg, c),
                        arm.span.line,
                    ));
                    self.current_block.terminator = Terminator::BrIf {
                        cond: cond_reg,
                        then_blk: arm_bb,
                        else_blk: next_check_bb.unwrap(),
                    };
                    self.finalize_current_block();
                }
                MatchPattern::Bool(v) => {
                    let c = self.alloc_reg();
                    self.emit(Instruction::new(
                        Some(c),
                        Type::Bool,
                        Op::ConstBool(*v),
                        arm.span.line,
                    ));
                    let cond_reg = self.alloc_reg();
                    self.emit(Instruction::new(
                        Some(cond_reg),
                        Type::Bool,
                        Op::CmpEq(cmp_reg, c),
                        arm.span.line,
                    ));
                    self.current_block.terminator = Terminator::BrIf {
                        cond: cond_reg,
                        then_blk: arm_bb,
                        else_blk: next_check_bb.unwrap(),
                    };
                    self.finalize_current_block();
                }
                MatchPattern::Str(s) => {
                    let c = self.alloc_reg();
                    self.emit(Instruction::new(
                        Some(c),
                        Type::Str,
                        Op::ConstStr(s.clone()),
                        arm.span.line,
                    ));
                    let cond_reg = self.alloc_reg();
                    self.emit(Instruction::new(
                        Some(cond_reg),
                        Type::Bool,
                        Op::CmpEq(cmp_reg, c),
                        arm.span.line,
                    ));
                    self.current_block.terminator = Terminator::BrIf {
                        cond: cond_reg,
                        then_blk: arm_bb,
                        else_blk: next_check_bb.unwrap(),
                    };
                    self.finalize_current_block();
                }
            }

            arm_blocks_info.push((idx, arm_bb));

            if let Some(nb) = next_check_bb {
                if matches!(arm.pattern, MatchPattern::Wildcard) {
                    break;
                }
                self.start_block(nb);
            }
        }

        // Lower each arm body
        for (idx, arm_bb) in arm_blocks_info {
            let arm = &arms[idx];
            self.scopes = scopes_snap.clone();
            self.linear_tracker.restore(tracker_snap.clone());
            self.start_block(arm_bb);
            self.enter_scope();

            // Bind variant payload variables via StructGet
            if let (
                Some(ref em),
                MatchPattern::Variant {
                    variant, bindings, ..
                },
            ) = (&enum_meta, &arm.pattern)
            {
                let vmeta = em.get_variant(&variant.name).unwrap();
                if bindings.len() != vmeta.field_slots.len() {
                    return Err(LowerError::ArgCountMismatch {
                        line: arm.span.line,
                        func: format!("{}::{}", em.name, variant.name),
                        expected: vmeta.field_slots.len(),
                        actual: bindings.len(),
                    });
                }
                for (b_ident, (slot_idx, slot_ty)) in
                    bindings.iter().zip(vmeta.field_slots.iter())
                {
                    let dst = self.alloc_reg();
                    self.emit(Instruction::new(
                        Some(dst),
                        slot_ty.clone(),
                        Op::StructGet(scrut_reg, *slot_idx),
                        b_ident.span.line,
                    ));
                    self.bind_var(&b_ident.name, dst, slot_ty.clone(), false, b_ident.span);
                }
            }

            let (arm_reg, arm_ty) =
                self.lower_expr_with_expected(&arm.body, inferred_res_ty.as_ref())?;
            let arm_term = self.is_terminated;
            if !arm_term {
                if let Some(ref prev_ty) = inferred_res_ty {
                    if *prev_ty != arm_ty {
                        return Err(LowerError::TypeMismatch {
                            line: arm.span.line,
                            expected: prev_ty.to_string(),
                            actual: arm_ty.to_string(),
                        });
                    }
                } else {
                    inferred_res_ty = Some(arm_ty.clone());
                }
                self.drop_scope_linear_allocations(
                    arm.span.line,
                    if arm_ty == Type::Void { None } else { Some(arm_reg) },
                )?;
                if arm_ty.is_linear() {
                    let _ = self.linear_tracker.mark_moved(arm_reg, arm.span.line);
                }
                let mut branch_args = Vec::new();
                if arm_ty != Type::Void {
                    branch_args.push(arm_reg);
                }
                for (name, _, _, _, _) in &mut_vars {
                    let (vr, _) = self.lookup_var(name, arm.span.line)?;
                    branch_args.push(vr);
                }
                self.current_block.terminator = if branch_args.is_empty() {
                    Terminator::Br(merge_bb)
                } else {
                    Terminator::BrArgs {
                        target: merge_bb,
                        args: branch_args,
                    }
                };
                self.finalize_current_block();
                any_reaches_merge = true;
                arm_trackers.push(self.linear_tracker.snapshot());
            }
            self.leave_scope();
        }

        self.scopes = scopes_snap;
        if !any_reaches_merge {
            self.is_terminated = true;
            return Ok((0, inferred_res_ty.unwrap_or(Type::Void)));
        }

        if let Some(first_tr) = arm_trackers.first() {
            let mut_reg_set: HashSet<RegId> = mut_vars.iter().map(|(_, r, _, _, _)| *r).collect();
            let mut merged_tr = first_tr.clone();
            merged_tr.states.retain(|r, _| !mut_reg_set.contains(r));
            self.linear_tracker.restore(merged_tr);
        } else {
            self.linear_tracker.restore(tracker_snap);
        }

        let final_ty = inferred_res_ty.unwrap_or(Type::Void);
        let mut merge_params = Vec::new();
        let res_reg = if final_ty != Type::Void {
            let r = self.alloc_reg();
            merge_params.push((r, final_ty.clone()));
            r
        } else {
            0
        };
        let mut var_merge_regs = Vec::with_capacity(mut_vars.len());
        for (_, _, ty, _, _) in &mut_vars {
            let pr = self.alloc_reg();
            merge_params.push((pr, ty.clone()));
            var_merge_regs.push(pr);
        }
        if merge_params.is_empty() {
            self.start_block(merge_bb);
        } else {
            self.start_block_with_params(merge_bb, merge_params);
        }
        for ((name, _, ty, local_alloc, span), pr) in mut_vars.iter().zip(var_merge_regs.iter()) {
            self.update_var_binding(name, *pr, ty.clone(), *local_alloc, *span)?;
        }
        if final_ty != Type::Void && final_ty.is_linear() {
            self.linear_tracker
                .mark_alive(res_reg, final_ty.clone(), true, line);
        }
        Ok((res_reg, final_ty))
    }

    fn lower_color_hex(
        &mut self,
        hex: &str,
        expected_ty: Option<&Type>,
        span: Span,
    ) -> Result<(RegId, Type), LowerError> {
        let line = span.line;
        if let Some(Type::Struct(sname)) = expected_ty {
            if sname == "Color" {
                if let Some(schema) = self.schemas.get("Color").cloned() {
                    if schema.fields.len() >= 4
                        && schema.fields[0].ty == Type::F64
                        && schema.fields[1].ty == Type::F64
                        && schema.fields[2].ty == Type::F64
                        && schema.fields[3].ty == Type::F64
                    {
                        let s = hex.trim().strip_prefix('#').unwrap_or(hex);
                        let (r, g, b, a) = match s.len() {
                            3 => (
                                u8::from_str_radix(&s[0..1], 16).unwrap_or(0) * 17,
                                u8::from_str_radix(&s[1..2], 16).unwrap_or(0) * 17,
                                u8::from_str_radix(&s[2..3], 16).unwrap_or(0) * 17,
                                255u8,
                            ),
                            6 => (
                                u8::from_str_radix(&s[0..2], 16).unwrap_or(0),
                                u8::from_str_radix(&s[2..4], 16).unwrap_or(0),
                                u8::from_str_radix(&s[4..6], 16).unwrap_or(0),
                                255u8,
                            ),
                            8 => (
                                u8::from_str_radix(&s[0..2], 16).unwrap_or(0),
                                u8::from_str_radix(&s[2..4], 16).unwrap_or(0),
                                u8::from_str_radix(&s[4..6], 16).unwrap_or(0),
                                u8::from_str_radix(&s[6..8], 16).unwrap_or(255),
                            ),
                            _ => (0, 0, 0, 255),
                        };
                        let comps = [
                            (r as f64) / 255.0,
                            (g as f64) / 255.0,
                            (b as f64) / 255.0,
                            (a as f64) / 255.0,
                        ];
                        let struct_ty = Type::Struct("Color".to_string());
                        let mut cur = self.alloc_reg();
                        self.emit(Instruction::new(
                            Some(cur),
                            struct_ty.clone(),
                            Op::StructNew("Color".to_string()),
                            line,
                        ));
                        self.linear_tracker
                            .mark_alive(cur, struct_ty.clone(), true, line);
                        for (idx, val) in comps.iter().enumerate() {
                            let c_reg = self.alloc_reg();
                            self.emit(Instruction::new(
                                Some(c_reg),
                                Type::F64,
                                Op::ConstF64(*val),
                                line,
                            ));
                            let next = self.alloc_reg();
                            self.emit(Instruction::new(
                                Some(next),
                                struct_ty.clone(),
                                Op::StructSet(cur, idx as u32, c_reg),
                                line,
                            ));
                            self.linear_tracker.mark_moved(cur, line)?;
                            self.linear_tracker
                                .mark_alive(next, struct_ty.clone(), true, line);
                            cur = next;
                        }
                        return Ok((cur, struct_ty));
                    }
                }
            }
        }
        let dst = self.alloc_reg();
        self.record_span(dst, span);
        self.emit(Instruction::new(
            Some(dst),
            Type::Str,
            Op::ConstStr(hex.to_string()),
            line,
        ));
        Ok((dst, Type::Str))
    }

    fn lower_template_str(
        &mut self,
        parts: &[crate::syntax::ast::TemplatePart],
        span: Span,
    ) -> Result<(RegId, Type), LowerError> {
        let line = span.line;
        let mut acc: Option<RegId> = None;
        for part in parts {
            let r = match part {
                crate::syntax::ast::TemplatePart::Literal(s) => {
                    let dst = self.alloc_reg();
                    self.emit(Instruction::new(
                        Some(dst),
                        Type::Str,
                        Op::ConstStr(s.clone()),
                        line,
                    ));
                    dst
                }
                crate::syntax::ast::TemplatePart::Expr(e) => {
                    let (er, ety) = self.lower_expr(e)?;
                    self.coerce_to_str(er, &ety, e.span.line)?
                }
            };
            acc = Some(match acc {
                None => r,
                Some(prev) => {
                    let dst = self.alloc_reg();
                    self.emit(Instruction::new(
                        Some(dst),
                        Type::Str,
                        Op::Add(prev, r),
                        line,
                    ));
                    dst
                }
            });
        }
        let dst = acc.unwrap_or_else(|| {
            let d = self.alloc_reg();
            self.emit(Instruction::new(
                Some(d),
                Type::Str,
                Op::ConstStr(String::new()),
                line,
            ));
            d
        });
        Ok((dst, Type::Str))
    }

    pub fn lower_struct_lit(
        &mut self,
        explicit_name: Option<&str>,
        fields: &[crate::syntax::ast::FieldInit],
        spread: Option<&Expr>,
        expected_ty: Option<&Type>,
        line: usize,
    ) -> Result<(RegId, Type), LowerError> {
        // Resolve schema name bidirectionally: explicit -> expected_ty -> spread expr -> field match
        let resolved_name: String = if let Some(n) = explicit_name {
            n.to_string()
        } else if let Some(Type::Struct(sname)) = expected_ty {
            sname.clone()
        } else if let Some(spread_expr) = spread {
            if let ExprKind::Variable(ref ident) = spread_expr.kind {
                if let Some(binding) = self.lookup_var_binding(&ident.name) {
                    if let Type::Struct(ref sname) = binding.ty {
                        sname.clone()
                    } else {
                        return Err(LowerError::InvalidConstruct {
                            line,
                            msg: format!("Cannot spread non-struct type '{}'", binding.ty),
                        });
                    }
                } else {
                    return Err(LowerError::UndefinedVariable {
                        line,
                        name: ident.name.clone(),
                    });
                }
            } else {
                return Err(LowerError::InvalidConstruct {
                    line,
                    msg: "Object spread requires a known target type annotation".to_string(),
                });
            }
        } else {
            let fnames: HashSet<&str> = fields.iter().map(|f| f.name.name.as_str()).collect();
            let mut matches: Vec<&String> = self
                .schemas
                .iter()
                .filter(|(_, sch)| {
                    sch.fields.len() == fnames.len()
                        && sch.fields.iter().all(|fd| {
                            fd.name
                                .as_deref()
                                .is_some_and(|n| fnames.contains(n))
                        })
                })
                .map(|(k, _)| k)
                .collect();
            matches.sort();
            if matches.len() == 1 {
                matches[0].clone()
            } else {
                return Err(LowerError::InvalidConstruct {
                    line,
                    msg: format!(
                        "Cannot infer object literal type from fields {:?}; add a type annotation (e.g. `const x: MyType = {{ ... }}`)",
                        fnames
                    ),
                });
            }
        };

        let schema = self
            .schemas
            .get(&resolved_name)
            .cloned()
            .ok_or_else(|| LowerError::UnknownSchema {
                line,
                name: resolved_name.clone(),
            })?;

        let struct_ty = Type::Struct(resolved_name.clone());

        // CRITICAL ERGONOMIC FIX: evaluate all field override expressions BEFORE moving the
        // `...spread` base variable so `{ ...s, count: s.count + 1 }` NEVER triggers UseAfterMove!
        // For non-spread literals, elide zero-default scalar fields (0 / 0.0 / false) on freshly
        // zero-initialized `StructNew` structs — shrinks `style_default()` by ~3.6x across all backends!
        let mut evaluated_fields: Vec<(u32, RegId, Type)> = Vec::with_capacity(fields.len());
        let mut seen_fields: HashSet<u32> = HashSet::with_capacity(fields.len());
        for init in fields {
            let field_name = &init.name.name;
            let field_def = schema
                .get_field_by_name(field_name)
                .ok_or_else(|| LowerError::UndeclaredField {
                    line,
                    schema: resolved_name.clone(),
                    field: field_name.clone(),
                })?
                .clone();

            let is_fresh_zero_default = spread.is_none()
                && !seen_fields.contains(&field_def.index)
                && match (&field_def.ty, &init.value.kind) {
                    (Type::I64, ExprKind::Int(0)) => true,
                    (Type::F64, ExprKind::Float(v)) if *v == 0.0 && v.is_sign_positive() => true,
                    (Type::F64, ExprKind::Int(0)) => true,
                    (Type::Bool, ExprKind::Bool(false)) => true,
                    _ => false,
                };
            seen_fields.insert(field_def.index);
            if is_fresh_zero_default {
                continue;
            }

            let (val_reg, val_ty) =
                self.lower_expr_with_expected(&init.value, Some(&field_def.ty))?;
            if val_ty != field_def.ty {
                return Err(LowerError::TypeMismatch {
                    line,
                    expected: field_def.ty.to_string(),
                    actual: val_ty.to_string(),
                });
            }
            evaluated_fields.push((field_def.index, val_reg, val_ty));
        }

        // 1. Initial struct allocation OR consume base struct from `...spread`
        let mut current_struct_reg = if let Some(spread_expr) = spread {
            let (base_reg, base_ty) = self.lower_expr_with_expected(spread_expr, Some(&struct_ty))?;
            if base_ty != struct_ty {
                return Err(LowerError::TypeMismatch {
                    line,
                    expected: struct_ty.to_string(),
                    actual: base_ty.to_string(),
                });
            }
            if evaluated_fields.is_empty() {
                return Ok((base_reg, struct_ty));
            }
            base_reg
        } else {
            let r = self.alloc_reg();
            self.emit(Instruction::new(
                Some(r),
                struct_ty.clone(),
                Op::StructNew(resolved_name.clone()),
                line,
            ));
            self.linear_tracker
                .mark_alive(r, struct_ty.clone(), true, line);
            r
        };

        // 2. Linear field threading
        for (field_index, val_reg, val_ty) in evaluated_fields {
            let next_struct_reg = self.alloc_reg();
            self.emit(Instruction::new(
                Some(next_struct_reg),
                struct_ty.clone(),
                Op::StructSet(current_struct_reg, field_index, val_reg),
                line,
            ));

            self.linear_tracker.mark_moved(current_struct_reg, line)?;
            if val_ty.is_linear() {
                self.linear_tracker.mark_moved(val_reg, line)?;
            }
            self.linear_tracker
                .mark_alive(next_struct_reg, struct_ty.clone(), true, line);

            current_struct_reg = next_struct_reg;
        }

        Ok((current_struct_reg, struct_ty))
    }

    pub fn lower_array_lit(
        &mut self,
        elements: &[Expr],
        expected_ty: Option<&Type>,
        span: Span,
    ) -> Result<(RegId, Type), LowerError> {
        let line = span.line;
        if elements.is_empty() {
            if let Some(Type::Vec(elem_ty)) = expected_ty {
                let vec_ty = Type::Vec(elem_ty.clone());
                let current_vec_reg = self.alloc_reg();
                self.record_span(current_vec_reg, span);
                self.emit(Instruction::new(
                    Some(current_vec_reg),
                    vec_ty.clone(),
                    Op::VecNew((**elem_ty).clone()),
                    line,
                ));
                self.linear_tracker
                    .mark_alive(current_vec_reg, vec_ty.clone(), true, line);
                return Ok((current_vec_reg, vec_ty));
            }
            return Err(LowerError::InvalidConstruct {
                line,
                msg: "Empty vector literal requires explicit type annotation".to_string(),
            });
        }

        let expected_elem = match expected_ty {
            Some(Type::Vec(inner)) => Some(inner.as_ref()),
            _ => None,
        };

        let (first_reg, elem_ty) = self.lower_expr_with_expected(&elements[0], expected_elem)?;
        if let Some(exp) = expected_elem {
            if &elem_ty != exp {
                return Err(LowerError::TypeMismatch {
                    line: elements[0].span.line,
                    expected: exp.to_string(),
                    actual: elem_ty.to_string(),
                });
            }
        }
        let vec_ty = Type::Vec(Box::new(elem_ty.clone()));

        let mut current_vec_reg = self.alloc_reg();
        self.record_span(current_vec_reg, span);
        self.emit(Instruction::new(
            Some(current_vec_reg),
            vec_ty.clone(),
            Op::VecNew(elem_ty.clone()),
            line,
        ));
        self.linear_tracker
            .mark_alive(current_vec_reg, vec_ty.clone(), true, line);

        let mut next_vec_reg = self.alloc_reg();
        self.emit(Instruction::new(
            Some(next_vec_reg),
            vec_ty.clone(),
            Op::VecPush(current_vec_reg, first_reg),
            line,
        ));
        self.linear_tracker.mark_moved(current_vec_reg, line)?;
        if elem_ty.is_linear() {
            self.linear_tracker.mark_moved(first_reg, line)?;
        }
        self.linear_tracker
            .mark_alive(next_vec_reg, vec_ty.clone(), true, line);
        current_vec_reg = next_vec_reg;

        for elem_expr in &elements[1..] {
            let (val_reg, val_ty) = self.lower_expr_with_expected(elem_expr, Some(&elem_ty))?;
            if val_ty != elem_ty {
                return Err(LowerError::TypeMismatch {
                    line: elem_expr.span.line,
                    expected: elem_ty.to_string(),
                    actual: val_ty.to_string(),
                });
            }

            next_vec_reg = self.alloc_reg();
            self.emit(Instruction::new(
                Some(next_vec_reg),
                vec_ty.clone(),
                Op::VecPush(current_vec_reg, val_reg),
                line,
            ));
            self.linear_tracker.mark_moved(current_vec_reg, line)?;
            if val_ty.is_linear() {
                self.linear_tracker.mark_moved(val_reg, line)?;
            }
            self.linear_tracker
                .mark_alive(next_vec_reg, vec_ty.clone(), true, line);
            current_vec_reg = next_vec_reg;
        }

        Ok((current_vec_reg, vec_ty))
    }

    fn lower_field_access(
        &mut self,
        target: &Expr,
        field: &str,
        expected_ty: Option<&Type>,
        line: usize,
    ) -> Result<(RegId, Type), LowerError> {
        // Unit enum variant access: `Action.Reset` or generic `Option.None` inferred from expected type
        if let ExprKind::Variable(ref enum_ident) = target.kind {
            if self.enums.contains_key(&enum_ident.name) {
                return self.lower_enum_lit(&enum_ident.name, field, &[], line);
            }
            if let Some(Type::Struct(exp_name)) = expected_ty {
                if exp_name.starts_with(&format!("{}__", enum_ident.name))
                    && self.enums.contains_key(exp_name)
                {
                    return self.lower_enum_lit(exp_name, field, &[], line);
                }
            }
        }

        let (target_reg, target_ty) = self.lower_expr(target)?;

        // Ergonomic `.length` on arrays (`Vec`) and strings (`Str`)
        if field == "length" {
            if matches!(target_ty, Type::Vec(_)) {
                let dst = self.alloc_reg();
                self.emit(Instruction::new(Some(dst), Type::I64, Op::VecLen(target_reg), line));
                return Ok((dst, Type::I64));
            }
            if target_ty == Type::Str {
                let dst = self.alloc_reg();
                self.emit(Instruction::new(Some(dst), Type::I64, Op::StrLen(target_reg), line));
                return Ok((dst, Type::I64));
            }
        }

        let schema_name = match target_ty {
            Type::Struct(ref s) => s,
            _ => {
                return Err(LowerError::TypeMismatch {
                    line,
                    expected: "struct[...]".to_string(),
                    actual: target_ty.to_string(),
                });
            }
        };

        let schema = self
            .schemas
            .get(schema_name)
            .ok_or_else(|| LowerError::UnknownSchema {
                line,
                name: schema_name.clone(),
            })?;

        let field_def = schema
            .get_field_by_name(field)
            .ok_or_else(|| LowerError::UndeclaredField {
                line,
                schema: schema_name.clone(),
                field: field.to_string(),
            })?;

        let dst = self.alloc_reg();
        self.emit(Instruction::new(
            Some(dst),
            field_def.ty.clone(),
            Op::StructGet(target_reg, field_def.index),
            line,
        ));
        Ok((dst, field_def.ty.clone()))
    }

    fn lower_index(
        &mut self,
        target: &Expr,
        index: &Expr,
        line: usize,
    ) -> Result<(RegId, Type), LowerError> {
        let (target_reg, target_ty) = self.lower_expr(target)?;
        let (idx_reg, idx_ty) = self.lower_expr(index)?;

        match target_ty {
            Type::Vec(inner) => {
                if idx_ty != Type::I64 {
                    return Err(LowerError::TypeMismatch {
                        line,
                        expected: "i64".to_string(),
                        actual: idx_ty.to_string(),
                    });
                }
                let dst = self.alloc_reg();
                self.emit(Instruction::new(
                    Some(dst),
                    *inner.clone(),
                    Op::VecGet(target_reg, idx_reg),
                    line,
                ));
                Ok((dst, *inner))
            }
            Type::Map(k, v) => {
                if idx_ty != *k {
                    return Err(LowerError::TypeMismatch {
                        line,
                        expected: k.to_string(),
                        actual: idx_ty.to_string(),
                    });
                }
                let dst = self.alloc_reg();
                self.emit(Instruction::new(
                    Some(dst),
                    *v.clone(),
                    Op::MapGet(target_reg, idx_reg),
                    line,
                ));
                Ok((dst, *v))
            }
            _ => Err(LowerError::TypeMismatch {
                line,
                expected: "vec[...] or map[...]".to_string(),
                actual: target_ty.to_string(),
            }),
        }
    }

    pub(crate) fn resolve_enum_name(
        &self,
        raw_name: &str,
        type_args: &[crate::syntax::ast::TypeExpr],
        expected_ty: Option<&Type>,
        line: usize,
    ) -> Result<String, LowerError> {
        if !type_args.is_empty() {
            let mut concrete = Vec::with_capacity(type_args.len());
            for targ in type_args {
                concrete.push(targ.to_core_type().map_err(|msg| {
                    LowerError::InvalidConstruct { line, msg }
                })?);
            }
            return Ok(mangle_generic_name(raw_name, &concrete));
        }
        if self.enums.contains_key(raw_name) {
            return Ok(raw_name.to_string());
        }
        if let Some(Type::Struct(exp_name)) = expected_ty {
            if exp_name.starts_with(&format!("{raw_name}__")) && self.enums.contains_key(exp_name) {
                return Ok(exp_name.clone());
            }
        }
        Ok(raw_name.to_string())
    }

    fn lower_method_call(
        &mut self,
        receiver: &Expr,
        method: &str,
        args: &[Expr],
        expected_ty: Option<&Type>,
        line: usize,
    ) -> Result<(RegId, Type), LowerError> {
        match method {
            "map" => self.lower_array_map(receiver, args, expected_ty, line),
            "filter" => self.lower_array_filter(receiver, args, expected_ty, line),
            "reduce" => self.lower_array_reduce(receiver, args, expected_ty, line),
            "some" => self.lower_array_some_every(receiver, args, true, line),
            "every" => self.lower_array_some_every(receiver, args, false, line),
            "count" => self.lower_array_count(receiver, args, line),
            "findIndex" => self.lower_array_find_index(receiver, args, line),
            "includes" => self.lower_array_includes_index_of(receiver, args, true, line),
            "indexOf" => self.lower_array_includes_index_of(receiver, args, false, line),
            "flatMap" => self.lower_array_flat_map(receiver, args, expected_ty, line),
            "slice" => self.lower_array_slice(receiver, args, expected_ty, line),
            "concat" => self.lower_array_concat(receiver, args, expected_ty, line),
            "reverse" => self.lower_array_reverse(receiver, args, expected_ty, line),
            other => Err(LowerError::InvalidConstruct {
                line,
                msg: format!("Unsupported method '.{other}()'"),
            }),
        }
    }

    fn lower_arrow_body(
        &mut self,
        body: &ArrowBody,
        expected_ty: Option<&Type>,
        line: usize,
    ) -> Result<(RegId, Type), LowerError> {
        self.lower_arrow_body_with_owned(body, expected_ty, None, line)
    }

    fn lower_arrow_body_with_owned(
        &mut self,
        body: &ArrowBody,
        expected_ty: Option<&Type>,
        owned_param: Option<RegId>,
        line: usize,
    ) -> Result<(RegId, Type), LowerError> {
        match body {
            ArrowBody::Expr(e) => self.lower_expr_with_expected(e, expected_ty),
            ArrowBody::Block(blk) => {
                let merge_bb = self.alloc_block();
                let pre_tracker = self.linear_tracker.snapshot();
                let mut pre_linear_regs: HashSet<RegId> =
                    pre_tracker.states.keys().copied().collect();
                if let Some(op_reg) = owned_param {
                    pre_linear_regs.remove(&op_reg);
                }

                self.arrow_ret_stack.push(ArrowReturnCtx {
                    merge_bb,
                    expected_ty: expected_ty.cloned(),
                    inferred_ty: None,
                    pre_linear_regs,
                    reached_merge: false,
                });

                self.enter_scope();
                for stmt in &blk.stmts {
                    if self.is_terminated {
                        break;
                    }
                    self.lower_stmt(stmt)?;
                }

                if !self.is_terminated {
                    if let Some(ref tail) = blk.tail_expr {
                        self.lower_return(Some(tail), tail.span.line)?;
                    } else {
                        self.leave_scope();
                        self.arrow_ret_stack.pop();
                        return Err(LowerError::InvalidConstruct {
                            line,
                            msg: "Arrow function block body must end with a return statement"
                                .to_string(),
                        });
                    }
                }
                self.leave_scope();

                let arrow_ctx = self.arrow_ret_stack.pop().unwrap();
                let final_ty = arrow_ctx.inferred_ty.ok_or_else(|| {
                    LowerError::InvalidConstruct {
                        line,
                        msg: "Arrow function block body must return a value".to_string(),
                    }
                })?;

                let res_reg = self.alloc_reg();
                self.start_block_with_params(merge_bb, vec![(res_reg, final_ty.clone())]);
                self.linear_tracker.restore(pre_tracker);
                if let Some(op_reg) = owned_param {
                    let _ = self.linear_tracker.mark_moved(op_reg, line);
                }
                if final_ty.is_linear() {
                    self.linear_tracker
                        .mark_alive(res_reg, final_ty.clone(), true, line);
                }
                Ok((res_reg, final_ty))
            }
        }
    }

    fn bind_callback_params(
        &mut self,
        params: &[Ident],
        item_reg: RegId,
        elem_ty: &Type,
        idx_reg: RegId,
    ) {
        if let Some(p0) = params.first() {
            if elem_ty.is_linear() {
                // Borrow linear elements from the source array so reading `item.title` or passing
                // `item` to `readonly` components never violates linear ownership.
                self.bind_borrowed_param(&p0.name, item_reg, elem_ty.clone(), p0.span);
            } else {
                self.bind_var(&p0.name, item_reg, elem_ty.clone(), false, p0.span);
            }
        }
        if let Some(p1) = params.get(1) {
            self.bind_var(&p1.name, idx_reg, Type::I64, false, p1.span);
        }
    }

    /// Lowers `arr.map((item, idx?) => body)` into a verified monotonic SSA loop:
    /// allocates `VecNew(ret_elem_ty)`, iterates `0..VecLen(arr)`, borrows each element
    /// via `VecGet(arr, i)`, pushes the mapped result via `VecPush`, and returns the new `Vec`.
    fn lower_array_map(
        &mut self,
        receiver: &Expr,
        args: &[Expr],
        expected_ty: Option<&Type>,
        line: usize,
    ) -> Result<(RegId, Type), LowerError> {
        if args.len() != 1 {
            return Err(LowerError::ArgCountMismatch {
                line,
                func: ".map".to_string(),
                expected: 1,
                actual: args.len(),
            });
        }
        let (params, body) = match &args[0].kind {
            ExprKind::Arrow { params, body } => (params, body),
            _ => {
                return Err(LowerError::InvalidConstruct {
                    line: args[0].span.line,
                    msg: "Argument to '.map(...)' must be an arrow function '(item, idx) => ...'"
                        .to_string(),
                });
            }
        };
        if params.len() > 2 {
            return Err(LowerError::InvalidConstruct {
                line: args[0].span.line,
                msg: format!(
                    "'.map(...)' callback accepts at most 2 parameters (item, index), got {}",
                    params.len()
                ),
            });
        }

        let (arr_reg, arr_ty) = self.lower_expr(receiver)?;
        let elem_ty = match arr_ty {
            Type::Vec(inner) => *inner,
            other => {
                return Err(LowerError::TypeMismatch {
                    line: receiver.span.line,
                    expected: "vec[...]".to_string(),
                    actual: other.to_string(),
                });
            }
        };

        // Track whether receiver was a temporary owned Vec (e.g. `[1,2,3].map(...)` or `.filter(...).map(...)`)
        let arr_is_temp_owned = !self.is_reg_bound_to_var(arr_reg)
            && matches!(
                self.linear_tracker.states.get(&arr_reg),
                Some(LinearState::Alive {
                    local_alloc: true,
                    borrowed: false,
                    ..
                })
            );

        let expected_ret_elem = match expected_ty {
            Some(Type::Vec(inner)) => Some(inner.as_ref()),
            _ => None,
        };
        let placeholder_elem_ty = expected_ret_elem.cloned().unwrap_or(Type::Void);
        let placeholder_vec_ty = Type::Vec(Box::new(placeholder_elem_ty.clone()));

        let len_reg = self.alloc_reg();
        self.emit(Instruction::new(
            Some(len_reg),
            Type::I64,
            Op::VecLen(arr_reg),
            line,
        ));
        let zero_reg = self.alloc_reg();
        self.emit(Instruction::new(
            Some(zero_reg),
            Type::I64,
            Op::ConstI64(0),
            line,
        ));

        let out0_inst_idx = self.current_block.instructions.len();
        let out0_reg = self.alloc_reg();
        self.emit(Instruction::new(
            Some(out0_reg),
            placeholder_vec_ty.clone(),
            Op::VecNew(placeholder_elem_ty),
            line,
        ));

        let header_bb = self.alloc_block();
        let body_bb = self.alloc_block();
        let exit_bb = self.alloc_block();

        self.current_block.terminator = Terminator::BrArgs {
            target: header_bb,
            args: vec![zero_reg, out0_reg],
        };
        let pre_block_idx = self.completed_blocks.len();
        self.finalize_current_block();

        let i_param = self.alloc_reg();
        let out_param = self.alloc_reg();
        self.start_block_with_params(
            header_bb,
            vec![(i_param, Type::I64), (out_param, placeholder_vec_ty)],
        );
        let cond_reg = self.alloc_reg();
        self.emit(Instruction::new(
            Some(cond_reg),
            Type::Bool,
            Op::CmpLt(i_param, len_reg),
            line,
        ));
        self.current_block.terminator = Terminator::BrIfArgs {
            cond: cond_reg,
            then_blk: body_bb,
            then_args: Vec::new(),
            else_blk: exit_bb,
            else_args: vec![out_param],
        };
        let header_block_idx = self.completed_blocks.len();
        self.finalize_current_block();

        // Snapshot outer state before entering loop body
        let outer_scopes = self.scopes.clone();
        let outer_tracker = self.linear_tracker.snapshot();
        let pre_body_alive_linear: HashSet<RegId> = outer_tracker
            .states
            .iter()
            .filter_map(|(&r, s)| matches!(s, LinearState::Alive { .. }).then_some(r))
            .collect();

        self.start_block(body_bb);
        let item_reg = self.alloc_reg();
        self.emit(Instruction::new(
            Some(item_reg),
            elem_ty.clone(),
            Op::VecGet(arr_reg, i_param),
            line,
        ));

        self.enter_scope();
        self.bind_callback_params(params, item_reg, &elem_ty, i_param);
        let (mapped_reg, mapped_ty) = self.lower_arrow_body(body, expected_ret_elem, line)?;
        self.drop_scope_linear_allocations(line, Some(mapped_reg))?;
        self.leave_scope();

        if mapped_ty == Type::Void {
            return Err(LowerError::InvalidConstruct {
                line,
                msg: "'.map(...)' callback cannot return void".to_string(),
            });
        }
        if let Some(exp) = expected_ret_elem {
            if &mapped_ty != exp {
                return Err(LowerError::TypeMismatch {
                    line,
                    expected: exp.to_string(),
                    actual: mapped_ty.to_string(),
                });
            }
        }
        if self.linear_tracker.is_borrowed(mapped_reg) {
            return Err(LowerError::InvalidConstruct {
                line,
                msg: "Cannot return borrowed element directly from '.map(...)' callback"
                    .to_string(),
            });
        }

        // Ensure callback did not move an outer linear variable inside the loop body
        for r in &pre_body_alive_linear {
            if !self.linear_tracker.is_alive(*r) {
                let var_name = self
                    .source_map
                    .get_var(&self.fn_name, *r)
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| format!("%{r}"));
                return Err(LowerError::InvalidConstruct {
                    line,
                    msg: format!(
                        "Cannot move outer linear variable '{var_name}' inside '.map(...)' callback"
                    ),
                });
            }
        }

        let out_vec_ty = Type::Vec(Box::new(mapped_ty.clone()));

        // Back-patch `VecNew` and `header_bb` parameter with the exact inferred mapped element type
        self.completed_blocks[pre_block_idx].instructions[out0_inst_idx].ty = out_vec_ty.clone();
        self.completed_blocks[pre_block_idx].instructions[out0_inst_idx].op =
            Op::VecNew(mapped_ty.clone());
        self.completed_blocks[header_block_idx].params[1].1 = out_vec_ty.clone();

        let out_next = self.alloc_reg();
        self.emit(Instruction::new(
            Some(out_next),
            out_vec_ty.clone(),
            Op::VecPush(out_param, mapped_reg),
            line,
        ));
        if mapped_ty.is_linear() {
            let _ = self.linear_tracker.mark_moved(mapped_reg, line);
        }

        let one_reg = self.alloc_reg();
        self.emit(Instruction::new(
            Some(one_reg),
            Type::I64,
            Op::ConstI64(1),
            line,
        ));
        let i_next = self.alloc_reg();
        self.emit(Instruction::new(
            Some(i_next),
            Type::I64,
            Op::Add(i_param, one_reg),
            line,
        ));
        self.current_block.terminator = Terminator::BrArgs {
            target: header_bb,
            args: vec![i_next, out_next],
        };
        self.finalize_current_block();

        let res_reg = self.alloc_reg();
        self.start_block_with_params(exit_bb, vec![(res_reg, out_vec_ty.clone())]);
        self.scopes = outer_scopes;
        self.linear_tracker.restore(outer_tracker);
        self.linear_tracker
            .mark_alive(res_reg, out_vec_ty.clone(), true, line);

        if arr_is_temp_owned {
            self.emit(Instruction::new(None, Type::Void, Op::Free(arr_reg), line));
            self.linear_tracker.mark_moved(arr_reg, line)?;
        }

        Ok((res_reg, out_vec_ty))
    }

    /// Lowers `arr.filter((item, idx?) => pred)` into a verified monotonic SSA loop
    /// for Copy element types (`i64`, `f64`, `bool`, `string`), avoiding aliasing linear elements.
    fn lower_array_filter(
        &mut self,
        receiver: &Expr,
        args: &[Expr],
        expected_ty: Option<&Type>,
        line: usize,
    ) -> Result<(RegId, Type), LowerError> {
        if args.len() != 1 {
            return Err(LowerError::ArgCountMismatch {
                line,
                func: ".filter".to_string(),
                expected: 1,
                actual: args.len(),
            });
        }
        let (params, body) = match &args[0].kind {
            ExprKind::Arrow { params, body } => (params, body),
            _ => {
                return Err(LowerError::InvalidConstruct {
                    line: args[0].span.line,
                    msg: "Argument to '.filter(...)' must be an arrow function '(item, idx) => cond'"
                        .to_string(),
                });
            }
        };
        if params.len() > 2 {
            return Err(LowerError::InvalidConstruct {
                line: args[0].span.line,
                msg: format!(
                    "'.filter(...)' callback accepts at most 2 parameters (item, index), got {}",
                    params.len()
                ),
            });
        }

        let (arr_reg, arr_ty) = self.lower_expr(receiver)?;
        let elem_ty = match &arr_ty {
            Type::Vec(inner) => (**inner).clone(),
            other => {
                return Err(LowerError::TypeMismatch {
                    line: receiver.span.line,
                    expected: "vec[...]".to_string(),
                    actual: other.to_string(),
                });
            }
        };
        if elem_ty.is_linear() {
            return Err(LowerError::InvalidConstruct {
                line,
                msg: format!(
                    "'.filter(...)' requires a Copy element type (i64, f64, bool, string); cannot alias linear element type '{elem_ty}'"
                ),
            });
        }
        if let Some(exp) = expected_ty {
            if exp != &arr_ty {
                return Err(LowerError::TypeMismatch {
                    line,
                    expected: exp.to_string(),
                    actual: arr_ty.to_string(),
                });
            }
        }

        let arr_is_temp_owned = !self.is_reg_bound_to_var(arr_reg)
            && matches!(
                self.linear_tracker.states.get(&arr_reg),
                Some(LinearState::Alive {
                    local_alloc: true,
                    borrowed: false,
                    ..
                })
            );

        let len_reg = self.alloc_reg();
        self.emit(Instruction::new(
            Some(len_reg),
            Type::I64,
            Op::VecLen(arr_reg),
            line,
        ));
        let zero_reg = self.alloc_reg();
        self.emit(Instruction::new(
            Some(zero_reg),
            Type::I64,
            Op::ConstI64(0),
            line,
        ));
        let out0_reg = self.alloc_reg();
        self.emit(Instruction::new(
            Some(out0_reg),
            arr_ty.clone(),
            Op::VecNew(elem_ty.clone()),
            line,
        ));

        let header_bb = self.alloc_block();
        let body_bb = self.alloc_block();
        let push_bb = self.alloc_block();
        let step_bb = self.alloc_block();
        let exit_bb = self.alloc_block();

        self.current_block.terminator = Terminator::BrArgs {
            target: header_bb,
            args: vec![zero_reg, out0_reg],
        };
        self.finalize_current_block();

        let i_param = self.alloc_reg();
        let out_param = self.alloc_reg();
        self.start_block_with_params(
            header_bb,
            vec![(i_param, Type::I64), (out_param, arr_ty.clone())],
        );
        let cond_reg = self.alloc_reg();
        self.emit(Instruction::new(
            Some(cond_reg),
            Type::Bool,
            Op::CmpLt(i_param, len_reg),
            line,
        ));
        self.current_block.terminator = Terminator::BrIfArgs {
            cond: cond_reg,
            then_blk: body_bb,
            then_args: Vec::new(),
            else_blk: exit_bb,
            else_args: vec![out_param],
        };
        self.finalize_current_block();

        let outer_scopes = self.scopes.clone();
        let outer_tracker = self.linear_tracker.snapshot();
        let pre_body_alive_linear: HashSet<RegId> = outer_tracker
            .states
            .iter()
            .filter_map(|(&r, s)| matches!(s, LinearState::Alive { .. }).then_some(r))
            .collect();

        self.start_block(body_bb);
        let item_reg = self.alloc_reg();
        self.emit(Instruction::new(
            Some(item_reg),
            elem_ty.clone(),
            Op::VecGet(arr_reg, i_param),
            line,
        ));

        self.enter_scope();
        self.bind_callback_params(params, item_reg, &elem_ty, i_param);
        let (pred_reg, pred_ty) = self.lower_arrow_body(body, Some(&Type::Bool), line)?;
        self.drop_scope_linear_allocations(line, None)?;
        self.leave_scope();

        if pred_ty != Type::Bool {
            return Err(LowerError::TypeMismatch {
                line,
                expected: "bool".to_string(),
                actual: pred_ty.to_string(),
            });
        }
        for r in &pre_body_alive_linear {
            if !self.linear_tracker.is_alive(*r) {
                let var_name = self
                    .source_map
                    .get_var(&self.fn_name, *r)
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| format!("%{r}"));
                return Err(LowerError::InvalidConstruct {
                    line,
                    msg: format!(
                        "Cannot move outer linear variable '{var_name}' inside '.filter(...)' callback"
                    ),
                });
            }
        }

        self.current_block.terminator = Terminator::BrIfArgs {
            cond: pred_reg,
            then_blk: push_bb,
            then_args: Vec::new(),
            else_blk: step_bb,
            else_args: vec![out_param],
        };
        self.finalize_current_block();

        self.start_block(push_bb);
        let out_pushed = self.alloc_reg();
        self.emit(Instruction::new(
            Some(out_pushed),
            arr_ty.clone(),
            Op::VecPush(out_param, item_reg),
            line,
        ));
        self.current_block.terminator = Terminator::BrArgs {
            target: step_bb,
            args: vec![out_pushed],
        };
        self.finalize_current_block();

        let out_after = self.alloc_reg();
        self.start_block_with_params(step_bb, vec![(out_after, arr_ty.clone())]);
        let one_reg = self.alloc_reg();
        self.emit(Instruction::new(
            Some(one_reg),
            Type::I64,
            Op::ConstI64(1),
            line,
        ));
        let i_next = self.alloc_reg();
        self.emit(Instruction::new(
            Some(i_next),
            Type::I64,
            Op::Add(i_param, one_reg),
            line,
        ));
        self.current_block.terminator = Terminator::BrArgs {
            target: header_bb,
            args: vec![i_next, out_after],
        };
        self.finalize_current_block();

        let res_reg = self.alloc_reg();
        self.start_block_with_params(exit_bb, vec![(res_reg, arr_ty.clone())]);
        self.scopes = outer_scopes;
        self.linear_tracker.restore(outer_tracker);
        self.linear_tracker
            .mark_alive(res_reg, arr_ty.clone(), true, line);

        if arr_is_temp_owned {
            self.emit(Instruction::new(None, Type::Void, Op::Free(arr_reg), line));
            self.linear_tracker.mark_moved(arr_reg, line)?;
        }

        Ok((res_reg, arr_ty))
    }

    /// Lowers `arr.reduce((acc, item, idx?) => expr, init)` into a verified monotonic SSA loop
    /// with a loop-carried accumulator block parameter (supports both Copy and Linear accumulators).
    fn lower_array_reduce(
        &mut self,
        receiver: &Expr,
        args: &[Expr],
        expected_ty: Option<&Type>,
        line: usize,
    ) -> Result<(RegId, Type), LowerError> {
        if args.len() != 2 {
            return Err(LowerError::ArgCountMismatch {
                line,
                func: ".reduce".to_string(),
                expected: 2,
                actual: args.len(),
            });
        }
        let (params, body) = match &args[0].kind {
            ExprKind::Arrow { params, body } => (params, body),
            _ => {
                return Err(LowerError::InvalidConstruct {
                    line: args[0].span.line,
                    msg: "First argument to '.reduce(...)' must be an arrow function '(acc, item, idx?) => ...'"
                        .to_string(),
                });
            }
        };
        if params.len() > 3 {
            return Err(LowerError::InvalidConstruct {
                line: args[0].span.line,
                msg: format!(
                    "'.reduce(...)' callback accepts at most 3 parameters (acc, item, index), got {}",
                    params.len()
                ),
            });
        }

        let (arr_reg, arr_ty) = self.lower_expr(receiver)?;
        let elem_ty = match arr_ty {
            Type::Vec(inner) => *inner,
            other => {
                return Err(LowerError::TypeMismatch {
                    line: receiver.span.line,
                    expected: "vec[...]".to_string(),
                    actual: other.to_string(),
                });
            }
        };
        let arr_is_temp_owned = !self.is_reg_bound_to_var(arr_reg)
            && matches!(
                self.linear_tracker.states.get(&arr_reg),
                Some(LinearState::Alive {
                    local_alloc: true,
                    borrowed: false,
                    ..
                })
            );

        let (init_reg, acc_ty) = self.lower_expr_with_expected(&args[1], expected_ty)?;
        if let Some(exp) = expected_ty {
            if exp != &acc_ty {
                return Err(LowerError::TypeMismatch {
                    line: args[1].span.line,
                    expected: exp.to_string(),
                    actual: acc_ty.to_string(),
                });
            }
        }
        if acc_ty == Type::Void {
            return Err(LowerError::InvalidConstruct {
                line: args[1].span.line,
                msg: "'.reduce(...)' initial value cannot be void".to_string(),
            });
        }
        if acc_ty.is_linear() {
            if self.linear_tracker.is_borrowed(init_reg) {
                return Err(LowerError::InvalidConstruct {
                    line: args[1].span.line,
                    msg: "Cannot move borrowed parameter as initial accumulator to '.reduce(...)'"
                        .to_string(),
                });
            }
            if init_reg == arr_reg {
                return Err(LowerError::InvalidConstruct {
                    line: args[1].span.line,
                    msg: "Cannot pass the array being iterated as the owned linear accumulator in '.reduce(...)'"
                        .to_string(),
                });
            }
            self.linear_tracker.mark_moved(init_reg, line)?;
        }

        let len_reg = self.alloc_reg();
        self.emit(Instruction::new(
            Some(len_reg),
            Type::I64,
            Op::VecLen(arr_reg),
            line,
        ));
        let zero_reg = self.alloc_reg();
        self.emit(Instruction::new(
            Some(zero_reg),
            Type::I64,
            Op::ConstI64(0),
            line,
        ));

        let header_bb = self.alloc_block();
        let body_bb = self.alloc_block();
        let exit_bb = self.alloc_block();

        self.current_block.terminator = Terminator::BrArgs {
            target: header_bb,
            args: vec![zero_reg, init_reg],
        };
        self.finalize_current_block();

        let i_param = self.alloc_reg();
        let acc_param = self.alloc_reg();
        self.start_block_with_params(
            header_bb,
            vec![(i_param, Type::I64), (acc_param, acc_ty.clone())],
        );
        let cond_reg = self.alloc_reg();
        self.emit(Instruction::new(
            Some(cond_reg),
            Type::Bool,
            Op::CmpLt(i_param, len_reg),
            line,
        ));
        self.current_block.terminator = Terminator::BrIfArgs {
            cond: cond_reg,
            then_blk: body_bb,
            then_args: Vec::new(),
            else_blk: exit_bb,
            else_args: vec![acc_param],
        };
        self.finalize_current_block();

        let outer_scopes = self.scopes.clone();
        let outer_tracker = self.linear_tracker.snapshot();
        let pre_body_alive_linear: HashSet<RegId> = outer_tracker
            .states
            .iter()
            .filter_map(|(&r, s)| matches!(s, LinearState::Alive { .. }).then_some(r))
            .collect();

        self.start_block(body_bb);
        let item_reg = self.alloc_reg();
        self.emit(Instruction::new(
            Some(item_reg),
            elem_ty.clone(),
            Op::VecGet(arr_reg, i_param),
            line,
        ));

        self.enter_scope();
        if acc_ty.is_linear() {
            self.linear_tracker
                .mark_alive(acc_param, acc_ty.clone(), true, line);
        }
        if let Some(p0) = params.first() {
            self.bind_var(
                &p0.name,
                acc_param,
                acc_ty.clone(),
                acc_ty.is_linear(),
                p0.span,
            );
        }
        if let Some(p1) = params.get(1) {
            if elem_ty.is_linear() {
                self.bind_borrowed_param(&p1.name, item_reg, elem_ty.clone(), p1.span);
            } else {
                self.bind_var(&p1.name, item_reg, elem_ty.clone(), false, p1.span);
            }
        }
        if let Some(p2) = params.get(2) {
            self.bind_var(&p2.name, i_param, Type::I64, false, p2.span);
        }

        let owned_acc = acc_ty.is_linear().then_some(acc_param);
        let (next_acc_reg, next_acc_ty) =
            self.lower_arrow_body_with_owned(body, Some(&acc_ty), owned_acc, line)?;
        if next_acc_ty != acc_ty {
            return Err(LowerError::TypeMismatch {
                line,
                expected: acc_ty.to_string(),
                actual: next_acc_ty.to_string(),
            });
        }
        if self.linear_tracker.is_borrowed(next_acc_reg) {
            return Err(LowerError::InvalidConstruct {
                line,
                msg: "Cannot return borrowed element directly as accumulator from '.reduce(...)'"
                    .to_string(),
            });
        }
        self.drop_scope_linear_allocations(line, Some(next_acc_reg))?;
        self.leave_scope();

        // If `params` was empty and linear `acc_param` was ignored in an Expr body, drop it cleanly
        if acc_ty.is_linear()
            && next_acc_reg != acc_param
            && self.linear_tracker.is_alive(acc_param)
        {
            self.emit(Instruction::new(None, Type::Void, Op::Free(acc_param), line));
            self.linear_tracker.mark_moved(acc_param, line)?;
        }

        for r in &pre_body_alive_linear {
            if !self.linear_tracker.is_alive(*r) {
                let var_name = self
                    .source_map
                    .get_var(&self.fn_name, *r)
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| format!("%{r}"));
                return Err(LowerError::InvalidConstruct {
                    line,
                    msg: format!(
                        "Cannot move outer linear variable '{var_name}' inside '.reduce(...)' callback"
                    ),
                });
            }
        }

        let one_reg = self.alloc_reg();
        self.emit(Instruction::new(
            Some(one_reg),
            Type::I64,
            Op::ConstI64(1),
            line,
        ));
        let i_next = self.alloc_reg();
        self.emit(Instruction::new(
            Some(i_next),
            Type::I64,
            Op::Add(i_param, one_reg),
            line,
        ));
        self.current_block.terminator = Terminator::BrArgs {
            target: header_bb,
            args: vec![i_next, next_acc_reg],
        };
        self.finalize_current_block();

        let res_reg = self.alloc_reg();
        self.start_block_with_params(exit_bb, vec![(res_reg, acc_ty.clone())]);
        self.scopes = outer_scopes;
        self.linear_tracker.restore(outer_tracker);
        if acc_ty.is_linear() {
            self.linear_tracker
                .mark_alive(res_reg, acc_ty.clone(), true, line);
        }

        if arr_is_temp_owned {
            self.emit(Instruction::new(None, Type::Void, Op::Free(arr_reg), line));
            self.linear_tracker.mark_moved(arr_reg, line)?;
        }

        Ok((res_reg, acc_ty))
    }

    /// Lowers short-circuiting `arr.some((item, idx?) => cond)` (`is_some == true`) and
    /// `arr.every((item, idx?) => cond)` (`is_some == false`) into verified monotonic SSA loops.
    fn lower_array_some_every(
        &mut self,
        receiver: &Expr,
        args: &[Expr],
        is_some: bool,
        line: usize,
    ) -> Result<(RegId, Type), LowerError> {
        let method_name = if is_some { ".some" } else { ".every" };
        if args.len() != 1 {
            return Err(LowerError::ArgCountMismatch {
                line,
                func: method_name.to_string(),
                expected: 1,
                actual: args.len(),
            });
        }
        let (params, body) = match &args[0].kind {
            ExprKind::Arrow { params, body } => (params, body),
            _ => {
                return Err(LowerError::InvalidConstruct {
                    line: args[0].span.line,
                    msg: format!(
                        "Argument to '{method_name}(...)' must be an arrow function '(item, idx?) => cond'"
                    ),
                });
            }
        };
        if params.len() > 2 {
            return Err(LowerError::InvalidConstruct {
                line: args[0].span.line,
                msg: format!(
                    "'{method_name}(...)' callback accepts at most 2 parameters (item, index), got {}",
                    params.len()
                ),
            });
        }

        let (arr_reg, arr_ty) = self.lower_expr(receiver)?;
        let elem_ty = match arr_ty {
            Type::Vec(inner) => *inner,
            other => {
                return Err(LowerError::TypeMismatch {
                    line: receiver.span.line,
                    expected: "vec[...]".to_string(),
                    actual: other.to_string(),
                });
            }
        };
        let arr_is_temp_owned = !self.is_reg_bound_to_var(arr_reg)
            && matches!(
                self.linear_tracker.states.get(&arr_reg),
                Some(LinearState::Alive {
                    local_alloc: true,
                    borrowed: false,
                    ..
                })
            );

        let len_reg = self.alloc_reg();
        self.emit(Instruction::new(
            Some(len_reg),
            Type::I64,
            Op::VecLen(arr_reg),
            line,
        ));
        let zero_reg = self.alloc_reg();
        self.emit(Instruction::new(
            Some(zero_reg),
            Type::I64,
            Op::ConstI64(0),
            line,
        ));
        let default_res = self.alloc_reg();
        self.emit(Instruction::new(
            Some(default_res),
            Type::Bool,
            Op::ConstBool(!is_some),
            line,
        ));
        let early_res = self.alloc_reg();
        self.emit(Instruction::new(
            Some(early_res),
            Type::Bool,
            Op::ConstBool(is_some),
            line,
        ));

        let header_bb = self.alloc_block();
        let body_bb = self.alloc_block();
        let step_bb = self.alloc_block();
        let exit_bb = self.alloc_block();

        self.current_block.terminator = Terminator::BrArgs {
            target: header_bb,
            args: vec![zero_reg],
        };
        self.finalize_current_block();

        let i_param = self.alloc_reg();
        self.start_block_with_params(header_bb, vec![(i_param, Type::I64)]);
        let cond_reg = self.alloc_reg();
        self.emit(Instruction::new(
            Some(cond_reg),
            Type::Bool,
            Op::CmpLt(i_param, len_reg),
            line,
        ));
        self.current_block.terminator = Terminator::BrIfArgs {
            cond: cond_reg,
            then_blk: body_bb,
            then_args: Vec::new(),
            else_blk: exit_bb,
            else_args: vec![default_res],
        };
        self.finalize_current_block();

        let outer_scopes = self.scopes.clone();
        let outer_tracker = self.linear_tracker.snapshot();
        let pre_body_alive_linear: HashSet<RegId> = outer_tracker
            .states
            .iter()
            .filter_map(|(&r, s)| matches!(s, LinearState::Alive { .. }).then_some(r))
            .collect();

        self.start_block(body_bb);
        let item_reg = self.alloc_reg();
        self.emit(Instruction::new(
            Some(item_reg),
            elem_ty.clone(),
            Op::VecGet(arr_reg, i_param),
            line,
        ));
        self.enter_scope();
        self.bind_callback_params(params, item_reg, &elem_ty, i_param);
        let (pred_reg, pred_ty) = self.lower_arrow_body(body, Some(&Type::Bool), line)?;
        self.drop_scope_linear_allocations(line, None)?;
        self.leave_scope();

        if pred_ty != Type::Bool {
            return Err(LowerError::TypeMismatch {
                line,
                expected: "bool".to_string(),
                actual: pred_ty.to_string(),
            });
        }
        for r in &pre_body_alive_linear {
            if !self.linear_tracker.is_alive(*r) {
                let var_name = self
                    .source_map
                    .get_var(&self.fn_name, *r)
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| format!("%{r}"));
                return Err(LowerError::InvalidConstruct {
                    line,
                    msg: format!(
                        "Cannot move outer linear variable '{var_name}' inside '{method_name}(...)' callback"
                    ),
                });
            }
        }

        if is_some {
            self.current_block.terminator = Terminator::BrIfArgs {
                cond: pred_reg,
                then_blk: exit_bb,
                then_args: vec![early_res],
                else_blk: step_bb,
                else_args: Vec::new(),
            };
        } else {
            self.current_block.terminator = Terminator::BrIfArgs {
                cond: pred_reg,
                then_blk: step_bb,
                then_args: Vec::new(),
                else_blk: exit_bb,
                else_args: vec![early_res],
            };
        }
        self.finalize_current_block();

        self.start_block(step_bb);
        let one_reg = self.alloc_reg();
        self.emit(Instruction::new(
            Some(one_reg),
            Type::I64,
            Op::ConstI64(1),
            line,
        ));
        let i_next = self.alloc_reg();
        self.emit(Instruction::new(
            Some(i_next),
            Type::I64,
            Op::Add(i_param, one_reg),
            line,
        ));
        self.current_block.terminator = Terminator::BrArgs {
            target: header_bb,
            args: vec![i_next],
        };
        self.finalize_current_block();

        let res_reg = self.alloc_reg();
        self.start_block_with_params(exit_bb, vec![(res_reg, Type::Bool)]);
        self.scopes = outer_scopes;
        self.linear_tracker.restore(outer_tracker);

        if arr_is_temp_owned {
            self.emit(Instruction::new(None, Type::Void, Op::Free(arr_reg), line));
            self.linear_tracker.mark_moved(arr_reg, line)?;
        }

        Ok((res_reg, Type::Bool))
    }

    /// Lowers `arr.count((item, idx?) => cond)` -> `i64`.
    fn lower_array_count(
        &mut self,
        receiver: &Expr,
        args: &[Expr],
        line: usize,
    ) -> Result<(RegId, Type), LowerError> {
        if args.len() != 1 {
            return Err(LowerError::ArgCountMismatch {
                line,
                func: ".count".to_string(),
                expected: 1,
                actual: args.len(),
            });
        }
        let (params, body) = match &args[0].kind {
            ExprKind::Arrow { params, body } => (params, body),
            _ => {
                return Err(LowerError::InvalidConstruct {
                    line: args[0].span.line,
                    msg: "Argument to '.count(...)' must be an arrow function '(item, idx?) => cond'"
                        .to_string(),
                });
            }
        };
        if params.len() > 2 {
            return Err(LowerError::InvalidConstruct {
                line: args[0].span.line,
                msg: format!(
                    "'.count(...)' callback accepts at most 2 parameters (item, index), got {}",
                    params.len()
                ),
            });
        }

        let (arr_reg, arr_ty) = self.lower_expr(receiver)?;
        let elem_ty = match arr_ty {
            Type::Vec(inner) => *inner,
            other => {
                return Err(LowerError::TypeMismatch {
                    line: receiver.span.line,
                    expected: "vec[...]".to_string(),
                    actual: other.to_string(),
                });
            }
        };
        let arr_is_temp_owned = !self.is_reg_bound_to_var(arr_reg)
            && matches!(
                self.linear_tracker.states.get(&arr_reg),
                Some(LinearState::Alive {
                    local_alloc: true,
                    borrowed: false,
                    ..
                })
            );

        let len_reg = self.alloc_reg();
        self.emit(Instruction::new(
            Some(len_reg),
            Type::I64,
            Op::VecLen(arr_reg),
            line,
        ));
        let zero_i = self.alloc_reg();
        self.emit(Instruction::new(
            Some(zero_i),
            Type::I64,
            Op::ConstI64(0),
            line,
        ));
        let zero_cnt = self.alloc_reg();
        self.emit(Instruction::new(
            Some(zero_cnt),
            Type::I64,
            Op::ConstI64(0),
            line,
        ));

        let header_bb = self.alloc_block();
        let body_bb = self.alloc_block();
        let inc_bb = self.alloc_block();
        let step_bb = self.alloc_block();
        let exit_bb = self.alloc_block();

        self.current_block.terminator = Terminator::BrArgs {
            target: header_bb,
            args: vec![zero_i, zero_cnt],
        };
        self.finalize_current_block();

        let i_param = self.alloc_reg();
        let cnt_param = self.alloc_reg();
        self.start_block_with_params(
            header_bb,
            vec![(i_param, Type::I64), (cnt_param, Type::I64)],
        );
        let cond_reg = self.alloc_reg();
        self.emit(Instruction::new(
            Some(cond_reg),
            Type::Bool,
            Op::CmpLt(i_param, len_reg),
            line,
        ));
        self.current_block.terminator = Terminator::BrIfArgs {
            cond: cond_reg,
            then_blk: body_bb,
            then_args: Vec::new(),
            else_blk: exit_bb,
            else_args: vec![cnt_param],
        };
        self.finalize_current_block();

        let outer_scopes = self.scopes.clone();
        let outer_tracker = self.linear_tracker.snapshot();
        let pre_body_alive_linear: HashSet<RegId> = outer_tracker
            .states
            .iter()
            .filter_map(|(&r, s)| matches!(s, LinearState::Alive { .. }).then_some(r))
            .collect();

        self.start_block(body_bb);
        let item_reg = self.alloc_reg();
        self.emit(Instruction::new(
            Some(item_reg),
            elem_ty.clone(),
            Op::VecGet(arr_reg, i_param),
            line,
        ));
        self.enter_scope();
        self.bind_callback_params(params, item_reg, &elem_ty, i_param);
        let (pred_reg, pred_ty) = self.lower_arrow_body(body, Some(&Type::Bool), line)?;
        self.drop_scope_linear_allocations(line, None)?;
        self.leave_scope();

        if pred_ty != Type::Bool {
            return Err(LowerError::TypeMismatch {
                line,
                expected: "bool".to_string(),
                actual: pred_ty.to_string(),
            });
        }
        for r in &pre_body_alive_linear {
            if !self.linear_tracker.is_alive(*r) {
                let var_name = self
                    .source_map
                    .get_var(&self.fn_name, *r)
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| format!("%{r}"));
                return Err(LowerError::InvalidConstruct {
                    line,
                    msg: format!(
                        "Cannot move outer linear variable '{var_name}' inside '.count(...)' callback"
                    ),
                });
            }
        }

        self.current_block.terminator = Terminator::BrIfArgs {
            cond: pred_reg,
            then_blk: inc_bb,
            then_args: Vec::new(),
            else_blk: step_bb,
            else_args: vec![cnt_param],
        };
        self.finalize_current_block();

        self.start_block(inc_bb);
        let one_c = self.alloc_reg();
        self.emit(Instruction::new(
            Some(one_c),
            Type::I64,
            Op::ConstI64(1),
            line,
        ));
        let cnt_next = self.alloc_reg();
        self.emit(Instruction::new(
            Some(cnt_next),
            Type::I64,
            Op::Add(cnt_param, one_c),
            line,
        ));
        self.current_block.terminator = Terminator::BrArgs {
            target: step_bb,
            args: vec![cnt_next],
        };
        self.finalize_current_block();

        let cnt_after = self.alloc_reg();
        self.start_block_with_params(step_bb, vec![(cnt_after, Type::I64)]);
        let one_i = self.alloc_reg();
        self.emit(Instruction::new(
            Some(one_i),
            Type::I64,
            Op::ConstI64(1),
            line,
        ));
        let i_next = self.alloc_reg();
        self.emit(Instruction::new(
            Some(i_next),
            Type::I64,
            Op::Add(i_param, one_i),
            line,
        ));
        self.current_block.terminator = Terminator::BrArgs {
            target: header_bb,
            args: vec![i_next, cnt_after],
        };
        self.finalize_current_block();

        let res_reg = self.alloc_reg();
        self.start_block_with_params(exit_bb, vec![(res_reg, Type::I64)]);
        self.scopes = outer_scopes;
        self.linear_tracker.restore(outer_tracker);

        if arr_is_temp_owned {
            self.emit(Instruction::new(None, Type::Void, Op::Free(arr_reg), line));
            self.linear_tracker.mark_moved(arr_reg, line)?;
        }

        Ok((res_reg, Type::I64))
    }

    /// Lowers short-circuiting `arr.findIndex((item, idx?) => cond)` -> `i64` (first match index or `-1`).
    fn lower_array_find_index(
        &mut self,
        receiver: &Expr,
        args: &[Expr],
        line: usize,
    ) -> Result<(RegId, Type), LowerError> {
        if args.len() != 1 {
            return Err(LowerError::ArgCountMismatch {
                line,
                func: ".findIndex".to_string(),
                expected: 1,
                actual: args.len(),
            });
        }
        let (params, body) = match &args[0].kind {
            ExprKind::Arrow { params, body } => (params, body),
            _ => {
                return Err(LowerError::InvalidConstruct {
                    line: args[0].span.line,
                    msg: "Argument to '.findIndex(...)' must be an arrow function '(item, idx?) => cond'"
                        .to_string(),
                });
            }
        };
        if params.len() > 2 {
            return Err(LowerError::InvalidConstruct {
                line: args[0].span.line,
                msg: format!(
                    "'.findIndex(...)' callback accepts at most 2 parameters (item, index), got {}",
                    params.len()
                ),
            });
        }

        let (arr_reg, arr_ty) = self.lower_expr(receiver)?;
        let elem_ty = match arr_ty {
            Type::Vec(inner) => *inner,
            other => {
                return Err(LowerError::TypeMismatch {
                    line: receiver.span.line,
                    expected: "vec[...]".to_string(),
                    actual: other.to_string(),
                });
            }
        };
        let arr_is_temp_owned = !self.is_reg_bound_to_var(arr_reg)
            && matches!(
                self.linear_tracker.states.get(&arr_reg),
                Some(LinearState::Alive {
                    local_alloc: true,
                    borrowed: false,
                    ..
                })
            );

        let len_reg = self.alloc_reg();
        self.emit(Instruction::new(
            Some(len_reg),
            Type::I64,
            Op::VecLen(arr_reg),
            line,
        ));
        let zero_reg = self.alloc_reg();
        self.emit(Instruction::new(
            Some(zero_reg),
            Type::I64,
            Op::ConstI64(0),
            line,
        ));
        let neg_one = self.alloc_reg();
        self.emit(Instruction::new(
            Some(neg_one),
            Type::I64,
            Op::ConstI64(-1),
            line,
        ));

        let header_bb = self.alloc_block();
        let body_bb = self.alloc_block();
        let step_bb = self.alloc_block();
        let exit_bb = self.alloc_block();

        self.current_block.terminator = Terminator::BrArgs {
            target: header_bb,
            args: vec![zero_reg],
        };
        self.finalize_current_block();

        let i_param = self.alloc_reg();
        self.start_block_with_params(header_bb, vec![(i_param, Type::I64)]);
        let cond_reg = self.alloc_reg();
        self.emit(Instruction::new(
            Some(cond_reg),
            Type::Bool,
            Op::CmpLt(i_param, len_reg),
            line,
        ));
        self.current_block.terminator = Terminator::BrIfArgs {
            cond: cond_reg,
            then_blk: body_bb,
            then_args: Vec::new(),
            else_blk: exit_bb,
            else_args: vec![neg_one],
        };
        self.finalize_current_block();

        let outer_scopes = self.scopes.clone();
        let outer_tracker = self.linear_tracker.snapshot();
        let pre_body_alive_linear: HashSet<RegId> = outer_tracker
            .states
            .iter()
            .filter_map(|(&r, s)| matches!(s, LinearState::Alive { .. }).then_some(r))
            .collect();

        self.start_block(body_bb);
        let item_reg = self.alloc_reg();
        self.emit(Instruction::new(
            Some(item_reg),
            elem_ty.clone(),
            Op::VecGet(arr_reg, i_param),
            line,
        ));
        self.enter_scope();
        self.bind_callback_params(params, item_reg, &elem_ty, i_param);
        let (pred_reg, pred_ty) = self.lower_arrow_body(body, Some(&Type::Bool), line)?;
        self.drop_scope_linear_allocations(line, None)?;
        self.leave_scope();

        if pred_ty != Type::Bool {
            return Err(LowerError::TypeMismatch {
                line,
                expected: "bool".to_string(),
                actual: pred_ty.to_string(),
            });
        }
        for r in &pre_body_alive_linear {
            if !self.linear_tracker.is_alive(*r) {
                let var_name = self
                    .source_map
                    .get_var(&self.fn_name, *r)
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| format!("%{r}"));
                return Err(LowerError::InvalidConstruct {
                    line,
                    msg: format!(
                        "Cannot move outer linear variable '{var_name}' inside '.findIndex(...)' callback"
                    ),
                });
            }
        }

        self.current_block.terminator = Terminator::BrIfArgs {
            cond: pred_reg,
            then_blk: exit_bb,
            then_args: vec![i_param],
            else_blk: step_bb,
            else_args: Vec::new(),
        };
        self.finalize_current_block();

        self.start_block(step_bb);
        let one_reg = self.alloc_reg();
        self.emit(Instruction::new(
            Some(one_reg),
            Type::I64,
            Op::ConstI64(1),
            line,
        ));
        let i_next = self.alloc_reg();
        self.emit(Instruction::new(
            Some(i_next),
            Type::I64,
            Op::Add(i_param, one_reg),
            line,
        ));
        self.current_block.terminator = Terminator::BrArgs {
            target: header_bb,
            args: vec![i_next],
        };
        self.finalize_current_block();

        let res_reg = self.alloc_reg();
        self.start_block_with_params(exit_bb, vec![(res_reg, Type::I64)]);
        self.scopes = outer_scopes;
        self.linear_tracker.restore(outer_tracker);

        if arr_is_temp_owned {
            self.emit(Instruction::new(None, Type::Void, Op::Free(arr_reg), line));
            self.linear_tracker.mark_moved(arr_reg, line)?;
        }

        Ok((res_reg, Type::I64))
    }

    /// Lowers short-circuiting `arr.includes(val)` -> `bool` (`is_includes == true`) and
    /// `arr.indexOf(val)` -> `i64` (`is_includes == false`) for `i64`, `f64`, `bool`, `string`.
    fn lower_array_includes_index_of(
        &mut self,
        receiver: &Expr,
        args: &[Expr],
        is_includes: bool,
        line: usize,
    ) -> Result<(RegId, Type), LowerError> {
        let method_name = if is_includes { ".includes" } else { ".indexOf" };
        if args.len() != 1 {
            return Err(LowerError::ArgCountMismatch {
                line,
                func: method_name.to_string(),
                expected: 1,
                actual: args.len(),
            });
        }

        let (arr_reg, arr_ty) = self.lower_expr(receiver)?;
        let elem_ty = match arr_ty {
            Type::Vec(inner) => *inner,
            other => {
                return Err(LowerError::TypeMismatch {
                    line: receiver.span.line,
                    expected: "vec[...]".to_string(),
                    actual: other.to_string(),
                });
            }
        };
        if !matches!(elem_ty, Type::I64 | Type::F64 | Type::Bool | Type::Str) {
            return Err(LowerError::InvalidConstruct {
                line,
                msg: format!(
                    "'{method_name}(...)' requires comparable element type (i64, f64, bool, string), found '{elem_ty}'"
                ),
            });
        }
        let arr_is_temp_owned = !self.is_reg_bound_to_var(arr_reg)
            && matches!(
                self.linear_tracker.states.get(&arr_reg),
                Some(LinearState::Alive {
                    local_alloc: true,
                    borrowed: false,
                    ..
                })
            );

        let (val_reg, val_ty) = self.lower_expr_with_expected(&args[0], Some(&elem_ty))?;
        if val_ty != elem_ty {
            return Err(LowerError::TypeMismatch {
                line: args[0].span.line,
                expected: elem_ty.to_string(),
                actual: val_ty.to_string(),
            });
        }

        let ret_ty = if is_includes { Type::Bool } else { Type::I64 };
        let len_reg = self.alloc_reg();
        self.emit(Instruction::new(
            Some(len_reg),
            Type::I64,
            Op::VecLen(arr_reg),
            line,
        ));
        let zero_reg = self.alloc_reg();
        self.emit(Instruction::new(
            Some(zero_reg),
            Type::I64,
            Op::ConstI64(0),
            line,
        ));
        let not_found_reg = self.alloc_reg();
        if is_includes {
            self.emit(Instruction::new(
                Some(not_found_reg),
                Type::Bool,
                Op::ConstBool(false),
                line,
            ));
        } else {
            self.emit(Instruction::new(
                Some(not_found_reg),
                Type::I64,
                Op::ConstI64(-1),
                line,
            ));
        }
        let found_bool_reg = if is_includes {
            let r = self.alloc_reg();
            self.emit(Instruction::new(
                Some(r),
                Type::Bool,
                Op::ConstBool(true),
                line,
            ));
            Some(r)
        } else {
            None
        };

        let header_bb = self.alloc_block();
        let body_bb = self.alloc_block();
        let step_bb = self.alloc_block();
        let exit_bb = self.alloc_block();

        self.current_block.terminator = Terminator::BrArgs {
            target: header_bb,
            args: vec![zero_reg],
        };
        self.finalize_current_block();

        let i_param = self.alloc_reg();
        self.start_block_with_params(header_bb, vec![(i_param, Type::I64)]);
        let cond_reg = self.alloc_reg();
        self.emit(Instruction::new(
            Some(cond_reg),
            Type::Bool,
            Op::CmpLt(i_param, len_reg),
            line,
        ));
        self.current_block.terminator = Terminator::BrIfArgs {
            cond: cond_reg,
            then_blk: body_bb,
            then_args: Vec::new(),
            else_blk: exit_bb,
            else_args: vec![not_found_reg],
        };
        self.finalize_current_block();

        self.start_block(body_bb);
        let item_reg = self.alloc_reg();
        self.emit(Instruction::new(
            Some(item_reg),
            elem_ty,
            Op::VecGet(arr_reg, i_param),
            line,
        ));
        let eq_reg = self.alloc_reg();
        self.emit(Instruction::new(
            Some(eq_reg),
            Type::Bool,
            Op::CmpEq(item_reg, val_reg),
            line,
        ));
        let hit_val = found_bool_reg.unwrap_or(i_param);
        self.current_block.terminator = Terminator::BrIfArgs {
            cond: eq_reg,
            then_blk: exit_bb,
            then_args: vec![hit_val],
            else_blk: step_bb,
            else_args: Vec::new(),
        };
        self.finalize_current_block();

        self.start_block(step_bb);
        let one_reg = self.alloc_reg();
        self.emit(Instruction::new(
            Some(one_reg),
            Type::I64,
            Op::ConstI64(1),
            line,
        ));
        let i_next = self.alloc_reg();
        self.emit(Instruction::new(
            Some(i_next),
            Type::I64,
            Op::Add(i_param, one_reg),
            line,
        ));
        self.current_block.terminator = Terminator::BrArgs {
            target: header_bb,
            args: vec![i_next],
        };
        self.finalize_current_block();

        let res_reg = self.alloc_reg();
        self.start_block_with_params(exit_bb, vec![(res_reg, ret_ty.clone())]);

        if arr_is_temp_owned {
            self.emit(Instruction::new(None, Type::Void, Op::Free(arr_reg), line));
            self.linear_tracker.mark_moved(arr_reg, line)?;
        }

        Ok((res_reg, ret_ty))
    }

    /// Lowers `arr.flatMap((item, idx?) => sub_arr)` -> `U[]` via nested verified monotonic SSA loops.
    fn lower_array_flat_map(
        &mut self,
        receiver: &Expr,
        args: &[Expr],
        expected_ty: Option<&Type>,
        line: usize,
    ) -> Result<(RegId, Type), LowerError> {
        if args.len() != 1 {
            return Err(LowerError::ArgCountMismatch {
                line,
                func: ".flatMap".to_string(),
                expected: 1,
                actual: args.len(),
            });
        }
        let (params, body) = match &args[0].kind {
            ExprKind::Arrow { params, body } => (params, body),
            _ => {
                return Err(LowerError::InvalidConstruct {
                    line: args[0].span.line,
                    msg: "Argument to '.flatMap(...)' must be an arrow function '(item, idx?) => U[]'"
                        .to_string(),
                });
            }
        };
        if params.len() > 2 {
            return Err(LowerError::InvalidConstruct {
                line: args[0].span.line,
                msg: format!(
                    "'.flatMap(...)' callback accepts at most 2 parameters (item, index), got {}",
                    params.len()
                ),
            });
        }

        let (arr_reg, arr_ty) = self.lower_expr(receiver)?;
        let elem_ty = match arr_ty {
            Type::Vec(inner) => *inner,
            other => {
                return Err(LowerError::TypeMismatch {
                    line: receiver.span.line,
                    expected: "vec[...]".to_string(),
                    actual: other.to_string(),
                });
            }
        };
        let arr_is_temp_owned = !self.is_reg_bound_to_var(arr_reg)
            && matches!(
                self.linear_tracker.states.get(&arr_reg),
                Some(LinearState::Alive {
                    local_alloc: true,
                    borrowed: false,
                    ..
                })
            );

        let placeholder_elem_ty = match expected_ty {
            Some(Type::Vec(inner)) => (**inner).clone(),
            _ => Type::Void,
        };
        let placeholder_vec_ty = Type::Vec(Box::new(placeholder_elem_ty.clone()));

        let len_reg = self.alloc_reg();
        self.emit(Instruction::new(
            Some(len_reg),
            Type::I64,
            Op::VecLen(arr_reg),
            line,
        ));
        let zero_i = self.alloc_reg();
        self.emit(Instruction::new(
            Some(zero_i),
            Type::I64,
            Op::ConstI64(0),
            line,
        ));

        let out0_inst_idx = self.current_block.instructions.len();
        let out0_reg = self.alloc_reg();
        self.emit(Instruction::new(
            Some(out0_reg),
            placeholder_vec_ty.clone(),
            Op::VecNew(placeholder_elem_ty),
            line,
        ));

        let outer_header_bb = self.alloc_block();
        let outer_body_bb = self.alloc_block();
        let inner_header_bb = self.alloc_block();
        let inner_body_bb = self.alloc_block();
        let inner_exit_bb = self.alloc_block();
        let outer_exit_bb = self.alloc_block();

        self.current_block.terminator = Terminator::BrArgs {
            target: outer_header_bb,
            args: vec![zero_i, out0_reg],
        };
        let pre_block_idx = self.completed_blocks.len();
        self.finalize_current_block();

        let i_param = self.alloc_reg();
        let out_param = self.alloc_reg();
        self.start_block_with_params(
            outer_header_bb,
            vec![(i_param, Type::I64), (out_param, placeholder_vec_ty)],
        );
        let cond_i = self.alloc_reg();
        self.emit(Instruction::new(
            Some(cond_i),
            Type::Bool,
            Op::CmpLt(i_param, len_reg),
            line,
        ));
        self.current_block.terminator = Terminator::BrIfArgs {
            cond: cond_i,
            then_blk: outer_body_bb,
            then_args: Vec::new(),
            else_blk: outer_exit_bb,
            else_args: vec![out_param],
        };
        let header_block_idx = self.completed_blocks.len();
        self.finalize_current_block();

        let outer_scopes = self.scopes.clone();
        let outer_tracker = self.linear_tracker.snapshot();
        let pre_body_alive_linear: HashSet<RegId> = outer_tracker
            .states
            .iter()
            .filter_map(|(&r, s)| matches!(s, LinearState::Alive { .. }).then_some(r))
            .collect();

        self.start_block(outer_body_bb);
        let item_reg = self.alloc_reg();
        self.emit(Instruction::new(
            Some(item_reg),
            elem_ty.clone(),
            Op::VecGet(arr_reg, i_param),
            line,
        ));

        self.enter_scope();
        self.bind_callback_params(params, item_reg, &elem_ty, i_param);
        let expected_sub = match expected_ty {
            Some(Type::Vec(_)) => expected_ty,
            _ => None,
        };
        let (sub_reg, sub_ty) = self.lower_arrow_body(body, expected_sub, line)?;
        self.drop_scope_linear_allocations(line, Some(sub_reg))?;
        self.leave_scope();

        let out_elem_ty = match sub_ty {
            Type::Vec(inner) => *inner,
            other => {
                return Err(LowerError::TypeMismatch {
                    line,
                    expected: "vec[...]".to_string(),
                    actual: other.to_string(),
                });
            }
        };
        let out_vec_ty = Type::Vec(Box::new(out_elem_ty.clone()));
        if let Some(exp) = expected_ty {
            if exp != &out_vec_ty {
                return Err(LowerError::TypeMismatch {
                    line,
                    expected: exp.to_string(),
                    actual: out_vec_ty.to_string(),
                });
            }
        }

        let sub_is_temp_owned = !self.is_reg_bound_to_var(sub_reg)
            && matches!(
                self.linear_tracker.states.get(&sub_reg),
                Some(LinearState::Alive {
                    local_alloc: true,
                    borrowed: false,
                    ..
                })
            );
        if out_elem_ty.is_linear() && !sub_is_temp_owned {
            return Err(LowerError::InvalidConstruct {
                line,
                msg: format!(
                    "Cannot flatten borrowed array of linear element type '{out_elem_ty}' in '.flatMap(...)'"
                ),
            });
        }

        for r in &pre_body_alive_linear {
            if !self.linear_tracker.is_alive(*r) {
                let var_name = self
                    .source_map
                    .get_var(&self.fn_name, *r)
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| format!("%{r}"));
                return Err(LowerError::InvalidConstruct {
                    line,
                    msg: format!(
                        "Cannot move outer linear variable '{var_name}' inside '.flatMap(...)' callback"
                    ),
                });
            }
        }

        // Back-patch `VecNew` and `outer_header_bb` parameter with the exact inferred element type
        self.completed_blocks[pre_block_idx].instructions[out0_inst_idx].ty = out_vec_ty.clone();
        self.completed_blocks[pre_block_idx].instructions[out0_inst_idx].op =
            Op::VecNew(out_elem_ty.clone());
        self.completed_blocks[header_block_idx].params[1].1 = out_vec_ty.clone();

        let sub_len = self.alloc_reg();
        self.emit(Instruction::new(
            Some(sub_len),
            Type::I64,
            Op::VecLen(sub_reg),
            line,
        ));
        let zero_j = self.alloc_reg();
        self.emit(Instruction::new(
            Some(zero_j),
            Type::I64,
            Op::ConstI64(0),
            line,
        ));
        self.current_block.terminator = Terminator::BrArgs {
            target: inner_header_bb,
            args: vec![zero_j, out_param],
        };
        self.finalize_current_block();

        let j_param = self.alloc_reg();
        let inner_out = self.alloc_reg();
        self.start_block_with_params(
            inner_header_bb,
            vec![(j_param, Type::I64), (inner_out, out_vec_ty.clone())],
        );
        let cond_j = self.alloc_reg();
        self.emit(Instruction::new(
            Some(cond_j),
            Type::Bool,
            Op::CmpLt(j_param, sub_len),
            line,
        ));
        self.current_block.terminator = Terminator::BrIfArgs {
            cond: cond_j,
            then_blk: inner_body_bb,
            then_args: Vec::new(),
            else_blk: inner_exit_bb,
            else_args: vec![inner_out],
        };
        self.finalize_current_block();

        self.start_block(inner_body_bb);
        let sub_el = self.alloc_reg();
        self.emit(Instruction::new(
            Some(sub_el),
            out_elem_ty,
            Op::VecGet(sub_reg, j_param),
            line,
        ));
        let inner_out_next = self.alloc_reg();
        self.emit(Instruction::new(
            Some(inner_out_next),
            out_vec_ty.clone(),
            Op::VecPush(inner_out, sub_el),
            line,
        ));
        let one_j = self.alloc_reg();
        self.emit(Instruction::new(
            Some(one_j),
            Type::I64,
            Op::ConstI64(1),
            line,
        ));
        let j_next = self.alloc_reg();
        self.emit(Instruction::new(
            Some(j_next),
            Type::I64,
            Op::Add(j_param, one_j),
            line,
        ));
        self.current_block.terminator = Terminator::BrArgs {
            target: inner_header_bb,
            args: vec![j_next, inner_out_next],
        };
        self.finalize_current_block();

        let out_after_sub = self.alloc_reg();
        self.start_block_with_params(inner_exit_bb, vec![(out_after_sub, out_vec_ty.clone())]);
        if sub_is_temp_owned {
            self.emit(Instruction::new(None, Type::Void, Op::Free(sub_reg), line));
            self.linear_tracker.mark_moved(sub_reg, line)?;
        }
        let one_i = self.alloc_reg();
        self.emit(Instruction::new(
            Some(one_i),
            Type::I64,
            Op::ConstI64(1),
            line,
        ));
        let i_next = self.alloc_reg();
        self.emit(Instruction::new(
            Some(i_next),
            Type::I64,
            Op::Add(i_param, one_i),
            line,
        ));
        self.current_block.terminator = Terminator::BrArgs {
            target: outer_header_bb,
            args: vec![i_next, out_after_sub],
        };
        self.finalize_current_block();

        let res_reg = self.alloc_reg();
        self.start_block_with_params(outer_exit_bb, vec![(res_reg, out_vec_ty.clone())]);
        self.scopes = outer_scopes;
        self.linear_tracker.restore(outer_tracker);
        self.linear_tracker
            .mark_alive(res_reg, out_vec_ty.clone(), true, line);

        if arr_is_temp_owned {
            self.emit(Instruction::new(None, Type::Void, Op::Free(arr_reg), line));
            self.linear_tracker.mark_moved(arr_reg, line)?;
        }

        Ok((res_reg, out_vec_ty))
    }

    /// Clamps an `i64` register safely to `[lo, hi]` using distinct CFG blocks (no same-target BrIfArgs).
    fn emit_clamp_i64(&mut self, val: RegId, lo: RegId, hi: RegId, line: usize) -> RegId {
        let lt_lo = self.alloc_reg();
        self.emit(Instruction::new(
            Some(lt_lo),
            Type::Bool,
            Op::CmpLt(val, lo),
            line,
        ));
        let clamp_lo_bb = self.alloc_block();
        let after_lo_bb = self.alloc_block();
        self.current_block.terminator = Terminator::BrIfArgs {
            cond: lt_lo,
            then_blk: clamp_lo_bb,
            then_args: Vec::new(),
            else_blk: after_lo_bb,
            else_args: vec![val],
        };
        self.finalize_current_block();

        self.start_block(clamp_lo_bb);
        self.current_block.terminator = Terminator::BrArgs {
            target: after_lo_bb,
            args: vec![lo],
        };
        self.finalize_current_block();

        let v1 = self.alloc_reg();
        self.start_block_with_params(after_lo_bb, vec![(v1, Type::I64)]);
        let gt_hi = self.alloc_reg();
        self.emit(Instruction::new(
            Some(gt_hi),
            Type::Bool,
            Op::CmpLt(hi, v1),
            line,
        ));
        let clamp_hi_bb = self.alloc_block();
        let after_hi_bb = self.alloc_block();
        self.current_block.terminator = Terminator::BrIfArgs {
            cond: gt_hi,
            then_blk: clamp_hi_bb,
            then_args: Vec::new(),
            else_blk: after_hi_bb,
            else_args: vec![v1],
        };
        self.finalize_current_block();

        self.start_block(clamp_hi_bb);
        self.current_block.terminator = Terminator::BrArgs {
            target: after_hi_bb,
            args: vec![hi],
        };
        self.finalize_current_block();

        let v2 = self.alloc_reg();
        self.start_block_with_params(after_hi_bb, vec![(v2, Type::I64)]);
        v2
    }

    /// Lowers `arr.slice(start, end?)` -> cloned sub-slice `T[]` with indices clamped safely to `[0, len]`.
    fn lower_array_slice(
        &mut self,
        receiver: &Expr,
        args: &[Expr],
        expected_ty: Option<&Type>,
        line: usize,
    ) -> Result<(RegId, Type), LowerError> {
        if args.is_empty() || args.len() > 2 {
            return Err(LowerError::ArgCountMismatch {
                line,
                func: ".slice".to_string(),
                expected: 2,
                actual: args.len(),
            });
        }

        let (arr_reg, arr_ty) = self.lower_expr(receiver)?;
        let elem_ty = match &arr_ty {
            Type::Vec(inner) => (**inner).clone(),
            other => {
                return Err(LowerError::TypeMismatch {
                    line: receiver.span.line,
                    expected: "vec[...]".to_string(),
                    actual: other.to_string(),
                });
            }
        };
        if let Some(exp) = expected_ty {
            if exp != &arr_ty {
                return Err(LowerError::TypeMismatch {
                    line,
                    expected: exp.to_string(),
                    actual: arr_ty.to_string(),
                });
            }
        }
        let arr_is_temp_owned = !self.is_reg_bound_to_var(arr_reg)
            && matches!(
                self.linear_tracker.states.get(&arr_reg),
                Some(LinearState::Alive {
                    local_alloc: true,
                    borrowed: false,
                    ..
                })
            );
        if elem_ty.is_linear() && !arr_is_temp_owned {
            return Err(LowerError::InvalidConstruct {
                line,
                msg: format!(
                    "'.slice(...)' requires a Copy element type (i64, f64, bool, string); cannot alias linear element type '{elem_ty}'"
                ),
            });
        }

        let (raw_start, start_ty) = self.lower_expr_with_expected(&args[0], Some(&Type::I64))?;
        if start_ty != Type::I64 {
            return Err(LowerError::TypeMismatch {
                line: args[0].span.line,
                expected: "i64".to_string(),
                actual: start_ty.to_string(),
            });
        }

        let len_reg = self.alloc_reg();
        self.emit(Instruction::new(
            Some(len_reg),
            Type::I64,
            Op::VecLen(arr_reg),
            line,
        ));
        let zero_reg = self.alloc_reg();
        self.emit(Instruction::new(
            Some(zero_reg),
            Type::I64,
            Op::ConstI64(0),
            line,
        ));

        let raw_end = if let Some(end_expr) = args.get(1) {
            let (r, ty) = self.lower_expr_with_expected(end_expr, Some(&Type::I64))?;
            if ty != Type::I64 {
                return Err(LowerError::TypeMismatch {
                    line: end_expr.span.line,
                    expected: "i64".to_string(),
                    actual: ty.to_string(),
                });
            }
            r
        } else {
            len_reg
        };

        let start_clamped = self.emit_clamp_i64(raw_start, zero_reg, len_reg, line);
        let end_clamped = self.emit_clamp_i64(raw_end, zero_reg, len_reg, line);

        let out0_reg = self.alloc_reg();
        self.emit(Instruction::new(
            Some(out0_reg),
            arr_ty.clone(),
            Op::VecNew(elem_ty.clone()),
            line,
        ));

        let header_bb = self.alloc_block();
        let body_bb = self.alloc_block();
        let exit_bb = self.alloc_block();

        self.current_block.terminator = Terminator::BrArgs {
            target: header_bb,
            args: vec![start_clamped, out0_reg],
        };
        self.finalize_current_block();

        let i_param = self.alloc_reg();
        let out_param = self.alloc_reg();
        self.start_block_with_params(
            header_bb,
            vec![(i_param, Type::I64), (out_param, arr_ty.clone())],
        );
        let cond_reg = self.alloc_reg();
        self.emit(Instruction::new(
            Some(cond_reg),
            Type::Bool,
            Op::CmpLt(i_param, end_clamped),
            line,
        ));
        self.current_block.terminator = Terminator::BrIfArgs {
            cond: cond_reg,
            then_blk: body_bb,
            then_args: Vec::new(),
            else_blk: exit_bb,
            else_args: vec![out_param],
        };
        self.finalize_current_block();

        self.start_block(body_bb);
        let item_reg = self.alloc_reg();
        self.emit(Instruction::new(
            Some(item_reg),
            elem_ty,
            Op::VecGet(arr_reg, i_param),
            line,
        ));
        let out_next = self.alloc_reg();
        self.emit(Instruction::new(
            Some(out_next),
            arr_ty.clone(),
            Op::VecPush(out_param, item_reg),
            line,
        ));
        let one_reg = self.alloc_reg();
        self.emit(Instruction::new(
            Some(one_reg),
            Type::I64,
            Op::ConstI64(1),
            line,
        ));
        let i_next = self.alloc_reg();
        self.emit(Instruction::new(
            Some(i_next),
            Type::I64,
            Op::Add(i_param, one_reg),
            line,
        ));
        self.current_block.terminator = Terminator::BrArgs {
            target: header_bb,
            args: vec![i_next, out_next],
        };
        self.finalize_current_block();

        let res_reg = self.alloc_reg();
        self.start_block_with_params(exit_bb, vec![(res_reg, arr_ty.clone())]);
        self.linear_tracker
            .mark_alive(res_reg, arr_ty.clone(), true, line);

        if arr_is_temp_owned {
            self.emit(Instruction::new(None, Type::Void, Op::Free(arr_reg), line));
            self.linear_tracker.mark_moved(arr_reg, line)?;
        }

        Ok((res_reg, arr_ty))
    }

    /// Lowers `arr.concat(other)` -> combined `T[]` without mutating inputs.
    fn lower_array_concat(
        &mut self,
        receiver: &Expr,
        args: &[Expr],
        expected_ty: Option<&Type>,
        line: usize,
    ) -> Result<(RegId, Type), LowerError> {
        if args.len() != 1 {
            return Err(LowerError::ArgCountMismatch {
                line,
                func: ".concat".to_string(),
                expected: 1,
                actual: args.len(),
            });
        }

        let (arr_reg, arr_ty) = self.lower_expr_with_expected(receiver, expected_ty)?;
        let elem_ty = match &arr_ty {
            Type::Vec(inner) => (**inner).clone(),
            other => {
                return Err(LowerError::TypeMismatch {
                    line: receiver.span.line,
                    expected: "vec[...]".to_string(),
                    actual: other.to_string(),
                });
            }
        };
        if let Some(exp) = expected_ty {
            if exp != &arr_ty {
                return Err(LowerError::TypeMismatch {
                    line,
                    expected: exp.to_string(),
                    actual: arr_ty.to_string(),
                });
            }
        }
        let arr_is_temp_owned = !self.is_reg_bound_to_var(arr_reg)
            && matches!(
                self.linear_tracker.states.get(&arr_reg),
                Some(LinearState::Alive {
                    local_alloc: true,
                    borrowed: false,
                    ..
                })
            );

        let (other_reg, other_ty) = self.lower_expr_with_expected(&args[0], Some(&arr_ty))?;
        if other_ty != arr_ty {
            return Err(LowerError::TypeMismatch {
                line: args[0].span.line,
                expected: arr_ty.to_string(),
                actual: other_ty.to_string(),
            });
        }
        let other_is_temp_owned = !self.is_reg_bound_to_var(other_reg)
            && matches!(
                self.linear_tracker.states.get(&other_reg),
                Some(LinearState::Alive {
                    local_alloc: true,
                    borrowed: false,
                    ..
                })
            );

        if elem_ty.is_linear() && (!arr_is_temp_owned || !other_is_temp_owned) {
            return Err(LowerError::InvalidConstruct {
                line,
                msg: format!(
                    "'.concat(...)' requires a Copy element type (i64, f64, bool, string); cannot alias linear element type '{elem_ty}'"
                ),
            });
        }

        let out0_reg = self.alloc_reg();
        self.emit(Instruction::new(
            Some(out0_reg),
            arr_ty.clone(),
            Op::VecNew(elem_ty.clone()),
            line,
        ));

        // Copy loop 1: receiver elements
        let len1 = self.alloc_reg();
        self.emit(Instruction::new(
            Some(len1),
            Type::I64,
            Op::VecLen(arr_reg),
            line,
        ));
        let zero1 = self.alloc_reg();
        self.emit(Instruction::new(
            Some(zero1),
            Type::I64,
            Op::ConstI64(0),
            line,
        ));
        let h1 = self.alloc_block();
        let b1 = self.alloc_block();
        let mid_bb = self.alloc_block();

        self.current_block.terminator = Terminator::BrArgs {
            target: h1,
            args: vec![zero1, out0_reg],
        };
        self.finalize_current_block();

        let i1 = self.alloc_reg();
        let acc1 = self.alloc_reg();
        self.start_block_with_params(h1, vec![(i1, Type::I64), (acc1, arr_ty.clone())]);
        let c1 = self.alloc_reg();
        self.emit(Instruction::new(
            Some(c1),
            Type::Bool,
            Op::CmpLt(i1, len1),
            line,
        ));
        self.current_block.terminator = Terminator::BrIfArgs {
            cond: c1,
            then_blk: b1,
            then_args: Vec::new(),
            else_blk: mid_bb,
            else_args: vec![acc1],
        };
        self.finalize_current_block();

        self.start_block(b1);
        let el1 = self.alloc_reg();
        self.emit(Instruction::new(
            Some(el1),
            elem_ty.clone(),
            Op::VecGet(arr_reg, i1),
            line,
        ));
        let acc1_next = self.alloc_reg();
        self.emit(Instruction::new(
            Some(acc1_next),
            arr_ty.clone(),
            Op::VecPush(acc1, el1),
            line,
        ));
        let one1 = self.alloc_reg();
        self.emit(Instruction::new(
            Some(one1),
            Type::I64,
            Op::ConstI64(1),
            line,
        ));
        let i1_next = self.alloc_reg();
        self.emit(Instruction::new(
            Some(i1_next),
            Type::I64,
            Op::Add(i1, one1),
            line,
        ));
        self.current_block.terminator = Terminator::BrArgs {
            target: h1,
            args: vec![i1_next, acc1_next],
        };
        self.finalize_current_block();

        // Copy loop 2: other elements
        let mid_vec = self.alloc_reg();
        self.start_block_with_params(mid_bb, vec![(mid_vec, arr_ty.clone())]);
        let len2 = self.alloc_reg();
        self.emit(Instruction::new(
            Some(len2),
            Type::I64,
            Op::VecLen(other_reg),
            line,
        ));
        let zero2 = self.alloc_reg();
        self.emit(Instruction::new(
            Some(zero2),
            Type::I64,
            Op::ConstI64(0),
            line,
        ));
        let h2 = self.alloc_block();
        let b2 = self.alloc_block();
        let exit_bb = self.alloc_block();

        self.current_block.terminator = Terminator::BrArgs {
            target: h2,
            args: vec![zero2, mid_vec],
        };
        self.finalize_current_block();

        let i2 = self.alloc_reg();
        let acc2 = self.alloc_reg();
        self.start_block_with_params(h2, vec![(i2, Type::I64), (acc2, arr_ty.clone())]);
        let c2 = self.alloc_reg();
        self.emit(Instruction::new(
            Some(c2),
            Type::Bool,
            Op::CmpLt(i2, len2),
            line,
        ));
        self.current_block.terminator = Terminator::BrIfArgs {
            cond: c2,
            then_blk: b2,
            then_args: Vec::new(),
            else_blk: exit_bb,
            else_args: vec![acc2],
        };
        self.finalize_current_block();

        self.start_block(b2);
        let el2 = self.alloc_reg();
        self.emit(Instruction::new(
            Some(el2),
            elem_ty,
            Op::VecGet(other_reg, i2),
            line,
        ));
        let acc2_next = self.alloc_reg();
        self.emit(Instruction::new(
            Some(acc2_next),
            arr_ty.clone(),
            Op::VecPush(acc2, el2),
            line,
        ));
        let one2 = self.alloc_reg();
        self.emit(Instruction::new(
            Some(one2),
            Type::I64,
            Op::ConstI64(1),
            line,
        ));
        let i2_next = self.alloc_reg();
        self.emit(Instruction::new(
            Some(i2_next),
            Type::I64,
            Op::Add(i2, one2),
            line,
        ));
        self.current_block.terminator = Terminator::BrArgs {
            target: h2,
            args: vec![i2_next, acc2_next],
        };
        self.finalize_current_block();

        let res_reg = self.alloc_reg();
        self.start_block_with_params(exit_bb, vec![(res_reg, arr_ty.clone())]);
        self.linear_tracker
            .mark_alive(res_reg, arr_ty.clone(), true, line);

        if arr_is_temp_owned {
            self.emit(Instruction::new(None, Type::Void, Op::Free(arr_reg), line));
            self.linear_tracker.mark_moved(arr_reg, line)?;
        }
        if other_is_temp_owned && other_reg != arr_reg {
            self.emit(Instruction::new(
                None,
                Type::Void,
                Op::Free(other_reg),
                line,
            ));
            self.linear_tracker.mark_moved(other_reg, line)?;
        }

        Ok((res_reg, arr_ty))
    }

    /// Lowers `arr.reverse()` -> reversed `T[]` without mutating input.
    fn lower_array_reverse(
        &mut self,
        receiver: &Expr,
        args: &[Expr],
        expected_ty: Option<&Type>,
        line: usize,
    ) -> Result<(RegId, Type), LowerError> {
        if !args.is_empty() {
            return Err(LowerError::ArgCountMismatch {
                line,
                func: ".reverse".to_string(),
                expected: 0,
                actual: args.len(),
            });
        }

        let (arr_reg, arr_ty) = self.lower_expr_with_expected(receiver, expected_ty)?;
        let elem_ty = match &arr_ty {
            Type::Vec(inner) => (**inner).clone(),
            other => {
                return Err(LowerError::TypeMismatch {
                    line: receiver.span.line,
                    expected: "vec[...]".to_string(),
                    actual: other.to_string(),
                });
            }
        };
        if let Some(exp) = expected_ty {
            if exp != &arr_ty {
                return Err(LowerError::TypeMismatch {
                    line,
                    expected: exp.to_string(),
                    actual: arr_ty.to_string(),
                });
            }
        }
        let arr_is_temp_owned = !self.is_reg_bound_to_var(arr_reg)
            && matches!(
                self.linear_tracker.states.get(&arr_reg),
                Some(LinearState::Alive {
                    local_alloc: true,
                    borrowed: false,
                    ..
                })
            );
        if elem_ty.is_linear() && !arr_is_temp_owned {
            return Err(LowerError::InvalidConstruct {
                line,
                msg: format!(
                    "'.reverse()' requires a Copy element type (i64, f64, bool, string); cannot alias linear element type '{elem_ty}'"
                ),
            });
        }

        let len_reg = self.alloc_reg();
        self.emit(Instruction::new(
            Some(len_reg),
            Type::I64,
            Op::VecLen(arr_reg),
            line,
        ));
        let one_init = self.alloc_reg();
        self.emit(Instruction::new(
            Some(one_init),
            Type::I64,
            Op::ConstI64(1),
            line,
        ));
        let start_i = self.alloc_reg();
        self.emit(Instruction::new(
            Some(start_i),
            Type::I64,
            Op::Sub(len_reg, one_init),
            line,
        ));
        let zero_reg = self.alloc_reg();
        self.emit(Instruction::new(
            Some(zero_reg),
            Type::I64,
            Op::ConstI64(0),
            line,
        ));
        let out0_reg = self.alloc_reg();
        self.emit(Instruction::new(
            Some(out0_reg),
            arr_ty.clone(),
            Op::VecNew(elem_ty.clone()),
            line,
        ));

        let header_bb = self.alloc_block();
        let body_bb = self.alloc_block();
        let exit_bb = self.alloc_block();

        self.current_block.terminator = Terminator::BrArgs {
            target: header_bb,
            args: vec![start_i, out0_reg],
        };
        self.finalize_current_block();

        let i_param = self.alloc_reg();
        let out_param = self.alloc_reg();
        self.start_block_with_params(
            header_bb,
            vec![(i_param, Type::I64), (out_param, arr_ty.clone())],
        );
        let done_cond = self.alloc_reg();
        self.emit(Instruction::new(
            Some(done_cond),
            Type::Bool,
            Op::CmpLt(i_param, zero_reg),
            line,
        ));
        self.current_block.terminator = Terminator::BrIfArgs {
            cond: done_cond,
            then_blk: exit_bb,
            then_args: vec![out_param],
            else_blk: body_bb,
            else_args: Vec::new(),
        };
        self.finalize_current_block();

        self.start_block(body_bb);
        let item_reg = self.alloc_reg();
        self.emit(Instruction::new(
            Some(item_reg),
            elem_ty,
            Op::VecGet(arr_reg, i_param),
            line,
        ));
        let out_next = self.alloc_reg();
        self.emit(Instruction::new(
            Some(out_next),
            arr_ty.clone(),
            Op::VecPush(out_param, item_reg),
            line,
        ));
        let one_step = self.alloc_reg();
        self.emit(Instruction::new(
            Some(one_step),
            Type::I64,
            Op::ConstI64(1),
            line,
        ));
        let i_next = self.alloc_reg();
        self.emit(Instruction::new(
            Some(i_next),
            Type::I64,
            Op::Sub(i_param, one_step),
            line,
        ));
        self.current_block.terminator = Terminator::BrArgs {
            target: header_bb,
            args: vec![i_next, out_next],
        };
        self.finalize_current_block();

        let res_reg = self.alloc_reg();
        self.start_block_with_params(exit_bb, vec![(res_reg, arr_ty.clone())]);
        self.linear_tracker
            .mark_alive(res_reg, arr_ty.clone(), true, line);

        if arr_is_temp_owned {
            self.emit(Instruction::new(None, Type::Void, Op::Free(arr_reg), line));
            self.linear_tracker.mark_moved(arr_reg, line)?;
        }

        Ok((res_reg, arr_ty))
    }
}
