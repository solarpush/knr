use knr_core::{
    decode_knrbin, encode_knrbin, format_ssa, parse_ssa, FieldDef, Function, Instruction, Module,
    Op, BasicBlock, Schema, Terminator, Type,
};

#[test]
fn test_schema_ast_and_parsing() {
    let ssa = r#"
schema Point {
  0: x i64,
  1: y i64,
}

fn make_point() -> struct[Point] {
bb0:
  %0 = struct_new Point
  %1 = const_i64 42
  %2 = struct_set %0 0 %1
  ret %2
}
"#;

    let module = parse_ssa(ssa).expect("Failed to parse SSA with schema");
    assert_eq!(module.schemas.len(), 1);
    let schema = &module.schemas[0];
    assert_eq!(schema.name, "Point");
    assert_eq!(schema.fields.len(), 2);
    assert_eq!(schema.fields[0].index, 0);
    assert_eq!(schema.fields[0].name.as_deref(), Some("x"));
    assert_eq!(schema.fields[0].ty, Type::I64);
    assert_eq!(schema.fields[1].index, 1);
    assert_eq!(schema.fields[1].name.as_deref(), Some("y"));
    assert_eq!(schema.fields[1].ty, Type::I64);

    let func = &module.functions[0];
    assert_eq!(func.name, "make_point");
    assert_eq!(func.ret_ty, Type::Struct("Point".to_string()));
    assert_eq!(func.blocks.len(), 1);

    let insts = &func.blocks[0].instructions;
    assert_eq!(insts[0].op, Op::StructNew("Point".to_string()));
    assert_eq!(insts[0].ty, Type::Struct("Point".to_string()));
    assert_eq!(insts[1].op, Op::ConstI64(42));
    assert_eq!(insts[2].op, Op::StructSet(0, 0, 1));
}

#[test]
fn test_textual_format_roundtrip() {
    let ssa = r#"schema User {
  0: id i64,
  1: name str,
  2: is_active bool,
}

fn update_user(%0: struct[User]) -> struct[User] {
bb0:
  %1 = const_bool true
  %2 = struct_set %0 2 %1
  ret %2
}
"#;

    let module = parse_ssa(ssa).expect("Failed to parse SSA");
    let formatted = format_ssa(&module);
    let reparsed = parse_ssa(&formatted).expect("Failed to re-parse formatted SSA");
    assert_eq!(module, reparsed);
}

#[test]
fn test_binary_codec_roundtrip() {
    let mut module = Module::new();
    let point_schema = Schema {
        name: "Point".to_string(),
        fields: vec![
            FieldDef {
                index: 0,
                name: Some("x".to_string()),
                ty: Type::I64,
            },
            FieldDef {
                index: 1,
                name: Some("y".to_string()),
                ty: Type::I64,
            },
        ],
    };
    module.schemas.push(point_schema);

    let mut func = Function::new("test_binary", Type::I64);
    let mut bb0 = BasicBlock::new(0, Terminator::Ret(Some(5)));
    bb0.push(Instruction::new(Some(0), Type::Struct("Point".to_string()), Op::StructNew("Point".to_string()), 3));
    bb0.push(Instruction::new(Some(1), Type::I64, Op::ConstI64(10), 4));
    bb0.push(Instruction::new(Some(2), Type::Struct("Point".to_string()), Op::StructSet(0, 0, 1), 5));
    bb0.push(Instruction::new(Some(3), Type::I64, Op::ConstI64(20), 6));
    bb0.push(Instruction::new(Some(4), Type::Struct("Point".to_string()), Op::StructSet(2, 1, 3), 7));
    bb0.push(Instruction::new(Some(5), Type::I64, Op::StructGet(4, 0), 8));
    func.add_block(bb0);
    module.functions.push(func);

    let bin = encode_knrbin(&module).expect("Encoding knrbin failed");
    let decoded = decode_knrbin(&bin).expect("Decoding knrbin failed");
    assert_eq!(module, decoded);
}

#[test]
fn test_multiple_schemas_and_struct_types() {
    let ssa = r#"
schema Color {
  0: r i64,
  1: g i64,
  2: b i64,
}

schema Particle {
  0: x f64,
  1: y f64,
  2: color struct[Color],
}

fn particle_demo() -> f64 {
bb0:
  %0 = struct_new Color
  %1 = struct_new Particle
  %2 = struct_set %1 2 %0
  %3 = const_f64 1.5
  %4 = struct_set %2 0 %3
  %5 = struct_get %4 0
  ret %5
}
"#;

    let module = parse_ssa(ssa).expect("Failed to parse multiple schemas");
    assert_eq!(module.schemas.len(), 2);
    assert_eq!(module.schemas[0].name, "Color");
    assert_eq!(module.schemas[1].name, "Particle");
    assert_eq!(
        module.schemas[1].fields[2].ty,
        Type::Struct("Color".to_string())
    );

    let bin = encode_knrbin(&module).expect("Binary encode failed");
    let decoded = decode_knrbin(&bin).expect("Binary decode failed");
    assert_eq!(module.schemas, decoded.schemas);
    assert_eq!(format_ssa(&module), format_ssa(&decoded));
}

#[test]
fn test_invalid_field_name_rejected_by_parser() {
    let ssa_get = r#"
schema Point {
  0: x i64,
  1: y i64,
}

fn bad_field_get() -> i64 {
bb0:
  %0 = struct_new Point
  %1 = struct_get %0 zzz_invalid
  free %0
  ret %1
}
"#;
    let err = parse_ssa(ssa_get).expect_err("Unknown field name must fail parsing");
    match err {
        knr_core::ParseError::InvalidSyntax { msg, .. } => {
            assert!(msg.contains("zzz_invalid"), "Error message should mention the invalid field: {msg}");
        }
        other => panic!("Expected ParseError::InvalidSyntax, got: {:?}", other),
    }

    let ssa_set = r#"
schema Point {
  0: x i64,
  1: y i64,
}

fn bad_field_set() -> struct[Point] {
bb0:
  %0 = struct_new Point
  %1 = const_i64 123
  %2 = struct_set %0 non_existent %1
  ret %2
}
"#;
    let err = parse_ssa(ssa_set).expect_err("Unknown field name in struct_set must fail parsing");
    match err {
        knr_core::ParseError::InvalidSyntax { msg, .. } => {
            assert!(msg.contains("non_existent"), "Error message should mention the invalid field: {msg}");
        }
        other => panic!("Expected ParseError::InvalidSyntax, got: {:?}", other),
    }
}

#[test]
fn test_corrupted_knrbin_fuzzing() {
    let mut module = Module::new();
    let point_schema = Schema {
        name: "Point".to_string(),
        fields: vec![
            FieldDef {
                index: 0,
                name: Some("x".to_string()),
                ty: Type::I64,
            },
        ],
    };
    module.schemas.push(point_schema);
    let mut func = Function::new("dummy", Type::Void);
    let bb0 = BasicBlock::new(0, Terminator::Ret(None));
    func.add_block(bb0);
    module.functions.push(func);

    let valid_bytes = encode_knrbin(&module).expect("encoding should succeed");
    assert!(valid_bytes.len() > 30);

    // 1. Truncate valid binary in the middle of schema definitions
    for cut in 1..valid_bytes.len() {
        let truncated = &valid_bytes[..cut];
        let res = decode_knrbin(truncated);
        assert!(res.is_err(), "Truncated binary at cut={cut} must fail gracefully");
    }

    // 2. Corrupt schema count to an absurd number (e.g. 0xFFFFFF00)
    let mut corrupted = valid_bytes.clone();
    // Locate schema section: after header (6 bytes) + const pool + type pool + extra pool
    // In our module, search for 4 bytes of schema count (1u32)
    let schema_count_pattern = 1u32.to_le_bytes();
    if let Some(pos) = corrupted.windows(4).position(|w| w == schema_count_pattern) {
        corrupted[pos] = 0xFE;
        corrupted[pos + 1] = 0xFF;
        corrupted[pos + 2] = 0xFF;
        corrupted[pos + 3] = 0x7F;
        let res = decode_knrbin(&corrupted);
        assert!(res.is_err(), "Absurd schema count must be rejected");
    }
}

#[test]
fn test_struct_new_syntax_variants_and_trailing_rejection() {
    // 1. Bracketed unquoted syntax: struct_new struct[Point]
    let ssa_brackets = r#"
schema Point {
  0: x i64,
}

fn test_brackets() -> struct[Point] {
bb0:
  %0 = struct_new struct[Point]
  ret %0
}
"#;
    let m = parse_ssa(ssa_brackets).expect("Parsing unquoted struct[Point] must succeed");
    assert_eq!(m.functions[0].blocks[0].instructions[0].op, Op::StructNew("Point".to_string()));

    // 2. Angle brackets syntax: struct_new struct<Point>
    let ssa_angles = r#"
schema Point {
  0: x i64,
}

fn test_angles() -> struct[Point] {
bb0:
  %0 = struct_new struct<Point>
  ret %0
}
"#;
    let m2 = parse_ssa(ssa_angles).expect("Parsing unquoted struct<Point> must succeed");
    assert_eq!(m2.functions[0].blocks[0].instructions[0].op, Op::StructNew("Point".to_string()));

    // 3. Trailing arguments must be rejected
    let ssa_trailing = r#"
schema Point {
  0: x i64,
}

fn bad_trailing() -> struct[Point] {
bb0:
  %0 = struct_new Point 123 unexpected
  ret %0
}
"#;
    let err = parse_ssa(ssa_trailing).expect_err("Trailing arguments in struct_new must be rejected");
    match err {
        knr_core::ParseError::InvalidSyntax { msg, .. } => {
            assert!(msg.contains("trailing"), "Error message should mention trailing arguments: {msg}");
        }
        other => panic!("Expected ParseError::InvalidSyntax, got: {:?}", other),
    }
}

#[test]
fn test_invalid_schema_and_field_identifiers_rejected() {
    // 1. Schema name starting with digit
    let bad_schema_name = r#"
schema 123Point {
  0: x i64,
}
fn dummy() -> void {
bb0:
  ret
}
"#;
    assert!(parse_ssa(bad_schema_name).is_err(), "Schema name starting with digit must fail");

    // 2. Field with empty identifier
    let empty_field = r#"
schema BadField {
  : i64,
}
fn dummy() -> void {
bb0:
  ret
}
"#;
    assert!(parse_ssa(empty_field).is_err(), "Empty field name must fail");

    // 3. Field with negative index
    let negative_idx = r#"
schema BadIdx {
  -1: x i64,
}
fn dummy() -> void {
bb0:
  ret
}
"#;
    assert!(parse_ssa(negative_idx).is_err(), "Negative field index must fail");

    // 4. Invalid schema identifier in struct_new
    let bad_struct_new = r#"
fn bad() -> void {
bb0:
  %0 = struct_new 123Bad
  ret
}
"#;
    assert!(parse_ssa(bad_struct_new).is_err(), "struct_new with digit-leading ident must fail");
}

#[test]
fn test_field_index_boundary_rejection() {
    // Field index > 65535 in schema
    let overflow_schema = r#"
schema HugeIdx {
  70000: x i64,
}
fn dummy() -> void {
bb0:
  ret
}
"#;
    assert!(parse_ssa(overflow_schema).is_err(), "Schema field index > 65535 must fail parsing");

    // Field index > 65535 in struct_get
    let overflow_get = r#"
schema Valid {
  0: x i64,
}
fn dummy(%0: struct[Valid]) -> i64 {
bb0:
  %1 = struct_get %0 70000
  ret %1
}
"#;
    assert!(parse_ssa(overflow_get).is_err(), "struct_get index > 65535 must fail parsing");

    // Field index > 65535 in struct_set
    let overflow_set = r#"
schema Valid {
  0: x i64,
}
fn dummy(%0: struct[Valid], %1: i64) -> struct[Valid] {
bb0:
  %2 = struct_set %0 70000 %1
  ret %2
}
"#;
    assert!(parse_ssa(overflow_set).is_err(), "struct_set index > 65535 must fail parsing");
}

#[test]
fn test_corrupted_empty_struct_type_name_decode() {
    use knr_core::binary::const_pool::TypePool;
    // Encoding tag 0x13 (Struct) with length 0
    let corrupted_bytes = vec![0x13, 0x00];
    let mut cursor = corrupted_bytes.as_slice();
    let res = TypePool::decode(&mut cursor);
    assert!(res.is_err(), "Decoding Type::Struct with empty name must fail");
}



