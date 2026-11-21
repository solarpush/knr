use std::collections::HashMap;

use crate::ast::{Function, Op, RegId, Terminator};
use crate::lower::context::SourceMap;

/// Renumbers all registers in a function to be strictly ascending (0, 1, 2, ..., N)
/// without gaps or regressions, and updates the source map.
pub fn normalize_function_registers(func: &mut Function, source_map: &mut SourceMap) {
    let mut reg_map: HashMap<RegId, RegId> = HashMap::new();
    let mut counter: RegId = 0;

    // 1. Remap parameters sequentially: 0, 1, ..., P-1
    for (param_reg, _) in &mut func.params {
        let old = *param_reg;
        let new_id = counter;
        counter += 1;
        reg_map.insert(old, new_id);
        *param_reg = new_id;
    }

    // 2. Discover and assign dense monotonic IDs in block/instruction order
    for block in &func.blocks {
        for (preg, _) in &block.params {
            reg_map.entry(*preg).or_insert_with(|| {
                let new_id = counter;
                counter += 1;
                new_id
            });
        }
        for inst in &block.instructions {
            if let Some(dst) = inst.dst {
                reg_map.entry(dst).or_insert_with(|| {
                    let new_id = counter;
                    counter += 1;
                    new_id
                });
            }
        }
    }

    // 3. Rewrite all block params, instruction destinations and opcode operands
    for block in &mut func.blocks {
        for (preg, _) in &mut block.params {
            if let Some(&new_id) = reg_map.get(preg) {
                *preg = new_id;
            }
        }
        for inst in &mut block.instructions {
            if let Some(ref mut dst) = inst.dst {
                if let Some(&new_id) = reg_map.get(dst) {
                    *dst = new_id;
                }
            }
            remap_op_registers(&mut inst.op, &reg_map);
        }

        // 4. Rewrite block terminator operands
        match &mut block.terminator {
            Terminator::Ret(Some(ref mut r)) => {
                if let Some(&new_id) = reg_map.get(r) {
                    *r = new_id;
                }
            }
            Terminator::BrIf { ref mut cond, .. } => {
                if let Some(&new_id) = reg_map.get(cond) {
                    *cond = new_id;
                }
            }
            Terminator::BrArgs { ref mut args, .. } => {
                for a in args {
                    remap(a, &reg_map);
                }
            }
            Terminator::BrIfArgs {
                ref mut cond,
                ref mut then_args,
                ref mut else_args,
                ..
            } => {
                remap(cond, &reg_map);
                for a in then_args {
                    remap(a, &reg_map);
                }
                for a in else_args {
                    remap(a, &reg_map);
                }
            }
            Terminator::Ret(None) | Terminator::Br(_) => {}
        }
    }

    // 5. Update SourceMap with new register IDs for this function
    let fn_name = &func.name;
    let mut new_reg_to_var = HashMap::new();
    let mut new_reg_to_span = HashMap::new();

    for (&old_reg, &new_reg) in &reg_map {
        if let Some(var_name) = source_map.reg_to_var.remove(&(fn_name.clone(), old_reg)) {
            new_reg_to_var.insert((fn_name.clone(), new_reg), var_name);
        }
        if let Some(span) = source_map.reg_to_span.remove(&(fn_name.clone(), old_reg)) {
            new_reg_to_span.insert((fn_name.clone(), new_reg), span);
        }
    }

    source_map.reg_to_var.extend(new_reg_to_var);
    source_map.reg_to_span.extend(new_reg_to_span);
}

fn remap(r: &mut RegId, map: &HashMap<RegId, RegId>) {
    if let Some(&new_id) = map.get(r) {
        *r = new_id;
    }
}

pub fn remap_op_registers(op: &mut Op, map: &HashMap<RegId, RegId>) {
    match op {
        Op::ConstI64(_) | Op::ConstF64(_) | Op::ConstBool(_) | Op::ConstStr(_) => {}
        Op::Add(a, b)
        | Op::Sub(a, b)
        | Op::Mul(a, b)
        | Op::Div(a, b)
        | Op::Rem(a, b)
        | Op::CmpEq(a, b)
        | Op::CmpLt(a, b) => {
            remap(a, map);
            remap(b, map);
        }
        Op::IntToStr(a) | Op::StrLen(a) => remap(a, map),
        Op::BufAlloc(s) => remap(s, map),
        Op::BufReadU8(b, i) => {
            remap(b, map);
            remap(i, map);
        }
        Op::BufWriteU8(b, i, v) => {
            remap(b, map);
            remap(i, map);
            remap(v, map);
        }
        Op::BufSlice(b, s, l) => {
            remap(b, map);
            remap(s, map);
            remap(l, map);
        }
        Op::VecNew(_) => {}
        Op::VecPush(v, item) => {
            remap(v, map);
            remap(item, map);
        }
        Op::VecGet(v, i) => {
            remap(v, map);
            remap(i, map);
        }
        Op::VecLen(v) => remap(v, map),
        Op::MapNew(_, _) => {}
        Op::MapSet(m, k, v) => {
            remap(m, map);
            remap(k, map);
            remap(v, map);
        }
        Op::MapGet(m, k) => {
            remap(m, map);
            remap(k, map);
        }
        Op::MapDel(m, k) => {
            remap(m, map);
            remap(k, map);
        }
        Op::ChanNew(_, cap) => remap(cap, map),
        Op::ChanSend(c, val) => {
            remap(c, map);
            remap(val, map);
        }
        Op::ChanRecv(c) => remap(c, map),
        Op::Spawn(_, arg) => remap(arg, map),
        Op::Syscall(_, _, payload) => remap(payload, map),
        Op::Free(r) => remap(r, map),
        Op::StructNew(_) => {}
        Op::StructGet(s, _) => remap(s, map),
        Op::StructSet(s, _, val) => {
            remap(s, map);
            remap(val, map);
        }
        Op::Call(_, args) => {
            for arg in args {
                remap(arg, map);
            }
        }
    }
}
