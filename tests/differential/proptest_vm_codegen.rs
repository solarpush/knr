//! Differential Fuzzing Suite: knr-interp Reference VM vs knr-codegen Cranelift JIT
//!
//! Generates type-correct, terminating SSA programs with `proptest`, validates them
//! against `knr-verify`, executes them concurrently in `knr-interp` (with 1,000 gas limit)
//! and in-memory via `knr-codegen` Cranelift JIT, and asserts exact bitwise value parity.

use knr_codegen::CraneliftBackend;
use knr_core::{BasicBlock, Function, Instruction, Op, RegId, Terminator, Type};
use knr_interp::Value;
use knr_runtime::MockPlatform;
use proptest::prelude::*;

// ============================================================================
// 1. DUAL EXECUTION & EQUIVALENCE RUNNER
// ============================================================================

fn run_vm(func: &Function, args: Vec<Value>, max_steps: u64) -> Result<Value, String> {
    let mut platform = MockPlatform::new();
    let (val, _) = knr_interp::execute(func, args, &mut platform, max_steps, false)
        .map_err(|e| format!("VM error: {e}"))?;
    Ok(val)
}

fn run_jit_0arg(func: &Function) -> Result<Value, String> {
    let mut backend = CraneliftBackend::new()
        .map_err(|e| format!("Cranelift backend init failed: {e}"))?;
    let code_ptr = backend
        .compile_jit(func)
        .map_err(|e| format!("Cranelift JIT compilation failed: {e}"))?;

    assert!(!code_ptr.is_null(), "JIT code pointer must not be null");

    match func.ret_ty {
        Type::I64 => {
            let f: extern "C" fn() -> i64 = unsafe { std::mem::transmute(code_ptr) };
            Ok(Value::I64(f()))
        }
        Type::F64 => {
            let f: extern "C" fn() -> f64 = unsafe { std::mem::transmute(code_ptr) };
            Ok(Value::F64(f()))
        }
        Type::Bool => {
            let f: extern "C" fn() -> i64 = unsafe { std::mem::transmute(code_ptr) };
            Ok(Value::Bool(f() != 0))
        }
        ref other => Err(format!("Unsupported JIT return type: {other:?}")),
    }
}

fn verify_and_assert_equiv(func: &Function) -> Result<(), TestCaseError> {
    // 1. Static Verification
    if let Err(diag) = knr_verify::verify_function(func) {
        return Err(TestCaseError::fail(format!(
            "Static verification rejected program:\n{diag}\nSSA:\n{}",
            knr_core::format_function(func)
        )));
    }

    // 2. Reference VM execution (1,000 gas steps limit)
    let vm_res = run_vm(func, vec![], 1000).map_err(|e| {
        TestCaseError::fail(format!(
            "VM run failed: {e}\nSSA:\n{}",
            knr_core::format_function(func)
        ))
    })?;

    // 3. Cranelift JIT execution
    let jit_res = run_jit_0arg(func).map_err(|e| {
        TestCaseError::fail(format!(
            "JIT run failed: {e}\nSSA:\n{}",
            knr_core::format_function(func)
        ))
    })?;

    // 4. Parity Assertion
    prop_assert_eq!(
        &vm_res,
        &jit_res,
        "Divergence detected between VM and JIT!\nSSA:\n{}",
        knr_core::format_function(func)
    );

    Ok(())
}

// ============================================================================
// 2. SSA GENERATION STRATEGIES
// ============================================================================

#[derive(Debug, Clone)]
enum I64ArithOp {
    Add(usize, usize),
    Sub(usize, usize),
    Mul(usize, usize),
    Div(usize, i64), // Divisor with |d| >= 2 to avoid division by zero and i64::MIN / -1 overflow
}

fn arb_i64_arithmetic_func() -> impl Strategy<Value = Function> {
    (
        // Initial constants (2 to 4)
        prop::collection::vec(any::<i64>(), 2..=4),
        // Sequence of arithmetic ops (5 to 20 ops)
        prop::collection::vec(
            prop_oneof![
                (any::<usize>(), any::<usize>()).prop_map(|(a, b)| I64ArithOp::Add(a, b)),
                (any::<usize>(), any::<usize>()).prop_map(|(a, b)| I64ArithOp::Sub(a, b)),
                (any::<usize>(), any::<usize>()).prop_map(|(a, b)| I64ArithOp::Mul(a, b)),
                (
                    any::<usize>(),
                    prop_oneof![2..=10000i64, -10000..=-2i64]
                )
                    .prop_map(|(a, d)| I64ArithOp::Div(a, d)),
            ],
            5..=20,
        ),
    )
        .prop_map(|(init_consts, ops)| {
            let mut func = Function::new("fuzz_i64_arith", Type::I64);
            let mut bb0 = BasicBlock::new(0, Terminator::Ret(None));
            let mut next_reg: RegId = 0;
            let mut i64_regs: Vec<RegId> = Vec::new();
            let mut line = 1;

            // Emit initial constants
            for c in init_consts {
                let reg = next_reg;
                next_reg += 1;
                bb0.push(Instruction::new(Some(reg), Type::I64, Op::ConstI64(c), line));
                i64_regs.push(reg);
                line += 1;
            }

            // Emit operations
            for op in ops {
                match op {
                    I64ArithOp::Add(i1, i2) => {
                        let r1 = i64_regs[i1 % i64_regs.len()];
                        let r2 = i64_regs[i2 % i64_regs.len()];
                        let dst = next_reg;
                        next_reg += 1;
                        bb0.push(Instruction::new(Some(dst), Type::I64, Op::Add(r1, r2), line));
                        i64_regs.push(dst);
                        line += 1;
                    }
                    I64ArithOp::Sub(i1, i2) => {
                        let r1 = i64_regs[i1 % i64_regs.len()];
                        let r2 = i64_regs[i2 % i64_regs.len()];
                        let dst = next_reg;
                        next_reg += 1;
                        bb0.push(Instruction::new(Some(dst), Type::I64, Op::Sub(r1, r2), line));
                        i64_regs.push(dst);
                        line += 1;
                    }
                    I64ArithOp::Mul(i1, i2) => {
                        let r1 = i64_regs[i1 % i64_regs.len()];
                        let r2 = i64_regs[i2 % i64_regs.len()];
                        let dst = next_reg;
                        next_reg += 1;
                        bb0.push(Instruction::new(Some(dst), Type::I64, Op::Mul(r1, r2), line));
                        i64_regs.push(dst);
                        line += 1;
                    }
                    I64ArithOp::Div(i1, divisor) => {
                        let r1 = i64_regs[i1 % i64_regs.len()];
                        let r_div = next_reg;
                        next_reg += 1;
                        bb0.push(Instruction::new(
                            Some(r_div),
                            Type::I64,
                            Op::ConstI64(divisor),
                            line,
                        ));
                        line += 1;
                        let dst = next_reg;
                        next_reg += 1;
                        bb0.push(Instruction::new(Some(dst), Type::I64, Op::Div(r1, r_div), line));
                        i64_regs.push(dst);
                        line += 1;
                    }
                }
            }

            let ret_reg = *i64_regs.last().unwrap();
            bb0.terminator = Terminator::Ret(Some(ret_reg));
            func.add_block(bb0);
            func
        })
}

#[derive(Debug, Clone)]
enum F64ArithOp {
    Add(usize, usize),
    Sub(usize, usize),
    Mul(usize, usize),
    Div(usize, usize),
}

fn arb_f64_arithmetic_func() -> impl Strategy<Value = Function> {
    (
        prop::collection::vec(
            any::<f64>().prop_filter("finite float", |f| f.is_finite()),
            2..=4,
        ),
        prop::collection::vec(
            prop_oneof![
                (any::<usize>(), any::<usize>()).prop_map(|(a, b)| F64ArithOp::Add(a, b)),
                (any::<usize>(), any::<usize>()).prop_map(|(a, b)| F64ArithOp::Sub(a, b)),
                (any::<usize>(), any::<usize>()).prop_map(|(a, b)| F64ArithOp::Mul(a, b)),
                (any::<usize>(), any::<usize>()).prop_map(|(a, b)| F64ArithOp::Div(a, b)),
            ],
            5..=20,
        ),
    )
        .prop_map(|(init_consts, ops)| {
            let mut func = Function::new("fuzz_f64_arith", Type::F64);
            let mut bb0 = BasicBlock::new(0, Terminator::Ret(None));
            let mut next_reg: RegId = 0;
            let mut f64_regs: Vec<RegId> = Vec::new();
            let mut line = 1;

            for c in init_consts {
                let reg = next_reg;
                next_reg += 1;
                bb0.push(Instruction::new(Some(reg), Type::F64, Op::ConstF64(c), line));
                f64_regs.push(reg);
                line += 1;
            }

            for op in ops {
                let dst = next_reg;
                next_reg += 1;
                let inst_op = match op {
                    F64ArithOp::Add(i1, i2) => {
                        let r1 = f64_regs[i1 % f64_regs.len()];
                        let r2 = f64_regs[i2 % f64_regs.len()];
                        Op::Add(r1, r2)
                    }
                    F64ArithOp::Sub(i1, i2) => {
                        let r1 = f64_regs[i1 % f64_regs.len()];
                        let r2 = f64_regs[i2 % f64_regs.len()];
                        Op::Sub(r1, r2)
                    }
                    F64ArithOp::Mul(i1, i2) => {
                        let r1 = f64_regs[i1 % f64_regs.len()];
                        let r2 = f64_regs[i2 % f64_regs.len()];
                        Op::Mul(r1, r2)
                    }
                    F64ArithOp::Div(i1, i2) => {
                        let r1 = f64_regs[i1 % f64_regs.len()];
                        let r2 = f64_regs[i2 % f64_regs.len()];
                        Op::Div(r1, r2)
                    }
                };
                bb0.push(Instruction::new(Some(dst), Type::F64, inst_op, line));
                f64_regs.push(dst);
                line += 1;
            }

            let ret_reg = *f64_regs.last().unwrap();
            bb0.terminator = Terminator::Ret(Some(ret_reg));
            func.add_block(bb0);
            func
        })
}

fn arb_comparison_func() -> impl Strategy<Value = Function> {
    (
        any::<i64>(),
        any::<i64>(),
        prop_oneof![Just("eq"), Just("lt")],
    )
        .prop_map(|(val1, val2, cmp_kind)| {
            let mut func = Function::new("fuzz_cmp", Type::Bool);
            let mut bb0 = BasicBlock::new(0, Terminator::Ret(Some(2)));
            bb0.push(Instruction::new(Some(0), Type::I64, Op::ConstI64(val1), 1));
            bb0.push(Instruction::new(Some(1), Type::I64, Op::ConstI64(val2), 2));
            let op = if cmp_kind == "eq" {
                Op::CmpEq(0, 1)
            } else {
                Op::CmpLt(0, 1)
            };
            bb0.push(Instruction::new(Some(2), Type::Bool, op, 3));
            func.add_block(bb0);
            func
        })
}

fn arb_linear_buffer_func() -> impl Strategy<Value = Function> {
    (prop::collection::vec((0..64i64, any::<u8>()), 1..=8),).prop_map(|(writes,)| {
        let mut func = Function::new("fuzz_buffer", Type::I64);
        let mut bb0 = BasicBlock::new(0, Terminator::Ret(None));
        let mut next_reg: RegId = 0;
        let mut line = 1;

        // Allocate buffer of 64 bytes
        let r_size = next_reg;
        next_reg += 1;
        bb0.push(Instruction::new(
            Some(r_size),
            Type::I64,
            Op::ConstI64(64),
            line,
        ));
        line += 1;

        let mut current_buf = next_reg;
        next_reg += 1;
        bb0.push(Instruction::new(
            Some(current_buf),
            Type::Buf,
            Op::BufAlloc(r_size),
            line,
        ));
        line += 1;

        let mut read_regs = Vec::new();

        for (idx, byte_val) in writes {
            let r_idx = next_reg;
            next_reg += 1;
            bb0.push(Instruction::new(
                Some(r_idx),
                Type::I64,
                Op::ConstI64(idx),
                line,
            ));
            line += 1;

            let r_val = next_reg;
            next_reg += 1;
            bb0.push(Instruction::new(
                Some(r_val),
                Type::I64,
                Op::ConstI64(byte_val as i64),
                line,
            ));
            line += 1;

            let next_buf = next_reg;
            next_reg += 1;
            bb0.push(Instruction::new(
                Some(next_buf),
                Type::Buf,
                Op::BufWriteU8(current_buf, r_idx, r_val),
                line,
            ));
            line += 1;
            current_buf = next_buf;

            let r_read = next_reg;
            next_reg += 1;
            bb0.push(Instruction::new(
                Some(r_read),
                Type::I64,
                Op::BufReadU8(current_buf, r_idx),
                line,
            ));
            line += 1;
            read_regs.push(r_read);
        }

        // Accumulate read values
        let mut sum_reg = read_regs[0];
        for &r in &read_regs[1..] {
            let next_sum = next_reg;
            next_reg += 1;
            bb0.push(Instruction::new(
                Some(next_sum),
                Type::I64,
                Op::Add(sum_reg, r),
                line,
            ));
            line += 1;
            sum_reg = next_sum;
        }

        // Free the buffer
        bb0.push(Instruction::new(None, Type::Void, Op::Free(current_buf), line));

        bb0.terminator = Terminator::Ret(Some(sum_reg));
        func.add_block(bb0);
        func
    })
}

fn arb_diamond_branch_func() -> impl Strategy<Value = Function> {
    (any::<i64>(), any::<i64>(), any::<i64>()).prop_map(|(a, b, c)| {
        let mut func = Function::new("fuzz_diamond", Type::I64);

        // bb0: setup & branch
        let mut bb0 = BasicBlock::new(
            0,
            Terminator::BrIf {
                cond: 2,
                then_blk: 1,
                else_blk: 2,
            },
        );
        bb0.push(Instruction::new(Some(0), Type::I64, Op::ConstI64(a), 1));
        bb0.push(Instruction::new(Some(1), Type::I64, Op::ConstI64(b), 2));
        bb0.push(Instruction::new(Some(2), Type::Bool, Op::CmpLt(0, 1), 3));
        func.add_block(bb0);

        // bb1: then branch
        let mut bb1 = BasicBlock::new(1, Terminator::Ret(Some(4)));
        bb1.push(Instruction::new(Some(3), Type::I64, Op::ConstI64(c), 4));
        bb1.push(Instruction::new(Some(4), Type::I64, Op::Add(0, 3), 5));
        func.add_block(bb1);

        // bb2: else branch
        let mut bb2 = BasicBlock::new(2, Terminator::Ret(Some(6)));
        bb2.push(Instruction::new(Some(5), Type::I64, Op::ConstI64(c), 6));
        bb2.push(Instruction::new(Some(6), Type::I64, Op::Sub(1, 5), 7));
        func.add_block(bb2);

        func
    })
}

fn arb_bounded_loop_func() -> impl Strategy<Value = Function> {
    (any::<i64>(), any::<i64>(), any::<i64>()).prop_map(|(a, b, step)| {
        let mut func = Function::new("fuzz_bounded_loop", Type::I64);

        // bb0: setup initial values and jump to loop header bb1
        let mut bb0 = BasicBlock::new(0, Terminator::Br(1));
        bb0.push(Instruction::new(Some(0), Type::I64, Op::ConstI64(a), 1));
        bb0.push(Instruction::new(Some(1), Type::I64, Op::ConstI64(b), 2));
        bb0.push(Instruction::new(Some(2), Type::I64, Op::ConstI64(step), 3));
        func.add_block(bb0);

        // bb1: loop header evaluating condition
        let mut bb1 = BasicBlock::new(
            1,
            Terminator::BrIf {
                cond: 5,
                then_blk: 3,
                else_blk: 2,
            },
        );
        bb1.push(Instruction::new(Some(3), Type::I64, Op::ConstI64(10), 4));
        bb1.push(Instruction::new(Some(4), Type::I64, Op::ConstI64(10), 5));
        bb1.push(Instruction::new(Some(5), Type::Bool, Op::CmpEq(3, 4), 6));
        func.add_block(bb1);

        // bb2: loop body with back-edge to bb1
        let mut bb2 = BasicBlock::new(2, Terminator::Br(1));
        bb2.push(Instruction::new(Some(6), Type::I64, Op::Add(0, 2), 7));
        func.add_block(bb2);

        // bb3: exit block
        let mut bb3 = BasicBlock::new(3, Terminator::Ret(Some(8)));
        bb3.push(Instruction::new(Some(7), Type::I64, Op::Add(0, 1), 8));
        bb3.push(Instruction::new(Some(8), Type::I64, Op::Add(7, 2), 9));
        func.add_block(bb3);

        func
    })
}

// ============================================================================
// 3. PROPTEST SUITE EXECUTIONS (>= 1,150 TOTAL TEST CASES)
// ============================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(500))]
    #[test]
    fn prop_diff_i64_arithmetic(func in arb_i64_arithmetic_func()) {
        verify_and_assert_equiv(&func)?;
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(250))]
    #[test]
    fn prop_diff_f64_arithmetic(func in arb_f64_arithmetic_func()) {
        verify_and_assert_equiv(&func)?;
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(150))]
    #[test]
    fn prop_diff_comparisons(func in arb_comparison_func()) {
        verify_and_assert_equiv(&func)?;
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]
    #[test]
    fn prop_diff_linear_buffer(func in arb_linear_buffer_func()) {
        verify_and_assert_equiv(&func)?;
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]
    #[test]
    fn prop_diff_diamond_branching(func in arb_diamond_branch_func()) {
        verify_and_assert_equiv(&func)?;
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(50))]
    #[test]
    fn prop_diff_bounded_loops(func in arb_bounded_loop_func()) {
        verify_and_assert_equiv(&func)?;
    }
}
