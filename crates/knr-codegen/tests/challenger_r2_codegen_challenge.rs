use knr_codegen::{CraneliftBackend, WasmBackend};
use knr_core::{BasicBlock, Function, Instruction, Module, Op, Terminator, Type};

#[test]
fn test_adversarial_cranelift_aot_20_functions_many_strings() {
    let mut backend = CraneliftBackend::new().expect("Failed to initialize CraneliftBackend");
    let mut module = Module::new();

    // Create 20 functions, each with 5 string constants (including empty string, duplicates, symbols)
    for f_idx in 0..20 {
        let func_name = format!("adversarial_func_{f_idx}");
        let mut func = Function::new(&func_name, Type::Str);
        let mut block = BasicBlock::new(0, Terminator::Ret(Some(4)));

        block.push(Instruction::new(Some(0), Type::Str, Op::ConstStr("constant_prefix_same_across_funcs".to_string()), 1));
        block.push(Instruction::new(Some(1), Type::Str, Op::ConstStr("".to_string()), 2)); // empty string
        block.push(Instruction::new(Some(2), Type::Str, Op::ConstStr("hello \n\t\"'\\ world \0 with special chars".to_string()), 3));
        block.push(Instruction::new(Some(3), Type::Str, Op::ConstStr(format!("unique_str_for_func_{f_idx}")), 4));
        block.push(Instruction::new(Some(4), Type::Str, Op::ConstStr("constant_prefix_same_across_funcs".to_string()), 5)); // duplicate within same func

        func.add_block(block);
        module.add_function(func);
    }

    let obj_bytes = backend.compile_object(&module).expect("Failed to compile 20-function module with strings");
    assert!(!obj_bytes.is_empty());
    assert_eq!(&obj_bytes[0..4], &[0x7F, b'E', b'L', b'F']);

    // Check ELF relocatable type
    let elf_type = u16::from_le_bytes([obj_bytes[16], obj_bytes[17]]);
    assert_eq!(elf_type, 1, "ELF file must be relocatable (ET_REL=1)");

    // Check that function symbols are present in the object file
    let obj_str = String::from_utf8_lossy(&obj_bytes);
    for f_idx in 0..20 {
        let sym = format!("adversarial_func_{f_idx}");
        assert!(obj_str.contains(&sym), "Missing function symbol {sym}");
    }
}

#[test]
fn test_adversarial_cranelift_consecutive_module_compilations() {
    let mut backend = CraneliftBackend::new().unwrap();

    // Compile 5 different modules consecutively on the same CraneliftBackend instance
    for i in 0..5 {
        let mut module = Module::new();
        let mut func = Function::new(format!("module_{i}_func"), Type::Str);
        let mut block = BasicBlock::new(0, Terminator::Ret(Some(0)));
        block.push(Instruction::new(Some(0), Type::Str, Op::ConstStr(format!("string in module {i}")), 1));
        func.add_block(block);
        module.add_function(func);

        let obj = backend.compile_object(&module).expect("Consecutive compile_object failed");
        assert_eq!(&obj[0..4], &[0x7F, b'E', b'L', b'F']);
    }
}

#[test]
fn test_adversarial_cranelift_jit_string_pointer_dereference() {
    let mut backend = CraneliftBackend::new().unwrap();

    let target_str = "Knr SSA JIT String Test 12345";
    let mut func = Function::new("return_str_ptr", Type::Str);
    let mut block = BasicBlock::new(0, Terminator::Ret(Some(0)));
    block.push(Instruction::new(Some(0), Type::Str, Op::ConstStr(target_str.to_string()), 1));
    func.add_block(block);

    let code_ptr = backend.compile_jit(&func).expect("JIT compilation failed");
    let get_str_ptr: extern "C" fn() -> *const u8 = unsafe { std::mem::transmute(code_ptr) };

    let ptr = get_str_ptr();
    assert!(!ptr.is_null(), "Returned string pointer must not be null");

    // Read bytes from the pointer
    let slice = unsafe { std::slice::from_raw_parts(ptr, target_str.len()) };
    assert_eq!(slice, target_str.as_bytes(), "JIT returned string content mismatch");
}

#[test]
fn test_adversarial_wasm_single_block_loops_and_branches() {
    let backend = WasmBackend::new();

    // 1. Single-block infinite loop with instructions before br:
    // fn compute_and_loop() -> void
    // bb0:
    //   %0 = const_i64 100
    //   %1 = const_i64 200
    //   %2 = add %0, %1
    //   br bb0
    let mut f1 = Function::new("compute_and_loop", Type::Void);
    let mut b1 = BasicBlock::new(0, Terminator::Br(0));
    b1.push(Instruction::new(Some(0), Type::I64, Op::ConstI64(100), 1));
    b1.push(Instruction::new(Some(1), Type::I64, Op::ConstI64(200), 2));
    b1.push(Instruction::new(Some(2), Type::I64, Op::Add(0, 1), 3));
    f1.add_block(b1);

    let bytes1 = backend.compile_function(&f1).expect("Failed to compile single block loop");
    assert_eq!(&bytes1[0..4], &[0x00, 0x61, 0x73, 0x6D]);
    // Opcode 0x03 is Loop, 0x0C is Br
    assert!(bytes1.contains(&0x03), "Must emit Wasm Loop instruction (0x03)");
    assert!(bytes1.contains(&0x0C), "Must emit Wasm Br instruction (0x0C)");
    assert!(!bytes1[bytes1.len() - 4..].contains(&0x0F), "Must not end with Return opcode (0x0F)");

    // 2. Single-block conditional self loop:
    // fn cond_loop(flag: bool) -> void
    // bb0:
    //   br_if %0, bb0, bb0
    let mut f2 = Function::new("cond_loop", Type::Void);
    f2.add_param(0, Type::Bool);
    let b2 = BasicBlock::new(0, Terminator::BrIf {
        cond: 0,
        then_blk: 0,
        else_blk: 0,
    });
    f2.add_block(b2);

    let bytes2 = backend.compile_function(&f2).expect("Failed to compile single block cond loop");
    assert_eq!(&bytes2[0..4], &[0x00, 0x61, 0x73, 0x6D]);
    assert!(bytes2.contains(&0x03), "Must emit Wasm Loop instruction (0x03)");
    assert!(bytes2.contains(&0x0C), "Must emit Wasm Br instruction (0x0C)");
    assert!(!bytes2[bytes2.len() - 4..].contains(&0x0F), "Must not end with Return opcode (0x0F)");
}

#[test]
fn test_adversarial_wasm_mixed_module_straight_line_and_loop() {
    let backend = WasmBackend::new();
    let mut module = Module::new();

    // Normal straight-line function
    let mut f_norm = Function::new("f_norm", Type::I64);
    let mut b_norm = BasicBlock::new(0, Terminator::Ret(Some(0)));
    b_norm.push(Instruction::new(Some(0), Type::I64, Op::ConstI64(42), 1));
    f_norm.add_block(b_norm);
    module.add_function(f_norm);

    // Single-block loop function
    let mut f_loop = Function::new("f_loop", Type::Void);
    let b_loop = BasicBlock::new(0, Terminator::Br(0));
    f_loop.add_block(b_loop);
    module.add_function(f_loop);

    let bytes = backend.compile(&module).expect("Failed to compile mixed module");
    assert_eq!(&bytes[0..4], &[0x00, 0x61, 0x73, 0x6D]);
    assert!(bytes.contains(&0x03), "Must contain Loop opcode");
    assert!(bytes.contains(&0x0C), "Must contain Br opcode");
    assert!(bytes.contains(&0x0F), "Must contain Return opcode for f_norm");
}
