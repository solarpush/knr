use knr_codegen::{CodegenError, WasmBackend};
use knr_core::{BasicBlock, Function, Instruction, Module, Op, Terminator, Type};

const WASM_MAGIC: [u8; 4] = [0x00, 0x61, 0x73, 0x6D];
const WASM_VERSION_1: [u8; 4] = [0x01, 0x00, 0x00, 0x00];
const WASM_OP_CALL: u8 = 0x10;
const WASM_OP_DROP: u8 = 0x1A;


/// Helper to verify that a byte slice contains a WASM `Call` instruction targeting `target_idx`.
/// Opcode: 0x10 followed by LEB128-encoded target_idx.
fn contains_call_to(bytes: &[u8], target_idx: u32) -> bool {
    let mut pattern = vec![WASM_OP_CALL];
    let mut val = target_idx;
    loop {
        let byte = (val & 0x7F) as u8;
        val >>= 7;
        if val != 0 {
            pattern.push(byte | 0x80);
        } else {
            pattern.push(byte);
            break;
        }
    }
    bytes.windows(pattern.len()).any(|w| w == pattern.as_slice())
}

/// Helper to count occurrences of a specific byte opcode.
fn count_opcode(bytes: &[u8], opcode: u8) -> usize {
    bytes.iter().filter(|&&b| b == opcode).count()
}

fn assert_wasm_valid(bytes: &[u8], expected_exports: &[&str]) {
    assert!(bytes.len() >= 8, "WASM binary too short: {} bytes", bytes.len());
    assert_eq!(&bytes[0..4], &WASM_MAGIC, "WASM magic mismatch");
    assert_eq!(&bytes[4..8], &WASM_VERSION_1, "WASM version mismatch");

    for &name in expected_exports {
        let name_bytes = name.as_bytes();
        let mut expected = vec![name_bytes.len() as u8];
        expected.extend_from_slice(name_bytes);
        assert!(
            bytes.windows(expected.len()).any(|w| w == expected.as_slice()),
            "Export '{}' not found in WASM export section",
            name
        );
    }
}

// ============================================================================
// 1. Arity Stress: 0 args, 1 arg, multiple (5 mixed-type) args
// ============================================================================

#[test]
fn test_adv_wasm_call_arity_0_1_and_5_args() {
    let backend = WasmBackend::new();
    let mut module = Module::new();

    // Callee 0 args: fn f0() -> i64 { bb0: %0 = 42; ret %0 }
    let mut f0 = Function::new("f0", Type::I64);
    let mut b0 = BasicBlock::new(0, Terminator::Ret(Some(0)));
    b0.push(Instruction::new(Some(0), Type::I64, Op::ConstI64(42), 1));
    f0.add_block(b0);
    module.add_function(f0);

    // Callee 1 arg: fn f1(%0: i64) -> i64 { bb0: ret %0 }
    let mut f1 = Function::new("f1", Type::I64);
    f1.add_param(0, Type::I64);
    f1.add_block(BasicBlock::new(0, Terminator::Ret(Some(0))));
    module.add_function(f1);

    // Callee 5 mixed args: fn f5(%0: i64, %1: f64, %2: bool, %3: i64, %4: f64) -> f64 { bb0: %5 = add %1 %4; ret %5 }
    let mut f5 = Function::new("f5", Type::F64);
    f5.add_param(0, Type::I64);
    f5.add_param(1, Type::F64);
    f5.add_param(2, Type::Bool);
    f5.add_param(3, Type::I64);
    f5.add_param(4, Type::F64);
    let mut b5 = BasicBlock::new(0, Terminator::Ret(Some(5)));
    b5.push(Instruction::new(Some(5), Type::F64, Op::Add(1, 4), 1));
    f5.add_block(b5);
    module.add_function(f5);

    // Caller: fn caller() -> f64
    let mut caller = Function::new("caller", Type::F64);
    let mut bc = BasicBlock::new(0, Terminator::Ret(Some(8)));
    // Call f0: %0 = call f0
    bc.push(Instruction::new(Some(0), Type::I64, Op::Call("f0".to_string(), vec![]), 1));
    // Call f1: %1 = call f1 %0
    bc.push(Instruction::new(Some(1), Type::I64, Op::Call("f1".to_string(), vec![0]), 2));
    // Setup args for f5
    bc.push(Instruction::new(Some(2), Type::I64, Op::ConstI64(10), 3));
    bc.push(Instruction::new(Some(3), Type::F64, Op::ConstF64(1.5), 4));
    bc.push(Instruction::new(Some(4), Type::Bool, Op::ConstBool(true), 5));
    bc.push(Instruction::new(Some(5), Type::I64, Op::ConstI64(20), 6));
    bc.push(Instruction::new(Some(6), Type::F64, Op::ConstF64(2.5), 7));
    // Call f5: %7 = call f5 %2 %3 %4 %5 %6
    bc.push(Instruction::new(
        Some(7),
        Type::F64,
        Op::Call("f5".to_string(), vec![2, 3, 4, 5, 6]),
        8,
    ));
    // %8 = add %3 %7
    bc.push(Instruction::new(Some(8), Type::F64, Op::Add(3, 7), 9));
    caller.add_block(bc);
    module.add_function(caller);

    let bytes = backend.compile(&module).expect("Compilation must succeed for 0, 1, and 5-arg calls");
    assert_wasm_valid(&bytes, &["f0", "f1", "f5", "caller"]);

    // Function indices: f0 = 0, f1 = 1, f5 = 2, caller = 3
    assert!(contains_call_to(&bytes, 0), "Must contain Call(0) for f0");
    assert!(contains_call_to(&bytes, 1), "Must contain Call(1) for f1");
    assert!(contains_call_to(&bytes, 2), "Must contain Call(2) for f5");
}

// ============================================================================
// 2. Reject Calls to Non-Existent Functions
// ============================================================================

#[test]
fn test_adv_wasm_call_unknown_function_rejection() {
    let backend = WasmBackend::new();
    let mut module = Module::new();

    let mut caller = Function::new("caller", Type::I64);
    let mut b0 = BasicBlock::new(0, Terminator::Ret(Some(0)));
    b0.push(Instruction::new(
        Some(0),
        Type::I64,
        Op::Call("ghost_fn_that_does_not_exist".to_string(), vec![]),
        1,
    ));
    caller.add_block(b0);
    module.add_function(caller);

    let res = backend.compile(&module);
    assert!(res.is_err(), "Must reject call to non-existent function");
    match res.unwrap_err() {
        CodegenError::Wasm(msg) => {
            assert!(
                msg.contains("Unknown function in call: ghost_fn_that_does_not_exist"),
                "Expected unknown function message, got: {msg}"
            );
        }
        other => panic!("Expected CodegenError::Wasm, got: {other:?}"),
    }
}

#[test]
fn test_adv_wasm_call_empty_function_name_rejection() {
    let backend = WasmBackend::new();
    let mut module = Module::new();

    let mut caller = Function::new("caller", Type::I64);
    let mut b0 = BasicBlock::new(0, Terminator::Ret(Some(0)));
    b0.push(Instruction::new(
        Some(0),
        Type::I64,
        Op::Call("".to_string(), vec![]),
        1,
    ));
    caller.add_block(b0);
    module.add_function(caller);

    let res = backend.compile(&module);
    assert!(res.is_err(), "Must reject call with empty function name");
}

// ============================================================================
// 3. Reject Calls with Unknown / Unmapped Argument Registers
// ============================================================================

#[test]
fn test_adv_wasm_call_unknown_argument_register_rejection() {
    let backend = WasmBackend::new();
    let mut module = Module::new();

    let mut callee = Function::new("callee", Type::I64);
    callee.add_param(0, Type::I64);
    callee.add_block(BasicBlock::new(0, Terminator::Ret(Some(0))));
    module.add_function(callee);

    let mut caller = Function::new("caller", Type::I64);
    let mut b0 = BasicBlock::new(0, Terminator::Ret(Some(0)));
    // Register 99 was never defined or passed as a param
    b0.push(Instruction::new(
        Some(0),
        Type::I64,
        Op::Call("callee".to_string(), vec![99]),
        1,
    ));
    caller.add_block(b0);
    module.add_function(caller);

    let res = backend.compile(&module);
    assert!(res.is_err(), "Must reject call with unmapped argument register");
    match res.unwrap_err() {
        CodegenError::Wasm(msg) => {
            assert!(
                msg.contains("Unknown argument register %99 in call to callee"),
                "Expected unknown arg message, got: {msg}"
            );
        }
        other => panic!("Expected CodegenError::Wasm, got: {other:?}"),
    }
}

// ============================================================================
// 4. Callee with Multiple Basic Blocks (Diamond CFG + Loop CFG)
// ============================================================================

#[test]
fn test_adv_wasm_call_callee_diamond_and_loop_cfgs() {
    let backend = WasmBackend::new();
    let mut module = Module::new();

    // 1. Diamond CFG Callee:
    // fn abs_val(%0: i64) -> i64
    // bb0: %1 = 0; %2 = cmp_lt %0 %1; br_if %2 bb1 bb2
    // bb1: %3 = sub %1 %0; ret %3
    // bb2: ret %0
    let mut f_abs = Function::new("abs_val", Type::I64);
    f_abs.add_param(0, Type::I64);
    let mut b0 = BasicBlock::new(0, Terminator::BrIf { cond: 2, then_blk: 1, else_blk: 2 });
    b0.push(Instruction::new(Some(1), Type::I64, Op::ConstI64(0), 1));
    b0.push(Instruction::new(Some(2), Type::Bool, Op::CmpLt(0, 1), 2));
    f_abs.add_block(b0);

    let mut b1 = BasicBlock::new(1, Terminator::Ret(Some(3)));
    b1.push(Instruction::new(Some(3), Type::I64, Op::Sub(1, 0), 3));
    f_abs.add_block(b1);

    let b2 = BasicBlock::new(2, Terminator::Ret(Some(0)));
    f_abs.add_block(b2);
    module.add_function(f_abs);

    // 2. Loop CFG Callee:
    // fn sum_n(%0: i64) -> i64
    // bb0: %1 = 0 (acc); %2 = 0 (i); br bb1
    // bb1: %3 = cmp_lt %2 %0; br_if %3 bb2 bb3
    // bb2: %4 = add %1 %2; %5 = 1; %6 = add %2 %5; br bb1
    // bb3: ret %1
    let mut f_loop = Function::new("sum_n", Type::I64);
    f_loop.add_param(0, Type::I64);
    let mut lb0 = BasicBlock::new(0, Terminator::Br(1));
    lb0.push(Instruction::new(Some(1), Type::I64, Op::ConstI64(0), 1));
    lb0.push(Instruction::new(Some(2), Type::I64, Op::ConstI64(0), 2));
    f_loop.add_block(lb0);

    let mut lb1 = BasicBlock::new(1, Terminator::BrIf { cond: 3, then_blk: 2, else_blk: 3 });
    lb1.push(Instruction::new(Some(3), Type::Bool, Op::CmpLt(2, 0), 3));
    f_loop.add_block(lb1);

    let mut lb2 = BasicBlock::new(2, Terminator::Br(1));
    lb2.push(Instruction::new(Some(4), Type::I64, Op::Add(1, 2), 4));
    lb2.push(Instruction::new(Some(5), Type::I64, Op::ConstI64(1), 5));
    lb2.push(Instruction::new(Some(6), Type::I64, Op::Add(2, 5), 6));
    f_loop.add_block(lb2);

    let lb3 = BasicBlock::new(3, Terminator::Ret(Some(1)));
    f_loop.add_block(lb3);
    module.add_function(f_loop);

    // 3. Multi-block Caller calling both diamond and loop callees:
    // fn caller_multi(%0: i64) -> i64
    // bb0: %1 = call abs_val %0; %2 = call sum_n %1; ret %2
    let mut caller = Function::new("caller_multi", Type::I64);
    caller.add_param(0, Type::I64);
    let mut cb0 = BasicBlock::new(0, Terminator::Ret(Some(2)));
    cb0.push(Instruction::new(
        Some(1),
        Type::I64,
        Op::Call("abs_val".to_string(), vec![0]),
        1,
    ));
    cb0.push(Instruction::new(
        Some(2),
        Type::I64,
        Op::Call("sum_n".to_string(), vec![1]),
        2,
    ));
    caller.add_block(cb0);
    module.add_function(caller);

    let bytes = backend.compile(&module).expect("Compilation must succeed for multi-block callees");
    assert_wasm_valid(&bytes, &["abs_val", "sum_n", "caller_multi"]);

    // Indices: abs_val = 0, sum_n = 1, caller_multi = 2
    assert!(contains_call_to(&bytes, 0), "Must call abs_val (0)");
    assert!(contains_call_to(&bytes, 1), "Must call sum_n (1)");
    assert!(bytes.contains(&0x03), "Must emit loop dispatch opcode (0x03)");
}

// ============================================================================
// 5. Return Types: Void vs I64 vs F64 vs Bool
// ============================================================================

#[test]
fn test_adv_wasm_call_return_types_void_i64_f64_bool() {
    let backend = WasmBackend::new();
    let mut module = Module::new();

    // 1. Void callee
    let mut f_void = Function::new("ret_void", Type::Void);
    f_void.add_block(BasicBlock::new(0, Terminator::Ret(None)));
    module.add_function(f_void);

    // 2. I64 callee
    let mut f_i64 = Function::new("ret_i64", Type::I64);
    let mut bi = BasicBlock::new(0, Terminator::Ret(Some(0)));
    bi.push(Instruction::new(Some(0), Type::I64, Op::ConstI64(100), 1));
    f_i64.add_block(bi);
    module.add_function(f_i64);

    // 3. F64 callee
    let mut f_f64 = Function::new("ret_f64", Type::F64);
    let mut bf = BasicBlock::new(0, Terminator::Ret(Some(0)));
    bf.push(Instruction::new(Some(0), Type::F64, Op::ConstF64(42.5), 1));
    f_f64.add_block(bf);
    module.add_function(f_f64);

    // 4. Bool callee
    let mut f_bool = Function::new("ret_bool", Type::Bool);
    let mut bb = BasicBlock::new(0, Terminator::Ret(Some(0)));
    bb.push(Instruction::new(Some(0), Type::Bool, Op::ConstBool(true), 1));
    f_bool.add_block(bb);
    module.add_function(f_bool);

    // Caller calling all four:
    // %0 = call ret_void (dst: None, ty: Void)
    // %1 = call ret_i64  (dst: Some(1), ty: I64)
    // %2 = call ret_f64  (dst: Some(2), ty: F64)
    // %3 = call ret_bool (dst: Some(3), ty: Bool)
    let mut caller = Function::new("caller_all_types", Type::I64);
    let mut cb = BasicBlock::new(0, Terminator::Ret(Some(1)));
    cb.push(Instruction::new(None, Type::Void, Op::Call("ret_void".to_string(), vec![]), 1));
    cb.push(Instruction::new(Some(1), Type::I64, Op::Call("ret_i64".to_string(), vec![]), 2));
    cb.push(Instruction::new(Some(2), Type::F64, Op::Call("ret_f64".to_string(), vec![]), 3));
    cb.push(Instruction::new(Some(3), Type::Bool, Op::Call("ret_bool".to_string(), vec![]), 4));
    caller.add_block(cb);
    module.add_function(caller);

    let bytes = backend.compile(&module).expect("Compilation must succeed for all return types");
    assert_wasm_valid(&bytes, &["ret_void", "ret_i64", "ret_f64", "ret_bool", "caller_all_types"]);

    // Indices: ret_void = 0, ret_i64 = 1, ret_f64 = 2, ret_bool = 3, caller = 4
    assert!(contains_call_to(&bytes, 0), "Must call ret_void (0)");
    assert!(contains_call_to(&bytes, 1), "Must call ret_i64 (1)");
    assert!(contains_call_to(&bytes, 2), "Must call ret_f64 (2)");
    assert!(contains_call_to(&bytes, 3), "Must call ret_bool (3)");
}

#[test]
fn test_adv_wasm_call_void_callee_with_bogus_dst_does_not_panic() {
    let backend = WasmBackend::new();
    let mut module = Module::new();

    let mut f_void = Function::new("v_fn", Type::Void);
    f_void.add_block(BasicBlock::new(0, Terminator::Ret(None)));
    module.add_function(f_void);

    // Malformed SSA instruction: inst.dst is Some(0), but inst.ty is Type::Void
    let mut caller = Function::new("caller_bogus", Type::Void);
    let mut b0 = BasicBlock::new(0, Terminator::Ret(None));
    b0.push(Instruction::new(
        Some(0),
        Type::Void,
        Op::Call("v_fn".to_string(), vec![]),
        1,
    ));
    caller.add_block(b0);
    module.add_function(caller);

    // Must compile cleanly without panic: dst_loc is None for Void, so no local_set or drop is emitted
    let bytes = backend.compile(&module).expect("Must safely handle void call with bogus dst");
    assert_wasm_valid(&bytes, &["v_fn", "caller_bogus"]);
    assert!(contains_call_to(&bytes, 0), "Must call v_fn (0)");
}

// ============================================================================
// 6. Stack Balance: Drop Emitted for Unused Non-Void Return Values
// ============================================================================

#[test]
fn test_adv_wasm_call_drop_emitted_for_unused_i64_f64_bool() {
    let backend = WasmBackend::new();
    let mut module = Module::new();

    // 1. Callee I64
    let mut f_i = Function::new("callee_i", Type::I64);
    let mut bi = BasicBlock::new(0, Terminator::Ret(Some(0)));
    bi.push(Instruction::new(Some(0), Type::I64, Op::ConstI64(1), 1));
    f_i.add_block(bi);
    module.add_function(f_i);

    // 2. Callee F64
    let mut f_f = Function::new("callee_f", Type::F64);
    let mut bf = BasicBlock::new(0, Terminator::Ret(Some(0)));
    bf.push(Instruction::new(Some(0), Type::F64, Op::ConstF64(2.0), 1));
    f_f.add_block(bf);
    module.add_function(f_f);

    // 3. Callee Bool
    let mut f_b = Function::new("callee_b", Type::Bool);
    let mut bb = BasicBlock::new(0, Terminator::Ret(Some(0)));
    bb.push(Instruction::new(Some(0), Type::Bool, Op::ConstBool(false), 1));
    f_b.add_block(bb);
    module.add_function(f_b);

    // Caller: calls all 3 with dst: None, discarding return values!
    let mut caller = Function::new("caller_drops_all", Type::Void);
    let mut bc = BasicBlock::new(0, Terminator::Ret(None));
    bc.push(Instruction::new(None, Type::I64, Op::Call("callee_i".to_string(), vec![]), 1));
    bc.push(Instruction::new(None, Type::F64, Op::Call("callee_f".to_string(), vec![]), 2));
    bc.push(Instruction::new(None, Type::Bool, Op::Call("callee_b".to_string(), vec![]), 3));
    caller.add_block(bc);
    module.add_function(caller);

    let bytes = backend.compile(&module).expect("Compilation must succeed");
    assert_wasm_valid(&bytes, &["callee_i", "callee_f", "callee_b", "caller_drops_all"]);

    // Verify Drop (0x1A) is emitted at least 3 times (once per discarded return)
    let drops = count_opcode(&bytes, WASM_OP_DROP);
    assert!(
        drops >= 3,
        "Expected at least 3 Drop (0x1A) opcodes for 3 discarded call returns, found {drops}"
    );
}

// ============================================================================
// 7. Function Index Accounting for num_imports
// ============================================================================

#[test]
fn test_adv_wasm_call_num_imports_offset_exhaustive() {
    let backend = WasmBackend::new();

    // Module A: 0 imports needed
    let mut mod_a = Module::new();
    let mut f_a1 = Function::new("a1", Type::I64);
    let mut ba1 = BasicBlock::new(0, Terminator::Ret(Some(0)));
    ba1.push(Instruction::new(Some(0), Type::I64, Op::ConstI64(10), 1));
    f_a1.add_block(ba1);
    mod_a.add_function(f_a1);

    let mut f_a2 = Function::new("a2", Type::I64);
    let mut ba2 = BasicBlock::new(0, Terminator::Ret(Some(0)));
    ba2.push(Instruction::new(Some(0), Type::I64, Op::Call("a1".to_string(), vec![]), 1));
    f_a2.add_block(ba2);
    mod_a.add_function(f_a2);

    let bytes_a = backend.compile(&mod_a).expect("Compilation of mod_a ok");
    assert_wasm_valid(&bytes_a, &["a1", "a2"]);
    // a1 is index 0, a2 is index 1. Call to a1 must be Call(0).
    assert!(contains_call_to(&bytes_a, 0), "mod_a: Must call a1 at index 0");

    // Module B: Imports needed (11 imports from collect_imports: rt_map_*, rt_vec_*, rt_buf_*)
    // Triggered by Op::VecNew
    let mut mod_b = Module::new();
    let mut f_b1 = Function::new("b1", Type::I64);
    let mut bb1 = BasicBlock::new(0, Terminator::Ret(Some(0)));
    bb1.push(Instruction::new(Some(0), Type::Vec(Box::new(Type::I64)), Op::VecNew(Type::I64), 1));
    bb1.push(Instruction::new(None, Type::Void, Op::Free(0), 2));
    f_b1.add_block(bb1);
    mod_b.add_function(f_b1);

    let mut f_b2 = Function::new("b2", Type::I64);
    let mut bb2 = BasicBlock::new(0, Terminator::Ret(Some(0)));
    bb2.push(Instruction::new(Some(0), Type::I64, Op::ConstI64(99), 1));
    f_b2.add_block(bb2);
    mod_b.add_function(f_b2);

    let mut f_b3 = Function::new("b3", Type::I64);
    let mut bb3 = BasicBlock::new(0, Terminator::Ret(Some(1)));
    // b3 calls b2 (should be index 11 + 1 = 12)
    bb3.push(Instruction::new(Some(1), Type::I64, Op::Call("b2".to_string(), vec![]), 1));
    f_b3.add_block(bb3);
    mod_b.add_function(f_b3);

    let bytes_b = backend.compile(&mod_b).expect("Compilation of mod_b ok");
    assert_wasm_valid(&bytes_b, &["b1", "b2", "b3"]);

    // With 11 runtime imports:
    // b1 = 11, b2 = 12, b3 = 13
    // Call in b3 targeting b2 MUST be Call(12)
    assert!(
        contains_call_to(&bytes_b, 12),
        "mod_b: Expected Call(12) targeting b2 with 11 imports offset"
    );
    assert!(
        !contains_call_to(&bytes_b, 1),
        "mod_b: Must NOT emit Call(1) without imports offset!"
    );
}

// ============================================================================
// 8. Direct and Mutual Recursion
// ============================================================================

#[test]
fn test_adv_wasm_call_direct_recursion() {
    let backend = WasmBackend::new();
    let mut module = Module::new();

    // fn fact(%0: i64) -> i64
    // bb0:
    //   %1 = const_i64 1
    //   %2 = cmp_lt %0 %1
    //   br_if %2 bb1 bb2
    // bb1:
    //   ret %1
    // bb2:
    //   %3 = sub %0 %1
    //   %4 = call fact %3
    //   %5 = mul %0 %4
    //   ret %5
    let mut fact = Function::new("fact", Type::I64);
    fact.add_param(0, Type::I64);

    let mut bb0 = BasicBlock::new(0, Terminator::BrIf { cond: 2, then_blk: 1, else_blk: 2 });
    bb0.push(Instruction::new(Some(1), Type::I64, Op::ConstI64(1), 1));
    bb0.push(Instruction::new(Some(2), Type::Bool, Op::CmpLt(0, 1), 2));
    fact.add_block(bb0);

    let bb1 = BasicBlock::new(1, Terminator::Ret(Some(1)));
    fact.add_block(bb1);

    let mut bb2 = BasicBlock::new(2, Terminator::Ret(Some(5)));
    bb2.push(Instruction::new(Some(3), Type::I64, Op::Sub(0, 1), 3));
    bb2.push(Instruction::new(
        Some(4),
        Type::I64,
        Op::Call("fact".to_string(), vec![3]),
        4,
    ));
    bb2.push(Instruction::new(Some(5), Type::I64, Op::Mul(0, 4), 5));
    fact.add_block(bb2);

    module.add_function(fact);

    let bytes = backend.compile(&module).expect("Direct recursion must compile");
    assert_wasm_valid(&bytes, &["fact"]);
    // fact is at index 0. Call must be Call(0).
    assert!(contains_call_to(&bytes, 0), "Must contain self-call Call(0) for direct recursion");
}

#[test]
fn test_adv_wasm_call_mutual_recursion() {
    let backend = WasmBackend::new();
    let mut module = Module::new();

    // fn is_even(%0: i64) -> bool
    // bb0: %1 = 0; %2 = cmp_eq %0 %1; br_if %2 bb1 bb2
    // bb1: %3 = true; ret %3
    // bb2: %4 = 1; %5 = sub %0 %4; %6 = call is_odd %5; ret %6
    let mut f_even = Function::new("is_even", Type::Bool);
    f_even.add_param(0, Type::I64);

    let mut eb0 = BasicBlock::new(0, Terminator::BrIf { cond: 2, then_blk: 1, else_blk: 2 });
    eb0.push(Instruction::new(Some(1), Type::I64, Op::ConstI64(0), 1));
    eb0.push(Instruction::new(Some(2), Type::Bool, Op::CmpEq(0, 1), 2));
    f_even.add_block(eb0);

    let mut eb1 = BasicBlock::new(1, Terminator::Ret(Some(3)));
    eb1.push(Instruction::new(Some(3), Type::Bool, Op::ConstBool(true), 3));
    f_even.add_block(eb1);

    let mut eb2 = BasicBlock::new(2, Terminator::Ret(Some(6)));
    eb2.push(Instruction::new(Some(4), Type::I64, Op::ConstI64(1), 4));
    eb2.push(Instruction::new(Some(5), Type::I64, Op::Sub(0, 4), 5));
    eb2.push(Instruction::new(
        Some(6),
        Type::Bool,
        Op::Call("is_odd".to_string(), vec![5]),
        6,
    ));
    f_even.add_block(eb2);
    module.add_function(f_even);

    // fn is_odd(%0: i64) -> bool
    // bb0: %1 = 0; %2 = cmp_eq %0 %1; br_if %2 bb1 bb2
    // bb1: %3 = false; ret %3
    // bb2: %4 = 1; %5 = sub %0 %4; %6 = call is_even %5; ret %6
    let mut f_odd = Function::new("is_odd", Type::Bool);
    f_odd.add_param(0, Type::I64);

    let mut ob0 = BasicBlock::new(0, Terminator::BrIf { cond: 2, then_blk: 1, else_blk: 2 });
    ob0.push(Instruction::new(Some(1), Type::I64, Op::ConstI64(0), 1));
    ob0.push(Instruction::new(Some(2), Type::Bool, Op::CmpEq(0, 1), 2));
    f_odd.add_block(ob0);

    let mut ob1 = BasicBlock::new(1, Terminator::Ret(Some(3)));
    ob1.push(Instruction::new(Some(3), Type::Bool, Op::ConstBool(false), 3));
    f_odd.add_block(ob1);

    let mut ob2 = BasicBlock::new(2, Terminator::Ret(Some(6)));
    ob2.push(Instruction::new(Some(4), Type::I64, Op::ConstI64(1), 4));
    ob2.push(Instruction::new(Some(5), Type::I64, Op::Sub(0, 4), 5));
    ob2.push(Instruction::new(
        Some(6),
        Type::Bool,
        Op::Call("is_even".to_string(), vec![5]),
        6,
    ));
    f_odd.add_block(ob2);
    module.add_function(f_odd);

    let bytes = backend.compile(&module).expect("Mutual recursion must compile");
    assert_wasm_valid(&bytes, &["is_even", "is_odd"]);
    // is_even = 0, is_odd = 1
    assert!(contains_call_to(&bytes, 1), "is_even must emit Call(1) targeting is_odd");
    assert!(contains_call_to(&bytes, 0), "is_odd must emit Call(0) targeting is_even");
}
