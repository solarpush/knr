//! First-class JSX lowering: desugars declarative `<Column>`, `<Row>`, `<Grid>`, `<Card>`,
//! `<Text>`, `<Code>`, `<Badge>`, `<Button>`, `<Slider>`, `<Toggle>`, `<Input>`, `<Shader>`,
//! `<Icon>`, `<Image>`, `<Divider>`, `<Progress>`, `<Spacer>`, and custom user components
//! directly into canonical statically verified linear SSA `Widget` + `Style` instructions.

use std::collections::HashSet;

use crate::ast::{Instruction, Op, RegId, Terminator, Type};
use crate::lower::context::{FnLoweringContext, LinearState};
use crate::lower::error::LowerError;
use crate::syntax::ast::{BinaryOp, Expr, ExprKind, Ident, JsxChild, JsxProp, Span};

impl<'a> FnLoweringContext<'a> {
    pub fn lower_jsx(
        &mut self,
        tag: &Ident,
        props: &[JsxProp],
        children: &[JsxChild],
        span: Span,
    ) -> Result<(RegId, Type), LowerError> {
        let line = span.line;
        if let Some(kind_str) = builtin_jsx_kind(&tag.name) {
            return self.lower_builtin_jsx(kind_str, props, children, span);
        }

        // Custom user component function: `<Hero s={s} />` or `<StatCard metric="..." label="..." />`
        let sig = self
            .fn_sigs
            .get(&tag.name)
            .cloned()
            .ok_or_else(|| LowerError::UnknownFunction {
                line,
                name: tag.name.clone(),
            })?;

        for eff in &sig.effects {
            if !self.fn_effects.contains(eff) {
                return Err(LowerError::UndeclaredEffect {
                    line,
                    effect: eff.to_string(),
                    msg: format!(
                        "Component '{}' calls effectful component '{}' requiring effect {eff}",
                        self.fn_name, tag.name
                    ),
                });
            }
        }

        // Match props by parameter name (or positional fallback if single param & 1 prop)
        let mut arg_regs = Vec::with_capacity(sig.param_types.len());
        for (i, expected_ty) in sig.param_types.iter().enumerate() {
            let param_name = sig.param_names.get(i).map(|s| s.as_str()).unwrap_or("");
            let is_borrowed = sig.is_param_borrowed(i);
            let matched_prop = props
                .iter()
                .find(|p| p.name.name == param_name)
                .or_else(|| {
                    if sig.param_types.len() == 1 && props.len() == 1 {
                        Some(&props[0])
                    } else {
                        None
                    }
                });

            let (r_arg, ty_arg) = if let Some(prop) = matched_prop {
                if let Some(ref val_expr) = prop.value {
                    self.lower_expr_with_expected(val_expr, Some(expected_ty))?
                } else {
                    let r = self.alloc_reg();
                    self.emit(Instruction::new(Some(r), Type::Bool, Op::ConstBool(true), line));
                    (r, Type::Bool)
                }
            } else if param_name == "children" && matches!(expected_ty, Type::Vec(_)) {
                self.lower_jsx_widget_children(children, line)?
            } else {
                return Err(LowerError::InvalidConstruct {
                    line,
                    msg: format!(
                        "Missing required prop '{}' for component '<{} />'",
                        param_name, tag.name
                    ),
                });
            };

            if &ty_arg != expected_ty {
                return Err(LowerError::TypeMismatch {
                    line,
                    expected: expected_ty.to_string(),
                    actual: ty_arg.to_string(),
                });
            }
            if ty_arg.is_linear() && !is_borrowed {
                self.mark_consumed(r_arg, line)?;
            }
            arg_regs.push(r_arg);
        }

        let dst = self.alloc_reg();
        self.emit(Instruction::new(
            Some(dst),
            sig.ret_ty.clone(),
            Op::Call(tag.name.clone(), arg_regs),
            line,
        ));
        if sig.ret_ty.is_linear() {
            self.linear_tracker
                .mark_alive(dst, sig.ret_ty.clone(), true, line);
        }
        Ok((dst, sig.ret_ty))
    }

    fn lower_builtin_jsx(
        &mut self,
        kind_str: &str,
        props: &[JsxProp],
        children: &[JsxChild],
        span: Span,
    ) -> Result<(RegId, Type), LowerError> {
        let line = span.line;
        let style_schema = self
            .schemas
            .get("Style")
            .cloned()
            .ok_or_else(|| LowerError::UnknownSchema {
                line,
                name: "Style (did you forget `import \"std/ui\";`?)".to_string(),
            })?;
        let widget_schema = self
            .schemas
            .get("Widget")
            .cloned()
            .ok_or_else(|| LowerError::UnknownSchema {
                line,
                name: "Widget (did you forget `import \"std/ui\";`?)".to_string(),
            })?;

        // 1. Start Style from sensible semantic defaults for each widget kind
        let base_fn = match kind_str {
            "card" => "style_card",
            _ => "style_default",
        };
        let style_ty = Type::Struct("Style".to_string());
        let mut style_reg = self.alloc_reg();
        self.emit(Instruction::new(
            Some(style_reg),
            style_ty.clone(),
            Op::Call(base_fn.to_string(), Vec::new()),
            line,
        ));
        self.linear_tracker
            .mark_alive(style_reg, style_ty.clone(), true, line);

        // Apply kind-specific defaults before user prop overrides
        match kind_str {
            "button" => {
                style_reg = self.set_style_str(style_reg, &style_schema, "bg", "#131c31", line)?;
                style_reg = self.set_style_str(style_reg, &style_schema, "color", "#f8fafc", line)?;
                style_reg = self.set_style_str(style_reg, &style_schema, "border", "#ffffff1e", line)?;
                style_reg = self.set_style_f64_all_radii(style_reg, &style_schema, 10.0, line)?;
                style_reg = self.set_style_pad_xy(style_reg, &style_schema, 16.0, 10.0, line)?;
                style_reg = self.set_style_f64(style_reg, &style_schema, "font_size", 14.0, line)?;
                style_reg = self.set_style_i64(style_reg, &style_schema, "font_weight", 600, line)?;
                style_reg = self.set_style_f64(style_reg, &style_schema, "hover_scale", 1.02, line)?;
                style_reg = self.set_style_f64(style_reg, &style_schema, "transition_ms", 140.0, line)?;
            }
            "badge" => {
                style_reg = self.set_style_str(style_reg, &style_schema, "bg", "#0c1930", line)?;
                style_reg = self.set_style_str(style_reg, &style_schema, "color", "#38bdf8", line)?;
                style_reg = self.set_style_str(style_reg, &style_schema, "border", "#38bdf844", line)?;
                style_reg = self.set_style_f64_all_radii(style_reg, &style_schema, 999.0, line)?;
                style_reg = self.set_style_pad_xy(style_reg, &style_schema, 11.0, 5.0, line)?;
                style_reg = self.set_style_f64(style_reg, &style_schema, "font_size", 12.0, line)?;
                style_reg = self.set_style_i64(style_reg, &style_schema, "font_weight", 600, line)?;
            }
            "code" => {
                style_reg = self.set_style_str(style_reg, &style_schema, "bg", "#070b14", line)?;
                style_reg = self.set_style_str(style_reg, &style_schema, "color", "#e2e8f0", line)?;
                style_reg = self.set_style_str(style_reg, &style_schema, "border", "#ffffff14", line)?;
                style_reg = self.set_style_f64_all_radii(style_reg, &style_schema, 12.0, line)?;
                style_reg = self.set_style_pad_xy(style_reg, &style_schema, 16.0, 14.0, line)?;
                style_reg = self.set_style_f64(style_reg, &style_schema, "font_size", 13.0, line)?;
                style_reg = self.set_style_str(style_reg, &style_schema, "font_family", "mono", line)?;
                style_reg = self.set_style_f64(style_reg, &style_schema, "line_height", 1.55, line)?;
            }
            "input" => {
                style_reg = self.set_style_str(style_reg, &style_schema, "bg", "#080d1a", line)?;
                style_reg = self.set_style_str(style_reg, &style_schema, "color", "#f8fafc", line)?;
                style_reg = self.set_style_str(style_reg, &style_schema, "border", "#334155aa", line)?;
                style_reg = self.set_style_f64_all_radii(style_reg, &style_schema, 10.0, line)?;
                style_reg = self.set_style_pad_xy(style_reg, &style_schema, 14.0, 10.0, line)?;
                style_reg = self.set_style_f64(style_reg, &style_schema, "font_size", 14.0, line)?;
            }
            "icon" => {
                style_reg = self.set_style_f64(style_reg, &style_schema, "font_size", 18.0, line)?;
                style_reg = self.set_style_str(style_reg, &style_schema, "color", "#94a3b8", line)?;
            }
            "divider" => {
                style_reg = self.set_style_str(style_reg, &style_schema, "bg", "#ffffff14", line)?;
                style_reg = self.set_style_f64(style_reg, &style_schema, "h", 1.0, line)?;
            }
            "progress" => {
                style_reg = self.set_style_str(style_reg, &style_schema, "bg", "#1e293b", line)?;
                style_reg = self.set_style_str(style_reg, &style_schema, "color", "linear(90, #38bdf8, #818cf8)", line)?;
                style_reg = self.set_style_f64(style_reg, &style_schema, "h", 7.0, line)?;
                style_reg = self.set_style_f64_all_radii(style_reg, &style_schema, 999.0, line)?;
            }
            "toggle" => {
                style_reg = self.set_style_str(style_reg, &style_schema, "bg", "#0f172a", line)?;
                style_reg = self.set_style_str(style_reg, &style_schema, "color", "#e2e8f0", line)?;
                style_reg = self.set_style_str(style_reg, &style_schema, "border", "#ffffff1c", line)?;
                style_reg = self.set_style_f64_all_radii(style_reg, &style_schema, 10.0, line)?;
                style_reg = self.set_style_pad_xy(style_reg, &style_schema, 14.0, 9.0, line)?;
                style_reg = self.set_style_f64(style_reg, &style_schema, "font_size", 13.0, line)?;
                style_reg = self.set_style_i64(style_reg, &style_schema, "font_weight", 600, line)?;
            }
            _ => {}
        }

        // Track Widget-level fields
        let mut key_reg: Option<RegId> = None;
        let mut text_reg: Option<RegId> = None;
        let mut action_reg: Option<RegId> = None;
        let mut value_reg: Option<RegId> = None;
        let mut uniforms_reg: Option<RegId> = None;

        // 2. Apply JSX props
        for prop in props {
            let pname = prop.name.name.as_str();
            let pline = prop.span.line;

            match pname {
                "key" => {
                    if let Some(ref e) = prop.value {
                        let (r, _) = self.lower_expr_with_expected(e, Some(&Type::Str))?;
                        key_reg = Some(r);
                    }
                }
                "text" | "label" | "src" | "name" => {
                    if let Some(ref e) = prop.value {
                        let (r, _) = self.lower_expr_with_expected(e, Some(&Type::Str))?;
                        text_reg = Some(r);
                    }
                }
                "fn" | "shader" => {
                    if let Some(ref e) = prop.value {
                        match &e.kind {
                            ExprKind::Variable(fn_ident) => {
                                if let Some(sig) = self.fn_sigs.get(&fn_ident.name) {
                                    if !sig.is_shader {
                                        return Err(LowerError::InvalidConstruct {
                                            line: pline,
                                            msg: format!(
                                                "Function '{}' passed to <Shader fn={{{}}} /> must be annotated with @shader",
                                                fn_ident.name, fn_ident.name
                                            ),
                                        });
                                    }
                                }
                                let r = self.emit_const_str(&fn_ident.name, pline);
                                text_reg = Some(r);
                            }
                            _ => {
                                let (r, _) = self.lower_expr_with_expected(e, Some(&Type::Str))?;
                                text_reg = Some(r);
                            }
                        }
                    }
                }
                "onClick" | "onChange" | "onInput" | "onToggle" | "action" => {
                    if let Some(ref e) = prop.value {
                        let r = self.lower_jsx_action_prop(e, pline)?;
                        action_reg = Some(r);
                    }
                }
                "lang" => {
                    if let Some(ref e) = prop.value {
                        let (r, _) = self.lower_expr_with_expected(e, Some(&Type::Str))?;
                        action_reg = Some(r);
                    }
                }
                "value" => {
                    if let Some(ref e) = prop.value {
                        if kind_str == "input" {
                            let (r, _) = self.lower_expr_with_expected(e, Some(&Type::Str))?;
                            text_reg = Some(r);
                        } else {
                            let (r, _) = self.lower_expr_with_expected(e, Some(&Type::F64))?;
                            value_reg = Some(r);
                        }
                    }
                }
                "checked" => {
                    if let Some(ref e) = prop.value {
                        let (b_reg, b_ty) = self.lower_expr(e)?;
                        if b_ty == Type::Bool {
                            let r = self.lower_bool_to_f64(b_reg, pline);
                            value_reg = Some(r);
                        } else {
                            value_reg = Some(b_reg);
                        }
                    } else {
                        value_reg = Some(self.emit_const_f64(1.0, pline));
                    }
                }
                "uniforms" => {
                    if let Some(ref e) = prop.value {
                        let exp = Type::Vec(Box::new(Type::F64));
                        let (r, ty) = self.lower_expr_with_expected(e, Some(&exp))?;
                        if ty != exp {
                            return Err(LowerError::TypeMismatch {
                                line: pline,
                                expected: exp.to_string(),
                                actual: ty.to_string(),
                            });
                        }
                        uniforms_reg = Some(r);
                    }
                }
                "style" => {
                    if let Some(ref e) = prop.value {
                        let (new_s, ty) = self.lower_expr_with_expected(e, Some(&style_ty))?;
                        if ty != style_ty {
                            return Err(LowerError::TypeMismatch {
                                line: pline,
                                expected: style_ty.to_string(),
                                actual: ty.to_string(),
                            });
                        }
                        // Drop initial default style and replace with user style
                        self.emit(Instruction::new(None, Type::Void, Op::Free(style_reg), pline));
                        self.linear_tracker.mark_moved(style_reg, pline)?;
                        style_reg = new_s;
                    }
                }
                // ---- Shorthand & Direct Style Props ----
                "w" | "width" => {
                    if let Some(ref e) = prop.value {
                        let r = self.lower_dim_prop(e, pline)?;
                        style_reg = self.set_style_reg(style_reg, &style_schema, "w", r, Type::F64, pline)?;
                    }
                }
                "h" | "height" => {
                    if let Some(ref e) = prop.value {
                        let r = self.lower_dim_prop(e, pline)?;
                        style_reg = self.set_style_reg(style_reg, &style_schema, "h", r, Type::F64, pline)?;
                    }
                }
                "minW" => {
                    if let Some(ref e) = prop.value {
                        let (r, _) = self.lower_expr_with_expected(e, Some(&Type::F64))?;
                        style_reg = self.set_style_reg(style_reg, &style_schema, "min_w", r, Type::F64, pline)?;
                    }
                }
                "maxW" => {
                    if let Some(ref e) = prop.value {
                        let (r, _) = self.lower_expr_with_expected(e, Some(&Type::F64))?;
                        style_reg = self.set_style_reg(style_reg, &style_schema, "max_w", r, Type::F64, pline)?;
                    }
                }
                "grow" | "flex" => {
                    if let Some(ref e) = prop.value {
                        let (r, _) = self.lower_expr_with_expected(e, Some(&Type::F64))?;
                        style_reg = self.set_style_reg(style_reg, &style_schema, "grow", r, Type::F64, pline)?;
                    } else {
                        style_reg = self.set_style_f64(style_reg, &style_schema, "grow", 1.0, pline)?;
                    }
                }
                "bg" | "background" => {
                    if let Some(ref e) = prop.value {
                        let (r, _) = self.lower_expr_with_expected(e, Some(&Type::Str))?;
                        style_reg = self.set_style_reg(style_reg, &style_schema, "bg", r, Type::Str, pline)?;
                    }
                }
                "color" => {
                    if let Some(ref e) = prop.value {
                        let (r, _) = self.lower_expr_with_expected(e, Some(&Type::Str))?;
                        style_reg = self.set_style_reg(style_reg, &style_schema, "color", r, Type::Str, pline)?;
                    }
                }
                "border" => {
                    if let Some(ref e) = prop.value {
                        let (r, _) = self.lower_expr_with_expected(e, Some(&Type::Str))?;
                        style_reg = self.set_style_reg(style_reg, &style_schema, "border", r, Type::Str, pline)?;
                    }
                }
                "borderW" => {
                    if let Some(ref e) = prop.value {
                        let (r, _) = self.lower_expr_with_expected(e, Some(&Type::F64))?;
                        style_reg = self.set_style_reg(style_reg, &style_schema, "border_w", r, Type::F64, pline)?;
                    }
                }
                "r" | "radius" => {
                    if let Some(ref e) = prop.value {
                        let (r, _) = self.lower_expr_with_expected(e, Some(&Type::F64))?;
                        for fname in ["r_tl", "r_tr", "r_br", "r_bl"] {
                            style_reg = self.set_style_reg(style_reg, &style_schema, fname, r, Type::F64, pline)?;
                        }
                    }
                }
                "rTl" | "rTr" | "rBr" | "rBl" => {
                    let field = match pname {
                        "rTl" => "r_tl",
                        "rTr" => "r_tr",
                        "rBr" => "r_br",
                        _ => "r_bl",
                    };
                    if let Some(ref e) = prop.value {
                        let (r, _) = self.lower_expr_with_expected(e, Some(&Type::F64))?;
                        style_reg = self.set_style_reg(style_reg, &style_schema, field, r, Type::F64, pline)?;
                    }
                }
                "p" | "pad" => {
                    if let Some(ref e) = prop.value {
                        let (r, _) = self.lower_expr_with_expected(e, Some(&Type::F64))?;
                        for fname in ["pad_t", "pad_r", "pad_b", "pad_l"] {
                            style_reg = self.set_style_reg(style_reg, &style_schema, fname, r, Type::F64, pline)?;
                        }
                    }
                }
                "px" => {
                    if let Some(ref e) = prop.value {
                        let (r, _) = self.lower_expr_with_expected(e, Some(&Type::F64))?;
                        style_reg = self.set_style_reg(style_reg, &style_schema, "pad_l", r, Type::F64, pline)?;
                        style_reg = self.set_style_reg(style_reg, &style_schema, "pad_r", r, Type::F64, pline)?;
                    }
                }
                "py" => {
                    if let Some(ref e) = prop.value {
                        let (r, _) = self.lower_expr_with_expected(e, Some(&Type::F64))?;
                        style_reg = self.set_style_reg(style_reg, &style_schema, "pad_t", r, Type::F64, pline)?;
                        style_reg = self.set_style_reg(style_reg, &style_schema, "pad_b", r, Type::F64, pline)?;
                    }
                }
                "pt" | "pr" | "pb" | "pl" => {
                    let field = match pname {
                        "pt" => "pad_t",
                        "pr" => "pad_r",
                        "pb" => "pad_b",
                        _ => "pad_l",
                    };
                    if let Some(ref e) = prop.value {
                        let (r, _) = self.lower_expr_with_expected(e, Some(&Type::F64))?;
                        style_reg = self.set_style_reg(style_reg, &style_schema, field, r, Type::F64, pline)?;
                    }
                }
                "gap" => {
                    if let Some(ref e) = prop.value {
                        let (r, _) = self.lower_expr_with_expected(e, Some(&Type::F64))?;
                        style_reg = self.set_style_reg(style_reg, &style_schema, "gap_x", r, Type::F64, pline)?;
                        style_reg = self.set_style_reg(style_reg, &style_schema, "gap_y", r, Type::F64, pline)?;
                    }
                }
                "gapX" => {
                    if let Some(ref e) = prop.value {
                        let (r, _) = self.lower_expr_with_expected(e, Some(&Type::F64))?;
                        style_reg = self.set_style_reg(style_reg, &style_schema, "gap_x", r, Type::F64, pline)?;
                    }
                }
                "gapY" => {
                    if let Some(ref e) = prop.value {
                        let (r, _) = self.lower_expr_with_expected(e, Some(&Type::F64))?;
                        style_reg = self.set_style_reg(style_reg, &style_schema, "gap_y", r, Type::F64, pline)?;
                    }
                }
                "size" | "fontSize" => {
                    if let Some(ref e) = prop.value {
                        let (r, _) = self.lower_expr_with_expected(e, Some(&Type::F64))?;
                        if kind_str == "spacer" {
                            style_reg = self.set_style_reg(style_reg, &style_schema, "h", r, Type::F64, pline)?;
                        } else {
                            style_reg = self.set_style_reg(style_reg, &style_schema, "font_size", r, Type::F64, pline)?;
                        }
                    }
                }
                "weight" | "fontWeight" => {
                    if let Some(ref e) = prop.value {
                        let r = match &e.kind {
                            ExprKind::Str(s) => {
                                let w = match s.as_str() {
                                    "normal" | "regular" => 400,
                                    "medium" => 500,
                                    "semibold" => 600,
                                    "bold" => 700,
                                    "extrabold" | "black" => 800,
                                    _ => s.parse::<i64>().unwrap_or(400),
                                };
                                self.emit_const_i64(w, pline)
                            }
                            _ => self.lower_expr_with_expected(e, Some(&Type::I64))?.0,
                        };
                        style_reg = self.set_style_reg(style_reg, &style_schema, "font_weight", r, Type::I64, pline)?;
                    }
                }
                "font" | "fontFamily" => {
                    if let Some(ref e) = prop.value {
                        let (r, _) = self.lower_expr_with_expected(e, Some(&Type::Str))?;
                        style_reg = self.set_style_reg(style_reg, &style_schema, "font_family", r, Type::Str, pline)?;
                    }
                }
                "textAlign" => {
                    if let Some(ref e) = prop.value {
                        let (r, _) = self.lower_expr_with_expected(e, Some(&Type::Str))?;
                        style_reg = self.set_style_reg(style_reg, &style_schema, "text_align", r, Type::Str, pline)?;
                    }
                }
                "lineHeight" => {
                    if let Some(ref e) = prop.value {
                        let (r, _) = self.lower_expr_with_expected(e, Some(&Type::F64))?;
                        style_reg = self.set_style_reg(style_reg, &style_schema, "line_height", r, Type::F64, pline)?;
                    }
                }
                "letterSpacing" | "tracking" => {
                    if let Some(ref e) = prop.value {
                        let (r, _) = self.lower_expr_with_expected(e, Some(&Type::F64))?;
                        style_reg = self.set_style_reg(style_reg, &style_schema, "letter_spacing", r, Type::F64, pline)?;
                    }
                }
                "align" => {
                    if let Some(ref e) = prop.value {
                        let (r, _) = self.lower_expr_with_expected(e, Some(&Type::Str))?;
                        style_reg = self.set_style_reg(style_reg, &style_schema, "align", r, Type::Str, pline)?;
                    }
                }
                "justify" => {
                    if let Some(ref e) = prop.value {
                        let (r, _) = self.lower_expr_with_expected(e, Some(&Type::Str))?;
                        style_reg = self.set_style_reg(style_reg, &style_schema, "justify", r, Type::Str, pline)?;
                    }
                }
                "cols" | "gridCols" => {
                    if let Some(ref e) = prop.value {
                        let (r, _) = self.lower_expr_with_expected(e, Some(&Type::I64))?;
                        style_reg = self.set_style_reg(style_reg, &style_schema, "grid_cols", r, Type::I64, pline)?;
                    }
                }
                "span" | "colSpan" => {
                    if let Some(ref e) = prop.value {
                        let (r, _) = self.lower_expr_with_expected(e, Some(&Type::I64))?;
                        style_reg = self.set_style_reg(style_reg, &style_schema, "col_span", r, Type::I64, pline)?;
                    }
                }
                "shadowX" => {
                    if let Some(ref e) = prop.value {
                        let (r, _) = self.lower_expr_with_expected(e, Some(&Type::F64))?;
                        style_reg = self.set_style_reg(style_reg, &style_schema, "shadow_x", r, Type::F64, pline)?;
                    }
                }
                "shadowY" => {
                    if let Some(ref e) = prop.value {
                        let (r, _) = self.lower_expr_with_expected(e, Some(&Type::F64))?;
                        style_reg = self.set_style_reg(style_reg, &style_schema, "shadow_y", r, Type::F64, pline)?;
                    }
                }
                "shadowBlur" => {
                    if let Some(ref e) = prop.value {
                        let (r, _) = self.lower_expr_with_expected(e, Some(&Type::F64))?;
                        style_reg = self.set_style_reg(style_reg, &style_schema, "shadow_blur", r, Type::F64, pline)?;
                    }
                }
                "shadowColor" => {
                    if let Some(ref e) = prop.value {
                        let (r, _) = self.lower_expr_with_expected(e, Some(&Type::Str))?;
                        style_reg = self.set_style_reg(style_reg, &style_schema, "shadow_color", r, Type::Str, pline)?;
                    }
                }
                "blur" | "backdropBlur" => {
                    if let Some(ref e) = prop.value {
                        let (r, _) = self.lower_expr_with_expected(e, Some(&Type::F64))?;
                        style_reg = self.set_style_reg(style_reg, &style_schema, "backdrop_blur", r, Type::F64, pline)?;
                    }
                }
                "opacity" => {
                    if let Some(ref e) = prop.value {
                        let (r, _) = self.lower_expr_with_expected(e, Some(&Type::F64))?;
                        style_reg = self.set_style_reg(style_reg, &style_schema, "opacity", r, Type::F64, pline)?;
                    }
                }
                "tx" => {
                    if let Some(ref e) = prop.value {
                        let (r, _) = self.lower_expr_with_expected(e, Some(&Type::F64))?;
                        style_reg = self.set_style_reg(style_reg, &style_schema, "translate_x", r, Type::F64, pline)?;
                    }
                }
                "ty" => {
                    if let Some(ref e) = prop.value {
                        let (r, _) = self.lower_expr_with_expected(e, Some(&Type::F64))?;
                        style_reg = self.set_style_reg(style_reg, &style_schema, "translate_y", r, Type::F64, pline)?;
                    }
                }
                "scale" => {
                    if let Some(ref e) = prop.value {
                        let (r, _) = self.lower_expr_with_expected(e, Some(&Type::F64))?;
                        style_reg = self.set_style_reg(style_reg, &style_schema, "scale", r, Type::F64, pline)?;
                    }
                }
                "rotate" => {
                    if let Some(ref e) = prop.value {
                        let (r, _) = self.lower_expr_with_expected(e, Some(&Type::F64))?;
                        style_reg = self.set_style_reg(style_reg, &style_schema, "rotate_deg", r, Type::F64, pline)?;
                    }
                }
                "hoverBg" => {
                    if let Some(ref e) = prop.value {
                        let (r, _) = self.lower_expr_with_expected(e, Some(&Type::Str))?;
                        style_reg = self.set_style_reg(style_reg, &style_schema, "hover_bg", r, Type::Str, pline)?;
                    }
                }
                "hoverBorder" => {
                    if let Some(ref e) = prop.value {
                        let (r, _) = self.lower_expr_with_expected(e, Some(&Type::Str))?;
                        style_reg = self.set_style_reg(style_reg, &style_schema, "hover_border", r, Type::Str, pline)?;
                    }
                }
                "hoverScale" => {
                    if let Some(ref e) = prop.value {
                        let (r, _) = self.lower_expr_with_expected(e, Some(&Type::F64))?;
                        style_reg = self.set_style_reg(style_reg, &style_schema, "hover_scale", r, Type::F64, pline)?;
                    }
                }
                "transition" => {
                    if let Some(ref e) = prop.value {
                        let (r, _) = self.lower_expr_with_expected(e, Some(&Type::F64))?;
                        style_reg = self.set_style_reg(style_reg, &style_schema, "transition_ms", r, Type::F64, pline)?;
                    }
                }
                "overflow" => {
                    if let Some(ref e) = prop.value {
                        let (r, _) = self.lower_expr_with_expected(e, Some(&Type::Str))?;
                        style_reg = self.set_style_reg(style_reg, &style_schema, "overflow", r, Type::Str, pline)?;
                    }
                }
                "z" | "zIndex" => {
                    if let Some(ref e) = prop.value {
                        let (r, _) = self.lower_expr_with_expected(e, Some(&Type::I64))?;
                        style_reg = self.set_style_reg(style_reg, &style_schema, "z_index", r, Type::I64, pline)?;
                    }
                }
                other => {
                    return Err(LowerError::InvalidConstruct {
                        line: pline,
                        msg: format!("Unknown JSX prop '{other}' on <{kind_str}>"),
                    });
                }
            }
        }

        // 3. Lower children: text-leaf vs widget-children
        let has_element_child = children.iter().any(|c| match c {
            JsxChild::Element(_) => true,
            JsxChild::Expr(e) => expr_contains_widget(e),
            JsxChild::Text(_, _) => false,
        });
        let is_text_leaf = matches!(kind_str, "text" | "code" | "badge" | "toggle")
            || (kind_str == "button" && !has_element_child);

        let children_vec_reg = if !children.is_empty() && is_text_leaf {
            let joined = self.lower_jsx_text_children(children, line)?;
            text_reg = Some(joined);
            self.emit_empty_widget_vec(line)
        } else if !children.is_empty() {
            self.lower_jsx_widget_children(children, line)?.0
        } else {
            self.emit_empty_widget_vec(line)
        };

        // 4. Construct Widget struct (skip `value` StructSet when omitted since StructNew zero-initializes f64 to 0.0)
        let final_key = key_reg.unwrap_or_else(|| {
            let auto_k = format!("jsx_{}_{}_{}", kind_str, span.line, span.col);
            self.emit_const_str(&auto_k, line)
        });
        let final_kind = self.emit_const_str(kind_str, line);
        let final_text = text_reg.unwrap_or_else(|| self.emit_const_str("", line));
        let final_action = action_reg.unwrap_or_else(|| self.emit_const_str("", line));
        let final_uniforms = uniforms_reg.unwrap_or_else(|| self.emit_empty_f64_vec(line));

        let widget_ty = Type::Struct("Widget".to_string());
        let mut w_reg = self.alloc_reg();
        self.emit(Instruction::new(
            Some(w_reg),
            widget_ty.clone(),
            Op::StructNew("Widget".to_string()),
            line,
        ));
        self.linear_tracker
            .mark_alive(w_reg, widget_ty.clone(), true, line);

        let mut field_regs: Vec<(&str, RegId, bool)> = Vec::with_capacity(8);
        field_regs.push(("key", final_key, false));
        field_regs.push(("kind", final_kind, false));
        field_regs.push(("text", final_text, false));
        field_regs.push(("action", final_action, false));
        if let Some(final_value) = value_reg {
            field_regs.push(("value", final_value, false));
        }
        field_regs.push(("uniforms", final_uniforms, true));
        field_regs.push(("style", style_reg, true));
        field_regs.push(("children", children_vec_reg, true));

        for (fname, fval, is_lin) in field_regs {
            let fdef = widget_schema
                .get_field_by_name(fname)
                .ok_or_else(|| LowerError::UndeclaredField {
                    line,
                    schema: "Widget".to_string(),
                    field: fname.to_string(),
                })?;
            let next_w = self.alloc_reg();
            self.emit(Instruction::new(
                Some(next_w),
                widget_ty.clone(),
                Op::StructSet(w_reg, fdef.index, fval),
                line,
            ));
            self.linear_tracker.mark_moved(w_reg, line)?;
            if is_lin {
                self.linear_tracker.mark_moved(fval, line)?;
            }
            self.linear_tracker
                .mark_alive(next_w, widget_ty.clone(), true, line);
            w_reg = next_w;
        }

        Ok((w_reg, widget_ty))
    }

    fn lower_dim_prop(&mut self, expr: &Expr, line: usize) -> Result<RegId, LowerError> {
        if let ExprKind::Str(ref s) = expr.kind {
            if s == "full" || s == "100%" {
                return Ok(self.emit_const_f64(-1.0, line));
            }
            if s == "auto" {
                return Ok(self.emit_const_f64(0.0, line));
            }
        }
        let (r, _) = self.lower_expr_with_expected(expr, Some(&Type::F64))?;
        Ok(r)
    }

    fn lower_bool_to_f64(&mut self, b_reg: RegId, line: usize) -> RegId {
        let then_bb = self.alloc_block();
        let else_bb = self.alloc_block();
        let merge_bb = self.alloc_block();
        self.current_block.terminator = crate::ast::Terminator::BrIf {
            cond: b_reg,
            then_blk: then_bb,
            else_blk: else_bb,
        };
        self.finalize_current_block();

        self.start_block(then_bb);
        let one = self.emit_const_f64(1.0, line);
        self.current_block.terminator = crate::ast::Terminator::BrArgs {
            target: merge_bb,
            args: vec![one],
        };
        self.finalize_current_block();

        self.start_block(else_bb);
        let zero = self.emit_const_f64(0.0, line);
        self.current_block.terminator = crate::ast::Terminator::BrArgs {
            target: merge_bb,
            args: vec![zero],
        };
        self.finalize_current_block();

        let res = self.alloc_reg();
        self.start_block_with_params(merge_bb, vec![(res, Type::F64)]);
        res
    }

    fn lower_jsx_text_children(
        &mut self,
        children: &[JsxChild],
        line: usize,
    ) -> Result<RegId, LowerError> {
        let mut acc: Option<RegId> = None;
        for ch in children {
            let part_reg = match ch {
                JsxChild::Text(txt, sp) => self.emit_const_str(txt, sp.line),
                JsxChild::Expr(e) => {
                    let (r, ty) = self.lower_expr(e)?;
                    self.coerce_to_str(r, &ty, e.span.line)?
                }
                JsxChild::Element(e) => {
                    return Err(LowerError::InvalidConstruct {
                        line: e.span.line,
                        msg: "Nested JSX element inside text leaf is not supported; wrap siblings in <Row> or <Column>".to_string(),
                    });
                }
            };
            acc = Some(match acc {
                None => part_reg,
                Some(prev) => {
                    let dst = self.alloc_reg();
                    self.emit(Instruction::new(
                        Some(dst),
                        Type::Str,
                        Op::Add(prev, part_reg),
                        line,
                    ));
                    dst
                }
            });
        }
        Ok(acc.unwrap_or_else(|| self.emit_const_str("", line)))
    }

    fn lower_jsx_action_prop(&mut self, expr: &Expr, line: usize) -> Result<RegId, LowerError> {
        match &expr.kind {
            ExprKind::EnumLit {
                enum_name,
                type_args,
                variant,
                args,
            } => {
                let resolved_enum =
                    self.resolve_enum_name(&enum_name.name, type_args, None, line)?;
                self.lower_typed_action_enum(&resolved_enum, &variant.name, args, line)
            }
            ExprKind::FieldAccess { target, field } => {
                if let ExprKind::Variable(ref obj_ident) = target.kind {
                    let is_declared_enum = self.enums.contains_key(&obj_ident.name);
                    let looks_like_enum = obj_ident
                        .name
                        .chars()
                        .next()
                        .is_some_and(|c| c.is_ascii_uppercase())
                        && self.lookup_var_binding(&obj_ident.name).is_none()
                        && !self.const_exprs.contains_key(&obj_ident.name);
                    if is_declared_enum || looks_like_enum {
                        return self.lower_typed_action_enum(
                            &obj_ident.name,
                            &field.name,
                            &[],
                            line,
                        );
                    }
                }
                let (r, ty) = self.lower_expr_with_expected(expr, Some(&Type::Str))?;
                if ty != Type::Str {
                    return Err(LowerError::TypeMismatch {
                        line,
                        expected: "str or enum Action".to_string(),
                        actual: ty.to_string(),
                    });
                }
                Ok(r)
            }
            _ => {
                let (r, ty) = self.lower_expr_with_expected(expr, Some(&Type::Str))?;
                if ty != Type::Str {
                    return Err(LowerError::TypeMismatch {
                        line,
                        expected: "str or enum Action".to_string(),
                        actual: ty.to_string(),
                    });
                }
                Ok(r)
            }
        }
    }

    fn lower_typed_action_enum(
        &mut self,
        enum_name: &str,
        variant_name: &str,
        args: &[Expr],
        line: usize,
    ) -> Result<RegId, LowerError> {
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
                func: format!("{enum_name}.{variant_name}"),
                expected: variant.field_slots.len(),
                actual: args.len(),
            });
        }

        let mut acc = self.emit_const_str(variant_name, line);
        if args.is_empty() {
            return Ok(acc);
        }

        for (arg_expr, (_slot_idx, expected_ty)) in args.iter().zip(variant.field_slots.iter()) {
            let (arg_reg, arg_ty) = self.lower_expr_with_expected(arg_expr, Some(expected_ty))?;
            if &arg_ty != expected_ty {
                return Err(LowerError::TypeMismatch {
                    line: arg_expr.span.line,
                    expected: expected_ty.to_string(),
                    actual: arg_ty.to_string(),
                });
            }
            let arg_str = self.coerce_to_str(arg_reg, &arg_ty, arg_expr.span.line)?;
            let colon = self.emit_const_str(":", line);
            let with_colon = self.alloc_reg();
            self.emit(Instruction::new(
                Some(with_colon),
                Type::Str,
                Op::Add(acc, colon),
                line,
            ));
            let next = self.alloc_reg();
            self.emit(Instruction::new(
                Some(next),
                Type::Str,
                Op::Add(with_colon, arg_str),
                line,
            ));
            acc = next;
        }

        Ok(acc)
    }

    fn lower_jsx_widget_children(
        &mut self,
        children: &[JsxChild],
        line: usize,
    ) -> Result<(RegId, Type), LowerError> {
        let widget_ty = Type::Struct("Widget".to_string());
        let vec_ty = Type::Vec(Box::new(widget_ty.clone()));
        let mut cur_vec = self.emit_empty_widget_vec(line);

        for ch in children {
            match ch {
                JsxChild::Element(el) => {
                    let (child_reg, child_ty) =
                        self.lower_expr_with_expected(el, Some(&widget_ty))?;
                    if child_ty != widget_ty {
                        return Err(LowerError::TypeMismatch {
                            line: el.span.line,
                            expected: widget_ty.to_string(),
                            actual: child_ty.to_string(),
                        });
                    }
                    cur_vec = self.emit_push_widget(cur_vec, child_reg, el.span.line)?;
                }
                JsxChild::Expr(e) => {
                    // Conditional JSX: `{cond && <Tag />}` or `{cond && s.items.map(...)}`
                    if let ExprKind::Binary {
                        op: BinaryOp::And,
                        lhs,
                        rhs,
                    } = &e.kind
                    {
                        cur_vec =
                            self.lower_conditional_jsx_child(cur_vec, lhs, rhs, e.span.line)?;
                        continue;
                    }

                    let exp_hint = match &e.kind {
                        ExprKind::StructLit { .. } => Some(&widget_ty),
                        ExprKind::ArrayLit(els) if els.is_empty() => Some(&vec_ty),
                        _ => None,
                    };
                    let (r, ty) = self.lower_expr_with_expected(e, exp_hint)?;
                    if ty == widget_ty {
                        cur_vec = self.emit_push_widget(cur_vec, r, e.span.line)?;
                    } else if ty == vec_ty {
                        cur_vec = self.emit_append_widget_vec(cur_vec, r, e.span.line)?;
                    } else {
                        // Auto-wrap string/number interpolation inside container as a `<Text>` child!
                        let s_reg = self.coerce_to_str(r, &ty, e.span.line)?;
                        let (w_reg, _) = self.emit_inline_text_widget(s_reg, e.span)?;
                        cur_vec = self.emit_push_widget(cur_vec, w_reg, e.span.line)?;
                    }
                }
                JsxChild::Text(txt, sp) => {
                    let trimmed = txt.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    let s_reg = self.emit_const_str(trimmed, sp.line);
                    let (w_reg, _) = self.emit_inline_text_widget(s_reg, *sp)?;
                    cur_vec = self.emit_push_widget(cur_vec, w_reg, sp.line)?;
                }
            }
        }

        Ok((cur_vec, vec_ty))
    }

    fn emit_push_widget(
        &mut self,
        cur_vec: RegId,
        child_reg: RegId,
        line: usize,
    ) -> Result<RegId, LowerError> {
        let vec_ty = Type::Vec(Box::new(Type::Struct("Widget".to_string())));
        let next_vec = self.alloc_reg();
        self.emit(Instruction::new(
            Some(next_vec),
            vec_ty.clone(),
            Op::VecPush(cur_vec, child_reg),
            line,
        ));
        self.linear_tracker.mark_moved(cur_vec, line)?;
        self.linear_tracker.mark_moved(child_reg, line)?;
        self.linear_tracker
            .mark_alive(next_vec, vec_ty, true, line);
        Ok(next_vec)
    }

    /// Emits a verified monotonic SSA loop flattening every element of `child_vec: Widget[]`
    /// into `parent_vec: Widget[]`, then freeing `child_vec` if it is owned.
    fn emit_append_widget_vec(
        &mut self,
        parent_vec: RegId,
        child_vec: RegId,
        line: usize,
    ) -> Result<RegId, LowerError> {
        let widget_ty = Type::Struct("Widget".to_string());
        let vec_ty = Type::Vec(Box::new(widget_ty.clone()));

        let child_is_owned = matches!(
            self.linear_tracker.states.get(&child_vec),
            Some(LinearState::Alive {
                borrowed: false,
                ..
            })
        );

        let len_reg = self.alloc_reg();
        self.emit(Instruction::new(
            Some(len_reg),
            Type::I64,
            Op::VecLen(child_vec),
            line,
        ));
        let zero_reg = self.emit_const_i64(0, line);

        let header_bb = self.alloc_block();
        let body_bb = self.alloc_block();
        let exit_bb = self.alloc_block();

        self.linear_tracker.mark_moved(parent_vec, line)?;
        self.current_block.terminator = Terminator::BrArgs {
            target: header_bb,
            args: vec![zero_reg, parent_vec],
        };
        self.finalize_current_block();

        let i_param = self.alloc_reg();
        let acc_param = self.alloc_reg();
        self.start_block_with_params(
            header_bb,
            vec![(i_param, Type::I64), (acc_param, vec_ty.clone())],
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

        self.start_block(body_bb);
        let elem_reg = self.alloc_reg();
        self.emit(Instruction::new(
            Some(elem_reg),
            widget_ty,
            Op::VecGet(child_vec, i_param),
            line,
        ));
        let acc_next = self.alloc_reg();
        self.emit(Instruction::new(
            Some(acc_next),
            vec_ty.clone(),
            Op::VecPush(acc_param, elem_reg),
            line,
        ));
        let one_reg = self.emit_const_i64(1, line);
        let i_next = self.alloc_reg();
        self.emit(Instruction::new(
            Some(i_next),
            Type::I64,
            Op::Add(i_param, one_reg),
            line,
        ));
        self.current_block.terminator = Terminator::BrArgs {
            target: header_bb,
            args: vec![i_next, acc_next],
        };
        self.finalize_current_block();

        let final_vec = self.alloc_reg();
        self.start_block_with_params(exit_bb, vec![(final_vec, vec_ty.clone())]);
        self.linear_tracker
            .mark_alive(final_vec, vec_ty, true, line);

        if child_is_owned {
            self.emit(Instruction::new(None, Type::Void, Op::Free(child_vec), line));
            self.linear_tracker.mark_moved(child_vec, line)?;
        }

        Ok(final_vec)
    }

    /// Lowers conditional JSX `{cond && <Tag />}` (or `{cond && s.items.map(...)}`)
    /// using balanced SSA block-parameter merging on `cur_vec`.
    fn lower_conditional_jsx_child(
        &mut self,
        cur_vec: RegId,
        cond_expr: &Expr,
        rhs_expr: &Expr,
        line: usize,
    ) -> Result<RegId, LowerError> {
        let widget_ty = Type::Struct("Widget".to_string());
        let vec_ty = Type::Vec(Box::new(widget_ty.clone()));

        let (cond_reg, cond_ty) = self.lower_expr(cond_expr)?;
        if cond_ty != Type::Bool {
            return Err(LowerError::TypeMismatch {
                line: cond_expr.span.line,
                expected: "bool".to_string(),
                actual: cond_ty.to_string(),
            });
        }

        let then_bb = self.alloc_block();
        let merge_bb = self.alloc_block();

        let scopes_snap = self.scopes.clone();
        let tracker_before_branch = self.linear_tracker.snapshot();

        self.current_block.terminator = Terminator::BrIfArgs {
            cond: cond_reg,
            then_blk: then_bb,
            then_args: Vec::new(),
            else_blk: merge_bb,
            else_args: vec![cur_vec],
        };
        self.finalize_current_block();

        // Else-edge linear tracker: cur_vec was moved into merge_bb
        let mut else_tracker = tracker_before_branch.clone();
        else_tracker.mark_moved(cur_vec, line)?;

        // Lower Then branch
        self.start_block(then_bb);
        let exp_hint = match &rhs_expr.kind {
            ExprKind::StructLit { .. } => Some(&widget_ty),
            ExprKind::ArrayLit(els) if els.is_empty() => Some(&vec_ty),
            _ => None,
        };
        let (rhs_reg, rhs_ty) = self.lower_expr_with_expected(rhs_expr, exp_hint)?;
        let next_vec = if rhs_ty == widget_ty {
            self.emit_push_widget(cur_vec, rhs_reg, rhs_expr.span.line)?
        } else if rhs_ty == vec_ty {
            self.emit_append_widget_vec(cur_vec, rhs_reg, rhs_expr.span.line)?
        } else {
            let s_reg = self.coerce_to_str(rhs_reg, &rhs_ty, rhs_expr.span.line)?;
            let (w_reg, _) = self.emit_inline_text_widget(s_reg, rhs_expr.span)?;
            self.emit_push_widget(cur_vec, w_reg, rhs_expr.span.line)?
        };
        self.linear_tracker.mark_moved(next_vec, line)?;
        let then_tracker = self.linear_tracker.snapshot();

        self.current_block.terminator = Terminator::BrArgs {
            target: merge_bb,
            args: vec![next_vec],
        };
        self.finalize_current_block();

        // Reconcile outer linear state across both edges into merge_bb
        self.scopes = scopes_snap;
        let skip_set: HashSet<RegId> = [cur_vec].into_iter().collect();
        let mut merged_base = tracker_before_branch;
        merged_base.states.retain(|r, _| !skip_set.contains(r));
        merged_base.merge_branches(
            &then_tracker,
            &else_tracker,
            &self.source_map,
            &self.fn_name,
            line,
        )?;
        self.linear_tracker = merged_base;

        let merged_vec = self.alloc_reg();
        self.start_block_with_params(merge_bb, vec![(merged_vec, vec_ty.clone())]);
        self.linear_tracker
            .mark_alive(merged_vec, vec_ty, true, line);
        Ok(merged_vec)
    }

    fn emit_inline_text_widget(
        &mut self,
        text_reg: RegId,
        span: Span,
    ) -> Result<(RegId, Type), LowerError> {
        let line = span.line;
        let widget_schema = self
            .schemas
            .get("Widget")
            .cloned()
            .ok_or_else(|| LowerError::UnknownSchema {
                line,
                name: "Widget".to_string(),
            })?;
        let style_ty = Type::Struct("Style".to_string());
        let style_reg = self.alloc_reg();
        self.emit(Instruction::new(
            Some(style_reg),
            style_ty.clone(),
            Op::Call("style_default".to_string(), Vec::new()),
            line,
        ));
        self.linear_tracker
            .mark_alive(style_reg, style_ty, true, line);

        let key_reg = self.emit_const_str(&format!("jsx_txt_{}_{}", span.line, span.col), line);
        let kind_reg = self.emit_const_str("text", line);
        let act_reg = self.emit_const_str("", line);
        let u_reg = self.emit_empty_f64_vec(line);
        let c_reg = self.emit_empty_widget_vec(line);

        let widget_ty = Type::Struct("Widget".to_string());
        let mut w_reg = self.alloc_reg();
        self.emit(Instruction::new(
            Some(w_reg),
            widget_ty.clone(),
            Op::StructNew("Widget".to_string()),
            line,
        ));
        self.linear_tracker
            .mark_alive(w_reg, widget_ty.clone(), true, line);

        for (fname, fval, is_lin) in [
            ("key", key_reg, false),
            ("kind", kind_reg, false),
            ("text", text_reg, false),
            ("action", act_reg, false),
            ("uniforms", u_reg, true),
            ("style", style_reg, true),
            ("children", c_reg, true),
        ] {
            let fdef = widget_schema.get_field_by_name(fname).unwrap();
            let next_w = self.alloc_reg();
            self.emit(Instruction::new(
                Some(next_w),
                widget_ty.clone(),
                Op::StructSet(w_reg, fdef.index, fval),
                line,
            ));
            self.linear_tracker.mark_moved(w_reg, line)?;
            if is_lin {
                self.linear_tracker.mark_moved(fval, line)?;
            }
            self.linear_tracker
                .mark_alive(next_w, widget_ty.clone(), true, line);
            w_reg = next_w;
        }
        Ok((w_reg, widget_ty))
    }

    pub(crate) fn coerce_to_str(
        &mut self,
        reg: RegId,
        ty: &Type,
        line: usize,
    ) -> Result<RegId, LowerError> {
        if *ty == Type::Str {
            return Ok(reg);
        }
        if matches!(ty, Type::I64 | Type::F64 | Type::Bool) {
            let dst = self.alloc_reg();
            self.emit(Instruction::new(Some(dst), Type::Str, Op::IntToStr(reg), line));
            return Ok(dst);
        }
        Err(LowerError::TypeMismatch {
            line,
            expected: "string, i64, f64, or bool".to_string(),
            actual: ty.to_string(),
        })
    }

    fn emit_const_str(&mut self, s: &str, line: usize) -> RegId {
        let r = self.alloc_reg();
        self.emit(Instruction::new(
            Some(r),
            Type::Str,
            Op::ConstStr(s.to_string()),
            line,
        ));
        r
    }

    fn emit_const_f64(&mut self, v: f64, line: usize) -> RegId {
        let r = self.alloc_reg();
        self.emit(Instruction::new(Some(r), Type::F64, Op::ConstF64(v), line));
        r
    }

    fn emit_const_i64(&mut self, v: i64, line: usize) -> RegId {
        let r = self.alloc_reg();
        self.emit(Instruction::new(Some(r), Type::I64, Op::ConstI64(v), line));
        r
    }

    fn emit_empty_widget_vec(&mut self, line: usize) -> RegId {
        let w_ty = Type::Struct("Widget".to_string());
        let vec_ty = Type::Vec(Box::new(w_ty.clone()));
        let r = self.alloc_reg();
        self.emit(Instruction::new(
            Some(r),
            vec_ty.clone(),
            Op::VecNew(w_ty),
            line,
        ));
        self.linear_tracker.mark_alive(r, vec_ty, true, line);
        r
    }

    fn emit_empty_f64_vec(&mut self, line: usize) -> RegId {
        let vec_ty = Type::Vec(Box::new(Type::F64));
        let r = self.alloc_reg();
        self.emit(Instruction::new(
            Some(r),
            vec_ty.clone(),
            Op::VecNew(Type::F64),
            line,
        ));
        self.linear_tracker.mark_alive(r, vec_ty, true, line);
        r
    }

    fn set_style_reg(
        &mut self,
        style_reg: RegId,
        schema: &crate::ast::Schema,
        field: &str,
        val_reg: RegId,
        _ty: Type,
        line: usize,
    ) -> Result<RegId, LowerError> {
        let fdef = schema
            .get_field_by_name(field)
            .ok_or_else(|| LowerError::UndeclaredField {
                line,
                schema: "Style".to_string(),
                field: field.to_string(),
            })?;
        let style_ty = Type::Struct("Style".to_string());
        let next = self.alloc_reg();
        self.emit(Instruction::new(
            Some(next),
            style_ty.clone(),
            Op::StructSet(style_reg, fdef.index, val_reg),
            line,
        ));
        self.linear_tracker.mark_moved(style_reg, line)?;
        self.linear_tracker.mark_alive(next, style_ty, true, line);
        Ok(next)
    }

    fn set_style_str(
        &mut self,
        style_reg: RegId,
        schema: &crate::ast::Schema,
        field: &str,
        val: &str,
        line: usize,
    ) -> Result<RegId, LowerError> {
        let r = self.emit_const_str(val, line);
        self.set_style_reg(style_reg, schema, field, r, Type::Str, line)
    }

    fn set_style_f64(
        &mut self,
        style_reg: RegId,
        schema: &crate::ast::Schema,
        field: &str,
        val: f64,
        line: usize,
    ) -> Result<RegId, LowerError> {
        let r = self.emit_const_f64(val, line);
        self.set_style_reg(style_reg, schema, field, r, Type::F64, line)
    }

    fn set_style_i64(
        &mut self,
        style_reg: RegId,
        schema: &crate::ast::Schema,
        field: &str,
        val: i64,
        line: usize,
    ) -> Result<RegId, LowerError> {
        let r = self.emit_const_i64(val, line);
        self.set_style_reg(style_reg, schema, field, r, Type::I64, line)
    }

    fn set_style_f64_all_radii(
        &mut self,
        mut style_reg: RegId,
        schema: &crate::ast::Schema,
        r_val: f64,
        line: usize,
    ) -> Result<RegId, LowerError> {
        let r = self.emit_const_f64(r_val, line);
        for fname in ["r_tl", "r_tr", "r_br", "r_bl"] {
            style_reg = self.set_style_reg(style_reg, schema, fname, r, Type::F64, line)?;
        }
        Ok(style_reg)
    }

    fn set_style_pad_xy(
        &mut self,
        mut style_reg: RegId,
        schema: &crate::ast::Schema,
        px: f64,
        py: f64,
        line: usize,
    ) -> Result<RegId, LowerError> {
        let rx = self.emit_const_f64(px, line);
        let ry = self.emit_const_f64(py, line);
        style_reg = self.set_style_reg(style_reg, schema, "pad_l", rx, Type::F64, line)?;
        style_reg = self.set_style_reg(style_reg, schema, "pad_r", rx, Type::F64, line)?;
        style_reg = self.set_style_reg(style_reg, schema, "pad_t", ry, Type::F64, line)?;
        style_reg = self.set_style_reg(style_reg, schema, "pad_b", ry, Type::F64, line)?;
        Ok(style_reg)
    }
}

fn block_contains_widget(blk: &crate::syntax::ast::Block) -> bool {
    if blk.tail_expr.as_deref().is_some_and(expr_contains_widget) {
        return true;
    }
    blk.stmts.iter().any(|s| match &s.kind {
        crate::syntax::ast::StmtKind::Return { value: Some(e) } => expr_contains_widget(e),
        crate::syntax::ast::StmtKind::If {
            then_block,
            else_block,
            ..
        } => {
            block_contains_widget(then_block)
                || else_block.as_ref().is_some_and(block_contains_widget)
        }
        crate::syntax::ast::StmtKind::Block(inner) => block_contains_widget(inner),
        _ => false,
    })
}

fn expr_contains_widget(expr: &Expr) -> bool {
    match &expr.kind {
        ExprKind::Jsx { .. } => true,
        ExprKind::ArrayLit(els) => els.iter().any(expr_contains_widget),
        ExprKind::Binary {
            op: BinaryOp::And,
            rhs,
            ..
        } => expr_contains_widget(rhs),
        ExprKind::Binary {
            op: BinaryOp::NullishCoalesce,
            lhs,
            rhs,
        } => expr_contains_widget(lhs) || expr_contains_widget(rhs),
        ExprKind::If {
            then_branch,
            else_branch,
            ..
        } => {
            then_branch
                .tail_expr
                .as_deref()
                .is_some_and(expr_contains_widget)
                || else_branch
                    .tail_expr
                    .as_deref()
                    .is_some_and(expr_contains_widget)
        }
        ExprKind::MethodCall {
            receiver,
            method,
            args,
        } => match method.name.as_str() {
            "map" | "flatMap" => args.first().is_some_and(|a| match &a.kind {
                ExprKind::Arrow {
                    body: crate::syntax::ast::ArrowBody::Expr(e),
                    ..
                } => expr_contains_widget(e),
                ExprKind::Arrow {
                    body: crate::syntax::ast::ArrowBody::Block(blk),
                    ..
                } => block_contains_widget(blk),
                _ => false,
            }),
            "filter" | "slice" | "reverse" => expr_contains_widget(receiver),
            "concat" => {
                expr_contains_widget(receiver) || args.first().is_some_and(expr_contains_widget)
            }
            _ => false,
        },
        _ => false,
    }
}

fn builtin_jsx_kind(tag: &str) -> Option<&'static str> {
    match tag {
        "Column" | "Col" => Some("col"),
        "Row" => Some("row"),
        "Grid" => Some("grid"),
        "Stack" | "ZStack" => Some("stack"),
        "Card" | "Box" => Some("card"),
        "Scroll" | "ScrollView" => Some("scroll"),
        "Text" => Some("text"),
        "Code" => Some("code"),
        "Badge" | "Pill" => Some("badge"),
        "Button" => Some("button"),
        "Input" | "TextInput" => Some("input"),
        "Slider" => Some("slider"),
        "Toggle" | "Switch" => Some("toggle"),
        "Shader" | "ShaderView" => Some("shader"),
        "Icon" => Some("icon"),
        "Image" => Some("image"),
        "Divider" => Some("divider"),
        "Progress" => Some("progress"),
        "Spacer" => Some("spacer"),
        _ => None,
    }
}
