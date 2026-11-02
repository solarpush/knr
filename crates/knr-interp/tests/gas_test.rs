use knr_core::{parse_ssa, BasicBlock, Effect, Function, Instruction, Op, Terminator, Type};
use knr_interp::{execute, execute_module, Value, VmError};
use knr_runtime::{HostPlatform, MockPlatform, RuntimeError};

#[test]
fn test_infinite_loop_gas_exhaustion() {
    let mut platform = MockPlatform::new();

    // bb0:
    //   br bb0
    let mut func = Function::new("inf_loop", Type::Void);
    let bb0 = BasicBlock::new(0, Terminator::Br(0));
    func.add_block(bb0);

    let err = execute(&func, vec![], &mut platform, 100, false).unwrap_err();
    assert_eq!(err, VmError::GasExhausted { max_steps: 100 });
}

#[test]
fn test_loop_with_instructions_gas_exhaustion() {
    let mut platform = MockPlatform::new();

    // bb0:
    //   %0 = const_i64 1
    //   %1 = add %0 %0
    //   br bb0
    let mut func = Function::new("loop_insts", Type::Void);
    let mut bb0 = BasicBlock::new(0, Terminator::Br(0));
    bb0.push(Instruction::new(Some(0), Type::I64, Op::ConstI64(1), 1));
    bb0.push(Instruction::new(Some(1), Type::I64, Op::Add(0, 0), 2));
    func.add_block(bb0);

    let err = execute(&func, vec![], &mut platform, 50, false).unwrap_err();
    assert_eq!(err, VmError::GasExhausted { max_steps: 50 });
}

#[test]
fn test_zero_max_steps() {
    let mut platform = MockPlatform::new();

    let mut func = Function::new("one_inst", Type::I64);
    let mut bb0 = BasicBlock::new(0, Terminator::Ret(Some(0)));
    bb0.push(Instruction::new(Some(0), Type::I64, Op::ConstI64(42), 1));
    func.add_block(bb0);

    let err = execute(&func, vec![], &mut platform, 0, false).unwrap_err();
    assert_eq!(err, VmError::GasExhausted { max_steps: 0 });
}

#[test]
fn test_exact_gas_boundary() {
    let mut platform = MockPlatform::new();

    // bb0:
    //   %0 = const_i64 1 (inst 1)
    //   %1 = const_i64 2 (inst 2)
    //   ret %1           (term 3)
    // Total steps = 3.
    let mut func = Function::new("three_steps", Type::I64);
    let mut bb0 = BasicBlock::new(0, Terminator::Ret(Some(1)));
    bb0.push(Instruction::new(Some(0), Type::I64, Op::ConstI64(1), 1));
    bb0.push(Instruction::new(Some(1), Type::I64, Op::ConstI64(2), 2));
    func.add_block(bb0);

    // 2 steps is not enough -> GasExhausted
    let err = execute(&func, vec![], &mut platform, 2, false).unwrap_err();
    assert_eq!(err, VmError::GasExhausted { max_steps: 2 });

    // 3 steps is exactly enough
    let (res, _) = execute(&func, vec![], &mut platform, 3, false)
        .expect("3 steps should suffice");
    assert_eq!(res, Value::I64(2));
}

#[test]
fn test_server_loop_multi_iteration_gas_refill() {
    let ssa = r#"
fn main() -> i64 effects(Net, Async) {
bb0:
    %0 = const_i64 1
    %1 = chan_new i64 %0
    %2 = const_i64 0
    %3 = chan_send %1 %2
    %4 = const_i64 100
    br bb1

bb1:
    %5 = chan_recv %1
    %6 = cmp_lt %5 %4
    br_if %6 bb2 bb3

bb2:
    %7 = syscall Net "net.wait_req" %5
    %8 = add %5 %0
    %9 = chan_send %1 %8
    br bb1

bb3:
    free %1
    ret %5
}
"#;
    let module = parse_ssa(ssa).expect("valid SSA");
    let mut platform = MockPlatform::new();
    platform.set_response(Effect::Net, "net.wait_req", Value::I64(42));

    // Gas limit = 15 (each iteration is ~7 instructions, 100 iterations = ~706 steps).
    // Cumulative gas would fail after 2 iterations (~14 steps).
    // With auto-refill on net.wait_req, it executes all 100 requests successfully.
    let (res, _) = execute_module(&module, "main", vec![], &mut platform, 15, false)
        .expect("Server loop with gas refill must succeed");

    assert_eq!(res, Value::I64(100));
    assert_eq!(platform.call_count(), 100);
}

#[test]
fn test_pure_cpu_loop_inside_request_handler_exhausts_gas() {
    let ssa = r#"
fn main() -> i64 effects(Net) {
bb0:
    %0 = const_i64 1
    br bb1

bb1:
    %1 = syscall Net "net.wait_req" %0
    br bb2

bb2:
    br bb2
}
"#;
    let module = parse_ssa(ssa).expect("valid SSA");
    let mut platform = MockPlatform::new();
    platform.set_response(Effect::Net, "net.wait_req", Value::I64(42));

    // Gas limit = 50. net.wait_req refills to 50, then bb2 loops 50 times and exhausts gas.
    let err = execute_module(&module, "main", vec![], &mut platform, 50, false).unwrap_err();
    assert_eq!(err, VmError::GasExhausted { max_steps: 50 });
}


#[test]
fn test_ui_poll_event_gas_refill() {
    let ssa = r#"
fn main() -> i64 effects(Ui, Async) {
bb0:
    %0 = const_i64 1
    %1 = chan_new i64 %0
    %2 = const_i64 0
    %3 = chan_send %1 %2
    %4 = const_i64 100
    br bb1

bb1:
    %5 = chan_recv %1
    %6 = cmp_lt %5 %4
    br_if %6 bb2 bb3

bb2:
    %7 = syscall Ui "ui.poll_event" %5
    %8 = add %5 %0
    %9 = chan_send %1 %8
    br bb1

bb3:
    free %1
    ret %5
}
"#;
    let module = parse_ssa(ssa).expect("valid SSA");
    let mut platform = MockPlatform::new();
    platform.set_response(Effect::Ui, "ui.poll_event", Value::Void);

    // Gas limit = 15. Auto-refill on ui.poll_event allows all 100 ticks to complete.
    let (res, _) = execute_module(&module, "main", vec![], &mut platform, 15, false)
        .expect("UI event loop with gas refill must succeed");

    assert_eq!(res, Value::I64(100));
    assert_eq!(platform.call_count(), 100);
}

#[test]
fn test_unlimited_gas_u64_max_terminates_cleanly() {
    let ssa = r#"
fn main() -> i64 effects(Async) {
bb0:
    %0 = const_i64 1
    %1 = chan_new i64 %0
    %2 = const_i64 0
    %3 = chan_send %1 %2
    %4 = const_i64 5000
    br bb1

bb1:
    %5 = chan_recv %1
    %6 = cmp_lt %5 %4
    br_if %6 bb2 bb3

bb2:
    %7 = add %5 %0
    %8 = chan_send %1 %7
    br bb1

bb3:
    free %1
    ret %5
}
"#;
    let module = parse_ssa(ssa).expect("valid SSA");
    let mut platform = MockPlatform::new();

    // 5000 iterations = ~30,000 steps.
    // Under 100 steps -> exhausts gas.
    let err = execute_module(&module, "main", vec![], &mut platform, 100, false).unwrap_err();
    assert_eq!(err, VmError::GasExhausted { max_steps: 100 });

    // Under u64::MAX (unlimited) -> succeeds!
    let (res, _) = execute_module(&module, "main", vec![], &mut platform, u64::MAX, false)
        .expect("Unlimited gas execution must succeed");
    assert_eq!(res, Value::I64(5000));
}

struct BoundedServerPlatform {
    requests_left: usize,
    requests_processed: usize,
}

impl HostPlatform for BoundedServerPlatform {
    fn host_call(&mut self, effect: Effect, op: &str, _payload: Value) -> Result<Value, RuntimeError> {
        match (effect, op) {
            (Effect::Net, "net.listen") => {
                let mut map = std::collections::BTreeMap::new();
                map.insert("chan_id".to_string(), Value::I64(1));
                map.insert("port".to_string(), Value::I64(8080));
                Ok(Value::Map(map))
            }
            (Effect::Net, "net.wait_req") => {
                if self.requests_left == 0 {
                    return Err(RuntimeError::Channel("Server completed test requests".into()));
                }
                self.requests_left -= 1;
                self.requests_processed += 1;
                Ok(Value::Struct {
                    schema: "HttpRequest".to_string(),
                    fields: vec![
                        Value::I64(self.requests_processed as i64),
                        Value::Str("POST".to_string()),
                        Value::Str("/api/inc".to_string()),
                    ],
                })
            }
            (Effect::Net, "net.respond") => Ok(Value::Bool(true)),
            (Effect::Ui, "ui.render") => Ok(Value::Str("<h1>OK</h1>".to_string())),
            _ => Ok(Value::Void),
        }
    }
}

#[test]
fn test_site_server_ssa_execution_with_gas_refill_and_unlimited() {
    let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let repo_root = manifest_dir.parent().unwrap().parent().unwrap();
    let server_path = repo_root.join("site").join("server.ssa");
    let content = std::fs::read_to_string(&server_path).expect("read site/server.ssa");

    // 1. Module must parse cleanly into AST
    let module = parse_ssa(&content).expect("parse site/server.ssa");

    // 2. Run under gas = 50 with 10 requests.
    // Each request is ~15 instructions, total steps ~150 >> 50.
    // With auto-refill on net.wait_req, it executes all 10 requests without premature gas exhaustion!
    let mut platform_finite = BoundedServerPlatform {
        requests_left: 10,
        requests_processed: 0,
    };
    let err_finite = execute_module(&module, "main", vec![], &mut platform_finite, 50, false).unwrap_err();
    assert_eq!(platform_finite.requests_processed, 10);
    assert_eq!(
        err_finite,
        VmError::HostCallError(RuntimeError::Channel("Server completed test requests".into()))
    );

    // 3. Run under unlimited gas (u64::MAX) with 20 requests.
    let mut platform_unlimited = BoundedServerPlatform {
        requests_left: 20,
        requests_processed: 0,
    };
    let err_unlim = execute_module(&module, "main", vec![], &mut platform_unlimited, u64::MAX, false).unwrap_err();
    assert_eq!(platform_unlimited.requests_processed, 20);
    assert_eq!(
        err_unlim,
        VmError::HostCallError(RuntimeError::Channel("Server completed test requests".into()))
    );
}



