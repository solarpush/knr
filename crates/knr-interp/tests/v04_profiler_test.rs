//! Comprehensive v0.4.0 Deterministic VM Profiler (`ExecutionStats` & `execute_module_with_stats`) Tests.

use knr_core::{
    BasicBlock, FieldDef, Function, Instruction, Module, Op, Schema, Terminator, Type,
};
use knr_interp::{execute_module, execute_module_with_stats, ExecutionStats, Value, VmError};
use knr_runtime::MockPlatform;

#[test]
fn test_profiler_basic_instructions_gas_and_determinism() {
    let mut module = Module::new();
    // fn main(%0: i64) -> i64 {
    //   %1 = const_i64 10
    //   %2 = add %0 %1
    //   %3 = const_i64 2
    //   %4 = mul %2 %3
    //   ret %4
    // }
    // 4 instructions + 1 terminator = 5 gas steps
    let mut main_fn = Function::new("main", Type::I64);
    main_fn.add_param(0, Type::I64);
    let mut bb0 = BasicBlock::new(0, Terminator::Ret(Some(4)));
    bb0.push(Instruction::new(Some(1), Type::I64, Op::ConstI64(10), 1));
    bb0.push(Instruction::new(Some(2), Type::I64, Op::Add(0, 1), 2));
    bb0.push(Instruction::new(Some(3), Type::I64, Op::ConstI64(2), 3));
    bb0.push(Instruction::new(Some(4), Type::I64, Op::Mul(2, 3), 4));
    main_fn.add_block(bb0);
    module.add_function(main_fn);

    let mut p1 = MockPlatform::new();
    let (val_plain, _) =
        execute_module(&module, "main", vec![Value::I64(5)], &mut p1, 1000, false).unwrap();

    let mut p2 = MockPlatform::new();
    let (val_prof, traces, stats) =
        execute_module_with_stats(&module, "main", vec![Value::I64(5)], &mut p2, 1000, true)
            .unwrap();

    assert_eq!(val_plain, Value::I64(30));
    assert_eq!(val_prof, val_plain);
    assert_eq!(traces.len(), 5); // 4 insts + 1 ret
    assert_eq!(stats.total_instructions, 4);
    assert_eq!(stats.gas_used, 5);
    assert_eq!(stats.function_calls.get("main"), Some(&1));
    assert_eq!(stats.function_instructions.get("main"), Some(&4));
    assert_eq!(stats.allocations, 0);
    assert_eq!(stats.frees, 0);

    // Determinism + serde JSON roundtrip
    let json = serde_json::to_string(&stats).expect("serialize ExecutionStats");
    let decoded: ExecutionStats = serde_json::from_str(&json).expect("deserialize ExecutionStats");
    assert_eq!(stats, decoded);
}

#[test]
fn test_profiler_per_function_breakdown_with_calls_and_recursion() {
    let mut module = Module::new();

    // helper double(%0: i64) -> i64 { %1 = add %0 %0; ret %1 }  (1 inst per call)
    let mut f_dbl = Function::new("double", Type::I64);
    f_dbl.add_param(0, Type::I64);
    let mut bd = BasicBlock::new(0, Terminator::Ret(Some(1)));
    bd.push(Instruction::new(Some(1), Type::I64, Op::Add(0, 0), 1));
    f_dbl.add_block(bd);
    module.add_function(f_dbl);

    // recursive fact(%0: i64) -> i64
    // bb0: %1 = 1; %2 = cmp_lt %0 %1; br_if %2 bb1 bb2 (2 insts)
    // bb1: ret %1                                       (0 insts)
    // bb2: %3 = sub %0 %1; %4 = call fact %3; %5 = mul %0 %4; ret %5 (3 insts)
    let mut f_fact = Function::new("fact", Type::I64);
    f_fact.add_param(0, Type::I64);
    let mut fb0 = BasicBlock::new(
        0,
        Terminator::BrIf {
            cond: 2,
            then_blk: 1,
            else_blk: 2,
        },
    );
    fb0.push(Instruction::new(Some(1), Type::I64, Op::ConstI64(1), 1));
    fb0.push(Instruction::new(Some(2), Type::Bool, Op::CmpLt(0, 1), 2));
    f_fact.add_block(fb0);
    f_fact.add_block(BasicBlock::new(1, Terminator::Ret(Some(1))));
    let mut fb2 = BasicBlock::new(2, Terminator::Ret(Some(5)));
    fb2.push(Instruction::new(Some(3), Type::I64, Op::Sub(0, 1), 3));
    fb2.push(Instruction::new(
        Some(4),
        Type::I64,
        Op::Call("fact".into(), vec![3]),
        4,
    ));
    fb2.push(Instruction::new(Some(5), Type::I64, Op::Mul(0, 4), 5));
    f_fact.add_block(fb2);
    module.add_function(f_fact);

    // main() -> i64: calls fact(5) (=120), then double twice (=480)
    let mut f_main = Function::new("main", Type::I64);
    let mut mb0 = BasicBlock::new(0, Terminator::Ret(Some(3)));
    mb0.push(Instruction::new(Some(0), Type::I64, Op::ConstI64(5), 1));
    mb0.push(Instruction::new(
        Some(1),
        Type::I64,
        Op::Call("fact".into(), vec![0]),
        2,
    ));
    mb0.push(Instruction::new(
        Some(2),
        Type::I64,
        Op::Call("double".into(), vec![1]),
        3,
    ));
    mb0.push(Instruction::new(
        Some(3),
        Type::I64,
        Op::Call("double".into(), vec![2]),
        4,
    ));
    f_main.add_block(mb0);
    module.add_function(f_main);

    let mut platform = MockPlatform::new();
    let (val, traces, stats) =
        execute_module_with_stats(&module, "main", vec![], &mut platform, 10_000, false).unwrap();

    assert_eq!(val, Value::I64(480));
    assert!(traces.is_empty());

    // Call counts: main=1, fact=6 (n=5,4,3,2,1,0), double=2
    assert_eq!(stats.function_calls.get("main"), Some(&1));
    assert_eq!(stats.function_calls.get("fact"), Some(&6));
    assert_eq!(stats.function_calls.get("double"), Some(&2));

    // Per-function instruction counts (strictly attributed, no double-counting):
    // main: 4 insts
    // double: 2 calls * 1 inst = 2 insts
    // fact: 5 recursive calls * (2 + 3) + 1 base call * 2 = 27 insts
    assert_eq!(stats.function_instructions.get("main"), Some(&4));
    assert_eq!(stats.function_instructions.get("double"), Some(&2));
    assert_eq!(stats.function_instructions.get("fact"), Some(&27));

    // Sum of per-function instructions MUST equal total_instructions
    let sum_fn_insts: u64 = stats.function_instructions.values().sum();
    assert_eq!(sum_fn_insts, stats.total_instructions);
    assert_eq!(stats.total_instructions, 33);
    // Gas includes terminators: main(1) + double(2) + fact(5*2 + 1*2 = 12) = 15 terminators -> 33 + 15 = 48
    assert_eq!(stats.gas_used, 48);
}

#[test]
fn test_profiler_allocations_and_frees_counters() {
    let mut module = Module::new();
    let mut sc = Schema::new("Node");
    sc.fields.push(FieldDef::with_name(0, "val", Type::I64));
    module.add_schema(sc);

    // Allocates: 1 struct, 1 vec, 1 map, 1 buf, 1 chan => 5 allocations
    // Frees: struct, vec, map, buf => 4 frees (chan left unfreed on purpose to check asymmetry)
    let mut f = Function::new("alloc_test", Type::I64);
    let mut bb = BasicBlock::new(0, Terminator::Ret(Some(10)));
    bb.push(Instruction::new(
        Some(0),
        Type::Struct("Node".into()),
        Op::StructNew("Node".into()),
        1,
    ));
    bb.push(Instruction::new(
        Some(1),
        Type::Vec(Box::new(Type::I64)),
        Op::VecNew(Type::I64),
        2,
    ));
    bb.push(Instruction::new(
        Some(2),
        Type::Map(Box::new(Type::Str), Box::new(Type::I64)),
        Op::MapNew(Type::Str, Type::I64),
        3,
    ));
    bb.push(Instruction::new(Some(3), Type::I64, Op::ConstI64(32), 4));
    bb.push(Instruction::new(Some(4), Type::Buf, Op::BufAlloc(3), 5));
    bb.push(Instruction::new(
        Some(5),
        Type::Chan(Box::new(Type::I64)),
        Op::ChanNew(Type::I64, 3),
        6,
    ));
    bb.push(Instruction::new(None, Type::Void, Op::Free(0), 7));
    bb.push(Instruction::new(None, Type::Void, Op::Free(1), 8));
    bb.push(Instruction::new(None, Type::Void, Op::Free(2), 9));
    bb.push(Instruction::new(None, Type::Void, Op::Free(4), 10));
    bb.push(Instruction::new(Some(10), Type::I64, Op::ConstI64(99), 11));
    f.add_block(bb);
    module.add_function(f);

    let mut platform = MockPlatform::new();
    let (val, _, stats) =
        execute_module_with_stats(&module, "alloc_test", vec![], &mut platform, 1000, false)
            .unwrap();

    assert_eq!(val, Value::I64(99));
    assert_eq!(stats.allocations, 5);
    assert_eq!(stats.frees, 4);
    assert_eq!(stats.total_instructions, 11);
    assert_eq!(stats.gas_used, 12);
}

#[test]
fn test_profiler_f64_rem_and_error_paths() {
    let mut module = Module::new();
    let mut f = Function::new("fmod_fn", Type::F64);
    let mut bb = BasicBlock::new(0, Terminator::Ret(Some(2)));
    bb.push(Instruction::new(Some(0), Type::F64, Op::ConstF64(9.25), 1));
    bb.push(Instruction::new(Some(1), Type::F64, Op::ConstF64(2.5), 2));
    bb.push(Instruction::new(Some(2), Type::F64, Op::Rem(0, 1), 3));
    f.add_block(bb);
    module.add_function(f);

    let mut platform = MockPlatform::new();
    let (val, _, stats) =
        execute_module_with_stats(&module, "fmod_fn", vec![], &mut platform, 100, false).unwrap();
    assert_eq!(val, Value::F64(1.75));
    assert_eq!(stats.total_instructions, 3);
    assert_eq!(stats.gas_used, 4);

    // Missing function error
    let err_missing =
        execute_module_with_stats(&module, "missing", vec![], &mut platform, 100, false)
            .unwrap_err();
    assert_eq!(
        err_missing,
        VmError::NoSuchFunction {
            name: "missing".into()
        }
    );

    // Gas exhaustion error
    let err_gas =
        execute_module_with_stats(&module, "fmod_fn", vec![], &mut platform, 2, false)
            .unwrap_err();
    assert_eq!(err_gas, VmError::GasExhausted { max_steps: 2 });
}
