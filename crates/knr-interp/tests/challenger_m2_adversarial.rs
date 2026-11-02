use knr_core::{parse_ssa, Effect};
use knr_interp::{execute_module, Value, Vm, VmError};
use knr_runtime::MockPlatform;

#[test]
fn test_gas_refill_in_callee_allows_event_loop_exceeding_tick_budget() {
    let mut platform = MockPlatform::new();
    platform.set_response(Effect::Net, "net.wait_req", Value::I64(1));

    let ssa = r#"
fn handle_request(%0: i64) -> i64 effects(Net) {
bb0:
  %1 = syscall Net "net.wait_req" %0
  %2 = add %0 %1
  ret %2
}

fn server_loop(%0: i64) -> i64 effects(Net) {
bb0:
  %1 = call handle_request %0
  %2 = call handle_request %1
  %3 = call handle_request %2
  %4 = call handle_request %3
  %5 = call handle_request %4
  ret %5
}
"#;

    let module = parse_ssa(ssa).expect("Parse SSA server loop");
    // With max_steps = 10, 5 sequential calls take ~25 steps total (> 10). Under cumulative lifetime gas
    // it would exhaust gas after ~2 calls. Because handle_request refills gas on net.wait_req,
    // all 5 sequential calls complete successfully.
    let mut vm = Vm::new(&mut platform, Some(&module), 10, false);
    let server_fn = module.get_function("server_loop").unwrap();
    let (res, _) = vm.run(server_fn, vec![Value::I64(0)]).expect("Server loop completes with gas refill");
    assert_eq!(res, Value::I64(5));
    assert_eq!(platform.call_count(), 5);
}

#[test]
fn test_gas_refill_with_ui_poll_event_in_callee() {
    let mut platform = MockPlatform::new();
    platform.set_response(Effect::Ui, "ui.poll_event", Value::I64(10));

    let ssa = r#"
fn poll_ui_step(%0: i64) -> i64 effects(Ui) {
bb0:
  %1 = syscall Ui "ui.poll_event" %0
  %2 = add %0 %1
  ret %2
}

fn ui_loop(%0: i64) -> i64 effects(Ui) {
bb0:
  %1 = call poll_ui_step %0
  %2 = call poll_ui_step %1
  %3 = call poll_ui_step %2
  %4 = call poll_ui_step %3
  %5 = call poll_ui_step %4
  ret %5
}
"#;

    let module = parse_ssa(ssa).expect("Parse SSA UI loop");
    let mut vm = Vm::new(&mut platform, Some(&module), 8, false);
    let ui_fn = module.get_function("ui_loop").unwrap();
    let (res, _) = vm.run(ui_fn, vec![Value::I64(0)]).expect("UI loop completes with gas refill");
    assert_eq!(res, Value::I64(50));
    assert_eq!(platform.call_count(), 5);
}

#[test]
fn test_pure_cpu_loop_inside_callee_exhausts_gas() {
    let mut platform = MockPlatform::new();

    let ssa = r#"
fn cpu_spin() -> void {
bb0:
  br bb1

bb1:
  br bb1
}

fn main() -> void {
bb0:
  call cpu_spin
  ret
}
"#;

    let module = parse_ssa(ssa).expect("Parse SSA cpu spin");
    let err = execute_module(&module, "main", vec![], &mut platform, 50, false).unwrap_err();
    assert!(
        matches!(err, VmError::GasExhausted { max_steps: 50 }),
        "Expected GasExhausted in callee spin, got: {:?}",
        err
    );
}

#[test]
fn test_pure_cpu_loop_in_caller_after_callee_exhausts_gas() {
    let mut platform = MockPlatform::new();
    platform.set_response(Effect::Net, "net.wait_req", Value::I64(1));

    let ssa = r#"
fn handle_req(%0: i64) -> i64 effects(Net) {
bb0:
  %1 = syscall Net "net.wait_req" %0
  ret %1
}

fn main() -> void effects(Net) {
bb0:
  %0 = const_i64 0
  %1 = call handle_req %0
  br bb1

bb1:
  br bb1
}
"#;

    let module = parse_ssa(ssa).expect("Parse SSA caller spin");
    let err = execute_module(&module, "main", vec![], &mut platform, 30, false).unwrap_err();
    assert!(
        matches!(err, VmError::GasExhausted { max_steps: 30 }),
        "Expected GasExhausted in caller spin, got: {:?}",
        err
    );
}

#[test]
fn test_infinite_recursion_low_gas_triggers_gas_exhausted() {
    let mut platform = MockPlatform::new();

    let ssa = r#"
fn recurse(%0: i64) -> i64 {
bb0:
  %1 = const_i64 1
  %2 = add %0 %1
  %3 = call recurse %2
  ret %3
}

fn main() -> i64 {
bb0:
  %0 = const_i64 0
  %1 = call recurse %0
  ret %1
}
"#;

    let module = parse_ssa(ssa).expect("Parse SSA infinite recursion");
    let err = execute_module(&module, "main", vec![], &mut platform, 25, false).unwrap_err();
    assert!(
        matches!(err, VmError::GasExhausted { .. }),
        "Expected GasExhausted under low gas, got: {:?}",
        err
    );
}

#[test]
fn test_infinite_recursion_high_gas_triggers_clean_call_depth_guard() {
    let mut platform = MockPlatform::new();

    let ssa = r#"
fn recurse(%0: i64) -> i64 {
bb0:
  %1 = const_i64 1
  %2 = add %0 %1
  %3 = call recurse %2
  ret %3
}

fn main() -> i64 {
bb0:
  %0 = const_i64 0
  %1 = call recurse %0
  ret %1
}
"#;

    let module = parse_ssa(ssa).expect("Parse SSA infinite recursion");
    // With 100,000 gas limit, it exceeds MAX_CALL_DEPTH (64) before exhausting gas.
    let err = execute_module(&module, "main", vec![], &mut platform, 100_000, false).unwrap_err();
    assert!(
        matches!(err, VmError::InvalidInstruction { .. } | VmError::StackOverflow),
        "Expected clean stack depth error without crashing host process, got: {:?}",
        err
    );
}

#[test]
fn test_caller_callee_register_isolation() {
    let mut platform = MockPlatform::new();

    let ssa = r#"
fn callee_clobber(%0: i64, %1: i64) -> i64 {
bb0:
  %2 = const_i64 999
  %3 = mul %0 %2
  %4 = add %3 %1
  ret %4
}

fn caller() -> i64 {
bb0:
  %0 = const_i64 10
  %1 = const_i64 20
  %2 = call callee_clobber %0 %1
  %3 = add %0 %1
  %4 = add %2 %3
  ret %4
}
"#;

    let module = parse_ssa(ssa).expect("Parse SSA register isolation");
    let (val, _) = execute_module(&module, "caller", vec![], &mut platform, 500, false)
        .expect("Execution should succeed with isolated register frames");
    // %0 = 10, %1 = 20
    // callee: %3 = 10 * 999 = 9990; %4 = 9990 + 20 = 10010
    // caller resumes: %3 = %0 + %1 = 10 + 20 = 30
    // %4 = %2 (10010) + %3 (30) = 10040
    assert_eq!(val, Value::I64(10040));
}
