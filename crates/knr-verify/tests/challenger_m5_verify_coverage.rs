//! Milestone 5 Phase 2: Adversarial Coverage Hardening Suite for knr-verify.
//! Targets:
//! 5. `crates/knr-verify/src/program.rs` (Source map diagnostics, linear verification, dominance verification)

use knr_core::parse_knr;
use knr_verify::verify_program;

#[test]
fn test_adv_verify_source_map_lowering_diagnostic_enrichment() {
    let src = r#"
schema Item {
    0: id i64,
}

fn consume(it: Item) -> i64 {
    return it.id;
}

fn trigger_uam() -> i64 {
    let my_item = Item { id: 42 };
    let a = consume(my_item);
    let b = consume(my_item);
    return a + b;
}
"#;
    let prog = parse_knr(src).expect("parse uam");
    let res = verify_program(&prog);
    assert!(res.is_err(), "Use after move must fail verify_program");

    let diags = res.unwrap_err();
    assert!(!diags.is_empty());
    let uam_diag = diags
        .iter()
        .find(|d| d.error_kind == "UseAfterMove")
        .expect("UseAfterMove diagnostic expected");

    // Lowering diagnostics carry source variable name and line number
    assert_eq!(uam_diag.line, 13);
    assert_eq!(uam_diag.source_variable.as_deref(), Some("my_item"));
    assert!(
        uam_diag.fix_hint.as_ref().unwrap().contains("my_item"),
        "Fix hint must reference the variable name"
    );
}

#[test]
fn test_adv_verify_source_map_verification_pass_enrichment() {
    // Construct a program where lowering succeeds, but knr-verify detects an unmanaged linear leak.
    // Container holds an unmanaged `buf`, so knr-core::lower does not auto-drop it.
    // knr-verify detects the LinearLeak and verify_program enriches the diagnostic
    // with source_variable, column, and span from the SourceMap!
    let src = r#"
schema Container {
    0: b buf,
    1: id i64,
}

fn leak_container(raw_buf: buf) -> i64 {
    let leaked_box = Container { b: raw_buf, id: 42 };
    return 0;
}
"#;
    let prog = parse_knr(src).expect("parse leak");
    let res = verify_program(&prog);
    assert!(res.is_err(), "Unmanaged linear container leak must fail verify_program");

    let diags = res.unwrap_err();
    let leak_diag = diags
        .iter()
        .find(|d| d.error_kind == "LinearLeak")
        .expect("LinearLeak diagnostic expected");

    assert_eq!(leak_diag.function, "leak_container");
    assert_eq!(leak_diag.source_variable.as_deref(), Some("leaked_box"));
    assert!(leak_diag.column.is_some(), "Column must be enriched from SourceMap");
    assert!(leak_diag.span.is_some(), "Span must be enriched from SourceMap");
    let (start, end) = leak_diag.span.unwrap();
    assert!(start < end, "Byte span start must precede end");
}

#[test]
fn test_adv_verify_schema_recursive_dependency_rejected() {
    let src = r#"
schema Alpha {
    0: b struct[Beta],
}

schema Beta {
    0: a struct[Alpha],
}

fn dummy() -> void {}
"#;
    let prog = parse_knr(src).expect("parse cyclic schemas");
    let res = verify_program(&prog);
    assert!(res.is_err(), "Recursive schema definitions must be rejected");

    let diags = res.unwrap_err();
    assert!(
        diags.iter().any(|d| d.error_kind == "RecursiveSchema"),
        "Expected RecursiveSchema error kind, got: {:?}",
        diags
    );
}

#[test]
fn test_adv_verify_schema_duplicate_name_rejected() {
    let src = r#"
schema DupName {
    0: x i64,
}

schema DupName {
    0: y i64,
}

fn dummy() -> void {}
"#;
    let prog = parse_knr(src).expect("parse duplicate schemas");
    let res = verify_program(&prog);
    assert!(res.is_err(), "Duplicate schema names must be rejected");

    let diags = res.unwrap_err();
    assert!(
        diags.iter().any(|d| d.error_kind == "DuplicateSchema"),
        "Expected DuplicateSchema error kind, got: {:?}",
        diags
    );
}

#[test]
fn test_adv_verify_schema_duplicate_field_index_rejected() {
    let src = r#"
schema ConflictFields {
    0: first i64,
    0: second i64,
}

fn dummy() -> void {}
"#;
    let prog = parse_knr(src).expect("parse conflict fields");
    let res = verify_program(&prog);
    assert!(res.is_err(), "Duplicate field indices must be rejected");

    let diags = res.unwrap_err();
    assert!(
        diags.iter().any(|d| d.error_kind == "DuplicateField"),
        "Expected DuplicateField error kind, got: {:?}",
        diags
    );
}

#[test]
fn test_adv_verify_complex_diamond_linear_join_clean() {
    let src = r#"
schema Card {
    0: id i64,
    1: tag str,
}

fn modify_card(c: Card, flag: bool) -> Card {
    if flag {
        let updated = Card { id: c.id + 1, tag: "then" };
        return updated;
    } else {
        let updated = Card { id: c.id + 2, tag: "else" };
        return updated;
    }
}
"#;
    let prog = parse_knr(src).expect("parse diamond join");
    let res = verify_program(&prog);
    assert!(
        res.is_ok(),
        "Valid diamond CFG linear joins must pass static verification: {:?}",
        res.err()
    );
}

#[test]
fn test_adv_verify_struct_borrow_does_not_consume_linear() {
    let src = r#"
schema Node {
    0: key str,
    1: val i64,
}

fn read_node(n: Node) -> i64 {
    let k = n.key;
    let v = n.val;
    return v;
}
"#;
    let prog = parse_knr(src).expect("parse node borrow");
    let res = verify_program(&prog);
    assert!(
        res.is_ok(),
        "Field reads from linear struct must not consume the struct: {:?}",
        res.err()
    );
}

#[test]
fn test_adv_verify_dominance_across_nested_branches() {
    let src = r#"
schema Item { 0: v i64, }

fn complex_control(a: i64, b: i64) -> i64 {
    let it = Item { v: 10 };
    if a > 0 {
        if b > 0 {
            return it.v + 1;
        } else {
            return it.v + 2;
        }
    } else {
        return it.v + 3;
    }
}
"#;
    let prog = parse_knr(src).expect("parse complex control");
    let res = verify_program(&prog);
    assert!(
        res.is_ok(),
        "Properly dominated nested branches must pass verification: {:?}",
        res.err()
    );
}
