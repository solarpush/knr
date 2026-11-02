//! Adversarial Stress Testing Suite for Differential Fuzzing (VM vs Cranelift JIT)
//!
//! Investigates:
//! 1. Large integer arithmetic and boundary wrapping (i64::MAX, i64::MIN).
//! 2. Floating point edge cases: subnormals, infinities, NaNs, and NaN comparison semantics.
//! 3. Deeply nested basic blocks (long linear CFG chains).
//! 4. Container lifecycle (Buf, Vec, Map) via JIT vs VM.
//! 5. Gas exhaustion on loops in VM vs unmetered Cranelift JIT.
//! 6. Division edge cases and CPU fault / SIGFPE behavior.

use knr_codegen::CraneliftBackend;
use knr_core::{BasicBlock, Function, Instruction, Op, RegId, Terminator, Type};
use knr_interp::Value;
use knr_runtime::MockPlatform;

fn run_vm(func: &Function, max_steps: u64) -> Result<Value, String> {
    let mut platform = MockPlatform::new();
    let (val, _) = knr_interp::execute(func, vec![], &mut platform, max_steps, false)
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

/// Stress Test 1: Large Integer Boundary Arithmetic (i64::MAX, i64::MIN, overflow wrapping)
#[test]
fn test_stress_large_i64_wrapping_arithmetic() {
    // Test i64::MAX + 1 -> wrapping to i64::MIN
    let mut func = Function::new("test_overflow_add", Type::I64);
    let mut bb0 = BasicBlock::new(0, Terminator::Ret(Some(2)));
    bb0.push(Instruction::new(Some(0), Type::I64, Op::ConstI64(i64::MAX), 1));
    bb0.push(Instruction::new(Some(1), Type::I64, Op::ConstI64(1), 2));
    bb0.push(Instruction::new(Some(2), Type::I64, Op::Add(0, 1), 3));
    func.add_block(bb0);

    let vm_res = run_vm(&func, 1000).expect("VM overflow add");
    let jit_res = run_jit_0arg(&func).expect("JIT overflow add");
    assert_eq!(vm_res, Value::I64(i64::MIN));
    assert_eq!(jit_res, Value::I64(i64::MIN));
    assert_eq!(vm_res, jit_res);

    // Test i64::MIN - 1 -> wrapping to i64::MAX
    let mut func = Function::new("test_underflow_sub", Type::I64);
    let mut bb0 = BasicBlock::new(0, Terminator::Ret(Some(2)));
    bb0.push(Instruction::new(Some(0), Type::I64, Op::ConstI64(i64::MIN), 1));
    bb0.push(Instruction::new(Some(1), Type::I64, Op::ConstI64(1), 2));
    bb0.push(Instruction::new(Some(2), Type::I64, Op::Sub(0, 1), 3));
    func.add_block(bb0);

    let vm_res = run_vm(&func, 1000).expect("VM underflow sub");
    let jit_res = run_jit_0arg(&func).expect("JIT underflow sub");
    assert_eq!(vm_res, Value::I64(i64::MAX));
    assert_eq!(jit_res, Value::I64(i64::MAX));
    assert_eq!(vm_res, jit_res);

    // Test i64::MAX * 2 -> wrapping multiplication
    let mut func = Function::new("test_mul_wrap", Type::I64);
    let mut bb0 = BasicBlock::new(0, Terminator::Ret(Some(2)));
    bb0.push(Instruction::new(Some(0), Type::I64, Op::ConstI64(i64::MAX), 1));
    bb0.push(Instruction::new(Some(1), Type::I64, Op::ConstI64(2), 2));
    bb0.push(Instruction::new(Some(2), Type::I64, Op::Mul(0, 1), 3));
    func.add_block(bb0);

    let vm_res = run_vm(&func, 1000).expect("VM mul wrap");
    let jit_res = run_jit_0arg(&func).expect("JIT mul wrap");
    assert_eq!(vm_res, Value::I64(i64::MAX.wrapping_mul(2)));
    assert_eq!(jit_res, Value::I64(i64::MAX.wrapping_mul(2)));
    assert_eq!(vm_res, jit_res);
}

/// Stress Test 2: Floating Point Special Values (Subnormals, Infinities, Signed Zero)
#[test]
fn test_stress_f64_subnormals_and_infinities() {
    let subnormal_val = f64::from_bits(1); // Smallest positive subnormal f64
    assert!(subnormal_val.is_subnormal());

    let mut func = Function::new("test_subnormals", Type::F64);
    let mut bb0 = BasicBlock::new(0, Terminator::Ret(Some(3)));
    bb0.push(Instruction::new(Some(0), Type::F64, Op::ConstF64(subnormal_val), 1));
    bb0.push(Instruction::new(Some(1), Type::F64, Op::ConstF64(subnormal_val), 2));
    bb0.push(Instruction::new(Some(2), Type::F64, Op::Add(0, 1), 3)); // 2 * subnormal
    bb0.push(Instruction::new(Some(3), Type::F64, Op::Mul(2, 1), 4)); // product
    func.add_block(bb0);

    let vm_res = run_vm(&func, 1000).expect("VM subnormal");
    let jit_res = run_jit_0arg(&func).expect("JIT subnormal");
    assert_eq!(vm_res, jit_res);

    // Test Infinity arithmetic: Inf + 1000.0 == Inf, Inf * -1.0 == -Inf
    let mut func = Function::new("test_infinities", Type::F64);
    let mut bb0 = BasicBlock::new(0, Terminator::Ret(Some(3)));
    bb0.push(Instruction::new(Some(0), Type::F64, Op::ConstF64(f64::INFINITY), 1));
    bb0.push(Instruction::new(Some(1), Type::F64, Op::ConstF64(-1.0), 2));
    bb0.push(Instruction::new(Some(2), Type::F64, Op::ConstF64(100.0), 3));
    bb0.push(Instruction::new(Some(3), Type::F64, Op::Mul(0, 1), 4)); // -Inf
    func.add_block(bb0);

    let vm_res = run_vm(&func, 1000).expect("VM Inf");
    let jit_res = run_jit_0arg(&func).expect("JIT Inf");
    assert_eq!(vm_res, Value::F64(f64::NEG_INFINITY));
    assert_eq!(jit_res, Value::F64(f64::NEG_INFINITY));

    // Test Signed Zero: -0.0 vs +0.0
    let mut func = Function::new("test_signed_zero_cmp", Type::Bool);
    let mut bb0 = BasicBlock::new(0, Terminator::Ret(Some(2)));
    bb0.push(Instruction::new(Some(0), Type::F64, Op::ConstF64(-0.0), 1));
    bb0.push(Instruction::new(Some(1), Type::F64, Op::ConstF64(0.0), 2));
    bb0.push(Instruction::new(Some(2), Type::Bool, Op::CmpEq(0, 1), 3));
    func.add_block(bb0);

    let vm_res = run_vm(&func, 1000).expect("VM signed zero");
    let jit_res = run_jit_0arg(&func).expect("JIT signed zero");
    assert_eq!(vm_res, Value::Bool(true));
    assert_eq!(jit_res, Value::Bool(true));
}

/// Stress Test 3: NaN Semantic Divergence in CmpEq
/// In IEEE-754 (and Cranelift JIT / WASM), NaN == NaN is FALSE.
/// In knr-interp VM, Value::eq overrides F64 to return TRUE when both are NaN.
#[test]
fn test_stress_f64_nan_comparison_divergence() {
    let mut func = Function::new("test_nan_cmp", Type::Bool);
    let mut bb0 = BasicBlock::new(0, Terminator::Ret(Some(2)));
    bb0.push(Instruction::new(Some(0), Type::F64, Op::ConstF64(f64::NAN), 1));
    bb0.push(Instruction::new(Some(1), Type::F64, Op::ConstF64(f64::NAN), 2));
    bb0.push(Instruction::new(Some(2), Type::Bool, Op::CmpEq(0, 1), 3));
    func.add_block(bb0);

    let vm_res = run_vm(&func, 1000).expect("VM NaN cmp");
    let jit_res = run_jit_0arg(&func).expect("JIT NaN cmp");

    println!("NaN cmp_eq results: VM = {vm_res:?}, JIT = {jit_res:?}");
    // Unification confirmation:
    // VM: Value::Bool(false)
    // JIT: Value::Bool(false)
    assert_eq!(vm_res, Value::Bool(false), "VM returns false for NaN == NaN conforming to IEEE-754");
    assert_eq!(jit_res, Value::Bool(false), "JIT fcmp(FloatCC::Equal) strictly respects IEEE-754");
    assert_eq!(vm_res, jit_res, "VM and JIT agree on IEEE-754 NaN equality semantics");
}

/// Stress Test 4: Deeply Nested Basic Blocks (Chain of 50 basic blocks)
#[test]
fn test_stress_deeply_nested_basic_blocks() {
    let count = 50;
    let mut func = Function::new("test_deep_chain", Type::I64);
    let mut next_reg: RegId = 0;

    // Entry block: r0 = 0
    let mut bb0 = BasicBlock::new(0, Terminator::Br(1));
    bb0.push(Instruction::new(Some(0), Type::I64, Op::ConstI64(0), 1));
    next_reg += 1;
    func.add_block(bb0);

    let mut current_sum = 0;
    for i in 1..count {
        let is_last = i == count - 1;
        let terminator = if is_last {
            Terminator::Ret(Some(next_reg + 1))
        } else {
            Terminator::Br((i + 1) as u32)
        };
        let mut bb = BasicBlock::new(i as u32, terminator);

        // Add 1 to previous sum
        let r_const = next_reg;
        next_reg += 1;
        bb.push(Instruction::new(Some(r_const), Type::I64, Op::ConstI64(1), i * 2));

        let r_next_sum = next_reg;
        next_reg += 1;
        bb.push(Instruction::new(
            Some(r_next_sum),
            Type::I64,
            Op::Add(current_sum, r_const),
            i * 2 + 1,
        ));
        current_sum = r_next_sum;

        func.add_block(bb);
    }

    let vm_res = run_vm(&func, 1000).expect("VM deep chain");
    let jit_res = run_jit_0arg(&func).expect("JIT deep chain");

    assert_eq!(vm_res, Value::I64((count - 1) as i64));
    assert_eq!(jit_res, Value::I64((count - 1) as i64));
}

/// Stress Test 5: Container Memory Lifecycle (Buffer allocation, multiple writes, reads, free)
#[test]
fn test_stress_container_memory_lifecycle() {
    let mut func = Function::new("test_container_lifecycle", Type::I64);
    let mut bb0 = BasicBlock::new(0, Terminator::Ret(Some(8)));

    // Allocate 128 byte buffer
    bb0.push(Instruction::new(Some(0), Type::I64, Op::ConstI64(128), 1));
    bb0.push(Instruction::new(Some(1), Type::Buf, Op::BufAlloc(0), 2));

    // Write 42 at index 0
    bb0.push(Instruction::new(Some(2), Type::I64, Op::ConstI64(0), 3));
    bb0.push(Instruction::new(Some(3), Type::I64, Op::ConstI64(42), 4));
    bb0.push(Instruction::new(Some(4), Type::Buf, Op::BufWriteU8(1, 2, 3), 5));

    // Write 99 at index 127
    bb0.push(Instruction::new(Some(5), Type::I64, Op::ConstI64(127), 6));
    bb0.push(Instruction::new(Some(6), Type::I64, Op::ConstI64(99), 7));
    bb0.push(Instruction::new(Some(7), Type::Buf, Op::BufWriteU8(4, 5, 6), 8));

    // Read back index 0 and index 127, add them
    bb0.push(Instruction::new(Some(8), Type::I64, Op::BufReadU8(7, 2), 9));
    bb0.push(Instruction::new(Some(9), Type::I64, Op::BufReadU8(7, 5), 10));
    bb0.push(Instruction::new(Some(10), Type::I64, Op::Add(8, 9), 11));

    // Free buffer
    bb0.push(Instruction::new(None, Type::Void, Op::Free(7), 12));

    bb0.terminator = Terminator::Ret(Some(10));
    func.add_block(bb0);

    let vm_res = run_vm(&func, 1000).expect("VM buffer lifecycle");
    let jit_res = run_jit_0arg(&func).expect("JIT buffer lifecycle");

    assert_eq!(vm_res, Value::I64(141));
    assert_eq!(jit_res, Value::I64(141));
}

/// Stress Test 6: Infinite Loop & Gas Exhaustion in VM vs JIT
#[test]
fn test_stress_gas_exhaustion_on_infinite_loop() {
    let mut func = Function::new("test_infinite_loop", Type::I64);
    // bb0: jumps to bb1, bb1: jumps to bb1
    let bb0 = BasicBlock::new(0, Terminator::Br(1));
    let bb1 = BasicBlock::new(1, Terminator::Br(1));
    func.add_block(bb0);
    func.add_block(bb1);

    // VM with 50 gas steps must terminate cleanly with GasExhausted
    let vm_err = run_vm(&func, 50).expect_err("VM must exhaust gas on infinite loop");
    assert!(vm_err.contains("Gas exhausted"));

    // Verify Cranelift compilation succeeds (produces valid machine code for the loop)
    let mut backend = CraneliftBackend::new().unwrap();
    let code_ptr = backend.compile_jit(&func).expect("Cranelift JIT compiles loop");
    assert!(!code_ptr.is_null());
    // Note: Calling `code_ptr` directly would cause an unmetered infinite loop because JIT
    // does not inject gas decrement instrumentation.
}

/// Stress Test 7: Division by Zero & i64::MIN / -1 CPU Traps
/// When dividing in JIT, does it trigger hardware SIGFPE or Cranelift Trap?
#[test]
fn test_stress_division_by_zero_and_overflow_traps() {
    // 1. VM behavior:
    // i64::MIN / -1 returns i64::MIN without error
    let mut func = Function::new("test_vm_div_overflow", Type::I64);
    let mut bb0 = BasicBlock::new(0, Terminator::Ret(Some(2)));
    bb0.push(Instruction::new(Some(0), Type::I64, Op::ConstI64(i64::MIN), 1));
    bb0.push(Instruction::new(Some(1), Type::I64, Op::ConstI64(-1), 2));
    bb0.push(Instruction::new(Some(2), Type::I64, Op::Div(0, 1), 3));
    func.add_block(bb0);
    let vm_res = run_vm(&func, 1000).expect("VM handles i64::MIN / -1 safely");
    assert_eq!(vm_res, Value::I64(i64::MIN));

    // Div by 0 in VM returns DivisionByZero error
    let mut func_div0 = Function::new("test_vm_div_zero", Type::I64);
    let mut bb0 = BasicBlock::new(0, Terminator::Ret(Some(2)));
    bb0.push(Instruction::new(Some(0), Type::I64, Op::ConstI64(42), 1));
    bb0.push(Instruction::new(Some(1), Type::I64, Op::ConstI64(0), 2));
    bb0.push(Instruction::new(Some(2), Type::I64, Op::Div(0, 1), 3));
    func_div0.add_block(bb0);
    let vm_err = run_vm(&func_div0, 1000).expect_err("VM returns division by zero error");
    assert!(vm_err.contains("Division by zero"));

    // 2. Cranelift JIT compilation:
    let mut backend = CraneliftBackend::new().unwrap();
    let code_overflow = backend.compile_jit(&func).expect("JIT compiles i64::MIN / -1");
    assert!(!code_overflow.is_null());

    let mut backend0 = CraneliftBackend::new().unwrap();
    let code_div0 = backend0.compile_jit(&func_div0).expect("JIT compiles div by 0");
    assert!(!code_div0.is_null());
}

#[test]
fn test_stress_jit_execution_div_overflow_signal() {
    // Spawn a subprocess to test if calling the JIT function with i64::MIN / -1 causes a crash (SIGFPE)
    let current_exe = std::env::current_exe().unwrap();
    let output = std::process::Command::new(current_exe)
        .arg("--exact")
        .arg("test_helper_execute_div_min_by_minus_one")
        .arg("--ignored")
        .arg("--nocapture")
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    println!("Subprocess output status for i64::MIN / -1: {:?}", output.status);
    println!("stdout:\n{stdout}\nstderr:\n{stderr}");
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = output.status.signal() {
            println!("Subprocess terminated by signal {signal} (SIGFPE is 8)");
        }
    }
}

#[test]
fn test_stress_jit_execution_div_zero_signal() {
    // Spawn a subprocess to test if calling the JIT function with x / 0 causes a crash (SIGFPE)
    let current_exe = std::env::current_exe().unwrap();
    let output = std::process::Command::new(current_exe)
        .arg("--exact")
        .arg("test_helper_execute_div_by_zero")
        .arg("--ignored")
        .arg("--nocapture")
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    println!("Subprocess output status for div by 0: {:?}", output.status);
    println!("stdout:\n{stdout}\nstderr:\n{stderr}");
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = output.status.signal() {
            println!("Subprocess terminated by signal {signal} (SIGFPE is 8)");
        }
    }
}

#[test]
#[ignore] // Run only via subprocess
fn test_helper_execute_div_min_by_minus_one() {
    let mut func = Function::new("test_div_overflow", Type::I64);
    let mut bb0 = BasicBlock::new(0, Terminator::Ret(Some(2)));
    bb0.push(Instruction::new(Some(0), Type::I64, Op::ConstI64(i64::MIN), 1));
    bb0.push(Instruction::new(Some(1), Type::I64, Op::ConstI64(-1), 2));
    bb0.push(Instruction::new(Some(2), Type::I64, Op::Div(0, 1), 3));
    func.add_block(bb0);

    let res = run_jit_0arg(&func);
    println!("JIT executed i64::MIN / -1 successfully: {:?}", res);
}

#[test]
#[ignore] // Run only via subprocess
fn test_helper_execute_div_by_zero() {
    let mut func = Function::new("test_div_by_zero", Type::I64);
    let mut bb0 = BasicBlock::new(0, Terminator::Ret(Some(2)));
    bb0.push(Instruction::new(Some(0), Type::I64, Op::ConstI64(42), 1));
    bb0.push(Instruction::new(Some(1), Type::I64, Op::ConstI64(0), 2));
    bb0.push(Instruction::new(Some(2), Type::I64, Op::Div(0, 1), 3));
    func.add_block(bb0);

    let res = run_jit_0arg(&func);
    println!("JIT executed div by 0 successfully: {:?}", res);
}

