use knr_core::syntax::ast::*;
use knr_core::syntax::parse_knr;
use std::fs;
use std::path::Path;

#[test]
fn test_parse_struct_definition() {
    let src = r#"
        schema Point {
            0: x i64,
            1: y i64,
        }
        struct Widget {
            id: i64,
            name: str,
        }
    "#;
    let prog = parse_knr(src).expect("parse schema and struct");
    assert_eq!(prog.declarations.len(), 2);
    if let Declaration::Struct(s) = &prog.declarations[0] {
        assert_eq!(s.name.name, "Point");
        assert_eq!(s.fields.len(), 2);
        assert_eq!(s.fields[0].index, Some(0));
        assert_eq!(s.fields[0].name.name, "x");
    } else {
        panic!("Expected struct declaration");
    }
}

#[test]
fn test_parse_struct_literal() {
    let src = r#"
        fn make() -> Point {
            let p = Point {
                x: 10,
                y: 20,
            };
            return p;
        }
    "#;
    let prog = parse_knr(src).expect("parse struct literal");
    assert_eq!(prog.declarations.len(), 1);
}

#[test]
fn test_parse_array_literal() {
    let src = r#"
        fn nums() -> vec[i64] {
            return [1, 2, 3, 4,];
        }
    "#;
    let prog = parse_knr(src).expect("parse array literal");
    if let Declaration::Function(f) = &prog.declarations[0] {
        if let StmtKind::Return { value: Some(expr) } = &f.body.stmts[0].kind {
            if let ExprKind::ArrayLit(elems) = &expr.kind {
                assert_eq!(elems.len(), 4);
            } else {
                panic!("Expected array literal");
            }
        }
    }
}

#[test]
fn test_parse_expressions_precedence() {
    let src = r#"
        fn calc(a: bool, b: bool, c: bool) -> i64 {
            let x = 2 + 3 * 4;
            let cond = a && b || c;
            return x;
        }
    "#;
    let prog = parse_knr(src).expect("parse expressions precedence");
    if let Declaration::Function(f) = &prog.declarations[0] {
        if let StmtKind::Let { init, .. } = &f.body.stmts[0].kind {
            if let ExprKind::Binary { op, lhs, rhs } = &init.kind {
                assert_eq!(*op, BinaryOp::Add);
                assert!(matches!(lhs.kind, ExprKind::Int(2)));
                assert!(matches!(rhs.kind, ExprKind::Binary { op: BinaryOp::Mul, .. }));
            }
        }
    }
}

#[test]
fn test_parse_control_flow() {
    let src = r#"
        fn flow(x: i64) -> i64 {
            if x > 10 {
                return 1;
            } else if x == 0 {
                return 0;
            } else {
                return -1;
            }
        }
    "#;
    let prog = parse_knr(src).expect("parse control flow");
    assert_eq!(prog.declarations.len(), 1);
}

#[test]
fn test_parse_reference_exec_graph() {
    let component_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("site/components/exec_graph.knr");
    assert!(component_path.exists());
    let src = fs::read_to_string(&component_path).expect("read exec_graph.knr");
    let prog = parse_knr(&src).expect("parse exec_graph.knr");
    assert_eq!(prog.imports.len(), 1);
    assert_eq!(prog.imports[0].path, "../schemas/vdom.ssa.schema");
    assert_eq!(prog.declarations.len(), 1);
    if let Declaration::Function(f) = &prog.declarations[0] {
        assert_eq!(f.name.name, "build_exec_graph");
        assert_eq!(f.body.stmts.len(), 9); // 8 cards let-stmts + 1 return
    } else {
        panic!("Expected function");
    }
}

#[test]
fn test_syntax_errors() {
    let src_unclosed = "fn test() -> i64 { return 1;";
    let err = parse_knr(src_unclosed);
    assert!(err.is_err());

    let src_invalid_tok = "fn test() { let @ = 5; }";
    let err2 = parse_knr(src_invalid_tok);
    assert!(err2.is_err());
}
