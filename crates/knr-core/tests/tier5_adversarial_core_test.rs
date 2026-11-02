use knr_core::ast::*;
use knr_core::binary::{decode_knrbin, encode_knrbin};
use knr_core::error::{BinaryError, ParseError};
use knr_core::formatter::format_function;
use knr_core::parser::{parse_full_type, parse_function, parse_ssa};

#[test]
fn test_tier5_empty_and_whitespace_module_parsing() {
    // Completely empty string
    let mod_empty = parse_ssa("").expect("empty SSA should produce empty module");
    assert_eq!(mod_empty.functions.len(), 0);

    // Whitespace only
    let mod_ws = parse_ssa("   \n\t  \n  ").expect("whitespace SSA should produce empty module");
    assert_eq!(mod_ws.functions.len(), 0);

    // Comment only (# and //)
    let mod_comments = parse_ssa("# Comment line 1\n// Comment line 2\n   # Indented comment\n")
        .expect("comments SSA should produce empty module");
    assert_eq!(mod_comments.functions.len(), 0);

    // parse_function on empty input should fail with UnexpectedEof
    let err = parse_function("").unwrap_err();
    assert!(matches!(err, ParseError::UnexpectedEof { .. }));
}

#[test]
fn test_tier5_fn_header_whitespace_and_trailing_commas() {
    let ssa = "fn strange_spacing(  \t  %0: i64 , \t %1: bool ,  ) -> void {
bb0:
    ret
}";
    let func = parse_function(ssa).expect("should accept flexible whitespace and trailing comma");
    assert_eq!(func.name, "strange_spacing");
    assert_eq!(func.params.len(), 2);
    assert_eq!(func.params[0], (0, Type::I64));
    assert_eq!(func.params[1], (1, Type::Bool));
    assert_eq!(func.ret_ty, Type::Void);
}

#[test]
fn test_tier5_effects_parsing_edge_cases() {
    // All 4 effects with whitespace and arbitrary order
    let ssa = "fn eff_func() -> void effects(  Net ,  Async , Ui , Hw  ) {
bb0:
    ret
}";
    let func = parse_function(ssa).expect("should parse all 4 effects with spaces");
    assert_eq!(func.effects.len(), 4);
    assert!(func.effects.contains(&Effect::Net));
    assert!(func.effects.contains(&Effect::Async));
    assert!(func.effects.contains(&Effect::Ui));
    assert!(func.effects.contains(&Effect::Hw));

    // Duplicate effects should be deduplicated
    let ssa_dup = "fn eff_dup() -> void effects(Net, Net, Ui, Net) {
bb0:
    ret
}";
    let func_dup = parse_function(ssa_dup).expect("should parse duplicate effects without error");
    assert_eq!(func_dup.effects.len(), 2);

    // Invalid effect keyword
    let ssa_inv = "fn eff_inv() -> void effects(Database) {
bb0:
    ret
}";
    let err = parse_function(ssa_inv).unwrap_err();
    assert!(matches!(err, ParseError::InvalidEffect { .. }));

    // Missing opening paren in effects
    let ssa_no_open = "fn eff_bad() -> void effects Net) {
bb0:
    ret
}";
    let err_no_open = parse_function(ssa_no_open).unwrap_err();
    assert!(matches!(err_no_open, ParseError::InvalidSyntax { .. }));

    // Missing closing paren in effects
    let ssa_no_close = "fn eff_bad() -> void effects(Net {
bb0:
    ret
}";
    let err_no_close = parse_function(ssa_no_close).unwrap_err();
    assert!(matches!(err_no_close, ParseError::InvalidSyntax { .. }));
}

#[test]
fn test_tier5_malformed_function_headers() {
    // Missing '->'
    let ssa1 = "fn foo() void { bb0: ret }";
    assert!(matches!(
        parse_function(ssa1).unwrap_err(),
        ParseError::InvalidSyntax { .. }
    ));

    // Missing opening '('
    let ssa2 = "fn foo %0: i64) -> void { bb0: ret }";
    assert!(matches!(
        parse_function(ssa2).unwrap_err(),
        ParseError::InvalidSyntax { .. }
    ));

    // Missing closing ')'
    let ssa3 = "fn foo(%0: i64 -> void { bb0: ret }";
    assert!(matches!(
        parse_function(ssa3).unwrap_err(),
        ParseError::InvalidSyntax { .. }
    ));

    // Missing trailing '{'
    let ssa4 = "fn foo() -> void bb0: ret }";
    assert!(matches!(
        parse_function(ssa4).unwrap_err(),
        ParseError::InvalidSyntax { .. }
    ));

    // Non-function line at top-level
    let ssa5 = "invalid_statement\nfn foo() -> void { bb0: ret }";
    assert!(matches!(
        parse_ssa(ssa5).unwrap_err(),
        ParseError::InvalidSyntax { .. }
    ));
}

#[test]
fn test_tier5_deeply_nested_types_and_bracket_mismatches() {
    // 16 levels of nested vec
    let deeply_nested = "vec[vec[vec[vec[vec[vec[vec[vec[vec[vec[vec[vec[vec[vec[vec[vec[i64]]]]]]]]]]]]]]]]";
    let ty = parse_full_type(deeply_nested, 1).expect("16 levels of vec should parse");
    assert!(matches!(ty, Type::Vec(_)));

    // Deeply nested mixed containers
    let mixed = "map[str, vec[chan[map[str, buf]]]]";
    let ty_mixed = parse_full_type(mixed, 1).expect("nested map/vec/chan/buf should parse");
    assert!(matches!(ty_mixed, Type::Map(_, _)));

    // Bracket mismatch: vec[i64)
    assert!(matches!(
        parse_full_type("vec[i64)", 1).unwrap_err(),
        ParseError::InvalidType { .. }
    ));

    // Bracket mismatch: map[str, i64>
    assert!(matches!(
        parse_full_type("map[str, i64>", 1).unwrap_err(),
        ParseError::InvalidType { .. }
    ));

    // Bracket mismatch: chan[i64}
    assert!(matches!(
        parse_full_type("chan[i64}", 1).unwrap_err(),
        ParseError::InvalidType { .. }
    ));

    // Incomplete map: map[str]
    assert!(matches!(
        parse_full_type("map[str]", 1).unwrap_err(),
        ParseError::InvalidType { .. }
    ));

    // Type with trailing tokens: i64 extra_token
    assert!(matches!(
        parse_full_type("i64 extra_token", 1).unwrap_err(),
        ParseError::InvalidType { .. }
    ));
}

#[test]
fn test_tier5_strings_and_escapes_edge_cases() {
    // String containing escaped quotes, newline, tabs, carriage return, and null byte
    let ssa = r#"fn str_test() -> str {
bb0:
    %0 = const_str "a \" b \t c \n d \r e \0 f"
    ret %0
}"#;
    let func = parse_function(ssa).expect("escaped characters should parse");
    if let Op::ConstStr(ref s) = func.blocks[0].instructions[0].op {
        assert_eq!(s, "a \" b \t c \n d \r e \0 f");
    } else {
        panic!("expected ConstStr");
    }

    // String containing # and // inside the quotes (should not be stripped as comments)
    let ssa2 = r#"fn str_comments() -> str {
bb0:
    %0 = const_str "foo # not a comment // also not"
    ret %0
}"#;
    let func2 = parse_function(ssa2).expect("quotes containing comment chars should parse intact");
    if let Op::ConstStr(ref s) = func2.blocks[0].instructions[0].op {
        assert_eq!(s, "foo # not a comment // also not");
    } else {
        panic!("expected ConstStr");
    }

    // Multibyte unicode string
    let ssa_unicode = r#"fn str_utf8() -> str {
bb0:
    %0 = const_str "こんにちは 🚀 𠮷野家 \n newline"
    ret %0
}"#;
    let func_u = parse_function(ssa_unicode).expect("unicode UTF-8 string should parse");
    if let Op::ConstStr(ref s) = func_u.blocks[0].instructions[0].op {
        assert_eq!(s, "こんにちは 🚀 𠮷野家 \n newline");
    } else {
        panic!("expected ConstStr");
    }
}

#[test]
fn test_tier5_all_terminators_edge_cases() {
    // Ret with invalid register syntax
    let ssa_bad_ret = "fn test() -> void {\nbb0:\n    ret %bad\n}";
    assert!(matches!(
        parse_function(ssa_bad_ret).unwrap_err(),
        ParseError::InvalidRegister { .. }
    ));

    // Ret with extra token
    let ssa_ret_extra = "fn test() -> void {\nbb0:\n    ret %0 %1\n}";
    assert!(matches!(
        parse_function(ssa_ret_extra).unwrap_err(),
        ParseError::InvalidSyntax { .. }
    ));

    // Br without arguments (trimmed to "br", treated as unknown opcode because terminators expect args)
    let ssa_br_missing = "fn test() -> void {\nbb0:\n    br\n}";
    let err_br_missing = parse_function(ssa_br_missing).unwrap_err();
    assert!(matches!(
        err_br_missing,
        ParseError::UnknownOpcode { .. } | ParseError::InvalidSyntax { .. }
    ));

    // Br with register instead of block label
    let ssa_br_reg = "fn test(%0: i64) -> void {\nbb0:\n    br %0\n}";
    let err_br_reg = parse_function(ssa_br_reg).unwrap_err();
    assert!(matches!(err_br_reg, ParseError::InvalidBlockLabel { .. }));

    // Br with extra target
    let ssa_br_extra = "fn test() -> void {\nbb0:\n    br bb1 bb2\n}";
    assert!(matches!(
        parse_function(ssa_br_extra).unwrap_err(),
        ParseError::InvalidSyntax { .. }
    ));

    // BrIf with missing else block
    let ssa_brif_missing = "fn test(%0: bool) -> void {\nbb0:\n    br_if %0 bb1\n}";
    assert!(matches!(
        parse_function(ssa_brif_missing).unwrap_err(),
        ParseError::InvalidSyntax { .. }
    ));

    // BrIf with extra args
    let ssa_brif_extra = "fn test(%0: bool) -> void {\nbb0:\n    br_if %0 bb1 bb2 bb3\n}";
    assert!(matches!(
        parse_function(ssa_brif_extra).unwrap_err(),
        ParseError::InvalidSyntax { .. }
    ));
}

#[test]
fn test_tier5_opcodes_operand_count_validation() {
    // free requires 1 register
    let ssa_free_0 = "fn test() -> void {\nbb0:\n    free\n    ret\n}";
    assert!(matches!(
        parse_function(ssa_free_0).unwrap_err(),
        ParseError::InvalidSyntax { .. }
    ));

    let ssa_free_2 = "fn test(%0: i64, %1: i64) -> void {\nbb0:\n    free %0 %1\n    ret\n}";
    assert!(matches!(
        parse_function(ssa_free_2).unwrap_err(),
        ParseError::InvalidSyntax { .. }
    ));

    // add requires 2 registers
    let ssa_add_1 = "fn test(%0: i64) -> i64 {\nbb0:\n    %1 = add %0\n    ret %1\n}";
    assert!(matches!(
        parse_function(ssa_add_1).unwrap_err(),
        ParseError::InvalidSyntax { .. }
    ));

    let ssa_add_3 = "fn test(%0: i64, %1: i64) -> i64 {\nbb0:\n    %2 = add %0 %1 %0\n    ret %2\n}";
    assert!(matches!(
        parse_function(ssa_add_3).unwrap_err(),
        ParseError::InvalidSyntax { .. }
    ));

    // buf_write_u8 requires 3 registers
    let ssa_bufw_2 = "fn test(%0: buf, %1: i64) -> buf {\nbb0:\n    %2 = buf_write_u8 %0 %1\n    ret %2\n}";
    assert!(matches!(
        parse_function(ssa_bufw_2).unwrap_err(),
        ParseError::InvalidSyntax { .. }
    ));
}

#[test]
fn test_tier5_binary_corrupted_header_and_versions() {
    let mut module = Module::new();
    let func = parse_function("fn f() -> void {\nbb0:\n    ret\n}").unwrap();
    module.add_function(func);
    let bytes = encode_knrbin(&module).unwrap();

    // Corrupt magic bytes
    let mut bad_magic = bytes.clone();
    bad_magic[0] = b'X';
    let err_magic = decode_knrbin(&bad_magic).unwrap_err();
    assert!(matches!(err_magic, BinaryError::BadMagic(_)));

    // Corrupt version
    let mut bad_ver = bytes.clone();
    bad_ver[4] = 99; // Version 99
    bad_ver[5] = 0;
    let err_ver = decode_knrbin(&bad_ver).unwrap_err();
    assert!(matches!(err_ver, BinaryError::UnsupportedVersion(99)));

    // Truncated binary at all small lengths (0 to 12 bytes)
    for len in 0..12.min(bytes.len()) {
        let truncated = &bytes[..len];
        let err = decode_knrbin(truncated).unwrap_err();
        assert!(matches!(err, BinaryError::UnexpectedEof { .. } | BinaryError::BadMagic(_)));
    }
}

#[test]
fn test_tier5_binary_floats_and_integers_preservation() {
    // Tests preservation of IEEE 754 bit representations:
    // +0.0 vs -0.0, NaN, Inf, -Inf, subnormals, i64::MIN, i64::MAX
    let ssa = r#"fn float_extremes() -> void {
bb0:
    %0 = const_f64 0.0
    %1 = const_f64 -0.0
    %2 = const_f64 inf
    %3 = const_f64 -inf
    %4 = const_f64 NaN
    %5 = const_i64 -9223372036854775808
    %6 = const_i64 9223372036854775807
    ret
}"#;
    let func = parse_function(ssa).unwrap();
    let mut module = Module::new();
    module.add_function(func);

    let bin = encode_knrbin(&module).unwrap();
    let decoded = decode_knrbin(&bin).unwrap();
    assert_eq!(decoded.functions.len(), 1);

    let dec_f = &decoded.functions[0];
    let insts = &dec_f.blocks[0].instructions;

    // +0.0
    if let Op::ConstF64(v) = insts[0].op {
        assert_eq!(v.to_bits(), 0.0f64.to_bits());
    } else { panic!(); }

    // -0.0
    if let Op::ConstF64(v) = insts[1].op {
        assert_eq!(v.to_bits(), (-0.0f64).to_bits());
    } else { panic!(); }

    // +inf
    if let Op::ConstF64(v) = insts[2].op {
        assert!(v.is_infinite() && v.is_sign_positive());
    } else { panic!(); }

    // -inf
    if let Op::ConstF64(v) = insts[3].op {
        assert!(v.is_infinite() && v.is_sign_negative());
    } else { panic!(); }

    // NaN
    if let Op::ConstF64(v) = insts[4].op {
        assert!(v.is_nan());
    } else { panic!(); }

    // i64::MIN
    if let Op::ConstI64(v) = insts[5].op {
        assert_eq!(v, i64::MIN);
    } else { panic!(); }

    // i64::MAX
    if let Op::ConstI64(v) = insts[6].op {
        assert_eq!(v, i64::MAX);
    } else { panic!(); }
}

#[test]
fn test_tier5_roundtrip_bijection_idempotency() {
    let ssa = r#"fn complex_roundtrip(%0: buf, %1: i64) -> buf effects(Net, Ui) {
bb0:
    %2 = const_i64 100
    %3 = add %1 %2
    %4 = const_str "packet-header"
    %5 = cmp_lt %1 %2
    br_if %5 bb1 bb2
bb1:
    %6 = const_i64 0
    %7 = const_i64 42
    %8 = buf_write_u8 %0 %6 %7
    ret %8
bb2:
    %9 = const_i64 0
    %10 = const_i64 10
    %11 = buf_slice %0 %9 %10
    ret %11
}
"#;
    let func1 = parse_function(ssa).unwrap();
    let text1 = format_function(&func1);

    let func2 = parse_function(&text1).unwrap();
    let text2 = format_function(&func2);
    assert_eq!(text1, text2, "textual format must be strictly idempotent");

    let mut mod1 = Module::new();
    mod1.add_function(func1);
    let bin = encode_knrbin(&mod1).unwrap();

    let mod_dec = decode_knrbin(&bin).unwrap();
    let text_from_bin = format_function(&mod_dec.functions[0]);
    assert_eq!(text1, text_from_bin, "binary roundtrip must match canonical formatted text exactly");
}
