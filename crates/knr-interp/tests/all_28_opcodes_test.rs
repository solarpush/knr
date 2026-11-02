use knr_core::{parse_ssa, Effect};
use knr_interp::{execute_module, Value};
use knr_runtime::MockPlatform;

const ALL_28_OPCODES_SSA: &str = r#"
fn worker(%0: chan[i64]) -> void effects(Async) {
bb0:
    ret
}

fn test_all_opcodes(%0: i64) -> i64 effects(Net, Ui, Hw, Async) {
bb0:
    %1 = const_i64 100
    %2 = const_f64 3.14
    %3 = const_bool true
    %4 = const_str "hello\nworld"
    %5 = add %0 %1
    %6 = sub %5 %1
    %7 = mul %6 %1
    %8 = div %7 %1
    %9 = cmp_eq %8 %1
    %10 = cmp_lt %8 %1
    %11 = buf_alloc %1
    %12 = const_i64 0
    %13 = const_i64 42
    %14 = buf_write_u8 %11 %12 %13
    %15 = buf_read_u8 %14 %12
    %16 = buf_slice %14 %12 %1
    free %16
    %17 = vec_new i64
    %18 = vec_push %17 %1
    %19 = vec_get %18 %12
    %20 = vec_len %18
    free %18
    %21 = map_new str i64
    %22 = map_set %21 %4 %1
    %23 = map_get %22 %4
    %24 = map_del %22 %4
    free %24
    %25 = chan_new i64 %1
    %26 = chan_send %25 %1
    %27 = chan_recv %25
    %28 = spawn "worker" %25
    free %25
    %29 = syscall Net "net.listen" %1
    free %4
    br_if %9 bb1 bb2
bb1:
    br bb2
bb2:
    ret %15
}
"#;

#[test]
fn test_all_28_opcodes_execution() {
    let module = parse_ssa(ALL_28_OPCODES_SSA).expect("Valid SSA should parse");
    let mut platform = MockPlatform::new();
    platform.set_response(Effect::Net, "net.listen", Value::I64(0));

    let (result, trace) = execute_module(
        &module,
        "test_all_opcodes",
        vec![Value::I64(100)],
        &mut platform,
        10_000,
        true,
    )
    .expect("All 28 opcodes should execute successfully");

    // %15 is the read byte which is 42
    assert_eq!(result, Value::I64(42));

    // Verify trace was collected
    assert!(!trace.is_empty());
    assert!(trace.iter().any(|t| t.opcode == "buf_alloc"));
    assert!(trace.iter().any(|t| t.opcode == "buf_read_u8"));
    assert!(trace.iter().any(|t| t.opcode == "buf_write_u8"));
    assert!(trace.iter().any(|t| t.opcode == "buf_slice"));
    assert!(trace.iter().any(|t| t.opcode == "vec_new"));
    assert!(trace.iter().any(|t| t.opcode == "map_set"));
    assert!(trace.iter().any(|t| t.opcode == "chan_new"));
    assert!(trace.iter().any(|t| t.opcode == "spawn"));
    assert!(trace.iter().any(|t| t.opcode == "syscall"));
    assert!(trace.iter().any(|t| t.opcode == "free"));
    assert!(trace.iter().any(|t| t.opcode == "br_if"));
    assert!(trace.iter().any(|t| t.opcode == "ret"));
}
