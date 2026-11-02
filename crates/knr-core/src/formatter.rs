use crate::ast::*;

/// Formats a Module into canonical .ssa text.
pub fn format_ssa(module: &Module) -> String {
    let mut out = String::new();
    for (i, schema) in module.schemas.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        format_schema_into(schema, &mut out);
    }
    if !module.schemas.is_empty() && !module.functions.is_empty() {
        out.push('\n');
    }
    for (i, func) in module.functions.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        format_function_into(func, &mut out);
    }
    out
}

/// Formats a single Schema into canonical .ssa text.
pub fn format_schema(schema: &Schema) -> String {
    let mut out = String::new();
    format_schema_into(schema, &mut out);
    out
}

fn format_schema_into(schema: &Schema, out: &mut String) {
    out.push_str(&format!("schema {} {{\n", schema.name));
    for field in &schema.fields {
        match &field.name {
            Some(name) => {
                out.push_str(&format!("    {}: {} {},\n", field.index, name, field.ty));
            }
            None => {
                out.push_str(&format!("    {}: {},\n", field.index, field.ty));
            }
        }
    }
    out.push_str("}\n");
}

/// Formats a single Function into canonical .ssa text.
pub fn format_function(func: &Function) -> String {
    let mut out = String::new();
    format_function_into(func, &mut out);
    out
}

fn format_function_into(func: &Function, out: &mut String) {
    let params_str = func
        .params
        .iter()
        .map(|(reg, ty)| format!("%{reg}: {ty}"))
        .collect::<Vec<_>>()
        .join(", ");

    let effects_str = if func.effects.is_empty() {
        String::new()
    } else {
        let mut sorted = func.effects.clone();
        sorted.sort_by_key(|e| match e {
            Effect::Net => 0,
            Effect::Ui => 1,
            Effect::Hw => 2,
            Effect::Async => 3,
        });
        format!(
            " effects({})",
            sorted
                .iter()
                .map(|e| e.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        )
    };

    out.push_str(&format!(
        "fn {}({}) -> {}{} {{\n",
        func.name, params_str, func.ret_ty, effects_str
    ));

    for block in &func.blocks {
        out.push_str(&format!("bb{}:\n", block.id));
        for inst in &block.instructions {
            out.push_str(&format!("    {}\n", format_instruction(inst)));
        }
        out.push_str(&format!("    {}\n", format_terminator(&block.terminator)));
    }

    out.push_str("}\n");
}

fn format_instruction(inst: &Instruction) -> String {
    let op_str = format_op(&inst.op);
    match inst.dst {
        Some(dst) => format!("%{dst} = {op_str}"),
        None => op_str,
    }
}

fn format_op(op: &Op) -> String {
    match op {
        Op::ConstI64(v) => format!("const_i64 {v}"),
        Op::ConstF64(v) => {
            let s = v.to_string();
            if !s.contains('.') && !s.contains('e') && !s.contains('E') && v.is_finite() {
                format!("const_f64 {s}.0")
            } else {
                format!("const_f64 {s}")
            }
        }
        Op::ConstBool(v) => format!("const_bool {v}"),
        Op::ConstStr(s) => format!("const_str \"{}\"", escape_string(s)),
        Op::Add(a, b) => format!("add %{a} %{b}"),
        Op::Sub(a, b) => format!("sub %{a} %{b}"),
        Op::Mul(a, b) => format!("mul %{a} %{b}"),
        Op::Div(a, b) => format!("div %{a} %{b}"),
        Op::CmpEq(a, b) => format!("cmp_eq %{a} %{b}"),
        Op::CmpLt(a, b) => format!("cmp_lt %{a} %{b}"),
        Op::BufAlloc(s) => format!("buf_alloc %{s}"),
        Op::BufReadU8(b, i) => format!("buf_read_u8 %{b} %{i}"),
        Op::BufWriteU8(b, i, v) => format!("buf_write_u8 %{b} %{i} %{v}"),
        Op::BufSlice(b, s, l) => format!("buf_slice %{b} %{s} %{l}"),
        Op::VecNew(t) => format!("vec_new {t}"),
        Op::VecPush(v, item) => format!("vec_push %{v} %{item}"),
        Op::VecGet(v, i) => format!("vec_get %{v} %{i}"),
        Op::VecLen(v) => format!("vec_len %{v}"),
        Op::MapNew(k, v) => format!("map_new {k} {v}"),
        Op::MapSet(m, k, v) => format!("map_set %{m} %{k} %{v}"),
        Op::MapGet(m, k) => format!("map_get %{m} %{k}"),
        Op::MapDel(m, k) => format!("map_del %{m} %{k}"),
        Op::ChanNew(t, cap) => format!("chan_new {t} %{cap}"),
        Op::ChanSend(c, v) => format!("chan_send %{c} %{v}"),
        Op::ChanRecv(c) => format!("chan_recv %{c}"),
        Op::Spawn(name, arg) => format!("spawn \"{}\" %{arg}", escape_string(name)),
        Op::Syscall(eff, name, payload) => {
            format!("syscall {eff} \"{}\" %{payload}", escape_string(name))
        }
        Op::Free(r) => format!("free %{r}"),
        Op::StructNew(name) => format!("struct_new {name}"),
        Op::StructGet(s, idx) => format!("struct_get %{s} {idx}"),
        Op::StructSet(s, idx, v) => format!("struct_set %{s} {idx} %{v}"),
    }
}

fn format_terminator(term: &Terminator) -> String {
    match term {
        Terminator::Ret(Some(r)) => format!("ret %{r}"),
        Terminator::Ret(None) => "ret".to_string(),
        Terminator::Br(target) => format!("br bb{target}"),
        Terminator::BrIf {
            cond,
            then_blk,
            else_blk,
        } => {
            format!("br_if %{cond} bb{then_blk} bb{else_blk}")
        }
    }
}

fn escape_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            '\0' => out.push_str("\\0"),
            other => out.push(other),
        }
    }
    out
}
