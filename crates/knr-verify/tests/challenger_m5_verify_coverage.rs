//! Milestone 5 Phase 2: Adversarial Coverage Hardening Suite for knr-verify.
//! Targets:
//! 5. `crates/knr-verify/src/program.rs` (Source map diagnostics, linear verification, dominance verification)

use knr_core::parse_knr;
use knr_verify::verify_program;

#[test]
fn test_adv_verify_source_map_lowering_diagnostic_enrichment() {
    let src = r#"
type Item = {
    id: i64,
};

function consume(it: Item): i64 {
    return it.id;
}

function trigger_uam(): i64 {
    const my_item: Item = { id: 42 };
    const a = consume(my_item);
    const b = consume(my_item);
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
    // Container holds an unmanaged `Buffer`, so knr-core::lower does not auto-drop it.
    // knr-verify detects the LinearLeak and verify_program enriches the diagnostic
    // with source_variable, column, and span from the SourceMap!
    let src = r#"
type Container = {
    b: Buffer,
    id: i64,
};

function leak_container(raw_buf: Buffer): i64 {
    const leaked_box: Container = { b: raw_buf, id: 42 };
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
type Alpha = {
    b: Beta,
};

type Beta = {
    a: Alpha,
};

function dummy(): void {}
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
type DupName = {
    x: i64,
};

type DupName = {
    y: i64,
};

function dummy(): void {}
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
type ConflictFields = {
    first: i64,
    first: i64,
};

function dummy(): void {}
"#;
    let prog = parse_knr(src).expect("parse conflict fields");
    let res = verify_program(&prog);
    assert!(res.is_err(), "Duplicate fields must be rejected");

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
type Card = {
    id: i64,
    tag: string,
};

function modify_card(c: Card, flag: bool): Card {
    if (flag) {
        const updated: Card = { id: c.id + 1, tag: "then" };
        return updated;
    } else {
        const updated: Card = { id: c.id + 2, tag: "else" };
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
type Node = {
    key: string,
    val: i64,
};

function read_node(n: Node): i64 {
    const k = n.key;
    const v = n.val;
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
type Item = { v: i64 };

function complex_control(a: i64, b: i64): i64 {
    const it: Item = { v: 10 };
    if (a > 0) {
        if (b > 0) {
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
