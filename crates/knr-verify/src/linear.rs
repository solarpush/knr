use crate::diagnostic::{format_instruction, format_terminator, Diagnostic};
use knr_core::{BasicBlock, BlockId, Function, Instruction, Op, RegId, Terminator, Type};
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegState {
    Alive { ty: Type, line: usize, owned: bool, borrowed: bool },
    Moved { ty: Type, consumed_at_line: usize },
}

/// Validates linear ownership rules without external callee signatures.
pub fn verify_linear(func: &Function) -> Result<(), Diagnostic> {
    verify_linear_with_env(func, &[])
}

/// Validates linear ownership & second-class borrow rules:
/// 1. Use-after-move: No register in `Moved` state can be read or consumed.
/// 2. Borrow escape: Borrowed (`&T`) parameters can be read or re-borrowed, never moved/freed/returned.
/// 3. Linear leak: Owned linear resources (Buf, Vec, Map, Chan, Struct) must be consumed or returned at `ret`.
pub fn verify_linear_with_env(func: &Function, functions: &[Function]) -> Result<(), Diagnostic> {
    if func.blocks.is_empty() {
        return Ok(());
    }

    let block_map: HashMap<BlockId, &BasicBlock> =
        func.blocks.iter().map(|b| (b.id, b)).collect();

    // Identify block-local registers: registers defined in block B at instruction index `i_def`
    // that are never params/block-params and whose uses across the entire function all occur in block B after `i_def`.
    // Once such a register is in `Moved` state at the end of block B, it can never be read before
    // being redefined on loop re-entry and cannot leak, avoiding 2^N state explosion on server loops.
    let param_regs: HashSet<RegId> = func.params.iter().map(|(r, _)| *r).collect();
    let mut def_loc: HashMap<RegId, (BlockId, usize)> = HashMap::new();
    let mut multi_defs: HashSet<RegId> = HashSet::new();
    for b in &func.blocks {
        for (preg, _) in &b.params {
            multi_defs.insert(*preg);
        }
        for (idx, inst) in b.instructions.iter().enumerate() {
            if let Some(dst) = inst.dst {
                if def_loc.insert(dst, (b.id, idx)).is_some() || param_regs.contains(&dst) {
                    multi_defs.insert(dst);
                }
            }
        }
    }
    let mut non_local_regs: HashSet<RegId> = multi_defs;
    for b in &func.blocks {
        for (idx, inst) in b.instructions.iter().enumerate() {
            for u in get_used_registers(&inst.op) {
                match def_loc.get(&u) {
                    Some(&(def_blk, def_idx)) if def_blk == b.id && idx > def_idx => {}
                    _ => {
                        non_local_regs.insert(u);
                    }
                }
            }
        }
        for r in b.terminator.used_regs() {
            match def_loc.get(&r) {
                Some(&(def_blk, _)) if def_blk == b.id => {}
                _ => {
                    non_local_regs.insert(r);
                }
            }
        }
    }

    // Initial state for entry block (bb0)
    let mut initial_state: BTreeMap<RegId, RegState> = BTreeMap::new();
    for (idx, (reg, ty)) in func.params.iter().enumerate() {
        if ty.is_linear() {
            let borrowed = func.is_param_borrowed(idx);
            initial_state.insert(*reg, RegState::Alive {
                ty: ty.clone(),
                line: 1,
                owned: false,
                borrowed,
            });
        }
    }

    // Worklist for CFG traversal: (block_id, state)
    let entry_id = if block_map.contains_key(&0) { 0 } else { func.blocks[0].id };
    let mut worklist: VecDeque<(BlockId, BTreeMap<RegId, RegState>)> = VecDeque::new();
    worklist.push_back((entry_id, initial_state));

    let mut visited_states: HashSet<(BlockId, Vec<(RegId, bool)>)> = HashSet::new();

    while let Some((blk_id, mut state)) = worklist.pop_front() {
        let block = match block_map.get(&blk_id) {
            Some(b) => *b,
            None => continue,
        };

        // State fingerprint: (block_id, [(reg, is_alive)])
        let fingerprint: Vec<(RegId, bool)> = state
            .iter()
            .map(|(&r, s)| (r, matches!(s, RegState::Alive { .. })))
            .collect();
        if !visited_states.insert((blk_id, fingerprint)) {
            continue;
        }

        // Process instructions in this block
        for inst in &block.instructions {
            check_and_update_instruction(func, functions, inst, &mut state)?;
        }

        // Prune block-local registers that are already Moved at block exit
        if !matches!(block.terminator, Terminator::Ret(_)) {
            state.retain(|reg, reg_state| {
                !matches!(reg_state, RegState::Moved { .. }) || non_local_regs.contains(reg)
            });
        }

        let term_line = get_terminator_line(block);

        // Process terminator
        match &block.terminator {
            Terminator::Ret(ret_val) => {
                // Check if the returned register itself was used after move or is borrowed
                if let Some(ret_reg) = ret_val {
                    match state.get(ret_reg) {
                        Some(RegState::Moved { consumed_at_line, .. }) => {
                            return Err(Diagnostic::new(
                                "UseAfterMove",
                                &func.name,
                                term_line,
                                format_terminator(&block.terminator),
                            )
                            .with_violated_register(format!("%{ret_reg}"))
                            .with_consumed_at(*consumed_at_line)
                            .with_alive_registers(collect_alive_regs(&state))
                            .with_fix_hint(format!(
                                "Register %{ret_reg} was already moved at line {consumed_at_line}"
                            )));
                        }
                        Some(RegState::Alive { borrowed: true, .. }) => {
                            return Err(Diagnostic::new(
                                "BorrowEscape",
                                &func.name,
                                term_line,
                                format_terminator(&block.terminator),
                            )
                            .with_violated_register(format!("%{ret_reg}"))
                            .with_alive_registers(collect_alive_regs(&state))
                            .with_fix_hint(format!(
                                "Borrowed parameter %{ret_reg} cannot be returned from function '{}'",
                                func.name
                            )));
                        }
                        _ => {}
                    }
                }

                // Check for linear leaks: any Alive owned linear resource that is not returned
                for (&reg, reg_state) in &state {
                    if let RegState::Alive { ty, owned, .. } = reg_state {
                        if *owned && is_leakable_linear_type(ty) && Some(reg) != *ret_val {
                            return Err(Diagnostic::new(
                                "LinearLeak",
                                &func.name,
                                term_line,
                                format_terminator(&block.terminator),
                            )
                            .with_violated_register(format!("%{reg}"))
                            .with_alive_registers(collect_alive_regs(&state))
                            .with_fix_hint(format!(
                                "Linear resource %{reg} of type {ty} was not freed or returned before function exit"
                            )));
                        }
                    }
                }
            }
            Terminator::Br(target) => {
                worklist.push_back((*target, state));
            }
            Terminator::BrIf {
                cond,
                then_blk,
                else_blk,
            } => {
                check_cond_not_moved(func, block, *cond, term_line, &state)?;
                worklist.push_back((*then_blk, state.clone()));
                worklist.push_back((*else_blk, state));
            }
            Terminator::BrArgs { target, args } => {
                let next_state = transfer_edge_args(
                    func,
                    block,
                    term_line,
                    &state,
                    block_map.get(target).copied(),
                    args,
                )?;
                worklist.push_back((*target, next_state));
            }
            Terminator::BrIfArgs {
                cond,
                then_blk,
                then_args,
                else_blk,
                else_args,
            } => {
                check_cond_not_moved(func, block, *cond, term_line, &state)?;
                let then_state = transfer_edge_args(
                    func,
                    block,
                    term_line,
                    &state,
                    block_map.get(then_blk).copied(),
                    then_args,
                )?;
                let else_state = transfer_edge_args(
                    func,
                    block,
                    term_line,
                    &state,
                    block_map.get(else_blk).copied(),
                    else_args,
                )?;
                worklist.push_back((*then_blk, then_state));
                worklist.push_back((*else_blk, else_state));
            }
        }
    }

    Ok(())
}

fn check_cond_not_moved(
    func: &Function,
    block: &BasicBlock,
    cond: RegId,
    term_line: usize,
    state: &BTreeMap<RegId, RegState>,
) -> Result<(), Diagnostic> {
    if let Some(RegState::Moved { consumed_at_line, .. }) = state.get(&cond) {
        return Err(Diagnostic::new(
            "UseAfterMove",
            &func.name,
            term_line,
            format_terminator(&block.terminator),
        )
        .with_violated_register(format!("%{cond}"))
        .with_consumed_at(*consumed_at_line)
        .with_alive_registers(collect_alive_regs(state))
        .with_fix_hint(format!(
            "Register %{cond} was already moved at line {consumed_at_line}"
        )));
    }
    Ok(())
}

fn transfer_edge_args(
    func: &Function,
    block: &BasicBlock,
    term_line: usize,
    base_state: &BTreeMap<RegId, RegState>,
    target_blk: Option<&BasicBlock>,
    args: &[RegId],
) -> Result<BTreeMap<RegId, RegState>, Diagnostic> {
    let mut edge_state = base_state.clone();
    let mut seen_linear = HashSet::new();
    let mut pending_bindings: Vec<(RegId, RegState)> = Vec::new();

    for &arg in args {
        if let Some(RegState::Moved { consumed_at_line, .. }) = edge_state.get(&arg) {
            return Err(Diagnostic::new(
                "UseAfterMove",
                &func.name,
                term_line,
                format_terminator(&block.terminator),
            )
            .with_violated_register(format!("%{arg}"))
            .with_consumed_at(*consumed_at_line)
            .with_alive_registers(collect_alive_regs(&edge_state))
            .with_fix_hint(format!(
                "Branch argument %{arg} was already moved at line {consumed_at_line}"
            )));
        }
    }

    if let Some(target) = target_blk {
        for (&arg, (param_reg, param_ty)) in args.iter().zip(target.params.iter()) {
            if param_ty.is_linear() {
                if let Some(RegState::Alive { borrowed: true, .. }) = edge_state.get(&arg) {
                    return Err(Diagnostic::new(
                        "BorrowEscape",
                        &func.name,
                        term_line,
                        format_terminator(&block.terminator),
                    )
                    .with_violated_register(format!("%{arg}"))
                    .with_alive_registers(collect_alive_regs(&edge_state))
                    .with_fix_hint(format!(
                        "Borrowed register %{arg} cannot be moved into block parameter %{param_reg}"
                    )));
                }
                if !seen_linear.insert(arg) {
                    return Err(Diagnostic::new(
                        "UseAfterMove",
                        &func.name,
                        term_line,
                        format_terminator(&block.terminator),
                    )
                    .with_violated_register(format!("%{arg}"))
                    .with_consumed_at(term_line)
                    .with_alive_registers(collect_alive_regs(&edge_state))
                    .with_fix_hint(format!(
                        "Linear register %{arg} is passed multiple times as branch argument"
                    )));
                }
                let owned = match edge_state.get(&arg) {
                    Some(RegState::Alive { owned, .. }) => *owned,
                    _ => true,
                };
                if arg != *param_reg {
                    edge_state.insert(
                        arg,
                        RegState::Moved {
                            ty: param_ty.clone(),
                            consumed_at_line: term_line,
                        },
                    );
                }
                pending_bindings.push((
                    *param_reg,
                    RegState::Alive {
                        ty: param_ty.clone(),
                        line: term_line,
                        owned,
                        borrowed: false,
                    },
                ));
            }
        }
    }

    for (param_reg, st) in pending_bindings {
        edge_state.insert(param_reg, st);
    }

    Ok(edge_state)
}

fn check_and_update_instruction(
    func: &Function,
    functions: &[Function],
    inst: &Instruction,
    state: &mut BTreeMap<RegId, RegState>,
) -> Result<(), Diagnostic> {
    // 1. Verify no operand is used after move
    let used_regs = get_used_registers(&inst.op);
    for reg in used_regs {
        if let Some(RegState::Moved { consumed_at_line, .. }) = state.get(&reg) {
            return Err(Diagnostic::new(
                "UseAfterMove",
                &func.name,
                inst.line,
                format_instruction(inst),
            )
            .with_violated_register(format!("%{reg}"))
            .with_consumed_at(*consumed_at_line)
            .with_alive_registers(collect_alive_regs(state))
            .with_fix_hint(format!(
                "Register %{reg} was already moved at line {consumed_at_line}"
            )));
        }
    }

    // 2. Mark consumed registers as Moved (and reject moving borrowed parameters!)
    let consumed_regs = get_consumed_registers(func, functions, &inst.op, state);
    let mut moved_in_this_op = std::collections::HashSet::new();
    for reg in consumed_regs {
        if let Some(RegState::Alive { borrowed: true, .. }) = state.get(&reg) {
            return Err(Diagnostic::new(
                "BorrowEscape",
                &func.name,
                inst.line,
                format_instruction(inst),
            )
            .with_violated_register(format!("%{reg}"))
            .with_alive_registers(collect_alive_regs(state))
            .with_fix_hint(format!(
                "Borrowed parameter %{reg} is read-only and cannot be moved or consumed"
            )));
        }
        if !moved_in_this_op.insert(reg) {
            return Err(Diagnostic::new(
                "UseAfterMove",
                &func.name,
                inst.line,
                format_instruction(inst),
            )
            .with_violated_register(format!("%{reg}"))
            .with_consumed_at(inst.line)
            .with_alive_registers(collect_alive_regs(state))
            .with_fix_hint(format!(
                "Register %{reg} is consumed multiple times in the same instruction"
            )));
        }
        let ty = match state.get(&reg) {
            Some(RegState::Alive { ty, .. }) => ty.clone(),
            _ => Type::Void,
        };
        state.insert(reg, RegState::Moved {
            ty,
            consumed_at_line: inst.line,
        });
    }

    // 3. Mark newly defined destination register as Alive (if linear or tracked)
    if let Some(dst) = inst.dst {
        if inst.ty.is_linear() {
            let owned = !matches!(
                inst.op,
                Op::MapGet(_, _)
                    | Op::VecGet(_, _)
                    | Op::ConstStr(_)
                    | Op::StructGet(_, _)
                    | Op::IntToStr(_)
            );
            state.insert(dst, RegState::Alive {
                ty: inst.ty.clone(),
                line: inst.line,
                owned,
                borrowed: false,
            });
        }
    } else if is_leakable_linear_type(&inst.ty) {
        let owned = !matches!(
            inst.op,
            Op::MapGet(_, _)
                | Op::VecGet(_, _)
                | Op::ConstStr(_)
                | Op::StructGet(_, _)
                | Op::IntToStr(_)
        );
        if owned {
            return Err(Diagnostic::new(
                "LinearLeak",
                &func.name,
                inst.line,
                format_instruction(inst),
            )
            .with_fix_hint(format!(
                "Instruction creates linear resource of type {} without a destination register, leaking it immediately",
                inst.ty
            )));
        }
    }

    Ok(())
}

/// Identifies all registers used (borrowed or consumed) by an opcode.
fn get_used_registers(op: &Op) -> Vec<RegId> {
    match op {
        Op::ConstI64(_) | Op::ConstF64(_) | Op::ConstBool(_) | Op::ConstStr(_) => Vec::new(),
        Op::Add(a, b)
        | Op::Sub(a, b)
        | Op::Mul(a, b)
        | Op::Div(a, b)
        | Op::Rem(a, b)
        | Op::Pow(a, b)
        | Op::Atan2(a, b)
        | Op::Min(a, b)
        | Op::Max(a, b)
        | Op::CmpEq(a, b)
        | Op::CmpLt(a, b) => vec![*a, *b],
        Op::IntToStr(a)
        | Op::StrLen(a)
        | Op::Sin(a)
        | Op::Cos(a)
        | Op::Tan(a)
        | Op::Sqrt(a)
        | Op::Abs(a)
        | Op::Floor(a)
        | Op::Ceil(a)
        | Op::IntToFloat(a)
        | Op::FloatToInt(a) => vec![*a],
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

/// Identifies which registers are consumed (ownership transferred) by an opcode.
fn get_consumed_registers(
    func: &Function,
    functions: &[Function],
    op: &Op,
    state: &BTreeMap<RegId, RegState>,
) -> Vec<RegId> {
    match op {
        Op::BufWriteU8(b, _, _) => vec![*b],
        Op::VecPush(v, item) => {
            let mut res = vec![*v];
            if is_reg_linear(item, state) {
                res.push(*item);
            }
            res
        }
        Op::MapSet(m, k, v) => {
            let mut res = vec![*m];
            if is_reg_linear(k, state) {
                res.push(*k);
            }
            if is_reg_linear(v, state) {
                res.push(*v);
            }
            res
        }
        Op::MapDel(m, _) => vec![*m],
        Op::ChanSend(_, val) => {
            if is_reg_linear(val, state) {
                vec![*val]
            } else {
                Vec::new()
            }
        }
        Op::Syscall(_, _, payload) => {
            if is_reg_linear(payload, state) {
                vec![*payload]
            } else {
                Vec::new()
            }
        }
        Op::Free(r) => vec![*r],
        Op::StructSet(s, _, val) => {
            let mut res = vec![*s];
            if is_reg_linear(val, state) {
                res.push(*val);
            }
            res
        }
        Op::Call(fn_name, args) => {
            let callee = functions
                .iter()
                .find(|f| &f.name == fn_name)
                .or_else(|| if &func.name == fn_name { Some(func) } else { None });
            args.iter()
                .enumerate()
                .filter(|(i, r)| {
                    is_reg_linear(r, state) && !callee.is_some_and(|c| c.is_param_borrowed(*i))
                })
                .map(|(_, r)| *r)
                .collect()
        }
        _ => Vec::new(),
    }
}

fn is_reg_linear(reg: &RegId, state: &BTreeMap<RegId, RegState>) -> bool {
    match state.get(reg) {
        Some(RegState::Alive { ty, .. }) | Some(RegState::Moved { ty, .. }) => ty.is_linear(),
        None => false,
    }
}

/// Linear types that must be consumed or freed before function return to prevent leaks.
/// Note: String constants (`Type::Str`) do not allocate unbounded heap memory and do not leak.
fn is_leakable_linear_type(ty: &Type) -> bool {
    matches!(
        ty,
        Type::Buf | Type::Vec(_) | Type::Map(_, _) | Type::Chan(_) | Type::Struct(_)
    )
}

fn collect_alive_regs(state: &BTreeMap<RegId, RegState>) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for (&reg, s) in state {
        if let RegState::Alive { ty, .. } = s {
            out.insert(format!("%{reg}"), ty.to_string());
        }
    }
    out
}

fn get_terminator_line(block: &BasicBlock) -> usize {
    if let Some(last_inst) = block.instructions.last() {
        last_inst.line + 1
    } else {
        1
    }
}
