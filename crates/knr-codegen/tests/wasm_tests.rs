use knr_codegen::WasmBackend;
use knr_core::{BasicBlock, Function, Instruction, Module, Op, Terminator, Type};

const WASM_MAGIC: [u8; 4] = [0x00, 0x61, 0x73, 0x6D];
const WASM_VERSION_1: [u8; 4] = [0x01, 0x00, 0x00, 0x00];

#[test]
fn test_wasm_magic_and_version() {
    let backend = WasmBackend::new();
    let mut func = Function::new("test_empty", Type::Void);
    let block = BasicBlock::new(0, Terminator::Ret(None));
    func.add_block(block);

    let bytes = backend.compile_function(&func).expect("compilation failed");

    assert!(bytes.len() >= 8);
    assert_eq!(&bytes[0..4], &WASM_MAGIC, "Wasm magic header mismatch");
    assert_eq!(
        &bytes[4..8],
        &WASM_VERSION_1,
        "Wasm version header mismatch"
    );
}

#[test]
fn test_wasm_pure_arithmetic_i64() {
    let backend = WasmBackend::new();
    // %0 = 10, %1 = 20, %2 = add %0, %1, %3 = 2, %4 = mul %2, %3, ret %4
    let mut func = Function::new("compute_i64", Type::I64);
    let mut block = BasicBlock::new(0, Terminator::Ret(Some(4)));

    block.push(Instruction::new(Some(0), Type::I64, Op::ConstI64(10), 1));
    block.push(Instruction::new(Some(1), Type::I64, Op::ConstI64(20), 2));
    block.push(Instruction::new(Some(2), Type::I64, Op::Add(0, 1), 3));
    block.push(Instruction::new(Some(3), Type::I64, Op::ConstI64(2), 4));
    block.push(Instruction::new(Some(4), Type::I64, Op::Mul(2, 3), 5));

    func.add_block(block);

    let bytes = backend.compile_function(&func).expect("compilation failed");
    assert_eq!(&bytes[0..4], &WASM_MAGIC);
    assert_eq!(&bytes[4..8], &WASM_VERSION_1);
    assert!(bytes.len() > 16);
}

#[test]
fn test_wasm_f64_arithmetic() {
    let backend = WasmBackend::new();
    let mut func = Function::new("compute_f64", Type::F64);
    let mut block = BasicBlock::new(0, Terminator::Ret(Some(3)));

    block.push(Instruction::new(
        Some(0),
        Type::F64,
        Op::ConstF64(12.5),
        1,
    ));
    block.push(Instruction::new(
        Some(1),
        Type::F64,
        Op::ConstF64(2.5),
        2,
    ));
    block.push(Instruction::new(Some(2), Type::F64, Op::Div(0, 1), 3));
    block.push(Instruction::new(Some(3), Type::F64, Op::Sub(2, 1), 4));

    func.add_block(block);

    let bytes = backend.compile_function(&func).expect("compilation failed");
    assert_eq!(&bytes[0..4], &WASM_MAGIC);
}

#[test]
fn test_wasm_bool_and_comparisons() {
    let backend = WasmBackend::new();
    let mut func = Function::new("check_lt", Type::Bool);
    let mut block = BasicBlock::new(0, Terminator::Ret(Some(2)));

    block.push(Instruction::new(Some(0), Type::I64, Op::ConstI64(15), 1));
    block.push(Instruction::new(Some(1), Type::I64, Op::ConstI64(30), 2));
    block.push(Instruction::new(Some(2), Type::Bool, Op::CmpLt(0, 1), 3));

    func.add_block(block);

    let bytes = backend.compile_function(&func).expect("compilation failed");
    assert_eq!(&bytes[0..4], &WASM_MAGIC);
}

#[test]
fn test_wasm_branching_br_if() {
    let backend = WasmBackend::new();
    // fn select_max(a: i64, b: i64) -> i64
    // bb0:
    //   %2 = cmp_lt %0, %1
    //   br_if %2, bb1, bb2
    // bb1:
    //   ret %1
    // bb2:
    //   ret %0
    let mut func = Function::new("select_max", Type::I64);
    func.add_param(0, Type::I64);
    func.add_param(1, Type::I64);

    let mut bb0 = BasicBlock::new(
        0,
        Terminator::BrIf {
            cond: 2,
            then_blk: 1,
            else_blk: 2,
        },
    );
    bb0.push(Instruction::new(Some(2), Type::Bool, Op::CmpLt(0, 1), 1));
    func.add_block(bb0);

    let bb1 = BasicBlock::new(1, Terminator::Ret(Some(1)));
    func.add_block(bb1);

    let bb2 = BasicBlock::new(2, Terminator::Ret(Some(0)));
    func.add_block(bb2);

    let bytes = backend.compile_function(&func).expect("compilation failed");
    assert_eq!(&bytes[0..4], &WASM_MAGIC);
    assert_eq!(&bytes[4..8], &WASM_VERSION_1);
}

#[test]
fn test_wasm_loop() {
    let backend = WasmBackend::new();
    // Loop counting down
    // bb0:
    //   %0 = const_i64 10
    //   %1 = const_i64 1
    //   br bb1
    // bb1:
    //   %2 = sub %0, %1
    //   %3 = const_i64 0
    //   %4 = cmp_lt %3, %2
    //   br_if %4, bb1, bb2
    // bb2:
    //   ret %2
    let mut func = Function::new("loop_countdown", Type::I64);

    let mut bb0 = BasicBlock::new(0, Terminator::Br(1));
    bb0.push(Instruction::new(Some(0), Type::I64, Op::ConstI64(10), 1));
    bb0.push(Instruction::new(Some(1), Type::I64, Op::ConstI64(1), 2));
    func.add_block(bb0);

    let mut bb1 = BasicBlock::new(
        1,
        Terminator::BrIf {
            cond: 5,
            then_blk: 1,
            else_blk: 2,
        },
    );
    bb1.push(Instruction::new(Some(2), Type::I64, Op::Sub(0, 1), 3));
    bb1.push(Instruction::new(Some(3), Type::I64, Op::ConstI64(0), 4));
    bb1.push(Instruction::new(Some(4), Type::I64, Op::ConstI64(1), 5));
    bb1.push(Instruction::new(Some(5), Type::Bool, Op::CmpLt(3, 4), 6));
    func.add_block(bb1);

    let bb2 = BasicBlock::new(2, Terminator::Ret(Some(2)));
    func.add_block(bb2);

    let bytes = backend.compile_function(&func).expect("compilation failed");
    assert_eq!(&bytes[0..4], &WASM_MAGIC);
}

#[test]
fn test_wasm_buf_operations_with_runtime() {
    let backend = WasmBackend::new();
    let mut func = Function::new("test_buf", Type::I64);
    let mut bb0 = BasicBlock::new(0, Terminator::Ret(Some(6)));

    // %0 = 128 (size)
    bb0.push(Instruction::new(Some(0), Type::I64, Op::ConstI64(128), 1));
    // %1 = buf_alloc %0
    bb0.push(Instruction::new(Some(1), Type::Buf, Op::BufAlloc(0), 2));
    // %2 = 0 (idx), %3 = 77 (val)
    bb0.push(Instruction::new(Some(2), Type::I64, Op::ConstI64(0), 3));
    bb0.push(Instruction::new(Some(3), Type::I64, Op::ConstI64(77), 4));
    // %4 = buf_write_u8 %1, %2, %3
    bb0.push(Instruction::new(
        Some(4),
        Type::Buf,
        Op::BufWriteU8(1, 2, 3),
        5,
    ));
    // %5 = buf_read_u8 %4, %2
    bb0.push(Instruction::new(
        Some(5),
        Type::I64,
        Op::BufReadU8(4, 2),
        6,
    ));
    // %6 = buf_slice %4, %2, %0
    bb0.push(Instruction::new(
        Some(6),
        Type::Buf,
        Op::BufSlice(4, 2, 0),
        7,
    ));
    // free %6
    bb0.push(Instruction::new(None, Type::Void, Op::Free(6), 8));

    func.add_block(bb0);

    let bytes = backend.compile_function(&func).expect("compilation failed");
    assert_eq!(&bytes[0..4], &WASM_MAGIC);
    assert_eq!(&bytes[4..8], &WASM_VERSION_1);
}

#[test]
fn test_wasm_multi_function_module() {
    let backend = WasmBackend::new();
    let mut module = Module::new();

    // func 1: f1() -> 100
    let mut f1 = Function::new("f1", Type::I64);
    let mut b1 = BasicBlock::new(0, Terminator::Ret(Some(0)));
    b1.push(Instruction::new(Some(0), Type::I64, Op::ConstI64(100), 1));
    f1.add_block(b1);
    module.add_function(f1);

    // func 2: main() -> 200
    let mut f2 = Function::new("main", Type::I64);
    let mut b2 = BasicBlock::new(0, Terminator::Ret(Some(0)));
    b2.push(Instruction::new(Some(0), Type::I64, Op::ConstI64(200), 1));
    f2.add_block(b2);
    module.add_function(f2);

    let bytes = backend.compile(&module).expect("compilation failed");
    assert_eq!(&bytes[0..4], &WASM_MAGIC);
    assert_eq!(&bytes[4..8], &WASM_VERSION_1);
}

#[test]
fn test_wasm_single_block_loop_and_branch_emission() {
    let backend = WasmBackend::new();

    // 1. Single-block self-loop: bb0: br bb0
    let mut f_loop = Function::new("f_loop", Type::Void);
    let bb0 = BasicBlock::new(0, Terminator::Br(0));
    f_loop.add_block(bb0);

    let bytes_loop = backend.compile_function(&f_loop).expect("loop compile ok");
    assert_eq!(&bytes_loop[0..4], &WASM_MAGIC);
    assert!(!bytes_loop[bytes_loop.len() - 4..].contains(&0x0F), "Must not end with Return for self loop");
    assert!(bytes_loop.contains(&0x03), "Must emit Loop opcode (0x03)");
    assert!(bytes_loop.contains(&0x0C), "Must emit Br opcode (0x0C)");

    // 2. Single-block branching: bb0: br_if %0, bb0, bb0
    let mut f_brif = Function::new("f_brif", Type::Void);
    let mut b_brif = BasicBlock::new(0, Terminator::BrIf {
        cond: 0,
        then_blk: 0,
        else_blk: 0,
    });
    b_brif.push(Instruction::new(Some(0), Type::Bool, Op::ConstBool(true), 1));
    f_brif.add_block(b_brif);

    let bytes_brif = backend.compile_function(&f_brif).expect("brif compile ok");
    assert_eq!(&bytes_brif[0..4], &WASM_MAGIC);
    assert!(!bytes_brif[bytes_brif.len() - 4..].contains(&0x0F), "Must not end with Return for branch terminator");
    assert!(bytes_brif.contains(&0x03), "Must emit Loop opcode (0x03)");
    assert!(bytes_brif.contains(&0x0C), "Must emit Br opcode (0x0C)");
}



// ============================================================================
// HELPER: Standalone WASM Bytecode Inspection Helpers
// ============================================================================

/// Helper to verify that a byte slice contains a WASM `Call` instruction targeting `target_idx`.
/// Opcode: 0x10 followed by LEB128-encoded target_idx.
fn contains_call_to(bytes: &[u8], target_idx: u32) -> bool {
    let mut pattern = vec![0x10];
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

/// Helper to assert valid WASM module header and exported function names.
fn assert_wasm_module_valid(bytes: &[u8], expected_exports: &[&str]) {
    assert!(bytes.len() >= 8, "WASM binary too short: {} bytes", bytes.len());
    assert_eq!(&bytes[0..4], &WASM_MAGIC, "WASM magic header mismatch");
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
// MILESTONE 1: Op::Call Test Scenarios
// ============================================================================

#[test]
fn test_wasm_call_single_caller_arithmetic() {
    let backend = WasmBackend::new();
    let mut module = Module::new();

    // 1. Callee: fn add(%0: i64, %1: i64) -> i64 { bb0: %2 = add %0, %1; ret %2 }
    let mut f_add = Function::new("add", Type::I64);
    f_add.add_param(0, Type::I64);
    f_add.add_param(1, Type::I64);
    let mut b_add = BasicBlock::new(0, Terminator::Ret(Some(2)));
    b_add.push(Instruction::new(Some(2), Type::I64, Op::Add(0, 1), 1));
    f_add.add_block(b_add);
    module.add_function(f_add);

    // 2. Caller: fn caller() -> i64 {
    //      bb0:
    //        %0 = const_i64 10
    //        %1 = const_i64 20
    //        %2 = call add %0 %1
    //        ret %2
    //    }
    let mut f_caller = Function::new("caller", Type::I64);
    let mut b_caller = BasicBlock::new(0, Terminator::Ret(Some(2)));
    b_caller.push(Instruction::new(Some(0), Type::I64, Op::ConstI64(10), 1));
    b_caller.push(Instruction::new(Some(1), Type::I64, Op::ConstI64(20), 2));
    b_caller.push(Instruction::new(
        Some(2),
        Type::I64,
        Op::Call("add".to_string(), vec![0, 1]),
        3,
    ));
    f_caller.add_block(b_caller);
    module.add_function(f_caller);

    let bytes = backend.compile(&module).expect("compilation failed for single call module");
    assert_wasm_module_valid(&bytes, &["add", "caller"]);

    // Function indices: add = 0, caller = 1.
    // In caller body, Instruction::Call(0) -> [0x10, 0x00] must be present.
    assert!(
        contains_call_to(&bytes, 0),
        "Expected Call(0) opcode [0x10, 0x00] in WASM bytecode"
    );
}

#[test]
fn test_wasm_call_multi_block_callee() {
    let backend = WasmBackend::new();
    let mut module = Module::new();

    // 1. Multi-block Callee:
    // fn select_max(a: i64, b: i64) -> i64
    // bb0: %2 = cmp_lt %0, %1; br_if %2, bb1, bb2
    // bb1: ret %1
    // bb2: ret %0
    let mut f_max = Function::new("select_max", Type::I64);
    f_max.add_param(0, Type::I64);
    f_max.add_param(1, Type::I64);

    let mut bb0 = BasicBlock::new(
        0,
        Terminator::BrIf {
            cond: 2,
            then_blk: 1,
            else_blk: 2,
        },
    );
    bb0.push(Instruction::new(Some(2), Type::Bool, Op::CmpLt(0, 1), 1));
    f_max.add_block(bb0);
    f_max.add_block(BasicBlock::new(1, Terminator::Ret(Some(1))));
    f_max.add_block(BasicBlock::new(2, Terminator::Ret(Some(0))));
    module.add_function(f_max);

    // 2. Caller: fn run_max() -> i64 {
    //      bb0:
    //        %0 = const_i64 42
    //        %1 = const_i64 99
    //        %2 = call select_max %0 %1
    //        ret %2
    //    }
    let mut f_caller = Function::new("run_max", Type::I64);
    let mut b_caller = BasicBlock::new(0, Terminator::Ret(Some(2)));
    b_caller.push(Instruction::new(Some(0), Type::I64, Op::ConstI64(42), 1));
    b_caller.push(Instruction::new(Some(1), Type::I64, Op::ConstI64(99), 2));
    b_caller.push(Instruction::new(
        Some(2),
        Type::I64,
        Op::Call("select_max".to_string(), vec![0, 1]),
        3,
    ));
    f_caller.add_block(b_caller);
    module.add_function(f_caller);

    let bytes = backend.compile(&module).expect("compilation failed for multi-block callee call");
    assert_wasm_module_valid(&bytes, &["select_max", "run_max"]);

    // Multi-block callee emits Loop opcode (0x03) for CFG dispatch
    assert!(bytes.contains(&0x03), "Must emit Loop opcode (0x03) for multi-block callee");
    // Caller emits Call to function 0 (select_max)
    assert!(contains_call_to(&bytes, 0), "Must emit Call(0) in caller");
}

#[test]
fn test_wasm_call_void_callee_vs_i64_callee() {
    let backend = WasmBackend::new();
    let mut module = Module::new();

    // 1. Callee void: fn do_nothing() -> void { bb0: ret }
    let mut f_void = Function::new("do_nothing", Type::Void);
    f_void.add_block(BasicBlock::new(0, Terminator::Ret(None)));
    module.add_function(f_void);

    // 2. Callee i64: fn get_value() -> i64 { bb0: %0 = const_i64 777; ret %0 }
    let mut f_val = Function::new("get_value", Type::I64);
    let mut b_val = BasicBlock::new(0, Terminator::Ret(Some(0)));
    b_val.push(Instruction::new(Some(0), Type::I64, Op::ConstI64(777), 1));
    f_val.add_block(b_val);
    module.add_function(f_val);

    // 3. Caller: fn run_tests() -> i64 {
    //      bb0:
    //        call do_nothing
    //        %0 = call get_value
    //        ret %0
    //    }
    let mut f_caller = Function::new("run_tests", Type::I64);
    let mut b_caller = BasicBlock::new(0, Terminator::Ret(Some(0)));
    b_caller.push(Instruction::new(
        None,
        Type::Void,
        Op::Call("do_nothing".to_string(), vec![]),
        1,
    ));
    b_caller.push(Instruction::new(
        Some(0),
        Type::I64,
        Op::Call("get_value".to_string(), vec![]),
        2,
    ));
    f_caller.add_block(b_caller);
    module.add_function(f_caller);

    let bytes = backend.compile(&module).expect("compilation failed for void vs i64 call");
    assert_wasm_module_valid(&bytes, &["do_nothing", "get_value", "run_tests"]);

    // Function indices: do_nothing = 0, get_value = 1, run_tests = 2.
    assert!(contains_call_to(&bytes, 0), "Must emit Call(0) for do_nothing");
    assert!(contains_call_to(&bytes, 1), "Must emit Call(1) for get_value");
}

#[test]
fn test_wasm_call_drop_unused_return_value() {
    let backend = WasmBackend::new();
    let mut module = Module::new();

    // 1. Callee: compute() -> i64
    let mut f_compute = Function::new("compute", Type::I64);
    let mut b_compute = BasicBlock::new(0, Terminator::Ret(Some(0)));
    b_compute.push(Instruction::new(Some(0), Type::I64, Op::ConstI64(123), 1));
    f_compute.add_block(b_compute);
    module.add_function(f_compute);

    // 2. Caller: caller_drop() -> void, calls compute with dst = None and ty = Type::I64
    let mut f_caller = Function::new("caller_drop", Type::Void);
    let mut b_caller = BasicBlock::new(0, Terminator::Ret(None));
    b_caller.push(Instruction::new(
        None,
        Type::I64,
        Op::Call("compute".to_string(), vec![]),
        1,
    ));
    f_caller.add_block(b_caller);
    module.add_function(f_caller);

    let bytes = backend.compile(&module).expect("compilation failed for drop unused call return");
    assert_wasm_module_valid(&bytes, &["compute", "caller_drop"]);
    assert!(contains_call_to(&bytes, 0), "Must contain Call(0)");
    assert!(bytes.contains(&0x1A), "Must contain Drop opcode (0x1A)");
}

#[test]
fn test_wasm_call_nested_chained_calls() {
    let backend = WasmBackend::new();
    let mut module = Module::new();

    // 1. Function C: fn leaf(%0: i64) -> i64 { bb0: %1 = 1; %2 = add %0, %1; ret %2 }
    let mut f_c = Function::new("leaf", Type::I64);
    f_c.add_param(0, Type::I64);
    let mut b_c = BasicBlock::new(0, Terminator::Ret(Some(2)));
    b_c.push(Instruction::new(Some(1), Type::I64, Op::ConstI64(1), 1));
    b_c.push(Instruction::new(Some(2), Type::I64, Op::Add(0, 1), 2));
    f_c.add_block(b_c);
    module.add_function(f_c);

    // 2. Function B: fn mid(%0: i64) -> i64 { bb0: %1 = call leaf %0; %2 = 2; %3 = mul %1, %2; ret %3 }
    let mut f_b = Function::new("mid", Type::I64);
    f_b.add_param(0, Type::I64);
    let mut b_b = BasicBlock::new(0, Terminator::Ret(Some(3)));
    b_b.push(Instruction::new(
        Some(1),
        Type::I64,
        Op::Call("leaf".to_string(), vec![0]),
        1,
    ));
    b_b.push(Instruction::new(Some(2), Type::I64, Op::ConstI64(2), 2));
    b_b.push(Instruction::new(Some(3), Type::I64, Op::Mul(1, 2), 3));
    f_b.add_block(b_b);
    module.add_function(f_b);

    // 3. Function A: fn top(%0: i64) -> i64 { bb0: %1 = call mid %0; ret %1 }
    let mut f_a = Function::new("top", Type::I64);
    f_a.add_param(0, Type::I64);
    let mut b_a = BasicBlock::new(0, Terminator::Ret(Some(1)));
    b_a.push(Instruction::new(
        Some(1),
        Type::I64,
        Op::Call("mid".to_string(), vec![0]),
        1,
    ));
    f_a.add_block(b_a);
    module.add_function(f_a);

    let bytes = backend.compile(&module).expect("compilation failed for chained calls");
    assert_wasm_module_valid(&bytes, &["leaf", "mid", "top"]);

    // leaf = 0, mid = 1, top = 2.
    assert!(contains_call_to(&bytes, 0), "Must emit Call(0) for leaf call in mid");
    assert!(contains_call_to(&bytes, 1), "Must emit Call(1) for mid call in top");
}

#[test]
fn test_wasm_call_with_runtime_imports_offset() {
    let backend = WasmBackend::new();
    let mut module = Module::new();

    // 1. Callee allocating a buffer (requires runtime imports "env" -> num_imports = 11)
    let mut f_callee = Function::new("callee_buf", Type::I64);
    let mut b_callee = BasicBlock::new(0, Terminator::Ret(Some(1)));
    b_callee.push(Instruction::new(Some(0), Type::I64, Op::ConstI64(64), 1));
    b_callee.push(Instruction::new(Some(1), Type::Buf, Op::BufAlloc(0), 2));
    f_callee.add_block(b_callee);
    module.add_function(f_callee);

    // 2. Caller calling callee_buf
    let mut f_caller = Function::new("caller_buf", Type::I64);
    let mut b_caller = BasicBlock::new(0, Terminator::Ret(Some(0)));
    b_caller.push(Instruction::new(
        Some(0),
        Type::I64,
        Op::Call("callee_buf".to_string(), vec![]),
        1,
    ));
    f_caller.add_block(b_caller);
    module.add_function(f_caller);

    let bytes = backend.compile(&module).expect("compilation failed with imports");
    assert_wasm_module_valid(&bytes, &["callee_buf", "caller_buf"]);

    // Since imports are present, import count is 11.
    // callee_buf is at index 11 (0x0B in LEB128).
    // Instruction::Call in caller_buf MUST be Call(11), NOT Call(0)!
    assert!(
        contains_call_to(&bytes, 11),
        "Expected Call(11) reflecting the 11 imported runtime functions"
    );
}

#[test]
fn test_wasm_call_forward_reference() {
    let backend = WasmBackend::new();
    let mut module = Module::new();

    // Caller is added FIRST in module, calling callee which is added SECOND
    let mut f_caller = Function::new("caller_first", Type::I64);
    let mut b_caller = BasicBlock::new(0, Terminator::Ret(Some(0)));
    b_caller.push(Instruction::new(
        Some(0),
        Type::I64,
        Op::Call("callee_second".to_string(), vec![]),
        1,
    ));
    f_caller.add_block(b_caller);
    module.add_function(f_caller);

    let mut f_callee = Function::new("callee_second", Type::I64);
    let mut b_callee = BasicBlock::new(0, Terminator::Ret(Some(0)));
    b_callee.push(Instruction::new(Some(0), Type::I64, Op::ConstI64(42), 1));
    f_callee.add_block(b_callee);
    module.add_function(f_callee);

    let bytes = backend.compile(&module).expect("forward call resolution failed");
    assert_wasm_module_valid(&bytes, &["caller_first", "callee_second"]);

    // caller_first is 0, callee_second is 1. Call must be Call(1).
    assert!(contains_call_to(&bytes, 1), "Must emit Call(1) for forward reference");
}

#[test]
fn test_wasm_call_unknown_function_error() {
    let backend = WasmBackend::new();
    let mut module = Module::new();

    let mut f_caller = Function::new("caller_err", Type::I64);
    let mut b_caller = BasicBlock::new(0, Terminator::Ret(Some(0)));
    b_caller.push(Instruction::new(
        Some(0),
        Type::I64,
        Op::Call("non_existent_func".to_string(), vec![]),
        1,
    ));
    f_caller.add_block(b_caller);
    module.add_function(f_caller);

    let result = backend.compile(&module);
    assert!(result.is_err(), "Expected error for calling undefined function");
    let err_msg = format!("{}", result.unwrap_err());
    assert!(
        err_msg.contains("Unknown function in call: non_existent_func")
            || err_msg.contains("non_existent_func"),
        "Error message should mention unknown function name, got: {}",
        err_msg
    );
}

#[test]
fn test_wasm_call_from_parsed_ssa_text() {
    let backend = WasmBackend::new();
    let ssa = r#"
fn square(%0: i64) -> i64 {
bb0:
    %1 = mul %0 %0
    ret %1
}

fn sum_of_squares(%0: i64, %1: i64) -> i64 {
bb0:
    %2 = call square %0
    %3 = call square %1
    %4 = add %2 %3
    ret %4
}
"#;
    let module = knr_core::parse_module(ssa).expect("Failed to parse SSA textual module");
    let bytes = backend.compile(&module).expect("Failed to compile parsed SSA module to WASM");
    assert_wasm_module_valid(&bytes, &["square", "sum_of_squares"]);
    assert!(contains_call_to(&bytes, 0), "Must contain Call(0) targeting square");
}
