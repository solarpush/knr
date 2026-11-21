use std::collections::HashSet;

use crate::ast::{Effect, Instruction, Op, RegId, Terminator, Type};
use crate::lower::context::FnLoweringContext;
use crate::lower::error::LowerError;
use crate::lower::stmt::{
    collect_assigned_vars_in_block, collect_assigned_vars_in_expr, parse_effect_name,
};
use crate::syntax::ast::{BinaryOp, Block, Expr, ExprKind, MatchArm, MatchPattern, Span, UnaryOp};

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
            ExprKind::Variable(ident) => {
                let (reg, ty) = self.lookup_var(&ident.name, line)?;
                self.record_span(reg, expr.span);
                Ok((reg, ty))
            }
            ExprKind::Borrow(inner) => {
                // Borrow evaluates inner without moving ownership
                self.lower_expr_with_expected(inner, expected_ty)
            }
            ExprKind::Unary { op, operand } => self.lower_unary(*op, operand, line),
            ExprKind::Binary { op, lhs, rhs } => self.lower_binary(*op, lhs, rhs, line),
            ExprKind::Call { callee, args } => {
                self.lower_call(&callee.name, args, expected_ty, line)
            }
            ExprKind::Syscall {
                effect,
                op,
                payload,
            } => self.lower_syscall_expr(&effect.name, op, payload, expected_ty, expr.span),
            ExprKind::StructLit { name, fields } => self.lower_struct_lit(&name.name, fields, line),
            ExprKind::EnumLit {
                enum_name,
                variant,
                args,
            } => self.lower_enum_lit(&enum_name.name, &variant.name, args, line),
            ExprKind::ArrayLit(elements) => self.lower_array_lit(elements, expected_ty, expr.span),
            ExprKind::FieldAccess { target, field } => {
                self.lower_field_access(target, &field.name, line)
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
        line: usize,
    ) -> Result<(RegId, Type), LowerError> {
        // Short-circuit logical operators using SSA block parameters
        if matches!(op, BinaryOp::And | BinaryOp::Or) {
            return self.lower_short_circuit(op, lhs, rhs, line);
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
            BinaryOp::And | BinaryOp::Or => unreachable!(),
        }
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
            "to_str" => {
                self.expect_argc(callee, args, 1, line)?;
                let (r, ty) = self.lower_expr(&args[0])?;
                if !matches!(ty, Type::I64 | Type::F64 | Type::Bool | Type::Str) {
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
                            if en.name != em.name {
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
                            "Non-exhaustive match on enum '{}': missing variant(s): {}",
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

    pub fn lower_struct_lit(
        &mut self,
        name: &str,
        fields: &[crate::syntax::ast::FieldInit],
        line: usize,
    ) -> Result<(RegId, Type), LowerError> {
        let schema = self
            .schemas
            .get(name)
            .cloned()
            .ok_or_else(|| LowerError::UnknownSchema {
                line,
                name: name.to_string(),
            })?;

        let struct_ty = Type::Struct(name.to_string());

        // 1. Initial struct allocation
        let mut current_struct_reg = self.alloc_reg();
        self.emit(Instruction::new(
            Some(current_struct_reg),
            struct_ty.clone(),
            Op::StructNew(name.to_string()),
            line,
        ));
        self.linear_tracker
            .mark_alive(current_struct_reg, struct_ty.clone(), true, line);

        // 2. Linear field threading
        for init in fields {
            let field_name = &init.name.name;
            let field_def = schema
                .get_field_by_name(field_name)
                .ok_or_else(|| LowerError::UndeclaredField {
                    line,
                    schema: name.to_string(),
                    field: field_name.clone(),
                })?
                .clone();

            let (val_reg, val_ty) =
                self.lower_expr_with_expected(&init.value, Some(&field_def.ty))?;
            if val_ty != field_def.ty {
                return Err(LowerError::TypeMismatch {
                    line,
                    expected: field_def.ty.to_string(),
                    actual: val_ty.to_string(),
                });
            }

            let next_struct_reg = self.alloc_reg();
            self.emit(Instruction::new(
                Some(next_struct_reg),
                struct_ty.clone(),
                Op::StructSet(current_struct_reg, field_def.index, val_reg),
                line,
            ));

            // State transition
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
        line: usize,
    ) -> Result<(RegId, Type), LowerError> {
        let (target_reg, target_ty) = self.lower_expr(target)?;
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
}
