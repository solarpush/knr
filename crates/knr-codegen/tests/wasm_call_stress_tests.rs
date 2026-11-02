use knr_codegen::WasmBackend;
use knr_core::{BasicBlock, Function, Instruction, Module, Op, Terminator, Type};

const WASM_MAGIC: [u8; 4] = [0x00, 0x61, 0x73, 0x6D];
const WASM_VERSION_1: [u8; 4] = [0x01, 0x00, 0x00, 0x00];

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

// 1. Forward Call Chain: fn_a calls fn_b, fn_b calls fn_c, fn_c calls fn_d.
// Defined in forward order (fn_a is index 0, fn_d is index 3).
#[test]
fn test_stress_forward_calls_chain() {
    let backend = WasmBackend::new();
    let ssa = r#"
fn fn_a(%0: i64) -> i64 {
bb0:
    %1 = call fn_b %0
    ret %1
}

fn fn_b(%0: i64) -> i64 {
bb0:
    %1 = call fn_c %0
    ret %1
}

fn fn_c(%0: i64) -> i64 {
bb0:
    %1 = call fn_d %0
    ret %1
}

fn fn_d(%0: i64) -> i64 {
bb0:
    %1 = const_i64 100
    %2 = add %0 %1
    ret %2
}
"#;
    let module = knr_core::parse_module(ssa).expect("parse failed");
    let bytes = backend.compile(&module).expect("compilation failed");
    assert_wasm_module_valid(&bytes, &["fn_a", "fn_b", "fn_c", "fn_d"]);

    // fn_a = 0, fn_b = 1, fn_c = 2, fn_d = 3
    assert!(contains_call_to(&bytes, 1), "fn_a must call fn_b (index 1)");
    assert!(contains_call_to(&bytes, 2), "fn_b must call fn_c (index 2)");
    assert!(contains_call_to(&bytes, 3), "fn_c must call fn_d (index 3)");
}

/// 2. Direct Recursion: factorial calling factorial.
#[test]
fn test_stress_direct_recursion_factorial() {
    let backend = WasmBackend::new();
    let ssa = r#"
fn fact(%0: i64) -> i64 {
bb0:
    %1 = const_i64 2
    %2 = cmp_lt %0 %1
    br_if %2 bb1 bb2
bb1:
    %7 = const_i64 1
    ret %7
bb2:
    %3 = const_i64 1
    %4 = sub %0 %3
    %5 = call fact %4
    %6 = mul %0 %5
    ret %6
}
"#;
    let module = knr_core::parse_module(ssa).expect("parse failed");
    let bytes = backend.compile(&module).expect("compilation failed");
    assert_wasm_module_valid(&bytes, &["fact"]);

    // fact is at index 0. It must call index 0 recursively.
    assert!(contains_call_to(&bytes, 0), "fact must call itself (index 0)");
}

/// 3. Mutual Recursion: is_even calling is_odd, is_odd calling is_even.
#[test]
fn test_stress_mutual_recursion() {
    let backend = WasmBackend::new();
    let ssa = r#"
fn is_even(%0: i64) -> bool {
bb0:
    %1 = const_i64 0
    %2 = cmp_eq %0 %1
    br_if %2 bb1 bb2
bb1:
    %3 = const_bool true
    ret %3
bb2:
    %4 = const_i64 1
    %5 = sub %0 %4
    %6 = call is_odd %5
    ret %6
}

fn is_odd(%0: i64) -> bool {
bb0:
    %1 = const_i64 0
    %2 = cmp_eq %0 %1
    br_if %2 bb1 bb2
bb1:
    %3 = const_bool false
    ret %3
bb2:
    %4 = const_i64 1
    %5 = sub %0 %4
    %6 = call is_even %5
    ret %6
}
"#;
    let module = knr_core::parse_module(ssa).expect("parse failed");
    let bytes = backend.compile(&module).expect("compilation failed");
    assert_wasm_module_valid(&bytes, &["is_even", "is_odd"]);

    // is_even = 0, is_odd = 1
    assert!(contains_call_to(&bytes, 1), "is_even must call is_odd (index 1)");
    assert!(contains_call_to(&bytes, 0), "is_odd must call is_even (index 0)");
}

// 4. Multiple calls in the SAME basic block:
// Mix of void calls, value calls stored to registers, dropped calls, and calls chaining results.
#[test]
fn test_stress_multiple_calls_same_basic_block() {
    let backend = WasmBackend::new();
    let mut module = Module::new();

    // Callee 1: void helper
    let mut f_void = Function::new("side_effect", Type::Void);
    f_void.add_block(BasicBlock::new(0, Terminator::Ret(None)));
    module.add_function(f_void);

    // Callee 2: double(x) -> i64
    let mut f_dbl = Function::new("double", Type::I64);
    f_dbl.add_param(0, Type::I64);
    let mut b_dbl = BasicBlock::new(0, Terminator::Ret(Some(1)));
    b_dbl.push(Instruction::new(Some(1), Type::I64, Op::Add(0, 0), 1));
    f_dbl.add_block(b_dbl);
    module.add_function(f_dbl);

    // Callee 3: add(x, y) -> i64
    let mut f_add = Function::new("add", Type::I64);
    f_add.add_param(0, Type::I64);
    f_add.add_param(1, Type::I64);
    let mut b_add = BasicBlock::new(0, Terminator::Ret(Some(2)));
    b_add.push(Instruction::new(Some(2), Type::I64, Op::Add(0, 1), 1));
    f_add.add_block(b_add);
    module.add_function(f_add);

    // Caller with 6 calls in single basic block bb0
    let mut f_caller = Function::new("orchestrate", Type::I64);
    f_caller.add_param(0, Type::I64); // %0: initial i64
    let mut bb0 = BasicBlock::new(0, Terminator::Ret(Some(6)));

    // 1. call side_effect (void)
    bb0.push(Instruction::new(
        None,
        Type::Void,
        Op::Call("side_effect".to_string(), vec![]),
        1,
    ));
    // 2. %1 = call double %0
    bb0.push(Instruction::new(
        Some(1),
        Type::I64,
        Op::Call("double".to_string(), vec![0]),
        2,
    ));
    // 3. call side_effect (void again)
    bb0.push(Instruction::new(
        None,
        Type::Void,
        Op::Call("side_effect".to_string(), vec![]),
        3,
    ));
    // 4. call double %1 (dst = None -> dropped!)
    bb0.push(Instruction::new(
        None,
        Type::I64,
        Op::Call("double".to_string(), vec![1]),
        4,
    ));
    // 5. %2 = call double %1
    bb0.push(Instruction::new(
        Some(2),
        Type::I64,
        Op::Call("double".to_string(), vec![1]),
        5,
    ));
    // 6. %6 = call add %1 %2
    bb0.push(Instruction::new(
        Some(6),
        Type::I64,
        Op::Call("add".to_string(), vec![1, 2]),
        6,
    ));

    f_caller.add_block(bb0);
    module.add_function(f_caller);

    let bytes = backend.compile(&module).expect("compilation failed for multiple calls in block");
    assert_wasm_module_valid(&bytes, &["side_effect", "double", "add", "orchestrate"]);

    // side_effect = 0, double = 1, add = 2, orchestrate = 3
    assert!(contains_call_to(&bytes, 0), "Must contain Call(0) for side_effect");
    assert!(contains_call_to(&bytes, 1), "Must contain Call(1) for double");
    assert!(contains_call_to(&bytes, 2), "Must contain Call(2) for add");
    assert!(bytes.contains(&0x1A), "Must contain Drop opcode (0x1A) for dropped call");
}

/// 5. Zero-argument function call.
#[test]
fn test_stress_call_zero_args() {
    let backend = WasmBackend::new();
    let ssa = r#"
fn get_constant() -> i64 {
bb0:
    %0 = const_i64 999
    ret %0
}

fn caller_zero_args() -> i64 {
bb0:
    %0 = call get_constant
    ret %0
}
"#;
    let module = knr_core::parse_module(ssa).expect("parse failed");
    let bytes = backend.compile(&module).expect("compilation failed");
    assert_wasm_module_valid(&bytes, &["get_constant", "caller_zero_args"]);
    assert!(contains_call_to(&bytes, 0), "Must call get_constant (index 0)");
}

/// 6. Call inside a loop body (multi-block CFG dispatch).
#[test]
fn test_stress_call_in_loop_body() {
    let backend = WasmBackend::new();
    let ssa = r#"
fn step(%0: i64) -> i64 {
bb0:
    %1 = const_i64 1
    %2 = add %0 %1
    ret %2
}

fn loop_with_calls(%0: i64) -> i64 {
bb0:
    %1 = const_i64 0
    br bb1
bb1:
    %2 = call step %1
    %3 = cmp_lt %2 %0
    br_if %3 bb1 bb2
bb2:
    ret %2
}
"#;
    let module = knr_core::parse_module(ssa).expect("parse failed");
    let bytes = backend.compile(&module).expect("compilation failed");
    assert_wasm_module_valid(&bytes, &["step", "loop_with_calls"]);
    assert!(contains_call_to(&bytes, 0), "Must call step inside loop");
}

/// 7. Multi-argument call with 5 arguments of mixed types.
#[test]
fn test_stress_multi_argument_mixed_types() {
    let backend = WasmBackend::new();
    let mut module = Module::new();

    // Callee with 4 params: (i64, f64, bool, i64) -> i64
    let mut f_callee = Function::new("complex_calc", Type::I64);
    f_callee.add_param(0, Type::I64);
    f_callee.add_param(1, Type::F64);
    f_callee.add_param(2, Type::Bool);
    f_callee.add_param(3, Type::I64);
    let mut b_callee = BasicBlock::new(0, Terminator::Ret(Some(4)));
    b_callee.push(Instruction::new(Some(4), Type::I64, Op::Add(0, 3), 1));
    f_callee.add_block(b_callee);
    module.add_function(f_callee);

    // Caller
    let mut f_caller = Function::new("caller_complex", Type::I64);
    let mut b_caller = BasicBlock::new(0, Terminator::Ret(Some(4)));
    b_caller.push(Instruction::new(Some(0), Type::I64, Op::ConstI64(10), 1));
    b_caller.push(Instruction::new(Some(1), Type::F64, Op::ConstF64(2.5), 2));
    b_caller.push(Instruction::new(Some(2), Type::Bool, Op::ConstBool(true), 3));
    b_caller.push(Instruction::new(Some(3), Type::I64, Op::ConstI64(20), 4));
    b_caller.push(Instruction::new(
        Some(4),
        Type::I64,
        Op::Call("complex_calc".to_string(), vec![0, 1, 2, 3]),
        5,
    ));
    f_caller.add_block(b_caller);
    module.add_function(f_caller);

    let bytes = backend.compile(&module).expect("compilation failed for multi-arg call");
    assert_wasm_module_valid(&bytes, &["complex_calc", "caller_complex"]);
    assert!(contains_call_to(&bytes, 0), "Must contain Call(0)");
}
