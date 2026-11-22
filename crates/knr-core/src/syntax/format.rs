//! Canonical Strict TypeScript / TSX (`.tsk`) source formatter.
//!
//! Parses input via `parse_tsx` (so invalid syntax is never mangled) and emits
//! deterministic, 4-space indented, strictly idempotent `.tsk` code:
//! `format_tsk(&format_tsk(src)?)? == format_tsk(src)?`.

use std::fmt::Write;

use crate::error::ParseError;
use crate::syntax::ast::*;
use crate::syntax::parse_tsx;

const INDENT: &str = "    ";

/// Formats Strict TypeScript / TSX (`.tsk`) source code into canonical deterministic form.
///
/// Guarantees:
/// - Rejects syntactically invalid source via `parse_tsx(src)?` without mangling.
/// - Preserves leading file header `//` comments.
/// - Strictly idempotent: `format_tsk(&format_tsk(src)?)? == format_tsk(src)?`.
/// - Output re-parses and lowers identically.
pub fn format_tsk(src: &str) -> Result<String, ParseError> {
    let prog = parse_tsx(src)?;
    let header_comments = extract_leading_comments(src);

    let mut out = String::with_capacity(src.len() + 128);

    if !header_comments.is_empty() {
        for line in &header_comments {
            out.push_str(line);
            out.push('\n');
        }
        if !prog.imports.is_empty() || !prog.declarations.is_empty() {
            out.push('\n');
        }
    }

    if !prog.imports.is_empty() {
        for imp in &prog.imports {
            format_import(imp, &mut out);
        }
        if !prog.declarations.is_empty() {
            out.push('\n');
        }
    }

    for (idx, decl) in prog.declarations.iter().enumerate() {
        if idx > 0 {
            out.push('\n');
        }
        format_declaration(decl, &mut out);
    }

    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }

    Ok(out)
}

fn extract_leading_comments(src: &str) -> Vec<String> {
    let mut lines = Vec::new();
    for raw in src.lines() {
        let trimmed_start = raw.trim_start();
        if trimmed_start.starts_with("//") {
            lines.push(raw.trim_end().to_string());
        } else if trimmed_start.is_empty() {
            if !lines.is_empty() {
                lines.push(String::new());
            }
        } else {
            break;
        }
    }
    while lines.last().is_some_and(|l| l.is_empty()) {
        lines.pop();
    }
    lines
}

fn push_indent(out: &mut String, depth: usize) {
    for _ in 0..depth {
        out.push_str(INDENT);
    }
}

fn indent_str(depth: usize) -> String {
    INDENT.repeat(depth)
}

fn format_import(imp: &ImportDecl, out: &mut String) {
    let path = escape_str(&imp.path);
    let kw = if imp.is_reexport { "export" } else { "import" };
    match &imp.kind {
        ImportKind::SideEffect => {
            if imp.is_reexport {
                let _ = writeln!(out, "export * from \"{path}\";");
            } else {
                let _ = writeln!(out, "import \"{path}\";");
            }
        }
        ImportKind::Namespace(alias) => {
            let _ = writeln!(out, "{kw} * as {} from \"{path}\";", alias.name);
        }
        ImportKind::Named(items) => {
            let rendered: Vec<String> = items
                .iter()
                .map(|(n, a)| match a {
                    Some(al) => format!("{} as {}", n.name, al.name),
                    None => n.name.clone(),
                })
                .collect();
            let single = format!("{kw} {{ {} }} from \"{path}\";", rendered.join(", "));
            if rendered.len() <= 6 && single.len() <= 96 {
                out.push_str(&single);
                out.push('\n');
            } else {
                let _ = writeln!(out, "{kw} {{");
                for item in &rendered {
                    let _ = writeln!(out, "{INDENT}{item},");
                }
                let _ = writeln!(out, "}} from \"{path}\";");
            }
        }
        ImportKind::Default { name, named } => {
            if named.is_empty() {
                let _ = writeln!(out, "{kw} {} from \"{path}\";", name.name);
            } else {
                let rendered: Vec<String> = named
                    .iter()
                    .map(|(n, a)| match a {
                        Some(al) => format!("{} as {}", n.name, al.name),
                        None => n.name.clone(),
                    })
                    .collect();
                let _ = writeln!(
                    out,
                    "{kw} {}, {{ {} }} from \"{path}\";",
                    name.name,
                    rendered.join(", ")
                );
            }
        }
    }
}

fn format_type_params(params: &[Ident]) -> String {
    if params.is_empty() {
        String::new()
    } else {
        let names: Vec<&str> = params.iter().map(|p| p.name.as_str()).collect();
        format!("<{}>", names.join(", "))
    }
}

fn format_type_expr(ty: &TypeExpr) -> String {
    match &ty.kind {
        TypeExprKind::Named(ident) | TypeExprKind::Struct(ident) => {
            if ty.type_args.is_empty() {
                ident.name.clone()
            } else {
                let args: Vec<String> = ty.type_args.iter().map(format_type_expr).collect();
                format!("{}<{}>", ident.name, args.join(", "))
            }
        }
        TypeExprKind::Vec(inner) => format!("{}[]", format_type_expr(inner)),
        TypeExprKind::Map(k, v) => {
            format!("Map<{}, {}>", format_type_expr(k), format_type_expr(v))
        }
        TypeExprKind::Chan(inner) => format!("Channel<{}>", format_type_expr(inner)),
    }
}

fn format_declaration(decl: &Declaration, out: &mut String) {
    match decl {
        Declaration::Const(c) => {
            let exp = if c.is_exported { "export " } else { "" };
            let ty_part = match &c.ty {
                Some(t) => format!(": {}", format_type_expr(t)),
                None => String::new(),
            };
            let val = format_expr(&c.value, 0, 0);
            let _ = writeln!(out, "{exp}const {}{ty_part} = {val};", c.name.name);
        }
        Declaration::Struct(s) => {
            let exp = if s.is_exported { "export " } else { "" };
            let tparams = format_type_params(&s.type_params);
            if s.fields.is_empty() {
                let _ = writeln!(out, "{exp}type {}{tparams} = {{}};", s.name.name);
            } else {
                let _ = writeln!(out, "{exp}type {}{tparams} = {{", s.name.name);
                for f in &s.fields {
                    let _ = writeln!(out, "{INDENT}{}: {};", f.name.name, format_type_expr(&f.ty));
                }
                out.push_str("};\n");
            }
        }
        Declaration::Enum(e) => {
            let exp = if e.is_exported { "export " } else { "" };
            let tparams = format_type_params(&e.type_params);
            if e.variants.is_empty() {
                let _ = writeln!(out, "{exp}enum {}{tparams} {{}}", e.name.name);
            } else {
                let _ = writeln!(out, "{exp}enum {}{tparams} {{", e.name.name);
                for v in &e.variants {
                    if v.fields.is_empty() {
                        let _ = writeln!(out, "{INDENT}{},", v.name.name);
                    } else {
                        let fields: Vec<String> = v.fields.iter().map(format_type_expr).collect();
                        let _ = writeln!(out, "{INDENT}{}({}),", v.name.name, fields.join(", "));
                    }
                }
                out.push_str("}\n");
            }
        }
        Declaration::Function(f) => {
            if f.is_shader {
                out.push_str("@shader\n");
            }
            if !f.effects.is_empty() {
                let effs: Vec<String> = f
                    .effects
                    .iter()
                    .map(|e| format!("\"{}\"", e.name.to_ascii_lowercase()))
                    .collect();
                let _ = writeln!(out, "@effects({})", effs.join(", "));
            }
            let exp = if f.is_exported { "export " } else { "" };
            let tparams = format_type_params(&f.type_params);
            let params: Vec<String> = f
                .params
                .iter()
                .map(|p| {
                    let ro = if p.is_borrowed { "readonly " } else { "" };
                    format!("{}: {ro}{}", p.name.name, format_type_expr(&p.ty))
                })
                .collect();
            let ret = format_type_expr(&f.ret_ty);
            let header = format!(
                "{exp}function {}{tparams}({}): {ret} {{",
                f.name.name,
                params.join(", ")
            );
            if header.len() <= 104 || params.is_empty() {
                out.push_str(&header);
                out.push('\n');
            } else {
                let _ = writeln!(out, "{exp}function {}{tparams}(", f.name.name);
                for p in &params {
                    let _ = writeln!(out, "{INDENT}{p},");
                }
                let _ = writeln!(out, "): {ret} {{");
            }
            format_block_contents(&f.body, 1, out);
            out.push_str("}\n");
        }
    }
}

fn format_block_contents(block: &Block, depth: usize, out: &mut String) {
    for stmt in &block.stmts {
        format_stmt(stmt, depth, out);
    }
    if let Some(ref tail) = block.tail_expr {
        push_indent(out, depth);
        out.push_str(&format_expr(tail, depth, 0));
        out.push_str(";\n");
    }
}

fn format_stmt(stmt: &Stmt, depth: usize, out: &mut String) {
    match &stmt.kind {
        StmtKind::Let {
            name,
            ty,
            init,
            is_const,
            destruct,
        } => {
            push_indent(out, depth);
            let kw = if *is_const { "const" } else { "let" };
            let ty_part = match ty {
                Some(t) => format!(": {}", format_type_expr(t)),
                None => String::new(),
            };
            let val = format_expr(init, depth, 0);
            match destruct {
                Some(DestructPattern::Object(fields)) => {
                    let rendered: Vec<String> = fields
                        .iter()
                        .map(|(f, alias)| match alias {
                            Some(al) => format!("{}: {}", f.name, al.name),
                            None => f.name.clone(),
                        })
                        .collect();
                    let _ = writeln!(out, "{kw} {{ {} }}{ty_part} = {val};", rendered.join(", "));
                }
                Some(DestructPattern::Array(elems)) => {
                    let rendered: Vec<&str> = elems.iter().map(|e| e.name.as_str()).collect();
                    let _ = writeln!(out, "{kw} [{}]{ty_part} = {val};", rendered.join(", "));
                }
                None => {
                    let _ = writeln!(out, "{kw} {}{ty_part} = {val};", name.name);
                }
            }
        }
        StmtKind::Assign { target, value } => {
            push_indent(out, depth);
            let lhs = format_expr(target, depth, 0);
            let rhs = format_expr(value, depth, 0);
            let _ = writeln!(out, "{lhs} = {rhs};");
        }
        StmtKind::Return { value: None } => {
            push_indent(out, depth);
            out.push_str("return;\n");
        }
        StmtKind::Return { value: Some(expr) } => {
            push_indent(out, depth);
            if matches!(expr.kind, ExprKind::Jsx { .. }) {
                let rendered = format_expr(expr, depth + 1, 0);
                if rendered.contains('\n') {
                    out.push_str("return (\n");
                    push_indent(out, depth + 1);
                    out.push_str(&rendered);
                    out.push('\n');
                    push_indent(out, depth);
                    out.push_str(");\n");
                    return;
                }
            }
            let val = format_expr(expr, depth, 0);
            let _ = writeln!(out, "return {val};");
        }
        StmtKind::If {
            cond,
            then_block,
            else_block,
        } => {
            push_indent(out, depth);
            format_if_chain(cond, then_block, else_block.as_ref(), depth, out);
            out.push('\n');
        }
        StmtKind::While { cond, body } => {
            push_indent(out, depth);
            let c = format_expr(cond, depth, 0);
            let _ = writeln!(out, "while ({c}) {{");
            format_block_contents(body, depth + 1, out);
            push_indent(out, depth);
            out.push_str("}\n");
        }
        StmtKind::Break => {
            push_indent(out, depth);
            out.push_str("break;\n");
        }
        StmtKind::Continue => {
            push_indent(out, depth);
            out.push_str("continue;\n");
        }
        StmtKind::Syscall {
            effect,
            op,
            payload,
        } => {
            push_indent(out, depth);
            let p = format_expr(payload, depth, 0);
            let _ = writeln!(
                out,
                "syscall(\"{}\", \"{}\", {p});",
                effect.name.to_ascii_lowercase(),
                escape_str(op)
            );
        }
        StmtKind::Expr(expr) => {
            if let ExprKind::Match { scrutinee, arms } = &expr.kind {
                format_match_as_switch(scrutinee, arms, depth, out);
                return;
            }
            push_indent(out, depth);
            let rendered = format_expr(expr, depth, 0);
            if matches!(expr.kind, ExprKind::StructLit { .. }) {
                let _ = writeln!(out, "({rendered});");
            } else {
                let _ = writeln!(out, "{rendered};");
            }
        }
        StmtKind::Block(blk) => {
            // Unwrap single-statement `if` blocks produced by desugared value `switch`
            if blk.stmts.len() == 1
                && blk.tail_expr.is_none()
                && matches!(blk.stmts[0].kind, StmtKind::If { .. })
            {
                format_stmt(&blk.stmts[0], depth, out);
                return;
            }
            push_indent(out, depth);
            out.push_str("{\n");
            format_block_contents(blk, depth + 1, out);
            push_indent(out, depth);
            out.push_str("}\n");
        }
    }
}

fn format_if_chain(
    cond: &Expr,
    then_block: &Block,
    else_block: Option<&Block>,
    depth: usize,
    out: &mut String,
) {
    let c = format_expr(cond, depth, 0);
    let _ = writeln!(out, "if ({c}) {{");
    format_block_contents(then_block, depth + 1, out);
    push_indent(out, depth);
    out.push('}');
    if let Some(eb) = else_block {
        if eb.stmts.len() == 1 && eb.tail_expr.is_none() {
            if let StmtKind::If {
                cond: ref c2,
                then_block: ref tb2,
                else_block: ref eb2,
            } = eb.stmts[0].kind
            {
                out.push_str(" else ");
                format_if_chain(c2, tb2, eb2.as_ref(), depth, out);
                return;
            }
        }
        out.push_str(" else {\n");
        format_block_contents(eb, depth + 1, out);
        push_indent(out, depth);
        out.push('}');
    }
}

fn format_match_as_switch(scrutinee: &Expr, arms: &[MatchArm], depth: usize, out: &mut String) {
    push_indent(out, depth);
    let scr = format_expr(scrutinee, depth, 0);
    let _ = writeln!(out, "switch ({scr}) {{");
    for arm in arms {
        push_indent(out, depth + 1);
        match &arm.pattern {
            MatchPattern::Wildcard => {
                out.push_str("default:\n");
            }
            MatchPattern::Int(v) => {
                let _ = writeln!(out, "case {v}:");
            }
            MatchPattern::Bool(b) => {
                let _ = writeln!(out, "case {b}:");
            }
            MatchPattern::Str(s) => {
                let _ = writeln!(out, "case \"{}\":", escape_str(s));
            }
            MatchPattern::Variant {
                enum_name,
                variant,
                bindings,
            } => {
                let prefix = match enum_name {
                    Some(en) => format!("{}.", en.name),
                    None => String::new(),
                };
                if bindings.is_empty() {
                    let _ = writeln!(out, "case {prefix}{}:", variant.name);
                } else {
                    let bs: Vec<&str> = bindings.iter().map(|b| b.name.as_str()).collect();
                    let _ = writeln!(out, "case {prefix}{}({}):", variant.name, bs.join(", "));
                }
            }
        }

        let needs_break = match &arm.body.kind {
            ExprKind::Block(blk) => {
                format_block_contents(blk, depth + 2, out);
                !block_ends_with_terminator(blk)
            }
            _ => {
                push_indent(out, depth + 2);
                let val = format_expr(&arm.body, depth + 2, 0);
                let _ = writeln!(out, "{val};");
                true
            }
        };
        if needs_break {
            push_indent(out, depth + 2);
            out.push_str("break;\n");
        }
    }
    push_indent(out, depth);
    out.push_str("}\n");
}

fn block_ends_with_terminator(blk: &Block) -> bool {
    matches!(
        blk.stmts.last().map(|s| &s.kind),
        Some(StmtKind::Return { .. } | StmtKind::Break | StmtKind::Continue)
    )
}

fn binary_prec(op: BinaryOp) -> u8 {
    match op {
        BinaryOp::NullishCoalesce => 1,
        BinaryOp::Or => 2,
        BinaryOp::And => 3,
        BinaryOp::Eq | BinaryOp::NotEq => 4,
        BinaryOp::Lt | BinaryOp::LtEq | BinaryOp::Gt | BinaryOp::GtEq => 5,
        BinaryOp::Add | BinaryOp::Sub => 6,
        BinaryOp::Mul | BinaryOp::Div | BinaryOp::Rem => 7,
    }
}

fn binary_op_str(op: BinaryOp) -> &'static str {
    match op {
        BinaryOp::Add => "+",
        BinaryOp::Sub => "-",
        BinaryOp::Mul => "*",
        BinaryOp::Div => "/",
        BinaryOp::Rem => "%",
        BinaryOp::Eq => "===",
        BinaryOp::NotEq => "!==",
        BinaryOp::Lt => "<",
        BinaryOp::LtEq => "<=",
        BinaryOp::Gt => ">",
        BinaryOp::GtEq => ">=",
        BinaryOp::And => "&&",
        BinaryOp::Or => "||",
        BinaryOp::NullishCoalesce => "??",
    }
}

fn expr_prec(expr: &Expr) -> u8 {
    match &expr.kind {
        ExprKind::Arrow { .. } | ExprKind::If { .. } => 0,
        ExprKind::Binary { op, .. } => binary_prec(*op),
        ExprKind::Unary { .. } | ExprKind::Borrow(_) => 8,
        _ => 9,
    }
}

fn format_expr(expr: &Expr, depth: usize, min_prec: u8) -> String {
    let my_prec = expr_prec(expr);
    let raw = match &expr.kind {
        ExprKind::Int(v) => v.to_string(),
        ExprKind::Float(v) => format_float(*v),
        ExprKind::Bool(b) => if *b { "true" } else { "false" }.to_string(),
        ExprKind::Str(s) => format!("\"{}\"", escape_str(s)),
        ExprKind::ColorHex(h) => h.clone(),
        ExprKind::TemplateStr(parts) => {
            let mut s = String::from("`");
            for part in parts {
                match part {
                    TemplatePart::Literal(lit) => s.push_str(&escape_template_lit(lit)),
                    TemplatePart::Expr(e) => {
                        s.push_str("${");
                        s.push_str(&format_expr(e, depth, 0));
                        s.push('}');
                    }
                }
            }
            s.push('`');
            s
        }
        ExprKind::Variable(id) => id.name.clone(),
        ExprKind::Borrow(inner) => {
            let s = format_expr(inner, depth, 8);
            format!("&{s}")
        }
        ExprKind::Unary { op, operand } => match op {
            UnaryOp::Neg => {
                let inner_prec = expr_prec(operand);
                let needs_parens = inner_prec < 8
                    || matches!(
                        operand.kind,
                        ExprKind::Unary {
                            op: UnaryOp::Neg,
                            ..
                        }
                    )
                    || matches!(operand.kind, ExprKind::Int(v) if v < 0)
                    || matches!(operand.kind, ExprKind::Float(v) if v < 0.0);
                if needs_parens {
                    format!("-({})", format_expr(operand, depth, 0))
                } else {
                    format!("-{}", format_expr(operand, depth, 8))
                }
            }
            UnaryOp::Not => {
                if expr_prec(operand) < 8 {
                    format!("!({})", format_expr(operand, depth, 0))
                } else {
                    format!("!{}", format_expr(operand, depth, 8))
                }
            }
        },
        ExprKind::Binary { op, lhs, rhs } => {
            let prec = binary_prec(*op);
            let l = format_expr(lhs, depth, prec);
            // Parser is strictly left-associative, so right child at equal precedence needs parens
            let r = format_expr(rhs, depth, prec + 1);
            format!("{l} {} {r}", binary_op_str(*op))
        }
        ExprKind::Call {
            callee,
            type_args,
            args,
        } => {
            let targs = if type_args.is_empty() {
                String::new()
            } else {
                let ts: Vec<String> = type_args.iter().map(format_type_expr).collect();
                format!("<{}>", ts.join(", "))
            };
            let rendered: Vec<String> = args.iter().map(|a| format_expr(a, depth, 0)).collect();
            format!("{}{targs}({})", callee.name, rendered.join(", "))
        }
        ExprKind::MethodCall {
            receiver,
            method,
            args,
        } => {
            let recv = format_expr(receiver, depth, 9);
            let rendered: Vec<String> = args.iter().map(|a| format_expr(a, depth, 0)).collect();
            format!("{recv}.{}({})", method.name, rendered.join(", "))
        }
        ExprKind::Syscall {
            effect,
            op,
            payload,
        } => {
            let p = format_expr(payload, depth, 0);
            format!(
                "syscall(\"{}\", \"{}\", {p})",
                effect.name.to_ascii_lowercase(),
                escape_str(op)
            )
        }
        ExprKind::FieldAccess { target, field } => {
            let t = format_expr(target, depth, 9);
            format!("{t}.{}", field.name)
        }
        ExprKind::Index { target, index } => {
            let t = format_expr(target, depth, 9);
            let i = format_expr(index, depth, 0);
            format!("{t}[{i}]")
        }
        ExprKind::ArrayLit(elems) => {
            if elems.is_empty() {
                "[]".to_string()
            } else {
                let rendered: Vec<String> = elems.iter().map(|e| format_expr(e, depth, 0)).collect();
                let single = format!("[{}]", rendered.join(", "));
                if !single.contains('\n') && single.len() <= 80 {
                    single
                } else {
                    let ind_inner = indent_str(depth + 1);
                    let ind_close = indent_str(depth);
                    let mut s = String::from("[\n");
                    for e in elems {
                        let _ = writeln!(s, "{ind_inner}{},", format_expr(e, depth + 1, 0));
                    }
                    let _ = write!(s, "{ind_close}]");
                    s
                }
            }
        }
        ExprKind::StructLit {
            name: _,
            fields,
            spread,
        } => {
            let mut items: Vec<String> = Vec::new();
            if let Some(sp) = spread {
                items.push(format!("...{}", format_expr(sp, depth, 0)));
            }
            for f in fields {
                items.push(format!("{}: {}", f.name.name, format_expr(&f.value, depth, 0)));
            }
            if items.is_empty() {
                "{}".to_string()
            } else {
                let single = format!("{{ {} }}", items.join(", "));
                let multiline_preferred = items.len() > 3
                    || (spread.is_some() && items.len() > 2)
                    || single.len() > 76
                    || single.contains('\n');
                if !multiline_preferred {
                    single
                } else {
                    let ind_inner = indent_str(depth + 1);
                    let ind_close = indent_str(depth);
                    let mut s = String::from("{\n");
                    if let Some(sp) = spread {
                        let _ = writeln!(s, "{ind_inner}...{},", format_expr(sp, depth + 1, 0));
                    }
                    for f in fields {
                        let _ = writeln!(
                            s,
                            "{ind_inner}{}: {},",
                            f.name.name,
                            format_expr(&f.value, depth + 1, 0)
                        );
                    }
                    let _ = write!(s, "{ind_close}}}");
                    s
                }
            }
        }
        ExprKind::EnumLit {
            enum_name,
            type_args,
            variant,
            args,
        } => {
            let targs = if type_args.is_empty() {
                String::new()
            } else {
                let ts: Vec<String> = type_args.iter().map(format_type_expr).collect();
                format!("<{}>", ts.join(", "))
            };
            if args.is_empty() {
                if !type_args.is_empty() {
                    format!("{}{targs}.{}", enum_name.name, variant.name)
                } else {
                    format!("{}.{}()", enum_name.name, variant.name)
                }
            } else {
                let rendered: Vec<String> = args.iter().map(|a| format_expr(a, depth, 0)).collect();
                format!(
                    "{}{targs}.{}({})",
                    enum_name.name,
                    variant.name,
                    rendered.join(", ")
                )
            }
        }
        ExprKind::Arrow { params, body } => {
            let pnames: Vec<&str> = params.iter().map(|p| p.name.as_str()).collect();
            let head = format!("({}) => ", pnames.join(", "));
            match body {
                ArrowBody::Expr(e) => match &e.kind {
                    ExprKind::StructLit { .. } => {
                        format!("{head}({})", format_expr(e, depth, 0))
                    }
                    ExprKind::Jsx { .. } => {
                        let rendered = format_expr(e, depth + 1, 0);
                        if rendered.contains('\n') {
                            let ind_inner = indent_str(depth + 1);
                            let ind_close = indent_str(depth);
                            format!("{head}(\n{ind_inner}{rendered}\n{ind_close})")
                        } else {
                            format!("{head}{rendered}")
                        }
                    }
                    _ => format!("{head}{}", format_expr(e, depth, 0)),
                },
                ArrowBody::Block(blk) => {
                    let mut s = format!("{head}{{\n");
                    format_block_contents(blk, depth + 1, &mut s);
                    push_indent(&mut s, depth);
                    s.push('}');
                    s
                }
            }
        }
        ExprKind::If {
            cond,
            then_branch,
            else_branch,
        } => {
            let c = format_expr(cond, depth, 1);
            let t = then_branch
                .tail_expr
                .as_ref()
                .map(|e| format_expr(e, depth, 1))
                .unwrap_or_else(|| "0".to_string());
            let e = else_branch
                .tail_expr
                .as_ref()
                .map(|e| format_expr(e, depth, 0))
                .unwrap_or_else(|| "0".to_string());
            format!("{c} ? {t} : {e}")
        }
        ExprKind::Match { scrutinee, arms } => {
            let mut s = String::new();
            format_match_as_switch(scrutinee, arms, depth, &mut s);
            s.trim_end().to_string()
        }
        ExprKind::Block(blk) => {
            let mut s = String::from("{\n");
            format_block_contents(blk, depth + 1, &mut s);
            push_indent(&mut s, depth);
            s.push('}');
            s
        }
        ExprKind::Jsx {
            tag,
            props,
            children,
        } => format_jsx(tag, props, children, depth),
    };

    if my_prec < min_prec {
        format!("({raw})")
    } else {
        raw
    }
}

fn format_jsx_prop(prop: &JsxProp, depth: usize) -> String {
    match &prop.value {
        None => prop.name.name.clone(),
        Some(expr) => {
            if let ExprKind::Str(s) = &expr.kind {
                if !s.contains('\n') && !s.contains('\r') && !s.contains('"') {
                    return format!("{}=\"{s}\"", prop.name.name);
                }
            }
            format!("{}={{{}}}", prop.name.name, format_expr(expr, depth, 0))
        }
    }
}

fn format_jsx(tag: &Ident, props: &[JsxProp], children: &[JsxChild], depth: usize) -> String {
    let rendered_props: Vec<String> = props.iter().map(|p| format_jsx_prop(p, depth + 1)).collect();
    let inline_props_str = if rendered_props.is_empty() {
        String::new()
    } else {
        format!(" {}", rendered_props.join(" "))
    };
    let props_multiline = rendered_props.len() > 4
        || rendered_props.iter().any(|p| p.contains('\n'))
        || (tag.name.len() + inline_props_str.len() + depth * 4 > 92);

    let ind_self = indent_str(depth);
    let ind_child = indent_str(depth + 1);

    if children.is_empty() {
        if !props_multiline {
            return format!("<{}{inline_props_str} />", tag.name);
        }
        let mut s = format!("<{}\n", tag.name);
        for p in &rendered_props {
            let _ = writeln!(s, "{ind_child}{p}");
        }
        let _ = write!(s, "{ind_self}/>");
        return s;
    }

    // Check if children are strictly inline text/simple-expression content (no Element, no multiline expr)
    let has_element = children.iter().any(|c| matches!(c, JsxChild::Element(_)));
    let inline_children_opt = if !has_element {
        let mut inline_buf = String::new();
        let mut ok = true;
        for ch in children {
            match ch {
                JsxChild::Text(t, _) => {
                    if t.contains('\n') {
                        ok = false;
                        break;
                    }
                    inline_buf.push_str(t);
                }
                JsxChild::Expr(e) => {
                    let rendered = format_expr(e, depth + 1, 0);
                    if rendered.contains('\n')
                        || matches!(
                            e.kind,
                            ExprKind::MethodCall { .. }
                                | ExprKind::Jsx { .. }
                                | ExprKind::Binary {
                                    op: BinaryOp::And,
                                    ..
                                }
                        )
                    {
                        ok = false;
                        break;
                    }
                    inline_buf.push('{');
                    inline_buf.push_str(&rendered);
                    inline_buf.push('}');
                }
                JsxChild::Element(_) => {
                    ok = false;
                    break;
                }
            }
        }
        if ok {
            Some(inline_buf)
        } else {
            None
        }
    } else {
        None
    };

    if let Some(ref inline_content) = inline_children_opt {
        if !props_multiline
            && (tag.name.len() * 2
                + inline_props_str.len()
                + inline_content.len()
                + depth * 4
                + 5
                <= 96)
        {
            return format!(
                "<{}{inline_props_str}>{inline_content}</{}>",
                tag.name, tag.name
            );
        }
        let mut s = String::new();
        if !props_multiline {
            let _ = writeln!(s, "<{}{inline_props_str}>", tag.name);
        } else {
            let _ = writeln!(s, "<{}", tag.name);
            for p in &rendered_props {
                let _ = writeln!(s, "{ind_child}{p}");
            }
            let _ = writeln!(s, "{ind_self}>");
        }
        let _ = writeln!(s, "{ind_child}{inline_content}");
        let _ = write!(s, "{ind_self}</{}>", tag.name);
        return s;
    }

    // Multi-line element with block children
    let mut s = String::new();
    if !props_multiline {
        let _ = writeln!(s, "<{}{inline_props_str}>", tag.name);
    } else {
        let _ = writeln!(s, "<{}", tag.name);
        for p in &rendered_props {
            let _ = writeln!(s, "{ind_child}{p}");
        }
        let _ = writeln!(s, "{ind_self}>");
    }

    for ch in children {
        match ch {
            JsxChild::Element(el) => {
                let _ = writeln!(s, "{ind_child}{}", format_expr(el, depth + 1, 0));
            }
            JsxChild::Expr(e) => {
                let _ = writeln!(s, "{ind_child}{{{}}}", format_expr(e, depth + 1, 0));
            }
            JsxChild::Text(t, _) => {
                let trimmed = t.trim();
                if !trimmed.is_empty() {
                    let _ = writeln!(s, "{ind_child}{trimmed}");
                }
            }
        }
    }

    let _ = write!(s, "{ind_self}</{}>", tag.name);
    s
}

fn format_float(v: f64) -> String {
    let s = format!("{v}");
    if !s.contains('.') && !s.contains('e') && !s.contains('E') {
        format!("{s}.0")
    } else {
        s
    }
}

fn escape_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\0' => out.push_str("\\0"),
            c => out.push(c),
        }
    }
    out
}

fn escape_template_lit(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '\\' => out.push_str("\\\\"),
            '`' => out.push_str("\\`"),
            '$' if i + 1 < chars.len() && chars[i + 1] == '{' => {
                out.push_str("\\$");
            }
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
        i += 1;
    }
    out
}
