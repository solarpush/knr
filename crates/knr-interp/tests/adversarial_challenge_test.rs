use knr_core::{BasicBlock, Function, Instruction, Op, Terminator, Type};
use knr_interp::{execute, TraceRecord, Value, VmError};
use knr_runtime::MockPlatform;

// ============================================================================
// 1. GAS METERING & INFINITE LOOP ADVERSARIAL STRESS TESTS
// ============================================================================

#[test]
fn test_infinite_loop_gas_exact_exhaustion() {
    let mut platform = MockPlatform::new();

    // bb0: br bb0
    let mut func = Function::new("inf_loop", Type::Void);
    let bb0 = BasicBlock::new(0, Terminator::Br(0));
    func.add_block(bb0);

    for gas in [1, 5, 20, 100, 1000] {
        let err = execute(&func, vec![], &mut platform, gas, false).expect_err("Must halt on gas exhaustion");
        assert_eq!(err, VmError::GasExhausted { max_steps: gas });
    }
}

#[test]
fn test_multi_block_infinite_loop_gas_exhaustion() {
    let mut platform = MockPlatform::new();

    // bb0: br bb1
    // bb1: br bb0
    let mut func = Function::new("ping_pong_loop", Type::Void);
    let bb0 = BasicBlock::new(0, Terminator::Br(1));
    let bb1 = BasicBlock::new(1, Terminator::Br(0));
    func.add_block(bb0);
    func.add_block(bb1);

    for gas in [2, 3, 10, 11, 50, 101] {
        let err = execute(&func, vec![], &mut platform, gas, false).expect_err("Must halt on gas exhaustion");
        assert_eq!(err, VmError::GasExhausted { max_steps: gas });
    }
}

#[test]
fn test_loop_with_instructions_and_branching_gas_exhaustion() {
    let mut platform = MockPlatform::new();

    // bb0:
    //   %0 = const_i64 100
    //   %1 = const_i64 1
    //   br bb1
    // bb1:
    //   %2 = sub %0 %1
    //   %3 = const_i64 0
    //   %4 = cmp_lt %3 %2
    //   br_if %4 bb1 bb2
    // bb2:
    //   ret %2
    let mut func = Function::new("countdown", Type::I64);
    let mut bb0 = BasicBlock::new(0, Terminator::Br(1));
    bb0.push(Instruction::new(Some(0), Type::I64, Op::ConstI64(100), 1));
    bb0.push(Instruction::new(Some(1), Type::I64, Op::ConstI64(1), 2));

    let mut bb1 = BasicBlock::new(
        1,
        Terminator::BrIf {
            cond: 4,
            then_blk: 1,
            else_blk: 2,
        },
    );
    bb1.push(Instruction::new(Some(2), Type::I64, Op::Sub(0, 1), 4));
    bb1.push(Instruction::new(Some(3), Type::I64, Op::ConstI64(0), 5));
    bb1.push(Instruction::new(Some(4), Type::Bool, Op::CmpLt(3, 2), 6));

    let bb2 = BasicBlock::new(2, Terminator::Ret(Some(2)));

    func.add_block(bb0);
    func.add_block(bb1);
    func.add_block(bb2);

    // With insufficient gas, it must halt with GasExhausted
    let err = execute(&func, vec![], &mut platform, 10, false).expect_err("Must run out of gas");
    assert_eq!(err, VmError::GasExhausted { max_steps: 10 });
}

#[test]
fn test_exact_gas_boundary_edge() {
    let mut platform = MockPlatform::new();

    // 1 inst + 1 term = 2 steps total
    let mut func = Function::new("two_steps", Type::I64);
    let mut bb0 = BasicBlock::new(0, Terminator::Ret(Some(0)));
    bb0.push(Instruction::new(Some(0), Type::I64, Op::ConstI64(42), 1));
    func.add_block(bb0);

    // 0 steps -> GasExhausted { max_steps: 0 }
    let err0 = execute(&func, vec![], &mut platform, 0, false).unwrap_err();
    assert_eq!(err0, VmError::GasExhausted { max_steps: 0 });

    // 1 step -> GasExhausted { max_steps: 1 } (exhausts after executing the const_i64)
    let err1 = execute(&func, vec![], &mut platform, 1, false).unwrap_err();
    assert_eq!(err1, VmError::GasExhausted { max_steps: 1 });

    // 2 steps -> Success (both const_i64 and ret execute)
    let (res, _) = execute(&func, vec![], &mut platform, 2, false).expect("2 steps must succeed");
    assert_eq!(res, Value::I64(42));
}

// ============================================================================
// 2. ARITHMETIC, DIVISION BY ZERO & OVERFLOW ADVERSARIAL STRESS TESTS
// ============================================================================

#[test]
fn test_div_by_zero_i64_exact_line() {
    let mut platform = MockPlatform::new();

    let mut func = Function::new("div_zero_line", Type::I64);
    let mut bb0 = BasicBlock::new(0, Terminator::Ret(Some(2)));
    bb0.push(Instruction::new(Some(0), Type::I64, Op::ConstI64(100), 55));
    bb0.push(Instruction::new(Some(1), Type::I64, Op::ConstI64(0), 56));
    bb0.push(Instruction::new(Some(2), Type::I64, Op::Div(0, 1), 57));
    func.add_block(bb0);

    let err = execute(&func, vec![], &mut platform, 100, false).expect_err("Must fail on div by zero");
    assert_eq!(err, VmError::DivisionByZero { line: 57 });
}

#[test]
fn test_div_i64_min_by_neg_one_no_panic() {
    let mut platform = MockPlatform::new();

    // i64::MIN / -1 would cause overflow panic in standard signed division!
    // VM must handle this without panicking and return i64::MIN.
    let mut func = Function::new("div_overflow", Type::I64);
    let mut bb0 = BasicBlock::new(0, Terminator::Ret(Some(2)));
    bb0.push(Instruction::new(Some(0), Type::I64, Op::ConstI64(i64::MIN), 1));
    bb0.push(Instruction::new(Some(1), Type::I64, Op::ConstI64(-1), 2));
    bb0.push(Instruction::new(Some(2), Type::I64, Op::Div(0, 1), 3));
    func.add_block(bb0);

    let (res, _) = execute(&func, vec![], &mut platform, 100, false)
        .expect("Must not panic on i64::MIN / -1");
    assert_eq!(res, Value::I64(i64::MIN));
}

#[test]
fn test_div_f64_by_zero_returns_infinity_no_panic() {
    let mut platform = MockPlatform::new();

    let mut func = Function::new("f64_div_zero", Type::F64);
    let mut bb0 = BasicBlock::new(0, Terminator::Ret(Some(2)));
    bb0.push(Instruction::new(Some(0), Type::F64, Op::ConstF64(1.0), 1));
    bb0.push(Instruction::new(Some(1), Type::F64, Op::ConstF64(0.0), 2));
    bb0.push(Instruction::new(Some(2), Type::F64, Op::Div(0, 1), 3));
    func.add_block(bb0);

    let (res, _) = execute(&func, vec![], &mut platform, 100, false)
        .expect("Float div by zero produces infinity without panicking");
    match res {
        Value::F64(f) => assert!(f.is_infinite() && f.is_sign_positive()),
        other => panic!("Expected F64, got {other:?}"),
    }
}

#[test]
fn test_arithmetic_wrapping_overflow() {
    let mut platform = MockPlatform::new();

    // i64::MAX + 1 -> i64::MIN
    // i64::MIN - 1 -> i64::MAX
    // i64::MAX * 2 -> wrapping mul
    let mut func = Function::new("wrapping_arith", Type::I64);
    let mut bb0 = BasicBlock::new(0, Terminator::Ret(Some(5)));
    bb0.push(Instruction::new(Some(0), Type::I64, Op::ConstI64(i64::MAX), 1));
    bb0.push(Instruction::new(Some(1), Type::I64, Op::ConstI64(1), 2));
    bb0.push(Instruction::new(Some(2), Type::I64, Op::Add(0, 1), 3)); // i64::MIN
    bb0.push(Instruction::new(Some(3), Type::I64, Op::Sub(2, 1), 4)); // i64::MAX
    bb0.push(Instruction::new(Some(4), Type::I64, Op::ConstI64(2), 5));
    bb0.push(Instruction::new(Some(5), Type::I64, Op::Mul(0, 4), 6)); // i64::MAX.wrapping_mul(2)
    func.add_block(bb0);

    let (res, _) = execute(&func, vec![], &mut platform, 100, false)
        .expect("Must wrap without panicking");
    assert_eq!(res, Value::I64(i64::MAX.wrapping_mul(2)));
}

// ============================================================================
// 3. REGISTER & CONTROL FLOW INTEGRITY ADVERSARIAL STRESS TESTS
// ============================================================================

#[test]
fn test_invalid_register_access_instruction() {
    let mut platform = MockPlatform::new();

    let mut func = Function::new("bad_reg_inst", Type::I64);
    let mut bb0 = BasicBlock::new(0, Terminator::Ret(Some(1)));
    bb0.push(Instruction::new(Some(0), Type::I64, Op::ConstI64(10), 1));
    // Accesses %99 which was never assigned!
    bb0.push(Instruction::new(Some(1), Type::I64, Op::Add(0, 99), 2));
    func.add_block(bb0);

    let err = execute(&func, vec![], &mut platform, 100, false).expect_err("Must reject invalid register");
    assert_eq!(err, VmError::InvalidRegisterAccess { reg: 99 });
}

#[test]
fn test_free_then_access_returns_invalid_register() {
    let mut platform = MockPlatform::new();

    // Free %0 removes %0 from VM register map.
    // Subsequent access to %0 must fail with InvalidRegisterAccess!
    let mut func = Function::new("free_then_read", Type::I64);
    let mut bb0 = BasicBlock::new(0, Terminator::Ret(Some(3)));
    bb0.push(Instruction::new(Some(0), Type::I64, Op::ConstI64(42), 1));
    bb0.push(Instruction::new(None, Type::Void, Op::Free(0), 2));
    bb0.push(Instruction::new(Some(1), Type::I64, Op::ConstI64(1), 3));
    bb0.push(Instruction::new(Some(3), Type::I64, Op::Add(0, 1), 4)); // accesses freed %0!
    func.add_block(bb0);

    let err = execute(&func, vec![], &mut platform, 100, false).expect_err("Accessing freed reg must fail");
    assert_eq!(err, VmError::InvalidRegisterAccess { reg: 0 });
}

#[test]
fn test_invalid_block_jump() {
    let mut platform = MockPlatform::new();

    let mut func = Function::new("jump_missing", Type::Void);
    let bb0 = BasicBlock::new(0, Terminator::Br(404));
    func.add_block(bb0);

    let err = execute(&func, vec![], &mut platform, 100, false).expect_err("Jump to missing block must fail");
    assert_eq!(err, VmError::NoSuchBlock { block_id: 404 });
}

#[test]
fn test_br_if_type_mismatch() {
    let mut platform = MockPlatform::new();

    // Condition register is an i64 instead of bool
    let mut func = Function::new("bad_cond_type", Type::Void);
    let mut bb0 = BasicBlock::new(
        0,
        Terminator::BrIf {
            cond: 0,
            then_blk: 1,
            else_blk: 1,
        },
    );
    bb0.push(Instruction::new(Some(0), Type::I64, Op::ConstI64(1), 1));
    let bb1 = BasicBlock::new(1, Terminator::Ret(None));
    func.add_block(bb0);
    func.add_block(bb1);

    let err = execute(&func, vec![], &mut platform, 100, false).expect_err("Non-bool condition must fail");
    match err {
        VmError::TypeMismatch { expected, actual, .. } => {
            assert_eq!(expected, "bool");
            assert_eq!(actual, "i64");
        }
        other => panic!("Expected TypeMismatch, got {other:?}"),
    }
}

// ============================================================================
// 4. BUFFER, CONTAINER & MEMORY BOUNDS ADVERSARIAL STRESS TESTS
// ============================================================================

#[test]
fn test_buf_alloc_negative_size() {
    let mut platform = MockPlatform::new();

    let mut func = Function::new("neg_buf", Type::Buf);
    let mut bb0 = BasicBlock::new(0, Terminator::Ret(Some(1)));
    bb0.push(Instruction::new(Some(0), Type::I64, Op::ConstI64(-10), 1));
    bb0.push(Instruction::new(Some(1), Type::Buf, Op::BufAlloc(0), 2));
    func.add_block(bb0);

    let err = execute(&func, vec![], &mut platform, 100, false).expect_err("Negative buf size must fail");
    match err {
        VmError::InvalidInstruction { message, line } => {
            assert!(message.contains("negative size"));
            assert_eq!(line, 2);
        }
        other => panic!("Expected InvalidInstruction, got {other:?}"),
    }
}

#[test]
fn test_buf_read_and_write_out_of_bounds() {
    let mut platform = MockPlatform::new();

    // Buffer size = 4 bytes. Access at index 4 (>= len 4).
    let mut func = Function::new("buf_oob", Type::I64);
    let mut bb0 = BasicBlock::new(0, Terminator::Ret(Some(3)));
    bb0.push(Instruction::new(Some(0), Type::I64, Op::ConstI64(4), 1));
    bb0.push(Instruction::new(Some(1), Type::Buf, Op::BufAlloc(0), 2));
    bb0.push(Instruction::new(Some(2), Type::I64, Op::ConstI64(4), 3));
    bb0.push(Instruction::new(Some(3), Type::I64, Op::BufReadU8(1, 2), 4));
    func.add_block(bb0);

    let err = execute(&func, vec![], &mut platform, 100, false).expect_err("Buf read OOB must fail");
    assert_eq!(
        err,
        VmError::IndexOutOfBounds {
            index: 4,
            len: 4,
            line: 4,
        }
    );
}

#[test]
fn test_buf_slice_overflow_causes_unhandled_panic() {
    let mut platform = MockPlatform::new();

    // Buffer size = 16 bytes.
    // start: i64::MAX - 5, len: 10
    // Because start + len overflows i64, line 251 in vm.rs:
    // `index: (start + len) as usize`
    // panics with "attempt to add with overflow" in debug mode instead of returning VmError::IndexOutOfBounds!
    let mut func = Function::new("buf_slice_overflow", Type::Buf);
    let mut bb0 = BasicBlock::new(0, Terminator::Ret(Some(4)));
    bb0.push(Instruction::new(Some(0), Type::I64, Op::ConstI64(16), 1));
    bb0.push(Instruction::new(Some(1), Type::Buf, Op::BufAlloc(0), 2));
    bb0.push(Instruction::new(Some(2), Type::I64, Op::ConstI64(i64::MAX - 5), 3));
    bb0.push(Instruction::new(Some(3), Type::I64, Op::ConstI64(10), 4));
    bb0.push(Instruction::new(Some(4), Type::Buf, Op::BufSlice(1, 2, 3), 5));
    func.add_block(bb0);

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        execute(&func, vec![], &mut platform, 100, false)
    }));

    // Remediation verified: VM does not crash via unhandled panic; returns IndexOutOfBounds
    assert!(result.is_ok(), "Must not panic on integer overflow");
    let exec_res = result.unwrap();
    assert!(
        matches!(exec_res, Err(VmError::IndexOutOfBounds { .. })),
        "Expected IndexOutOfBounds error, got {exec_res:?}"
    );
}

#[test]
fn test_vec_get_negative_and_oob() {
    let mut platform = MockPlatform::new();

    let mut func = Function::new("vec_oob", Type::I64);
    let mut bb0 = BasicBlock::new(0, Terminator::Ret(Some(2)));
    bb0.push(Instruction::new(Some(0), Type::Vec(Box::new(Type::I64)), Op::VecNew(Type::I64), 1));
    bb0.push(Instruction::new(Some(1), Type::I64, Op::ConstI64(-1), 2));
    bb0.push(Instruction::new(Some(2), Type::I64, Op::VecGet(0, 1), 3));
    func.add_block(bb0);

    let err = execute(&func, vec![], &mut platform, 100, false).expect_err("Negative vec index must fail");
    match err {
        VmError::IndexOutOfBounds { len, line, .. } => {
            assert_eq!(len, 0);
            assert_eq!(line, 3);
        }
        other => panic!("Expected IndexOutOfBounds, got {other:?}"),
    }
}

#[test]
fn test_chan_fifo_and_recv_empty() {
    let mut platform = MockPlatform::new();

    // 1. chan_recv on empty channel returns Value::Void
    let mut func1 = Function::new("chan_recv_empty", Type::Void);
    let mut bb0 = BasicBlock::new(0, Terminator::Ret(Some(2)));
    bb0.push(Instruction::new(Some(0), Type::I64, Op::ConstI64(10), 1));
    bb0.push(Instruction::new(Some(1), Type::Chan(Box::new(Type::I64)), Op::ChanNew(Type::I64, 0), 2));
    bb0.push(Instruction::new(Some(2), Type::Void, Op::ChanRecv(1), 3));
    func1.add_block(bb0);

    let (res1, _) = execute(&func1, vec![], &mut platform, 100, false).expect("Recv on empty chan succeeds");
    assert_eq!(res1, Value::Void);

    // 2. FIFO order verification
    let mut func2 = Function::new("chan_fifo", Type::I64);
    let mut bb_fifo = BasicBlock::new(0, Terminator::Ret(Some(7)));
    bb_fifo.push(Instruction::new(Some(0), Type::I64, Op::ConstI64(10), 1));
    bb_fifo.push(Instruction::new(Some(1), Type::Chan(Box::new(Type::I64)), Op::ChanNew(Type::I64, 0), 2));
    bb_fifo.push(Instruction::new(Some(2), Type::I64, Op::ConstI64(111), 3));
    bb_fifo.push(Instruction::new(Some(3), Type::I64, Op::ConstI64(222), 4));
    bb_fifo.push(Instruction::new(Some(4), Type::Bool, Op::ChanSend(1, 2), 5));
    bb_fifo.push(Instruction::new(Some(5), Type::Bool, Op::ChanSend(1, 3), 6));
    bb_fifo.push(Instruction::new(Some(6), Type::I64, Op::ChanRecv(1), 7)); // gets 111
    bb_fifo.push(Instruction::new(Some(7), Type::I64, Op::ChanRecv(1), 8)); // gets 222
    func2.add_block(bb_fifo);

    let (res2, _) = execute(&func2, vec![], &mut platform, 100, false).expect("FIFO succeeds");
    assert_eq!(res2, Value::I64(222));
}

// ============================================================================
// 5. TRACE RECORDS & JSON SERIALIZATION ADVERSARIAL STRESS TESTS
// ============================================================================

#[test]
fn test_trace_sequential_steps_and_json_validity() {
    let mut platform = MockPlatform::new();

    // Multi-block function with branching to exercise all trace record kinds
    let mut func = Function::new("traced_flow", Type::I64);
    func.add_param(0, Type::Bool);

    let mut bb0 = BasicBlock::new(
        0,
        Terminator::BrIf {
            cond: 0,
            then_blk: 1,
            else_blk: 2,
        },
    );
    bb0.push(Instruction::new(Some(1), Type::I64, Op::ConstI64(10), 10));

    let mut bb1 = BasicBlock::new(1, Terminator::Br(3));
    bb1.push(Instruction::new(Some(2), Type::I64, Op::ConstI64(20), 20));

    let mut bb2 = BasicBlock::new(2, Terminator::Br(3));
    bb2.push(Instruction::new(Some(3), Type::I64, Op::ConstI64(30), 30));

    let mut bb3 = BasicBlock::new(3, Terminator::Ret(Some(4)));
    bb3.push(Instruction::new(Some(4), Type::I64, Op::ConstI64(100), 40));

    func.add_block(bb0);
    func.add_block(bb1);
    func.add_block(bb2);
    func.add_block(bb3);

    let (val, traces) = execute(&func, vec![Value::Bool(true)], &mut platform, 100, true)
        .expect("Traced execution succeeds");
    assert_eq!(val, Value::I64(100));

    // Steps should be:
    // 1. bb0: inst %1 = const_i64 10 (line 10)
    // 2. bb0: term br_if (line 11)
    // 3. bb1: inst %2 = const_i64 20 (line 20)
    // 4. bb1: term br (line 21)
    // 5. bb3: inst %4 = const_i64 100 (line 40)
    // 6. bb3: term ret (line 41)
    assert_eq!(traces.len(), 6);

    for (idx, trace) in traces.iter().enumerate() {
        assert_eq!(trace.step, (idx + 1) as u64, "Trace step must be strictly sequential starting at 1");
    }

    assert_eq!(traces[0].opcode, "const_i64");
    assert_eq!(traces[0].dst_reg, Some("%1".to_string()));
    assert_eq!(traces[0].new_value, Some(serde_json::json!(10)));

    assert_eq!(traces[1].opcode, "br_if");
    assert_eq!(traces[1].dst_reg, None);
    assert_eq!(traces[1].new_value, Some(serde_json::json!(true)));

    assert_eq!(traces[2].opcode, "const_i64");
    assert_eq!(traces[2].dst_reg, Some("%2".to_string()));

    assert_eq!(traces[3].opcode, "br");
    assert_eq!(traces[3].dst_reg, None);
    assert_eq!(traces[3].new_value, None);

    assert_eq!(traces[4].opcode, "const_i64");
    assert_eq!(traces[4].dst_reg, Some("%4".to_string()));

    assert_eq!(traces[5].opcode, "ret");
    assert_eq!(traces[5].dst_reg, None);
    assert_eq!(traces[5].new_value, Some(serde_json::json!(100)));

    // Serialize to JSON string and verify roundtrip
    let json_str = serde_json::to_string(&traces).unwrap();
    let deserialized: Vec<TraceRecord> = serde_json::from_str(&json_str).unwrap();
    assert_eq!(traces, deserialized);
}
