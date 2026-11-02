use knr_core::{parse_ssa, Effect};
use knr_interp::{execute_module, Value, Vm, VmError};
use knr_runtime::MockPlatform;

#[test]
fn test_interp_call_simple_add() {
    let mut platform = MockPlatform::new();
    let ssa = r#"
fn add(%0: i64, %1: i64) -> i64 {
bb0:
  %2 = add %0 %1
  ret %2
}

fn main() -> i64 {
bb0:
  %0 = const_i64 15
  %1 = const_i64 27
  %2 = call add %0 %1
  ret %2
}
"#;
    let module = parse_ssa(ssa).expect("Parse SSA");
    let (val, _) = execute_module(&module, "main", vec![], &mut platform, 100, false)
        .expect("Exec add call");
    assert_eq!(val, Value::I64(42));
}

#[test]
fn test_interp_call_nested_calls() {
    let mut platform = MockPlatform::new();
    let ssa = r#"
fn double(%0: i64) -> i64 {
bb0:
  %1 = add %0 %0
  ret %1
}

fn calc(%0: i64, %1: i64) -> i64 {
bb0:
  %2 = add %0 %1
  %3 = call double %2
  ret %3
}

fn main() -> i64 {
bb0:
  %0 = const_i64 10
  %1 = const_i64 20
  %2 = call calc %0 %1
  ret %2
}
"#;
    let module = parse_ssa(ssa).expect("Parse SSA");
    let (val, _) = execute_module(&module, "main", vec![], &mut platform, 200, false)
        .expect("Exec nested calls");
    assert_eq!(val, Value::I64(60));
}

#[test]
fn test_interp_call_recursive_factorial() {
    let mut platform = MockPlatform::new();
    let ssa = r#"
fn fact(%0: i64) -> i64 {
bb0:
  %1 = const_i64 1
  %2 = cmp_lt %0 %1
  br_if %2 bb1 bb2

bb1:
  %3 = const_i64 1
  ret %3

bb2:
  %4 = const_i64 1
  %5 = sub %0 %4
  %6 = call fact %5
  %7 = mul %0 %6
  ret %7
}

fn main() -> i64 {
bb0:
  %0 = const_i64 5
  %1 = call fact %0
  ret %1
}
"#;
    let module = parse_ssa(ssa).expect("Parse SSA");
    let (val, _) = execute_module(&module, "main", vec![], &mut platform, 1000, false)
        .expect("Exec factorial");
    assert_eq!(val, Value::I64(120));
}

#[test]
fn test_interp_call_returning_struct() {
    let mut platform = MockPlatform::new();
    let ssa = r#"
schema Pair {
  0: first i64,
  1: second i64,
}

fn make_pair(%0: i64, %1: i64) -> struct[Pair] {
bb0:
  %2 = struct_new Pair
  %3 = struct_set %2 0 %0
  %4 = struct_set %3 1 %1
  ret %4
}

fn main() -> i64 {
bb0:
  %0 = const_i64 100
  %1 = const_i64 250
  %2 = call make_pair %0 %1
  %3 = struct_get %2 0
  %4 = struct_get %2 1
  %5 = add %3 %4
  ret %5
}
"#;
    let module = parse_ssa(ssa).expect("Parse SSA");
    let (val, _) = execute_module(&module, "main", vec![], &mut platform, 500, false)
        .expect("Exec struct call");
    assert_eq!(val, Value::I64(350));
}

#[test]
fn test_interp_call_infinite_recursion_exhausts_gas() {
    let mut platform = MockPlatform::new();
    let ssa = r#"
fn runaway() -> void {
bb0:
  call runaway
  ret
}

fn main() -> void {
bb0:
  call runaway
  ret
}
"#;
    let module = parse_ssa(ssa).expect("Parse SSA");
    let err = execute_module(&module, "main", vec![], &mut platform, 20, false).unwrap_err();
    assert!(
        matches!(err, VmError::GasExhausted { .. }),
        "Expected GasExhausted, got: {:?}",
        err
    );
}

#[test]
fn test_interp_call_gas_refill_in_callee_across_calls() {
    let mut platform = MockPlatform::new();
    platform.set_response(Effect::Net, "net.wait_req", Value::I64(42));

    let ssa = r#"
fn process_event(%0: i64) -> i64 effects(Net) {
bb0:
  %1 = syscall Net "net.wait_req" %0
  %2 = const_i64 1
  ret %2
}

fn main() -> i64 effects(Net) {
bb0:
  %0 = const_i64 0
  %1 = call process_event %0
  %2 = call process_event %1
  %3 = call process_event %2
  %4 = call process_event %3
  %5 = call process_event %4
  ret %5
}
"#;
    let module = parse_ssa(ssa).expect("Parse SSA");
    // With gas limit = 10, executing 5 calls (~20 steps total) would fail under cumulative gas.
    // Because net.wait_req in process_event refills remaining_steps to 10 on each call,
    // the unrolled sequential calls succeed and return %5.
    let mut vm = Vm::new(&mut platform, Some(&module), 10, false);
    let main_fn = module.get_function("main").unwrap();
    let (val, _) = vm.run(main_fn, vec![]).expect("Refill allows sequential calls completion");
    assert_eq!(val, Value::I64(1));
    assert_eq!(platform.call_count(), 5);
}

#[test]
fn test_interp_call_execution_trace_interleaving() {
    let mut platform = MockPlatform::new();
    let ssa = r#"
fn helper(%0: i64) -> i64 {
bb0:
  %1 = const_i64 100
  %2 = add %0 %1
  ret %2
}

fn main() -> i64 {
bb0:
  %0 = const_i64 5
  %1 = call helper %0
  %2 = const_i64 1
  %3 = add %1 %2
  ret %3
}
"#;
    let module = parse_ssa(ssa).expect("Parse SSA");
    let (val, trace) = execute_module(&module, "main", vec![], &mut platform, 100, true)
        .expect("Exec with trace");
    assert_eq!(val, Value::I64(106));

    let opcodes: Vec<&str> = trace.iter().map(|t| t.opcode.as_str()).collect();
    // main: const_i64, call (enters helper)
    // helper: const_i64, add, ret
    // main resumes: const_i64, add, ret
    assert_eq!(
        opcodes,
        vec![
            "const_i64",
            "const_i64", // from helper
            "add",       // from helper
            "ret",       // helper ret
            "call",      // caller call instruction completes
            "const_i64", // main resumes
            "add",
            "ret"
        ]
    );
}
