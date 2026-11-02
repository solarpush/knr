use knr_core::{BasicBlock, Function, Instruction, Op, Terminator, Type};
use knr_interp::{execute, Value, VmError};
use knr_runtime::MockPlatform;

#[test]
fn test_exhaustive_buf_slice_matrix() {
    let mut platform = MockPlatform::new();

    let test_buffers = vec![
        vec![],
        vec![42u8],
        vec![10, 20, 30, 40, 50, 60, 70, 80, 90, 100],
    ];

    let test_starts: Vec<i64> = vec![
        i64::MIN,
        i64::MIN + 1,
        -1_000_000,
        -1,
        0,
        1,
        5,
        10,
        11,
        1_000_000,
        i64::MAX / 2,
        i64::MAX - 100,
        i64::MAX - 1,
        i64::MAX,
    ];

    let test_lens: Vec<i64> = vec![
        i64::MIN,
        i64::MIN + 1,
        -1_000_000,
        -1,
        0,
        1,
        5,
        10,
        11,
        1_000_000,
        i64::MAX / 2,
        i64::MAX - 100,
        i64::MAX - 1,
        i64::MAX,
    ];

    for raw_buf in test_buffers {
        let buf_size = raw_buf.len();

        for &start in &test_starts {
            for &len in &test_lens {
                // Build a function that creates buffer, writes data, slices it
                let mut func = Function::new("test_slice_comb", Type::Buf);
                let mut bb0 = BasicBlock::new(0, Terminator::Ret(Some(4)));
                bb0.push(Instruction::new(Some(0), Type::I64, Op::ConstI64(buf_size as i64), 1));
                bb0.push(Instruction::new(Some(1), Type::Buf, Op::BufAlloc(0), 2));

                // Populate buffer if non-empty
                let mut last_reg = 1;
                let mut cur_line = 3;
                for (i, &byte) in raw_buf.iter().enumerate() {
                    let next_reg = last_reg + 3;
                    bb0.push(Instruction::new(Some(last_reg + 1), Type::I64, Op::ConstI64(i as i64), cur_line));
                    bb0.push(Instruction::new(Some(last_reg + 2), Type::I64, Op::ConstI64(byte as i64), cur_line + 1));
                    bb0.push(Instruction::new(Some(next_reg), Type::Buf, Op::BufWriteU8(last_reg, last_reg + 1, last_reg + 2), cur_line + 2));
                    last_reg = next_reg;
                    cur_line += 3;
                }

                let start_reg = last_reg + 1;
                let len_reg = last_reg + 2;
                let slice_reg = last_reg + 3;

                bb0.push(Instruction::new(Some(start_reg), Type::I64, Op::ConstI64(start), cur_line));
                bb0.push(Instruction::new(Some(len_reg), Type::I64, Op::ConstI64(len), cur_line + 1));
                bb0.push(Instruction::new(Some(slice_reg), Type::Buf, Op::BufSlice(last_reg, start_reg, len_reg), cur_line + 2));
                bb0.terminator = Terminator::Ret(Some(slice_reg));
                func.add_block(bb0);

                let unwind_res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    execute(&func, vec![], &mut platform, 200, false)
                }));

                assert!(
                    unwind_res.is_ok(),
                    "PANIC DETECTED on buf.len={buf_size}, start={start}, len={len}"
                );

                let exec_res = unwind_res.unwrap();

                // Compute expected oracle outcome
                let is_valid = if start < 0 || len < 0 {
                    false
                } else if let Some(end) = start.checked_add(len) {
                    (start as usize) <= buf_size && (end as usize) <= buf_size
                } else {
                    false
                };

                if is_valid {
                    let end = (start + len) as usize;
                    let expected_slice = &raw_buf[(start as usize)..end];
                    match exec_res {
                        Ok((Value::Buf(b), _)) => {
                            assert_eq!(
                                b.as_slice(),
                                expected_slice,
                                "Slice content mismatch for buf.len={buf_size}, start={start}, len={len}"
                            );
                        }
                        other => panic!("Expected Ok(Value::Buf), got {other:?} for buf.len={buf_size}, start={start}, len={len}"),
                    }
                } else {
                    match exec_res {
                        Err(VmError::IndexOutOfBounds { .. }) => {
                            // Expected
                        }
                        other => panic!("Expected Err(IndexOutOfBounds), got {other:?} for buf.len={buf_size}, start={start}, len={len}"),
                    }
                }
            }
        }
    }
}

#[test]
fn test_buf_read_write_extremes_no_panic() {
    let mut platform = MockPlatform::new();

    let indices: Vec<i64> = vec![
        i64::MIN,
        i64::MIN + 1,
        -10,
        -1,
        0,
        4,
        5,
        10,
        11,
        i64::MAX - 1,
        i64::MAX,
    ];

    for &idx in &indices {
        // Test read
        let mut func = Function::new("test_read_extreme", Type::I64);
        let mut bb0 = BasicBlock::new(0, Terminator::Ret(Some(3)));
        bb0.push(Instruction::new(Some(0), Type::I64, Op::ConstI64(5), 1));
        bb0.push(Instruction::new(Some(1), Type::Buf, Op::BufAlloc(0), 2));
        bb0.push(Instruction::new(Some(2), Type::I64, Op::ConstI64(idx), 3));
        bb0.push(Instruction::new(Some(3), Type::I64, Op::BufReadU8(1, 2), 4));
        func.add_block(bb0);

        let unwind_res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            execute(&func, vec![], &mut platform, 100, false)
        }));
        assert!(unwind_res.is_ok(), "Panic on read at idx={idx}");

        let res = unwind_res.unwrap();
        if (0..5).contains(&idx) {
            assert!(matches!(res, Ok((Value::I64(0), _))));
        } else {
            assert!(matches!(res, Err(VmError::IndexOutOfBounds { .. })));
        }

        // Test write
        let mut func2 = Function::new("test_write_extreme", Type::Buf);
        let mut bb0_w = BasicBlock::new(0, Terminator::Ret(Some(4)));
        bb0_w.push(Instruction::new(Some(0), Type::I64, Op::ConstI64(5), 1));
        bb0_w.push(Instruction::new(Some(1), Type::Buf, Op::BufAlloc(0), 2));
        bb0_w.push(Instruction::new(Some(2), Type::I64, Op::ConstI64(idx), 3));
        bb0_w.push(Instruction::new(Some(3), Type::I64, Op::ConstI64(255), 4));
        bb0_w.push(Instruction::new(Some(4), Type::Buf, Op::BufWriteU8(1, 2, 3), 5));
        func2.add_block(bb0_w);

        let unwind_res_w = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            execute(&func2, vec![], &mut platform, 100, false)
        }));
        assert!(unwind_res_w.is_ok(), "Panic on write at idx={idx}");

        let res_w = unwind_res_w.unwrap();
        if (0..5).contains(&idx) {
            assert!(matches!(res_w, Ok((Value::Buf(_), _))));
        } else {
            assert!(matches!(res_w, Err(VmError::IndexOutOfBounds { .. })));
        }
    }
}
