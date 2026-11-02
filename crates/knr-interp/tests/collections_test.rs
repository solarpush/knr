use knr_core::{BasicBlock, Function, Instruction, Op, Terminator, Type};
use knr_interp::{execute, Value, VmError};
use knr_runtime::MockPlatform;

#[test]
fn test_buffer_operations() {
    let mut platform = MockPlatform::new();

    // bb0:
    //   %0 = const_i64 8
    //   %1 = buf_alloc %0
    //   %2 = const_i64 3
    //   %3 = const_i64 255
    //   %4 = buf_write_u8 %1 %2 %3
    //   %5 = buf_read_u8 %4 %2
    //   ret %5
    let mut func = Function::new("buf_ops", Type::I64);
    let mut bb0 = BasicBlock::new(0, Terminator::Ret(Some(5)));
    bb0.push(Instruction::new(Some(0), Type::I64, Op::ConstI64(8), 1));
    bb0.push(Instruction::new(Some(1), Type::Buf, Op::BufAlloc(0), 2));
    bb0.push(Instruction::new(Some(2), Type::I64, Op::ConstI64(3), 3));
    bb0.push(Instruction::new(Some(3), Type::I64, Op::ConstI64(255), 4));
    bb0.push(Instruction::new(Some(4), Type::Buf, Op::BufWriteU8(1, 2, 3), 5));
    bb0.push(Instruction::new(Some(5), Type::I64, Op::BufReadU8(4, 2), 6));
    func.add_block(bb0);

    let (res, _) = execute(&func, vec![], &mut platform, 100, false).expect("exec ok");
    assert_eq!(res, Value::I64(255));
}

#[test]
fn test_buf_slice() {
    let mut platform = MockPlatform::new();

    let mut func = Function::new("buf_slice_test", Type::Buf);
    let mut bb0 = BasicBlock::new(0, Terminator::Ret(Some(4)));
    bb0.push(Instruction::new(Some(0), Type::I64, Op::ConstI64(10), 1));
    bb0.push(Instruction::new(Some(1), Type::Buf, Op::BufAlloc(0), 2));
    bb0.push(Instruction::new(Some(2), Type::I64, Op::ConstI64(2), 3));
    bb0.push(Instruction::new(Some(3), Type::I64, Op::ConstI64(4), 4));
    bb0.push(Instruction::new(Some(4), Type::Buf, Op::BufSlice(1, 2, 3), 5));
    func.add_block(bb0);

    let (res, _) = execute(&func, vec![], &mut platform, 100, false).expect("exec ok");
    match res {
        Value::Buf(b) => assert_eq!(b.len(), 4),
        other => panic!("Expected Buf, got {other:?}"),
    }
}

#[test]
fn test_vector_operations() {
    let mut platform = MockPlatform::new();

    // %0 = vec_new i64
    // %1 = const_i64 100
    // %2 = vec_push %0 %1
    // %3 = const_i64 200
    // %4 = vec_push %2 %3
    // %5 = vec_len %4
    // %6 = const_i64 1
    // %7 = vec_get %4 %6
    let mut func = Function::new("vec_ops", Type::I64);
    let mut bb0 = BasicBlock::new(0, Terminator::Ret(Some(7)));
    bb0.push(Instruction::new(Some(0), Type::Vec(Box::new(Type::I64)), Op::VecNew(Type::I64), 1));
    bb0.push(Instruction::new(Some(1), Type::I64, Op::ConstI64(100), 2));
    bb0.push(Instruction::new(Some(2), Type::Vec(Box::new(Type::I64)), Op::VecPush(0, 1), 3));
    bb0.push(Instruction::new(Some(3), Type::I64, Op::ConstI64(200), 4));
    bb0.push(Instruction::new(Some(4), Type::Vec(Box::new(Type::I64)), Op::VecPush(2, 3), 5));
    bb0.push(Instruction::new(Some(5), Type::I64, Op::VecLen(4), 6));
    bb0.push(Instruction::new(Some(6), Type::I64, Op::ConstI64(1), 7));
    bb0.push(Instruction::new(Some(7), Type::I64, Op::VecGet(4, 6), 8));
    func.add_block(bb0);

    let (res, _) = execute(&func, vec![], &mut platform, 100, false).expect("exec ok");
    assert_eq!(res, Value::I64(200));
}

#[test]
fn test_map_operations() {
    let mut platform = MockPlatform::new();

    // %0 = map_new str i64
    // %1 = const_str "key1"
    // %2 = const_i64 999
    // %3 = map_set %0 %1 %2
    // %4 = map_get %3 %1
    let mut func = Function::new("map_ops", Type::I64);
    let mut bb0 = BasicBlock::new(0, Terminator::Ret(Some(4)));
    bb0.push(Instruction::new(
        Some(0),
        Type::Map(Box::new(Type::Str), Box::new(Type::I64)),
        Op::MapNew(Type::Str, Type::I64),
        1,
    ));
    bb0.push(Instruction::new(Some(1), Type::Str, Op::ConstStr("key1".into()), 2));
    bb0.push(Instruction::new(Some(2), Type::I64, Op::ConstI64(999), 3));
    bb0.push(Instruction::new(
        Some(3),
        Type::Map(Box::new(Type::Str), Box::new(Type::I64)),
        Op::MapSet(0, 1, 2),
        4,
    ));
    bb0.push(Instruction::new(Some(4), Type::I64, Op::MapGet(3, 1), 5));
    func.add_block(bb0);

    let (res, _) = execute(&func, vec![], &mut platform, 100, false).expect("exec ok");
    assert_eq!(res, Value::I64(999));
}

#[test]
fn test_channel_send_recv() {
    let mut platform = MockPlatform::new();

    // %0 = const_i64 10 (cap)
    // %1 = chan_new i64 %0
    // %2 = const_i64 777
    // %3 = chan_send %1 %2
    // %4 = chan_recv %1
    let mut func = Function::new("chan_ops", Type::I64);
    let mut bb0 = BasicBlock::new(0, Terminator::Ret(Some(4)));
    bb0.push(Instruction::new(Some(0), Type::I64, Op::ConstI64(10), 1));
    bb0.push(Instruction::new(Some(1), Type::Chan(Box::new(Type::I64)), Op::ChanNew(Type::I64, 0), 2));
    bb0.push(Instruction::new(Some(2), Type::I64, Op::ConstI64(777), 3));
    bb0.push(Instruction::new(Some(3), Type::Bool, Op::ChanSend(1, 2), 4));
    bb0.push(Instruction::new(Some(4), Type::I64, Op::ChanRecv(1), 5));
    func.add_block(bb0);

    let (res, _) = execute(&func, vec![], &mut platform, 100, false).expect("exec ok");
    assert_eq!(res, Value::I64(777));
}

#[test]
fn test_free_opcode() {
    let mut platform = MockPlatform::new();

    // %0 = const_i64 42
    // free %0
    // ret %0 -> should fail with InvalidRegisterAccess
    let mut func = Function::new("free_test", Type::I64);
    let mut bb0 = BasicBlock::new(0, Terminator::Ret(Some(0)));
    bb0.push(Instruction::new(Some(0), Type::I64, Op::ConstI64(42), 1));
    bb0.push(Instruction::new(None, Type::Void, Op::Free(0), 2));
    func.add_block(bb0);

    let err = execute(&func, vec![], &mut platform, 100, false).unwrap_err();
    assert_eq!(err, VmError::InvalidRegisterAccess { reg: 0 });
}

#[test]
fn test_buf_slice_boundary_and_overflow_cases() {
    let mut platform = MockPlatform::new();

    // Test negative start
    {
        let mut func = Function::new("neg_start", Type::Buf);
        let mut bb0 = BasicBlock::new(0, Terminator::Ret(Some(4)));
        bb0.push(Instruction::new(Some(0), Type::I64, Op::ConstI64(10), 1));
        bb0.push(Instruction::new(Some(1), Type::Buf, Op::BufAlloc(0), 2));
        bb0.push(Instruction::new(Some(2), Type::I64, Op::ConstI64(-1), 3));
        bb0.push(Instruction::new(Some(3), Type::I64, Op::ConstI64(5), 4));
        bb0.push(Instruction::new(Some(4), Type::Buf, Op::BufSlice(1, 2, 3), 5));
        func.add_block(bb0);
        let err = execute(&func, vec![], &mut platform, 100, false).unwrap_err();
        assert!(matches!(err, VmError::IndexOutOfBounds { .. }));
    }

    // Test negative length
    {
        let mut func = Function::new("neg_len", Type::Buf);
        let mut bb0 = BasicBlock::new(0, Terminator::Ret(Some(4)));
        bb0.push(Instruction::new(Some(0), Type::I64, Op::ConstI64(10), 1));
        bb0.push(Instruction::new(Some(1), Type::Buf, Op::BufAlloc(0), 2));
        bb0.push(Instruction::new(Some(2), Type::I64, Op::ConstI64(2), 3));
        bb0.push(Instruction::new(Some(3), Type::I64, Op::ConstI64(-3), 4));
        bb0.push(Instruction::new(Some(4), Type::Buf, Op::BufSlice(1, 2, 3), 5));
        func.add_block(bb0);
        let err = execute(&func, vec![], &mut platform, 100, false).unwrap_err();
        assert!(matches!(err, VmError::IndexOutOfBounds { .. }));
    }

    // Test start + len beyond buffer length
    {
        let mut func = Function::new("oob_slice", Type::Buf);
        let mut bb0 = BasicBlock::new(0, Terminator::Ret(Some(4)));
        bb0.push(Instruction::new(Some(0), Type::I64, Op::ConstI64(10), 1));
        bb0.push(Instruction::new(Some(1), Type::Buf, Op::BufAlloc(0), 2));
        bb0.push(Instruction::new(Some(2), Type::I64, Op::ConstI64(8), 3));
        bb0.push(Instruction::new(Some(3), Type::I64, Op::ConstI64(5), 4));
        bb0.push(Instruction::new(Some(4), Type::Buf, Op::BufSlice(1, 2, 3), 5));
        func.add_block(bb0);
        let err = execute(&func, vec![], &mut platform, 100, false).unwrap_err();
        assert_eq!(err, VmError::IndexOutOfBounds { index: 13, len: 10, line: 5 });
    }
}

