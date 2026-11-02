use knr_core::{parse_ssa, BasicBlock, Function, Instruction, Module, Op, Terminator, Type};
use knr_interp::{execute_module, Value, VmError};
use knr_runtime::MockPlatform;

#[test]
fn test_execute_module_entry() {
    let mut platform = MockPlatform::new();
    let mut module = Module::new();

    // Helper func: worker
    let mut worker = Function::new("worker", Type::I64);
    let mut w_bb0 = BasicBlock::new(0, Terminator::Ret(Some(0)));
    w_bb0.push(Instruction::new(Some(0), Type::I64, Op::ConstI64(123), 1));
    worker.add_block(w_bb0);
    module.add_function(worker);

    // Entry func: main(x: i64) -> i64
    let mut main_fn = Function::new("main", Type::I64);
    main_fn.add_param(0, Type::I64);
    let mut m_bb0 = BasicBlock::new(0, Terminator::Ret(Some(2)));
    m_bb0.push(Instruction::new(Some(1), Type::I64, Op::ConstI64(10), 1));
    m_bb0.push(Instruction::new(Some(2), Type::I64, Op::Add(0, 1), 2));
    main_fn.add_block(m_bb0);
    module.add_function(main_fn);

    let (res, _) = execute_module(&module, "main", vec![Value::I64(5)], &mut platform, 100, false)
        .expect("Module exec ok");
    assert_eq!(res, Value::I64(15));
}

#[test]
fn test_execute_module_no_such_function() {
    let mut platform = MockPlatform::new();
    let module = Module::new();

    let err = execute_module(&module, "non_existent", vec![], &mut platform, 100, false).unwrap_err();
    assert_eq!(
        err,
        VmError::NoSuchFunction {
            name: "non_existent".into()
        }
    );
}

#[test]
fn test_spawn_in_module() {
    let mut platform = MockPlatform::new();
    let mut module = Module::new();

    // worker func
    let mut worker = Function::new("worker", Type::Void);
    worker.add_param(0, Type::Chan(Box::new(Type::I64)));
    let w_bb0 = BasicBlock::new(0, Terminator::Ret(None));
    worker.add_block(w_bb0);
    module.add_function(worker);

    // main func with spawn
    let mut main_fn = Function::new("main", Type::Bool);
    let mut bb0 = BasicBlock::new(0, Terminator::Ret(Some(3)));
    bb0.push(Instruction::new(Some(0), Type::I64, Op::ConstI64(10), 1));
    bb0.push(Instruction::new(
        Some(1),
        Type::Chan(Box::new(Type::I64)),
        Op::ChanNew(Type::I64, 0),
        2,
    ));
    bb0.push(Instruction::new(
        Some(2),
        Type::Bool,
        Op::Spawn("worker".into(), 1),
        3,
    ));
    bb0.push(Instruction::new(Some(3), Type::Bool, Op::ConstBool(true), 4));
    main_fn.add_block(bb0);
    module.add_function(main_fn);

    let (res, _) = execute_module(&module, "main", vec![], &mut platform, 100, false).expect("exec ok");
    assert_eq!(res, Value::Bool(true));
}

#[test]
fn test_spawn_unknown_function_in_module_fails() {
    let mut platform = MockPlatform::new();
    let mut module = Module::new();

    let mut main_fn = Function::new("main", Type::Bool);
    let mut bb0 = BasicBlock::new(0, Terminator::Ret(Some(2)));
    bb0.push(Instruction::new(Some(0), Type::I64, Op::ConstI64(10), 1));
    bb0.push(Instruction::new(
        Some(1),
        Type::Chan(Box::new(Type::I64)),
        Op::ChanNew(Type::I64, 0),
        2,
    ));
    bb0.push(Instruction::new(
        Some(2),
        Type::Bool,
        Op::Spawn("ghost_worker".into(), 1),
        3,
    ));
    main_fn.add_block(bb0);
    module.add_function(main_fn);

    let err = execute_module(&module, "main", vec![], &mut platform, 100, false).unwrap_err();
    assert_eq!(
        err,
        VmError::NoSuchFunction {
            name: "ghost_worker".into()
        }
    );
}

#[test]
fn test_execute_parsed_ssa_module() {
    let ssa_code = r#"
fn compute(%0: i64) -> i64 {
bb0:
    %1 = const_i64 2
    %2 = mul %0 %1
    %3 = const_i64 5
    %4 = add %2 %3
    ret %4
}
"#;
    let module = parse_ssa(ssa_code).expect("Parsing should succeed");
    let mut platform = MockPlatform::new();

    let (res, _) = execute_module(&module, "compute", vec![Value::I64(10)], &mut platform, 1000, false)
        .expect("Execution should succeed");
    // (10 * 2) + 5 = 25
    assert_eq!(res, Value::I64(25));
}
