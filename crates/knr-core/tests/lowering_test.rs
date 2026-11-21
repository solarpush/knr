use knr_core::lower::lower_program;
use knr_core::syntax::parse_knr;
use knr_core::Op;

#[test]
fn test_lower_arithmetic() {
    let src = r#"
        fn calc(a: i64) -> i64 {
            let y = (a + 2) * 3;
            return y;
        }
    "#;
    let prog = parse_knr(src).unwrap();
    let module = lower_program(&prog).unwrap();
    assert_eq!(module.functions.len(), 1);
    let func = &module.functions[0];
    assert_eq!(func.params.len(), 1);
    assert_eq!(func.params[0].0, 0);

    // Verify monotonic instruction destination registers
    let mut last = 0;
    for block in &func.blocks {
        for inst in &block.instructions {
            if let Some(dst) = inst.dst {
                assert!(dst > last, "Destination %{dst} not monotonic after %{last}");
                last = dst;
            }
        }
    }
}

#[test]
fn test_lower_struct_literal_threading() {
    let src = r#"
        schema Point {
            0: x i64,
            1: y i64,
        }
        fn make() -> Point {
            let p = Point {
                x: 100,
                y: 200,
            };
            return p;
        }
    "#;
    let prog = parse_knr(src).unwrap();
    let module = lower_program(&prog).unwrap();
    let func = &module.functions[0];
    let insts = &func.blocks[0].instructions;

    // Check for StructNew and two StructSet
    assert!(insts.iter().any(|i| matches!(i.op, Op::StructNew(_))));
    let set_count = insts
        .iter()
        .filter(|i| matches!(i.op, Op::StructSet(_, _, _)))
        .count();
    assert_eq!(set_count, 2);
}

#[test]
fn test_lower_array_literal_threading() {
    let src = r#"
        fn list() -> vec[i64] {
            return [10, 20, 30];
        }
    "#;
    let prog = parse_knr(src).unwrap();
    let module = lower_program(&prog).unwrap();
    let func = &module.functions[0];
    let insts = &func.blocks[0].instructions;

    assert!(insts.iter().any(|i| matches!(i.op, Op::VecNew(_))));
    let push_count = insts
        .iter()
        .filter(|i| matches!(i.op, Op::VecPush(_, _)))
        .count();
    assert_eq!(push_count, 3);
}

#[test]
fn test_auto_drop_unreturned_linear_values() {
    let src = r#"
        schema Box {
            0: val i64,
        }
        fn drop_me() -> i64 {
            let b = Box { val: 42 };
            let numbers = [1, 2, 3];
            return 999;
        }
    "#;
    let prog = parse_knr(src).unwrap();
    let module = lower_program(&prog).unwrap();
    let func = &module.functions[0];
    let insts = &func.blocks[0].instructions;

    // Verify Op::Free was emitted for unreturned linear values
    let free_count = insts
        .iter()
        .filter(|i| matches!(i.op, Op::Free(_)))
        .count();
    assert!(
        free_count >= 2,
        "Expected at least 2 Op::Free instructions emitted for unreturned Box and vec, found {free_count}"
    );
}

#[test]
fn test_lower_branching() {
    let src = r#"
        fn branch(a: i64, b: i64) -> i64 {
            if a > b {
                return a;
            } else {
                return b;
            }
        }
    "#;
    let prog = parse_knr(src).unwrap();
    let module = lower_program(&prog).unwrap();
    let func = &module.functions[0];
    assert!(func.blocks.len() >= 3);
}
