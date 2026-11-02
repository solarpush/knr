use knr_core::{BasicBlock, Function, Instruction, Op, Terminator, Type};
use knr_interp::{execute, Value, VmError};
use knr_runtime::MockPlatform;

#[test]
fn test_infinite_loop_gas_exhaustion() {
    let mut platform = MockPlatform::new();

    // bb0:
    //   br bb0
    let mut func = Function::new("inf_loop", Type::Void);
    let bb0 = BasicBlock::new(0, Terminator::Br(0));
    func.add_block(bb0);

    let err = execute(&func, vec![], &mut platform, 100, false).unwrap_err();
    assert_eq!(err, VmError::GasExhausted { max_steps: 100 });
}

#[test]
fn test_loop_with_instructions_gas_exhaustion() {
    let mut platform = MockPlatform::new();

    // bb0:
    //   %0 = const_i64 1
    //   %1 = add %0 %0
    //   br bb0
    let mut func = Function::new("loop_insts", Type::Void);
    let mut bb0 = BasicBlock::new(0, Terminator::Br(0));
    bb0.push(Instruction::new(Some(0), Type::I64, Op::ConstI64(1), 1));
    bb0.push(Instruction::new(Some(1), Type::I64, Op::Add(0, 0), 2));
    func.add_block(bb0);

    let err = execute(&func, vec![], &mut platform, 50, false).unwrap_err();
    assert_eq!(err, VmError::GasExhausted { max_steps: 50 });
}

#[test]
fn test_zero_max_steps() {
    let mut platform = MockPlatform::new();

    let mut func = Function::new("one_inst", Type::I64);
    let mut bb0 = BasicBlock::new(0, Terminator::Ret(Some(0)));
    bb0.push(Instruction::new(Some(0), Type::I64, Op::ConstI64(42), 1));
    func.add_block(bb0);

    let err = execute(&func, vec![], &mut platform, 0, false).unwrap_err();
    assert_eq!(err, VmError::GasExhausted { max_steps: 0 });
}

#[test]
fn test_exact_gas_boundary() {
    let mut platform = MockPlatform::new();

    // bb0:
    //   %0 = const_i64 1 (inst 1)
    //   %1 = const_i64 2 (inst 2)
    //   ret %1           (term 3)
    // Total steps = 3.
    let mut func = Function::new("three_steps", Type::I64);
    let mut bb0 = BasicBlock::new(0, Terminator::Ret(Some(1)));
    bb0.push(Instruction::new(Some(0), Type::I64, Op::ConstI64(1), 1));
    bb0.push(Instruction::new(Some(1), Type::I64, Op::ConstI64(2), 2));
    func.add_block(bb0);

    // 2 steps is not enough -> GasExhausted
    let err = execute(&func, vec![], &mut platform, 2, false).unwrap_err();
    assert_eq!(err, VmError::GasExhausted { max_steps: 2 });

    // 3 steps is exactly enough
    let (res, _) = execute(&func, vec![], &mut platform, 3, false)
        .expect("3 steps should suffice");
    assert_eq!(res, Value::I64(2));
}
