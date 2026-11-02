use knr_core::{BasicBlock, Function, Instruction, Op, Terminator, Type};
use knr_interp::{execute, Value, VmError};
use knr_runtime::MockPlatform;

#[test]
fn test_i64_arithmetic() {
    let mut platform = MockPlatform::new();

    // fn test(a: i64, b: i64) -> i64
    let mut func = Function::new("arith_i64", Type::I64);
    func.add_param(0, Type::I64);
    func.add_param(1, Type::I64);

    let mut bb0 = BasicBlock::new(0, Terminator::Ret(Some(5)));
    // %2 = add %0 %1   (10 + 3 = 13)
    bb0.push(Instruction::new(Some(2), Type::I64, Op::Add(0, 1), 1));
    // %3 = sub %2 %1   (13 - 3 = 10)
    bb0.push(Instruction::new(Some(3), Type::I64, Op::Sub(2, 1), 2));
    // %4 = mul %3 %1   (10 * 3 = 30)
    bb0.push(Instruction::new(Some(4), Type::I64, Op::Mul(3, 1), 3));
    // %5 = div %4 %1   (30 / 3 = 10)
    bb0.push(Instruction::new(Some(5), Type::I64, Op::Div(4, 1), 4));
    func.add_block(bb0);

    let (res, _) = execute(&func, vec![Value::I64(10), Value::I64(3)], &mut platform, 1000, false)
        .expect("Execution should succeed");
    assert_eq!(res, Value::I64(10));
}

#[test]
fn test_i64_division_by_zero() {
    let mut platform = MockPlatform::new();

    let mut func = Function::new("div_zero", Type::I64);
    let mut bb0 = BasicBlock::new(0, Terminator::Ret(Some(2)));
    bb0.push(Instruction::new(Some(0), Type::I64, Op::ConstI64(10), 10));
    bb0.push(Instruction::new(Some(1), Type::I64, Op::ConstI64(0), 11));
    bb0.push(Instruction::new(Some(2), Type::I64, Op::Div(0, 1), 12));
    func.add_block(bb0);

    let err = execute(&func, vec![], &mut platform, 1000, false).unwrap_err();
    assert_eq!(err, VmError::DivisionByZero { line: 12 });
}

#[test]
fn test_f64_arithmetic() {
    let mut platform = MockPlatform::new();

    let mut func = Function::new("arith_f64", Type::F64);
    let mut bb0 = BasicBlock::new(0, Terminator::Ret(Some(5)));
    bb0.push(Instruction::new(Some(0), Type::F64, Op::ConstF64(10.5), 1));
    bb0.push(Instruction::new(Some(1), Type::F64, Op::ConstF64(2.5), 2));
    bb0.push(Instruction::new(Some(2), Type::F64, Op::Add(0, 1), 3)); // 13.0
    bb0.push(Instruction::new(Some(3), Type::F64, Op::Sub(2, 1), 4)); // 10.5
    bb0.push(Instruction::new(Some(4), Type::F64, Op::Mul(3, 1), 5)); // 26.25
    bb0.push(Instruction::new(Some(5), Type::F64, Op::Div(4, 1), 6)); // 10.5
    func.add_block(bb0);

    let (res, _) = execute(&func, vec![], &mut platform, 1000, false).expect("Execution should succeed");
    assert_eq!(res, Value::F64(10.5));
}

#[test]
fn test_str_concat_and_cmp() {
    let mut platform = MockPlatform::new();

    let mut func = Function::new("str_test", Type::Bool);
    let mut bb0 = BasicBlock::new(0, Terminator::Ret(Some(4)));
    bb0.push(Instruction::new(Some(0), Type::Str, Op::ConstStr("hello".into()), 1));
    bb0.push(Instruction::new(Some(1), Type::Str, Op::ConstStr(" world".into()), 2));
    bb0.push(Instruction::new(Some(2), Type::Str, Op::Add(0, 1), 3));
    bb0.push(Instruction::new(Some(3), Type::Str, Op::ConstStr("hello world".into()), 4));
    bb0.push(Instruction::new(Some(4), Type::Bool, Op::CmpEq(2, 3), 5));
    func.add_block(bb0);

    let (res, _) = execute(&func, vec![], &mut platform, 1000, false).expect("Execution should succeed");
    assert_eq!(res, Value::Bool(true));
}

#[test]
fn test_comparisons() {
    let mut platform = MockPlatform::new();

    let mut func = Function::new("cmp_test", Type::Bool);
    let mut bb0 = BasicBlock::new(0, Terminator::Ret(Some(2)));
    bb0.push(Instruction::new(Some(0), Type::I64, Op::ConstI64(10), 1));
    bb0.push(Instruction::new(Some(1), Type::I64, Op::ConstI64(20), 2));
    bb0.push(Instruction::new(Some(2), Type::Bool, Op::CmpLt(0, 1), 3));
    func.add_block(bb0);

    let (res, _) = execute(&func, vec![], &mut platform, 1000, false).expect("Execution should succeed");
    assert_eq!(res, Value::Bool(true));
}

#[test]
fn test_type_mismatch_error() {
    let mut platform = MockPlatform::new();

    let mut func = Function::new("mismatch", Type::I64);
    let mut bb0 = BasicBlock::new(0, Terminator::Ret(Some(2)));
    bb0.push(Instruction::new(Some(0), Type::I64, Op::ConstI64(10), 1));
    bb0.push(Instruction::new(Some(1), Type::Bool, Op::ConstBool(true), 2));
    bb0.push(Instruction::new(Some(2), Type::I64, Op::Add(0, 1), 3));
    func.add_block(bb0);

    let err = execute(&func, vec![], &mut platform, 1000, false).unwrap_err();
    match err {
        VmError::TypeMismatch { line, .. } => assert_eq!(line, 3),
        other => panic!("Expected TypeMismatch, got {other:?}"),
    }
}
