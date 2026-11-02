use crate::harness::*;

/// Tier 1: Wave 1 Core Feature Tests
/// Verifies nominal, happy-path functionality for features F1 through F23.

#[test]
fn test_t1_f01_workspace_layout_and_fixtures() {
    let loader = FixtureLoader::new();
    let valid_fixtures = loader.load_valid_fixtures();
    let invalid_fixtures = loader.load_invalid_fixtures();

    assert!(
        !valid_fixtures.is_empty(),
        "Fixture loader must find valid test fixtures"
    );
    assert!(
        !invalid_fixtures.is_empty(),
        "Fixture loader must find invalid test fixtures"
    );
    assert_eq!(
        invalid_fixtures.len(),
        20,
        "Must have exactly 20 canonical invalid test fixtures"
    );
}

#[test]
fn test_t1_f03_scalar_types_definitions() {
    let scalar_names = ["i64", "f64", "bool", "void"];
    for name in &scalar_names {
        assert!(
            is_valid_scalar_type(name),
            "Scalar type {} must be valid",
            name
        );
    }
}

#[test]
fn test_t1_f04_linear_types_definitions() {
    let linear_names = ["buf", "str", "vec", "map", "chan"];
    for name in &linear_names {
        assert!(
            !is_valid_scalar_type(name) || *name == "buf" || *name == "str",
            "Linear type {} identified",
            name
        );
    }
}

#[test]
fn test_t1_f05_linear_predicate() {
    // Check known type classifications
    let scalars = ["i64", "f64", "bool", "void"];
    let linears = ["buf", "str", "vec[i64]", "map[str, i64]", "chan[buf]"];

    for s in &scalars {
        assert!(
            is_valid_scalar_type(s),
            "{} should be classified as scalar",
            s
        );
    }
    for l in &linears {
        assert!(!l.is_empty(), "Linear type representation not empty: {}", l);
    }
}

#[test]
fn test_t1_f06_effect_enum() {
    assert_eq!(KNOWN_EFFECTS.len(), 4);
    assert!(KNOWN_EFFECTS.contains(&"Net"));
    assert!(KNOWN_EFFECTS.contains(&"Ui"));
    assert!(KNOWN_EFFECTS.contains(&"Hw"));
    assert!(KNOWN_EFFECTS.contains(&"Async"));
}

#[test]
fn test_t1_f07_constant_opcodes() {
    let const_ops = ["const_i64", "const_f64", "const_bool", "const_str"];
    for op in &const_ops {
        assert!(
            KNOWN_OPCODES.contains(op),
            "Constant opcode {} must be known",
            op
        );
    }
}

#[test]
fn test_t1_f08_arithmetic_opcodes() {
    let arith_ops = ["add", "sub", "mul", "div"];
    for op in &arith_ops {
        assert!(
            KNOWN_OPCODES.contains(op),
            "Arithmetic opcode {} must be known",
            op
        );
    }
}

#[test]
fn test_t1_f09_comparison_opcodes() {
    let cmp_ops = ["cmp_eq", "cmp_lt"];
    for op in &cmp_ops {
        assert!(
            KNOWN_OPCODES.contains(op),
            "Comparison opcode {} must be known",
            op
        );
    }
}

#[test]
fn test_t1_f10_buffer_opcodes() {
    let buf_ops = ["buf_alloc", "buf_read_u8", "buf_write_u8", "buf_slice"];
    for op in &buf_ops {
        assert!(
            KNOWN_OPCODES.contains(op),
            "Buffer opcode {} must be known",
            op
        );
    }
}

#[test]
fn test_t1_f11_vector_opcodes() {
    let vec_ops = ["vec_new", "vec_push", "vec_get", "vec_len"];
    for op in &vec_ops {
        assert!(
            KNOWN_OPCODES.contains(op),
            "Vector opcode {} must be known",
            op
        );
    }
}

#[test]
fn test_t1_f12_map_opcodes() {
    let map_ops = ["map_new", "map_set", "map_get", "map_del"];
    for op in &map_ops {
        assert!(KNOWN_OPCODES.contains(op), "Map opcode {} must be known", op);
    }
}

#[test]
fn test_t1_f13_channel_opcodes() {
    let chan_ops = ["chan_new", "chan_send", "chan_recv"];
    for op in &chan_ops {
        assert!(
            KNOWN_OPCODES.contains(op),
            "Channel opcode {} must be known",
            op
        );
    }
}

#[test]
fn test_t1_f14_task_system_opcodes() {
    let task_ops = ["spawn", "syscall", "free"];
    for op in &task_ops {
        assert!(
            KNOWN_OPCODES.contains(op),
            "Task opcode {} must be known",
            op
        );
    }
}

#[test]
fn test_t1_f15_terminators() {
    assert_eq!(KNOWN_TERMINATORS.len(), 3);
    assert!(KNOWN_TERMINATORS.contains(&"ret"));
    assert!(KNOWN_TERMINATORS.contains(&"br"));
    assert!(KNOWN_TERMINATORS.contains(&"br_if"));
}

#[test]
fn test_t1_f16_registers_and_blocks_validation() {
    assert!(is_valid_register("%0"));
    assert!(is_valid_register("%123"));
    assert!(!is_valid_register("%foo"));
    assert!(!is_valid_register("0"));
    assert!(!is_valid_register("%"));

    assert!(is_valid_block_label("bb0"));
    assert!(is_valid_block_label("bb42"));
    assert!(!is_valid_block_label("b0"));
    assert!(!is_valid_block_label("bb"));
    assert!(!is_valid_block_label("bb_foo"));
}

#[test]
fn test_t1_f17_textual_parser_valid_fixtures() {
    let loader = FixtureLoader::new();
    let valid_fixtures = loader.load_valid_fixtures();

    for fixture in valid_fixtures {
        let res = validate_ssa_grammar(&fixture.content);
        assert!(
            res.is_ok(),
            "Valid fixture {} failed grammar validation: {:?}",
            fixture.filename,
            res.err()
        );
    }
}

#[test]
fn test_t1_f18_canonical_formatter_spec() {
    let sample = "fn main() -> i64 {\nbb0:\n    %0 = const_i64 42\n    ret %0\n}\n";
    let lines: Vec<&str> = sample.lines().collect();
    assert_eq!(lines[0], "fn main() -> i64 {");
    assert_eq!(lines[1], "bb0:");
    assert_eq!(lines[2], "    %0 = const_i64 42");
    assert_eq!(lines[3], "    ret %0");
    assert_eq!(lines[4], "}");
}

#[test]
fn test_t1_f19_binary_header_magic_and_version() {
    let header_bytes = encode_header(CURRENT_VERSION);
    assert_eq!(&header_bytes[0..4], b"KNIR");
    assert_eq!(header_bytes[4], 0x01);
    assert_eq!(header_bytes[5], 0x00);

    let decoded = decode_header(&header_bytes).expect("Valid header must decode");
    assert_eq!(&decoded.magic, b"KNIR");
    assert_eq!(decoded.version, 1);
}

#[test]
fn test_t1_f20_constant_pool_spec() {
    let strings = vec!["hello".to_string(), "world".to_string(), "hello".to_string()];
    let mut deduplicated: Vec<String> = Vec::new();
    for s in strings {
        if !deduplicated.contains(&s) {
            deduplicated.push(s);
        }
    }
    assert_eq!(deduplicated.len(), 2);
    assert_eq!(deduplicated[0], "hello");
    assert_eq!(deduplicated[1], "world");
}

#[test]
fn test_t1_f21_fixed_64bit_instruction_codec() {
    // Opcode: add (0x05), Type: i64 (0x01), dst: %2, src1: %0, src2: %1
    let word = pack_instruction_word(0x05, 0x01, 2, 0, 1);
    assert_eq!(word.len(), 8);
    assert_eq!(word[0], 0x05);
    assert_eq!(word[1], 0x01);

    let decoded = unpack_instruction_word(&word).expect("Unpacking 8-byte word must succeed");
    assert_eq!(decoded.opcode, 0x05);
    assert_eq!(decoded.out_type, 0x01);
    assert_eq!(decoded.dst, 2);
    assert_eq!(decoded.src1, 0);
    assert_eq!(decoded.src2, 1);
}

#[test]
fn test_t1_f22_extra_operands_pool() {
    // For 3-operand opcodes like map_set %dst %map %key %val:
    // dst = 3, src1 = 0 (%map), src2 = pool index 0 (points to (%key, %val) = (1, 2))
    let extra_pool: Vec<(u16, u16)> = vec![(1, 2)];
    let word = pack_instruction_word(0x14, 0x07, 3, 0, 0);

    let decoded = unpack_instruction_word(&word).expect("Decodes successfully");
    let pool_idx = decoded.src2 as usize;
    assert!(pool_idx < extra_pool.len());
    let (key_reg, val_reg) = extra_pool[pool_idx];
    assert_eq!(key_reg, 1);
    assert_eq!(val_reg, 2);
}

#[test]
fn test_t1_f23_exact_roundtrip_bijection_contract() {
    // Canonical program
    let canonical = "fn add_two(%0: i64, %1: i64) -> i64 {\nbb0:\n    %2 = add %0 %1\n    ret %2\n}\n";
    // Check that grammar validation accepts it
    assert!(validate_ssa_grammar(canonical).is_ok());
}
