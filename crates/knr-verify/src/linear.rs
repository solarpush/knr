use crate::diagnostic::{format_instruction, format_terminator, Diagnostic};
use knr_core::{BasicBlock, BlockId, Function, Instruction, Op, RegId, Terminator, Type};
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegState {
    Alive { ty: Type, line: usize, owned: bool },
    Moved { ty: Type, consumed_at_line: usize },
}

/// Validates linear ownership rules:
/// 1. Use-after-move: No register in `Moved` state can be read or consumed.
/// 2. Linear leak: Linear resources (Buf, Vec, Map, Chan) must be consumed or returned at `ret`.
pub fn verify_linear(func: &Function) -> Result<(), Diagnostic> {
    if func.blocks.is_empty() {
        return Ok(());
    }

    let block_map: HashMap<BlockId, &BasicBlock> =
        func.blocks.iter().map(|b| (b.id, b)).collect();

    // Initial state for entry block (bb0)
    let mut initial_state: BTreeMap<RegId, RegState> = BTreeMap::new();
    for (reg, ty) in &func.params {
        if ty.is_linear() {
            initial_state.insert(*reg, RegState::Alive {
                ty: ty.clone(),
                line: 1,
                owned: false,
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
            check_and_update_instruction(func, inst, &mut state)?;
        }

        // Process terminator
        match &block.terminator {
            Terminator::Ret(ret_val) => {
                // Check if the returned register itself was used after move
                if let Some(ret_reg) = ret_val {
                    if let Some(RegState::Moved { consumed_at_line, .. }) = state.get(ret_reg) {
                        return Err(Diagnostic::new(
                            "UseAfterMove",
                            &func.name,
                            get_terminator_line(block),
                            format_terminator(&block.terminator),
                        )
                        .with_violated_register(format!("%{ret_reg}"))
                        .with_consumed_at(*consumed_at_line)
                        .with_alive_registers(collect_alive_regs(&state))
                        .with_fix_hint(format!(
                            "Register %{ret_reg} was already moved at line {consumed_at_line}"
                        )));
                    }
                }

                // Check for linear leaks: any Alive owned linear resource that is not returned
                for (&reg, reg_state) in &state {
                    if let RegState::Alive { ty, owned, .. } = reg_state {
                        if *owned && is_leakable_linear_type(ty) && Some(reg) != *ret_val {
                            return Err(Diagnostic::new(
                                "LinearLeak",
                                &func.name,
                                get_terminator_line(block),
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
                if let Some(RegState::Moved { consumed_at_line, .. }) = state.get(cond) {
                    return Err(Diagnostic::new(
                        "UseAfterMove",
                        &func.name,
                        get_terminator_line(block),
                        format_terminator(&block.terminator),
                    )
                    .with_violated_register(format!("%{cond}"))
                    .with_consumed_at(*consumed_at_line)
                    .with_alive_registers(collect_alive_regs(&state))
                    .with_fix_hint(format!(
                        "Register %{cond} was already moved at line {consumed_at_line}"
                    )));
                }
                worklist.push_back((*then_blk, state.clone()));
                worklist.push_back((*else_blk, state));
            }
        }
    }

    Ok(())
}

fn check_and_update_instruction(
    func: &Function,
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

    // 2. Mark consumed registers as Moved
    let consumed_regs = get_consumed_registers(&inst.op, state);
    for reg in consumed_regs {
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
                Op::MapGet(_, _) | Op::VecGet(_, _) | Op::ConstStr(_) | Op::StructGet(_, _)
            );
            state.insert(dst, RegState::Alive {
                ty: inst.ty.clone(),
                line: inst.line,
                owned,
            });
        }
    } else if is_leakable_linear_type(&inst.ty) {
        let owned = !matches!(
            inst.op,
            Op::MapGet(_, _) | Op::VecGet(_, _) | Op::ConstStr(_) | Op::StructGet(_, _)
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
    }
}

/// Identifies which registers are consumed (ownership transferred) by an opcode.
fn get_consumed_registers(op: &Op, state: &BTreeMap<RegId, RegState>) -> Vec<RegId> {
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
