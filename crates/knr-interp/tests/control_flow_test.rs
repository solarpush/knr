use knr_core::{BasicBlock, Function, Instruction, Op, Terminator, Type};
use knr_interp::{execute, Value, VmError};
use knr_runtime::MockPlatform;

#[test]
fn test_branching_and_ret() {
    let mut platform = MockPlatform::new();

    // fn max(a: i64, b: i64) -> i64
    // bb0:
    //   %2 = cmp_lt %0 %1
    //   br_if %2 bb1 bb2
    // bb1:
    //   ret %1
    // bb2:
    //   ret %0
    let mut func = Function::new("max", Type::I64);
    func.add_param(0, Type::I64);
    func.add_param(1, Type::I64);

    let mut bb0 = BasicBlock::new(0, Terminator::BrIf { cond: 2, then_blk: 1, else_blk: 2 });
    bb0.push(Instruction::new(Some(2), Type::Bool, Op::CmpLt(0, 1), 1));

    let bb1 = BasicBlock::new(1, Terminator::Ret(Some(1)));
    let bb2 = BasicBlock::new(2, Terminator::Ret(Some(0)));

    func.add_block(bb0);
    func.add_block(bb1);
    func.add_block(bb2);

    // Test then branch (10 < 20 -> 20)
    let (res1, _) = execute(&func, vec![Value::I64(10), Value::I64(20)], &mut platform, 1000, false)
        .expect("Execution should succeed");
    assert_eq!(res1, Value::I64(20));

    // Test else branch (30 < 20 -> 30)
    let (res2, _) = execute(&func, vec![Value::I64(30), Value::I64(20)], &mut platform, 1000, false)
        .expect("Execution should succeed");
    assert_eq!(res2, Value::I64(30));
}

#[test]
fn test_unconditional_br() {
    let mut platform = MockPlatform::new();

    let mut func = Function::new("uncond", Type::I64);
    let mut bb0 = BasicBlock::new(0, Terminator::Br(1));
    bb0.push(Instruction::new(Some(0), Type::I64, Op::ConstI64(42), 1));

    let mut bb1 = BasicBlock::new(1, Terminator::Ret(Some(1)));
    bb1.push(Instruction::new(Some(1), Type::I64, Op::Add(0, 0), 2));

    func.add_block(bb0);
    func.add_block(bb1);

    let (res, _) = execute(&func, vec![], &mut platform, 1000, false).expect("Execution should succeed");
    assert_eq!(res, Value::I64(84));
}

#[test]
fn test_no_such_block() {
    let mut platform = MockPlatform::new();

    let mut func = Function::new("bad_jump", Type::Void);
    let bb0 = BasicBlock::new(0, Terminator::Br(99));
    func.add_block(bb0);

    let err = execute(&func, vec![], &mut platform, 1000, false).unwrap_err();
    assert_eq!(err, VmError::NoSuchBlock { block_id: 99 });
}

#[test]
fn test_invalid_register_access() {
    let mut platform = MockPlatform::new();

    let mut func = Function::new("bad_reg", Type::I64);
    let bb0 = BasicBlock::new(0, Terminator::Ret(Some(42))); // %42 was never assigned
    func.add_block(bb0);

    let err = execute(&func, vec![], &mut platform, 1000, false).unwrap_err();
    assert_eq!(err, VmError::InvalidRegisterAccess { reg: 42 });
}

#[test]
fn test_ret_void() {
    let mut platform = MockPlatform::new();

    let mut func = Function::new("void_func", Type::Void);
    let bb0 = BasicBlock::new(0, Terminator::Ret(None));
    func.add_block(bb0);

    let (res, _) = execute(&func, vec![], &mut platform, 1000, false).expect("Execution should succeed");
    assert_eq!(res, Value::Void);
}
