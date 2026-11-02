use knr_codegen::{CraneliftBackend, WasmBackend};
use knr_core::{BasicBlock, Function, Instruction, Module, Op, Terminator, Type};

// ============================================================================
// HELPER: BUILD TEST SSA FUNCTIONS
// ============================================================================

fn make_ret_const_i64_func(name: &str, val: i64) -> Function {
    Function {
        name: name.to_string(),
        params: vec![],
        ret_ty: Type::I64,
        effects: vec![],
        blocks: vec![BasicBlock {
            id: 0,
            instructions: vec![Instruction {
                dst: Some(0),
                ty: Type::I64,
                op: Op::ConstI64(val),
                line: 1,
            }],
            terminator: Terminator::Ret(Some(0)),
        }],
    }
}

fn make_str_func(name: &str, s: &str) -> Function {
    Function {
        name: name.to_string(),
        params: vec![],
        ret_ty: Type::Str,
        effects: vec![],
        blocks: vec![BasicBlock {
            id: 0,
            instructions: vec![Instruction {
                dst: Some(0),
                ty: Type::Str,
                op: Op::ConstStr(s.to_string()),
                line: 1,
            }],
            terminator: Terminator::Ret(Some(0)),
        }],
    }
}

fn make_countdown_loop_func() -> Function {
    // fn loop_countdown(n: i64) -> i64
    // bb0:
    //   %1 = const_i64 0
    //   %2 = const_i64 1
    //   br bb1
    // bb1:
    //   %3 = cmp_eq %0, %1
    //   br_if %3, bb3, bb2
    // bb2:
    //   %4 = sub %0, %2
    //   %0 = ... in SSA we pass registers, but let's do a diamond or linear loop
    // Let's create standard diamond branch:
    // fn max(a: i64, b: i64) -> i64
    // bb0:
    //   %2 = cmp_lt %0, %1
    //   br_if %2, bb1, bb2
    // bb1:
    //   ret %1
    // bb2:
    //   ret %0
    Function {
        name: "max_val".to_string(),
        params: vec![(0, Type::I64), (1, Type::I64)],
        ret_ty: Type::I64,
        effects: vec![],
        blocks: vec![
            BasicBlock {
                id: 0,
                instructions: vec![Instruction {
                    dst: Some(2),
                    ty: Type::Bool,
                    op: Op::CmpLt(0, 1),
                    line: 1,
                }],
                terminator: Terminator::BrIf {
                    cond: 2,
                    then_blk: 1,
                    else_blk: 2,
                },
            },
            BasicBlock {
                id: 1,
                instructions: vec![],
                terminator: Terminator::Ret(Some(1)),
            },
            BasicBlock {
                id: 2,
                instructions: vec![],
                terminator: Terminator::Ret(Some(0)),
            },
        ],
    }
}

// ============================================================================
// 1. WASM BYTECODE VALIDITY & SECTION ORDER VERIFICATION
// ============================================================================

#[test]
fn test_wasm_binary_magic_version_and_section_order() {
    let mut module = Module::new();
    module.add_function(make_ret_const_i64_func("func_const", 42));
    module.add_function(make_str_func("func_str", "hello world"));

    let backend = WasmBackend::new();
    let wasm_bytes = backend.compile(&module).expect("Wasm compilation failed");

    // 1. Magic bytes: \0asm
    assert!(wasm_bytes.len() >= 8);
    assert_eq!(&wasm_bytes[0..4], b"\0asm");
    // 2. Version 1: 0x01, 0x00, 0x00, 0x00
    assert_eq!(&wasm_bytes[4..8], &[0x01, 0x00, 0x00, 0x00]);

    // Parse Wasm sections and verify strict monotonically increasing order
    let mut offset = 8;
    let mut previous_section_id: Option<u8> = None;
    let mut seen_sections: Vec<(u8, usize)> = Vec::new();

    while offset < wasm_bytes.len() {
        let section_id = wasm_bytes[offset];
        offset += 1;

        // Read LEB128 section length
        let mut section_len: usize = 0;
        let mut shift = 0;
        loop {
            assert!(offset < wasm_bytes.len(), "Unexpected EOF in LEB128 length");
            let byte = wasm_bytes[offset];
            offset += 1;
            section_len |= ((byte & 0x7F) as usize) << shift;
            if byte & 0x80 == 0 {
                break;
            }
            shift += 7;
        }

        assert!(
            offset + section_len <= wasm_bytes.len(),
            "Section {section_id} length {section_len} exceeds remaining bytes"
        );
        seen_sections.push((section_id, section_len));

        // In WebAssembly, standard non-custom (id > 0) sections must appear in strictly ascending order
        if section_id != 0 {
            if let Some(prev) = previous_section_id {
                assert!(
                    section_id > prev,
                    "Wasm section ordering violation: section {section_id} followed section {prev}"
                );
            }
            previous_section_id = Some(section_id);
        }

        offset += section_len;
    }

    assert_eq!(offset, wasm_bytes.len(), "Wasm bytecode had trailing junk bytes");

    let section_ids: Vec<u8> = seen_sections.iter().map(|(id, _)| *id).collect();
    println!("Emitted Wasm section IDs: {section_ids:?}");

    // Type(1), Function(3), Memory(5), Export(7), Code(10), Data(11)
    assert!(section_ids.contains(&1), "Missing Type section (1)");
    assert!(section_ids.contains(&3), "Missing Function section (3)");
    assert!(section_ids.contains(&5), "Missing Memory section (5)");
    assert!(section_ids.contains(&7), "Missing Export section (7)");
    assert!(section_ids.contains(&10), "Missing Code section (10)");
    assert!(section_ids.contains(&11), "Missing Data section (11)");
}

#[test]
fn test_wasm_branching_execution_model() {
    let func = make_countdown_loop_func();
    let backend = WasmBackend::new();
    let wasm_bytes = backend.compile_function(&func).expect("Wasm compile failed");

    assert_eq!(&wasm_bytes[0..4], b"\0asm");
    assert_eq!(&wasm_bytes[4..8], &[0x01, 0x00, 0x00, 0x00]);
}

#[test]
fn test_wasm_single_block_loop_converted_to_return_defect() {
    // Function with 1 basic block that loops to itself: bb0: br bb0
    let func = Function {
        name: "infinite_self_loop".to_string(),
        params: vec![],
        ret_ty: Type::Void,
        effects: vec![],
        blocks: vec![BasicBlock {
            id: 0,
            instructions: vec![],
            terminator: Terminator::Br(0),
        }],
    };

    let backend = WasmBackend::new();
    let wasm_bytes = backend.compile_function(&func).expect("Wasm compile failed");

    // In Wasm, the code section should contain the dispatch loop.
    // Single-block branch must NOT be compiled to Return (0x0F).
    assert!(
        !wasm_bytes.contains(&0x0F),
        "Single-block branch 'br bb0' must NOT be compiled to Return (0x0F)"
    );
    // 0x03 is Loop opcode, 0x0C is Br opcode
    assert!(
        wasm_bytes.contains(&0x03),
        "Single-block branch 'br bb0' must contain Loop instruction (0x03)"
    );
    assert!(
        wasm_bytes.contains(&0x0C),
        "Single-block branch 'br bb0' must contain Br instruction (0x0C)"
    );
}

// ============================================================================
// 2. CRANELIFT JIT EXECUTION CORRECTNESS
// ============================================================================

#[test]
fn test_cranelift_jit_i64_arithmetic_exhaustive() {
    let mut backend = CraneliftBackend::new().unwrap();

    // Test add, sub, mul, div with positive and negative numbers
    let func = Function {
        name: "math_combo".to_string(),
        params: vec![(0, Type::I64), (1, Type::I64)],
        ret_ty: Type::I64,
        effects: vec![],
        blocks: vec![BasicBlock {
            id: 0,
            instructions: vec![
                Instruction {
                    dst: Some(2),
                    ty: Type::I64,
                    op: Op::Add(0, 1),
                    line: 1,
                },
                Instruction {
                    dst: Some(3),
                    ty: Type::I64,
                    op: Op::Mul(2, 0),
                    line: 2,
                },
                Instruction {
                    dst: Some(4),
                    ty: Type::I64,
                    op: Op::Div(3, 1),
                    line: 3,
                },
                Instruction {
                    dst: Some(5),
                    ty: Type::I64,
                    op: Op::Sub(4, 1),
                    line: 4,
                },
            ],
            terminator: Terminator::Ret(Some(5)),
        }],
    };

    let code_ptr = backend.compile_jit(&func).expect("JIT compile failed");
    let math_fn: fn(i64, i64) -> i64 = unsafe { std::mem::transmute(code_ptr) };

    // ((a + b) * a / b) - b
    // ((10 + 2) * 10 / 2) - 2 = (12 * 10 / 2) - 2 = 60 - 2 = 58
    assert_eq!(math_fn(10, 2), 58);
    // ((20 + 4) * 20 / 4) - 4 = (24 * 20 / 4) - 4 = 120 - 4 = 116
    assert_eq!(math_fn(20, 4), 116);
    // ((-6 + 3) * -6 / 3) - 3 = (-3 * -6 / 3) - 3 = (18 / 3) - 3 = 6 - 3 = 3
    assert_eq!(math_fn(-6, 3), 3);
}

#[test]
fn test_cranelift_jit_diamond_branching() {
    let mut backend = CraneliftBackend::new().unwrap();
    let func = make_countdown_loop_func(); // max(a, b)

    let code_ptr = backend.compile_jit(&func).expect("JIT compile failed");
    let max_fn: fn(i64, i64) -> i64 = unsafe { std::mem::transmute(code_ptr) };

    assert_eq!(max_fn(10, 20), 20);
    assert_eq!(max_fn(99, 42), 99);
    assert_eq!(max_fn(-5, -10), -5);
    assert_eq!(max_fn(7, 7), 7);
}

#[test]
fn test_cranelift_jit_f64_math() {
    let mut backend = CraneliftBackend::new().unwrap();

    let func = Function {
        name: "f64_math".to_string(),
        params: vec![(0, Type::F64), (1, Type::F64)],
        ret_ty: Type::F64,
        effects: vec![],
        blocks: vec![BasicBlock {
            id: 0,
            instructions: vec![
                Instruction {
                    dst: Some(2),
                    ty: Type::F64,
                    op: Op::Add(0, 1),
                    line: 1,
                },
                Instruction {
                    dst: Some(3),
                    ty: Type::F64,
                    op: Op::Mul(2, 0),
                    line: 2,
                },
                Instruction {
                    dst: Some(4),
                    ty: Type::F64,
                    op: Op::Div(3, 1),
                    line: 3,
                },
            ],
            terminator: Terminator::Ret(Some(4)),
        }],
    };

    let code_ptr = backend.compile_jit(&func).expect("JIT compile failed");
    let f64_fn: fn(f64, f64) -> f64 = unsafe { std::mem::transmute(code_ptr) };

    // ((2.5 + 0.5) * 2.5) / 0.5 = (3.0 * 2.5) / 0.5 = 7.5 / 0.5 = 15.0
    let res = f64_fn(2.5, 0.5);
    assert!((res - 15.0).abs() < 1e-9);
}

#[test]
fn test_cranelift_jit_buffer_write_read() {
    let mut backend = CraneliftBackend::new().unwrap();

    // Allocates buffer, writes byte at offset 5, reads byte at offset 5, frees buffer
    let func = Function {
        name: "test_buf".to_string(),
        params: vec![],
        ret_ty: Type::I64,
        effects: vec![],
        blocks: vec![BasicBlock {
            id: 0,
            instructions: vec![
                Instruction {
                    dst: Some(0),
                    ty: Type::I64,
                    op: Op::ConstI64(64),
                    line: 1,
                },
                Instruction {
                    dst: Some(1),
                    ty: Type::Buf,
                    op: Op::BufAlloc(0),
                    line: 2,
                },
                Instruction {
                    dst: Some(2),
                    ty: Type::I64,
                    op: Op::ConstI64(5),
                    line: 3,
                },
                Instruction {
                    dst: Some(3),
                    ty: Type::I64,
                    op: Op::ConstI64(0x7F),
                    line: 4,
                },
                Instruction {
                    dst: Some(4),
                    ty: Type::Buf,
                    op: Op::BufWriteU8(1, 2, 3),
                    line: 5,
                },
                Instruction {
                    dst: Some(5),
                    ty: Type::I64,
                    op: Op::BufReadU8(4, 2),
                    line: 6,
                },
                Instruction {
                    dst: None,
                    ty: Type::Void,
                    op: Op::Free(4),
                    line: 7,
                },
            ],
            terminator: Terminator::Ret(Some(5)),
        }],
    };

    let code_ptr = backend.compile_jit(&func).expect("JIT compile failed");
    let buf_fn: fn() -> i64 = unsafe { std::mem::transmute(code_ptr) };

    assert_eq!(buf_fn(), 0x7F);
}

// ============================================================================
// 3. OBJECT FILE GENERATION & ELF VERIFICATION
// ============================================================================

#[test]
fn test_cranelift_compile_object_elf_header_and_symbols() {
    let mut backend = CraneliftBackend::new().unwrap();

    let mut module = Module::new();
    module.add_function(make_ret_const_i64_func("my_exported_func", 12345));

    let obj_bytes = backend
        .compile_object(&module)
        .expect("Object compilation failed");

    assert!(
        obj_bytes.len() > 64,
        "Object file bytes too short: {}",
        obj_bytes.len()
    );

    // On Linux x86_64, Cranelift object output should be a valid ELF binary:
    // Magic: 0x7F 'E' 'L' 'F'
    assert_eq!(
        &obj_bytes[0..4],
        &[0x7F, b'E', b'L', b'F'],
        "Object file must start with ELF magic \\x7fELF"
    );

    // Class: 2 (ELFCLASS64)
    assert_eq!(obj_bytes[4], 2, "ELF file should be 64-bit");
    // Data: 1 (ELFDATA2LSB - little endian)
    assert_eq!(obj_bytes[5], 1, "ELF file should be little endian");
    // Version: 1 (EV_CURRENT)
    assert_eq!(obj_bytes[6], 1, "ELF version should be 1");
    // Type: 1 (ET_REL - relocatable object file)
    let elf_type = u16::from_le_bytes([obj_bytes[16], obj_bytes[17]]);
    assert_eq!(elf_type, 1, "ELF file must be relocatable object file (ET_REL=1)");

    // The function name should be present in the symbol / string table
    let obj_str = String::from_utf8_lossy(&obj_bytes);
    assert!(
        obj_str.contains("my_exported_func"),
        "Object file must contain exported function symbol 'my_exported_func'"
    );
}

#[test]
fn test_cranelift_multi_function_string_constants_defect_check() {
    // Stress test: Multiple functions with ConstStr in the same module
    let mut backend = CraneliftBackend::new().unwrap();

    let mut module = Module::new();
    module.add_function(make_str_func("func_str_1", "String One"));
    module.add_function(make_str_func("func_str_2", "String Two"));

    // Check that compile_object succeeds with namespaced string data symbols
    let res = backend.compile_object(&module);
    assert!(
        res.is_ok(),
        "Expected successful compile_object with multi-function strings, got {res:?}"
    );
    let obj_bytes = res.unwrap();
    assert!(obj_bytes.len() > 64);
    assert_eq!(&obj_bytes[0..4], &[0x7F, b'E', b'L', b'F']);
}
