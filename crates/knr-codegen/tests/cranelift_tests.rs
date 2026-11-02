use knr_codegen::CraneliftBackend;
use knr_core::{BasicBlock, Function, Instruction, Module, Op, Terminator, Type};

#[test]
fn test_cranelift_jit_i64_add() {
    let mut backend = CraneliftBackend::new().expect("backend init failed");

    // fn add(a: i64, b: i64) -> i64
    let mut func = Function::new("add_i64", Type::I64);
    func.add_param(0, Type::I64);
    func.add_param(1, Type::I64);

    let mut bb0 = BasicBlock::new(0, Terminator::Ret(Some(2)));
    bb0.push(Instruction::new(Some(2), Type::I64, Op::Add(0, 1), 1));
    func.add_block(bb0);

    let code_ptr = backend.compile_jit(&func).expect("JIT failed");
    assert!(!code_ptr.is_null());

    let add_fn: extern "C" fn(i64, i64) -> i64 = unsafe { std::mem::transmute(code_ptr) };

    assert_eq!(add_fn(10, 20), 30);
    assert_eq!(add_fn(-5, 15), 10);
    assert_eq!(add_fn(1000, 2000), 3000);
}

#[test]
fn test_cranelift_jit_i64_arithmetic() {
    let mut backend = CraneliftBackend::new().expect("backend init failed");

    // fn calc(a: i64, b: i64, c: i64, d: i64) -> i64
    // %4 = mul %0, %1  (a * b)
    // %5 = sub %4, %2  ((a * b) - c)
    // %6 = div %5, %3  (((a * b) - c) / d)
    // ret %6
    let mut func = Function::new("calc", Type::I64);
    func.add_param(0, Type::I64);
    func.add_param(1, Type::I64);
    func.add_param(2, Type::I64);
    func.add_param(3, Type::I64);

    let mut bb0 = BasicBlock::new(0, Terminator::Ret(Some(6)));
    bb0.push(Instruction::new(Some(4), Type::I64, Op::Mul(0, 1), 1));
    bb0.push(Instruction::new(Some(5), Type::I64, Op::Sub(4, 2), 2));
    bb0.push(Instruction::new(Some(6), Type::I64, Op::Div(5, 3), 3));
    func.add_block(bb0);

    let code_ptr = backend.compile_jit(&func).expect("JIT failed");
    let calc_fn: extern "C" fn(i64, i64, i64, i64) -> i64 =
        unsafe { std::mem::transmute(code_ptr) };

    assert_eq!(calc_fn(10, 5, 10, 4), 10);
    assert_eq!(calc_fn(8, 8, 4, 3), 20);
}

#[test]
fn test_cranelift_jit_f64_arithmetic() {
    let mut backend = CraneliftBackend::new().expect("backend init failed");

    // fn fcalc(a: f64, b: f64, c: f64) -> f64
    // %3 = add %0, %1
    // %4 = mul %3, %2
    // ret %4
    let mut func = Function::new("fcalc", Type::F64);
    func.add_param(0, Type::F64);
    func.add_param(1, Type::F64);
    func.add_param(2, Type::F64);

    let mut bb0 = BasicBlock::new(0, Terminator::Ret(Some(4)));
    bb0.push(Instruction::new(Some(3), Type::F64, Op::Add(0, 1), 1));
    bb0.push(Instruction::new(Some(4), Type::F64, Op::Mul(3, 2), 2));
    func.add_block(bb0);

    let code_ptr = backend.compile_jit(&func).expect("JIT failed");
    let fcalc_fn: extern "C" fn(f64, f64, f64) -> f64 = unsafe { std::mem::transmute(code_ptr) };

    assert_eq!(fcalc_fn(2.0, 3.0, 4.0), 20.0);
    assert_eq!(fcalc_fn(1.5, 0.5, 3.0), 6.0);
}

#[test]
fn test_cranelift_jit_branching_conditional() {
    let mut backend = CraneliftBackend::new().expect("backend init failed");

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

    let code_ptr = backend.compile_jit(&func).expect("JIT failed");
    let max_fn: extern "C" fn(i64, i64) -> i64 = unsafe { std::mem::transmute(code_ptr) };

    assert_eq!(max_fn(10, 25), 25);
    assert_eq!(max_fn(50, 25), 50);
    assert_eq!(max_fn(7, 7), 7);
}

#[test]
fn test_cranelift_jit_buffer_operations() {
    let mut backend = CraneliftBackend::new().expect("backend init failed");

    // Allocates buffer of 64 bytes, writes 42 at index 0, reads it back, frees, returns byte
    let mut func = Function::new("test_buf_jit", Type::I64);
    let mut bb0 = BasicBlock::new(0, Terminator::Ret(Some(5)));

    // %0 = 64
    bb0.push(Instruction::new(Some(0), Type::I64, Op::ConstI64(64), 1));
    // %1 = buf_alloc %0
    bb0.push(Instruction::new(Some(1), Type::Buf, Op::BufAlloc(0), 2));
    // %2 = 0 (idx), %3 = 42 (val)
    bb0.push(Instruction::new(Some(2), Type::I64, Op::ConstI64(0), 3));
    bb0.push(Instruction::new(Some(3), Type::I64, Op::ConstI64(42), 4));
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
    // free %4
    bb0.push(Instruction::new(None, Type::Void, Op::Free(4), 7));

    func.add_block(bb0);

    let code_ptr = backend.compile_jit(&func).expect("JIT failed");
    let buf_fn: extern "C" fn() -> i64 = unsafe { std::mem::transmute(code_ptr) };

    let result = buf_fn();
    assert_eq!(result, 42);
}

#[test]
fn test_cranelift_compile_object_single_func() {
    let mut backend = CraneliftBackend::new().expect("backend init failed");

    let mut module = Module::new();
    let mut func = Function::new("add_one", Type::I64);
    func.add_param(0, Type::I64);

    let mut bb0 = BasicBlock::new(0, Terminator::Ret(Some(2)));
    bb0.push(Instruction::new(Some(1), Type::I64, Op::ConstI64(1), 1));
    bb0.push(Instruction::new(Some(2), Type::I64, Op::Add(0, 1), 2));
    func.add_block(bb0);

    module.add_function(func);

    let obj_bytes = backend.compile_object(&module).expect("object compilation failed");
    assert!(!obj_bytes.is_empty(), "emitted object bytes must not be empty");

    #[cfg(target_os = "linux")]
    {
        // Standard ELF header: 0x7F, 'E', 'L', 'F'
        assert_eq!(&obj_bytes[0..4], b"\x7fELF", "Object file must have ELF magic header on Linux");
    }
}

#[test]
fn test_cranelift_compile_object_multi_func_with_runtime() {
    let mut backend = CraneliftBackend::new().expect("backend init failed");

    let mut module = Module::new();

    // func 1: pure arithmetic
    let mut f1 = Function::new("f1", Type::I64);
    f1.add_param(0, Type::I64);
    let mut b1 = BasicBlock::new(0, Terminator::Ret(Some(1)));
    b1.push(Instruction::new(Some(1), Type::I64, Op::ConstI64(10), 1));
    f1.add_block(b1);
    module.add_function(f1);

    // func 2: uses runtime map
    let mut f2 = Function::new("f2", Type::I64);
    let mut b2 = BasicBlock::new(0, Terminator::Ret(Some(3)));
    b2.push(Instruction::new(
        Some(0),
        Type::Map(Box::new(Type::I64), Box::new(Type::I64)),
        Op::MapNew(Type::I64, Type::I64),
        1,
    ));
    b2.push(Instruction::new(Some(1), Type::I64, Op::ConstI64(1), 2));
    b2.push(Instruction::new(Some(2), Type::I64, Op::ConstI64(99), 3));
    b2.push(Instruction::new(
        Some(3),
        Type::Map(Box::new(Type::I64), Box::new(Type::I64)),
        Op::MapSet(0, 1, 2),
        4,
    ));
    f2.add_block(b2);
    module.add_function(f2);

    let obj_bytes = backend.compile_object(&module).expect("object compilation failed");
    assert!(!obj_bytes.is_empty());
}

#[test]
fn test_cranelift_jit_unconditional_br() {
    let mut backend = CraneliftBackend::new().expect("backend init failed");

    let mut func = Function::new("uncond", Type::I64);
    let mut bb0 = BasicBlock::new(0, Terminator::Br(1));
    bb0.push(Instruction::new(Some(0), Type::I64, Op::ConstI64(42), 1));

    let mut bb1 = BasicBlock::new(1, Terminator::Ret(Some(1)));
    bb1.push(Instruction::new(Some(1), Type::I64, Op::Add(0, 0), 2));

    func.add_block(bb0);
    func.add_block(bb1);

    let code_ptr = backend.compile_jit(&func).expect("JIT failed");
    let f: extern "C" fn() -> i64 = unsafe { std::mem::transmute(code_ptr) };
    assert_eq!(f(), 84);
}

#[test]
fn test_cranelift_jit_void_func() {
    let mut backend = CraneliftBackend::new().expect("backend init failed");

    let mut func = Function::new("void_func", Type::Void);
    let bb0 = BasicBlock::new(0, Terminator::Ret(None));
    func.add_block(bb0);

    let code_ptr = backend.compile_jit(&func).expect("JIT failed");
    let f: extern "C" fn() = unsafe { std::mem::transmute(code_ptr) };
    f(); // Should execute without crashing
}

#[test]
fn test_cranelift_jit_cmp_eq() {
    let mut backend = CraneliftBackend::new().expect("backend init failed");

    // fn eq(a: i64, b: i64) -> i64
    let mut func = Function::new("is_eq", Type::I64);
    func.add_param(0, Type::I64);
    func.add_param(1, Type::I64);

    let mut bb0 = BasicBlock::new(0, Terminator::Ret(Some(2)));
    bb0.push(Instruction::new(Some(2), Type::Bool, Op::CmpEq(0, 1), 1));
    func.add_block(bb0);

    let code_ptr = backend.compile_jit(&func).expect("JIT failed");
    let eq_fn: extern "C" fn(i64, i64) -> i64 = unsafe { std::mem::transmute(code_ptr) };

    assert_eq!(eq_fn(42, 42), 1);
    assert_eq!(eq_fn(42, 43), 0);
}

#[test]
fn test_cranelift_jit_vec_operations() {
    let mut backend = CraneliftBackend::new().expect("backend init failed");

    // Allocates vec, pushes 2 items, checks length
    let mut func = Function::new("test_vec", Type::I64);
    let mut bb0 = BasicBlock::new(0, Terminator::Ret(Some(6)));

    // %0 = vec_new
    bb0.push(Instruction::new(
        Some(0),
        Type::Vec(Box::new(Type::I64)),
        Op::VecNew(Type::I64),
        1,
    ));
    // %1 = 10, %2 = 20
    bb0.push(Instruction::new(Some(1), Type::I64, Op::ConstI64(10), 2));
    bb0.push(Instruction::new(Some(2), Type::I64, Op::ConstI64(20), 3));
    // %3 = vec_push %0, %1
    bb0.push(Instruction::new(
        Some(3),
        Type::Vec(Box::new(Type::I64)),
        Op::VecPush(0, 1),
        4,
    ));
    // %4 = vec_push %3, %2
    bb0.push(Instruction::new(
        Some(4),
        Type::Vec(Box::new(Type::I64)),
        Op::VecPush(3, 2),
        5,
    ));
    // %5 = vec_get %4, idx=1
    bb0.push(Instruction::new(Some(5), Type::I64, Op::ConstI64(1), 6));
    bb0.push(Instruction::new(Some(6), Type::I64, Op::VecGet(4, 5), 7));

    func.add_block(bb0);

    let code_ptr = backend.compile_jit(&func).expect("JIT failed");
    let vec_fn: extern "C" fn() -> i64 = unsafe { std::mem::transmute(code_ptr) };
    let res = vec_fn();
    assert_eq!(res, 20);
}

#[test]
fn test_cranelift_compile_object_multi_func_with_strings() {
    let mut backend = CraneliftBackend::new().expect("backend init failed");
    let mut module = Module::new();

    // func 1 with string constant
    let mut f1 = Function::new("f_greeting", Type::Str);
    let mut b1 = BasicBlock::new(0, Terminator::Ret(Some(0)));
    b1.push(Instruction::new(Some(0), Type::Str, Op::ConstStr("hello".into()), 1));
    f1.add_block(b1);
    module.add_function(f1);

    // func 2 with string constant
    let mut f2 = Function::new("f_target", Type::Str);
    let mut b2 = BasicBlock::new(0, Terminator::Ret(Some(0)));
    b2.push(Instruction::new(Some(0), Type::Str, Op::ConstStr("world".into()), 1));
    f2.add_block(b2);
    module.add_function(f2);

    let obj_bytes = backend.compile_object(&module).expect("object compilation with strings failed");
    assert!(!obj_bytes.is_empty());
    assert_eq!(&obj_bytes[0..4], &[0x7F, b'E', b'L', b'F']);
}


