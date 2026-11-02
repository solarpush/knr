use knr_core::ast::*;
use knr_core::error::{BinaryError, ParseError};
use knr_core::{decode_knrbin, encode_knrbin, format_ssa, parse_function, parse_ssa};

// ============================================================================
// 1. COMPLEX MULTI-FUNCTION, MULTI-BLOCK PROGRAMS WITH ALL 28 OPCODES & 3 TERMINATORS
// ============================================================================

const COMPLEX_MULTI_BLOCK_MODULE: &str = "\
fn diamond_cfg(%0: i64, %1: i64) -> i64 {
bb0:
    %2 = cmp_lt %0 %1
    br_if %2 bb1 bb2
bb1:
    %3 = add %0 %1
    br bb3
bb2:
    %4 = sub %0 %1
    br bb3
bb3:
    ret %0
}

fn loop_cfg(%0: i64) -> i64 {
bb0:
    %1 = const_i64 0
    %2 = const_i64 1
    %3 = const_i64 10
    br bb1
bb1:
    %4 = cmp_lt %1 %3
    br_if %4 bb2 bb3
bb2:
    %5 = add %1 %2
    br bb1
bb3:
    ret %1
}

fn all_28_opcodes_showcase(%0: i64) -> i64 effects(Net, Ui, Hw, Async) {
bb0:
    %1 = const_i64 42
    %2 = const_f64 3.14159
    %3 = const_bool true
    %4 = const_str \"knr_systems_language\"
    %5 = add %0 %1
    %6 = sub %5 %1
    %7 = mul %6 %1
    %8 = div %7 %1
    %9 = cmp_eq %8 %1
    %10 = cmp_lt %8 %1
    %11 = buf_alloc %1
    %12 = const_i64 0
    %13 = const_i64 255
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
    %28 = spawn \"background_worker\" %25
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
fn test_complex_multi_block_and_all_opcodes_bijection() {
    let parsed = parse_ssa(COMPLEX_MULTI_BLOCK_MODULE)
        .expect("Failed to parse complex multi-block module");
    assert_eq!(parsed.functions.len(), 3);

    // Verify Function 0 (diamond CFG)
    assert_eq!(parsed.functions[0].name, "diamond_cfg");
    assert_eq!(parsed.functions[0].blocks.len(), 4);
    assert_eq!(
        parsed.functions[0].blocks[0].terminator,
        Terminator::BrIf {
            cond: 2,
            then_blk: 1,
            else_blk: 2
        }
    );
    assert_eq!(
        parsed.functions[0].blocks[1].terminator,
        Terminator::Br(3)
    );
    assert_eq!(
        parsed.functions[0].blocks[2].terminator,
        Terminator::Br(3)
    );
    assert_eq!(
        parsed.functions[0].blocks[3].terminator,
        Terminator::Ret(Some(0))
    );

    // Verify Function 1 (loop CFG)
    assert_eq!(parsed.functions[1].name, "loop_cfg");
    assert_eq!(parsed.functions[1].blocks.len(), 4);

    // Verify Function 2 (all 28 opcodes)
    assert_eq!(parsed.functions[2].name, "all_28_opcodes_showcase");
    assert_eq!(
        parsed.functions[2].effects,
        vec![Effect::Net, Effect::Ui, Effect::Hw, Effect::Async]
    );

    // Encode to binary knrbin
    let encoded = encode_knrbin(&parsed).expect("Failed to encode to knrbin");

    // Decode back to Module
    let decoded = decode_knrbin(&encoded).expect("Failed to decode from knrbin");

    // Format back to SSA
    let formatted = format_ssa(&decoded);

    // Assert exact 1:1 byte identity
    assert_eq!(formatted, COMPLEX_MULTI_BLOCK_MODULE);
}

// ============================================================================
// 2. DEEPLY NESTED PARAMETRIC TYPES EMPIRICAL VERIFICATION
// ============================================================================

#[test]
fn test_deeply_nested_parametric_types_bijection() {
    let ssa = "\
fn deep_types(\
%0: vec[map[str, chan[buf]]], \
%1: map[vec[str], chan[map[str, buf]]], \
%2: chan[vec[map[str, vec[chan[buf]]]]], \
%3: vec[vec[vec[vec[i64]]]]\
) -> vec[map[str, chan[buf]]] {
bb0:
    ret %0
}
";
    let parsed = parse_ssa(ssa).expect("Failed to parse deeply nested parametric types");
    assert_eq!(parsed.functions.len(), 1);
    let func = &parsed.functions[0];
    assert_eq!(func.params.len(), 4);

    // Check parsed param types
    assert_eq!(
        func.params[0].1,
        Type::Vec(Box::new(Type::Map(
            Box::new(Type::Str),
            Box::new(Type::Chan(Box::new(Type::Buf)))
        )))
    );

    assert_eq!(
        func.params[3].1,
        Type::Vec(Box::new(Type::Vec(Box::new(Type::Vec(Box::new(Type::Vec(
            Box::new(Type::I64),
        )))))))
    );

    let encoded = encode_knrbin(&parsed).expect("Failed to encode deeply nested types");
    let decoded = decode_knrbin(&encoded).expect("Failed to decode deeply nested types");
    let formatted = format_ssa(&decoded);

    assert_eq!(formatted, ssa, "Deeply nested types roundtrip mismatch");
}

// ============================================================================
// 3. ALL EFFECT CONFIGURATIONS (0, 1, 2, 3, 4 EFFECTS & CANONICAL SORTING)
// ============================================================================

#[test]
fn test_all_effect_configurations_and_sorting() {
    let ssa = "\
fn fn_pure() -> void {
bb0:
    ret
}

fn fn_net() -> void effects(Net) {
bb0:
    ret
}

fn fn_ui() -> void effects(Ui) {
bb0:
    ret
}

fn fn_hw() -> void effects(Hw) {
bb0:
    ret
}

fn fn_async() -> void effects(Async) {
bb0:
    ret
}

fn fn_pair_net_hw() -> void effects(Net, Hw) {
bb0:
    ret
}

fn fn_triplet() -> void effects(Net, Ui, Async) {
bb0:
    ret
}

fn fn_quadruplet() -> void effects(Net, Ui, Hw, Async) {
bb0:
    ret
}
";
    let parsed = parse_ssa(ssa).expect("Failed to parse effects test");
    assert_eq!(parsed.functions.len(), 8);

    let encoded = encode_knrbin(&parsed).expect("Failed to encode effects test");
    let decoded = decode_knrbin(&encoded).expect("Failed to decode effects test");
    let formatted = format_ssa(&decoded);

    assert_eq!(formatted, ssa, "Effects configurations roundtrip mismatch");
}

#[test]
fn test_effects_canonical_sorting_normalization() {
    // Input has unordered effects: effects(Async, Net, Hw, Ui)
    let uncanonical_ssa = "\
fn unordered() -> void effects(Async, Net, Hw, Ui) {
bb0:
    ret
}
";
    let expected_canonical_ssa = "\
fn unordered() -> void effects(Net, Ui, Hw, Async) {
bb0:
    ret
}
";
    let parsed = parse_ssa(uncanonical_ssa).expect("Failed to parse uncanonical effects");
    let encoded = encode_knrbin(&parsed).expect("Failed to encode");
    let decoded = decode_knrbin(&encoded).expect("Failed to decode");
    let formatted = format_ssa(&decoded);

    assert_eq!(formatted, expected_canonical_ssa);
}

// ============================================================================
// 4. MULTI-OPERAND INSTRUCTIONS & EXTRA ARGS POOL STRESS (1,000 INSTRUCTIONS)
// ============================================================================

#[test]
fn test_extra_args_pool_stress_1000_instructions() {
    // Generate a function with 1,000 multi-operand instructions:
    // buf_write_u8, buf_slice, map_set, syscall, br_if
    let count = 1000;
    let mut ssa = String::new();
    ssa.push_str("fn stress_multi_op(%0: buf, %1: map[str, i64]) -> buf effects(Net) {\nbb0:\n");
    ssa.push_str("    %2 = const_i64 0\n");
    ssa.push_str("    %3 = const_i64 1\n");
    ssa.push_str("    %4 = const_str \"key\"\n");

    for i in 0..count {
        let r = 5 + i;
        if i % 3 == 0 {
            ssa.push_str(&format!("    %{r} = buf_write_u8 %0 %2 %3\n"));
        } else if i % 3 == 1 {
            ssa.push_str(&format!("    %{r} = buf_slice %0 %2 %3\n"));
        } else {
            ssa.push_str(&format!("    %{r} = map_set %1 %4 %3\n"));
        }
    }

    let final_reg = 5 + count - 1;
    ssa.push_str(&format!("    ret %{final_reg}\n}}\n"));

    let parsed = parse_ssa(&ssa).expect("Failed to parse 1000 multi-op SSA");
    let encoded = encode_knrbin(&parsed).expect("Failed to encode 1000 multi-op knrbin");
    let decoded = decode_knrbin(&encoded).expect("Failed to decode 1000 multi-op knrbin");
    let formatted = format_ssa(&decoded);

    assert_eq!(formatted, ssa, "1000 multi-op stress roundtrip mismatch");
}

// ============================================================================
// 5. STRING LITERALS: ASCII VS NON-ASCII EMPIRICAL TEST (CHALLENGE ORACLE)
// ============================================================================

#[test]
fn test_ascii_string_literals_bijection() {
    let ssa = "\
fn ascii_strings() -> void {
bb0:
    %0 = const_str \"\"
    free %0
    %1 = const_str \"The quick brown fox jumps over the lazy dog 0123456789!@#$%^&*()_+\"
    free %1
    %2 = const_str \"hello\\nworld\\ttab\\rreturn\\0null\\\\backslash\\\"quote\"
    free %2
    ret
}
";
    let parsed = parse_ssa(ssa).expect("Failed to parse ascii strings");
    let encoded = encode_knrbin(&parsed).expect("Failed to encode ascii strings");
    let decoded = decode_knrbin(&encoded).expect("Failed to decode ascii strings");
    let formatted = format_ssa(&decoded);

    assert_eq!(formatted, ssa, "ASCII strings bijection failed");
}

#[test]
fn test_non_ascii_unicode_string_bug_empirical_demonstration() {
    // CHALLENGE: This test empirically demonstrates the UTF-8 corruption bug in parse_quoted_string.
    let ssa = "\
fn unicode_strings() -> void {
bb0:
    %0 = const_str \"café\"
    free %0
    ret
}
";
    let parsed = parse_ssa(ssa).expect("Parser accepted unicode");
    let inst = &parsed.functions[0].blocks[0].instructions[0];
    if let Op::ConstStr(parsed_str) = &inst.op {
        assert_eq!(
            parsed_str, "café",
            "UTF-8 multi-byte sequence must roundtrip without Latin-1 byte corruption"
        );
    } else {
        panic!("Expected ConstStr");
    }
}

// ============================================================================
// 6. BOUNDARY VALUES & LIMITS
// ============================================================================

#[test]
fn test_i64_extremes_bijection() {
    let ssa = format!(
        "\
fn test_i64_extremes() -> void {{
bb0:
    %0 = const_i64 {}
    %1 = const_i64 {}
    %2 = const_i64 0
    %3 = const_i64 -1
    %4 = const_i64 1
    ret
}}
",
        i64::MIN,
        i64::MAX
    );
    let parsed = parse_ssa(&ssa).expect("Failed to parse i64 extremes");
    let encoded = encode_knrbin(&parsed).expect("Failed to encode i64 extremes");
    let decoded = decode_knrbin(&encoded).expect("Failed to decode i64 extremes");
    let formatted = format_ssa(&decoded);

    assert_eq!(formatted, ssa);
}

#[test]
fn test_register_id_max_65534_and_overflow_65535() {
    // Max allowed register is 65534 (0xFFFE)
    let valid_max = "\
fn max_reg() -> void {
bb0:
    %65534 = const_i64 100
    ret
}
";
    let parsed = parse_ssa(valid_max).expect("Parse max register failed");
    let encoded = encode_knrbin(&parsed).expect("Encode max register failed");
    let decoded = decode_knrbin(&encoded).expect("Decode max register failed");
    assert_eq!(format_ssa(&decoded), valid_max);

    // Register 65535 (0xFFFF is NO_REG) must produce RegisterOverflow during encode
    let overflow = "\
fn overflow_reg() -> void {
bb0:
    %65535 = const_i64 100
    ret
}
";
    let parsed_overflow = parse_ssa(overflow).expect("Parse overflow register failed");
    let encode_err = encode_knrbin(&parsed_overflow);
    assert!(
        matches!(encode_err, Err(BinaryError::RegisterOverflow(65535))),
        "Expected RegisterOverflow(65535), got {:?}",
        encode_err
    );
}

// ============================================================================
// 7. NEGATIVE INPUT HANDLING & ZERO-PANIC STRESS
// ============================================================================

#[test]
fn test_negative_binary_corruptions_no_panics() {
    // Test that various corrupt byte sequences return clean errors and never panic
    let corrupt_payloads: Vec<Vec<u8>> = vec![
        vec![],
        vec![0x00],
        vec![0x4B, 0x4E, 0x49],
        vec![0x4B, 0x4E, 0x49, 0x52], // magic only, missing version
        vec![0x4B, 0x4E, 0x49, 0x52, 0x99, 0x00], // unsupported version 0x99
        vec![0x00, 0x00, 0x00, 0x00, 0x01, 0x00], // invalid magic
    ];

    for (i, payload) in corrupt_payloads.iter().enumerate() {
        let res = decode_knrbin(payload);
        assert!(res.is_err(), "Payload {} unexpectedly succeeded", i);
    }
}

#[test]
fn test_fuzz_binary_corruptions_5000_mutations_no_panics() {
    // Generate valid binary payload, then mutate it 5,000 times
    let valid_ssa = "\
fn simple(%0: i64) -> i64 {
bb0:
    %1 = const_i64 42
    %2 = add %0 %1
    ret %2
}
";
    let module = parse_ssa(valid_ssa).unwrap();
    let valid_bytes = encode_knrbin(&module).unwrap();

    // 1. Truncate at every single byte offset
    for len in 0..valid_bytes.len() {
        let truncated = &valid_bytes[..len];
        let _ = decode_knrbin(truncated);
    }

    // 2. Fuzz with arbitrary pseudo-random byte mutations (restricted to safe counts or isolated)
    let mut rng_state: u64 = 0xDEADBEEFCAFE;
    let mut next_u32 = || {
        rng_state = rng_state.wrapping_mul(6364136223846793005).wrapping_add(1);
        (rng_state >> 32) as u32
    };

    for _ in 0..1000 {
        let mut corrupted = valid_bytes.clone();
        // Mutate opcodes, types, and register IDs in instruction and terminator words
        let offset = corrupted.len() - 16 + ((next_u32() as usize) % 16);
        corrupted[offset] = (next_u32() & 0xFF) as u8;

        // Must never panic on corrupted opcodes or registers!
        let _ = decode_knrbin(&corrupted);
    }
}

#[test]
fn test_cwe789_dos_unbounded_allocation_prevented() {
    let valid_ssa = "fn f() -> void {\nbb0:\n    ret\n}\n";
    let module = parse_ssa(valid_ssa).unwrap();
    let mut bytes = encode_knrbin(&module).unwrap();

    let len = bytes.len();
    assert!(len >= 12);
    let inst_count_offset = len - 12;
    bytes[inst_count_offset..inst_count_offset + 4].copy_from_slice(&0x7FFFFFFFu32.to_le_bytes());
    let res = decode_knrbin(&bytes);
    assert!(
        res.is_err(),
        "Expected clean BinaryError on corrupted inst_count without OOM abort, got: {:?}",
        res
    );
}

#[test]
fn test_negative_parser_syntax_errors_no_panics() {
    // Empty strings parse as empty Module in parse_ssa, but error in parse_function
    assert_eq!(parse_ssa("").unwrap().functions.len(), 0);
    assert_eq!(parse_ssa("   \n\n\t").unwrap().functions.len(), 0);
    assert_eq!(parse_ssa("# only comments\n// another comment\n").unwrap().functions.len(), 0);

    assert!(matches!(parse_function(""), Err(ParseError::UnexpectedEof { .. })));

    let malformed_inputs = [
        "fn",
        "fn (",
        "fn test(",
        "fn test()",
        "fn test() ->",
        "fn test() -> void",
        "fn test() -> void {",
        "fn test() -> void {\n    ret\n}",
        "fn test() -> void {\nbb0:\n    %0 = unknown_op\n    ret\n}",
        "fn test() -> void {\nbb0:\n    %0 = const_i64 not_a_number\n    ret\n}",
        "fn test() -> void {\nbb0:\n    %0 = const_f64 not_a_float\n    ret\n}",
        "fn test() -> void {\nbb0:\n    %0 = const_bool not_a_bool\n    ret\n}",
        "fn test() -> void {\nbb0:\n    %0 = const_str \"unclosed string\n    ret\n}",
        "fn test() -> void {\nbb0:\n    %not_a_reg = const_i64 1\n    ret\n}",
        "fn test() -> void {\nbb0:\n    %0 = add %1\n    ret\n}",
        "fn test() -> void {\nbb0:\n    %0 = add %1 %2 %3\n    ret\n}",
        "fn test() -> void {\nbb0:\n    br_if %1 bb1\n}",
        "fn test() -> void effects(UnknownEffect) {\nbb0:\n    ret\n}",
        "fn test(%0: invalid_type) -> void {\nbb0:\n    ret\n}",
        "fn test(%0: vec[) -> void {\nbb0:\n    ret\n}",
        "fn test(%0: map[str) -> void {\nbb0:\n    ret\n}",
    ];

    for (i, malformed) in malformed_inputs.iter().enumerate() {
        let res = parse_ssa(malformed);
        assert!(res.is_err(), "Malformed input {} unexpectedly parsed: {:?}", i, malformed);
    }
}

#[test]
fn test_missing_terminator_rejected() {
    let ssa_missing_term = "\
fn test() -> void {
bb0:
}
";
    let res = parse_ssa(ssa_missing_term);
    assert!(
        matches!(res, Err(ParseError::MissingTerminator { block_id: 0, .. })),
        "Expected MissingTerminator for block bb0, got: {:?}",
        res
    );
}

#[test]
fn test_unquoted_dotted_syscall_identifiers() {
    // In SPEC.md §5.1, syscall names are specified as e.g. net.listen
    let unquoted = "\
fn test() -> void effects(Net) {
bb0:
    %0 = const_i64 8080
    %1 = syscall Net net.listen %0
    ret
}
";
    let res = parse_ssa(unquoted);
    assert!(
        res.is_ok(),
        "Expected unquoted dotted syscall identifiers to parse successfully, got: {:?}",
        res
    );
    let module = res.unwrap();
    let inst = &module.functions[0].blocks[0].instructions[1];
    assert_eq!(
        inst.op,
        Op::Syscall(Effect::Net, "net.listen".to_string(), 0)
    );
}

// ============================================================================
// 8. EXHAUSTIVE MALICIOUS BINARY DECODER FUZZING & OOM PREVENTION
// ============================================================================

#[test]
fn test_adversarial_giant_lengths_no_oom() {
    let base_ssa = "fn target() -> void {\nbb0:\n    ret\n}\n";
    let valid_module = parse_ssa(base_ssa).expect("Base SSA valid");
    let valid_bytes = encode_knrbin(&valid_module).expect("Encoding succeeds");

    // Helper to test decoding a corrupt buffer without panic/OOM
    let assert_clean_err = |buf: &[u8], desc: &str| {
        let res = decode_knrbin(buf);
        assert!(
            res.is_err(),
            "Expected Err on {} but got: {:?}",
            desc,
            res
        );
    };

    // 1. Giant string count in ConstantPool (offset 6 is start of const pool)
    let mut giant_str_count = valid_bytes.clone();
    giant_str_count[6..10].copy_from_slice(&0xFFFFFFFFu32.to_le_bytes());
    assert_clean_err(&giant_str_count, "giant str_count (0xFFFFFFFF)");

    // 2. Giant string length in ConstantPool:
    // Construct a binary with a string length set to 0xFFFFFFFF
    let mut giant_str_len = Vec::new();
    giant_str_len.extend_from_slice(&[0x4B, 0x4E, 0x49, 0x52]); // MAGIC
    giant_str_len.extend_from_slice(&1u16.to_le_bytes()); // Version 1
    giant_str_len.extend_from_slice(&1u32.to_le_bytes()); // str_count = 1
    giant_str_len.extend_from_slice(&0xFFFFFFFFu32.to_le_bytes()); // giant length
    giant_str_len.extend_from_slice(b"fake data that is too short");
    assert_clean_err(&giant_str_len, "giant str_len (0xFFFFFFFF)");

    // 3. Giant float count in ConstantPool
    let mut giant_float_count = Vec::new();
    giant_float_count.extend_from_slice(&[0x4B, 0x4E, 0x49, 0x52]);
    giant_float_count.extend_from_slice(&1u16.to_le_bytes());
    giant_float_count.extend_from_slice(&0u32.to_le_bytes()); // str_count = 0
    giant_float_count.extend_from_slice(&0xFFFFFFFFu32.to_le_bytes()); // float_count
    assert_clean_err(&giant_float_count, "giant float_count (0xFFFFFFFF)");

    // 4. Giant int count in ConstantPool
    let mut giant_int_count = Vec::new();
    giant_int_count.extend_from_slice(&[0x4B, 0x4E, 0x49, 0x52]);
    giant_int_count.extend_from_slice(&1u16.to_le_bytes());
    giant_int_count.extend_from_slice(&0u32.to_le_bytes()); // str_count = 0
    giant_int_count.extend_from_slice(&0u32.to_le_bytes()); // float_count = 0
    giant_int_count.extend_from_slice(&0xFFFFFFFFu32.to_le_bytes()); // int_count
    assert_clean_err(&giant_int_count, "giant int_count (0xFFFFFFFF)");

    // 5. Giant TypePool count
    let mut giant_type_count = Vec::new();
    giant_type_count.extend_from_slice(&[0x4B, 0x4E, 0x49, 0x52]);
    giant_type_count.extend_from_slice(&1u16.to_le_bytes());
    giant_type_count.extend_from_slice(&0u32.to_le_bytes()); // str_count = 0
    giant_type_count.extend_from_slice(&0u32.to_le_bytes()); // float_count = 0
    giant_type_count.extend_from_slice(&0u32.to_le_bytes()); // int_count = 0
    giant_type_count.extend_from_slice(&0xFFFFu16.to_le_bytes()); // type_count = 65535
    assert_clean_err(&giant_type_count, "giant type_count (0xFFFF)");

    // 6. Giant ExtraArgsPool count
    let mut giant_extra_count = Vec::new();
    giant_extra_count.extend_from_slice(&[0x4B, 0x4E, 0x49, 0x52]);
    giant_extra_count.extend_from_slice(&1u16.to_le_bytes());
    giant_extra_count.extend_from_slice(&0u32.to_le_bytes()); // str_count = 0
    giant_extra_count.extend_from_slice(&0u32.to_le_bytes()); // float_count = 0
    giant_extra_count.extend_from_slice(&0u32.to_le_bytes()); // int_count = 0
    giant_extra_count.extend_from_slice(&0u16.to_le_bytes()); // type_count = 0
    giant_extra_count.extend_from_slice(&0xFFFFFFFFu32.to_le_bytes()); // extra_count
    assert_clean_err(&giant_extra_count, "giant extra_args_count (0xFFFFFFFF)");

    // 7. Giant function count
    let mut giant_fn_count = Vec::new();
    giant_fn_count.extend_from_slice(&[0x4B, 0x4E, 0x49, 0x52]);
    giant_fn_count.extend_from_slice(&1u16.to_le_bytes());
    giant_fn_count.extend_from_slice(&0u32.to_le_bytes()); // str_count = 0
    giant_fn_count.extend_from_slice(&0u32.to_le_bytes()); // float_count = 0
    giant_fn_count.extend_from_slice(&0u32.to_le_bytes()); // int_count = 0
    giant_fn_count.extend_from_slice(&0u16.to_le_bytes()); // type_count = 0
    giant_fn_count.extend_from_slice(&0u32.to_le_bytes()); // extra_count = 0
    giant_fn_count.extend_from_slice(&0xFFFFFFFFu32.to_le_bytes()); // fn_count
    assert_clean_err(&giant_fn_count, "giant fn_count (0xFFFFFFFF)");

    // 8. Giant param count in function
    let ssa_fn = "fn f(%0: i64) -> void {\nbb0:\n    ret\n}\n";
    let fn_bytes = encode_knrbin(&parse_ssa(ssa_fn).unwrap()).unwrap();
    // Offset for param_count in the encoded binary:
    // Find param count offset (it follows fn name idx 4 bytes + ret_ty 1 byte + effects 1 byte = 6 bytes after fn_count)
    let mut giant_param = fn_bytes.clone();
    // Search for the param_count 0x0001
    for i in 0..giant_param.len() - 2 {
        if giant_param[i..i + 2] == [0x01, 0x00] && i > 20 {
            giant_param[i..i + 2].copy_from_slice(&0xFFFFu16.to_le_bytes());
            break;
        }
    }
    assert_clean_err(&giant_param, "giant param_count");

    // 9. Giant block count
    let mut giant_block = valid_bytes.clone();
    let blk_offset = valid_bytes.len() - 20; // 4 block_id + 4 inst_count + 8 term = 16 bytes per block, block_count before it
    giant_block[blk_offset..blk_offset + 4].copy_from_slice(&0xFFFFFFFFu32.to_le_bytes());
    assert_clean_err(&giant_block, "giant block_count (0xFFFFFFFF)");

    // 10. Giant instruction count
    let mut giant_inst = valid_bytes.clone();
    let inst_offset = valid_bytes.len() - 12; // 4 inst_count + 8 term
    giant_inst[inst_offset..inst_offset + 4].copy_from_slice(&0xFFFFFFFFu32.to_le_bytes());
    assert_clean_err(&giant_inst, "giant inst_count (0xFFFFFFFF)");
}

#[test]
fn test_adversarial_corrupted_opcodes_and_tags() {
    let ssa = "fn f() -> void {\nbb0:\n    %0 = const_i64 10\n    ret\n}\n";
    let bytes = encode_knrbin(&parse_ssa(ssa).unwrap()).unwrap();

    // 1. Invalid instruction opcode (e.g. 0x00, 0x20, 0xFE)
    for bad_opcode in [0x00u8, 0x20u8, 0xFEu8] {
        let mut corrupted = bytes.clone();
        let inst_offset = bytes.len() - 16; // instruction is 8 bytes, terminator is 8 bytes
        corrupted[inst_offset] = bad_opcode;
        let res = decode_knrbin(&corrupted);
        assert!(
            matches!(res, Err(BinaryError::InvalidOpcode(op)) if op == bad_opcode),
            "Expected InvalidOpcode({:#04x}), got: {:?}",
            bad_opcode,
            res
        );
    }

    // 2. Invalid terminator opcode (e.g. 0x00, 0x1C, 0x20)
    for bad_term in [0x00u8, 0x1Cu8, 0x20u8] {
        let mut corrupted = bytes.clone();
        let term_offset = bytes.len() - 8;
        corrupted[term_offset] = bad_term;
        let res = decode_knrbin(&corrupted);
        assert!(
            matches!(res, Err(BinaryError::InvalidOpcode(op)) if op == bad_term),
            "Expected InvalidOpcode({:#04x}) for terminator, got: {:?}",
            bad_term,
            res
        );
    }

    // 3. Invalid Syscall effect tag (> 3 in ExtraArgsPool)
    let ssa_sys = "fn f() -> void effects(Net) {\nbb0:\n    %0 = const_i64 0\n    %1 = syscall Net net.listen %0\n    ret\n}\n";
    let sys_bytes = encode_knrbin(&parse_ssa(ssa_sys).unwrap()).unwrap();
    // ExtraArgsPool has entry [effect_tag, payload_reg]
    // Corrupt the effect_tag to 99
    let mut bad_eff = sys_bytes.clone();
    for i in 0..bad_eff.len() - 4 {
        // Find effect tag 0 in extra pool
        if bad_eff[i..i + 2] == [0x00, 0x00] && bad_eff[i + 2..i + 4] == [0x00, 0x00] && i > 15 {
            bad_eff[i] = 99;
            break;
        }
    }
    let res = decode_knrbin(&bad_eff);
    assert!(res.is_err(), "Expected error for invalid syscall effect tag");
}

#[test]
fn test_adversarial_out_of_bounds_pool_indices() {
    let ssa = "fn f() -> void {\nbb0:\n    %0 = const_i64 10\n    ret\n}\n";
    let bytes = encode_knrbin(&parse_ssa(ssa).unwrap()).unwrap();

    // Corrupt the int_pool index in const_i64 (src1 bytes)
    let mut oob_int = bytes.clone();
    let inst_offset = bytes.len() - 16;
    oob_int[inst_offset + 4..inst_offset + 6].copy_from_slice(&0x00FFu16.to_le_bytes());
    let res = decode_knrbin(&oob_int);
    assert!(
        matches!(res, Err(BinaryError::PoolIndexOutOfBounds { pool: "int_pool", .. })),
        "Expected PoolIndexOutOfBounds for int_pool, got: {:?}",
        res
    );

    // Corrupt function name string pool index
    let mut oob_name = bytes.clone();
    // Function name index is at fn_count (offset 37) + 4 = offset 41
    // Let's inspect where fn_count is:
    let mut found = false;
    for i in 25..oob_name.len() - 4 {
        // fn_count is 1 followed by name_idx 0
        if oob_name[i..i + 4] == 1u32.to_le_bytes() && oob_name[i + 4..i + 8] == 0u32.to_le_bytes() {
            oob_name[i + 4..i + 8].copy_from_slice(&9999u32.to_le_bytes());
            found = true;
            break;
        }
    }
    assert!(found, "Should find fn_count followed by name_idx");
    let res_name = decode_knrbin(&oob_name);
    assert!(
        matches!(res_name, Err(BinaryError::PoolIndexOutOfBounds { pool: "strings", index: 9999, .. })),
        "Expected PoolIndexOutOfBounds for strings, got: {:?}",
        res_name
    );
}

#[test]
fn test_adversarial_trailing_junk_and_circular_cfgs() {
    // 1. Circular CFG roundtrip
    let circular_ssa = "\
fn loop() -> void {
bb0:
    br bb0
}
";
    let module = parse_ssa(circular_ssa).expect("Circular CFG parses in AST");
    let bytes = encode_knrbin(&module).expect("Circular CFG encodes");
    let decoded = decode_knrbin(&bytes).expect("Circular CFG decodes");
    assert_eq!(format_ssa(&decoded), circular_ssa);

    // 2. Trailing junk bytes
    let mut with_trailing = bytes.clone();
    with_trailing.extend_from_slice(&[0xDE, 0xAD, 0xBE, 0xEF, 0xCA, 0xFE]);
    let decoded_trailing = decode_knrbin(&with_trailing);
    assert!(
        decoded_trailing.is_ok(),
        "Decoder gracefully decodes valid prefix ignoring trailing junk"
    );
    assert_eq!(format_ssa(&decoded_trailing.unwrap()), circular_ssa);
}
