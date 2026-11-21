use knr_core::ast::{Op, RegId, Terminator};
use knr_core::lower::lower_program;
use knr_core::syntax::parse_knr;
use std::collections::HashSet;

/// Helper to verify SSA single-assignment and strict monotonic register numbering
fn assert_ssa_monotonic_registers(func: &knr_core::Function) {
    let mut last_defined: Option<RegId> = None;
    let mut defined = HashSet::new();

    // Verify parameters: 0, 1, ..., P-1
    for (param_reg, _) in &func.params {
        assert!(
            defined.insert(*param_reg),
            "Multiple assignment for parameter %{param_reg} in function {}",
            func.name
        );
        if let Some(prev) = last_defined {
            assert!(
                *param_reg > prev,
                "Param %{param_reg} is not strictly greater than %{prev}"
            );
        }
        last_defined = Some(*param_reg);
    }

    // Verify all instruction destinations in block order
    for block in &func.blocks {
        for inst in &block.instructions {
            if let Some(dst) = inst.dst {
                assert!(
                    defined.insert(dst),
                    "Multiple assignment for destination %{dst} in function {}",
                    func.name
                );
                if let Some(prev) = last_defined {
                    assert!(
                        dst > prev,
                        "Destination %{dst} is not strictly greater than %{prev} in instruction {:?}",
                        inst
                    );
                }
                last_defined = Some(dst);
            }
        }
    }
}

/// Helper to verify CFG basic block terminator targets exist
fn assert_cfg_block_targets_valid(func: &knr_core::Function) {
    let block_ids: HashSet<_> = func.blocks.iter().map(|b| b.id).collect();
    assert!(
        block_ids.contains(&0),
        "Function {} is missing entry block bb0",
        func.name
    );

    for block in &func.blocks {
        for (target, _) in block.terminator.targets() {
            assert!(
                block_ids.contains(&target),
                "Terminator in block bb{} jumps to nonexistent block bb{}",
                block.id,
                target
            );
        }
    }
}

// -----------------------------------------------------------------------------
// 1. Stress Tests: Deeply Nested Expressions & Associativity / Precedence
// -----------------------------------------------------------------------------

#[test]
fn test_stress_deeply_nested_subtraction_left_associativity() {
    // Left-associative: 100 - 1 - 2 - 3 ... - 50 must parse as ((100 - 1) - 2) ...
    let terms: Vec<String> = (1..=50).map(|i| i.to_string()).collect();
    let expr_str = format!("100 - {}", terms.join(" - "));
    let src = format!("fn sub_chain() -> i64 {{ return {expr_str}; }}");

    let prog = parse_knr(&src).expect("Parse deeply nested subtraction chain");
    let module = lower_program(&prog).expect("Lower deeply nested subtraction chain");

    let func = &module.functions[0];
    assert_ssa_monotonic_registers(func);
    assert_cfg_block_targets_valid(func);

    // Verify the instructions represent sequential subtractions
    let sub_ops: Vec<_> = func.blocks[0]
        .instructions
        .iter()
        .filter_map(|i| match &i.op {
            Op::Sub(l, r) => Some((*l, *r, i.dst.unwrap())),
            _ => None,
        })
        .collect();

    assert_eq!(sub_ops.len(), 50);
    // In left-associative subtraction, the LHS of each subsequent Sub is the DST of the previous Sub
    for i in 1..sub_ops.len() {
        let prev_dst = sub_ops[i - 1].2;
        let curr_lhs = sub_ops[i].0;
        assert_eq!(
            curr_lhs, prev_dst,
            "Subtraction at step {i} is not left-associative! Expected LHS %{prev_dst}, got %{curr_lhs}"
        );
    }
}

#[test]
fn test_stress_deeply_nested_parentheses_recursion() {
    // 80 levels of nested parentheses: ((( ... (42 + 8) ... )))
    let depth = 80;
    let mut expr = "42 + 8".to_string();
    for _ in 0..depth {
        expr = format!("({expr})");
    }
    let src = format!("fn nested_parens() -> i64 {{ return {expr}; }}");

    let prog = parse_knr(&src).expect("Parse 80-level nested parentheses");
    let module = lower_program(&prog).expect("Lower 80-level nested parentheses");

    let func = &module.functions[0];
    assert_ssa_monotonic_registers(func);
    assert_cfg_block_targets_valid(func);

    let add_ops: Vec<_> = func.blocks[0]
        .instructions
        .iter()
        .filter(|i| matches!(i.op, Op::Add(_, _)))
        .collect();
    assert_eq!(add_ops.len(), 1);
}

#[test]
fn test_stress_complex_operator_precedence_hierarchy() {
    // Precedence: (a + (b * c)) - (d / e) == (f < g)
    let src = r#"
        fn precedence(a: i64, b: i64, c: i64, d: i64, e: i64, f: i64) -> bool {
            return a + b * c < d == e > f;
        }
    "#;

    let prog = parse_knr(src).expect("Parse precedence hierarchy");
    let module = lower_program(&prog).expect("Lower precedence hierarchy");

    let func = &module.functions[0];
    assert_ssa_monotonic_registers(func);
    assert_cfg_block_targets_valid(func);

    let insts = &func.blocks[0].instructions;
    let mul_idx = insts
        .iter()
        .position(|i| matches!(i.op, Op::Mul(_, _)))
        .expect("mul op");
    let add_idx = insts
        .iter()
        .position(|i| matches!(i.op, Op::Add(_, _)))
        .expect("add op");
    let lt_idx = insts
        .iter()
        .position(|i| matches!(i.op, Op::CmpLt(_, _)))
        .expect("cmplt op");
    let eq_idx = insts
        .iter()
        .position(|i| matches!(i.op, Op::CmpEq(_, _)))
        .expect("cmpeq op");

    assert!(mul_idx < add_idx, "b * c must precede a + (b * c)");
    assert!(add_idx < lt_idx, "a + (b * c) must precede (...) < d");
    assert!(lt_idx < eq_idx, "(...) < d must precede (...) == (...)");
}

#[test]
fn test_stress_chained_unary_prefix_operators() {
    let src = r#"
        fn unaries(x: i64, flag: bool) -> i64 {
            let a = - - -x;
            let b = ! !flag;
            if b {
                return a;
            } else {
                return 0;
            }
        }
    "#;

    let prog = parse_knr(src).expect("Parse chained unary operators");
    let module = lower_program(&prog).expect("Lower chained unary operators");

    let func = &module.functions[0];
    assert_ssa_monotonic_registers(func);
    assert_cfg_block_targets_valid(func);

    // Verify 3 Sub operations for triple negation
    let sub_count = func
        .blocks
        .iter()
        .flat_map(|b| &b.instructions)
        .filter(|i| matches!(i.op, Op::Sub(_, _)))
        .count();
    assert_eq!(sub_count, 3);
}

// -----------------------------------------------------------------------------
// 2. Stress Tests: Deeply Nested Array and Struct Literals (Chained Linear Moves)
// -----------------------------------------------------------------------------

#[test]
fn test_stress_deeply_nested_struct_literals_chained_moves() {
    let src = r#"
        schema Level0 { 0: v0 i64, }
        schema Level1 { 0: l0 struct[Level0], }
        schema Level2 { 0: l1 struct[Level1], }
        schema Level3 { 0: l2 struct[Level2], }
        schema Level4 { 0: l3 struct[Level3], }

        fn build_deep_struct() -> Level4 {
            let tree = Level4 {
                l3: Level3 {
                    l2: Level2 {
                        l1: Level1 {
                            l0: Level0 {
                                v0: 999,
                            },
                        },
                    },
                },
            };
            return tree;
        }
    "#;

    let prog = parse_knr(src).expect("Parse deeply nested struct literals");
    let module = lower_program(&prog).expect("Lower deeply nested struct literals");

    let func = &module.functions[0];
    assert_ssa_monotonic_registers(func);
    assert_cfg_block_targets_valid(func);

    let insts = &func.blocks[0].instructions;
    // Exactly 5 StructNew instructions and 5 StructSet instructions
    let new_count = insts
        .iter()
        .filter(|i| matches!(i.op, Op::StructNew(_)))
        .count();
    let set_count = insts
        .iter()
        .filter(|i| matches!(i.op, Op::StructSet(_, _, _)))
        .count();

    assert_eq!(new_count, 5);
    assert_eq!(set_count, 5);

    // Verify final returned register is the outer struct
    if let Terminator::Ret(Some(ret_reg)) = func.blocks[0].terminator {
        let last_set = insts
            .iter()
            .rfind(|i| matches!(i.op, Op::StructSet(_, _, _)))
            .unwrap();
        assert_eq!(last_set.dst, Some(ret_reg));
    } else {
        panic!("Expected Ret(Some(ret_reg))");
    }
}

#[test]
fn test_stress_deeply_nested_3d_array_literals_chained_moves() {
    let src = r#"
        fn build_matrix_3d() -> vec[vec[vec[i64]]] {
            let m = [
                [[1, 2], [3, 4]],
                [[5, 6], [7, 8]]
            ];
            return m;
        }
    "#;

    let prog = parse_knr(src).expect("Parse 3D nested vector");
    let module = lower_program(&prog).expect("Lower 3D nested vector");

    let func = &module.functions[0];
    assert_ssa_monotonic_registers(func);
    assert_cfg_block_targets_valid(func);

    let insts = &func.blocks[0].instructions;
    // 4 leaf vecs (1D) + 2 intermediate vecs (2D) + 1 root vec (3D) = 7 VecNew
    let vec_new_count = insts
        .iter()
        .filter(|i| matches!(i.op, Op::VecNew(_)))
        .count();
    assert_eq!(vec_new_count, 7);

    // 8 elements pushed into 1D + 4 vecs pushed into 2D + 2 vecs pushed into 3D = 14 VecPush
    let vec_push_count = insts
        .iter()
        .filter(|i| matches!(i.op, Op::VecPush(_, _)))
        .count();
    assert_eq!(vec_push_count, 14);
}

#[test]
fn test_stress_chained_field_access_and_indexing() {
    let src = r#"
        schema Item {
            0: weight i64,
        }
        schema Container {
            0: items vec[struct[Item]],
        }

        fn extract_weight(c: Container) -> i64 {
            let w = c.items[0].weight;
            return w;
        }
    "#;

    let prog = parse_knr(src).expect("Parse chained access and indexing");
    let module = lower_program(&prog).expect("Lower chained access and indexing");

    let func = &module.functions[0];
    assert_ssa_monotonic_registers(func);
    assert_cfg_block_targets_valid(func);

    let insts = &func.blocks[0].instructions;
    assert!(insts.iter().any(|i| matches!(i.op, Op::StructGet(_, 0))));
    assert!(insts.iter().any(|i| matches!(i.op, Op::VecGet(_, _))));
}

// -----------------------------------------------------------------------------
// 3. Stress Tests: Monotonic SSA Register Invariants Across Multi-Block Branches
// -----------------------------------------------------------------------------

#[test]
fn test_stress_multi_level_nested_if_else_branches() {
    let src = r#"
        fn classify(x: i64, y: i64) -> i64 {
            let res = 0;
            if x > 0 {
                if y > 0 {
                    return 1;
                } else if y == 0 {
                    return 2;
                } else {
                    return 3;
                }
            } else if x == 0 {
                if y > 0 {
                    return 4;
                } else {
                    return 5;
                }
            } else {
                if y > 0 {
                    return 6;
                } else {
                    return 7;
                }
            }
        }
    "#;

    let prog = parse_knr(src).expect("Parse multi-level nested if-else");
    let module = lower_program(&prog).expect("Lower multi-level nested if-else");

    let func = &module.functions[0];
    assert_ssa_monotonic_registers(func);
    assert_cfg_block_targets_valid(func);

    // Verify all leaf returns exist and have monotonic registers
    assert!(func.blocks.len() >= 8);
}

#[test]
fn test_stress_while_loop_simple() {
    let src = r#"
        fn count_loop() -> i64 {
            let i = 0;
            while i < 100 {
                i = i + 1;
            }
            return i;
        }
    "#;

    let prog = parse_knr(src).expect("Parse simple while loop");
    let module = lower_program(&prog).expect("Lower simple while loop");

    let func = &module.functions[0];
    assert_ssa_monotonic_registers(func);
    assert_cfg_block_targets_valid(func);
}

// -----------------------------------------------------------------------------
// Confirmed Lowering Engine Defect Remediation Tests
// -----------------------------------------------------------------------------

#[test]
fn test_defect_while_loop_nested_if_return_discards_merge_block() {
    // FIX: start_block and is_terminated ensure merge_bb (bb5) is retained and wired back
    // to loop_header (bb1), eliminating InvalidBranchTarget.
    let src = r#"
        fn search_loop(target: i64) -> i64 {
            let i = 0;
            while i < 100 {
                if i == target {
                    return i * 2;
                }
                i = i + 1;
            }
            return -1;
        }
    "#;

    let prog = parse_knr(src).expect("Parse succeeds");
    let module = lower_program(&prog).expect("Lowering succeeds");
    let func = &module.functions[0];

    let block_ids: HashSet<_> = func.blocks.iter().map(|b| b.id).collect();
    // Confirmed Fix: bb5 exists in the function's CFG blocks!
    assert!(
        block_ids.contains(&5),
        "Merge block bb5 must exist in function CFG blocks"
    );

    // Block bb2 branches to bb5
    let bb2 = func.blocks.iter().find(|b| b.id == 2).expect("bb2 exists");
    assert!(
        matches!(bb2.terminator, Terminator::BrIf { else_blk: 5, .. }),
        "bb2 terminator points to merge block bb5: {:?}",
        bb2.terminator
    );

    let bb5 = func.blocks.iter().find(|b| b.id == 5).expect("bb5 exists");
    assert!(
        matches!(
            bb5.terminator,
            Terminator::Br(1) | Terminator::BrArgs { target: 1, .. }
        ),
        "bb5 terminator points back to loop header bb1: {:?}",
        bb5.terminator
    );

    assert_ssa_monotonic_registers(func);
    assert_cfg_block_targets_valid(func);
}

#[test]
fn test_defect_branch_local_linear_allocations_autodrop_leak() {
    // FIX: LinearTracker branch isolation snapshots tracker before then-branch and restores it
    // before else-branch, dropping r_outer on both branches (4 Free ops total).
    let src = r#"
        schema Resource {
            0: id i64,
        }

        fn conditional_alloc(cond: bool) -> i64 {
            let r_outer = Resource { id: 1 };
            if cond {
                let r_then = Resource { id: 2 };
                return 10;
            } else {
                let r_else = Resource { id: 3 };
                return 20;
            }
        }
    "#;

    let prog = parse_knr(src).expect("Parse conditional linear alloc");
    let module = lower_program(&prog).expect("Lower conditional linear alloc");
    let func = &module.functions[0];

    let mut free_total = 0;
    for block in &func.blocks {
        let frees = block
            .instructions
            .iter()
            .filter(|i| matches!(i.op, Op::Free(_)))
            .count();
        free_total += frees;
    }
    // Confirmed Fix: exactly 4 Free ops emitted across then and else branches
    assert_eq!(
        free_total, 4,
        "Linear tracker must drop r_outer on both then and else branches (total 4 Free ops)"
    );
    assert_ssa_monotonic_registers(func);
    assert_cfg_block_targets_valid(func);
}

#[test]
fn test_defect_linear_move_in_mutually_exclusive_branches() {
    // FIX: LinearTracker branch isolation restores snapshot before else branch,
    // allowing mutually exclusive consumption without false UseAfterMove.
    let src = r#"
        schema Box { 0: val i64, }
        fn consume_box(b: Box) -> i64 {
            return b.val;
        }
        fn test_move(cond: bool) -> i64 {
            let b = Box { val: 42 };
            if cond {
                let x = consume_box(b);
                return x;
            } else {
                let y = consume_box(b);
                return y;
            }
        }
    "#;

    let prog = parse_knr(src).expect("Parse succeeds");
    let res = lower_program(&prog);
    assert!(
        res.is_ok(),
        "Mutually exclusive branches must not trigger false UseAfterMove: {:?}",
        res.err()
    );
    let (module, source_map) = knr_core::lower::lower_program_with_source_map(&prog).expect("SourceMap succeeds");
    assert_eq!(source_map.get_var("consume_box", 0), Some("b"));
    assert_eq!(source_map.get_var("test_move", 0), Some("cond"));
    assert_ssa_monotonic_registers(&module.functions[0]);
    assert_ssa_monotonic_registers(&module.functions[1]);
    assert_cfg_block_targets_valid(&module.functions[0]);
    assert_cfg_block_targets_valid(&module.functions[1]);
}

// -----------------------------------------------------------------------------
// 4. Edge Cases: Diagnostics & Error Handling Robustness
// -----------------------------------------------------------------------------

#[test]
fn test_stress_empty_vector_literal_requires_annotation() {
    let src = r#"
        fn empty_vec() {
            let v = [];
        }
    "#;
    let prog = parse_knr(src).expect("Parse succeeds");
    let err = lower_program(&prog);
    assert!(err.is_err());
    let err_msg = err.err().unwrap().to_string();
    assert!(
        err_msg.contains("Empty vector literal requires explicit type annotation"),
        "Unexpected error: {err_msg}"
    );

    // With explicit type annotation, empty vector lowering succeeds and synthesizes Op::VecNew
    let src_typed = r#"
        fn empty_vec_typed() -> vec[i64] {
            let v: vec[i64] = [];
            return v;
        }
    "#;
    let prog_typed = parse_knr(src_typed).expect("Parse typed empty vector");
    let module_typed = lower_program(&prog_typed).expect("Lower typed empty vector");
    let func = &module_typed.functions[0];
    assert!(func.blocks[0].instructions.iter().any(|i| matches!(i.op, Op::VecNew(knr_core::Type::I64))));
    assert_ssa_monotonic_registers(func);
    assert_cfg_block_targets_valid(func);
}

#[test]
fn test_stress_struct_literal_in_condition_parenthesized() {
    // Parenthesized struct literal in condition: `(Point { x: 1, y: 2 }.x > 0)`
    let src = r#"
        schema Point {
            0: x i64,
            1: y i64,
        }
        fn test_cond() -> bool {
            if (Point { x: 1, y: 2 }.x > 0) {
                return true;
            } else {
                return false;
            }
        }
    "#;

    let prog = parse_knr(src).expect("Parse parenthesized struct literal in if condition");
    let module = lower_program(&prog).expect("Lower struct literal in condition");

    let func = &module.functions[0];
    assert_ssa_monotonic_registers(func);
    assert_cfg_block_targets_valid(func);
}

#[test]
fn test_stress_large_function_stress_200_let_bindings() {
    // Generate a function with 200 sequential let bindings
    let mut lines = Vec::new();
    lines.push("fn chain_200() -> i64 {".to_string());
    lines.push("    let x0 = 1;".to_string());
    for i in 1..=200 {
        lines.push(format!("    let x{i} = x{} + 1;", i - 1));
    }
    lines.push("    return x200;".to_string());
    lines.push("}".to_string());

    let src = lines.join("\n");
    let prog = parse_knr(&src).expect("Parse 200 let bindings");
    let module = lower_program(&prog).expect("Lower 200 let bindings");

    let func = &module.functions[0];
    assert_ssa_monotonic_registers(func);
    assert_cfg_block_targets_valid(func);
    assert!(func.blocks[0].instructions.len() >= 400); // 200 consts + 200 adds
}
