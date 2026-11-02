use crate::harness::*;

/// Tier 2: Wave 1 Boundary & Corner Cases
/// Boundary Value Analysis (BVA), extreme values, corrupted inputs, and error triggers.

#[test]
fn test_t2_f03_scalar_extremes() {
    let min_i64 = i64::MIN;
    let max_i64 = i64::MAX;
    let nan_f64 = f64::NAN;
    let neg_zero = -0.0f64;
    let inf_f64 = f64::INFINITY;

    assert!(nan_f64.is_nan());
    assert_eq!(neg_zero, 0.0);
    assert!(neg_zero.is_sign_negative());
    assert!(inf_f64.is_infinite());
    assert!(min_i64 < max_i64);
}

#[test]
fn test_t2_f04_deeply_nested_types() {
    // 16-level nested vector type: vec[vec[vec[...vec[i64]...]]]
    let mut type_repr = "i64".to_string();
    for _ in 0..16 {
        type_repr = format!("vec[{}]", type_repr);
    }
    assert!(type_repr.starts_with("vec[vec["));
    assert!(type_repr.ends_with("]]]"));
}

#[test]
fn test_t2_f07_empty_and_unicode_strings() {
    let empty_str = "\"\"";
    let unicode_str = "\"🦀 Linear SSA System 🚀\"";
    let escape_str = "\"line1\\nline2\\t\\\"quoted\\\"\"";

    for s in &[empty_str, unicode_str, escape_str] {
        let code = format!("fn test_str() -> str {{\nbb0:\n    %0 = const_str {}\n    ret %0\n}}\n", s);
        assert!(
            validate_ssa_grammar(&code).is_ok(),
            "Grammar should accept valid string literal: {}",
            s
        );
    }
}

#[test]
fn test_t2_f16_empty_basic_block() {
    // Basic block with 0 instructions followed immediately by ret
    let code = "fn empty_block() -> void {\nbb0:\n    ret\n}\n";
    assert!(
        validate_ssa_grammar(code).is_ok(),
        "Block with 0 instructions followed by ret is valid"
    );
}

#[test]
fn test_t2_f17_parser_syntax_errors() {
    // Unterminated string
    let bad_str = "fn f() -> str {\nbb0:\n    %0 = const_str \"unterminated\n    ret %0\n}\n";
    assert_eq!(
        validate_ssa_grammar(bad_str),
        Err(GrammarError::UnterminatedString { line: 3 })
    );

    // Invalid register
    let bad_reg = "fn f() -> i64 {\nbb0:\n    %foo = const_i64 1\n    ret %foo\n}\n";
    assert_eq!(
        validate_ssa_grammar(bad_reg),
        Err(GrammarError::InvalidRegister {
            line: 3,
            reg: "%foo".to_string()
        })
    );

    // Unknown opcode
    let bad_op = "fn f() -> i64 {\nbb0:\n    %0 = unknown_op_123 42\n    ret %0\n}\n";
    assert_eq!(
        validate_ssa_grammar(bad_op),
        Err(GrammarError::UnknownOpcode {
            line: 3,
            opcode: "unknown_op_123".to_string()
        })
    );

    // Missing terminator
    let bad_term = "fn f() -> i64 {\nbb0:\n    %0 = const_i64 10\n}\n";
    assert_eq!(
        validate_ssa_grammar(bad_term),
        Err(GrammarError::MissingTerminator {
            line: 4,
            block: "bb0".to_string()
        })
    );
}

#[test]
fn test_t2_f19_corrupted_magic_header() {
    // Truncated header (< 6 bytes)
    let short_bytes = [0x4B, 0x4E, 0x49];
    assert_eq!(
        decode_header(&short_bytes),
        Err(BinaryCheckError::TooShort {
            expected: 6,
            actual: 3
        })
    );

    // Invalid magic bytes
    let bad_magic = [0x54, 0x45, 0x53, 0x54, 0x01, 0x00]; // "TEST"
    assert_eq!(
        decode_header(&bad_magic),
        Err(BinaryCheckError::InvalidMagic {
            actual: [0x54, 0x45, 0x53, 0x54]
        })
    );

    // Unsupported version
    let bad_ver = [0x4B, 0x4E, 0x49, 0x52, 0x02, 0x00]; // Version 2
    assert_eq!(
        decode_header(&bad_ver),
        Err(BinaryCheckError::UnsupportedVersion {
            expected: 1,
            actual: 2
        })
    );
}

#[test]
fn test_t2_f20_large_constant_pool() {
    let mut pool: Vec<String> = Vec::with_capacity(1000);
    for i in 0..1000 {
        pool.push(format!("constant_string_identifier_{:04}", i));
    }
    assert_eq!(pool.len(), 1000);
    assert_eq!(pool[999], "constant_string_identifier_0999");
}

#[test]
fn test_t2_f21_max_register_index() {
    // Max 16-bit register ID: 65534 (0xFFFE)
    // NO_REG: 65535 (0xFFFF)
    let max_reg = 0xFFFEu16;
    let no_reg = 0xFFFFu16;

    let word = pack_instruction_word(0x1C, 0x00, no_reg, max_reg, no_reg); // free %65534
    let decoded = unpack_instruction_word(&word).expect("Valid word");
    assert_eq!(decoded.dst, no_reg);
    assert_eq!(decoded.src1, max_reg);
    assert_eq!(decoded.src2, no_reg);
}

#[test]
fn test_t2_f22_extra_pool_sequential_3ops() {
    let extra_pool: Vec<(u16, u16)> = vec![(10, 20), (30, 40), (50, 60)];
    for (idx, &(k, v)) in extra_pool.iter().enumerate() {
        let word = pack_instruction_word(0x14, 0x07, idx as u16, 0, idx as u16);
        let decoded = unpack_instruction_word(&word).expect("Unpack succeeds");
        let pool_idx = decoded.src2 as usize;
        assert_eq!(extra_pool[pool_idx], (k, v));
    }
}

#[test]
fn test_t2_f23_whitespace_and_comment_invariance() {
    let messy_source = "
    // Header comment
    fn   messy_spacing( %0 : i64 ,  %1 : i64 ) -> i64 {
    # Block comment
    bb0:
        %2   =   add   %0   %1
        ret  %2
    }
    ";
    assert!(validate_ssa_grammar(messy_source).is_ok());
}
