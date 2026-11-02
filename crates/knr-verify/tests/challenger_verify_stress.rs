use knr_core::{BasicBlock, Effect, Function, Instruction, Module, Op, Terminator, Type};
use knr_verify::{verify_function, verify_module};

#[test]
fn test_verify_linear_branch_imbalance_rejected() {
    // Branch where then-block consumes %2 (BufWriteU8) but else-block does not consume %2.
    // Merge block returns %2. In then-block %2 is Moved, so using it at merge is UseAfterMove!
    let mut func = Function::new("branch_imbalance", Type::Buf);
    func.add_param(0, Type::Bool);

    let mut bb0 = BasicBlock::new(
        0,
        Terminator::BrIf {
            cond: 0,
            then_blk: 1,
            else_blk: 2,
        },
    );
    bb0.push(Instruction::new(Some(1), Type::I64, Op::ConstI64(10), 1));
    bb0.push(Instruction::new(Some(2), Type::Buf, Op::BufAlloc(1), 2));

    // bb1 consumes %2
    let mut bb1 = BasicBlock::new(1, Terminator::Br(3));
    bb1.push(Instruction::new(Some(3), Type::I64, Op::ConstI64(0), 3));
    bb1.push(Instruction::new(Some(4), Type::I64, Op::ConstI64(99), 4));
    bb1.push(Instruction::new(Some(5), Type::Buf, Op::BufWriteU8(2, 3, 4), 5));

    // bb2 does not consume %2
    let bb2 = BasicBlock::new(2, Terminator::Br(3));

    // bb3 tries to return %2 (which was consumed in bb1!)
    let bb3 = BasicBlock::new(3, Terminator::Ret(Some(2)));

    func.add_block(bb0);
    func.add_block(bb1);
    func.add_block(bb2);
    func.add_block(bb3);

    let err = verify_function(&func).expect_err("Must reject use of consumed linear register across branch");
    let json = serde_json::to_string(&err).unwrap();
    assert!(json.contains("error"));
    assert!(
        err.error_kind.contains("Linear") || err.error_kind.contains("UseAfterMove"),
        "Unexpected error_kind: {}",
        err.error_kind
    );
}

#[test]
fn test_verify_buf_slice_operand_type_rejection() {
    // Op::BufSlice expects (Buf, I64, I64)
    // Pass (I64, I64, I64)
    let mut func = Function::new("bad_slice_buf_type", Type::Buf);
    let mut bb0 = BasicBlock::new(0, Terminator::Ret(Some(3)));
    bb0.push(Instruction::new(Some(0), Type::I64, Op::ConstI64(10), 1));
    bb0.push(Instruction::new(Some(1), Type::I64, Op::ConstI64(0), 2));
    bb0.push(Instruction::new(Some(2), Type::I64, Op::ConstI64(5), 3));
    bb0.push(Instruction::new(Some(3), Type::Buf, Op::BufSlice(0, 1, 2), 4));
    func.add_block(bb0);

    let err = verify_function(&func).expect_err("Must reject BufSlice on non-Buf register");
    assert!(err.error_kind.contains("TypeMismatch") || err.error_kind.contains("Type"));
}

#[test]
fn test_verify_effect_safety_transitive_and_direct() {
    // Function without Net effect calling syscall with Effect::Net
    let mut func = Function::new("net_caller", Type::Void);
    // func has no effects declared
    let mut bb0 = BasicBlock::new(0, Terminator::Ret(None));
    bb0.push(Instruction::new(Some(0), Type::Str, Op::ConstStr("http://evil.com".into()), 1));
    bb0.push(Instruction::new(Some(1), Type::Void, Op::Syscall(Effect::Net, "listen".into(), 0), 2));
    func.add_block(bb0);

    let err = verify_function(&func).expect_err("Must reject undeclared effect");
    assert!(err.error_kind.contains("Effect") || err.error_kind.contains("Capability"));
}

#[test]
fn test_verify_valid_module_roundtrip() {
    let mut module = Module::new();

    let mut func = Function::new("valid_func", Type::I64);
    func.effects.push(Effect::Hw);
    let mut bb0 = BasicBlock::new(0, Terminator::Ret(Some(2)));
    bb0.push(Instruction::new(Some(0), Type::I64, Op::ConstI64(40), 1));
    bb0.push(Instruction::new(Some(1), Type::I64, Op::ConstI64(2), 2));
    bb0.push(Instruction::new(Some(2), Type::I64, Op::Add(0, 1), 3));
    func.add_block(bb0);

    module.functions.push(func);

    let res = verify_module(&module);
    assert!(res.is_ok(), "Valid module must pass verification cleanly");
}
