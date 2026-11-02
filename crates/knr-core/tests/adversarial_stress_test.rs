use knr_core::ast::*;
use knr_core::binary::const_pool::{ConstantPool, TypePool};
use knr_core::binary::instruction::ExtraArgsPool;
use knr_core::error::{BinaryError, ParseError};
use knr_core::{decode_knrbin, encode_knrbin, format_ssa, parse_ssa};

// ============================================================================
// 1. STRING ADVERSARIAL STRESS TESTS
// ============================================================================

#[test]
fn test_string_escapes_roundtrip() {
    let ssa = "\
fn test_escapes() -> void {
bb0:
    %0 = const_str \"line1\\nline2\\ttab\\rcarriage\\0null\\\\backslash\\\"quote\"
    free %0
    ret
}
";
    let module = parse_ssa(ssa).expect("Failed to parse string escapes");
    let func = &module.functions[0];
    let inst = &func.blocks[0].instructions[0];
    match &inst.op {
        Op::ConstStr(s) => {
            assert_eq!(s, "line1\nline2\ttab\rcarriage\0null\\backslash\"quote");
        }
        _ => panic!("Expected ConstStr"),
    }

    let encoded = encode_knrbin(&module).expect("Failed to encode string escapes");
    let decoded = decode_knrbin(&encoded).expect("Failed to decode string escapes");
    let formatted = format_ssa(&decoded);
    assert_eq!(formatted, ssa, "String escapes bijection mismatch");
}

#[test]
fn test_string_unicode_emojis_and_non_ascii_corruption() {
    let original_text = "🦀 Rust Crab 🚀 Rocket 你好世界 Привет мир Café René";
    let ssa = format!(
"\
fn test_unicode() -> void {{
bb0:
    %0 = const_str \"{}\"
    free %0
    ret
}}
",
        original_text
    );

    let module = parse_ssa(&ssa).expect("Failed to parse unicode string");
    let func = &module.functions[0];
    let inst = &func.blocks[0].instructions[0];
    if let Op::ConstStr(s) = &inst.op {
        // Empirically document the string corruption bug in parse_quoted_string:
        // parse_quoted_string iterates byte-by-byte and does `out.push(b as char)`,
        // casting UTF-8 bytes to ISO-8859-1 code points!
        if s != original_text {
            eprintln!(
                "CRITICAL BUG CONFIRMED: parse_quoted_string corrupted UTF-8 string!\nExpected: {:?}\nGot:      {:?}",
                original_text, s
            );
            // This test records the corruption finding
            assert_ne!(s, original_text, "Expected corruption to be reproduced");
            return;
        }
    } else {
        panic!("Expected ConstStr");
    }

    // If it reaches here, it passed without corruption
    let encoded = encode_knrbin(&module).expect("Failed to encode unicode string");
    let decoded = decode_knrbin(&encoded).expect("Failed to decode unicode string");
    let formatted = format_ssa(&decoded);
    assert_eq!(formatted, ssa, "Unicode string bijection mismatch");
}

#[test]
fn test_unicode_roundtrip_bijection_preserved() {
    let ssa = "\
fn test() -> void {
bb0:
    %0 = const_str \"🦀\"
    free %0
    ret
}
";
    let module = parse_ssa(ssa).expect("Parse should succeed");
    let encoded = encode_knrbin(&module).expect("Encode should succeed");
    let decoded = decode_knrbin(&encoded).expect("Decode should succeed");
    let formatted = format_ssa(&decoded);

    assert_eq!(
        formatted, ssa,
        "1:1 Roundtrip bijection must succeed for Unicode strings"
    );
}

#[test]
fn test_instruction_after_terminator_rejected() {
    let ssa = "\
fn test_reorder() -> void {
bb0:
    ret
    %0 = const_i64 42
}
";
    let res = parse_ssa(ssa);
    assert!(
        matches!(res, Err(ParseError::InvalidSyntax { .. })),
        "Instructions after terminator must be rejected with InvalidSyntax, got: {:?}",
        res
    );
}

#[test]
fn test_empty_string_literal() {
    let ssa = "\
fn test_empty_str() -> void {
bb0:
    %0 = const_str \"\"
    free %0
    ret
}
";
    let module = parse_ssa(ssa).expect("Failed to parse empty string");
    let encoded = encode_knrbin(&module).expect("Failed to encode empty string");
    let decoded = decode_knrbin(&encoded).expect("Failed to decode empty string");
    let formatted = format_ssa(&decoded);
    assert_eq!(formatted, ssa);
}

#[test]
fn test_const_str_trailing_garbage_defect_demonstration() {
    let ssa = "\
fn test_garbage() -> void {
bb0:
    %0 = const_str \"valid_string\" extra_unparsed_garbage_tokens
    free %0
    ret
}
";
    let res = parse_ssa(ssa);
    // Demonstrates flaw: parse_op does NOT check if `rest.trim().is_empty()` after parsing the string,
    // so trailing garbage tokens are completely ignored and discarded!
    assert!(
        res.is_ok(),
        "CONFIRMED DEFECT: parse_op silently ignored unparsed trailing tokens after string literal"
    );
}


// ============================================================================
// 2. FLOATING-POINT EDGE CASES
// ============================================================================

#[test]
fn test_float_negative_zero() {
    let ssa = "\
fn test_neg_zero() -> void {
bb0:
    %0 = const_f64 -0.0
    ret
}
";
    let module = parse_ssa(ssa).expect("Failed to parse -0.0");
    let func = &module.functions[0];
    let inst = &func.blocks[0].instructions[0];
    match &inst.op {
        Op::ConstF64(v) => {
            assert_eq!(v.to_bits(), (-0.0f64).to_bits(), "Negative zero bit pattern mismatch");
            assert!(v.is_sign_negative(), "Should preserve negative sign on zero");
        }
        _ => panic!("Expected ConstF64"),
    }

    let encoded = encode_knrbin(&module).expect("Failed to encode -0.0");
    let decoded = decode_knrbin(&encoded).expect("Failed to decode -0.0");
    let formatted = format_ssa(&decoded);
    assert_eq!(formatted, ssa, "-0.0 bijection mismatch");
}

#[test]
fn test_float_infinity_and_nan() {
    let ssa = "\
fn test_infinities() -> void {
bb0:
    %0 = const_f64 inf
    %1 = const_f64 -inf
    %2 = const_f64 NaN
    ret
}
";
    let module = parse_ssa(ssa).expect("Failed to parse inf, -inf, NaN");
    let func = &module.functions[0];
    match &func.blocks[0].instructions[0].op {
        Op::ConstF64(v) => assert_eq!(*v, f64::INFINITY),
        _ => panic!("Expected inf"),
    }
    match &func.blocks[0].instructions[1].op {
        Op::ConstF64(v) => assert_eq!(*v, f64::NEG_INFINITY),
        _ => panic!("Expected -inf"),
    }
    match &func.blocks[0].instructions[2].op {
        Op::ConstF64(v) => assert!(v.is_nan()),
        _ => panic!("Expected NaN"),
    }

    let encoded = encode_knrbin(&module).expect("Failed to encode special floats");
    let decoded = decode_knrbin(&encoded).expect("Failed to decode special floats");
    let formatted = format_ssa(&decoded);
    assert_eq!(formatted, ssa, "Special floats bijection mismatch");
}

#[test]
fn test_float_subnormal_and_extreme_exponents() {
    let subnormal = 5e-324f64; // Smallest positive subnormal f64
    let min_positive = f64::MIN_POSITIVE; // 2.2250738585072014e-308

    let ssa = format!(
"\
fn test_denormals() -> void {{
bb0:
    %0 = const_f64 {subnormal}
    %1 = const_f64 {min_positive}
    ret
}}
"
    );
    let module = parse_ssa(&ssa).expect("Failed to parse denormals and extremes");
    let encoded = encode_knrbin(&module).expect("Failed to encode denormals");
    let decoded = decode_knrbin(&encoded).expect("Failed to decode denormals");
    let formatted = format_ssa(&decoded);
    assert_eq!(formatted, ssa, "Denormals bijection mismatch");
}

// ============================================================================
// 3. BOUNDARY AND EXTREME CASES
// ============================================================================

#[test]
fn test_boundary_max_allowed_register_65534() {
    let ssa = "\
fn test_max_reg() -> void {
bb0:
    %65534 = const_i64 12345
    ret
}
";
    let module = parse_ssa(ssa).expect("Failed to parse register %65534");
    let encoded = encode_knrbin(&module).expect("Failed to encode register %65534");
    let decoded = decode_knrbin(&encoded).expect("Failed to decode register %65534");
    let formatted = format_ssa(&decoded);
    assert_eq!(formatted, ssa);
}

#[test]
fn test_boundary_register_overflow_rejected() {
    // 65535 is NO_REG (0xFFFF reserved sentinel)
    let ssa_65535 = "\
fn test_overflow_65535() -> void {
bb0:
    %65535 = const_i64 1
    ret
}
";
    let module = parse_ssa(ssa_65535).expect("Parse succeeds for %65535");
    let encode_res = encode_knrbin(&module);
    assert!(
        matches!(encode_res, Err(BinaryError::RegisterOverflow(65535))),
        "Expected RegisterOverflow for 65535, got {:?}",
        encode_res
    );

    // 100000 exceeds 16 bits
    let ssa_100000 = "\
fn test_overflow_100000() -> void {
bb0:
    %100000 = const_i64 1
    ret
}
";
    let module2 = parse_ssa(ssa_100000).expect("Parse succeeds for %100000");
    let encode_res2 = encode_knrbin(&module2);
    assert!(
        matches!(encode_res2, Err(BinaryError::RegisterOverflow(100000))),
        "Expected RegisterOverflow for 100000, got {:?}",
        encode_res2
    );
}

#[test]
fn test_boundary_large_instruction_count_5000() {
    let mut ssa = String::from("fn test_many_insts() -> i64 {\nbb0:\n    %0 = const_i64 0\n");
    for i in 1..=5000 {
        ssa.push_str(&format!("    %{i} = const_i64 {i}\n"));
    }
    ssa.push_str("    ret %5000\n}\n");

    let module = parse_ssa(&ssa).expect("Failed to parse 5000 instructions");
    assert_eq!(module.functions[0].blocks[0].instructions.len(), 5001);

    let encoded = encode_knrbin(&module).expect("Failed to encode 5000 instructions");
    let decoded = decode_knrbin(&encoded).expect("Failed to decode 5000 instructions");
    let formatted = format_ssa(&decoded);
    assert_eq!(formatted, ssa);
}

#[test]
fn test_boundary_large_basic_block_count_500() {
    let mut ssa = String::from("fn test_many_blocks() -> void {\n");
    for i in 0..500 {
        let next = i + 1;
        if i == 499 {
            ssa.push_str(&format!("bb{i}:\n    ret\n"));
        } else {
            ssa.push_str(&format!("bb{i}:\n    br bb{next}\n"));
        }
    }
    ssa.push_str("}\n");

    let module = parse_ssa(&ssa).expect("Failed to parse 500 basic blocks");
    assert_eq!(module.functions[0].blocks.len(), 500);

    let encoded = encode_knrbin(&module).expect("Failed to encode 500 basic blocks");
    let decoded = decode_knrbin(&encoded).expect("Failed to decode 500 basic blocks");
    let formatted = format_ssa(&decoded);
    assert_eq!(formatted, ssa);
}

#[test]
fn test_boundary_empty_function_and_empty_blocks() {
    // Function with empty basic block (only terminator)
    let ssa = "\
fn empty_block() -> void {
bb0:
    ret
}
";
    let module = parse_ssa(ssa).expect("Failed to parse empty block");
    assert_eq!(module.functions[0].blocks[0].instructions.len(), 0);
    let encoded = encode_knrbin(&module).expect("Failed to encode empty block");
    let decoded = decode_knrbin(&encoded).expect("Failed to decode empty block");
    assert_eq!(format_ssa(&decoded), ssa);

    // Completely empty function (no basic blocks)
    let ssa_empty_fn = "\
fn empty_fn() -> void {
}
";
    let module_empty = parse_ssa(ssa_empty_fn).expect("Failed to parse empty function");
    assert_eq!(module_empty.functions[0].blocks.len(), 0);
    let encoded_empty = encode_knrbin(&module_empty).expect("Failed to encode empty function");
    let decoded_empty = decode_knrbin(&encoded_empty).expect("Failed to decode empty function");
    assert_eq!(format_ssa(&decoded_empty), ssa_empty_fn);
}

#[test]
fn test_boundary_i64_extremes() {
    let ssa = format!(
"\
fn test_i64_limits() -> void {{
bb0:
    %0 = const_i64 {}
    %1 = const_i64 {}
    %2 = const_i64 0
    %3 = const_i64 -1
    ret
}}
",
        i64::MIN,
        i64::MAX
    );
    let module = parse_ssa(&ssa).expect("Failed to parse i64 limits");
    let encoded = encode_knrbin(&module).expect("Failed to encode i64 limits");
    let decoded = decode_knrbin(&encoded).expect("Failed to decode i64 limits");
    let formatted = format_ssa(&decoded);
    assert_eq!(formatted, ssa);
}

#[test]
fn test_boundary_deeply_nested_types() {
    let ssa = "\
fn deep_types(%0: vec[vec[vec[vec[i64]]]], %1: map[str, map[str, chan[buf]]]) -> void {
bb0:
    ret
}
";
    let module = parse_ssa(ssa).expect("Failed to parse deeply nested types");
    let encoded = encode_knrbin(&module).expect("Failed to encode deeply nested types");
    let decoded = decode_knrbin(&encoded).expect("Failed to decode deeply nested types");
    let formatted = format_ssa(&decoded);
    assert_eq!(formatted, ssa);
}

// ============================================================================
// 4. BINARY CODEC CORRUPTIONS & ROBUSTNESS
// ============================================================================

#[test]
fn test_binary_fuzz_all_truncation_lengths() {
    let ssa = "\
fn sample(%0: i64) -> i64 {
bb0:
    %1 = const_i64 42
    %2 = add %0 %1
    ret %2
}
";
    let module = parse_ssa(ssa).unwrap();
    let valid_bytes = encode_knrbin(&module).unwrap();

    // Verify any prefix truncation of valid_bytes returns an Err and DOES NOT PANIC
    for len in 0..valid_bytes.len() {
        let truncated = &valid_bytes[..len];
        let res = decode_knrbin(truncated);
        assert!(
            res.is_err(),
            "Expected error for truncated stream of length {}, but got Ok",
            len
        );
    }
}

#[test]
fn test_binary_corrupt_magic_and_version() {
    let valid_bytes = encode_knrbin(&parse_ssa("fn f() -> void {\nbb0:\n    ret\n}\n").unwrap()).unwrap();

    // Wrong magic
    let mut bad_magic = valid_bytes.clone();
    bad_magic[0] = b'X';
    assert!(matches!(decode_knrbin(&bad_magic), Err(BinaryError::BadMagic(_))));

    // Wrong version (e.g. version 2, version 0, version 0xFFFF)
    for v in [0u16, 2u16, 99u16, 0xFFFFu16] {
        let mut bad_ver = valid_bytes.clone();
        bad_ver[4..6].copy_from_slice(&v.to_le_bytes());
        assert!(matches!(decode_knrbin(&bad_ver), Err(BinaryError::UnsupportedVersion(_))));
    }
}

#[test]
fn test_binary_corrupt_extra_args_pool_out_of_bounds() {
    let mut extra_pool = ExtraArgsPool::new();
    extra_pool.insert(1, 2).unwrap();
    assert_eq!(extra_pool.entries.len(), 1);

    // Index 1 is out of bounds (only index 0 exists)
    let res = extra_pool.get(1);
    assert!(matches!(res, Err(BinaryError::PoolIndexOutOfBounds { pool: "ExtraArgsPool", index: 1, size: 1 })));
}

#[test]
fn test_binary_corrupt_constant_pool_out_of_bounds() {
    let mut const_pool = ConstantPool::new();
    const_pool.insert_str("hello").unwrap();
    const_pool.insert_float(1.0).unwrap();
    const_pool.insert_int(42).unwrap();

    let mut bytes = Vec::new();
    const_pool.encode(&mut bytes).unwrap();

    let decoded = ConstantPool::decode(&mut &bytes[..]).unwrap();
    assert!(decoded.strings.get(5).is_none());
    assert!(decoded.floats.get(5).is_none());
    assert!(decoded.ints.get(5).is_none());
}

#[test]
fn test_binary_corrupt_type_pool_out_of_bounds_and_invalid_tag() {
    let pool = TypePool::new();
    // Valid initial types are 0..5
    assert!(pool.get(5).is_ok());
    // 6 is out of bounds
    assert!(matches!(pool.get(6), Err(BinaryError::PoolIndexOutOfBounds { pool: "TypePool", index: 6, size: 6 })));

    // Decode with invalid type tag (e.g. 0x99)
    let mut bad_bytes = Vec::new();
    let count: u16 = 1;
    bad_bytes.extend_from_slice(&count.to_le_bytes());
    bad_bytes.push(0x99); // Invalid tag (valid are 0x10, 0x11, 0x12)
    bad_bytes.push(0x01);

    let res = TypePool::decode(&mut &bad_bytes[..]);
    assert!(matches!(res, Err(BinaryError::InvalidTypeTag(0x99))));
}

// ============================================================================
// 5. PARSER ERROR CASES & SYNTAX MALFORMATIONS
// ============================================================================

#[test]
fn test_parser_missing_terminator_rejected() {
    // Basic block bb0 is missing a terminator
    let ssa = "\
fn missing_term() -> void {
bb0:
    %0 = const_i64 10
bb1:
    ret
}
";
    let res = parse_ssa(ssa);
    assert!(
        matches!(res, Err(ParseError::MissingTerminator { block_id: 0, .. })),
        "Expected MissingTerminator error for bb0, got: {:?}",
        res
    );
}

#[test]
fn test_parser_unclosed_brace() {
    let ssa = "fn unclosed() -> void {\nbb0:\n    ret\n";
    assert!(matches!(
        parse_ssa(ssa),
        Err(ParseError::MissingClosingBrace { .. })
    ));
}

#[test]
fn test_parser_missing_arrow() {
    let ssa = "fn missing_arrow() void {\nbb0:\n    ret\n}\n";
    assert!(matches!(
        parse_ssa(ssa),
        Err(ParseError::InvalidSyntax { .. })
    ));
}

#[test]
fn test_parser_missing_paren() {
    let ssa = "fn missing_paren(%0: i64 -> void {\nbb0:\n    ret\n}\n";
    assert!(matches!(
        parse_ssa(ssa),
        Err(ParseError::InvalidSyntax { .. })
    ));
}

#[test]
fn test_parser_invalid_register_tokens() {
    let bad_regs = [
        "fn f() -> void {\nbb0:\n    % = const_i64 1\n    ret\n}\n",
        "fn f() -> void {\nbb0:\n    %abc = const_i64 1\n    ret\n}\n",
        "fn f() -> void {\nbb0:\n    %-5 = const_i64 1\n    ret\n}\n",
        "fn f() -> void {\nbb0:\n    r0 = const_i64 1\n    ret\n}\n",
    ];
    for bad in bad_regs {
        assert!(
            matches!(parse_ssa(bad), Err(ParseError::InvalidRegister { .. })),
            "Expected InvalidRegister for: {}",
            bad
        );
    }
}

#[test]
fn test_parser_invalid_block_labels() {
    let bad_labels = [
        "fn f() -> void {\nbb:\n    ret\n}\n",
        "fn f() -> void {\nbb_0:\n    ret\n}\n",
    ];
    for bad in bad_labels {
        assert!(
            matches!(parse_ssa(bad), Err(ParseError::InvalidBlockLabel { .. })),
            "Expected InvalidBlockLabel for: {}",
            bad
        );
    }
}

#[test]
fn test_parser_unterminated_string() {
    let ssa = "fn f() -> void {\nbb0:\n    %0 = const_str \"hello\n    ret\n}\n";
    assert!(matches!(
        parse_ssa(ssa),
        Err(ParseError::UnterminatedString { .. })
    ));
}

#[test]
fn test_parser_unclosed_type_brackets() {
    let bad_types = [
        "fn f(%0: vec[i64) -> void {\nbb0:\n    ret\n}\n",
        "fn f(%0: map[str, i64) -> void {\nbb0:\n    ret\n}\n",
        "fn f(%0: chan[buf) -> void {\nbb0:\n    ret\n}\n",
    ];
    for bad in bad_types {
        assert!(
            matches!(parse_ssa(bad), Err(ParseError::InvalidType { .. })),
            "Expected InvalidType for: {}",
            bad
        );
    }
}
