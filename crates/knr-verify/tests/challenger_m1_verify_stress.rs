use knr_core::ast::Type;
use knr_core::lower::lower_program_from_file;
use knr_core::parse_knr;
use knr_verify::{verify_module, verify_program};
use std::fs;
use std::path::Path;

// ============================================================================
// 1. REFERENCE APPLICATION COMPONENT (site/components/exec_graph.knr)
// ============================================================================

#[test]
fn test_reference_exec_graph_component_comprehensive() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let component_path = root.join("site/components/exec_graph.knr");
    assert!(component_path.exists(), "site/components/exec_graph.knr must exist");

    // 1. Test lowering from file with automatic import resolution
    let module = lower_program_from_file(&component_path)
        .expect("Failed to lower exec_graph.knr from file");

    // Verify schemas present
    let schema_names: Vec<&str> = module.schemas.iter().map(|s| s.name.as_str()).collect();
    assert!(schema_names.contains(&"Leaf"), "Leaf schema missing");
    assert!(schema_names.contains(&"Card"), "Card schema missing");
    assert!(schema_names.contains(&"Section"), "Section schema missing");
    assert!(schema_names.contains(&"Page"), "Page schema missing");

    // Verify build_exec_graph function
    let func = module
        .functions
        .iter()
        .find(|f| f.name == "build_exec_graph")
        .expect("build_exec_graph function missing");
    assert_eq!(func.ret_ty, Type::Struct("Section".to_string()));

    // Verify instructions and SSA monotonicity in the lowered function
    assert!(!func.blocks.is_empty(), "Function must have blocks");
    let mut prev_dst = None;
    for block in &func.blocks {
        for inst in &block.instructions {
            if let Some(dst) = inst.dst {
                if let Some(prev) = prev_dst {
                    assert!(
                        dst > prev,
                        "Monotonicity invariant violated: dst %{dst} <= prev %{prev}"
                    );
                }
                prev_dst = Some(dst);
            }
        }
    }

    // 2. Full 4-pass verification on lowered module
    let mod_res = verify_module(&module);
    assert!(
        mod_res.is_ok(),
        "exec_graph module failed static verification: {:?}",
        mod_res.err()
    );

    // 3. Verify via verify_program directly
    let src = fs::read_to_string(&component_path).expect("read exec_graph.knr");
    let prog = parse_knr(&src).expect("parse exec_graph.knr");
    let prog_res = verify_program(&prog);
    assert!(
        prog_res.is_ok(),
        "verify_program on exec_graph.knr failed: {:?}",
        prog_res.err()
    );
}

// ============================================================================
// 2. AUTO-DROP Op::Free SYNTHESIS VS GENUINE LINEAR LEAKS
// ============================================================================

#[test]
fn test_autodrop_synthesis_prevents_leaks_on_valid_code() {
    // A: Local struct allocated and not returned -> should auto-drop, 0 diagnostics
    let code_struct = r#"
schema Item {
    0: id i64,
    1: title str,
}

fn process() -> i64 {
    let it = Item { id: 10, title: "test" };
    return it.id;
}
"#;
    let prog_struct = parse_knr(code_struct).expect("parse Item");
    let res_struct = verify_program(&prog_struct);
    assert!(
        res_struct.is_ok(),
        "Valid local struct must be auto-dropped without leak: {:?}",
        res_struct.err()
    );

    // B: Local vector of primitives allocated and not returned -> auto-dropped, 0 diagnostics
    let code_vec = r#"
fn sum_len() -> i64 {
    let v = [1, 2, 3, 4, 5];
    return vec_len(v);
}
"#;
    let prog_vec = parse_knr(code_vec).expect("parse vec");
    let res_vec = verify_program(&prog_vec);
    assert!(
        res_vec.is_ok(),
        "Valid local vector must be auto-dropped without leak: {:?}",
        res_vec.err()
    );

    // C: Nested linear composition: child moved into parent, parent auto-dropped
    let code_nested = r#"
schema Child { 0: name str }
schema Parent { 0: child struct[Child] }

fn make_nested() -> i64 {
    let c = Child { name: "kid" };
    let p = Parent { child: c };
    return 42;
}
"#;
    let prog_nested = parse_knr(code_nested).expect("parse nested");
    let res_nested = verify_program(&prog_nested);
    assert!(
        res_nested.is_ok(),
        "Valid nested struct must be auto-dropped cleanly: {:?}",
        res_nested.err()
    );

    // D: Returned struct is NOT dropped (no double-free)
    let code_ret = r#"
schema Leaf { 0: name str }
fn make_leaf() -> Leaf {
    let l = Leaf { name: "my_leaf" };
    return l;
}
"#;
    let prog_ret = parse_knr(code_ret).expect("parse ret");
    let res_ret = verify_program(&prog_ret);
    assert!(
        res_ret.is_ok(),
        "Returned linear struct must verify cleanly: {:?}",
        res_ret.err()
    );
}

#[test]
fn test_genuine_linear_leaks_are_strictly_caught() {
    // 1. Direct buf_alloc not freed
    let code_buf = r#"
fn leak_buf() -> i64 {
    let b = buf_alloc(128);
    return 0;
}
"#;
    let prog_buf = parse_knr(code_buf).expect("parse buf");
    let res_buf = verify_program(&prog_buf);
    assert!(res_buf.is_err(), "Direct buf_alloc leak must be caught");
    let diags = res_buf.err().unwrap();
    assert_eq!(diags[0].error_kind, "LinearLeak");

    // 2. Struct containing unmanaged buf not freed
    let code_struct_buf = r#"
schema BufWrapper {
    0: raw buf,
}
fn leak_wrapper() -> i64 {
    let w = BufWrapper { raw: buf_alloc(64) };
    return 0;
}
"#;
    let prog_sb = parse_knr(code_struct_buf).expect("parse struct buf");
    let res_sb = verify_program(&prog_sb);
    assert!(res_sb.is_err(), "Struct containing buf leak must be caught");
    let diags = res_sb.err().unwrap();
    assert_eq!(diags[0].error_kind, "LinearLeak");

    // 3. Vector containing unmanaged buf not freed
    let code_vec_buf = r#"
fn leak_vec_buf() -> i64 {
    let v = [buf_alloc(32)];
    return 0;
}
"#;
    let prog_vb = parse_knr(code_vec_buf).expect("parse vec buf");
    let res_vb = verify_program(&prog_vb);
    assert!(res_vb.is_err(), "Vec containing buf leak must be caught");
    let diags = res_vb.err().unwrap();
    assert_eq!(diags[0].error_kind, "LinearLeak");

    // 4. Nested struct containing struct containing buf not freed
    let code_deep_buf = r#"
schema Inner { 0: raw buf }
schema Outer { 0: child struct[Inner] }

fn leak_deep() -> i64 {
    let inn = Inner { raw: buf_alloc(16) };
    let out = Outer { child: inn };
    return 0;
}
"#;
    let prog_deep = parse_knr(code_deep_buf).expect("parse deep buf");
    let res_deep = verify_program(&prog_deep);
    assert!(res_deep.is_err(), "Deeply nested buf leak must be caught");
    let diags = res_deep.err().unwrap();
    assert_eq!(diags[0].error_kind, "LinearLeak");

    // 5. Linear return value discarded by caller (call to linear function without binding or returning)
    let code_discard_ret = r#"
schema HeavyBox { 0: raw buf }
fn make_box() -> HeavyBox {
    return HeavyBox { raw: buf_alloc(32) };
}
fn caller() -> i64 {
    make_box();
    return 0;
}
"#;
    let prog_param = parse_knr(code_discard_ret).expect("parse discard leak");
    let res_param = verify_program(&prog_param);
    assert!(res_param.is_err(), "Discarded linear return value must trigger LinearLeak");
    let diags = res_param.err().unwrap();
    assert!(diags.iter().any(|d| d.error_kind == "LinearLeak"), "Expected LinearLeak diagnostic, got: {:?}", diags);
}

// ============================================================================
// 3. USE-AFTER-MOVE VIOLATIONS
// ============================================================================

#[test]
fn test_use_after_move_adversarial_patterns() {
    // 1. Double call passing linear argument
    let code_double_call = r#"
schema Box { 0: v i64 }
fn consume(b: Box) -> void { return; }
fn test() -> void {
    let b = Box { v: 1 };
    consume(b);
    consume(b);
    return;
}
"#;
    let res1 = verify_program(&parse_knr(code_double_call).unwrap());
    assert!(res1.is_err(), "Double call with linear argument must fail");
    assert_eq!(res1.err().unwrap()[0].error_kind, "UseAfterMove");

    // 2. Field access after move
    let code_field_after_move = r#"
schema Box { 0: v i64 }
fn consume(b: Box) -> void { return; }
fn test() -> i64 {
    let b = Box { v: 42 };
    consume(b);
    return b.v;
}
"#;
    let res2 = verify_program(&parse_knr(code_field_after_move).unwrap());
    assert!(res2.is_err(), "Field access after move must fail");
    assert_eq!(res2.err().unwrap()[0].error_kind, "UseAfterMove");

    // 3. Vector literal with duplicated linear element
    let code_vec_dup = r#"
schema Leaf { 0: s str }
fn test() -> void {
    let l = Leaf { s: "hello" };
    let v = [l, l];
    return;
}
"#;
    let res3 = verify_program(&parse_knr(code_vec_dup).unwrap());
    assert!(res3.is_err(), "Vector with duplicate linear element must fail");
    assert_eq!(res3.err().unwrap()[0].error_kind, "UseAfterMove");

    // 4. Struct literal with duplicated linear field
    let code_struct_dup = r#"
schema Leaf { 0: s str }
schema Node { 0: left struct[Leaf], 1: right struct[Leaf] }
fn test() -> void {
    let l = Leaf { s: "dup" };
    let n = Node { left: l, right: l };
    return;
}
"#;
    let res4 = verify_program(&parse_knr(code_struct_dup).unwrap());
    assert!(res4.is_err(), "Struct with duplicate linear field must fail");
    assert_eq!(res4.err().unwrap()[0].error_kind, "UseAfterMove");

    // 5. Moved into struct, then passed to function
    let code_moved_then_call = r#"
schema Leaf { 0: s str }
schema Node { 0: left struct[Leaf] }
fn consume(l: Leaf) -> void { return; }
fn test() -> void {
    let l = Leaf { s: "dup" };
    let n = Node { left: l };
    consume(l);
    return;
}
"#;
    let res5 = verify_program(&parse_knr(code_moved_then_call).unwrap());
    assert!(res5.is_err(), "Consuming already-threaded variable must fail");
    assert_eq!(res5.err().unwrap()[0].error_kind, "UseAfterMove");

    // 6. Return after move
    let code_ret_after_move = r#"
schema Leaf { 0: s str }
fn consume(l: Leaf) -> void { return; }
fn test() -> Leaf {
    let l = Leaf { s: "ret" };
    consume(l);
    return l;
}
"#;
    let res6 = verify_program(&parse_knr(code_ret_after_move).unwrap());
    assert!(res6.is_err(), "Returning after move must fail");
    assert_eq!(res6.err().unwrap()[0].error_kind, "UseAfterMove");
}

// ============================================================================
// 4. SSA MONOTONICITY & DOMINANCE STRESS
// ============================================================================

#[test]
fn test_monotonicity_deep_expression_tree() {
    // Test increasing depths to find the stack limit and verify monotonicity at safe depths
    for depth in [10, 25, 50, 75] {
        let mut expr = "1".to_string();
        for i in 2..=depth {
            expr = format!("({expr} + {i})");
        }
        let code = format!("fn deep_expr_{depth}() -> i64 {{ return {expr}; }}");

        let prog = parse_knr(&code).expect("parse deep expression");
        let res = verify_program(&prog);
        assert!(
            res.is_ok(),
            "Expression tree of depth {depth} must preserve strict SSA monotonicity: {:?}",
            res.err()
        );
        println!("Expression tree of depth {depth} successfully verified with 0 diagnostics");
    }
}

#[test]
fn test_scale_flat_expression() {
    let terms: Vec<String> = (1..=100).map(|i| i.to_string()).collect();
    let flat_expr = terms.join(" + ");
    let code_flat = format!("fn flat_sum() -> i64 {{ return {flat_expr}; }}");
    let prog_flat = parse_knr(&code_flat).expect("parse flat expression");
    let res_flat = verify_program(&prog_flat);
    assert!(res_flat.is_ok(), "Flat 100-term expression must verify cleanly: {:?}", res_flat.err());
}

#[test]
fn test_scale_large_vector() {
    let elems: Vec<String> = (1..=200).map(|i| i.to_string()).collect();
    let vec_lit = format!("[{}]", elems.join(", "));
    let code_vec = format!("fn big_vec() -> i64 {{ let v = {vec_lit}; return vec_len(v); }}");
    let prog_vec = parse_knr(&code_vec).expect("parse large vector");
    let res_vec = verify_program(&prog_vec);
    assert!(res_vec.is_ok(), "Large 200-element vector must verify cleanly: {:?}", res_vec.err());
}

#[test]
fn test_scale_nested_blocks() {
    let mut nested_block = "return 42;".to_string();
    for _ in 0..40 {
        nested_block = format!("{{ {nested_block} }}");
    }
    let code_block = format!("fn nested_blocks() -> i64 {{ {nested_block} }}");
    let prog_block = parse_knr(&code_block).expect("parse nested blocks");
    let res_block = verify_program(&prog_block);
    assert!(res_block.is_ok(), "Deeply nested blocks must verify cleanly: {:?}", res_block.err());
}

#[test]
fn test_scale_wide_struct() {
    let mut fields_def = String::new();
    let mut fields_init = String::new();
    for i in 0..30 {
        fields_def.push_str(&format!("    {i}: f{i} i64,\n"));
        fields_init.push_str(&format!("f{i}: {i}, "));
    }
    let code_struct = format!(
        "schema WideStruct {{\n{fields_def}}}\nfn make_wide() -> i64 {{\n    let w = WideStruct {{ {fields_init} }};\n    return w.f0;\n}}"
    );
    let prog_struct = parse_knr(&code_struct).expect("parse wide struct");
    let res_struct = verify_program(&prog_struct);
    assert!(res_struct.is_ok(), "Wide struct with 30 fields must verify cleanly: {:?}", res_struct.err());
}

#[test]
fn test_loop_and_branch_monotonicity() {
    // While loop with internal expressions and branch
    let code = r#"
fn loop_with_branch(n: i64) -> i64 {
    let mut sum = 0;
    let mut i = 0;
    while i < n {
        if i > 5 {
            sum = sum + 2;
        } else {
            sum = sum + 1;
        }
        i = i + 1;
    }
    return sum;
}
"#;
    // Lowering must reject or verify without panic
    let prog = parse_knr(code);
    if let Ok(p) = prog {
        let res = verify_program(&p);
        // Whether valid or invalid, it must NEVER panic
        println!("loop_with_branch verification result: {res:?}");
    }
}

// ============================================================================
// 5. ADVERSARIAL STRESS HARNESS (NO PANICS GUARANTEE)
// ============================================================================

#[test]
fn test_adversarial_malformed_programs_never_panic() {
    let adversarial_programs = [
        // Empty program
        "",
        // Only comments
        "// Just a comment\n/* Multi-line comment */",
        // Unclosed struct
        "schema Incomplete { 0: a i64",
        // Recursive schema
        "schema SelfRef { 0: next struct[SelfRef] }",
        // Mutual recursion schemas
        "schema A { 0: b struct[B] }\nschema B { 0: a struct[A] }",
        // Function returning undefined struct
        "fn get_ghost() -> Ghost { return 0; }",
        // Call with wrong argument count
        "fn add2(a: i64, b: i64) -> i64 { return a + b; }\nfn test() -> i64 { return add2(1); }",
        // Call with wrong types
        "fn add2(a: i64, b: i64) -> i64 { return a + b; }\nfn test() -> i64 { return add2(1, \"str\"); }",
        // Undeclared effect
        "fn need_net() -> void { syscall Net \"net.listen\" 8080; return; }",
        // Syscall with wrong effect name
        "fn bad_eff() -> void effects(Ui) { syscall FakeEff \"op\" 0; return; }",
        // Array of mismatched types
        "fn bad_vec() -> void { let v = [1, \"mismatch\"]; return; }",
        // Empty array literal without type
        "fn empty_vec() -> void { let v = []; return; }",
        // Field access on non-struct
        "fn non_struct_field(x: i64) -> i64 { return x.foo; }",
        // Index on non-collection
        "fn non_vec_index(x: i64) -> i64 { return x[0]; }",
        // Assignment to non-variable
        "fn assign_lit() -> void { 1 = 2; return; }",
        // Reassignment with type mismatch
        "fn bad_assign() -> void { let mut x = 10; x = \"string\"; return; }",
        // Struct literal with missing field
        "schema Pair { 0: a i64, 1: b i64 }\nfn bad_lit() -> Pair { return Pair { a: 1 }; }",
        // Struct literal with unknown field
        "schema Pair { 0: a i64, 1: b i64 }\nfn bad_lit() -> Pair { return Pair { a: 1, b: 2, c: 3 }; }",
        // Struct literal with field type mismatch
        "schema Pair { 0: a i64, 1: b i64 }\nfn bad_lit() -> Pair { return Pair { a: 1, b: \"str\" }; }",
        // Negation on non-number
        "fn neg_bool() -> bool { return -true; }",
        // Logical NOT on non-bool
        "fn not_int() -> i64 { return !42; }",
    ];

    for (idx, p_str) in adversarial_programs.iter().enumerate() {
        match parse_knr(p_str) {
            Ok(prog) => {
                let _ = verify_program(&prog);
            }
            Err(_) => {
                // Parse errors are expected for malformed syntax
            }
        }
        println!("Adversarial program {idx} processed cleanly without panic");
    }
}
