use knr_core::parse_ssa;
use knr_verify::verify_module;

#[test]
fn test_valid_struct_program() {
    let ssa = r#"
schema Point {
  0: x i64,
  1: y i64,
}

fn create_and_use_point() -> i64 {
bb0:
  %0 = struct_new Point
  %1 = const_i64 10
  %2 = struct_set %0 0 %1
  %3 = const_i64 20
  %4 = struct_set %2 1 %3
  %5 = struct_get %4 0
  %6 = struct_get %4 1
  %7 = add %5 %6
  free %4
  ret %7
}
"#;

    let module = parse_ssa(ssa).expect("Failed to parse valid SSA");
    let result = verify_module(&module);
    assert!(result.is_ok(), "Expected verification to pass, got: {:?}", result.err());
}

#[test]
fn test_struct_borrow_does_not_consume() {
    let ssa = r#"
schema Pair {
  0: first i64,
  1: second i64,
}

fn read_twice(%0: struct[Pair]) -> i64 {
bb0:
  %1 = struct_get %0 0
  %2 = struct_get %0 1
  %3 = struct_get %0 0
  %4 = add %1 %2
  %5 = add %4 %3
  ret %5
}
"#;

    let module = parse_ssa(ssa).expect("Failed to parse SSA");
    let result = verify_module(&module);
    assert!(result.is_ok(), "struct_get should borrow without consuming");
}

#[test]
fn test_struct_use_after_move() {
    let ssa = r#"
schema Point {
  0: x i64,
  1: y i64,
}

fn bad_use_after_move() -> i64 {
bb0:
  %0 = struct_new Point
  %1 = const_i64 100
  %2 = struct_set %0 0 %1
  %3 = struct_get %0 0
  free %2
  ret %3
}
"#;

    let module = parse_ssa(ssa).expect("Failed to parse SSA");
    let diagnostics = verify_module(&module).expect_err("Expected UseAfterMove error");
    assert_eq!(diagnostics.len(), 1);

    let diag = &diagnostics[0];
    assert_eq!(diag.error_kind, "UseAfterMove");
    assert_eq!(diag.violated_register.as_deref(), Some("%0"));
    assert!(diag.consumed_at_line.is_some());
    assert!(diag.fix_hint.is_some());

    // Validate structured JSON diagnostic
    let json_str = diag.to_json().expect("Serialization to JSON failed");
    let v: serde_json::Value = serde_json::from_str(&json_str).expect("Valid JSON required");
    assert_eq!(v["status"], "error");
    assert_eq!(v["error_kind"], "UseAfterMove");
    assert_eq!(v["violated_register"], "%0");
    assert!(v["fix_hint"].as_str().unwrap().contains("already moved"));
}

#[test]
fn test_struct_linear_leak() {
    let ssa = r#"
schema Resource {
  0: id i64,
}

fn leaking_struct() -> i64 {
bb0:
  %0 = struct_new Resource
  %1 = const_i64 42
  ret %1
}
"#;

    let module = parse_ssa(ssa).expect("Failed to parse SSA");
    let diagnostics = verify_module(&module).expect_err("Expected LinearLeak error");
    assert_eq!(diagnostics.len(), 1);

    let diag = &diagnostics[0];
    assert_eq!(diag.error_kind, "LinearLeak");
    assert_eq!(diag.violated_register.as_deref(), Some("%0"));
    assert!(diag.fix_hint.is_some());

    let json_str = diag.to_json().expect("Serialization to JSON failed");
    let v: serde_json::Value = serde_json::from_str(&json_str).expect("Valid JSON required");
    assert_eq!(v["status"], "error");
    assert_eq!(v["error_kind"], "LinearLeak");
    assert_eq!(v["violated_register"], "%0");
    assert!(v["fix_hint"].as_str().unwrap().contains("not freed or returned"));
}

#[test]
fn test_struct_field_type_mismatch() {
    let ssa = r#"
schema Config {
  0: timeout i64,
}

fn set_wrong_type() -> struct[Config] {
bb0:
  %0 = struct_new Config
  %1 = const_f64 3.14
  %2 = struct_set %0 0 %1
  ret %2
}
"#;

    let module = parse_ssa(ssa).expect("Failed to parse SSA");
    let diagnostics = verify_module(&module).expect_err("Expected TypeMismatch error");
    assert_eq!(diagnostics.len(), 1);

    let diag = &diagnostics[0];
    assert_eq!(diag.error_kind, "TypeMismatch");
    assert_eq!(diag.violated_register.as_deref(), Some("%1"));
    assert!(diag.fix_hint.is_some());

    let json_str = diag.to_json().expect("Serialization to JSON failed");
    let v: serde_json::Value = serde_json::from_str(&json_str).expect("Valid JSON required");
    assert_eq!(v["error_kind"], "TypeMismatch");
    assert!(v["fix_hint"].as_str().unwrap().contains("expected type i64, got f64"));
}

#[test]
fn test_struct_undeclared_field_index() {
    let ssa = r#"
schema Point {
  0: x i64,
  1: y i64,
}

fn access_out_of_bounds() -> i64 {
bb0:
  %0 = struct_new Point
  %1 = struct_get %0 99
  free %0
  ret %1
}
"#;

    let module = parse_ssa(ssa).expect("Failed to parse SSA");
    let diagnostics = verify_module(&module).expect_err("Expected UndeclaredField error");
    assert_eq!(diagnostics.len(), 1);

    let diag = &diagnostics[0];
    assert_eq!(diag.error_kind, "UndeclaredField");
    assert_eq!(diag.violated_register.as_deref(), Some("%0"));
    assert!(diag.fix_hint.is_some());

    let json_str = diag.to_json().expect("Serialization to JSON failed");
    let v: serde_json::Value = serde_json::from_str(&json_str).expect("Valid JSON required");
    assert_eq!(v["error_kind"], "UndeclaredField");
    assert!(v["fix_hint"].as_str().unwrap().contains("Field index 99 is not declared"));
}

#[test]
fn test_unknown_schema_without_declared_schemas() {
    let ssa = r#"
fn bad_ghost_struct() -> i64 {
bb0:
  %0 = struct_new Ghost
  %1 = struct_get %0 0
  free %0
  ret %1
}
"#;
    let module = parse_ssa(ssa).expect("Failed to parse SSA");
    let diagnostics = verify_module(&module).expect_err("Expected UnknownSchema error when no schemas exist");
    assert!(!diagnostics.is_empty());
    let diag = &diagnostics[0];
    assert_eq!(diag.error_kind, "UnknownSchema");
    assert!(diag.fix_hint.as_deref().unwrap().contains("Ghost"));
    assert_eq!(diag.violated_register.as_deref(), Some("%0"));
}

#[test]
fn test_duplicate_schema_name_rejected() {
    let ssa = r#"
schema Foo {
  0: x i64,
}

schema Foo {
  0: y f64,
}

fn dummy() -> void {
bb0:
  ret
}
"#;
    let module = parse_ssa(ssa).expect("parse SSA");
    let diagnostics = verify_module(&module).expect_err("Duplicate schema must be rejected");
    assert!(diagnostics.iter().any(|d| d.error_kind == "DuplicateSchema"));
}

#[test]
fn test_duplicate_field_index_or_name_rejected() {
    let ssa_idx = r#"
schema DupIdx {
  0: a i64,
  0: b i64,
}

fn dummy() -> void {
bb0:
  ret
}
"#;
    let module = parse_ssa(ssa_idx).expect("parse SSA");
    let diagnostics = verify_module(&module).expect_err("Duplicate field index must be rejected");
    assert!(diagnostics.iter().any(|d| d.error_kind == "DuplicateField"));

    let ssa_name = r#"
schema DupName {
  0: val i64,
  1: val f64,
}

fn dummy() -> void {
bb0:
  ret
}
"#;
    let module = parse_ssa(ssa_name).expect("parse SSA");
    let diagnostics = verify_module(&module).expect_err("Duplicate field name must be rejected");
    assert!(diagnostics.iter().any(|d| d.error_kind == "DuplicateField"));
}

#[test]
fn test_schema_field_referencing_unknown_schema_rejected() {
    let ssa = r#"
schema Container {
  0: item struct[GhostItem],
}

fn dummy() -> void {
bb0:
  ret
}
"#;
    let module = parse_ssa(ssa).expect("parse SSA");
    let diagnostics = verify_module(&module).expect_err("Unknown schema in field must be rejected");
    assert!(diagnostics.iter().any(|d| d.error_kind == "UnknownSchema"));
}

#[test]
fn test_cyclic_recursive_schema_rejected() {
    let ssa_self = r#"
schema Node {
  0: next struct[Node],
}

fn dummy() -> void {
bb0:
  ret
}
"#;
    let module = parse_ssa(ssa_self).expect("parse SSA");
    let diagnostics = verify_module(&module).expect_err("Self-recursive schema must be rejected");
    assert!(diagnostics.iter().any(|d| d.error_kind == "RecursiveSchema"));

    let ssa_mutual = r#"
schema A {
  0: b struct[B],
}

schema B {
  0: a struct[A],
}

fn dummy() -> void {
bb0:
  ret
}
"#;
    let module = parse_ssa(ssa_mutual).expect("parse SSA");
    let diagnostics = verify_module(&module).expect_err("Mutually recursive schema must be rejected");
    assert!(diagnostics.iter().any(|d| d.error_kind == "RecursiveSchema"));
}

#[test]
fn test_parameter_referencing_unknown_schema_rejected() {
    let ssa = r#"
fn bad_param(%0: struct[GhostParam]) -> void {
bb0:
  ret
}
"#;
    let module = parse_ssa(ssa).expect("parse SSA");
    let diagnostics = verify_module(&module).expect_err("Unknown schema in parameter must be rejected");
    assert!(diagnostics.iter().any(|d| d.error_kind == "UnknownSchema" && d.violated_register.as_deref() == Some("%0")));
}

#[test]
fn test_instruction_type_referencing_unknown_schema_rejected() {
    // 1. vec_new struct[Ghost]
    let ssa_vec = r#"
fn bad_vec() -> void {
bb0:
  %0 = vec_new struct[GhostVec]
  free %0
  ret
}
"#;
    let module = parse_ssa(ssa_vec).expect("parse SSA");
    let diagnostics = verify_module(&module).expect_err("Unknown schema in vec_new must be rejected");
    let diag = diagnostics.iter().find(|d| d.error_kind == "UnknownSchema").expect("Must emit UnknownSchema");
    assert_eq!(diag.violated_register.as_deref(), Some("%0"));
    assert!(diag.fix_hint.as_deref().unwrap().contains("GhostVec"));

    // 2. chan_new struct[Ghost]
    let ssa_chan = r#"
fn bad_chan() -> void effects(Async) {
bb0:
  %0 = const_i64 10
  %1 = chan_new struct[GhostChan] %0
  free %1
  ret
}
"#;
    let module = parse_ssa(ssa_chan).expect("parse SSA");
    let diagnostics = verify_module(&module).expect_err("Unknown schema in chan_new must be rejected");
    let diag = diagnostics.iter().find(|d| d.error_kind == "UnknownSchema").expect("Must emit UnknownSchema");
    assert_eq!(diag.violated_register.as_deref(), Some("%1"));
    assert!(diag.fix_hint.as_deref().unwrap().contains("GhostChan"));
}

#[test]
fn test_struct_new_and_set_missing_dst_linear_leak() {
    // 1. struct_new Point without destination leaks linear struct immediately
    let ssa_new = r#"
schema Point {
  0: x i64,
}

fn leak_new() -> void {
bb0:
  struct_new Point
  ret
}
"#;
    let module = parse_ssa(ssa_new).expect("parse SSA");
    let diagnostics = verify_module(&module).expect_err("struct_new without dst must be rejected as LinearLeak");
    assert!(
        diagnostics.iter().any(|d| d.error_kind == "LinearLeak"),
        "Expected LinearLeak for struct_new without dst, got: {:?}",
        diagnostics
    );

    // 2. struct_set without destination leaks updated linear struct immediately
    let ssa_set = r#"
schema Point {
  0: x i64,
}

fn leak_set(%0: struct[Point], %1: i64) -> void {
bb0:
  struct_set %0 0 %1
  ret
}
"#;
    let module = parse_ssa(ssa_set).expect("parse SSA");
    let diagnostics = verify_module(&module).expect_err("struct_set without dst must be rejected as LinearLeak");
    assert!(
        diagnostics.iter().any(|d| d.error_kind == "LinearLeak"),
        "Expected LinearLeak for struct_set without dst, got: {:?}",
        diagnostics
    );
}

#[test]
fn test_struct_get_missing_dst_rejected() {
    let ssa = r#"
schema Point {
  0: x i64,
}

fn bad_get(%0: struct[Point]) -> void {
bb0:
  struct_get %0 0
  free %0
  ret
}
"#;
    let module = parse_ssa(ssa).expect("parse SSA");
    let diagnostics = verify_module(&module).expect_err("struct_get without dst must be rejected");
    assert!(
        diagnostics.iter().any(|d| d.error_kind == "TypeMismatch"),
        "Expected TypeMismatch for struct_get without dst, got: {:?}",
        diagnostics
    );
}

#[test]
fn test_schema_semantic_validation_diagnostics() {
    use knr_core::{BasicBlock, Function, Instruction, Module, Op, Schema, Terminator, Type};

    // 1. InvalidSchemaName
    let mut module_bad_name = Module::new();
    module_bad_name.add_schema(Schema::new("123BadName"));
    let err = verify_module(&module_bad_name).expect_err("Invalid schema name must fail");
    assert!(err.iter().any(|d| d.error_kind == "InvalidSchemaName"));

    // 2. InvalidFieldName
    let mut module_bad_field = Module::new();
    let mut s = Schema::new("GoodSchema");
    s.add_field(0, Some("bad field with spaces".to_string()), Type::I64);
    module_bad_field.add_schema(s);
    let err = verify_module(&module_bad_field).expect_err("Invalid field name must fail");
    assert!(err.iter().any(|d| d.error_kind == "InvalidFieldName"));

    // 3. FieldIndexOverflow in Schema
    let mut module_idx_overflow = Module::new();
    let mut s2 = Schema::new("HugeSchema");
    s2.add_field(70000, Some("x".to_string()), Type::I64);
    module_idx_overflow.add_schema(s2);
    let err = verify_module(&module_idx_overflow).expect_err("Field index overflow in schema must fail");
    assert!(err.iter().any(|d| d.error_kind == "FieldIndexOverflow"));

    // 4. FieldIndexOverflow in Instruction (struct_get and struct_set)
    let mut module_inst = Module::new();
    let s3 = Schema::new("Point");
    module_inst.add_schema(s3);
    let mut func = Function::new("test_inst", Type::Void);
    func.add_param(0, Type::Struct("Point".to_string()));
    let mut bb = BasicBlock::new(0, Terminator::Ret(None));
    bb.push(Instruction::new(Some(1), Type::I64, Op::StructGet(0, 70000), 1));
    func.add_block(bb);
    module_inst.add_function(func);
    let err = verify_module(&module_inst).expect_err("Field index overflow in instruction must fail");
    assert!(err.iter().any(|d| d.error_kind == "FieldIndexOverflow"));
}



