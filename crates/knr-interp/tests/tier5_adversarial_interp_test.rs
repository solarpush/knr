use knr_core::parser::parse_function;
use knr_interp::{execute, execute_module, Value, VmError};
use knr_runtime::{MockPlatform, RuntimeError};

#[test]
fn test_tier5_gas_exact_boundaries() {
    let mut platform = MockPlatform::new();

    // Program:
    // %0 = const_i64 42  (step 1)
    // ret %0              (step 2)
    let ssa = "fn test() -> i64 {
bb0:
    %0 = const_i64 42
    ret %0
}";
    let func = parse_function(ssa).unwrap();

    // max_steps = 0 -> immediate GasExhausted before step 1
    let err0 = execute(&func, vec![], &mut platform, 0, false).unwrap_err();
    assert_eq!(err0, VmError::GasExhausted { max_steps: 0 });

    // max_steps = 1 -> executes instruction 1, then exhausts at terminator (step 2)
    let err1 = execute(&func, vec![], &mut platform, 1, false).unwrap_err();
    assert_eq!(err1, VmError::GasExhausted { max_steps: 1 });

    // max_steps = 2 -> exact steps needed, succeeds!
    let (val, trace) = execute(&func, vec![], &mut platform, 2, false).unwrap();
    assert_eq!(val, Value::I64(42));
    assert!(trace.is_empty());

    // Multi-block infinite loop with max_steps = 5
    let ssa_loop = "fn inf_loop() -> void {
bb0:
    br bb1
bb1:
    br bb0
}";
    let func_loop = parse_function(ssa_loop).unwrap();
    let err_loop = execute(&func_loop, vec![], &mut platform, 5, false).unwrap_err();
    assert_eq!(err_loop, VmError::GasExhausted { max_steps: 5 });
}

#[test]
fn test_tier5_arithmetic_extremes() {
    let mut platform = MockPlatform::new();

    // i64::MIN / -1: must wrap to i64::MIN without panicking
    let ssa_div_min = "fn div_min() -> i64 {
bb0:
    %0 = const_i64 -9223372036854775808
    %1 = const_i64 -1
    %2 = div %0 %1
    ret %2
}";
    let func_div_min = parse_function(ssa_div_min).unwrap();
    let (val_min, _) = execute(&func_div_min, vec![], &mut platform, 100, false).unwrap();
    assert_eq!(val_min, Value::I64(i64::MIN));

    // Division by zero in i64 -> VmError::DivisionByZero
    let ssa_div_zero = "fn div_zero() -> i64 {
bb0:
    %0 = const_i64 100
    %1 = const_i64 0
    %2 = div %0 %1
    ret %2
}";
    let func_div_zero = parse_function(ssa_div_zero).unwrap();
    let err_zero = execute(&func_div_zero, vec![], &mut platform, 100, false).unwrap_err();
    assert!(matches!(err_zero, VmError::DivisionByZero { .. }));

    // Float division by zero: produces Infinity and -Infinity
    let ssa_fdiv = "fn fdiv() -> f64 {
bb0:
    %0 = const_f64 1.0
    %1 = const_f64 0.0
    %2 = div %0 %1
    ret %2
}";
    let func_fdiv = parse_function(ssa_fdiv).unwrap();
    let (fval, _) = execute(&func_fdiv, vec![], &mut platform, 100, false).unwrap();
    if let Value::F64(f) = fval {
        assert!(f.is_infinite() && f.is_sign_positive());
    } else {
        panic!("expected f64");
    }

    // Wrapping overflow: i64::MAX + 1 -> i64::MIN
    let ssa_wrap_add = "fn wrap_add() -> i64 {
bb0:
    %0 = const_i64 9223372036854775807
    %1 = const_i64 1
    %2 = add %0 %1
    ret %2
}";
    let (v_wrap, _) = execute(&parse_function(ssa_wrap_add).unwrap(), vec![], &mut platform, 100, false).unwrap();
    assert_eq!(v_wrap, Value::I64(i64::MIN));

    // Wrapping underflow: i64::MIN - 1 -> i64::MAX
    let ssa_wrap_sub = "fn wrap_sub() -> i64 {
bb0:
    %0 = const_i64 -9223372036854775808
    %1 = const_i64 1
    %2 = sub %0 %1
    ret %2
}";
    let (v_sub, _) = execute(&parse_function(ssa_wrap_sub).unwrap(), vec![], &mut platform, 100, false).unwrap();
    assert_eq!(v_sub, Value::I64(i64::MAX));

    // NaN comparisons:
    // In knr-interp VM, Op::CmpEq uses standard IEEE-754 semantics: NaN == NaN is false
    let ssa_nan = "fn nan_cmp() -> bool {
bb0:
    %0 = const_f64 NaN
    %1 = const_f64 NaN
    %2 = cmp_eq %0 %1
    ret %2
}";
    let (v_nan, _) = execute(&parse_function(ssa_nan).unwrap(), vec![], &mut platform, 100, false).unwrap();
    assert_eq!(v_nan, Value::Bool(false), "VM uses standard IEEE-754 semantics: NaN == NaN is false");

    // NaN == 0.0 is false
    let ssa_nan_zero = "fn nan_zero() -> bool {
bb0:
    %0 = const_f64 NaN
    %1 = const_f64 0.0
    %2 = cmp_eq %0 %1
    ret %2
}";
    let (v_nz, _) = execute(&parse_function(ssa_nan_zero).unwrap(), vec![], &mut platform, 100, false).unwrap();
    assert_eq!(v_nz, Value::Bool(false));

    // NaN < 1.0 is false
    let ssa_nan_lt = "fn nan_lt() -> bool {
bb0:
    %0 = const_f64 NaN
    %1 = const_f64 1.0
    %2 = cmp_lt %0 %1
    ret %2
}";
    let (v_nlt, _) = execute(&parse_function(ssa_nan_lt).unwrap(), vec![], &mut platform, 100, false).unwrap();
    assert_eq!(v_nlt, Value::Bool(false));

    // -0.0 == +0.0 is true in IEEE-754
    let ssa_zero_cmp = "fn zero_cmp() -> bool {
bb0:
    %0 = const_f64 -0.0
    %1 = const_f64 0.0
    %2 = cmp_eq %0 %1
    ret %2
}";
    let (v_zcmp, _) = execute(&parse_function(ssa_zero_cmp).unwrap(), vec![], &mut platform, 100, false).unwrap();
    assert_eq!(v_zcmp, Value::Bool(true));
}

#[test]
fn test_tier5_buffer_and_slice_boundaries() {
    let mut platform = MockPlatform::new();

    // buf_alloc 0 produces empty buffer
    let ssa_empty = "fn buf_empty() -> i64 {
bb0:
    %0 = const_i64 0
    %1 = buf_alloc %0
    %2 = const_i64 0
    %3 = buf_read_u8 %1 %2
    free %1
    ret %3
}";
    // Reading index 0 of empty buffer must be IndexOutOfBounds
    let err_empty = execute(&parse_function(ssa_empty).unwrap(), vec![], &mut platform, 100, false).unwrap_err();
    assert!(matches!(err_empty, VmError::IndexOutOfBounds { .. }));

    // buf_alloc with negative size returns InvalidInstruction
    let ssa_neg = "fn buf_neg() -> buf {
bb0:
    %0 = const_i64 -10
    %1 = buf_alloc %0
    ret %1
}";
    let err_neg = execute(&parse_function(ssa_neg).unwrap(), vec![], &mut platform, 100, false).unwrap_err();
    assert!(matches!(err_neg, VmError::InvalidInstruction { .. }));

    // buf_write_u8 masking value > 255
    let ssa_mask = "fn buf_mask() -> i64 {
bb0:
    %0 = const_i64 4
    %1 = buf_alloc %0
    %2 = const_i64 0
    %3 = const_i64 300
    %4 = buf_write_u8 %1 %2 %3
    %5 = buf_read_u8 %4 %2
    free %4
    ret %5
}";
    let (val_mask, _) = execute(&parse_function(ssa_mask).unwrap(), vec![], &mut platform, 100, false).unwrap();
    assert_eq!(val_mask, Value::I64(300 & 0xFF));

    // buf_slice with negative start
    let ssa_slice_neg_start = "fn slice_neg() -> buf {
bb0:
    %0 = const_i64 16
    %1 = buf_alloc %0
    %2 = const_i64 -1
    %3 = const_i64 5
    %4 = buf_slice %1 %2 %3
    free %1
    ret %4
}";
    let err_sl1 = execute(&parse_function(ssa_slice_neg_start).unwrap(), vec![], &mut platform, 100, false).unwrap_err();
    assert!(matches!(err_sl1, VmError::IndexOutOfBounds { .. }));

    // buf_slice with start + len > buf.len()
    let ssa_slice_oob = "fn slice_oob() -> buf {
bb0:
    %0 = const_i64 16
    %1 = buf_alloc %0
    %2 = const_i64 10
    %3 = const_i64 10
    %4 = buf_slice %1 %2 %3
    free %1
    ret %4
}";
    let err_sl2 = execute(&parse_function(ssa_slice_oob).unwrap(), vec![], &mut platform, 100, false).unwrap_err();
    assert!(matches!(err_sl2, VmError::IndexOutOfBounds { .. }));

    // buf_slice with arithmetic overflow: start = i64::MAX, len = 1
    let ssa_slice_overflow = "fn slice_ovf() -> buf {
bb0:
    %0 = const_i64 16
    %1 = buf_alloc %0
    %2 = const_i64 9223372036854775807
    %3 = const_i64 1
    %4 = buf_slice %1 %2 %3
    free %1
    ret %4
}";
    let err_sl3 = execute(&parse_function(ssa_slice_overflow).unwrap(), vec![], &mut platform, 100, false).unwrap_err();
    assert!(matches!(err_sl3, VmError::IndexOutOfBounds { .. }));

    // buf_slice 0-length slice at end of buffer: start = 16, len = 0 (valid)
    let ssa_slice_end_zero = "fn slice_end() -> buf {
bb0:
    %0 = const_i64 16
    %1 = buf_alloc %0
    %2 = const_i64 16
    %3 = const_i64 0
    %4 = buf_slice %1 %2 %3
    free %1
    ret %4
}";
    let (slice_val, _) = execute(&parse_function(ssa_slice_end_zero).unwrap(), vec![], &mut platform, 100, false).unwrap();
    if let Value::Buf(b) = slice_val {
        assert_eq!(b.len(), 0);
    } else {
        panic!("expected buf");
    }
}

#[test]
fn test_tier5_collections_edge_cases() {
    let mut platform = MockPlatform::new();

    // vec_get negative index -> IndexOutOfBounds
    let ssa_vec_neg = "fn vec_neg() -> i64 {
bb0:
    %0 = vec_new i64
    %1 = const_i64 -1
    %2 = vec_get %0 %1
    free %0
    ret %2
}";
    let err_vn = execute(&parse_function(ssa_vec_neg).unwrap(), vec![], &mut platform, 100, false).unwrap_err();
    assert!(matches!(err_vn, VmError::IndexOutOfBounds { .. }));

    // vec_len on empty vector -> 0
    let ssa_vec_len = "fn vec_len_test() -> i64 {
bb0:
    %0 = vec_new i64
    %1 = vec_len %0
    free %0
    ret %1
}";
    let (vlen, _) = execute(&parse_function(ssa_vec_len).unwrap(), vec![], &mut platform, 100, false).unwrap();
    assert_eq!(vlen, Value::I64(0));

    // map_get missing key returns Value::Void
    let ssa_map_miss = "fn map_miss() -> i64 {
bb0:
    %0 = map_new str i64
    %1 = const_str \"missing\"
    %2 = map_get %0 %1
    free %0
    ret %2
}";
    let (vmiss, _) = execute(&parse_function(ssa_map_miss).unwrap(), vec![], &mut platform, 100, false).unwrap();
    assert_eq!(vmiss, Value::Void);

    // map_del on missing key preserves map intact
    let ssa_map_del = "fn map_del_test() -> map[str, i64] {
bb0:
    %0 = map_new str i64
    %1 = const_str \"absent\"
    %2 = map_del %0 %1
    ret %2
}";
    let (vdel, _) = execute(&parse_function(ssa_map_del).unwrap(), vec![], &mut platform, 100, false).unwrap();
    if let Value::Map(m) = vdel {
        assert!(m.is_empty());
    } else {
        panic!("expected map");
    }

    // Channel FIFO ordering and negative capacity rejection
    let ssa_chan_neg = "fn chan_neg() -> chan[i64] effects(Async) {
bb0:
    %0 = const_i64 -1
    %1 = chan_new i64 %0
    ret %1
}";
    let err_cn = execute(&parse_function(ssa_chan_neg).unwrap(), vec![], &mut platform, 100, false).unwrap_err();
    assert!(matches!(err_cn, VmError::InvalidInstruction { .. }));

    // Channel recv on empty channel returns Value::Void
    let ssa_chan_empty = "fn chan_empty() -> i64 effects(Async) {
bb0:
    %0 = const_i64 4
    %1 = chan_new i64 %0
    %2 = chan_recv %1
    free %1
    ret %2
}";
    let (crecv, _) = execute(&parse_function(ssa_chan_empty).unwrap(), vec![], &mut platform, 100, false).unwrap();
    assert_eq!(crecv, Value::Void);
}

#[test]
fn test_tier5_control_flow_and_errors() {
    let mut platform = MockPlatform::new();

    // Argument count mismatch
    let ssa_args = "fn test_args(%0: i64) -> i64 {
bb0:
    ret %0
}";
    let func = parse_function(ssa_args).unwrap();
    let err_arg = execute(&func, vec![Value::I64(1), Value::I64(2)], &mut platform, 100, false).unwrap_err();
    assert!(matches!(err_arg, VmError::InvalidInstruction { .. }));

    // br to non-existent block
    let ssa_bad_br = "fn bad_br() -> void {
bb0:
    br bb99
}";
    let func_br = parse_function(ssa_bad_br).unwrap();
    let err_br = execute(&func_br, vec![], &mut platform, 100, false).unwrap_err();
    assert_eq!(err_br, VmError::NoSuchBlock { block_id: 99 });

    // br_if condition not bool
    let ssa_bad_brif = "fn bad_brif(%0: i64) -> void {
bb0:
    br_if %0 bb1 bb1
bb1:
    ret
}";
    let func_brif = parse_function(ssa_bad_brif).unwrap();
    let err_brif = execute(&func_brif, vec![Value::I64(1)], &mut platform, 100, false).unwrap_err();
    assert!(matches!(err_brif, VmError::TypeMismatch { .. }));

    // ret with invalid register
    let ssa_bad_ret = "fn bad_ret() -> i64 {
bb0:
    ret %99
}";
    let func_ret = parse_function(ssa_bad_ret).unwrap();
    let err_ret = execute(&func_ret, vec![], &mut platform, 100, false).unwrap_err();
    assert_eq!(err_ret, VmError::InvalidRegisterAccess { reg: 99 });
}

#[test]
fn test_tier5_trace_records_json_serialization() {
    let mut platform = MockPlatform::new();

    let ssa = "fn trace_prog() -> i64 {
bb0:
    %0 = const_i64 10
    %1 = const_i64 20
    %2 = add %0 %1
    ret %2
}";
    let func = parse_function(ssa).unwrap();
    let (res, trace) = execute(&func, vec![], &mut platform, 100, true).unwrap();
    assert_eq!(res, Value::I64(30));

    // Must have 4 trace records: 3 instructions + 1 terminator
    assert_eq!(trace.len(), 4);
    assert_eq!(trace[0].step, 1);
    assert_eq!(trace[0].opcode, "const_i64");
    assert_eq!(trace[1].step, 2);
    assert_eq!(trace[2].step, 3);
    assert_eq!(trace[2].opcode, "add");
    assert_eq!(trace[3].step, 4);
    assert_eq!(trace[3].opcode, "ret");

    // Check JSON serialization of trace
    let json = serde_json::to_string(&trace).expect("trace records must serialize to JSON");
    assert!(json.contains("\"opcode\":\"add\""));
    assert!(json.contains("\"step\":3"));
}

struct FailingPlatform;

impl knr_runtime::HostPlatform for FailingPlatform {
    fn host_call(
        &mut self,
        _effect: knr_core::Effect,
        _op: &str,
        _payload: Value,
    ) -> Result<Value, RuntimeError> {
        Err(RuntimeError::Platform("IO failure".into()))
    }
}

#[test]
fn test_tier5_host_call_error_and_module_execution() {
    let mut failing_platform = FailingPlatform;

    let ssa_hw = "fn test_hw(%0: i64) -> buf effects(Hw) {
bb0:
    %1 = syscall Hw \"hw.bad\" %0
    ret %1
}";
    let func_hw = parse_function(ssa_hw).unwrap();
    let err_hw = execute(&func_hw, vec![Value::I64(0)], &mut failing_platform, 100, false).unwrap_err();
    assert!(matches!(err_hw, VmError::HostCallError(_)));

    // execute_module with non-existent function name
    let mut mock = MockPlatform::new();
    let mod_ssa = knr_core::parser::parse_ssa("fn entry() -> void {\nbb0:\n    ret\n}").unwrap();
    let err_mod = execute_module(&mod_ssa, "nonexistent", vec![], &mut mock, 100, false).unwrap_err();
    assert_eq!(err_mod, VmError::NoSuchFunction { name: "nonexistent".into() });
}
