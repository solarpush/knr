//! Milestone 5 Phase 2: Adversarial Coverage Hardening Suite for knr-core.
//! Targets:
//! 1. `syntax/` (Pratt parser, precedence levels, lexer, string escaping, comments, spans)
//! 2. `lower/` (Monotonic register synthesis, branch linear isolation, scope-local auto-drop, while loop merge blocks)
//! 3. `explore.rs` (`explore_path`, `.knr` lowering, raw file info, circular import detection)
//! 4. `manifest.rs` (Manifest parser, `[workspace]`, glob expansion, symlink cycle protection)

use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use knr_core::ast::Op;
use knr_core::error::ParseError;
use knr_core::explore::explore_path;
use knr_core::lower::{lower_program, LowerError};
use knr_core::manifest::{
    resolve_workspace_members, BuildConfig, DependencySpec, Manifest, ManifestError,
    PackageManifest, UiConfig,
};
use knr_core::syntax::ast::{BinaryOp, ExprKind, StmtKind};
use knr_core::syntax::parse_knr;

/// RAII helper for temporary directories.
struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(prefix: &str) -> Self {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let id = COUNTER.fetch_add(1, Ordering::SeqCst);
        let pid = std::process::id();
        let dir = std::env::temp_dir().join(format!("knr_adv_m5_{prefix}_{pid}_{id}"));
        fs::create_dir_all(&dir).expect("failed to create temp dir");
        Self { path: dir }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&self.path, fs::Permissions::from_mode(0o755));
        }
        let _ = fs::remove_dir_all(&self.path);
    }
}

// ============================================================================
// Target 1: crates/knr-core/src/syntax/
// ============================================================================

#[test]
fn test_adv_syntax_pratt_precedence_hierarchy_and_left_associativity() {
    // Left-associativity of Sub: a - b - c - d => (((a - b) - c) - d)
    let src = "fn test(a: i64, b: i64, c: i64, d: i64) -> i64 { return a - b - c - d; }";
    let prog = parse_knr(src).expect("parse sub chain");
    if let knr_core::syntax::ast::Declaration::Function(ref f) = prog.declarations[0] {
        if let StmtKind::Return { value: Some(ref expr) } = f.body.stmts[0].kind {
            if let ExprKind::Binary { op, lhs, rhs } = &expr.kind {
                assert_eq!(*op, BinaryOp::Sub);
                assert!(matches!(rhs.kind, ExprKind::Variable(ref id) if id.name == "d"));
                if let ExprKind::Binary { op: op2, lhs: lhs2, rhs: rhs2 } = &lhs.kind {
                    assert_eq!(*op2, BinaryOp::Sub);
                    assert!(matches!(rhs2.kind, ExprKind::Variable(ref id) if id.name == "c"));
                    if let ExprKind::Binary { op: op3, lhs: lhs3, rhs: rhs3 } = &lhs2.kind {
                        assert_eq!(*op3, BinaryOp::Sub);
                        assert!(matches!(lhs3.kind, ExprKind::Variable(ref id) if id.name == "a"));
                        assert!(matches!(rhs3.kind, ExprKind::Variable(ref id) if id.name == "b"));
                    } else {
                        panic!("Expected 3rd level binary sub");
                    }
                } else {
                    panic!("Expected 2nd level binary sub");
                }
            } else {
                panic!("Expected top-level binary sub");
            }
        }
    }

    // Precedence: Mul precedes Add: 2 + 3 * 4 => 2 + (3 * 4)
    let src2 = "fn test() -> i64 { return 2 + 3 * 4; }";
    let prog2 = parse_knr(src2).expect("parse precedence");
    if let knr_core::syntax::ast::Declaration::Function(ref f) = prog2.declarations[0] {
        if let StmtKind::Return { value: Some(ref expr) } = f.body.stmts[0].kind {
            if let ExprKind::Binary { op, lhs, rhs } = &expr.kind {
                assert_eq!(*op, BinaryOp::Add);
                assert!(matches!(lhs.kind, ExprKind::Int(2)));
                assert!(matches!(rhs.kind, ExprKind::Binary { op: BinaryOp::Mul, .. }));
            }
        }
    }

    // Relational precedes Equality: a < b == c > d => (a < b) == (c > d)
    let src3 = "fn test(a: i64, b: i64, c: i64, d: i64) -> bool { return a < b == c > d; }";
    let prog3 = parse_knr(src3).expect("parse rel vs eq");
    if let knr_core::syntax::ast::Declaration::Function(ref f) = prog3.declarations[0] {
        if let StmtKind::Return { value: Some(ref expr) } = f.body.stmts[0].kind {
            if let ExprKind::Binary { op, lhs, rhs } = &expr.kind {
                assert_eq!(*op, BinaryOp::Eq);
                assert!(matches!(lhs.kind, ExprKind::Binary { op: BinaryOp::Lt, .. }));
                assert!(matches!(rhs.kind, ExprKind::Binary { op: BinaryOp::Gt, .. }));
            }
        }
    }
}

#[test]
fn test_adv_syntax_lexer_string_escapes_and_unterminated_rejection() {
    // Valid escapes: \n, \t, \r, \", \\
    let src = r#"let s = "line1\nline2\ttabbed\r\n\"quote\"\\backslash";"#;
    let mut lexer = knr_core::syntax::lexer::Lexer::new(src);
    let tokens = lexer.tokenize_all().expect("tokenize valid string escapes");
    let str_tok = tokens
        .iter()
        .find(|t| matches!(t.kind, knr_core::syntax::token::TokenKind::StrLit(_)))
        .expect("string token found");
    if let knr_core::syntax::token::TokenKind::StrLit(ref s) = str_tok.kind {
        assert_eq!(s, "line1\nline2\ttabbed\r\n\"quote\"\\backslash");
    }

    // Unterminated string literal rejected
    let unclosed = r#"let s = "unclosed string without end;"#;
    let mut lexer_unclosed = knr_core::syntax::lexer::Lexer::new(unclosed);
    let err = lexer_unclosed.tokenize_all().unwrap_err();
    assert!(matches!(err, ParseError::UnterminatedString { .. }));

    // Unterminated escape at EOF
    let bad_esc = r#"let s = "ends with backslash\"#;
    let mut lexer_esc = knr_core::syntax::lexer::Lexer::new(bad_esc);
    let err_esc = lexer_esc.tokenize_all().unwrap_err();
    assert!(matches!(err_esc, ParseError::UnterminatedString { .. }));
}

#[test]
fn test_adv_syntax_comments_and_whitespace_invariance() {
    // Comment at EOF without trailing newline
    let src = "// initial comment\nfn test() -> void {}\n// comment at eof";
    let prog = parse_knr(src).expect("parse with EOF comment");
    assert_eq!(prog.declarations.len(), 1);

    // Comments between operands
    let src2 = r#"
        fn add(
            // comment in params
            a: i64,
            // another comment
            b: i64
        ) -> i64 {
            return a // inline comment
                + // another comment
                b;
        }
    "#;
    let prog2 = parse_knr(src2).expect("parse with inline comments");
    assert_eq!(prog2.declarations.len(), 1);
}

#[test]
fn test_adv_syntax_spans_and_merging_accuracy() {
    let src = "fn calc() -> i64 {\n    let val = 100 + 200;\n    return val;\n}";
    let prog = parse_knr(src).expect("parse spans");
    assert_eq!(prog.span.start, 0);
    assert_eq!(prog.span.line, 1);

    if let knr_core::syntax::ast::Declaration::Function(ref f) = prog.declarations[0] {
        assert_eq!(f.name.span.line, 1);
        assert_eq!(f.body.stmts[0].span.line, 2);
        assert_eq!(f.body.stmts[1].span.line, 3);
    }
}

// ============================================================================
// Target 2: crates/knr-core/src/lower/
// ============================================================================

#[test]
fn test_adv_lower_monotonic_register_synthesis_no_gaps() {
    let src = r#"
        fn complex_math(a: i64, b: i64, c: i64) -> i64 {
            let x = a + b * c - (a - b);
            let y = (x * 2) + (x * 3) - (x / 2);
            return y;
        }
    "#;
    let prog = parse_knr(src).expect("parse");
    let module = lower_program(&prog).expect("lower");
    let func = &module.functions[0];

    // Verify params: %0, %1, %2
    assert_eq!(func.params.len(), 3);
    assert_eq!(func.params[0].0, 0);
    assert_eq!(func.params[1].0, 1);
    assert_eq!(func.params[2].0, 2);

    // Verify all destinations are strictly increasing: %3, %4, %5, ...
    let mut expected_dst = 3;
    for block in &func.blocks {
        for inst in &block.instructions {
            if let Some(dst) = inst.dst {
                assert_eq!(
                    dst, expected_dst,
                    "Monotonic register gap or regression! Expected %{expected_dst}, got %{dst}"
                );
                expected_dst += 1;
            }
        }
    }
}

#[test]
fn test_adv_lower_branch_linear_isolation_mutually_exclusive_consumption() {
    let src = r#"
        schema Packet { 0: len i64, }
        fn drop_packet(p: Packet) -> i64 {
            return p.len;
        }
        fn route_packet(flag: bool) -> i64 {
            let pkt = Packet { len: 64 };
            if flag {
                let r1 = drop_packet(pkt);
                return r1;
            } else {
                let r2 = drop_packet(pkt);
                return r2;
            }
        }
    "#;
    let prog = parse_knr(src).expect("parse");
    let res = lower_program(&prog);
    assert!(
        res.is_ok(),
        "Mutually exclusive branch consumption must succeed without false use-after-move: {:?}",
        res.err()
    );
}

#[test]
fn test_adv_lower_branch_linear_imbalance_rejected() {
    let src = r#"
        schema Box { 0: val i64, }
        fn consume(b: Box) -> i64 { return b.val; }
        fn imbalanced(flag: bool) -> i64 {
            let b = Box { val: 42 };
            if flag {
                let x = consume(b);
            } else {
                // b is alive here, not consumed
            }
            return 0;
        }
    "#;
    let prog = parse_knr(src).expect("parse");
    let res = lower_program(&prog);
    assert!(res.is_err(), "Imbalanced linear branch join must be rejected");
    let err = res.unwrap_err();
    assert!(
        err.to_string().contains("is consumed in one branch") && err.to_string().contains("not in the other"),
        "Unexpected error: {err}"
    );
}

#[test]
fn test_adv_lower_scope_local_autodrop_nested_scopes() {
    let src = r#"
        schema Resource { 0: id i64, }
        fn nested_scopes() -> i64 {
            let r_outer = Resource { id: 1 };
            {
                let r_inner = Resource { id: 2 };
            }
            return 42;
        }
    "#;
    let prog = parse_knr(src).expect("parse");
    let module = lower_program(&prog).expect("lower");
    let func = &module.functions[0];

    // Both r_inner and r_outer must have Op::Free instructions synthesized
    let free_ops: Vec<_> = func.blocks[0]
        .instructions
        .iter()
        .filter_map(|i| match i.op {
            Op::Free(r) => Some(r),
            _ => None,
        })
        .collect();

    assert_eq!(free_ops.len(), 2, "Expected exactly 2 Op::Free for nested scopes");
}

#[test]
fn test_adv_lower_while_loop_cfg_structure() {
    let src = r#"
        fn loop_cfg(limit: i64) -> i64 {
            let counter = 0;
            while counter < limit {
                if counter == 5 {
                    return counter;
                }
            }
            return limit;
        }
    "#;
    let prog = parse_knr(src).expect("parse");
    let module = lower_program(&prog).expect("lower");
    let func = &module.functions[0];

    let block_ids: HashSet<_> = func.blocks.iter().map(|b| b.id).collect();
    assert!(block_ids.contains(&0), "Must have bb0");
    assert!(func.blocks.len() >= 4, "Must have header, body, exit blocks");

    // Verify all branch targets point to valid blocks
    for block in &func.blocks {
        for (tgt, _) in block.terminator.targets() {
            assert!(block_ids.contains(&tgt), "Target bb{tgt} missing");
        }
    }
}

#[test]
fn test_adv_lower_undeclared_syscall_effect_rejected() {
    let src = r#"
        fn send_bad() -> void effects(Ui) {
            syscall Net "net.listen" 8080;
        }
    "#;
    let prog = parse_knr(src).expect("parse");
    let err = lower_program(&prog).unwrap_err();
    assert!(matches!(err, LowerError::UndeclaredEffect { .. }));
}

#[test]
fn test_adv_lower_variable_reassignment_type_mismatch() {
    let src = r#"
        fn bad_assign() -> i64 {
            let x = 42;
            x = true;
            return x;
        }
    "#;
    let prog = parse_knr(src).expect("parse");
    let err = lower_program(&prog).unwrap_err();
    assert!(matches!(err, LowerError::TypeMismatch { .. }));
}

// ============================================================================
// Target 3: crates/knr-core/src/explore.rs
// ============================================================================

#[test]
fn test_adv_explore_direct_circular_import_detection() {
    let temp = TempDir::new("circ_direct");
    let a_path = temp.path().join("a.knr");
    let b_path = temp.path().join("b.knr");

    fs::write(&a_path, "import \"b.knr\";\nfn fa() -> i64 { return 1; }\n").unwrap();
    fs::write(&b_path, "import \"a.knr\";\nfn fb() -> i64 { return 2; }\n").unwrap();

    let res = explore_path(&a_path);
    assert!(res.is_err(), "Direct circular import must be detected");
    match res.unwrap_err() {
        ParseError::CircularImport { cycle } => {
            assert!(cycle.contains(&"a.knr".to_string()));
            assert!(cycle.contains(&"b.knr".to_string()));
        }
        other => panic!("Expected CircularImport error, got {:?}", other),
    }
}

#[test]
fn test_adv_explore_transitive_circular_import_detection() {
    let temp = TempDir::new("circ_transitive");
    let a_path = temp.path().join("a.knr");
    let b_path = temp.path().join("b.knr");
    let c_path = temp.path().join("c.knr");

    fs::write(&a_path, "import \"b.knr\";\nfn fa() -> i64 { return 1; }\n").unwrap();
    fs::write(&b_path, "import \"c.knr\";\nfn fb() -> i64 { return 2; }\n").unwrap();
    fs::write(&c_path, "import \"a.knr\";\nfn fc() -> i64 { return 3; }\n").unwrap();

    let res = explore_path(&a_path);
    assert!(res.is_err(), "Transitive circular import must be detected");
    match res.unwrap_err() {
        ParseError::CircularImport { cycle } => {
            assert!(cycle.contains(&"a.knr".to_string()));
            assert!(cycle.contains(&"b.knr".to_string()));
            assert!(cycle.contains(&"c.knr".to_string()));
        }
        other => panic!("Expected CircularImport error, got {:?}", other),
    }
}

#[test]
fn test_adv_explore_diamond_import_deduplicated() {
    let temp = TempDir::new("diamond_import");
    let d_path = temp.path().join("d.knr");
    let b_path = temp.path().join("b.knr");
    let c_path = temp.path().join("c.knr");
    let a_path = temp.path().join("a.knr");

    fs::write(&d_path, "fn fd() -> i64 { return 4; }\n").unwrap();
    fs::write(&b_path, "import \"d.knr\";\nfn fb() -> i64 { return 2; }\n").unwrap();
    fs::write(&c_path, "import \"d.knr\";\nfn fc() -> i64 { return 3; }\n").unwrap();
    fs::write(&a_path, "import \"b.knr\";\nimport \"c.knr\";\nfn fa() -> i64 { return 1; }\n").unwrap();

    let graph = explore_path(&a_path).expect("Diamond imports must succeed without false circular import");
    let file_paths: Vec<&str> = graph.files.iter().map(|f| f.path.as_str()).collect();
    let d_count = file_paths.iter().filter(|p| p.ends_with("d.knr")).count();
    assert_eq!(d_count, 1, "d.knr must appear exactly once in project graph");
}

#[test]
fn test_adv_explore_knr_lowering_and_raw_file_info() {
    let temp = TempDir::new("knr_explore_meta");
    let file_path = temp.path().join("app.knr");

    let code = r#"
        schema User {
            0: id i64,
            1: name str,
        }

        fn make_user() -> User {
            let u = User { id: 1, name: "Alice" };
            return u;
        }

        fn main() -> i64 {
            let u = make_user();
            return u.id;
        }
    "#;
    fs::write(&file_path, code).unwrap();

    let graph = explore_path(&file_path).expect("explore knr file");
    assert_eq!(graph.files.len(), 1);
    assert_eq!(graph.schemas.len(), 1);
    assert_eq!(graph.schemas[0].name, "User");
    assert_eq!(graph.functions.len(), 2);

    let main_fn = graph.functions.iter().find(|f| f.name == "main").unwrap();
    assert_eq!(main_fn.calls.len(), 1);
    assert_eq!(main_fn.calls[0].callee, "make_user");
}

// ============================================================================
// Target 4: crates/knr-core/src/manifest.rs
// ============================================================================

#[test]
fn test_adv_manifest_glob_expansion_depths_and_patterns() {
    let temp = TempDir::new("ws_glob_depths");
    let root = temp.path();

    // Create directory tree:
    // apps/app1/knr.toml
    // apps/app2/knr.toml
    // packages/core/sub/pkg/knr.toml
    // ignored_dir/no_manifest/
    fs::create_dir_all(root.join("apps/app1")).unwrap();
    fs::create_dir_all(root.join("apps/app2")).unwrap();
    fs::create_dir_all(root.join("packages/core/sub/pkg")).unwrap();
    fs::create_dir_all(root.join("ignored_dir/no_manifest")).unwrap();

    fs::write(root.join("knr.toml"), "[workspace]\nmembers = [\"apps/*\", \"packages/**\"]\n").unwrap();
    fs::write(root.join("apps/app1/knr.toml"), "[package]\nname = \"app1\"\nversion = \"0.1.0\"\n").unwrap();
    fs::write(root.join("apps/app2/knr.toml"), "[package]\nname = \"app2\"\nversion = \"0.1.0\"\n").unwrap();
    fs::write(root.join("packages/core/sub/pkg/knr.toml"), "[package]\nname = \"pkg\"\nversion = \"0.1.0\"\n").unwrap();

    let resolved = resolve_workspace_members(root, &["apps/*".to_string(), "packages/**".to_string()]).unwrap();
    assert_eq!(resolved.len(), 3);
    assert!(resolved.iter().any(|p| p.ends_with("apps/app1")));
    assert!(resolved.iter().any(|p| p.ends_with("apps/app2")));
    assert!(resolved.iter().any(|p| p.ends_with("packages/core/sub/pkg")));
}

#[test]
#[cfg(unix)]
fn test_adv_manifest_symlink_cycle_protection() {
    let temp = TempDir::new("ws_symlink_cycle");
    let root = temp.path();

    fs::create_dir_all(root.join("crates/member1")).unwrap();
    fs::write(root.join("knr.toml"), "[workspace]\nmembers = [\"**\"]\n").unwrap();
    fs::write(root.join("crates/member1/knr.toml"), "[package]\nname = \"m1\"\nversion = \"0.1.0\"\n").unwrap();

    // Create a circular directory symlink
    let loop_link = root.join("crates/loop");
    std::os::unix::fs::symlink(root, &loop_link).unwrap();

    // Resolving recursive glob "**" must terminate safely and not infinite-loop
    let resolved = resolve_workspace_members(root, &["**".to_string()]).unwrap();
    assert_eq!(resolved.len(), 1, "Only member1 should be discovered once; cycle skipped");
    assert!(resolved[0].ends_with("crates/member1"));
}

#[test]
fn test_adv_manifest_control_characters_roundtrip() {
    let mut deps = BTreeMap::new();
    deps.insert("sub_dep".to_string(), DependencySpec::Simple("1.2.3".to_string()));

    let pkg = PackageManifest {
        name: "ctrl-app".to_string(),
        version: "0.1.0".to_string(),
        entry: PathBuf::from("src/main.knr"),
        edition: "2026".to_string(),
        dependencies: deps,
        description: Some("Escapes: \n newline, \t tab, \r return, \" quote, \\ backslash".to_string()),
        authors: vec!["Alice <alice@test.org>".to_string()],
        build: Some(BuildConfig {
            target: Some("native".to_string()),
            opt_level: Some(2),
            out_dir: Some(PathBuf::from("target/out")),
        }),
        ui: Some(UiConfig {
            title: Some("UI with \"Quotes\" and \t Tabs".to_string()),
            width: Some(1280),
            height: Some(720),
            theme: Some("dark".to_string()),
        }),
    };

    let manifest = Manifest::Package(pkg.clone());
    let toml_str = manifest.to_toml();
    let parsed = Manifest::from_toml(&toml_str).expect("parse serialized manifest with special characters");
    assert_eq!(Manifest::Package(pkg), parsed);
}

#[test]
fn test_adv_manifest_nonexistent_explicit_member_error() {
    let temp = TempDir::new("ws_nonexistent");
    let root = temp.path();
    fs::write(root.join("knr.toml"), "[workspace]\nmembers = [\"does_not_exist\"]\n").unwrap();

    let err = resolve_workspace_members(root, &["does_not_exist".to_string()]).unwrap_err();
    assert!(matches!(err, ManifestError::MemberResolution(_)));
}
