use crate::diagnostic::{format_instruction, format_terminator, Diagnostic};
use knr_core::{BasicBlock, BlockId, Function, Op, RegId, Terminator};
use std::collections::{BTreeSet, HashMap, HashSet};

/// Validates SSA structural and dominance rules:
/// 1. Entry block `bb0` exists.
/// 2. Single-assignment property: every register %id is defined at most once per function.
/// 3. SSA monotonicity: register definitions are strictly increasing.
/// 4. Basic block terminators and branch targets are valid.
/// 5. SSA dominance: every register used in an instruction or terminator dominates the use site.
pub fn verify_ssa(func: &Function) -> Result<(), Diagnostic> {
    // 1. Entry block bb0 must exist
    let entry_exists = func.blocks.iter().any(|b| b.id == 0);
    if !entry_exists {
        return Err(Diagnostic::new(
            "MissingEntryBlock",
            &func.name,
            1,
            format!("fn {}...", func.name),
        )
        .with_fix_hint("Function must define entry block bb0"));
    }

    let block_ids: HashSet<BlockId> = func.blocks.iter().map(|b| b.id).collect();

    // 2. Branch targets validity
    for block in &func.blocks {
        match &block.terminator {
            Terminator::Br(target) => {
                if !block_ids.contains(target) {
                    return Err(Diagnostic::new(
                        "InvalidBranchTarget",
                        &func.name,
                        get_terminator_line(block),
                        format_terminator(&block.terminator),
                    )
                    .with_fix_hint(format!("Target block bb{target} does not exist in function")));
                }
            }
            Terminator::BrIf {
                then_blk,
                else_blk,
                ..
            } => {
                if !block_ids.contains(then_blk) {
                    return Err(Diagnostic::new(
                        "InvalidBranchTarget",
                        &func.name,
                        get_terminator_line(block),
                        format_terminator(&block.terminator),
                    )
                    .with_fix_hint(format!(
                        "Target block bb{then_blk} does not exist in function"
                    )));
                }
                if !block_ids.contains(else_blk) {
                    return Err(Diagnostic::new(
                        "InvalidBranchTarget",
                        &func.name,
                        get_terminator_line(block),
                        format_terminator(&block.terminator),
                    )
                    .with_fix_hint(format!(
                        "Target block bb{else_blk} does not exist in function"
                    )));
                }
            }
            Terminator::Ret(_) => {}
        }
    }

    // 3. Single-assignment and SSA monotonicity pass
    let mut defined_registers: HashMap<RegId, usize> = HashMap::new();
    let mut last_defined: Option<RegId> = None;

    // Check function parameters
    for (reg, _ty) in &func.params {
        if let Some(prev_line) = defined_registers.get(reg) {
            return Err(Diagnostic::new(
                "MultipleAssignment",
                &func.name,
                1,
                format!("fn {}(...%{}...)", func.name, reg),
            )
            .with_violated_register(format!("%{reg}"))
            .with_fix_hint(format!(
                "Parameter %{reg} is assigned multiple times (first at line {prev_line})"
            )));
        }
        if let Some(prev) = last_defined {
            if *reg <= prev {
                return Err(Diagnostic::new(
                    "NonMonotonicRegisters",
                    &func.name,
                    1,
                    format!("fn {}(...%{}...)", func.name, reg),
                )
                .with_violated_register(format!("%{reg}"))
                .with_fix_hint(format!(
                    "Parameter register %{reg} is not strictly greater than previous register %{prev}"
                )));
            }
        }
        defined_registers.insert(*reg, 1);
        last_defined = Some(*reg);
    }

    // Check instruction destinations
    for block in &func.blocks {
        for inst in &block.instructions {
            if let Some(dst) = inst.dst {
                if let Some(prev_line) = defined_registers.get(&dst) {
                    return Err(Diagnostic::new(
                        "MultipleAssignment",
                        &func.name,
                        inst.line,
                        format_instruction(inst),
                    )
                    .with_violated_register(format!("%{dst}"))
                    .with_fix_hint(format!(
                        "Register %{dst} is assigned multiple times (first at line {prev_line})"
                    )));
                }
                if let Some(prev) = last_defined {
                    if dst <= prev {
                        return Err(Diagnostic::new(
                            "NonMonotonicRegisters",
                            &func.name,
                            inst.line,
                            format_instruction(inst),
                        )
                        .with_violated_register(format!("%{dst}"))
                        .with_fix_hint(format!(
                            "Register %{dst} must be strictly greater than previously defined register %{prev}"
                        )));
                    }
                }
                defined_registers.insert(dst, inst.line);
                last_defined = Some(dst);
            }
        }
    }

    // 4. SSA Dominance Pass
    verify_dominance(func)?;

    Ok(())
}

fn verify_dominance(func: &Function) -> Result<(), Diagnostic> {
    let block_ids: HashSet<BlockId> = func.blocks.iter().map(|b| b.id).collect();

    // Map each register to the block where it is defined, or None if it's a function parameter
    // (parameters dominate all blocks)
    let param_regs: HashSet<RegId> = func.params.iter().map(|(r, _)| *r).collect();
    let mut reg_def_block: HashMap<RegId, BlockId> = HashMap::new();

    for block in &func.blocks {
        for inst in &block.instructions {
            if let Some(dst) = inst.dst {
                reg_def_block.insert(dst, block.id);
            }
        }
    }

    // Build CFG predecessors
    let mut preds: HashMap<BlockId, Vec<BlockId>> = HashMap::new();
    for &id in &block_ids {
        preds.entry(id).or_default();
    }
    for block in &func.blocks {
        match &block.terminator {
            Terminator::Ret(_) => {}
            Terminator::Br(target) => {
                if block_ids.contains(target) {
                    preds.entry(*target).or_default().push(block.id);
                }
            }
            Terminator::BrIf {
                then_blk,
                else_blk,
                ..
            } => {
                if block_ids.contains(then_blk) {
                    preds.entry(*then_blk).or_default().push(block.id);
                }
                if block_ids.contains(else_blk) {
                    preds.entry(*else_blk).or_default().push(block.id);
                }
            }
        }
    }

    // Compute Dominance for each block using standard iterative fixed point
    let all_blocks: BTreeSet<BlockId> = block_ids.iter().copied().collect();
    let mut dom: HashMap<BlockId, BTreeSet<BlockId>> = HashMap::new();

    // Entry block dominates only itself
    let mut entry_dom = BTreeSet::new();
    entry_dom.insert(0);
    dom.insert(0, entry_dom);

    // All other blocks initially dominated by all blocks
    for &id in &all_blocks {
        if id != 0 {
            dom.insert(id, all_blocks.clone());
        }
    }

    let mut changed = true;
    while changed {
        changed = false;
        for &id in &all_blocks {
            if id == 0 {
                continue;
            }
            let pred_list = match preds.get(&id) {
                Some(p) if !p.is_empty() => p,
                _ => continue,
            };

            let mut new_dom: Option<BTreeSet<BlockId>> = None;
            for &p in pred_list {
                if let Some(p_dom) = dom.get(&p) {
                    match new_dom {
                        None => new_dom = Some(p_dom.clone()),
                        Some(ref mut cur) => *cur = cur.intersection(p_dom).copied().collect(),
                    }
                }
            }

            let mut computed = new_dom.unwrap_or_default();
            computed.insert(id);

            if dom.get(&id) != Some(&computed) {
                dom.insert(id, computed);
                changed = true;
            }
        }
    }

    // Now check each instruction in each block
    for block in &func.blocks {
        // Track registers defined so far in THIS block
        let mut defs_in_this_block: HashSet<RegId> = HashSet::new();

        for inst in &block.instructions {
            let used_regs = get_used_registers(&inst.op);
            for reg in used_regs {
                // If reg is a param, it dominates everywhere
                if param_regs.contains(&reg) {
                    continue;
                }

                // If defined in this block, must be defined strictly before this instruction
                if defs_in_this_block.contains(&reg) {
                    continue;
                }

                // If defined in another block, that block must dominate current block
                if let Some(&def_blk) = reg_def_block.get(&reg) {
                    if def_blk != block.id {
                        let dominates = dom.get(&block.id).is_some_and(|d| d.contains(&def_blk));
                        if dominates {
                            continue;
                        }
                    }
                }

                // If not dominated or undefined: Dominance violation
                return Err(Diagnostic::new(
                    "DominanceViolation",
                    &func.name,
                    inst.line,
                    format_instruction(inst),
                )
                .with_violated_register(format!("%{reg}"))
                .with_fix_hint(format!(
                    "Register %{reg} is used before definition or definition does not dominate use site"
                )));
            }

            // Record dst as defined in this block after checking operands
            if let Some(dst) = inst.dst {
                defs_in_this_block.insert(dst);
            }
        }

        // Check terminator operands
        let term_used_regs = match &block.terminator {
            Terminator::Ret(Some(r)) => vec![*r],
            Terminator::BrIf { cond, .. } => vec![*cond],
            _ => Vec::new(),
        };

        for reg in term_used_regs {
            if param_regs.contains(&reg) {
                continue;
            }
            if defs_in_this_block.contains(&reg) {
                continue;
            }
            if let Some(&def_blk) = reg_def_block.get(&reg) {
                if def_blk != block.id {
                    let dominates = dom.get(&block.id).is_some_and(|d| d.contains(&def_blk));
                    if dominates {
                        continue;
                    }
                }
            }

            return Err(Diagnostic::new(
                "DominanceViolation",
                &func.name,
                get_terminator_line(block),
                format_terminator(&block.terminator),
            )
            .with_violated_register(format!("%{reg}"))
            .with_fix_hint(format!(
                "Register %{reg} in terminator is used before definition or definition does not dominate use site"
            )));
        }
    }

    Ok(())
}

fn get_used_registers(op: &Op) -> Vec<RegId> {
    match op {
        Op::ConstI64(_) | Op::ConstF64(_) | Op::ConstBool(_) | Op::ConstStr(_) => Vec::new(),
        Op::Add(a, b)
        | Op::Sub(a, b)
        | Op::Mul(a, b)
        | Op::Div(a, b)
        | Op::CmpEq(a, b)
        | Op::CmpLt(a, b) => vec![*a, *b],
        Op::BufAlloc(s) => vec![*s],
        Op::BufReadU8(b, i) => vec![*b, *i],
        Op::BufWriteU8(b, i, v) => vec![*b, *i, *v],
        Op::BufSlice(b, s, l) => vec![*b, *s, *l],
        Op::VecNew(_) => Vec::new(),
        Op::VecPush(v, item) => vec![*v, *item],
        Op::VecGet(v, i) => vec![*v, *i],
        Op::VecLen(v) => vec![*v],
        Op::MapNew(_, _) => Vec::new(),
        Op::MapSet(m, k, v) => vec![*m, *k, *v],
        Op::MapGet(m, k) => vec![*m, *k],
        Op::MapDel(m, k) => vec![*m, *k],
        Op::ChanNew(_, cap) => vec![*cap],
        Op::ChanSend(c, val) => vec![*c, *val],
        Op::ChanRecv(c) => vec![*c],
        Op::Spawn(_, arg) => vec![*arg],
        Op::Syscall(_, _, payload) => vec![*payload],
        Op::Free(r) => vec![*r],
        Op::StructNew(_) => Vec::new(),
        Op::StructGet(s, _) => vec![*s],
        Op::StructSet(s, _, val) => vec![*s, *val],
        Op::Call(_, args) => args.clone(),
    }
}

fn get_terminator_line(block: &BasicBlock) -> usize {
    if let Some(last_inst) = block.instructions.last() {
        last_inst.line + 1
    } else {
        1
    }
}
