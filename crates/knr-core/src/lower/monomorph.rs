//! On-demand AST monomorphization for generic `type`, `enum`, and `function` declarations
//! (e.g., `Option<i64>` -> `Option__i64`, `Result<string, i64>` -> `Result__str__i64`,
//! `Pair<i64, string>` -> `Pair__i64__str`, `first<i64, string>` -> `first__i64__str`).
//!
//! Runs immediately before SSA lowering so all concrete specialized types, enums, and functions
//! are verified by the existing canonical linear SSA pipeline with zero new IR opcodes.

use std::collections::{HashMap, HashSet};

use crate::lower::error::LowerError;
use crate::syntax::ast::{
    mangle_generic_name, ArrowBody, Block, ConstDef, Declaration, EnumDef, EnumVariantDef, Expr,
    ExprKind, FieldDef, FieldInit, FunctionDef, Ident, JsxChild, JsxProp, MatchArm, MatchPattern,
    ParamDef, Program, Stmt, StmtKind, StructDef, TemplatePart, TypeExpr, TypeExprKind,
};

const MAX_MONOMORPH_DEPTH: usize = 32;

/// Registry of generic AST templates collected across imported stdlib and user modules.
#[derive(Debug, Clone, Default)]
pub struct GenericRegistry {
    pub structs: HashMap<String, StructDef>,
    pub enums: HashMap<String, EnumDef>,
    pub functions: HashMap<String, FunctionDef>,
}

impl GenericRegistry {
    pub fn register_from_program(&mut self, prog: &Program) {
        for decl in &prog.declarations {
            match decl {
                Declaration::Struct(s) if !s.type_params.is_empty() => {
                    self.structs.insert(s.name.name.clone(), s.clone());
                }
                Declaration::Enum(e) if !e.type_params.is_empty() => {
                    self.enums.insert(e.name.name.clone(), e.clone());
                }
                Declaration::Function(f) if !f.type_params.is_empty() => {
                    self.functions.insert(f.name.name.clone(), f.clone());
                }
                _ => {}
            }
        }
    }
}

/// Monomorphizes all concrete instantiations of generic structs, enums, and functions
/// referenced by `prog`, returning a purely concrete `Program` ready for SSA lowering.
pub fn monomorphize_program(
    prog: &Program,
    generics: &mut GenericRegistry,
    existing_schemas: &HashSet<String>,
    existing_fns: &HashSet<String>,
) -> Result<Program, LowerError> {
    generics.register_from_program(prog);

    // Collect only concrete declarations from this program
    let mut concrete_decls: Vec<Declaration> = Vec::with_capacity(prog.declarations.len());
    for decl in &prog.declarations {
        match decl {
            Declaration::Struct(s) if !s.type_params.is_empty() => {}
            Declaration::Enum(e) if !e.type_params.is_empty() => {}
            Declaration::Function(f) if !f.type_params.is_empty() => {}
            other => concrete_decls.push(other.clone()),
        }
    }

    let mut mono = Monomorphizer {
        generics,
        existing_schemas,
        existing_fns,
        instantiated_types: HashSet::new(),
        instantiated_fns: HashSet::new(),
        out_types: Vec::new(),
        out_fns: Vec::new(),
        depth: 0,
    };

    for decl in &mut concrete_decls {
        mono.visit_decl(decl)?;
    }

    // Order: synthesized specialized structs/enums -> concrete declarations -> synthesized specialized functions
    let mut final_decls = Vec::with_capacity(
        mono.out_types.len() + concrete_decls.len() + mono.out_fns.len(),
    );
    final_decls.extend(mono.out_types);
    final_decls.extend(concrete_decls);
    final_decls.extend(mono.out_fns);

    Ok(Program {
        imports: prog.imports.clone(),
        declarations: final_decls,
        span: prog.span,
    })
}

struct Monomorphizer<'a> {
    generics: &'a GenericRegistry,
    existing_schemas: &'a HashSet<String>,
    existing_fns: &'a HashSet<String>,
    instantiated_types: HashSet<String>,
    instantiated_fns: HashSet<String>,
    out_types: Vec<Declaration>,
    out_fns: Vec<Declaration>,
    depth: usize,
}

impl<'a> Monomorphizer<'a> {
    fn visit_decl(&mut self, decl: &mut Declaration) -> Result<(), LowerError> {
        match decl {
            Declaration::Struct(s) => {
                for f in &mut s.fields {
                    self.visit_type_expr(&mut f.ty)?;
                }
            }
            Declaration::Enum(e) => {
                for v in &mut e.variants {
                    for fty in &mut v.fields {
                        self.visit_type_expr(fty)?;
                    }
                }
            }
            Declaration::Const(c) => {
                if let Some(ref mut ty) = c.ty {
                    self.visit_type_expr(ty)?;
                }
                self.visit_expr(&mut c.value)?;
            }
            Declaration::Function(f) => {
                for p in &mut f.params {
                    self.visit_type_expr(&mut p.ty)?;
                }
                self.visit_type_expr(&mut f.ret_ty)?;
                self.visit_block(&mut f.body)?;
            }
        }
        Ok(())
    }

    fn visit_type_expr(&mut self, ty: &mut TypeExpr) -> Result<(), LowerError> {
        match &mut ty.kind {
            TypeExprKind::Vec(inner) | TypeExprKind::Chan(inner) => {
                self.visit_type_expr(inner)?;
            }
            TypeExprKind::Map(k, v) => {
                self.visit_type_expr(k)?;
                self.visit_type_expr(v)?;
            }
            TypeExprKind::Named(ident) | TypeExprKind::Struct(ident) => {
                if !ty.type_args.is_empty() {
                    for arg in &mut ty.type_args {
                        self.visit_type_expr(arg)?;
                    }
                    let mangled =
                        self.instantiate_type(&ident.name, &ty.type_args, ty.span.line)?;
                    ident.name = mangled;
                    ty.type_args.clear();
                }
            }
        }
        Ok(())
    }

    fn visit_block(&mut self, block: &mut Block) -> Result<(), LowerError> {
        for stmt in &mut block.stmts {
            self.visit_stmt(stmt)?;
        }
        if let Some(ref mut tail) = block.tail_expr {
            self.visit_expr(tail)?;
        }
        Ok(())
    }

    fn visit_stmt(&mut self, stmt: &mut Stmt) -> Result<(), LowerError> {
        match &mut stmt.kind {
            StmtKind::Let { ty, init, .. } => {
                if let Some(ref mut t) = ty {
                    self.visit_type_expr(t)?;
                }
                self.visit_expr(init)?;
            }
            StmtKind::Assign { target, value } => {
                self.visit_expr(target)?;
                self.visit_expr(value)?;
            }
            StmtKind::Return { value: Some(e) } => self.visit_expr(e)?,
            StmtKind::If {
                cond,
                then_block,
                else_block,
            } => {
                self.visit_expr(cond)?;
                self.visit_block(then_block)?;
                if let Some(ref mut eb) = else_block {
                    self.visit_block(eb)?;
                }
            }
            StmtKind::While { cond, body } => {
                self.visit_expr(cond)?;
                self.visit_block(body)?;
            }
            StmtKind::Syscall { payload, .. } => self.visit_expr(payload)?,
            StmtKind::Expr(e) => self.visit_expr(e)?,
            StmtKind::Block(blk) => self.visit_block(blk)?,
            StmtKind::Return { value: None } | StmtKind::Break | StmtKind::Continue => {}
        }
        Ok(())
    }

    fn visit_expr(&mut self, expr: &mut Expr) -> Result<(), LowerError> {
        let line = expr.span.line;
        match &mut expr.kind {
            ExprKind::Call {
                callee,
                type_args,
                args,
            } => {
                for a in args.iter_mut() {
                    self.visit_expr(a)?;
                }
                if !type_args.is_empty() {
                    for targ in type_args.iter_mut() {
                        self.visit_type_expr(targ)?;
                    }
                    let mangled = self.instantiate_fn(&callee.name, type_args, line)?;
                    callee.name = mangled;
                    type_args.clear();
                }
            }
            ExprKind::EnumLit {
                enum_name,
                type_args,
                args,
                ..
            } => {
                for a in args.iter_mut() {
                    self.visit_expr(a)?;
                }
                if !type_args.is_empty() {
                    for targ in type_args.iter_mut() {
                        self.visit_type_expr(targ)?;
                    }
                    let mangled = self.instantiate_type(&enum_name.name, type_args, line)?;
                    enum_name.name = mangled;
                    type_args.clear();
                }
            }
            ExprKind::MethodCall { receiver, args, .. } => {
                self.visit_expr(receiver)?;
                for a in args {
                    self.visit_expr(a)?;
                }
            }
            ExprKind::Arrow { body, .. } => match body {
                ArrowBody::Expr(e) => self.visit_expr(e)?,
                ArrowBody::Block(blk) => self.visit_block(blk)?,
            },
            ExprKind::StructLit { fields, spread, .. } => {
                for f in fields {
                    self.visit_expr(&mut f.value)?;
                }
                if let Some(ref mut sp) = spread {
                    self.visit_expr(sp)?;
                }
            }
            ExprKind::ArrayLit(elems) => {
                for e in elems {
                    self.visit_expr(e)?;
                }
            }
            ExprKind::Borrow(inner) | ExprKind::Unary { operand: inner, .. } => {
                self.visit_expr(inner)?;
            }
            ExprKind::Binary { lhs, rhs, .. } => {
                self.visit_expr(lhs)?;
                self.visit_expr(rhs)?;
            }
            ExprKind::Syscall { payload, .. } => self.visit_expr(payload)?,
            ExprKind::FieldAccess { target, .. } => self.visit_expr(target)?,
            ExprKind::Index { target, index } => {
                self.visit_expr(target)?;
                self.visit_expr(index)?;
            }
            ExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => {
                self.visit_expr(cond)?;
                self.visit_block(then_branch)?;
                self.visit_block(else_branch)?;
            }
            ExprKind::Match { scrutinee, arms } => {
                self.visit_expr(scrutinee)?;
                for arm in arms {
                    self.visit_expr(&mut arm.body)?;
                }
            }
            ExprKind::Block(blk) => self.visit_block(blk)?,
            ExprKind::TemplateStr(parts) => {
                for p in parts {
                    if let TemplatePart::Expr(ref mut e) = p {
                        self.visit_expr(e)?;
                    }
                }
            }
            ExprKind::Jsx {
                props, children, ..
            } => {
                for p in props {
                    if let Some(ref mut val) = p.value {
                        self.visit_expr(val)?;
                    }
                }
                for ch in children {
                    match ch {
                        JsxChild::Expr(e) | JsxChild::Element(e) => self.visit_expr(e)?,
                        JsxChild::Text(_, _) => {}
                    }
                }
            }
            ExprKind::Int(_)
            | ExprKind::Float(_)
            | ExprKind::Bool(_)
            | ExprKind::Str(_)
            | ExprKind::ColorHex(_)
            | ExprKind::Variable(_) => {}
        }
        Ok(())
    }

    fn instantiate_type(
        &mut self,
        base_name: &str,
        type_args: &[TypeExpr],
        line: usize,
    ) -> Result<String, LowerError> {
        let mut core_args = Vec::with_capacity(type_args.len());
        for arg in type_args {
            core_args.push(arg.to_core_type().map_err(|msg| {
                LowerError::InvalidConstruct { line, msg }
            })?);
        }
        let mangled = mangle_generic_name(base_name, &core_args);

        if self.instantiated_types.contains(&mangled) || self.existing_schemas.contains(&mangled) {
            return Ok(mangled);
        }

        if self.depth >= MAX_MONOMORPH_DEPTH {
            return Err(LowerError::InvalidConstruct {
                line,
                msg: format!(
                    "Maximum generic instantiation depth ({MAX_MONOMORPH_DEPTH}) exceeded while instantiating '{mangled}'"
                ),
            });
        }

        if let Some(template) = self.generics.structs.get(base_name).cloned() {
            if template.type_params.len() != type_args.len() {
                return Err(LowerError::InvalidConstruct {
                    line,
                    msg: format!(
                        "Generic type '{}' expects {} type argument(s), got {}",
                        base_name,
                        template.type_params.len(),
                        type_args.len()
                    ),
                });
            }
            self.instantiated_types.insert(mangled.clone());
            let subst = build_subst(&template.type_params, type_args);
            let mut specialized = StructDef {
                name: Ident::new(mangled.clone(), template.name.span),
                type_params: Vec::new(),
                fields: template
                    .fields
                    .iter()
                    .map(|f| FieldDef {
                        index: f.index,
                        name: f.name.clone(),
                        ty: subst_type_expr(&f.ty, &subst),
                        span: f.span,
                    })
                    .collect(),
                is_exported: template.is_exported,
                span: template.span,
            };
            self.depth += 1;
            let mut decl = Declaration::Struct(specialized.clone());
            self.visit_decl(&mut decl)?;
            self.depth -= 1;
            if let Declaration::Struct(s) = decl {
                specialized = s;
            }
            self.out_types.push(Declaration::Struct(specialized));
            return Ok(mangled);
        }

        if let Some(template) = self.generics.enums.get(base_name).cloned() {
            if template.type_params.len() != type_args.len() {
                return Err(LowerError::InvalidConstruct {
                    line,
                    msg: format!(
                        "Generic enum '{}' expects {} type argument(s), got {}",
                        base_name,
                        template.type_params.len(),
                        type_args.len()
                    ),
                });
            }
            self.instantiated_types.insert(mangled.clone());
            let subst = build_subst(&template.type_params, type_args);
            let mut specialized = EnumDef {
                name: Ident::new(mangled.clone(), template.name.span),
                type_params: Vec::new(),
                variants: template
                    .variants
                    .iter()
                    .map(|v| EnumVariantDef {
                        name: v.name.clone(),
                        fields: v.fields.iter().map(|t| subst_type_expr(t, &subst)).collect(),
                        span: v.span,
                    })
                    .collect(),
                is_exported: template.is_exported,
                span: template.span,
            };
            self.depth += 1;
            let mut decl = Declaration::Enum(specialized.clone());
            self.visit_decl(&mut decl)?;
            self.depth -= 1;
            if let Declaration::Enum(e) = decl {
                specialized = e;
            }
            self.out_types.push(Declaration::Enum(specialized));
            return Ok(mangled);
        }

        Err(LowerError::UnknownSchema {
            line,
            name: format!("{base_name}<...>"),
        })
    }

    fn instantiate_fn(
        &mut self,
        base_name: &str,
        type_args: &[TypeExpr],
        line: usize,
    ) -> Result<String, LowerError> {
        let mut core_args = Vec::with_capacity(type_args.len());
        for arg in type_args {
            core_args.push(arg.to_core_type().map_err(|msg| {
                LowerError::InvalidConstruct { line, msg }
            })?);
        }
        let mangled = mangle_generic_name(base_name, &core_args);

        if self.instantiated_fns.contains(&mangled) || self.existing_fns.contains(&mangled) {
            return Ok(mangled);
        }

        if self.depth >= MAX_MONOMORPH_DEPTH {
            return Err(LowerError::InvalidConstruct {
                line,
                msg: format!(
                    "Maximum generic instantiation depth ({MAX_MONOMORPH_DEPTH}) exceeded while instantiating function '{mangled}'"
                ),
            });
        }

        let template = self
            .generics
            .functions
            .get(base_name)
            .cloned()
            .ok_or_else(|| LowerError::UnknownFunction {
                line,
                name: format!("{base_name}<...>"),
            })?;

        if template.type_params.len() != type_args.len() {
            return Err(LowerError::InvalidConstruct {
                line,
                msg: format!(
                    "Generic function '{}' expects {} type argument(s), got {}",
                    base_name,
                    template.type_params.len(),
                    type_args.len()
                ),
            });
        }

        self.instantiated_fns.insert(mangled.clone());
        let subst = build_subst(&template.type_params, type_args);
        let specialized = FunctionDef {
            name: Ident::new(mangled.clone(), template.name.span),
            type_params: Vec::new(),
            params: template
                .params
                .iter()
                .map(|p| ParamDef {
                    name: p.name.clone(),
                    ty: subst_type_expr(&p.ty, &subst),
                    is_borrowed: p.is_borrowed,
                    span: p.span,
                })
                .collect(),
            ret_ty: subst_type_expr(&template.ret_ty, &subst),
            effects: template.effects.clone(),
            is_shader: template.is_shader,
            is_exported: template.is_exported,
            body: subst_block(&template.body, &subst),
            span: template.span,
        };

        self.depth += 1;
        let mut decl = Declaration::Function(specialized);
        self.visit_decl(&mut decl)?;
        self.depth -= 1;
        self.out_fns.push(decl);
        Ok(mangled)
    }
}

fn build_subst(params: &[Ident], args: &[TypeExpr]) -> HashMap<String, TypeExpr> {
    params
        .iter()
        .zip(args.iter())
        .map(|(p, a)| (p.name.clone(), a.clone()))
        .collect()
}

fn subst_type_expr(ty: &TypeExpr, subst: &HashMap<String, TypeExpr>) -> TypeExpr {
    match &ty.kind {
        TypeExprKind::Named(ident) if ty.type_args.is_empty() => {
            if let Some(replacement) = subst.get(&ident.name) {
                let mut cloned = replacement.clone();
                cloned.span = ty.span;
                return cloned;
            }
            ty.clone()
        }
        TypeExprKind::Named(ident) => TypeExpr::with_type_args(
            TypeExprKind::Named(ident.clone()),
            ty.type_args
                .iter()
                .map(|a| subst_type_expr(a, subst))
                .collect(),
            ty.span,
        ),
        TypeExprKind::Struct(ident) => TypeExpr::with_type_args(
            TypeExprKind::Struct(ident.clone()),
            ty.type_args
                .iter()
                .map(|a| subst_type_expr(a, subst))
                .collect(),
            ty.span,
        ),
        TypeExprKind::Vec(inner) => TypeExpr::new(
            TypeExprKind::Vec(Box::new(subst_type_expr(inner, subst))),
            ty.span,
        ),
        TypeExprKind::Map(k, v) => TypeExpr::new(
            TypeExprKind::Map(
                Box::new(subst_type_expr(k, subst)),
                Box::new(subst_type_expr(v, subst)),
            ),
            ty.span,
        ),
        TypeExprKind::Chan(inner) => TypeExpr::new(
            TypeExprKind::Chan(Box::new(subst_type_expr(inner, subst))),
            ty.span,
        ),
    }
}

fn subst_block(block: &Block, subst: &HashMap<String, TypeExpr>) -> Block {
    Block {
        stmts: block.stmts.iter().map(|s| subst_stmt(s, subst)).collect(),
        tail_expr: block
            .tail_expr
            .as_ref()
            .map(|e| Box::new(subst_expr(e, subst))),
        span: block.span,
    }
}

fn subst_stmt(stmt: &Stmt, subst: &HashMap<String, TypeExpr>) -> Stmt {
    let kind = match &stmt.kind {
        StmtKind::Let {
            name,
            ty,
            init,
            is_const,
            destruct,
        } => StmtKind::Let {
            name: name.clone(),
            ty: ty.as_ref().map(|t| subst_type_expr(t, subst)),
            init: subst_expr(init, subst),
            is_const: *is_const,
            destruct: destruct.clone(),
        },
        StmtKind::Assign { target, value } => StmtKind::Assign {
            target: subst_expr(target, subst),
            value: subst_expr(value, subst),
        },
        StmtKind::Return { value } => StmtKind::Return {
            value: value.as_ref().map(|e| subst_expr(e, subst)),
        },
        StmtKind::If {
            cond,
            then_block,
            else_block,
        } => StmtKind::If {
            cond: subst_expr(cond, subst),
            then_block: subst_block(then_block, subst),
            else_block: else_block.as_ref().map(|b| subst_block(b, subst)),
        },
        StmtKind::While { cond, body } => StmtKind::While {
            cond: subst_expr(cond, subst),
            body: subst_block(body, subst),
        },
        StmtKind::Break => StmtKind::Break,
        StmtKind::Continue => StmtKind::Continue,
        StmtKind::Syscall {
            effect,
            op,
            payload,
        } => StmtKind::Syscall {
            effect: effect.clone(),
            op: op.clone(),
            payload: subst_expr(payload, subst),
        },
        StmtKind::Expr(e) => StmtKind::Expr(subst_expr(e, subst)),
        StmtKind::Block(blk) => StmtKind::Block(subst_block(blk, subst)),
    };
    Stmt {
        kind,
        span: stmt.span,
    }
}

fn subst_expr(expr: &Expr, subst: &HashMap<String, TypeExpr>) -> Expr {
    let kind = match &expr.kind {
        ExprKind::Int(v) => ExprKind::Int(*v),
        ExprKind::Float(v) => ExprKind::Float(*v),
        ExprKind::Bool(v) => ExprKind::Bool(*v),
        ExprKind::Str(s) => ExprKind::Str(s.clone()),
        ExprKind::ColorHex(h) => ExprKind::ColorHex(h.clone()),
        ExprKind::Variable(id) => ExprKind::Variable(id.clone()),
        ExprKind::TemplateStr(parts) => ExprKind::TemplateStr(
            parts
                .iter()
                .map(|p| match p {
                    TemplatePart::Literal(s) => TemplatePart::Literal(s.clone()),
                    TemplatePart::Expr(e) => TemplatePart::Expr(subst_expr(e, subst)),
                })
                .collect(),
        ),
        ExprKind::StructLit {
            name,
            fields,
            spread,
        } => ExprKind::StructLit {
            name: name.clone(),
            fields: fields
                .iter()
                .map(|f| FieldInit {
                    name: f.name.clone(),
                    value: subst_expr(&f.value, subst),
                    span: f.span,
                })
                .collect(),
            spread: spread.as_ref().map(|s| Box::new(subst_expr(s, subst))),
        },
        ExprKind::EnumLit {
            enum_name,
            type_args,
            variant,
            args,
        } => ExprKind::EnumLit {
            enum_name: enum_name.clone(),
            type_args: type_args
                .iter()
                .map(|t| subst_type_expr(t, subst))
                .collect(),
            variant: variant.clone(),
            args: args.iter().map(|a| subst_expr(a, subst)).collect(),
        },
        ExprKind::ArrayLit(elems) => {
            ExprKind::ArrayLit(elems.iter().map(|e| subst_expr(e, subst)).collect())
        }
        ExprKind::Borrow(inner) => ExprKind::Borrow(Box::new(subst_expr(inner, subst))),
        ExprKind::Arrow { params, body } => ExprKind::Arrow {
            params: params.clone(),
            body: match body {
                ArrowBody::Expr(e) => ArrowBody::Expr(Box::new(subst_expr(e, subst))),
                ArrowBody::Block(blk) => ArrowBody::Block(subst_block(blk, subst)),
            },
        },
        ExprKind::Jsx {
            tag,
            props,
            children,
        } => ExprKind::Jsx {
            tag: tag.clone(),
            props: props
                .iter()
                .map(|p| JsxProp {
                    name: p.name.clone(),
                    value: p.value.as_ref().map(|v| subst_expr(v, subst)),
                    span: p.span,
                })
                .collect(),
            children: children
                .iter()
                .map(|c| match c {
                    JsxChild::Text(t, sp) => JsxChild::Text(t.clone(), *sp),
                    JsxChild::Expr(e) => JsxChild::Expr(subst_expr(e, subst)),
                    JsxChild::Element(e) => JsxChild::Element(subst_expr(e, subst)),
                })
                .collect(),
        },
        ExprKind::Unary { op, operand } => ExprKind::Unary {
            op: *op,
            operand: Box::new(subst_expr(operand, subst)),
        },
        ExprKind::Binary { op, lhs, rhs } => ExprKind::Binary {
            op: *op,
            lhs: Box::new(subst_expr(lhs, subst)),
            rhs: Box::new(subst_expr(rhs, subst)),
        },
        ExprKind::Call {
            callee,
            type_args,
            args,
        } => ExprKind::Call {
            callee: callee.clone(),
            type_args: type_args
                .iter()
                .map(|t| subst_type_expr(t, subst))
                .collect(),
            args: args.iter().map(|a| subst_expr(a, subst)).collect(),
        },
        ExprKind::MethodCall {
            receiver,
            method,
            args,
        } => ExprKind::MethodCall {
            receiver: Box::new(subst_expr(receiver, subst)),
            method: method.clone(),
            args: args.iter().map(|a| subst_expr(a, subst)).collect(),
        },
        ExprKind::Syscall {
            effect,
            op,
            payload,
        } => ExprKind::Syscall {
            effect: effect.clone(),
            op: op.clone(),
            payload: Box::new(subst_expr(payload, subst)),
        },
        ExprKind::FieldAccess { target, field } => ExprKind::FieldAccess {
            target: Box::new(subst_expr(target, subst)),
            field: field.clone(),
        },
        ExprKind::Index { target, index } => ExprKind::Index {
            target: Box::new(subst_expr(target, subst)),
            index: Box::new(subst_expr(index, subst)),
        },
        ExprKind::If {
            cond,
            then_branch,
            else_branch,
        } => ExprKind::If {
            cond: Box::new(subst_expr(cond, subst)),
            then_branch: subst_block(then_branch, subst),
            else_branch: subst_block(else_branch, subst),
        },
        ExprKind::Match { scrutinee, arms } => ExprKind::Match {
            scrutinee: Box::new(subst_expr(scrutinee, subst)),
            arms: arms
                .iter()
                .map(|arm| MatchArm {
                    pattern: match &arm.pattern {
                        MatchPattern::Variant {
                            enum_name,
                            variant,
                            bindings,
                        } => MatchPattern::Variant {
                            enum_name: enum_name.clone(),
                            variant: variant.clone(),
                            bindings: bindings.clone(),
                        },
                        other => other.clone(),
                    },
                    body: subst_expr(&arm.body, subst),
                    span: arm.span,
                })
                .collect(),
        },
        ExprKind::Block(blk) => ExprKind::Block(subst_block(blk, subst)),
    };
    Expr {
        kind,
        span: expr.span,
    }
}

#[allow(dead_code)]
fn _unused_const_marker(_: &ConstDef) {}
