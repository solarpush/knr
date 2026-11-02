use knr_core::{
    decode_knrbin, encode_knrbin, format_ssa, parse_ssa, Effect, Op, ParseError, Terminator, Type,
};

// ============================================================================
// 1. ADVANCED UTF-8 ROBUSTNESS & STRING ESCAPE STRESS TESTS
// ============================================================================

#[test]
fn test_utf8_mixed_unicode_planes_roundtrip_bijection() {
    let test_strings = vec![
        // Basic ASCII
        "Hello, World!",
        // Latin-1 Supplement & Accents
        "Voix ambiguë d'un cœur qui, au zéphyr, préfère les jattes de kiwis. café résumé naïve",
        "Zwölf Boxkämpfer jagen Viktor quer über den großen Sylter Deich. ÄÖÜßäöü",
        // Cyrillic
        "Съешь же ещё этих мягких французских булок, да выпей чаю. Русский текст 12345",
        // Greek
        "ξεσκεπάζω την ψυχοφθόρα βδελυγμία. Ελληνικά γράμματα αβγδε",
        // CJK (Chinese, Japanese, Korean)
        "天地玄黃 宇宙洪荒 日月盈昃 辰宿列張 繁體中文 简体中文",
        "いろはにほへと ちりぬるを わかよたれそ つねならむ 平仮名 片仮名 カタカナ",
        "다람쥐 헌 쳇바퀴에 타고파. 대한민국 한글 테스트",
        // RTL scripts (Arabic, Hebrew)
        "نص عربي معقد مع تشكيل: بsmِ اللهِ الرَّحْمٰنِ الرَّحِيمِ",
        "דג סקרן שט בים מאוכזב ולפתע מצא חברה. שלום עליכם",
        // Devanagari & Thai
        "ऋषियों को सम्मान देना हमारी संस्कृति है। नमस्ते दुनिया",
        "เป็นมนุษย์สุดประเสริฐเลิศคุณค่า ภาษาไทย",
        // Combining characters & Zero-Width Joiners (ZWJ / ZWNJ)
        "e\u{0301} a\u{0308} o\u{0303}", // é, ä, õ composed with combining diacritics
        "prefix\u{200B}zero_width_space\u{200C}zwnj\u{200D}zwj_suffix",
        // Plane 1 (SMP) - Emojis & Complex Sequences
        "🦀🚀🔥🎉💻✨⚡🎯🛡️📦",
        "👨‍👩‍👧‍👦", // Family emoji (ZWJ sequence)
        "👩‍🔬", // Woman scientist (ZWJ sequence)
        "🏳️‍🌈", // Rainbow flag (ZWJ sequence)
        "𝄞𝄢𝄡𝅘𝅥𝅮𝅘𝅥𝅯", // Musical symbols (SMP)
        "𝔄𝔅ℭ𝔇𝔈𝔉", // Mathematical Fraktur (SMP)
        // Plane 2 (SIP) - CJK Extension B
        "𠀀𠀁𪚥",
        // Boundary Unicode scalars
        "\u{0000}", // Null byte
        "\u{007F}", // Delete
        "\u{0080}", // First 2-byte
        "\u{07FF}", // Last 2-byte
        "\u{0800}", // First 3-byte
        "\u{FFFF}", // Last 3-byte
        "\u{10000}", // First 4-byte
        "\u{10FFFF}", // Absolute maximum Unicode scalar value
        // Mixed Bidi
        "English English مرحبا Arabic 1234 עברית Hebrew Mixed",
    ];

    for (i, s) in test_strings.iter().enumerate() {
        let fn_name = format!("test_str_{i}");
        let mut module = knr_core::Module::new();
        let mut func = knr_core::Function::new(fn_name, Type::Void);
        let mut block = knr_core::BasicBlock::new(0, Terminator::Ret(None));
        block.push(knr_core::Instruction::new(
            Some(0),
            Type::Str,
            Op::ConstStr(s.to_string()),
            1,
        ));
        block.push(knr_core::Instruction::new(
            None,
            Type::Void,
            Op::Free(0),
            2,
        ));
        func.add_block(block);
        module.add_function(func);

        // 1. AST -> text
        let formatted = format_ssa(&module);

        // 2. text -> AST
        let parsed = parse_ssa(&formatted).unwrap_or_else(|e| {
            panic!("Failed to parse formatted SSA for string #{i} ({:?}): {:?}", s, e);
        });

        // Verify string value in parsed AST
        let parsed_str = match &parsed.functions[0].blocks[0].instructions[0].op {
            Op::ConstStr(val) => val.as_str(),
            other => panic!("Expected ConstStr, got {:?}", other),
        };
        assert_eq!(parsed_str, *s, "String value corrupted in text parse for #{i}");

        // 3. AST -> binary
        let binary = encode_knrbin(&parsed).unwrap_or_else(|e| {
            panic!("Failed to encode binary for string #{i}: {:?}", e);
        });

        // 4. binary -> AST
        let decoded = decode_knrbin(&binary).unwrap_or_else(|e| {
            panic!("Failed to decode binary for string #{i}: {:?}", e);
        });

        // Verify string value in decoded AST
        let decoded_str = match &decoded.functions[0].blocks[0].instructions[0].op {
            Op::ConstStr(val) => val.as_str(),
            other => panic!("Expected ConstStr in decoded binary, got {:?}", other),
        };
        assert_eq!(decoded_str, *s, "String value corrupted in binary decode for #{i}");

        // 5. Canonical format equality
        let re_formatted = format_ssa(&decoded);
        assert_eq!(re_formatted, formatted, "1:1 canonical formatting bijection failed for string #{i}");
    }
}

#[test]
fn test_deeply_nested_escape_sequences_roundtrip() {
    let escape_cases = vec![
        r#"\""#,
        r#"\\"#,
        r#"\\\""#,
        r#"\\\\\""#,
        r#"\"\"\""#,
        "Line1\nLine2\r\nLine3\tTabbed\0Null",
        r#"Mixed: \\n and \n and \\" and \" with 🦀"#,
        r#"Escapes at boundary: \"middle\""#,
        r#"Multiple consecutive backslashes: \\\\\\"#,
        "Null byte in between: hello\0world\0!",
    ];

    for (i, case) in escape_cases.iter().enumerate() {
        let mut module = knr_core::Module::new();
        let mut func = knr_core::Function::new(format!("fn_esc_{i}"), Type::Void);
        let mut block = knr_core::BasicBlock::new(0, Terminator::Ret(None));
        block.push(knr_core::Instruction::new(
            Some(0),
            Type::Str,
            Op::ConstStr(case.to_string()),
            1,
        ));
        block.push(knr_core::Instruction::new(
            None,
            Type::Void,
            Op::Free(0),
            2,
        ));
        func.add_block(block);
        module.add_function(func);

        let formatted = format_ssa(&module);
        let parsed = parse_ssa(&formatted).unwrap_or_else(|e| {
            panic!("Failed to parse escape case #{i} ({:?}): {:?}", case, e);
        });

        let parsed_val = match &parsed.functions[0].blocks[0].instructions[0].op {
            Op::ConstStr(val) => val.as_str(),
            other => panic!("Expected ConstStr, got {:?}", other),
        };
        assert_eq!(parsed_val, *case, "Escape mismatch for case #{i}");

        let bin = encode_knrbin(&parsed).expect("encode knrbin failed");
        let dec = decode_knrbin(&bin).expect("decode knrbin failed");
        let re_formatted = format_ssa(&dec);
        assert_eq!(re_formatted, formatted, "Escape bijection failed for case #{i}");
    }
}

#[test]
fn test_large_utf8_string_stress_64kb() {
    // Generate a 64 KiB string with alternating emojis, Chinese, Arabic, and escapes
    let mut large_str = String::with_capacity(65536);
    let chunk = "🦀 Rust knr 🚀 \n\t 中文 漢字 \0 \\\" مرحبا بالعالم \u{10FFFF} ";
    while large_str.len() < 65536 {
        large_str.push_str(chunk);
    }

    let mut module = knr_core::Module::new();
    let mut func = knr_core::Function::new("test_large_str", Type::Void);
    let mut block = knr_core::BasicBlock::new(0, Terminator::Ret(None));
    block.push(knr_core::Instruction::new(
        Some(0),
        Type::Str,
        Op::ConstStr(large_str.clone()),
        1,
    ));
    block.push(knr_core::Instruction::new(
        None,
        Type::Void,
        Op::Free(0),
        2,
    ));
    func.add_block(block);
    module.add_function(func);

    let formatted = format_ssa(&module);
    let parsed = parse_ssa(&formatted).expect("Failed to parse 64KB UTF-8 SSA");
    let bin = encode_knrbin(&parsed).expect("Failed to encode 64KB UTF-8 knrbin");
    let dec = decode_knrbin(&bin).expect("Failed to decode 64KB UTF-8 knrbin");
    let re_formatted = format_ssa(&dec);
    assert_eq!(re_formatted, formatted, "64KB UTF-8 string roundtrip failed");
}

// ============================================================================
// 2. BASIC BLOCK TERMINATOR ENFORCEMENT & MALFORMED CFG REJECTION
// ============================================================================

#[test]
fn test_reject_unterminated_single_block_ending_at_eof() {
    let ssa = "\
fn test_missing_term() -> void {
bb0:
    %0 = const_i64 42
}
";
    let res = parse_ssa(ssa);
    match res {
        Err(ParseError::MissingTerminator { block_id, .. }) => {
            assert_eq!(block_id, 0);
        }
        other => panic!("Expected MissingTerminator for bb0 at EOF, got: {:?}", other),
    }
}

#[test]
fn test_reject_unterminated_first_block_before_next_block() {
    let ssa = "\
fn test_missing_term_first() -> void {
bb0:
    %0 = const_i64 1
bb1:
    ret
}
";
    let res = parse_ssa(ssa);
    match res {
        Err(ParseError::MissingTerminator { block_id, .. }) => {
            assert_eq!(block_id, 0);
        }
        other => panic!("Expected MissingTerminator for bb0 before bb1, got: {:?}", other),
    }
}

#[test]
fn test_reject_unterminated_middle_block() {
    let ssa = "\
fn test_missing_term_mid() -> void {
bb0:
    br bb1
bb1:
    %0 = const_i64 2
bb2:
    ret
}
";
    let res = parse_ssa(ssa);
    match res {
        Err(ParseError::MissingTerminator { block_id, .. }) => {
            assert_eq!(block_id, 1);
        }
        other => panic!("Expected MissingTerminator for bb1, got: {:?}", other),
    }
}

#[test]
fn test_reject_unterminated_last_block_in_multiblock() {
    let ssa = "\
fn test_missing_term_last() -> void {
bb0:
    br bb1
bb1:
    %0 = const_i64 2
}
";
    let res = parse_ssa(ssa);
    match res {
        Err(ParseError::MissingTerminator { block_id, .. }) => {
            assert_eq!(block_id, 1);
        }
        other => panic!("Expected MissingTerminator for bb1, got: {:?}", other),
    }
}

#[test]
fn test_reject_empty_block_without_terminator() {
    let ssa = "\
fn test_empty_block_no_term() -> void {
bb0:
}
";
    let res = parse_ssa(ssa);
    match res {
        Err(ParseError::MissingTerminator { block_id, .. }) => {
            assert_eq!(block_id, 0);
        }
        other => panic!("Expected MissingTerminator for empty bb0, got: {:?}", other),
    }
}

#[test]
fn test_reject_empty_block_followed_by_another_block() {
    let ssa = "\
fn test_empty_first_block() -> void {
bb0:
bb1:
    ret
}
";
    let res = parse_ssa(ssa);
    match res {
        Err(ParseError::MissingTerminator { block_id, .. }) => {
            assert_eq!(block_id, 0);
        }
        other => panic!("Expected MissingTerminator for empty bb0 before bb1, got: {:?}", other),
    }
}

#[test]
fn test_reject_instruction_after_ret() {
    let ssa = "\
fn test_dead_after_ret() -> void {
bb0:
    ret
    %0 = const_i64 100
}
";
    let res = parse_ssa(ssa);
    assert!(
        matches!(res, Err(ParseError::InvalidSyntax { .. })),
        "Expected InvalidSyntax for instruction after ret, got: {:?}",
        res
    );
}

#[test]
fn test_reject_instruction_after_br() {
    let ssa = "\
fn test_dead_after_br() -> void {
bb0:
    br bb1
    %0 = const_i64 100
bb1:
    ret
}
";
    let res = parse_ssa(ssa);
    assert!(
        matches!(res, Err(ParseError::InvalidSyntax { .. })),
        "Expected InvalidSyntax for instruction after br, got: {:?}",
        res
    );
}

#[test]
fn test_reject_instruction_after_br_if() {
    let ssa = "\
fn test_dead_after_br_if() -> void {
bb0:
    %0 = const_bool true
    br_if %0 bb1 bb2
    %1 = const_i64 999
bb1:
    ret
bb2:
    ret
}
";
    let res = parse_ssa(ssa);
    assert!(
        matches!(res, Err(ParseError::InvalidSyntax { .. })),
        "Expected InvalidSyntax for instruction after br_if, got: {:?}",
        res
    );
}

#[test]
fn test_reject_second_terminator_in_block() {
    let ssa_ret_ret = "\
fn test_double_ret() -> void {
bb0:
    ret
    ret
}
";
    assert!(matches!(
        parse_ssa(ssa_ret_ret),
        Err(ParseError::InvalidSyntax { .. })
    ));

    let ssa_br_ret = "\
fn test_br_ret() -> void {
bb0:
    br bb1
    ret
bb1:
    ret
}
";
    assert!(matches!(
        parse_ssa(ssa_br_ret),
        Err(ParseError::InvalidSyntax { .. })
    ));
}

#[test]
fn test_reject_instruction_outside_basic_block() {
    let ssa_inst_before_bb = "\
fn test_outside_bb() -> void {
    %0 = const_i64 10
bb0:
    ret
}
";
    let res = parse_ssa(ssa_inst_before_bb);
    assert!(
        matches!(res, Err(ParseError::InvalidSyntax { .. })),
        "Expected InvalidSyntax for instruction before any bb label, got: {:?}",
        res
    );

    let ssa_term_before_bb = "\
fn test_term_before_bb() -> void {
    ret
bb0:
    ret
}
";
    let res = parse_ssa(ssa_term_before_bb);
    assert!(
        matches!(res, Err(ParseError::InvalidSyntax { .. })),
        "Expected InvalidSyntax for terminator before any bb label, got: {:?}",
        res
    );
}

#[test]
fn test_reject_malformed_terminator_tokens() {
    let malformed_cases = vec![
        "fn t() -> void { bb0: ret %not_a_reg }",
        "fn t() -> void { bb0: ret % }",
        "fn t() -> void { bb0: ret %0 %1 }",
        "fn t() -> void { bb0: br }",
        "fn t() -> void { bb0: br bb }",
        "fn t() -> void { bb0: br bb-1 }",
        "fn t() -> void { bb0: br %0 }",
        "fn t() -> void { bb0: br_if }",
        "fn t() -> void { bb0: br_if %0 }",
        "fn t() -> void { bb0: br_if %0 bb1 }",
        "fn t() -> void { bb0: br_if %0 bb1 bb2 bb3 }",
        "fn t() -> void { bb0: br_if 0 bb1 bb2 }",
        "fn t() -> void { bb0: br_if %0 1 bb2 }",
        "fn t() -> void { bb0: br_if %0 bb1 2 }",
    ];

    for case in malformed_cases {
        let res = parse_ssa(case);
        assert!(
            res.is_err(),
            "Malformed terminator must be rejected, accepted: {}",
            case
        );
    }
}

// ============================================================================
// 3. PARSER SYNTAX ERROR HANDLING & NEGATIVE VALIDATION
// ============================================================================

#[test]
fn test_reject_unclosed_string_at_eof() {
    let ssa = "\
fn test_unclosed() -> void {
bb0:
    %0 = const_str \"unclosed string without end quote
    ret
}
";
    let res = parse_ssa(ssa);
    assert!(
        matches!(res, Err(ParseError::UnterminatedString { .. })),
        "Expected UnterminatedString, got: {:?}",
        res
    );
}

#[test]
fn test_reject_unclosed_brace_at_eof() {
    let ssa = "\
fn test_unclosed_fn() -> void {
bb0:
    ret
";
    let res = parse_ssa(ssa);
    assert!(
        matches!(res, Err(ParseError::MissingClosingBrace { .. })),
        "Expected MissingClosingBrace, got: {:?}",
        res
    );
}

#[test]
fn test_reject_invalid_block_labels() {
    let invalid_labels = vec![
        "fn t() -> void { b0: ret }",
        "fn t() -> void { bb: ret }",
        "fn t() -> void { bb_0: ret }",
        "fn t() -> void { bbabc: ret }",
        "fn t() -> void { bb1 ret }", // missing colon
        "fn t() -> void { :bb1 ret }",
    ];

    for case in invalid_labels {
        let res = parse_ssa(case);
        assert!(
            res.is_err(),
            "Invalid block label must be rejected: {}",
            case
        );
    }
}

#[test]
fn test_reject_invalid_register_tokens_in_parser() {
    let invalid_regs = vec![
        "fn t() -> void { bb0: % = const_i64 1 \n ret }",
        "fn t() -> void { bb0: %abc = const_i64 1 \n ret }",
        "fn t() -> void { bb0: %-5 = const_i64 1 \n ret }",
        "fn t() -> void { bb0: 0 = const_i64 1 \n ret }",
        "fn t() -> void { bb0: %%0 = const_i64 1 \n ret }",
    ];

    for case in invalid_regs {
        let res = parse_ssa(case);
        assert!(
            res.is_err(),
            "Invalid register token must be rejected: {}",
            case
        );
    }
}

#[test]
fn test_reject_register_id_overflow_in_binary_codec() {
    use knr_core::BinaryError;
    // Max RegId is 65534 (u16::MAX - 1)
    let ssa_65535 = "\
fn t() -> void {
bb0:
    %65535 = const_i64 1
    ret
}
";
    let module = parse_ssa(ssa_65535).expect("Parser allows u32 reg id");
    let res = encode_knrbin(&module);
    assert!(
        matches!(res, Err(BinaryError::RegisterOverflow(65535))),
        "Register 65535 must trigger BinaryError::RegisterOverflow, got: {:?}",
        res
    );

    let ssa_100000 = "\
fn t() -> void {
bb0:
    %100000 = const_i64 1
    ret
}
";
    let module = parse_ssa(ssa_100000).expect("Parser allows u32 reg id");
    let res = encode_knrbin(&module);
    assert!(
        matches!(res, Err(BinaryError::RegisterOverflow(100000))),
        "Register 100000 must trigger BinaryError::RegisterOverflow, got: {:?}",
        res
    );
}

// ============================================================================
// 4. COMPLEX MULTI-BLOCK SSA & ALL OPCODES CANONICAL BIJECTION
// ============================================================================

#[test]
fn test_complex_multiblock_cfgs_and_syscalls_bijection() {
    let ssa = "\
fn compute_pipeline(%0: buf, %1: str) -> i64 effects(Net, Hw) {
bb0:
    %2 = const_i64 64
    %3 = buf_alloc %2
    %4 = const_i64 0
    %5 = const_i64 255
    %6 = buf_write_u8 %3 %4 %5
    %7 = buf_read_u8 %6 %4
    %8 = const_i64 0
    %9 = const_i64 32
    %10 = buf_slice %6 %8 %9
    %11 = vec_new i64
    %12 = vec_push %11 %7
    %13 = vec_len %12
    %14 = vec_get %12 %4
    %15 = map_new str i64
    %16 = map_set %15 %1 %14
    %17 = map_get %16 %1
    %18 = map_del %16 %1
    %19 = const_i64 8
    %20 = chan_new i64 %19
    %21 = chan_send %20 %17
    %22 = chan_recv %20
    %23 = spawn \"async_worker\" %22
    %24 = syscall Net \"net.listen\" %23
    %25 = syscall Hw \"hw.simd_xor\" %24
    %26 = const_i64 100
    %27 = cmp_lt %25 %26
    br_if %27 bb1 bb2
bb1:
    %28 = const_i64 1
    %29 = add %25 %28
    br bb3
bb2:
    %30 = const_i64 2
    %31 = sub %25 %30
    br bb3
bb3:
    %32 = const_i64 0
    free %0
    free %10
    free %12
    free %18
    free %20
    ret %32
}
";
    let module = parse_ssa(ssa).expect("Failed to parse complex multi-block SSA");
    assert_eq!(module.functions.len(), 1);
    let func = &module.functions[0];
    assert_eq!(func.effects, vec![Effect::Net, Effect::Hw]);
    assert_eq!(func.blocks.len(), 4);

    let formatted = format_ssa(&module);
    assert_eq!(formatted, ssa, "Canonical format mismatch on multi-block");

    let binary = encode_knrbin(&module).expect("Failed to encode multi-block to knrbin");
    let decoded = decode_knrbin(&binary).expect("Failed to decode knrbin to module");
    let re_formatted = format_ssa(&decoded);
    assert_eq!(re_formatted, ssa, "Binary roundtrip mismatch on multi-block");
}
