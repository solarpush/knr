use knr_core::parse_ssa;
use knr_interp::{execute_module, Value, VmError};
use knr_runtime::MockPlatform;

#[test]
fn test_struct_creation_mutation_and_read() {
    let ssa = r#"
schema Point {
  0: x i64,
  1: y i64,
}

fn compute_sum() -> i64 {
bb0:
  %0 = struct_new Point
  %1 = const_i64 15
  %2 = struct_set %0 0 %1
  %3 = const_i64 27
  %4 = struct_set %2 1 %3
  %5 = struct_get %4 0
  %6 = struct_get %4 1
  %7 = add %5 %6
  free %4
  ret %7
}
"#;

    let module = parse_ssa(ssa).expect("Failed to parse SSA");
    let mut platform = MockPlatform::new();

    let (res, _) = execute_module(&module, "compute_sum", vec![], &mut platform, 100, false)
        .expect("Execution should succeed");

    assert_eq!(res, Value::I64(42));
}

#[test]
fn test_struct_gas_metering() {
    let ssa = r#"
schema Item {
  0: val i64,
}

fn process_item() -> i64 {
bb0:
  %0 = struct_new Item
  %1 = const_i64 99
  %2 = struct_set %0 0 %1
  %3 = struct_get %2 0
  free %2
  ret %3
}
"#;

    let module = parse_ssa(ssa).expect("Failed to parse SSA");
    let mut platform = MockPlatform::new();

    // The function has 5 instructions (struct_new, const_i64, struct_set, struct_get, free) + 1 terminator (ret) = 6 steps.
    // If gas limit is 3, it must fail with GasExhausted.
    let err = execute_module(&module, "process_item", vec![], &mut platform, 3, false)
        .expect_err("Gas limit 3 should fail");

    assert_eq!(err, VmError::GasExhausted { max_steps: 3 });

    // With 6 steps, it should succeed.
    let (res, _) = execute_module(&module, "process_item", vec![], &mut platform, 6, false)
        .expect("Exact gas steps should succeed");
    assert_eq!(res, Value::I64(99));
}

#[test]
fn test_struct_trace_json_output() {
    let ssa = r#"
schema Point {
  0: x i64,
  1: y i64,
}

fn trace_point() -> i64 {
bb0:
  %0 = struct_new Point
  %1 = const_i64 10
  %2 = struct_set %0 0 %1
  %3 = struct_get %2 0
  free %2
  ret %3
}
"#;

    let module = parse_ssa(ssa).expect("Failed to parse SSA");
    let mut platform = MockPlatform::new();

    let (res, traces) = execute_module(&module, "trace_point", vec![], &mut platform, 100, true)
        .expect("Execution with trace should succeed");

    assert_eq!(res, Value::I64(10));
    assert!(!traces.is_empty(), "Trace records should not be empty");

    // Check struct_new trace
    let new_trace = traces.iter().find(|t| t.opcode == "struct_new").expect("struct_new trace record");
    assert_eq!(new_trace.dst_reg.as_deref(), Some("%0"));
    let val_json = new_trace.new_value.as_ref().expect("new_value json");
    assert_eq!(val_json["schema"], "Point");

    // Check struct_set trace
    let set_trace = traces.iter().find(|t| t.opcode == "struct_set").expect("struct_set trace record");
    assert_eq!(set_trace.dst_reg.as_deref(), Some("%2"));
    let set_val = set_trace.new_value.as_ref().expect("struct_set value json");
    assert_eq!(set_val["fields"][0], 10);

    // Check struct_get trace
    let get_trace = traces.iter().find(|t| t.opcode == "struct_get").expect("struct_get trace record");
    assert_eq!(get_trace.dst_reg.as_deref(), Some("%3"));
    assert_eq!(get_trace.new_value.as_ref().unwrap(), &serde_json::json!(10));

    // Ensure all traces serialize to valid JSON
    let serialized = serde_json::to_string(&traces).expect("Traces must be JSON serializable");
    assert!(!serialized.is_empty());
}

#[test]
fn test_returning_struct_instance() {
    let ssa = r#"
schema Vector2 {
  0: x i64,
  1: y i64,
}

fn create_vec(%0: i64, %1: i64) -> struct[Vector2] {
bb0:
  %2 = struct_new Vector2
  %3 = struct_set %2 0 %0
  %4 = struct_set %3 1 %1
  ret %4
}
"#;

    let module = parse_ssa(ssa).expect("Failed to parse SSA");
    let mut platform = MockPlatform::new();

    let (res, _) = execute_module(
        &module,
        "create_vec",
        vec![Value::I64(100), Value::I64(200)],
        &mut platform,
        100,
        false,
    )
    .expect("Execution should succeed");

    match res {
        Value::Struct { schema, fields } => {
            assert_eq!(schema, "Vector2");
            assert_eq!(fields.len(), 2);
            assert_eq!(fields[0], Value::I64(100));
            assert_eq!(fields[1], Value::I64(200));
        }
        other => panic!("Expected Value::Struct, got: {:?}", other),
    }
}

#[test]
fn test_nested_structs_execution() {
    let ssa = r#"
schema Inner {
  0: val i64,
}

schema Outer {
  0: inner struct[Inner],
  1: tag str,
}

fn test_nested() -> i64 {
bb0:
  %0 = struct_new Inner
  %1 = const_i64 999
  %2 = struct_set %0 0 %1
  %3 = struct_new Outer
  %4 = struct_set %3 0 %2
  %5 = const_str "hello"
  %6 = struct_set %4 1 %5
  %7 = struct_get %6 0
  %8 = struct_get %7 0
  free %6
  ret %8
}
"#;

    let module = parse_ssa(ssa).expect("Failed to parse nested SSA");
    let mut platform = MockPlatform::new();

    let (res, _) = execute_module(&module, "test_nested", vec![], &mut platform, 100, false)
        .expect("Execution should succeed");

    assert_eq!(res, Value::I64(999));
}

#[test]
fn test_nested_struct_default_initialization() {
    let ssa = r#"
schema Inner {
  0: val i64,
}

schema Outer {
  0: inner struct[Inner],
}

fn test_defaults() -> i64 {
bb0:
  %0 = struct_new Outer
  %1 = struct_get %0 0
  %2 = struct_get %1 0
  free %0
  ret %2
}
"#;
    let module = parse_ssa(ssa).expect("Failed to parse SSA");
    let mut platform = MockPlatform::new();

    let (res, _) = execute_module(&module, "test_defaults", vec![], &mut platform, 100, false)
        .expect("Execution should succeed without IndexOutOfBounds");

    assert_eq!(res, Value::I64(0));
}

#[test]
fn test_execute_with_schemas_standalone() {
    let ssa = r#"
schema Point {
  0: x i64,
  1: y i64,
}

fn standalone_run() -> i64 {
bb0:
  %0 = struct_new Point
  %1 = const_i64 123
  %2 = struct_set %0 0 %1
  %3 = struct_get %2 0
  free %2
  ret %3
}
"#;
    let module = parse_ssa(ssa).expect("Failed to parse SSA");
    let func = &module.functions[0];
    let mut platform = MockPlatform::new();

    let (res, _) = knr_interp::execute_with_schemas(
        func,
        &module.schemas,
        vec![],
        &mut platform,
        100,
        false,
    )
    .expect("Standalone execute_with_schemas should succeed");

    assert_eq!(res, Value::I64(123));
}

#[test]
fn test_deeply_nested_struct_execution() {
    let ssa = r#"
schema Level1 {
  0: count i64,
}

schema Level2 {
  0: l1 struct[Level1],
}

schema Level3 {
  0: l2 struct[Level2],
}

fn deep_test() -> i64 {
bb0:
  %0 = struct_new Level3
  %1 = const_i64 777
  %2 = struct_new Level1
  %3 = struct_set %2 0 %1
  %4 = struct_new Level2
  %5 = struct_set %4 0 %3
  %6 = struct_set %0 0 %5
  %7 = struct_get %6 0
  %8 = struct_get %7 0
  %9 = struct_get %8 0
  free %6
  ret %9
}
"#;
    let module = parse_ssa(ssa).expect("Failed to parse SSA");
    let mut platform = MockPlatform::new();

    let (res, _) = execute_module(&module, "deep_test", vec![], &mut platform, 100, false)
        .expect("Execution should succeed");

    assert_eq!(res, Value::I64(777));
}

#[test]
fn test_sparse_schema_bounds_checking() {
    let ssa = r#"
schema Sparse {
  0: first i64,
  5: fifth i64,
}

fn test_sparse_get() -> i64 {
bb0:
  %0 = struct_new Sparse
  %1 = struct_get %0 2
  free %0
  ret %1
}
"#;
    let module = parse_ssa(ssa).expect("Failed to parse SSA");
    let mut platform = MockPlatform::new();

    let err = execute_module(&module, "test_sparse_get", vec![], &mut platform, 100, false)
        .expect_err("Undeclared intermediate field index must fail with IndexOutOfBounds");

    match err {
        VmError::IndexOutOfBounds { index, .. } => assert_eq!(index, 2),
        other => panic!("Expected VmError::IndexOutOfBounds, got: {:?}", other),
    }

    let ssa_set = r#"
schema Sparse {
  0: first i64,
  5: fifth i64,
}

fn test_sparse_set() -> struct[Sparse] {
bb0:
  %0 = struct_new Sparse
  %1 = const_i64 42
  %2 = struct_set %0 2 %1
  ret %2
}
"#;
    let module_set = parse_ssa(ssa_set).expect("Failed to parse SSA");
    let err_set = execute_module(&module_set, "test_sparse_set", vec![], &mut platform, 100, false)
        .expect_err("Setting undeclared intermediate field must fail with IndexOutOfBounds");

    match err_set {
        VmError::IndexOutOfBounds { index, .. } => assert_eq!(index, 2),
        other => panic!("Expected VmError::IndexOutOfBounds, got: {:?}", other),
    }
}

#[test]
fn test_struct_set_out_of_bounds_rejected() {
    let ssa = r#"
schema Pair {
  0: x i64,
  1: y i64,
}

fn test_oob_set() -> struct[Pair] {
bb0:
  %0 = struct_new Pair
  %1 = const_i64 999
  %2 = struct_set %0 99 %1
  ret %2
}
"#;
    let module = parse_ssa(ssa).expect("Failed to parse SSA");
    let mut platform = MockPlatform::new();

    let err = execute_module(&module, "test_oob_set", vec![], &mut platform, 100, false)
        .expect_err("Out of bounds struct_set must fail with IndexOutOfBounds");

    match err {
        VmError::IndexOutOfBounds { index, .. } => assert_eq!(index, 99),
        other => panic!("Expected VmError::IndexOutOfBounds, got: {:?}", other),
    }
}

#[test]
fn test_struct_set_type_mismatch_at_runtime() {
    let ssa = r#"
schema Typed {
  0: count i64,
}

fn test_bad_set() -> struct[Typed] {
bb0:
  %0 = struct_new Typed
  %1 = const_str "not an integer"
  %2 = struct_set %0 0 %1
  ret %2
}
"#;
    let module = parse_ssa(ssa).expect("Failed to parse SSA");
    let mut platform = MockPlatform::new();

    let err = execute_module(&module, "test_bad_set", vec![], &mut platform, 100, false)
        .expect_err("Storing string in i64 field must fail with TypeMismatch");

    match err {
        VmError::TypeMismatch { expected, actual, .. } => {
            assert_eq!(expected, "i64");
            assert_eq!(actual, "str");
        }
        other => panic!("Expected VmError::TypeMismatch, got: {:?}", other),
    }
}

#[test]
fn test_default_value_for_type_with_schemas_dos_protection() {
    use knr_core::{FieldDef, Schema, Type};
    use knr_interp::default_value_for_type_with_schemas;

    let huge_schema = Schema {
        name: "Huge".to_string(),
        fields: vec![FieldDef {
            index: 70000,
            name: Some("x".to_string()),
            ty: Type::I64,
        }],
    };

    let val = default_value_for_type_with_schemas(&Type::Struct("Huge".to_string()), &[huge_schema]);
    match val {
        Value::Struct { fields, .. } => {
            assert!(fields.len() <= 65536, "Fields length must be capped at 65536 to prevent DoS OOM");
        }
        other => panic!("Expected Value::Struct, got: {:?}", other),
    }
}



