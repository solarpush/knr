use knr_core::{BasicBlock, Effect, Function, Instruction, Module, Op, Terminator, Type};
use knr_verify::{verify_function, verify_module, Diagnostic};
use std::collections::BTreeMap;

// ============================================================================
// 1. SSA DOMINANCE & STRUCTURAL INTEGRITY ADVERSARIAL STRESS TESTS
// ============================================================================

#[test]
fn test_cross_branch_dominance_violation() {
    // Diamond CFG: bb0 branches to bb1 and bb2. bb2 branches to bb3.
    // bb2 tries to use %1 defined in bb1!
    let mut func = Function::new("cross_branch_dom", Type::I64);
    func.add_param(0, Type::Bool);

    let bb0 = BasicBlock::new(
        0,
        Terminator::BrIf {
            cond: 0,
            then_blk: 1,
            else_blk: 2,
        },
    );

    let mut bb1 = BasicBlock::new(1, Terminator::Br(3));
    bb1.push(Instruction::new(Some(1), Type::I64, Op::ConstI64(10), 3));

    let mut bb2 = BasicBlock::new(2, Terminator::Br(3));
    // Violates dominance: %1 is only defined in sibling branch bb1!
    bb2.push(Instruction::new(Some(2), Type::I64, Op::Add(1, 1), 6));

    let mut bb3 = BasicBlock::new(3, Terminator::Ret(Some(3)));
    bb3.push(Instruction::new(Some(3), Type::I64, Op::ConstI64(42), 8));

    func.add_block(bb0);
    func.add_block(bb1);
    func.add_block(bb2);
    func.add_block(bb3);

    let err = verify_function(&func).expect_err("Must reject cross-branch register use");
    assert_eq!(err.error_kind, "DominanceViolation");
    assert_eq!(err.violated_register, Some("%1".to_string()));
}

#[test]
fn test_dominance_loop_backedge_use() {
    // Loop: bb0 -> bb1 (header) -> bb2 (body) -> bb1
    // Register %2 is defined in bb2, but bb1 uses %2 on the initial entry from bb0!
    // Since bb0 does not pass through bb2, bb2 does not dominate bb1.
    let mut func = Function::new("loop_backedge_dom", Type::I64);
    func.add_param(0, Type::Bool);

    let bb0 = BasicBlock::new(0, Terminator::Br(1));

    let mut bb1 = BasicBlock::new(
        1,
        Terminator::BrIf {
            cond: 0,
            then_blk: 2,
            else_blk: 3,
        },
    );
    // Uses %2 which is only defined in bb2
    bb1.push(Instruction::new(Some(1), Type::I64, Op::Add(2, 2), 4));

    let mut bb2 = BasicBlock::new(2, Terminator::Br(1));
    bb2.push(Instruction::new(Some(2), Type::I64, Op::ConstI64(100), 7));

    let mut bb3 = BasicBlock::new(3, Terminator::Ret(Some(3)));
    bb3.push(Instruction::new(Some(3), Type::I64, Op::ConstI64(0), 10));

    func.add_block(bb0);
    func.add_block(bb1);
    func.add_block(bb2);
    func.add_block(bb3);

    let err = verify_function(&func).expect_err("Must reject using loop body register in header");
    assert_eq!(err.error_kind, "DominanceViolation");
    assert_eq!(err.violated_register, Some("%2".to_string()));
}

#[test]
fn test_dominance_loop_exit_use() {
    // Loop: bb0 -> bb1 -> bb2 (body) -> bb1, bb1 -> bb3 (exit)
    // bb2 defines %2. bb3 (exit) tries to use %2.
    // If the loop condition is false initially, bb2 never runs, so bb2 does not dominate bb3!
    let mut func = Function::new("loop_exit_dom", Type::I64);
    func.add_param(0, Type::Bool);

    let bb0 = BasicBlock::new(0, Terminator::Br(1));

    let bb1 = BasicBlock::new(
        1,
        Terminator::BrIf {
            cond: 0,
            then_blk: 2,
            else_blk: 3,
        },
    );

    let mut bb2 = BasicBlock::new(2, Terminator::Br(1));
    bb2.push(Instruction::new(Some(1), Type::I64, Op::ConstI64(99), 5));

    let mut bb3 = BasicBlock::new(3, Terminator::Ret(Some(2)));
    bb3.push(Instruction::new(Some(2), Type::I64, Op::Add(1, 1), 8));

    func.add_block(bb0);
    func.add_block(bb1);
    func.add_block(bb2);
    func.add_block(bb3);

    let err = verify_function(&func).expect_err("Must reject using loop body register after loop exit");
    assert_eq!(err.error_kind, "DominanceViolation");
    assert_eq!(err.violated_register, Some("%1".to_string()));
}

#[test]
fn test_terminator_dominance_violation() {
    // Terminator br_if uses %99 which was never defined
    let mut func = Function::new("term_bad_cond", Type::Void);
    let bb0 = BasicBlock::new(
        0,
        Terminator::BrIf {
            cond: 99,
            then_blk: 1,
            else_blk: 1,
        },
    );
    let bb1 = BasicBlock::new(1, Terminator::Ret(None));
    func.add_block(bb0);
    func.add_block(bb1);

    let err = verify_function(&func).expect_err("Must reject undefined cond register in br_if");
    assert_eq!(err.error_kind, "DominanceViolation");
    assert_eq!(err.violated_register, Some("%99".to_string()));
}

#[test]
fn test_ret_terminator_dominance_violation() {
    // ret %99 where %99 is undefined
    let mut func = Function::new("term_bad_ret", Type::I64);
    let bb0 = BasicBlock::new(0, Terminator::Ret(Some(99)));
    func.add_block(bb0);

    let err = verify_function(&func).expect_err("Must reject undefined ret register");
    assert_eq!(err.error_kind, "DominanceViolation");
    assert_eq!(err.violated_register, Some("%99".to_string()));
}

#[test]
fn test_nested_loops_dominance() {
    // Outer loop bb1, inner loop bb2.
    // Inner loop bb2 defines %2.
    // Outer loop bb1 tries to use %2.
    let mut func = Function::new("nested_loop_dom", Type::I64);
    func.add_param(0, Type::Bool);

    let bb0 = BasicBlock::new(0, Terminator::Br(1));

    let mut bb1 = BasicBlock::new(
        1,
        Terminator::BrIf {
            cond: 0,
            then_blk: 2,
            else_blk: 4,
        },
    );
    // Uses %2 from inner loop
    bb1.push(Instruction::new(Some(1), Type::I64, Op::Add(2, 2), 4));

    let mut bb2 = BasicBlock::new(
        2,
        Terminator::BrIf {
            cond: 0,
            then_blk: 3,
            else_blk: 1,
        },
    );
    bb2.push(Instruction::new(Some(2), Type::I64, Op::ConstI64(10), 7));

    let bb3 = BasicBlock::new(3, Terminator::Br(2));

    let mut bb4 = BasicBlock::new(4, Terminator::Ret(Some(3)));
    bb4.push(Instruction::new(Some(3), Type::I64, Op::ConstI64(0), 12));

    func.add_block(bb0);
    func.add_block(bb1);
    func.add_block(bb2);
    func.add_block(bb3);
    func.add_block(bb4);

    let err = verify_function(&func).expect_err("Must reject inner loop register use in outer loop");
    assert_eq!(err.error_kind, "DominanceViolation");
    assert_eq!(err.violated_register, Some("%2".to_string()));
}

// ============================================================================
// 2. LINEAR STATE MACHINE & OWNERSHIP ADVERSARIAL STRESS TESTS
// ============================================================================

#[test]
fn test_use_after_move_diamond_cfg() {
    // bb0 allocates buffer %1.
    // bb0 branches to bb1 and bb2.
    // bb1 consumes %1 (free %1).
    // bb2 does not consume %1.
    // Both jump to bb3.
    // bb3 attempts to read %1 via buf_read_u8!
    // Since %1 was moved along bb1 path, this is a use-after-move!
    let mut func = Function::new("diamond_uam", Type::I64);
    func.add_param(0, Type::Bool);

    let mut bb0 = BasicBlock::new(
        0,
        Terminator::BrIf {
            cond: 0,
            then_blk: 1,
            else_blk: 2,
        },
    );
    bb0.push(Instruction::new(Some(1), Type::I64, Op::ConstI64(64), 2));
    bb0.push(Instruction::new(Some(2), Type::Buf, Op::BufAlloc(1), 3));

    let mut bb1 = BasicBlock::new(1, Terminator::Br(3));
    bb1.push(Instruction::new(None, Type::Void, Op::Free(2), 5));

    let bb2 = BasicBlock::new(2, Terminator::Br(3));

    let mut bb3 = BasicBlock::new(3, Terminator::Ret(Some(4)));
    bb3.push(Instruction::new(Some(3), Type::I64, Op::ConstI64(0), 8));
    bb3.push(Instruction::new(Some(4), Type::I64, Op::BufReadU8(2, 3), 9));
    bb3.push(Instruction::new(None, Type::Void, Op::Free(2), 10));

    func.add_block(bb0);
    func.add_block(bb1);
    func.add_block(bb2);
    func.add_block(bb3);

    let err = verify_function(&func).expect_err("Must reject use-after-move in diamond merge");
    assert_eq!(err.error_kind, "UseAfterMove");
    assert_eq!(err.violated_register, Some("%2".to_string()));
    assert_eq!(err.consumed_at_line, Some(5));
}

#[test]
fn test_linear_leak_diamond_cfg() {
    // bb0 allocates buffer %1.
    // bb0 branches to bb1 and bb2.
    // bb1 frees %1 and returns.
    // bb2 does NOT free %1 and returns 0. (Leak!)
    let mut func = Function::new("diamond_leak", Type::I64);
    func.add_param(0, Type::Bool);

    let mut bb0 = BasicBlock::new(
        0,
        Terminator::BrIf {
            cond: 0,
            then_blk: 1,
            else_blk: 2,
        },
    );
    bb0.push(Instruction::new(Some(1), Type::I64, Op::ConstI64(64), 2));
    bb0.push(Instruction::new(Some(2), Type::Buf, Op::BufAlloc(1), 3));

    let mut bb1 = BasicBlock::new(1, Terminator::Ret(Some(3)));
    bb1.push(Instruction::new(None, Type::Void, Op::Free(2), 5));
    bb1.push(Instruction::new(Some(3), Type::I64, Op::ConstI64(1), 6));

    let mut bb2 = BasicBlock::new(2, Terminator::Ret(Some(4)));
    bb2.push(Instruction::new(Some(4), Type::I64, Op::ConstI64(0), 8)); // Leaks %2!

    func.add_block(bb0);
    func.add_block(bb1);
    func.add_block(bb2);

    let err = verify_function(&func).expect_err("Must detect linear leak on else branch");
    assert_eq!(err.error_kind, "LinearLeak");
    assert_eq!(err.violated_register, Some("%2".to_string()));
}

#[test]
fn test_vec_push_linear_element_consumed() {
    // %1 is a linear Buf.
    // %2 is a Vec[Buf].
    // vec_push %2 %1 consumes %1 and %2, producing %3.
    // Attempting to read %1 afterwards must be rejected as UseAfterMove!
    let mut func = Function::new("vec_push_linear_move", Type::I64);
    let mut bb0 = BasicBlock::new(0, Terminator::Ret(Some(6)));
    bb0.push(Instruction::new(Some(0), Type::I64, Op::ConstI64(32), 2));
    bb0.push(Instruction::new(Some(1), Type::Buf, Op::BufAlloc(0), 3));
    bb0.push(Instruction::new(Some(2), Type::Vec(Box::new(Type::Buf)), Op::VecNew(Type::Buf), 4));
    bb0.push(Instruction::new(Some(3), Type::Vec(Box::new(Type::Buf)), Op::VecPush(2, 1), 5));
    // Violates linear ownership: %1 was moved into the vector!
    bb0.push(Instruction::new(Some(4), Type::I64, Op::ConstI64(0), 6));
    bb0.push(Instruction::new(Some(5), Type::I64, Op::BufReadU8(1, 4), 7));
    bb0.push(Instruction::new(Some(6), Type::I64, Op::ConstI64(0), 8));
    bb0.push(Instruction::new(None, Type::Void, Op::Free(3), 9));
    func.add_block(bb0);

    let err = verify_function(&func).expect_err("Must reject use of buffer after vec_push");
    assert_eq!(err.error_kind, "UseAfterMove");
    assert_eq!(err.violated_register, Some("%1".to_string()));
    assert_eq!(err.consumed_at_line, Some(5));
}

#[test]
fn test_chan_send_linear_value_consumed() {
    // ChanSend consumes the sent value if linear!
    let mut func = Function::new("chan_send_linear_move", Type::Void);
    func.add_effect(Effect::Async);

    let mut bb0 = BasicBlock::new(0, Terminator::Ret(None));
    bb0.push(Instruction::new(Some(0), Type::I64, Op::ConstI64(10), 2));
    bb0.push(Instruction::new(Some(1), Type::Chan(Box::new(Type::Buf)), Op::ChanNew(Type::Buf, 0), 3));
    bb0.push(Instruction::new(Some(2), Type::I64, Op::ConstI64(64), 4));
    bb0.push(Instruction::new(Some(3), Type::Buf, Op::BufAlloc(2), 5));
    // ChanSend %1 %3 consumes %3
    bb0.push(Instruction::new(Some(4), Type::Bool, Op::ChanSend(1, 3), 6));
    // Attempt to free %3 again! (Double free / UseAfterMove)
    bb0.push(Instruction::new(None, Type::Void, Op::Free(3), 7));
    bb0.push(Instruction::new(None, Type::Void, Op::Free(1), 8));
    func.add_block(bb0);

    let err = verify_function(&func).expect_err("Must reject using buffer after chan_send");
    assert_eq!(err.error_kind, "UseAfterMove");
    assert_eq!(err.violated_register, Some("%3".to_string()));
    assert_eq!(err.consumed_at_line, Some(6));
}

#[test]
fn test_syscall_linear_payload_consumed() {
    // Syscall consumes the linear payload!
    let mut func = Function::new("syscall_linear_move", Type::Void);
    func.add_effect(Effect::Hw);

    let mut bb0 = BasicBlock::new(0, Terminator::Ret(None));
    bb0.push(Instruction::new(Some(0), Type::I64, Op::ConstI64(64), 2));
    bb0.push(Instruction::new(Some(1), Type::Buf, Op::BufAlloc(0), 3));
    // Syscall Hw "simd_xor" consumes %1
    bb0.push(Instruction::new(Some(2), Type::Buf, Op::Syscall(Effect::Hw, "simd_xor".into(), 1), 4));
    // Attempt to read from original %1
    bb0.push(Instruction::new(Some(3), Type::I64, Op::ConstI64(0), 5));
    bb0.push(Instruction::new(Some(4), Type::I64, Op::BufReadU8(1, 3), 6));
    bb0.push(Instruction::new(None, Type::Void, Op::Free(2), 7));
    func.add_block(bb0);

    let err = verify_function(&func).expect_err("Must reject using buffer after syscall consumed it");
    assert_eq!(err.error_kind, "UseAfterMove");
    assert_eq!(err.violated_register, Some("%1".to_string()));
    assert_eq!(err.consumed_at_line, Some(4));
}

#[test]
fn test_returned_linear_resource_is_not_leaked() {
    // Returning an owned linear resource directly at ret is valid and must NOT report LinearLeak
    let mut func = Function::new("valid_ret_buf", Type::Buf);
    let mut bb0 = BasicBlock::new(0, Terminator::Ret(Some(1)));
    bb0.push(Instruction::new(Some(0), Type::I64, Op::ConstI64(64), 2));
    bb0.push(Instruction::new(Some(1), Type::Buf, Op::BufAlloc(0), 3));
    func.add_block(bb0);

    let res = verify_function(&func);
    assert!(res.is_ok(), "Returning allocated buffer must be accepted");
}

#[test]
fn test_const_str_not_leaked() {
    // String constants are not leakable heap resources and do not need to be freed before ret
    let mut func = Function::new("valid_str_no_leak", Type::Void);
    let mut bb0 = BasicBlock::new(0, Terminator::Ret(None));
    bb0.push(Instruction::new(Some(0), Type::Str, Op::ConstStr("hello world".into()), 2));
    func.add_block(bb0);

    let res = verify_function(&func);
    assert!(res.is_ok(), "const_str does not leak heap memory");
}

// ============================================================================
// 3. STATIC TYPING & EFFECT CAPABILITY ADVERSARIAL STRESS TESTS
// ============================================================================

#[test]
fn test_cmp_lt_disallows_invalid_types() {
    // cmp_lt only allows i64 or f64; bool, buf, vec, map are disallowed
    let mut func = Function::new("bad_cmp_lt_bool", Type::Bool);
    let mut bb0 = BasicBlock::new(0, Terminator::Ret(Some(2)));
    bb0.push(Instruction::new(Some(0), Type::Bool, Op::ConstBool(true), 2));
    bb0.push(Instruction::new(Some(1), Type::Bool, Op::ConstBool(false), 3));
    bb0.push(Instruction::new(Some(2), Type::Bool, Op::CmpLt(0, 1), 4));
    func.add_block(bb0);

    let err = verify_function(&func).expect_err("cmp_lt must disallow bool comparison");
    assert_eq!(err.error_kind, "TypeMismatch");
}

#[test]
fn test_buf_read_u8_requires_i64_index() {
    let mut func = Function::new("bad_buf_read_idx", Type::I64);
    let mut bb0 = BasicBlock::new(0, Terminator::Ret(Some(3)));
    bb0.push(Instruction::new(Some(0), Type::I64, Op::ConstI64(16), 2));
    bb0.push(Instruction::new(Some(1), Type::Buf, Op::BufAlloc(0), 3));
    bb0.push(Instruction::new(Some(2), Type::Bool, Op::ConstBool(true), 4));
    bb0.push(Instruction::new(Some(3), Type::I64, Op::BufReadU8(1, 2), 5)); // idx is bool!
    bb0.push(Instruction::new(None, Type::Void, Op::Free(1), 6));
    func.add_block(bb0);

    let err = verify_function(&func).expect_err("buf_read_u8 requires i64 index");
    assert_eq!(err.error_kind, "TypeMismatch");
}

#[test]
fn test_transitive_module_effect_violation() {
    // A spawns B, B spawns C.
    // C has effect Hw.
    // B declares Hw and Async.
    // A declares only Async (missing Hw!).
    // verify_module must detect that A spawns B which requires Hw!
    let mut module = Module::new();

    let mut func_c = Function::new("worker_c", Type::Void);
    func_c.add_effect(Effect::Hw);
    let bb_c = BasicBlock::new(0, Terminator::Ret(None));
    func_c.add_block(bb_c);

    let mut func_b = Function::new("worker_b", Type::Void);
    func_b.add_effect(Effect::Hw);
    func_b.add_effect(Effect::Async);
    let mut bb_b = BasicBlock::new(0, Terminator::Ret(None));
    bb_b.push(Instruction::new(Some(0), Type::I64, Op::ConstI64(0), 2));
    bb_b.push(Instruction::new(Some(1), Type::Bool, Op::Spawn("worker_c".into(), 0), 3));
    func_b.add_block(bb_b);

    let mut func_a = Function::new("main_a", Type::Void);
    func_a.add_effect(Effect::Async); // Missing Hw!
    let mut bb_a = BasicBlock::new(0, Terminator::Ret(None));
    bb_a.push(Instruction::new(Some(0), Type::I64, Op::ConstI64(0), 2));
    bb_a.push(Instruction::new(Some(1), Type::Bool, Op::Spawn("worker_b".into(), 0), 3));
    func_a.add_block(bb_a);

    module.add_function(func_c);
    module.add_function(func_b);
    module.add_function(func_a);

    let errs = verify_module(&module).expect_err("Must detect missing Hw effect in main_a");
    assert_eq!(errs.len(), 1);
    assert_eq!(errs[0].error_kind, "UndeclaredEffect");
    assert_eq!(errs[0].function, "main_a");
}

// ============================================================================
// 4. JSON DIAGNOSTIC STRICT SCHEMA COMPLIANCE
// ============================================================================

#[test]
fn test_diagnostic_json_fields_and_roundtrip() {
    let mut alive = BTreeMap::new();
    alive.insert("%1".to_string(), "buf".to_string());
    alive.insert("%3".to_string(), "vec[i64]".to_string());

    let diag = Diagnostic::new(
        "UseAfterMove",
        "process_payload",
        42,
        "%5 = buf_read_u8 %1 %2",
    )
    .with_violated_register("%1")
    .with_consumed_at(30)
    .with_alive_registers(alive)
    .with_fix_hint("Re-allocate buffer or avoid moving %1 at line 30.");

    let json_str = diag.to_json().unwrap();
    let val: serde_json::Value = serde_json::from_str(&json_str).unwrap();

    assert_eq!(val["status"], "error");
    assert_eq!(val["error_kind"], "UseAfterMove");
    assert_eq!(val["function"], "process_payload");
    assert_eq!(val["line"], 42);
    assert_eq!(val["instruction"], "%5 = buf_read_u8 %1 %2");
    assert_eq!(val["violated_register"], "%1");
    assert_eq!(val["consumed_at_line"], 30);
    assert_eq!(val["alive_registers"]["%1"], "buf");
    assert_eq!(val["alive_registers"]["%3"], "vec[i64]");
    assert!(val["fix_hint"].as_str().unwrap().contains("line 30"));

    // Verify deserialization back to Diagnostic struct
    let roundtripped: Diagnostic = serde_json::from_str(&json_str).unwrap();
    assert_eq!(diag, roundtripped);
}

#[test]
fn test_unreachable_cycle_dominance() {
    // bb0 is entry.
    // bb1 jumps to bb2, bb2 jumps to bb1. Neither is reachable from bb0!
    // bb1 defines %1 using %2 (from bb2).
    // bb2 defines %2.
    // In SSA, since bb1 and bb2 are unreachable, what happens?
    let mut func = Function::new("unreachable_cycle", Type::Void);
    let bb0 = BasicBlock::new(0, Terminator::Ret(None));

    let mut bb1 = BasicBlock::new(1, Terminator::Br(2));
    bb1.push(Instruction::new(Some(1), Type::I64, Op::Add(2, 2), 3));

    let mut bb2 = BasicBlock::new(2, Terminator::Br(1));
    bb2.push(Instruction::new(Some(2), Type::I64, Op::ConstI64(10), 5));

    func.add_block(bb0);
    func.add_block(bb1);
    func.add_block(bb2);

    let res = verify_function(&func);
    assert!(res.is_err(), "Must reject circular dependency in unreachable cycle");
}
