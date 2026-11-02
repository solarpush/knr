use knr_core::ast::*;
use knr_core::binary::header::{MAGIC, CURRENT_VERSION};
use knr_core::error::{BinaryError, ParseError};
use knr_core::{decode_knrbin, encode_knrbin, format_ssa, parse_ssa};

const CANONICAL_15_LINES: &str = "\
fn main() -> i64 {
bb0:
    %0 = const_i64 10
    %1 = const_i64 20
    %2 = add %0 %1
    %3 = const_i64 2
    %4 = mul %2 %3
    %5 = buf_alloc %4
    %6 = const_i64 0
    %7 = const_i64 255
    %8 = buf_write_u8 %5 %6 %7
    %9 = buf_read_u8 %8 %6
    free %8
    ret %9
}
";

const CANONICAL_28_OPCODES: &str = "\
fn test_all_opcodes(%0: i64) -> i64 effects(Net, Ui, Hw, Async) {
bb0:
    %1 = const_i64 100
    %2 = const_f64 3.14
    %3 = const_bool true
    %4 = const_str \"hello\\nworld\"
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
    free %25
    %28 = spawn \"worker\" %25
    %29 = syscall Net \"net.listen\" %1
    free %4
    br_if %9 bb1 bb2
bb1:
    br bb2
bb2:
    ret %15
}
";

#[test]
fn test_header_magic_and_version() {
    let module = parse_ssa(CANONICAL_15_LINES).expect("Failed to parse 15-line SSA");
    let bytes = encode_knrbin(&module).expect("Failed to encode knrbin");

    // Must be at least 6 bytes (4 magic + 2 version)
    assert!(bytes.len() >= 6);
    assert_eq!(&bytes[0..4], &MAGIC);
    assert_eq!(&bytes[0..4], b"KNIR");

    let version = u16::from_le_bytes([bytes[4], bytes[5]]);
    assert_eq!(version, CURRENT_VERSION);
    assert_eq!(version, 1);
}

#[test]
fn test_15_line_text_binary_bijection() {
    // 1. Verify exact 15 lines count
    assert_eq!(CANONICAL_15_LINES.lines().count(), 15);

    // 2. Parse text to SSA Module
    let module = parse_ssa(CANONICAL_15_LINES).expect("Failed to parse 15-line program");
    assert_eq!(module.functions.len(), 1);
    let func = &module.functions[0];
    assert_eq!(func.name, "main");
    assert_eq!(func.ret_ty, Type::I64);
    assert_eq!(func.blocks.len(), 1);
    let bb0 = &func.blocks[0];
    assert_eq!(bb0.id, 0);
    assert_eq!(bb0.instructions.len(), 11);
    assert_eq!(bb0.terminator, Terminator::Ret(Some(9)));

    // Verify 1-indexed source line numbers
    for (i, inst) in bb0.instructions.iter().enumerate() {
        assert_eq!(inst.line, i + 3, "Instruction {} has line {}, expected {}", i, inst.line, i + 3);
    }

    // 3. Encode to .knrbin binary
    let bytes = encode_knrbin(&module).expect("Failed to encode .knrbin");

    // 4. Decode from .knrbin binary
    let decoded = decode_knrbin(&bytes).expect("Failed to decode .knrbin");

    // 5. Format decoded Module back to canonical SSA text
    let formatted = format_ssa(&decoded);

    // 6. Assert EXACT byte-for-byte bijection identity
    assert_eq!(formatted, CANONICAL_15_LINES);
    assert_eq!(formatted.as_bytes(), CANONICAL_15_LINES.as_bytes());
}

#[test]
fn test_all_28_opcodes_roundtrip() {
    // 1. Parse program containing all 28 opcodes and 3 terminators
    let module = parse_ssa(CANONICAL_28_OPCODES).expect("Failed to parse all opcodes SSA");
    assert_eq!(module.functions.len(), 1);
    let func = &module.functions[0];
    assert_eq!(func.name, "test_all_opcodes");
    assert_eq!(func.ret_ty, Type::I64);
    assert_eq!(
        func.effects,
        vec![Effect::Net, Effect::Ui, Effect::Hw, Effect::Async]
    );
    assert_eq!(func.blocks.len(), 3);

    // 2. Encode to .knrbin
    let bytes = encode_knrbin(&module).expect("Failed to encode all opcodes to knrbin");

    // 3. Decode from .knrbin
    let decoded = decode_knrbin(&bytes).expect("Failed to decode all opcodes knrbin");

    // 4. Format to canonical SSA text
    let formatted = format_ssa(&decoded);

    // 5. Assert 1:1 byte identity
    assert_eq!(formatted, CANONICAL_28_OPCODES);
    assert_eq!(formatted.as_bytes(), CANONICAL_28_OPCODES.as_bytes());
}

#[test]
fn test_extra_args_pool_multi_operand() {
    let ssa = "\
fn multi_op(%0: buf, %1: map[str, i64]) -> buf {
bb0:
    %2 = const_i64 0
    %3 = const_i64 10
    %4 = const_i64 65
    %5 = buf_write_u8 %0 %2 %4
    %6 = buf_slice %5 %2 %3
    %7 = const_str \"key\"
    %8 = map_set %1 %7 %3
    free %8
    %9 = const_bool true
    br_if %9 bb1 bb2
bb1:
    ret %6
bb2:
    ret %6
}
";
    let module = parse_ssa(ssa).expect("Parse failed");
    let bytes = encode_knrbin(&module).expect("Encode failed");
    let decoded = decode_knrbin(&bytes).expect("Decode failed");
    let formatted = format_ssa(&decoded);
    assert_eq!(formatted, ssa);
}

#[test]
fn test_linear_types_predicate() {
    // Scalars are Copy (is_linear == false)
    assert!(!Type::I64.is_linear());
    assert!(!Type::F64.is_linear());
    assert!(!Type::Bool.is_linear());
    assert!(!Type::Void.is_linear());

    // Linear types are Move (is_linear == true)
    assert!(Type::Str.is_linear());
    assert!(Type::Buf.is_linear());
    assert!(Type::Vec(Box::new(Type::I64)).is_linear());
    assert!(Type::Map(Box::new(Type::Str), Box::new(Type::I64)).is_linear());
    assert!(Type::Chan(Box::new(Type::I64)).is_linear());
}

#[test]
fn test_negative_parser_errors() {
    // Unterminated string
    let bad_str = "fn f() -> void {\nbb0:\n    %0 = const_str \"unterminated\n    ret\n}";
    assert!(matches!(
        parse_ssa(bad_str),
        Err(ParseError::UnterminatedString { .. })
    ));

    // Invalid register
    let bad_reg = "fn f() -> void {\nbb0:\n    foo = const_i64 1\n    ret\n}";
    assert!(matches!(
        parse_ssa(bad_reg),
        Err(ParseError::InvalidRegister { .. })
    ));

    // Invalid opcode
    let bad_op = "fn f() -> void {\nbb0:\n    %0 = non_existent_opcode\n    ret\n}";
    assert!(matches!(
        parse_ssa(bad_op),
        Err(ParseError::UnknownOpcode { .. })
    ));

    // Missing closing brace
    let unclosed = "fn f() -> void {\nbb0:\n    ret\n";
    assert!(matches!(
        parse_ssa(unclosed),
        Err(ParseError::MissingClosingBrace { .. })
    ));
}

#[test]
fn test_negative_binary_errors() {
    // Bad magic
    let bad_magic = vec![0x00, 0x00, 0x00, 0x00, 0x01, 0x00];
    assert!(matches!(
        decode_knrbin(&bad_magic),
        Err(BinaryError::BadMagic([0, 0, 0, 0]))
    ));

    // Unsupported version
    let bad_version = vec![0x4B, 0x4E, 0x49, 0x52, 0x02, 0x00];
    assert!(matches!(
        decode_knrbin(&bad_version),
        Err(BinaryError::UnsupportedVersion(2))
    ));

    // Truncated header
    let truncated = vec![0x4B, 0x4E];
    assert!(matches!(
        decode_knrbin(&truncated),
        Err(BinaryError::UnexpectedEof { .. })
    ));
}
