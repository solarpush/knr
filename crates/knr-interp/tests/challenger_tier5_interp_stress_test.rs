//! Tier 5 Adversarial Hardening Suite for knr-interp Reference VM Engine
//!
//! Scope:
//! 1. Gas boundary checks (0, 1, exact N, unlimited gas u64::MAX, event-loop syscall refills)
//! 2. Instruction trace stepping oracle fidelity across opcodes, branches, and returns
//! 3. Maximum call depth (64) and recursion boundary limits
//! 4. Container boundary extremes (Buffer, Vector, Map, Channel, Struct)

use knr_core::parser::{parse_function, parse_module};
use knr_core::Effect;
use knr_interp::{execute, execute_module, default_value_for_type_with_schemas, Value, VmError};
use knr_runtime::{HostPlatform, MockPlatform, RuntimeError};

// =========================================================================
// 1. Gas Boundary Checks & Event-Loop Gas Refill
// =========================================================================

#[test]
fn test_adv_interp_gas_boundary_matrix() {
    let mut platform = MockPlatform::new();

    // 2-instruction program:
    // %0 = const_i64 100   (step 1)
    // %1 = const_i64 200   (step 2)
    // ret %1               (step 3)
    let ssa = "fn three_steps() -> i64 {
bb0:
    %0 = const_i64 100
    %1 = const_i64 200
    ret %1
}";
    let func = parse_function(ssa).unwrap();

    // max_steps = 0: Exhausts immediately before instruction 1
    let err0 = execute(&func, vec![], &mut platform, 0, false).unwrap_err();
    assert_eq!(err0, VmError::GasExhausted { max_steps: 0 });

    // max_steps = 1: Executes step 1, exhausts before step 2
    let err1 = execute(&func, vec![], &mut platform, 1, false).unwrap_err();
    assert_eq!(err1, VmError::GasExhausted { max_steps: 1 });

    // max_steps = 2: Executes steps 1 & 2, exhausts before step 3 (ret)
    let err2 = execute(&func, vec![], &mut platform, 2, false).unwrap_err();
    assert_eq!(err2, VmError::GasExhausted { max_steps: 2 });

    // max_steps = 3: Exact budget -> succeeds!
    let (val3, trace3) = execute(&func, vec![], &mut platform, 3, false).unwrap();
    assert_eq!(val3, Value::I64(200));
    assert!(trace3.is_empty());

    // max_steps = u64::MAX: Unlimited gas mode
    let (val_max, _) = execute(&func, vec![], &mut platform, u64::MAX, false).unwrap();
    assert_eq!(val_max, Value::I64(200));
}

struct SyscallRefillMock {
    call_count: usize,
}

impl HostPlatform for SyscallRefillMock {
    fn host_call(&mut self, _effect: Effect, _op: &str, _payload: Value) -> Result<Value, RuntimeError> {
        self.call_count += 1;
        Ok(Value::I64(42))
    }
}

#[test]
fn test_adv_interp_event_loop_gas_refill() {
    let mut platform = SyscallRefillMock { call_count: 0 };

    // Loop that would exceed 5 gas without refills, but net.wait_req refills remaining_steps
    // bb0:
    //   %0 = const_i64 0        (1)
    //   br bb1                  (2)
    // bb1:
    //   %1 = syscall net "net.wait_req" %0 (3) -> refills gas to max_steps (3)
    //   %2 = const_i64 1        (1 after refill)
    //   ret %2                  (2 after refill)
    let ssa = "fn event_loop() -> i64 effects(Net) {
bb0:
    %0 = const_i64 0
    br bb1
bb1:
    %1 = syscall Net \"net.wait_req\" %0
    %2 = const_i64 1
    ret %2
}";
    let func = parse_function(ssa).unwrap();

    // Budget of 3: Without refill, 5 steps would exhaust gas at step 3 or 4.
    // With refill on net.wait_req, remaining_steps is reset to 3!
    let (val, _) = execute(&func, vec![], &mut platform, 3, false).unwrap();
    assert_eq!(val, Value::I64(1));
    assert_eq!(platform.call_count, 1);
}

// =========================================================================
// 2. Trace Stepping Oracle Fidelity
// =========================================================================

#[test]
fn test_adv_interp_trace_fidelity() {
    let mut platform = MockPlatform::new();

    let ssa = "fn compute(%0: i64) -> i64 {
bb0:
    %1 = const_i64 10
    %2 = cmp_lt %0 %1
    br_if %2 bb1 bb2
bb1:
    %3 = const_i64 100
    ret %3
bb2:
    %4 = const_i64 200
    ret %4
}";
    let func = parse_function(ssa).unwrap();

    // Case A: x = 5 -> takes then_blk bb1
    let (val_then, trace_then) = execute(&func, vec![Value::I64(5)], &mut platform, 1000, true).unwrap();
    assert_eq!(val_then, Value::I64(100));
    assert_eq!(trace_then.len(), 5); // const_i64, cmp_lt, br_if, const_i64, ret

    assert_eq!(trace_then[0].opcode, "const_i64");
    assert_eq!(trace_then[0].dst_reg.as_deref(), Some("%1"));
    assert_eq!(trace_then[0].new_value, Some(serde_json::json!(10)));

    assert_eq!(trace_then[1].opcode, "cmp_lt");
    assert_eq!(trace_then[1].dst_reg.as_deref(), Some("%2"));
    assert_eq!(trace_then[1].new_value, Some(serde_json::json!(true)));

    assert_eq!(trace_then[2].opcode, "br_if");
    assert_eq!(trace_then[2].new_value, Some(serde_json::json!(true)));

    assert_eq!(trace_then[3].opcode, "const_i64");
    assert_eq!(trace_then[3].dst_reg.as_deref(), Some("%3"));
    assert_eq!(trace_then[3].new_value, Some(serde_json::json!(100)));

    assert_eq!(trace_then[4].opcode, "ret");
    assert_eq!(trace_then[4].new_value, Some(serde_json::json!(100)));

    // Case B: x = 15 -> takes else_blk bb2
    let (val_else, trace_else) = execute(&func, vec![Value::I64(15)], &mut platform, 1000, true).unwrap();
    assert_eq!(val_else, Value::I64(200));
    assert_eq!(trace_else.len(), 5);
    assert_eq!(trace_else[1].new_value, Some(serde_json::json!(false)));
    assert_eq!(trace_else[2].new_value, Some(serde_json::json!(false)));
    assert_eq!(trace_else[3].dst_reg.as_deref(), Some("%4"));
    assert_eq!(trace_else[3].new_value, Some(serde_json::json!(200)));
    assert_eq!(trace_else[4].opcode, "ret");
    assert_eq!(trace_else[4].new_value, Some(serde_json::json!(200)));
}

// =========================================================================
// 3. Maximum Call Depth (64) & Recursion Boundary Limits
// =========================================================================

#[test]
fn test_adv_interp_call_depth_and_recursion_limit() {
    let mut platform = MockPlatform::new();

    // Module with recursive countdown:
    // fn countdown(%0: i64) -> i64:
    //   if %0 <= 0 return 0 else return countdown(%0 - 1)
    let module_src = "
fn countdown(%0: i64) -> i64 {
bb0:
    %1 = const_i64 0
    %2 = cmp_lt %1 %0
    br_if %2 bb1 bb2
bb1:
    %3 = const_i64 1
    %4 = sub %0 %3
    %5 = call countdown %4
    ret %5
bb2:
    %6 = const_i64 0
    ret %6
}";
    let module = parse_module(module_src).unwrap();

    // countdown(60): recursion depth 60 <= 64 -> succeeds!
    let (res_ok, _) = execute_module(&module, "countdown", vec![Value::I64(60)], &mut platform, 100_000, false).unwrap();
    assert_eq!(res_ok, Value::I64(0));

    // countdown(70): recursion depth exceeds 64 -> must return VmError::InvalidInstruction with "Maximum call depth exceeded (64)"
    let err_overflow = execute_module(&module, "countdown", vec![Value::I64(70)], &mut platform, 100_000, false).unwrap_err();
    match err_overflow {
        VmError::InvalidInstruction { message, .. } => {
            assert!(message.contains("Maximum call depth exceeded"), "Expected call depth error, got: {message}");
        }
        other => panic!("Expected InvalidInstruction with depth exceeded, got: {other:?}"),
    }
}

// =========================================================================
// 4. Container Boundary Extremes (Buffer, Vec, Map, Chan, Struct)
// =========================================================================

#[test]
fn test_adv_interp_buffer_and_container_boundaries() {
    let mut platform = MockPlatform::new();

    // Negative buffer size
    let ssa_neg_buf = "fn neg_buf() -> buf {
bb0:
    %0 = const_i64 -5
    %1 = buf_alloc %0
    ret %1
}";
    let err_neg = execute(&parse_function(ssa_neg_buf).unwrap(), vec![], &mut platform, 100, false).unwrap_err();
    assert!(matches!(err_neg, VmError::InvalidInstruction { .. }));

    // Buffer read out of bounds
    let ssa_oob_read = "fn oob_read() -> i64 {
bb0:
    %0 = const_i64 10
    %1 = buf_alloc %0
    %2 = const_i64 10
    %3 = buf_read_u8 %1 %2
    ret %3
}";
    let err_oob_read = execute(&parse_function(ssa_oob_read).unwrap(), vec![], &mut platform, 100, false).unwrap_err();
    assert!(matches!(err_oob_read, VmError::IndexOutOfBounds { index: 10, len: 10, .. }));

    // Buffer slice overflow / negative indices
    let ssa_slice_neg = "fn slice_neg() -> buf {
bb0:
    %0 = const_i64 10
    %1 = buf_alloc %0
    %2 = const_i64 -1
    %3 = const_i64 5
    %4 = buf_slice %1 %2 %3
    ret %4
}";
    let err_slice = execute(&parse_function(ssa_slice_neg).unwrap(), vec![], &mut platform, 100, false).unwrap_err();
    assert!(matches!(err_slice, VmError::IndexOutOfBounds { .. }));

    // Channel negative capacity
    let ssa_chan_neg = "fn chan_neg() -> chan<i64> {
bb0:
    %0 = const_i64 -1
    %1 = chan_new i64 %0
    ret %1
}";
    let err_chan = execute(&parse_function(ssa_chan_neg).unwrap(), vec![], &mut platform, 100, false).unwrap_err();
    assert!(matches!(err_chan, VmError::InvalidInstruction { .. }));

    // Struct field out-of-bounds index
    let schema_mod = "
schema Point {
    0: x i64,
    1: y i64,
}

fn point_oob() -> i64 {
bb0:
    %0 = struct_new Point
    %1 = struct_get %0 5
    ret %1
}";
    let mod_struct = parse_module(schema_mod).unwrap();
    let err_struct_oob = execute_module(&mod_struct, "point_oob", vec![], &mut platform, 100, false).unwrap_err();
    assert!(matches!(err_struct_oob, VmError::IndexOutOfBounds { .. }));

    // Struct field type mismatch
    let schema_type_mismatch = "
schema Point {
    0: x i64,
    1: y i64,
}

fn point_bad_type() -> Point {
bb0:
    %0 = struct_new Point
    %1 = const_str \"hello\"
    %2 = struct_set %0 0 %1
    ret %2
}";
    let mod_bad_type = parse_module(schema_type_mismatch).unwrap();
    let err_bad_type = execute_module(&mod_bad_type, "point_bad_type", vec![], &mut platform, 100, false).unwrap_err();
    assert!(matches!(err_bad_type, VmError::TypeMismatch { .. }));

    // Default value with schemas verification
    let default_point = default_value_for_type_with_schemas(&knr_core::Type::Struct("Point".to_string()), &mod_struct.schemas);
    if let Value::Struct { schema, fields } = default_point {
        assert_eq!(schema, "Point");
        assert_eq!(fields.len(), 2);
        assert_eq!(fields[0], Value::I64(0));
        assert_eq!(fields[1], Value::I64(0));
    } else {
        panic!("expected struct");
    }
}
