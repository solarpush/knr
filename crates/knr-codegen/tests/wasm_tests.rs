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

