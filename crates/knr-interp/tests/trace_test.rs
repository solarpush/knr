use knr_core::{BasicBlock, Function, Instruction, Op, Terminator, Type};
use knr_interp::{execute, TraceRecord, Value};
use knr_runtime::MockPlatform;

#[test]
fn test_trace_disabled_returns_empty_trace() {
    let mut platform = MockPlatform::new();

    let mut func = Function::new("no_trace", Type::I64);
    let mut bb0 = BasicBlock::new(0, Terminator::Ret(Some(0)));
    bb0.push(Instruction::new(Some(0), Type::I64, Op::ConstI64(42), 1));
    func.add_block(bb0);

    let (_, traces) = execute(&func, vec![], &mut platform, 100, false).expect("exec ok");
    assert!(traces.is_empty());
}

#[test]
fn test_trace_records_and_json_serialization() {
    let mut platform = MockPlatform::new();

    let mut func = Function::new("traced", Type::I64);
    let mut bb0 = BasicBlock::new(0, Terminator::Ret(Some(1)));
    bb0.push(Instruction::new(Some(0), Type::I64, Op::ConstI64(10), 5));
    bb0.push(Instruction::new(Some(1), Type::I64, Op::Add(0, 0), 6));
    func.add_block(bb0);

    let (val, traces) = execute(&func, vec![], &mut platform, 100, true).expect("exec ok");
    assert_eq!(val, Value::I64(20));
    assert_eq!(traces.len(), 3);

    // Verify step 1
    assert_eq!(traces[0].step, 1);
    assert_eq!(traces[0].line, 5);
    assert_eq!(traces[0].block, 0);
    assert_eq!(traces[0].opcode, "const_i64");
    assert_eq!(traces[0].dst_reg, Some("%0".to_string()));
    assert_eq!(traces[0].new_value, Some(serde_json::json!(10)));

    // Verify step 2
    assert_eq!(traces[1].step, 2);
    assert_eq!(traces[1].line, 6);
    assert_eq!(traces[1].block, 0);
    assert_eq!(traces[1].opcode, "add");
    assert_eq!(traces[1].dst_reg, Some("%1".to_string()));
    assert_eq!(traces[1].new_value, Some(serde_json::json!(20)));

    // Verify step 3 (ret terminator)
    assert_eq!(traces[2].step, 3);
    assert_eq!(traces[2].block, 0);
    assert_eq!(traces[2].opcode, "ret");
    assert_eq!(traces[2].dst_reg, None);
    assert_eq!(traces[2].new_value, Some(serde_json::json!(20)));

    // Test JSON serialization & deserialization roundtrip
    let json_str = serde_json::to_string(&traces).expect("Serialization to JSON should succeed");
    let deserialized: Vec<TraceRecord> =
        serde_json::from_str(&json_str).expect("Deserialization from JSON should succeed");
    assert_eq!(traces, deserialized);

    // Verify fields in serialized JSON
    let parsed_json: serde_json::Value = serde_json::from_str(&json_str).unwrap();
    assert!(parsed_json.is_array());
    let arr = parsed_json.as_array().unwrap();
    assert_eq!(arr.len(), 3);
    assert_eq!(arr[0]["opcode"], "const_i64");
    assert_eq!(arr[0]["dst_reg"], "%0");
    assert_eq!(arr[0]["new_value"], 10);
}
