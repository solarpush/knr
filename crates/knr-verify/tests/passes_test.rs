use knr_core::{BasicBlock, Effect, Function, Instruction, Module, Op, Terminator, Type};
use knr_verify::{verify_function, verify_module, Diagnostic};
use std::collections::BTreeMap;

#[test]
fn test_missing_entry_block() {
    let mut func = Function::new("no_entry", Type::I64);
    let mut bb1 = BasicBlock::new(1, Terminator::Ret(Some(0)));
    bb1.push(Instruction::new(Some(0), Type::I64, Op::ConstI64(42), 2));
    func.add_block(bb1);

    let err = verify_function(&func).unwrap_err();
    assert_eq!(err.error_kind, "MissingEntryBlock");
    assert_eq!(err.function, "no_entry");
}

#[test]
fn test_invalid_branch_targets() {
    // br bb99
    let mut func = Function::new("bad_br", Type::Void);
    let bb0 = BasicBlock::new(0, Terminator::Br(99));
    func.add_block(bb0);
    let err = verify_function(&func).unwrap_err();
    assert_eq!(err.error_kind, "InvalidBranchTarget");

    // br_if %0 bb1 bb99
    let mut func2 = Function::new("bad_br_if", Type::Void);
    func2.add_param(0, Type::Bool);
    let bb0 = BasicBlock::new(
        0,
        Terminator::BrIf {
            cond: 0,
            then_blk: 1,
            else_blk: 99,
        },
    );
    let bb1 = BasicBlock::new(1, Terminator::Ret(None));
    func2.add_block(bb0);
    func2.add_block(bb1);
    let err = verify_function(&func2).unwrap_err();
    assert_eq!(err.error_kind, "InvalidBranchTarget");
}

#[test]
fn test_single_assignment_violations() {
    // Parameter duplicate
    let mut func = Function::new("dup_param", Type::Void);
    func.add_param(0, Type::I64);
    func.add_param(0, Type::I64);
    let bb0 = BasicBlock::new(0, Terminator::Ret(None));
    func.add_block(bb0);
    let err = verify_function(&func).unwrap_err();
    assert_eq!(err.error_kind, "MultipleAssignment");

    // Instruction redefining parameter
    let mut func2 = Function::new("redef_param", Type::Void);
    func2.add_param(0, Type::I64);
    let mut bb0 = BasicBlock::new(0, Terminator::Ret(None));
    bb0.push(Instruction::new(Some(0), Type::I64, Op::ConstI64(10), 3));
    func2.add_block(bb0);
    let err = verify_function(&func2).unwrap_err();
    assert_eq!(err.error_kind, "MultipleAssignment");

    // Instruction redefining earlier instruction
    let mut func3 = Function::new("redef_inst", Type::I64);
    let mut bb0 = BasicBlock::new(0, Terminator::Ret(Some(0)));
    bb0.push(Instruction::new(Some(0), Type::I64, Op::ConstI64(1), 2));
    bb0.push(Instruction::new(Some(0), Type::I64, Op::ConstI64(2), 3));
    func3.add_block(bb0);
    let err = verify_function(&func3).unwrap_err();
    assert_eq!(err.error_kind, "MultipleAssignment");
}

#[test]
fn test_ssa_monotonicity() {
    // Non-monotonic instructions: %0 then %3 then %1
    let mut func = Function::new("non_mono", Type::I64);
    let mut bb0 = BasicBlock::new(0, Terminator::Ret(Some(1)));
    bb0.push(Instruction::new(Some(0), Type::I64, Op::ConstI64(1), 2));
    bb0.push(Instruction::new(Some(3), Type::I64, Op::ConstI64(2), 3));
    bb0.push(Instruction::new(Some(1), Type::I64, Op::Add(0, 3), 4));
    func.add_block(bb0);
    let err = verify_function(&func).unwrap_err();
    assert_eq!(err.error_kind, "NonMonotonicRegisters");
    assert_eq!(err.violated_register, Some("%1".to_string()));

    // Non-monotonic across parameters: %1 then %0
    let mut func2 = Function::new("non_mono_params", Type::Void);
    func2.add_param(1, Type::I64);
    func2.add_param(0, Type::I64);
    let bb0 = BasicBlock::new(0, Terminator::Ret(None));
    func2.add_block(bb0);
    let err = verify_function(&func2).unwrap_err();
    assert_eq!(err.error_kind, "NonMonotonicRegisters");
    assert_eq!(err.violated_register, Some("%0".to_string()));
}

#[test]
fn test_ssa_dominance_advanced() {
    // Diamond CFG: bb0 branches to bb1 and bb2, which merge into bb3.
    // Register %2 is defined in bb1, but used in bb3.
    // bb1 does not dominate bb3 because bb0 can reach bb3 through bb2!
    let mut func = Function::new("diamond_dom", Type::I64);
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
    bb1.push(Instruction::new(Some(1), Type::I64, Op::ConstI64(10), 4));

    let mut bb2 = BasicBlock::new(2, Terminator::Br(3));
    bb2.push(Instruction::new(Some(2), Type::I64, Op::ConstI64(20), 6));

    let mut bb3 = BasicBlock::new(3, Terminator::Ret(Some(3)));
    // Uses %1 which is only defined in bb1
    bb3.push(Instruction::new(Some(3), Type::I64, Op::Add(1, 2), 8));

    func.add_block(bb0);
    func.add_block(bb1);
    func.add_block(bb2);
    func.add_block(bb3);

    let err = verify_function(&func).unwrap_err();
    assert_eq!(err.error_kind, "DominanceViolation");
    assert_eq!(err.violated_register, Some("%1".to_string()));
}

#[test]
fn test_linear_use_after_move() {
    // 1. Double free
    let mut func = Function::new("double_free", Type::Void);
    let mut bb0 = BasicBlock::new(0, Terminator::Ret(None));
    bb0.push(Instruction::new(Some(0), Type::I64, Op::ConstI64(32), 2));
    bb0.push(Instruction::new(Some(1), Type::Buf, Op::BufAlloc(0), 3));
    bb0.push(Instruction::new(None, Type::Void, Op::Free(1), 4));
    bb0.push(Instruction::new(None, Type::Void, Op::Free(1), 5));
    func.add_block(bb0);

    let err = verify_function(&func).unwrap_err();
    assert_eq!(err.error_kind, "UseAfterMove");
    assert_eq!(err.violated_register, Some("%1".to_string()));
    assert_eq!(err.consumed_at_line, Some(4));

    // 2. MapSet then MapGet on old map
    let mut func2 = Function::new("map_use_after_move", Type::I64);
    let mut bb0 = BasicBlock::new(0, Terminator::Ret(Some(5)));
    bb0.push(Instruction::new(Some(0), Type::Map(Box::new(Type::Str), Box::new(Type::I64)), Op::MapNew(Type::Str, Type::I64), 2));
    bb0.push(Instruction::new(Some(1), Type::Str, Op::ConstStr("k".into()), 3));
    bb0.push(Instruction::new(Some(2), Type::I64, Op::ConstI64(42), 4));
    bb0.push(Instruction::new(Some(3), Type::Map(Box::new(Type::Str), Box::new(Type::I64)), Op::MapSet(0, 1, 2), 5));
    bb0.push(Instruction::new(Some(4), Type::Str, Op::ConstStr("k".into()), 6));
    bb0.push(Instruction::new(Some(5), Type::I64, Op::MapGet(0, 4), 7)); // uses %0 which moved at line 5
    func2.add_block(bb0);

    let err = verify_function(&func2).unwrap_err();
    assert_eq!(err.error_kind, "UseAfterMove");
    assert_eq!(err.violated_register, Some("%0".to_string()));
    assert_eq!(err.consumed_at_line, Some(5));
}

#[test]
fn test_linear_leak_detection() {
    // Leaked buffer
    let mut func = Function::new("leak_buf", Type::I64);
    let mut bb0 = BasicBlock::new(0, Terminator::Ret(Some(0)));
    bb0.push(Instruction::new(Some(0), Type::I64, Op::ConstI64(10), 2));
    bb0.push(Instruction::new(Some(1), Type::Buf, Op::BufAlloc(0), 3));
    func.add_block(bb0);

    let err = verify_function(&func).unwrap_err();
    assert_eq!(err.error_kind, "LinearLeak");
    assert_eq!(err.violated_register, Some("%1".to_string()));

    // Leaked vector
    let mut func2 = Function::new("leak_vec", Type::Void);
    let mut bb0 = BasicBlock::new(0, Terminator::Ret(None));
    bb0.push(Instruction::new(Some(0), Type::Vec(Box::new(Type::I64)), Op::VecNew(Type::I64), 2));
    func2.add_block(bb0);

    let err = verify_function(&func2).unwrap_err();
    assert_eq!(err.error_kind, "LinearLeak");
    assert_eq!(err.violated_register, Some("%0".to_string()));
}

#[test]
fn test_static_type_checking() {
    // Arithmetic mismatch: i64 + f64
    let mut func = Function::new("arith_mismatch", Type::I64);
    let mut bb0 = BasicBlock::new(0, Terminator::Ret(Some(2)));
    bb0.push(Instruction::new(Some(0), Type::I64, Op::ConstI64(1), 2));
    bb0.push(Instruction::new(Some(1), Type::F64, Op::ConstF64(2.0), 3));
    bb0.push(Instruction::new(Some(2), Type::I64, Op::Add(0, 1), 4));
    func.add_block(bb0);
    let err = verify_function(&func).unwrap_err();
    assert_eq!(err.error_kind, "TypeMismatch");

    // br_if condition not bool
    let mut func2 = Function::new("cond_not_bool", Type::Void);
    let mut bb0 = BasicBlock::new(
        0,
        Terminator::BrIf {
            cond: 0,
            then_blk: 1,
            else_blk: 1,
        },
    );
    bb0.push(Instruction::new(Some(0), Type::I64, Op::ConstI64(1), 2));
    let bb1 = BasicBlock::new(1, Terminator::Ret(None));
    func2.add_block(bb0);
    func2.add_block(bb1);
    let err = verify_function(&func2).unwrap_err();
    assert_eq!(err.error_kind, "TypeMismatch");

    // Return type mismatch
    let mut func3 = Function::new("ret_mismatch", Type::I64);
    let mut bb0 = BasicBlock::new(0, Terminator::Ret(Some(0)));
    bb0.push(Instruction::new(Some(0), Type::Bool, Op::ConstBool(true), 2));
    func3.add_block(bb0);
    let err = verify_function(&func3).unwrap_err();
    assert_eq!(err.error_kind, "TypeMismatch");
}

#[test]
fn test_effect_capabilities() {
    // Undeclared Net syscall
    let mut func = Function::new("pure_fn_net", Type::I64);
    let mut bb0 = BasicBlock::new(0, Terminator::Ret(Some(0)));
    bb0.push(Instruction::new(Some(0), Type::I64, Op::ConstI64(1), 2));
    bb0.push(Instruction::new(Some(1), Type::I64, Op::Syscall(Effect::Net, "listen".into(), 0), 3));
    func.add_block(bb0);
    let err = verify_function(&func).unwrap_err();
    assert_eq!(err.error_kind, "UndeclaredEffect");

    // Undeclared Async for chan_new
    let mut func2 = Function::new("pure_fn_chan", Type::Void);
    let mut bb0 = BasicBlock::new(0, Terminator::Ret(None));
    bb0.push(Instruction::new(Some(0), Type::I64, Op::ConstI64(10), 2));
    bb0.push(Instruction::new(Some(1), Type::Chan(Box::new(Type::I64)), Op::ChanNew(Type::I64, 0), 3));
    func2.add_block(bb0);
    let err = verify_function(&func2).unwrap_err();
    assert_eq!(err.error_kind, "UndeclaredEffect");

    // Undeclared Async for spawn
    let mut func3 = Function::new("pure_fn_spawn", Type::Void);
    let mut bb0 = BasicBlock::new(0, Terminator::Ret(None));
    bb0.push(Instruction::new(Some(0), Type::I64, Op::ConstI64(10), 2));
    bb0.push(Instruction::new(Some(1), Type::Bool, Op::Spawn("task".into(), 0), 3));
    func3.add_block(bb0);
    let err = verify_function(&func3).unwrap_err();
    assert_eq!(err.error_kind, "UndeclaredEffect");

    // Multi-function effect propagation in module
    let mut module = Module::new();
    let mut callee = Function::new("callee", Type::Void);
    callee.add_effect(Effect::Net);
    let bb_callee = BasicBlock::new(0, Terminator::Ret(None));
    callee.add_block(bb_callee);

    let mut caller = Function::new("caller", Type::Void);
    caller.add_effect(Effect::Async); // Has Async, but missing Net!
    let mut bb_caller = BasicBlock::new(0, Terminator::Ret(None));
    bb_caller.push(Instruction::new(Some(0), Type::I64, Op::ConstI64(0), 2));
    bb_caller.push(Instruction::new(Some(1), Type::Bool, Op::Spawn("callee".into(), 0), 3));
    caller.add_block(bb_caller);

    module.add_function(callee);
    module.add_function(caller);

    let errs = verify_module(&module).unwrap_err();
    assert_eq!(errs.len(), 1);
    assert_eq!(errs[0].error_kind, "UndeclaredEffect");
}

#[test]
fn test_diagnostic_json_serialization() {
    let mut alive = BTreeMap::new();
    alive.insert("%2".to_string(), "I64".to_string());
    alive.insert("%4".to_string(), "Map<Str, Buf>".to_string());

    let diag = Diagnostic::new(
        "UseAfterMove",
        "handle_request",
        14,
        "%6 = map_get %2 %3",
    )
    .with_violated_register("%2")
    .with_consumed_at(11)
    .with_alive_registers(alive)
    .with_fix_hint("Replace %2 with %4 (the updated Map returned by map_set at line 11).");

    let json_str = diag.to_json().unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();

    assert_eq!(parsed["status"], "error");
    assert_eq!(parsed["error_kind"], "UseAfterMove");
    assert_eq!(parsed["function"], "handle_request");
    assert_eq!(parsed["line"], 14);
    assert_eq!(parsed["instruction"], "%6 = map_get %2 %3");
    assert_eq!(parsed["violated_register"], "%2");
    assert_eq!(parsed["consumed_at_line"], 11);
    assert_eq!(parsed["alive_registers"]["%2"], "I64");
    assert_eq!(parsed["alive_registers"]["%4"], "Map<Str, Buf>");
    assert!(parsed["fix_hint"].as_str().unwrap().contains("Replace %2 with %4"));
}


#[test]
fn test_container_type_mismatches() {
    // 1. vec_push wrong element type
    let mut func1 = Function::new("bad_vec_push", Type::Void);
    let mut bb0 = BasicBlock::new(0, Terminator::Ret(None));
    bb0.push(Instruction::new(Some(0), Type::Vec(Box::new(Type::I64)), Op::VecNew(Type::I64), 2));
    bb0.push(Instruction::new(Some(1), Type::Str, Op::ConstStr("hello".into()), 3));
    bb0.push(Instruction::new(Some(2), Type::Vec(Box::new(Type::I64)), Op::VecPush(0, 1), 4));
    bb0.push(Instruction::new(None, Type::Void, Op::Free(2), 5));
    func1.add_block(bb0);
    let err1 = verify_function(&func1).unwrap_err();
    assert_eq!(err1.error_kind, "TypeMismatch");

    // 2. map_set wrong key type
    let mut func2 = Function::new("bad_map_key", Type::Void);
    let mut bb0 = BasicBlock::new(0, Terminator::Ret(None));
    bb0.push(Instruction::new(Some(0), Type::Map(Box::new(Type::Str), Box::new(Type::I64)), Op::MapNew(Type::Str, Type::I64), 2));
    bb0.push(Instruction::new(Some(1), Type::I64, Op::ConstI64(999), 3));
    bb0.push(Instruction::new(Some(2), Type::I64, Op::ConstI64(42), 4));
    bb0.push(Instruction::new(Some(3), Type::Map(Box::new(Type::Str), Box::new(Type::I64)), Op::MapSet(0, 1, 2), 5));
    bb0.push(Instruction::new(None, Type::Void, Op::Free(3), 6));
    func2.add_block(bb0);
    let err2 = verify_function(&func2).unwrap_err();
    assert_eq!(err2.error_kind, "TypeMismatch");

    // 3. chan_send wrong val type
    let mut func3 = Function::new("bad_chan_send", Type::Void);
    func3.add_effect(Effect::Async);
    let mut bb0 = BasicBlock::new(0, Terminator::Ret(None));
    bb0.push(Instruction::new(Some(0), Type::I64, Op::ConstI64(10), 2));
    bb0.push(Instruction::new(Some(1), Type::Chan(Box::new(Type::I64)), Op::ChanNew(Type::I64, 0), 3));
    bb0.push(Instruction::new(Some(2), Type::Str, Op::ConstStr("wrong".into()), 4));
    bb0.push(Instruction::new(Some(3), Type::Bool, Op::ChanSend(1, 2), 5));
    bb0.push(Instruction::new(None, Type::Void, Op::Free(1), 6));
    func3.add_block(bb0);
    let err3 = verify_function(&func3).unwrap_err();
    assert_eq!(err3.error_kind, "TypeMismatch");
}

#[test]
fn test_dominance_self_referential() {
    let mut func = Function::new("self_ref", Type::I64);
    let mut bb0 = BasicBlock::new(0, Terminator::Ret(Some(0)));
    bb0.push(Instruction::new(Some(0), Type::I64, Op::Add(0, 1), 2));
    bb0.push(Instruction::new(Some(1), Type::I64, Op::ConstI64(10), 3));
    func.add_block(bb0);
    let err = verify_function(&func).unwrap_err();
    assert_eq!(err.error_kind, "DominanceViolation");
}

#[test]
fn test_linear_leak_map_and_chan() {
    // Leaked map
    let mut func = Function::new("leak_map", Type::Void);
    let mut bb0 = BasicBlock::new(0, Terminator::Ret(None));
    bb0.push(Instruction::new(Some(0), Type::Map(Box::new(Type::Str), Box::new(Type::I64)), Op::MapNew(Type::Str, Type::I64), 2));
    func.add_block(bb0);
    let err = verify_function(&func).unwrap_err();
    assert_eq!(err.error_kind, "LinearLeak");
    assert_eq!(err.violated_register, Some("%0".to_string()));

    // Leaked channel
    let mut func2 = Function::new("leak_chan", Type::Void);
    func2.add_effect(Effect::Async);
    let mut bb0 = BasicBlock::new(0, Terminator::Ret(None));
    bb0.push(Instruction::new(Some(0), Type::I64, Op::ConstI64(10), 2));
    bb0.push(Instruction::new(Some(1), Type::Chan(Box::new(Type::I64)), Op::ChanNew(Type::I64, 0), 3));
    func2.add_block(bb0);
    let err = verify_function(&func2).unwrap_err();
    assert_eq!(err.error_kind, "LinearLeak");
    assert_eq!(err.violated_register, Some("%1".to_string()));
}

#[test]
fn test_verifier_performance_spec() {
    // SPEC.md §3: verification designed to run in under 100 microseconds per function
    let mut func = Function::new("perf_test", Type::I64);
    func.add_param(0, Type::I64);
    func.add_param(1, Type::I64);
    let mut bb0 = BasicBlock::new(0, Terminator::Ret(Some(7)));
    bb0.push(Instruction::new(Some(2), Type::I64, Op::Add(0, 1), 2));
    bb0.push(Instruction::new(Some(3), Type::I64, Op::ConstI64(2), 3));
    bb0.push(Instruction::new(Some(4), Type::I64, Op::Mul(2, 3), 4));
    bb0.push(Instruction::new(Some(5), Type::I64, Op::ConstI64(1), 5));
    bb0.push(Instruction::new(Some(6), Type::I64, Op::Sub(4, 5), 6));
    bb0.push(Instruction::new(Some(7), Type::I64, Op::Div(6, 3), 7));
    func.add_block(bb0);

    let start = std::time::Instant::now();
    let iters = 1_000;
    for _ in 0..iters {
        let res = verify_function(&func);
        assert!(res.is_ok());
    }
    let elapsed = start.elapsed();
    let micros_per_func = (elapsed.as_micros() as f64) / (iters as f64);
    println!("Average verify_function time: {micros_per_func:.2} µs");
    assert!(
        micros_per_func < 100.0,
        "Verification too slow: {micros_per_func} µs > 100 µs target"
    );
}
