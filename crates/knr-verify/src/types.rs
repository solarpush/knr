use crate::diagnostic::{format_instruction, format_terminator, Diagnostic};
use knr_core::{BasicBlock, Function, Op, RegId, Schema, Terminator, Type};
use std::collections::HashMap;

/// Validates static typing for all SSA opcodes and terminators:
/// 1. Type agreement for all operations (arithmetic, comparison, containers, buffers, channels, structs).
/// 2. br_if condition must be bool.
/// 3. Return value type must match declared function return type.
pub fn verify_types(func: &Function) -> Result<(), Diagnostic> {
    verify_types_with_schemas(func, &[])
}

pub fn verify_types_with_schemas(func: &Function, schemas: &[Schema]) -> Result<(), Diagnostic> {
    let mut reg_types: HashMap<RegId, Type> = HashMap::new();

    // Register function parameter types
    for (reg, ty) in &func.params {
        if let Err(mut diag) = check_type_schemas(ty, func, 1, &format!("%{reg}: {ty}"), schemas) {
            diag = diag.with_violated_register(format!("%{reg}"));
            return Err(diag);
        }
        reg_types.insert(*reg, ty.clone());
    }
    check_type_schemas(&func.ret_ty, func, 1, &format!("fn {} -> {}", func.name, func.ret_ty), schemas)?;

    for block in &func.blocks {
        for inst in &block.instructions {
            let inst_str = format_instruction(inst);

            // Check opcode typing
            check_op_types(func, inst, &inst_str, &reg_types, schemas)?;

            // Verify that instruction type references only declared schemas
            if let Err(mut diag) = check_type_schemas(&inst.ty, func, inst.line, &inst_str, schemas) {
                if let Some(d) = inst.dst {
                    diag = diag.with_violated_register(format!("%{d}"));
                }
                return Err(diag);
            }

            // Record destination register type
            if let Some(dst) = inst.dst {
                let actual_ty = match &inst.op {
                    Op::Syscall(knr_core::Effect::Hw, op_name, _)
                        if op_name.contains("simd_xor") || op_name.contains("mmio_map") =>
                    {
                        Type::Buf
                    }
                    Op::Syscall(knr_core::Effect::Net, op_name, _)
                        if op_name.contains("respond") || op_name.contains("listen") =>
                    {
                        if func.ret_ty == Type::Bool {
                            Type::Bool
                        } else {
                            inst.ty.clone()
                        }
                    }
                    _ => inst.ty.clone(),
                };
                reg_types.insert(dst, actual_ty);
            }
        }

        // Check terminator typing
        let term_str = format_terminator(&block.terminator);
        let term_line = get_terminator_line(block);

        match &block.terminator {
            Terminator::Ret(Some(reg)) => {
                let ty = get_reg_ty(func, *reg, term_line, &term_str, &reg_types)?;
                if *ty != func.ret_ty {
                    return Err(Diagnostic::new("TypeMismatch", &func.name, term_line, term_str)
                        .with_violated_register(format!("%{reg}"))
                        .with_fix_hint(format!(
                            "Function declared return type {} but returns register %{reg} of type {ty}",
                            func.ret_ty
                        )));
                }
            }
            Terminator::Ret(None) => {
                if func.ret_ty != Type::Void {
                    return Err(Diagnostic::new("TypeMismatch", &func.name, term_line, term_str)
                        .with_fix_hint(format!(
                            "Function declared return type {} but returns void",
                            func.ret_ty
                        )));
                }
            }
            Terminator::BrIf { cond, .. } => {
                let ty = get_reg_ty(func, *cond, term_line, &term_str, &reg_types)?;
                if *ty != Type::Bool {
                    return Err(Diagnostic::new("TypeMismatch", &func.name, term_line, term_str)
                        .with_violated_register(format!("%{cond}"))
                        .with_fix_hint(format!(
                            "Branch condition register %{cond} must be bool, found {ty}"
                        )));
                }
            }
            Terminator::Br(_) => {}
        }
    }

    Ok(())
}

fn get_reg_ty<'a>(
    func: &Function,
    reg: RegId,
    line: usize,
    inst_str: &str,
    reg_types: &'a HashMap<RegId, Type>,
) -> Result<&'a Type, Diagnostic> {
    reg_types.get(&reg).ok_or_else(|| {
        Diagnostic::new("TypeMismatch", &func.name, line, inst_str)
            .with_violated_register(format!("%{reg}"))
            .with_fix_hint(format!("Register %{reg} has unknown or unrecorded type"))
    })
}

fn check_op_types(
    func: &Function,
    inst: &knr_core::Instruction,
    inst_str: &str,
    reg_types: &HashMap<RegId, Type>,
    schemas: &[Schema],
) -> Result<(), Diagnostic> {
    let line = inst.line;

    match &inst.op {
        Op::ConstI64(_) => {
            if inst.ty != Type::I64 {
                return Err(type_err(func, line, inst_str, "const_i64 destination must be i64", None));
            }
        }
        Op::ConstF64(_) => {
            if inst.ty != Type::F64 {
                return Err(type_err(func, line, inst_str, "const_f64 destination must be f64", None));
            }
        }
        Op::ConstBool(_) => {
            if inst.ty != Type::Bool {
                return Err(type_err(func, line, inst_str, "const_bool destination must be bool", None));
            }
        }
        Op::ConstStr(_) => {
            if inst.ty != Type::Str {
                return Err(type_err(func, line, inst_str, "const_str destination must be str", None));
            }
        }
        Op::Add(a, b) | Op::Sub(a, b) | Op::Mul(a, b) | Op::Div(a, b) => {
            let ty_a = get_reg_ty(func, *a, line, inst_str, reg_types)?;
            let ty_b = get_reg_ty(func, *b, line, inst_str, reg_types)?;
            if ty_a != ty_b {
                return Err(Diagnostic::new("TypeMismatch", &func.name, line, inst_str)
                    .with_violated_register(format!("%{b}"))
                    .with_fix_hint(format!(
                        "Operand types must match: %{a} is {ty_a}, but %{b} is {ty_b}"
                    )));
            }
            if *ty_a != Type::I64 && *ty_a != Type::F64 {
                return Err(type_err(
                    func,
                    line,
                    inst_str,
                    &format!("Arithmetic operation not supported for type {ty_a}"),
                    Some(*a),
                ));
            }
            if inst.ty != *ty_a {
                return Err(type_err(
                    func,
                    line,
                    inst_str,
                    &format!("Instruction type {} does not match operand type {ty_a}", inst.ty),
                    None,
                ));
            }
        }
        Op::CmpEq(a, b) => {
            let ty_a = get_reg_ty(func, *a, line, inst_str, reg_types)?;
            let ty_b = get_reg_ty(func, *b, line, inst_str, reg_types)?;
            if ty_a != ty_b {
                return Err(Diagnostic::new("TypeMismatch", &func.name, line, inst_str)
                    .with_violated_register(format!("%{b}"))
                    .with_fix_hint(format!(
                        "Comparison operands must have same type: %{a} is {ty_a}, %{b} is {ty_b}"
                    )));
            }
            if inst.ty != Type::Bool {
                return Err(type_err(func, line, inst_str, "cmp_eq must return bool", None));
            }
        }
        Op::CmpLt(a, b) => {
            let ty_a = get_reg_ty(func, *a, line, inst_str, reg_types)?;
            let ty_b = get_reg_ty(func, *b, line, inst_str, reg_types)?;
            if ty_a != ty_b {
                return Err(Diagnostic::new("TypeMismatch", &func.name, line, inst_str)
                    .with_violated_register(format!("%{b}"))
                    .with_fix_hint(format!(
                        "Comparison operands must have same type: %{a} is {ty_a}, %{b} is {ty_b}"
                    )));
            }
            if *ty_a != Type::I64 && *ty_a != Type::F64 {
                return Err(type_err(
                    func,
                    line,
                    inst_str,
                    &format!("cmp_lt requires i64 or f64, found {ty_a}"),
                    Some(*a),
                ));
            }
            if inst.ty != Type::Bool {
                return Err(type_err(func, line, inst_str, "cmp_lt must return bool", None));
            }
        }
        Op::BufAlloc(s) => {
            let ty_s = get_reg_ty(func, *s, line, inst_str, reg_types)?;
            if *ty_s != Type::I64 {
                return Err(type_err(
                    func,
                    line,
                    inst_str,
                    &format!("buf_alloc size must be i64, found {ty_s}"),
                    Some(*s),
                ));
            }
            if inst.ty != Type::Buf {
                return Err(type_err(func, line, inst_str, "buf_alloc must return buf", None));
            }
        }
        Op::BufReadU8(b, i) => {
            let ty_b = get_reg_ty(func, *b, line, inst_str, reg_types)?;
            let ty_i = get_reg_ty(func, *i, line, inst_str, reg_types)?;
            if *ty_b != Type::Buf {
                return Err(type_err(
                    func,
                    line,
                    inst_str,
                    &format!("buf_read_u8 target must be buf, found {ty_b}"),
                    Some(*b),
                ));
            }
            if *ty_i != Type::I64 {
                return Err(type_err(
                    func,
                    line,
                    inst_str,
                    &format!("buf_read_u8 index must be i64, found {ty_i}"),
                    Some(*i),
                ));
            }
            if inst.ty != Type::I64 {
                return Err(type_err(func, line, inst_str, "buf_read_u8 must return i64", None));
            }
        }
        Op::BufWriteU8(b, i, v) => {
            let ty_b = get_reg_ty(func, *b, line, inst_str, reg_types)?;
            let ty_i = get_reg_ty(func, *i, line, inst_str, reg_types)?;
            let ty_v = get_reg_ty(func, *v, line, inst_str, reg_types)?;
            if *ty_b != Type::Buf {
                return Err(type_err(
                    func,
                    line,
                    inst_str,
                    &format!("buf_write_u8 target must be buf, found {ty_b}"),
                    Some(*b),
                ));
            }
            if *ty_i != Type::I64 {
                return Err(type_err(
                    func,
                    line,
                    inst_str,
                    &format!("buf_write_u8 index must be i64, found {ty_i}"),
                    Some(*i),
                ));
            }
            if *ty_v != Type::I64 {
                return Err(type_err(
                    func,
                    line,
                    inst_str,
                    &format!("buf_write_u8 value must be i64, found {ty_v}"),
                    Some(*v),
                ));
            }
            if inst.ty != Type::Buf {
                return Err(type_err(func, line, inst_str, "buf_write_u8 must return buf", None));
            }
        }
        Op::BufSlice(b, s, l) => {
            let ty_b = get_reg_ty(func, *b, line, inst_str, reg_types)?;
            let ty_s = get_reg_ty(func, *s, line, inst_str, reg_types)?;
            let ty_l = get_reg_ty(func, *l, line, inst_str, reg_types)?;
            if *ty_b != Type::Buf {
                return Err(type_err(
                    func,
                    line,
                    inst_str,
                    &format!("buf_slice target must be buf, found {ty_b}"),
                    Some(*b),
                ));
            }
            if *ty_s != Type::I64 {
                return Err(type_err(
                    func,
                    line,
                    inst_str,
                    &format!("buf_slice start index must be i64, found {ty_s}"),
                    Some(*s),
                ));
            }
            if *ty_l != Type::I64 {
                return Err(type_err(
                    func,
                    line,
                    inst_str,
                    &format!("buf_slice length must be i64, found {ty_l}"),
                    Some(*l),
                ));
            }
            if inst.ty != Type::Buf {
                return Err(type_err(func, line, inst_str, "buf_slice must return buf", None));
            }
        }
        Op::VecNew(inner) => {
            let expected = Type::Vec(Box::new(inner.clone()));
            if inst.ty != expected {
                return Err(type_err(
                    func,
                    line,
                    inst_str,
                    &format!("vec_new declared for {expected}, but instruction type is {}", inst.ty),
                    None,
                ));
            }
        }
        Op::VecPush(v, item) => {
            let ty_v = get_reg_ty(func, *v, line, inst_str, reg_types)?;
            let ty_item = get_reg_ty(func, *item, line, inst_str, reg_types)?;
            match ty_v {
                Type::Vec(elem_ty) => {
                    if **elem_ty != *ty_item {
                        return Err(Diagnostic::new("TypeMismatch", &func.name, line, inst_str)
                            .with_violated_register(format!("%{item}"))
                            .with_fix_hint(format!(
                                "vec_push element type mismatch: expected {}, got {ty_item}",
                                **elem_ty
                            )));
                    }
                    if inst.ty != *ty_v {
                        return Err(type_err(
                            func,
                            line,
                            inst_str,
                            &format!("vec_push destination type {} does not match {ty_v}", inst.ty),
                            None,
                        ));
                    }
                }
                _ => {
                    return Err(type_err(
                        func,
                        line,
                        inst_str,
                        &format!("vec_push target must be vec, found {ty_v}"),
                        Some(*v),
                    ));
                }
            }
        }
        Op::VecGet(v, i) => {
            let ty_v = get_reg_ty(func, *v, line, inst_str, reg_types)?;
            let ty_i = get_reg_ty(func, *i, line, inst_str, reg_types)?;
            if *ty_i != Type::I64 {
                return Err(type_err(
                    func,
                    line,
                    inst_str,
                    &format!("vec_get index must be i64, found {ty_i}"),
                    Some(*i),
                ));
            }
            match ty_v {
                Type::Vec(elem_ty) => {
                    if inst.ty != **elem_ty {
                        return Err(type_err(
                            func,
                            line,
                            inst_str,
                            &format!("vec_get return type {} does not match element type {}", inst.ty, **elem_ty),
                            None,
                        ));
                    }
                }
                _ => {
                    return Err(type_err(
                        func,
                        line,
                        inst_str,
                        &format!("vec_get target must be vec, found {ty_v}"),
                        Some(*v),
                    ));
                }
            }
        }
        Op::VecLen(v) => {
            let ty_v = get_reg_ty(func, *v, line, inst_str, reg_types)?;
            if !matches!(ty_v, Type::Vec(_)) {
                return Err(type_err(
                    func,
                    line,
                    inst_str,
                    &format!("vec_len target must be vec, found {ty_v}"),
                    Some(*v),
                ));
            }
            if inst.ty != Type::I64 {
                return Err(type_err(func, line, inst_str, "vec_len must return i64", None));
            }
        }
        Op::MapNew(k, v) => {
            let expected = Type::Map(Box::new(k.clone()), Box::new(v.clone()));
            if inst.ty != expected {
                return Err(type_err(
                    func,
                    line,
                    inst_str,
                    &format!("map_new declared for {expected}, but instruction type is {}", inst.ty),
                    None,
                ));
            }
        }
        Op::MapSet(m, k, v) => {
            let ty_m = get_reg_ty(func, *m, line, inst_str, reg_types)?;
            let ty_k = get_reg_ty(func, *k, line, inst_str, reg_types)?;
            let ty_v = get_reg_ty(func, *v, line, inst_str, reg_types)?;
            match ty_m {
                Type::Map(expected_k, expected_v) => {
                    if **expected_k != *ty_k {
                        return Err(Diagnostic::new("TypeMismatch", &func.name, line, inst_str)
                            .with_violated_register(format!("%{k}"))
                            .with_fix_hint(format!(
                                "Map key type mismatch: expected {}, got {ty_k}",
                                **expected_k
                            )));
                    }
                    if **expected_v != *ty_v {
                        return Err(Diagnostic::new("TypeMismatch", &func.name, line, inst_str)
                            .with_violated_register(format!("%{v}"))
                            .with_fix_hint(format!(
                                "Map value type mismatch: expected {}, got {ty_v}",
                                **expected_v
                            )));
                    }
                    if inst.ty != *ty_m {
                        return Err(type_err(
                            func,
                            line,
                            inst_str,
                            &format!("map_set destination type {} does not match {ty_m}", inst.ty),
                            None,
                        ));
                    }
                }
                _ => {
                    return Err(type_err(
                        func,
                        line,
                        inst_str,
                        &format!("map_set target must be map, found {ty_m}"),
                        Some(*m),
                    ));
                }
            }
        }
        Op::MapGet(m, k) => {
            let ty_m = get_reg_ty(func, *m, line, inst_str, reg_types)?;
            let ty_k = get_reg_ty(func, *k, line, inst_str, reg_types)?;
            match ty_m {
                Type::Map(expected_k, expected_v) => {
                    if **expected_k != *ty_k {
                        return Err(Diagnostic::new("TypeMismatch", &func.name, line, inst_str)
                            .with_violated_register(format!("%{k}"))
                            .with_fix_hint(format!(
                                "Map key type mismatch in map_get: expected {}, got {ty_k}",
                                **expected_k
                            )));
                    }
                    if inst.ty != **expected_v {
                        return Err(type_err(
                            func,
                            line,
                            inst_str,
                            &format!("map_get destination type {} does not match map value type {}", inst.ty, **expected_v),
                            None,
                        ));
                    }
                }
                _ => {
                    return Err(type_err(
                        func,
                        line,
                        inst_str,
                        &format!("map_get target must be map, found {ty_m}"),
                        Some(*m),
                    ));
                }
            }
        }
        Op::MapDel(m, k) => {
            let ty_m = get_reg_ty(func, *m, line, inst_str, reg_types)?;
            let ty_k = get_reg_ty(func, *k, line, inst_str, reg_types)?;
            match ty_m {
                Type::Map(expected_k, _) => {
                    if **expected_k != *ty_k {
                        return Err(Diagnostic::new("TypeMismatch", &func.name, line, inst_str)
                            .with_violated_register(format!("%{k}"))
                            .with_fix_hint(format!(
                                "Map key type mismatch in map_del: expected {}, got {ty_k}",
                                **expected_k
                            )));
                    }
                    if inst.ty != *ty_m {
                        return Err(type_err(
                            func,
                            line,
                            inst_str,
                            &format!("map_del destination type {} does not match {ty_m}", inst.ty),
                            None,
                        ));
                    }
                }
                _ => {
                    return Err(type_err(
                        func,
                        line,
                        inst_str,
                        &format!("map_del target must be map, found {ty_m}"),
                        Some(*m),
                    ));
                }
            }
        }
        Op::ChanNew(inner, cap) => {
            let ty_c = get_reg_ty(func, *cap, line, inst_str, reg_types)?;
            if *ty_c != Type::I64 {
                return Err(type_err(
                    func,
                    line,
                    inst_str,
                    &format!("chan_new capacity must be i64, found {ty_c}"),
                    Some(*cap),
                ));
            }
            let expected = Type::Chan(Box::new(inner.clone()));
            if inst.ty != expected {
                return Err(type_err(
                    func,
                    line,
                    inst_str,
                    &format!("chan_new declared for {expected}, but instruction type is {}", inst.ty),
                    None,
                ));
            }
        }
        Op::ChanSend(c, val) => {
            let ty_c = get_reg_ty(func, *c, line, inst_str, reg_types)?;
            let ty_val = get_reg_ty(func, *val, line, inst_str, reg_types)?;
            match ty_c {
                Type::Chan(elem_ty) => {
                    if **elem_ty != *ty_val {
                        return Err(Diagnostic::new("TypeMismatch", &func.name, line, inst_str)
                            .with_violated_register(format!("%{val}"))
                            .with_fix_hint(format!(
                                "chan_send value type mismatch: channel holds {}, got {ty_val}",
                                **elem_ty
                            )));
                    }
                    if inst.ty != Type::Bool {
                        return Err(type_err(func, line, inst_str, "chan_send must return bool", None));
                    }
                }
                _ => {
                    return Err(type_err(
                        func,
                        line,
                        inst_str,
                        &format!("chan_send target must be chan, found {ty_c}"),
                        Some(*c),
                    ));
                }
            }
        }
        Op::ChanRecv(c) => {
            let ty_c = get_reg_ty(func, *c, line, inst_str, reg_types)?;
            match ty_c {
                Type::Chan(elem_ty) => {
                    if inst.ty != **elem_ty {
                        return Err(type_err(
                            func,
                            line,
                            inst_str,
                            &format!("chan_recv destination type {} does not match channel type {}", inst.ty, **elem_ty),
                            None,
                        ));
                    }
                }
                _ => {
                    return Err(type_err(
                        func,
                        line,
                        inst_str,
                        &format!("chan_recv target must be chan, found {ty_c}"),
                        Some(*c),
                    ));
                }
            }
        }
        Op::Spawn(_, arg) => {
            get_reg_ty(func, *arg, line, inst_str, reg_types)?;
            if inst.ty != Type::Bool {
                return Err(type_err(func, line, inst_str, "spawn must return bool", None));
            }
        }
        Op::Syscall(_, _, payload) => {
            get_reg_ty(func, *payload, line, inst_str, reg_types)?;
        }
        Op::Free(r) => {
            get_reg_ty(func, *r, line, inst_str, reg_types)?;
        }
        Op::StructNew(name) => {
            if inst.dst.is_none() {
                return Err(Diagnostic::new("LinearLeak", &func.name, line, inst_str)
                    .with_fix_hint(format!(
                        "struct_new allocates linear struct '{name}' and requires a destination register '%dst ='"
                    )));
            }
            let expected_ty = Type::Struct(name.clone());
            if inst.ty != expected_ty {
                let mut diag = Diagnostic::new("TypeMismatch", &func.name, line, inst_str)
                    .with_fix_hint(format!(
                        "struct_new destination type mismatch: expected {expected_ty}, got {}",
                        inst.ty
                    ));
                if let Some(d) = inst.dst {
                    diag = diag.with_violated_register(format!("%{d}"));
                }
                return Err(diag);
            }
            if !schemas.iter().any(|s| s.name == *name) {
                let mut diag = Diagnostic::new("UnknownSchema", &func.name, line, inst_str)
                    .with_fix_hint(format!("Schema '{name}' is not declared in module"));
                if let Some(d) = inst.dst {
                    diag = diag.with_violated_register(format!("%{d}"));
                }
                return Err(diag);
            }
        }
        Op::StructGet(s, field_idx) => {
            if *field_idx > u16::MAX as u32 {
                return Err(Diagnostic::new("FieldIndexOverflow", &func.name, line, inst_str)
                    .with_violated_register(format!("%{s}"))
                    .with_fix_hint(format!("Field index {field_idx} exceeds maximum allowed 65535")));
            }
            if inst.dst.is_none() {
                return Err(Diagnostic::new("TypeMismatch", &func.name, line, inst_str)
                    .with_violated_register(format!("%{s}"))
                    .with_fix_hint("struct_get produces a field value and requires a destination register '%dst ='".to_string()));
            }
            let ty_s = get_reg_ty(func, *s, line, inst_str, reg_types)?;
            match ty_s {
                Type::Struct(schema_name) => {
                    if let Some(schema) = schemas.iter().find(|sc| sc.name == *schema_name) {
                        if let Some(field) = schema.fields.iter().find(|f| f.index == *field_idx) {
                            if inst.ty != field.ty {
                                let mut diag = Diagnostic::new("TypeMismatch", &func.name, line, inst_str)
                                    .with_fix_hint(format!(
                                        "struct_get destination type {} does not match field {field_idx} type {}",
                                        inst.ty, field.ty
                                    ));
                                if let Some(d) = inst.dst {
                                    diag = diag.with_violated_register(format!("%{d}"));
                                } else {
                                    diag = diag.with_violated_register(format!("%{s}"));
                                }
                                return Err(diag);
                            }
                        } else {
                            return Err(Diagnostic::new("UndeclaredField", &func.name, line, inst_str)
                                .with_violated_register(format!("%{s}"))
                                .with_fix_hint(format!(
                                    "Field index {field_idx} is not declared in schema '{schema_name}'"
                                )));
                        }
                    } else {
                        return Err(Diagnostic::new("UnknownSchema", &func.name, line, inst_str)
                            .with_violated_register(format!("%{s}"))
                            .with_fix_hint(format!("Schema '{schema_name}' is not declared in module")));
                    }
                }
                _ => {
                    return Err(type_err(
                        func,
                        line,
                        inst_str,
                        &format!("struct_get target must be struct, found {ty_s}"),
                        Some(*s),
                    ));
                }
            }
        }
        Op::StructSet(s, field_idx, val) => {
            if *field_idx > u16::MAX as u32 {
                return Err(Diagnostic::new("FieldIndexOverflow", &func.name, line, inst_str)
                    .with_violated_register(format!("%{s}"))
                    .with_fix_hint(format!("Field index {field_idx} exceeds maximum allowed 65535")));
            }
            if inst.dst.is_none() {
                return Err(Diagnostic::new("LinearLeak", &func.name, line, inst_str)
                    .with_violated_register(format!("%{s}"))
                    .with_fix_hint("struct_set produces an updated linear struct and requires a destination register '%dst ='".to_string()));
            }
            let ty_s = get_reg_ty(func, *s, line, inst_str, reg_types)?;
            let ty_val = get_reg_ty(func, *val, line, inst_str, reg_types)?;
            match ty_s {
                Type::Struct(schema_name) => {
                    let schema = match schemas.iter().find(|sc| sc.name == *schema_name) {
                        Some(sc) => sc,
                        None => {
                            return Err(Diagnostic::new("UnknownSchema", &func.name, line, inst_str)
                                .with_violated_register(format!("%{s}"))
                                .with_fix_hint(format!("Schema '{schema_name}' is not declared in module")));
                        }
                    };
                    if inst.ty != *ty_s {
                        let mut diag = Diagnostic::new("TypeMismatch", &func.name, line, inst_str)
                            .with_fix_hint(format!(
                                "struct_set destination type mismatch: expected {ty_s}, got {}",
                                inst.ty
                            ));
                        if let Some(d) = inst.dst {
                            diag = diag.with_violated_register(format!("%{d}"));
                        } else {
                            diag = diag.with_violated_register(format!("%{s}"));
                        }
                        return Err(diag);
                    }
                    if let Some(field) = schema.fields.iter().find(|f| f.index == *field_idx) {
                        if *ty_val != field.ty {
                            return Err(Diagnostic::new("TypeMismatch", &func.name, line, inst_str)
                                .with_violated_register(format!("%{val}"))
                                .with_fix_hint(format!(
                                    "struct_set field {field_idx} expected type {}, got {ty_val}",
                                    field.ty
                                )));
                        }
                    } else {
                        return Err(Diagnostic::new("UndeclaredField", &func.name, line, inst_str)
                            .with_violated_register(format!("%{s}"))
                            .with_fix_hint(format!(
                                "Field index {field_idx} is not declared in schema '{schema_name}'"
                            )));
                    }
                }
                _ => {
                    return Err(type_err(
                        func,
                        line,
                        inst_str,
                        &format!("struct_set target must be struct, found {ty_s}"),
                        Some(*s),
                    ));
                }
            }
        }
    }

    Ok(())
}

fn check_type_schemas(
    ty: &Type,
    func: &Function,
    line: usize,
    inst_str: &str,
    schemas: &[Schema],
) -> Result<(), Diagnostic> {
    match ty {
        Type::Struct(name) => {
            if !schemas.iter().any(|s| s.name == *name) {
                return Err(Diagnostic::new("UnknownSchema", &func.name, line, inst_str)
                    .with_fix_hint(format!("Schema '{name}' is not declared in module")));
            }
        }
        Type::Vec(inner) | Type::Chan(inner) => {
            check_type_schemas(inner, func, line, inst_str, schemas)?;
        }
        Type::Map(k, v) => {
            check_type_schemas(k, func, line, inst_str, schemas)?;
            check_type_schemas(v, func, line, inst_str, schemas)?;
        }
        _ => {}
    }
    Ok(())
}

fn type_err(
    func: &Function,
    line: usize,
    inst_str: &str,
    hint: &str,
    violated: Option<RegId>,
) -> Diagnostic {
    let mut diag = Diagnostic::new("TypeMismatch", &func.name, line, inst_str)
        .with_fix_hint(hint);
    if let Some(r) = violated {
        diag = diag.with_violated_register(format!("%{r}"));
    }
    diag
}

fn get_terminator_line(block: &BasicBlock) -> usize {
    if let Some(last_inst) = block.instructions.last() {
        last_inst.line + 1
    } else {
        1
    }
}
