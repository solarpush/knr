use knr_core::{
    decode_knrbin, encode_knrbin, format_ssa, load_module_from_path, parse_module, parse_ssa,
    parse_ssa_with_role, FileRole, ParseError,
};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

struct TestDir {
    path: PathBuf,
}

impl TestDir {
    fn new(prefix: &str) -> Self {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let count = COUNTER.fetch_add(1, Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!(
            "knr_test_{prefix}_{}_{}",
            std::process::id(),
            count
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("create test dir");
        Self { path }
    }

    fn write_file(&self, rel: &str, content: &str) -> PathBuf {
        let full_path = self.path.join(rel);
        if let Some(parent) = full_path.parent() {
            std::fs::create_dir_all(parent).expect("create parent dir");
        }
        std::fs::write(&full_path, content).expect("write test file");
        full_path
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

#[test]
fn test_diamond_import_idempotent() {
    let td = TestDir::new("diamond");

    td.write_file(
        "schemas/vdom.ssa.schema",
        r#"schema Leaf {
    0: tag str,
    1: text str,
}

schema Card {
    0: title str,
    1: body str,
}
"#,
    );

    td.write_file(
        "components/nav.ssa",
        r#"import "../schemas/vdom.ssa.schema"

fn build_nav() -> void {
bb0:
    ret
}
"#,
    );

    td.write_file(
        "components/hero.ssa",
        r#"import "../schemas/vdom.ssa.schema"

fn build_hero() -> void {
bb0:
    ret
}
"#,
    );

    let main_path = td.write_file(
        "site.ssa",
        r#"import "components/nav.ssa"
import "components/hero.ssa"

fn main() -> void {
bb0:
    ret
}
"#,
    );

    let module = load_module_from_path(&main_path).expect("load diamond import module");
    assert_eq!(module.imports, vec!["components/nav.ssa", "components/hero.ssa"]);
    assert_eq!(module.schemas.len(), 2);
    assert!(module.get_schema("Leaf").is_some());
    assert!(module.get_schema("Card").is_some());
    assert_eq!(module.functions.len(), 3);
    assert!(module.get_function("build_nav").is_some());
    assert!(module.get_function("build_hero").is_some());
    assert!(module.get_function("main").is_some());
}

#[test]
fn test_circular_import_detection_two_files() {
    let td = TestDir::new("cycle2");

    td.write_file(
        "a.ssa",
        r#"import "b.ssa"

fn fn_a() -> void {
bb0:
    ret
}
"#,
    );

    let b_path = td.write_file(
        "b.ssa",
        r#"import "a.ssa"

fn fn_b() -> void {
bb0:
    ret
}
"#,
    );

    let res = load_module_from_path(&b_path);
    match res {
        Err(ParseError::CircularImport { cycle }) => {
            let cycle_str = cycle.join(" -> ");
            assert!(
                cycle_str.contains("a.ssa -> b.ssa -> a.ssa")
                    || cycle_str.contains("b.ssa -> a.ssa -> b.ssa"),
                "Unexpected cycle string: {cycle_str}"
            );
        }
        other => panic!("Expected CircularImport error, got {other:?}"),
    }
}

#[test]
fn test_circular_import_detection_three_files() {
    let td = TestDir::new("cycle3");

    td.write_file(
        "a.ssa",
        r#"import "b.ssa"
fn fn_a() -> void {
bb0:
    ret
}
"#,
    );
    td.write_file(
        "b.ssa",
        r#"import "c.ssa"
fn fn_b() -> void {
bb0:
    ret
}
"#,
    );
    let c_path = td.write_file(
        "c.ssa",
        r#"import "a.ssa"
fn fn_c() -> void {
bb0:
    ret
}
"#,
    );

    let res = load_module_from_path(&c_path);
    match res {
        Err(ParseError::CircularImport { cycle }) => {
            let cycle_str = cycle.join(" -> ");
            assert!(
                cycle_str.contains("a.ssa")
                    && cycle_str.contains("b.ssa")
                    && cycle_str.contains("c.ssa"),
                "Cycle chain should contain all 3 files: {cycle_str}"
            );
        }
        other => panic!("Expected CircularImport error, got {other:?}"),
    }
}

#[test]
fn test_file_role_schema_cannot_declare_function() {
    let td = TestDir::new("schema_with_fn");

    let path = td.write_file(
        "bad.ssa.schema",
        r#"schema Point {
    0: x i64,
}

fn make_point() -> void {
bb0:
    ret
}
"#,
    );

    let res = load_module_from_path(&path);
    match res {
        Err(ParseError::InvalidConstructInSchemaFile { line, msg }) => {
            assert_eq!(line, 5);
            assert!(msg.contains("Functions cannot be declared in a .ssa.schema file"));
        }
        other => panic!("Expected InvalidConstructInSchemaFile, got {other:?}"),
    }
}

#[test]
fn test_file_role_schema_cannot_import_code_file() {
    let td = TestDir::new("schema_imports_code");

    td.write_file(
        "helper.ssa",
        r#"fn helper() -> void {
bb0:
    ret
}
"#,
    );

    let path = td.write_file(
        "types.ssa.schema",
        r#"import "helper.ssa"

schema Config {
    0: port i64,
}
"#,
    );

    let res = load_module_from_path(&path);
    match res {
        Err(ParseError::InvalidConstructInSchemaFile { line, msg }) => {
            assert_eq!(line, 1);
            assert!(msg.contains("Schema files may only import other '.ssa.schema' files"));
        }
        other => panic!("Expected InvalidConstructInSchemaFile, got {other:?}"),
    }
}

#[test]
fn test_file_role_code_inline_schema_forbidden_with_imports() {
    let td = TestDir::new("code_inline_schema_imports");

    td.write_file(
        "types.ssa.schema",
        r#"schema External {
    0: id i64,
}
"#,
    );

    let path = td.write_file(
        "app.ssa",
        r#"import "types.ssa.schema"

schema Local {
    0: val str,
}

fn run() -> void {
bb0:
    ret
}
"#,
    );

    let res = load_module_from_path(&path);
    match res {
        Err(ParseError::InlineSchemaInCodeFile { line, name, fix_hint }) => {
            assert_eq!(line, 3);
            assert_eq!(name, "Local");
            assert_eq!(
                fix_hint,
                "Move schema 'Local' into a '.ssa.schema' file and import it with 'import \"...\"'"
            );
        }
        other => panic!("Expected InlineSchemaInCodeFile, got {other:?}"),
    }
}

#[test]
fn test_file_role_code_inline_schema_forbidden_disk() {
    let td = TestDir::new("code_inline_schema_disk");

    let path = td.write_file(
        "app.ssa",
        r#"schema Single {
    0: count i64,
}

fn run() -> void {
bb0:
    ret
}
"#,
    );

    let res = load_module_from_path(&path);
    match res {
        Err(ParseError::InlineSchemaInCodeFile { line, name, fix_hint }) => {
            assert_eq!(line, 1);
            assert_eq!(name, "Single");
            assert_eq!(
                fix_hint,
                "Move schema 'Single' into a '.ssa.schema' file and import it with 'import \"...\"'"
            );
        }
        other => panic!("Expected InlineSchemaInCodeFile, got {other:?}"),
    }
}

#[test]
fn test_legacy_parse_ssa_string_backward_compatibility() {
    let src = r#"schema Point {
    0: x i64,
    1: y i64,
}

fn main() -> i64 {
bb0:
    %0 = const_i64 42
    ret %0
}
"#;

    let module = parse_ssa(src).expect("legacy parse_ssa must succeed with inline schema");
    assert_eq!(module.schemas.len(), 1);
    assert_eq!(module.functions.len(), 1);
    assert!(module.imports.is_empty());

    let module_alias = parse_module(src).expect("parse_module must succeed identically");
    assert_eq!(module, module_alias);
}

#[test]
fn test_duplicate_schema_collision() {
    let td = TestDir::new("duplicate_schema");

    td.write_file(
        "first.ssa.schema",
        r#"schema Point {
    0: x i64,
}
"#,
    );

    td.write_file(
        "second.ssa.schema",
        r#"schema Point {
    0: x i64,
    1: y i64,
}
"#,
    );

    let main_path = td.write_file(
        "main.ssa",
        r#"import "first.ssa.schema"
import "second.ssa.schema"

fn main() -> void {
bb0:
    ret
}
"#,
    );

    let res = load_module_from_path(&main_path);
    match res {
        Err(ParseError::DuplicateSymbol { symbol, first_file, second_file }) => {
            assert_eq!(symbol, "Point");
            assert!(first_file.contains("first.ssa.schema"));
            assert!(second_file.contains("second.ssa.schema"));
        }
        other => panic!("Expected DuplicateSymbol error, got {other:?}"),
    }
}

#[test]
fn test_duplicate_function_collision() {
    let td = TestDir::new("duplicate_fn");

    td.write_file(
        "a.ssa",
        r#"fn calculate() -> i64 {
bb0:
    %0 = const_i64 1
    ret %0
}
"#,
    );

    td.write_file(
        "b.ssa",
        r#"fn calculate() -> i64 {
bb0:
    %0 = const_i64 2
    ret %0
}
"#,
    );

    let main_path = td.write_file(
        "main.ssa",
        r#"import "a.ssa"
import "b.ssa"

fn main() -> void {
bb0:
    ret
}
"#,
    );

    let res = load_module_from_path(&main_path);
    match res {
        Err(ParseError::DuplicateSymbol { symbol, first_file, second_file }) => {
            assert_eq!(symbol, "calculate");
            assert!(first_file.contains("a.ssa"));
            assert!(second_file.contains("b.ssa"));
        }
        other => panic!("Expected DuplicateSymbol error, got {other:?}"),
    }
}

#[test]
fn test_import_after_declaration_rejected() {
    let td = TestDir::new("import_after_decl");

    let path = td.write_file(
        "invalid.ssa",
        r#"fn main() -> void {
bb0:
    ret
}

import "other.ssa"
"#,
    );

    let res = load_module_from_path(&path);
    match res {
        Err(ParseError::ImportAfterDeclaration { line }) => {
            assert_eq!(line, 6);
        }
        other => panic!("Expected ImportAfterDeclaration error, got {other:?}"),
    }

    // Direct parser verification
    let content = r#"schema Foo {
    0: val i64,
}
import "bar.ssa.schema"
"#;
    let res = parse_ssa_with_role(content, FileRole::Schema);
    match res {
        Err(ParseError::ImportAfterDeclaration { line }) => {
            assert_eq!(line, 4);
        }
        other => panic!("Expected ImportAfterDeclaration, got {other:?}"),
    }
}

#[test]
fn test_file_not_found_error() {
    let td = TestDir::new("missing_file");

    let path = td.write_file(
        "main.ssa",
        r#"import "nonexistent_file.ssa.schema"

fn main() -> void {
bb0:
    ret
}
"#,
    );

    let res = load_module_from_path(&path);
    match res {
        Err(ParseError::FileNotFound { line, path: missing }) => {
            assert_eq!(line, 1);
            assert!(missing.contains("nonexistent_file.ssa.schema"));
        }
        other => panic!("Expected FileNotFound, got {other:?}"),
    }
}

#[test]
fn test_topological_schema_resolution() {
    let td = TestDir::new("topological_schema");

    td.write_file(
        "schemas/point.ssa.schema",
        r#"schema Point {
    0: x i64,
    1: y i64,
}
"#,
    );

    let comp_path = td.write_file(
        "components/reader.ssa",
        r#"import "../schemas/point.ssa.schema"

fn get_x(%0: struct[Point]) -> i64 {
bb0:
    %1 = struct_get %0 x
    ret %1
}
"#,
    );

    let module = load_module_from_path(&comp_path).expect("schema must be available when parsing struct_get");
    assert_eq!(module.schemas.len(), 1);
    assert_eq!(module.functions.len(), 1);
    assert_eq!(module.schemas[0].name, "Point");
}

#[test]
fn test_roundtrip_formatting_with_imports() {
    let src = r#"import "schemas/vdom.ssa.schema"
import "schemas/http.ssa.schema"

fn main() -> void {
bb0:
    ret
}
"#;

    let module = parse_ssa_with_role(src, FileRole::Code).expect("parse code with imports");
    assert_eq!(
        module.imports,
        vec!["schemas/vdom.ssa.schema", "schemas/http.ssa.schema"]
    );
    assert_eq!(module.functions.len(), 1);

    let formatted = format_ssa(&module);
    assert_eq!(formatted, src);

    let reparsed = parse_ssa_with_role(&formatted, FileRole::Code).expect("re-parse formatted SSA");
    assert_eq!(module, reparsed);
}

#[test]
fn test_circular_import_self_import() {
    let td = TestDir::new("cycle_self");
    let path = td.write_file(
        "self_ref.ssa",
        r#"import "self_ref.ssa"

fn loop_fn() -> void {
bb0:
    ret
}
"#,
    );

    let res = load_module_from_path(&path);
    match res {
        Err(ParseError::CircularImport { cycle }) => {
            assert_eq!(cycle, vec!["self_ref.ssa", "self_ref.ssa"]);
        }
        other => panic!("Expected CircularImport error, got {other:?}"),
    }
}

#[test]
fn test_circular_import_four_files() {
    let td = TestDir::new("cycle4");
    td.write_file(
        "a.ssa",
        r#"import "b.ssa"
fn fa() -> void {
bb0:
    ret
}
"#,
    );
    td.write_file(
        "b.ssa",
        r#"import "c.ssa"
fn fb() -> void {
bb0:
    ret
}
"#,
    );
    td.write_file(
        "c.ssa",
        r#"import "d.ssa"
fn fc() -> void {
bb0:
    ret
}
"#,
    );
    let d_path = td.write_file(
        "d.ssa",
        r#"import "a.ssa"
fn fd() -> void {
bb0:
    ret
}
"#,
    );

    let res = load_module_from_path(&d_path);
    match res {
        Err(ParseError::CircularImport { cycle }) => {
            assert_eq!(cycle.len(), 5);
            assert_eq!(cycle[0], cycle[4]);
            let s = cycle.join(" -> ");
            assert!(s.contains("a.ssa") && s.contains("b.ssa") && s.contains("c.ssa") && s.contains("d.ssa"));
        }
        other => panic!("Expected CircularImport error, got {other:?}"),
    }
}

#[test]
fn test_complex_multilevel_relative_diamond_import() {
    let td = TestDir::new("complex_diamond");

    td.write_file(
        "schemas/common.ssa.schema",
        r#"schema SharedConfig {
    0: timeout i64,
}
"#,
    );

    td.write_file(
        "sub1/comp1.ssa",
        r#"import "../schemas/common.ssa.schema"

fn fn_comp1() -> void {
bb0:
    ret
}
"#,
    );

    td.write_file(
        "sub1/sub2/comp2.ssa",
        r#"import "../../schemas/common.ssa.schema"

fn fn_comp2() -> void {
bb0:
    ret
}
"#,
    );

    td.write_file(
        "sub1/sub2/sub3/comp3.ssa",
        r#"import "../../../schemas/common.ssa.schema"

fn fn_comp3() -> void {
bb0:
    ret
}
"#,
    );

    let main_path = td.write_file(
        "main.ssa",
        r#"import "sub1/comp1.ssa"
import "sub1/sub2/comp2.ssa"
import "sub1/sub2/sub3/comp3.ssa"

fn main() -> void {
bb0:
    ret
}
"#,
    );

    let module = load_module_from_path(&main_path).expect("multilevel relative diamond import should succeed");
    assert_eq!(module.schemas.len(), 1);
    assert_eq!(module.schemas[0].name, "SharedConfig");
    assert_eq!(module.functions.len(), 4);
    assert!(module.get_function("fn_comp1").is_some());
    assert!(module.get_function("fn_comp2").is_some());
    assert!(module.get_function("fn_comp3").is_some());
    assert!(module.get_function("main").is_some());
}

#[test]
fn test_duplicate_symbol_in_same_file() {
    let td = TestDir::new("dup_same_file");

    let schema_path = td.write_file(
        "dup.ssa.schema",
        r#"schema Widget {
    0: id i64,
}
schema Widget {
    0: id i64,
    1: label str,
}
"#,
    );

    let res = load_module_from_path(&schema_path);
    match res {
        Err(ParseError::DuplicateSymbol { symbol, first_file, second_file }) => {
            assert_eq!(symbol, "Widget");
            assert_eq!(first_file, second_file);
        }
        other => panic!("Expected DuplicateSymbol for schema in same file, got {other:?}"),
    }

    let code_path = td.write_file(
        "dup_fn.ssa",
        r#"fn do_work() -> void {
bb0:
    ret
}

fn do_work() -> void {
bb0:
    ret
}
"#,
    );

    let res = load_module_from_path(&code_path);
    match res {
        Err(ParseError::DuplicateSymbol { symbol, first_file, second_file }) => {
            assert_eq!(symbol, "do_work");
            assert_eq!(first_file, second_file);
        }
        other => panic!("Expected DuplicateSymbol for function in same file, got {other:?}"),
    }
}

#[test]
fn test_root_file_not_found() {
    let non_existent = std::path::Path::new("/tmp/definitely_not_existing_knr_file_xyz.ssa");
    let res = load_module_from_path(non_existent);
    match res {
        Err(ParseError::FileNotFound { line, path }) => {
            assert_eq!(line, 1);
            assert!(path.contains("definitely_not_existing_knr_file_xyz.ssa"));
        }
        other => panic!("Expected FileNotFound for root file, got {other:?}"),
    }
}

#[test]
fn test_invalid_file_extension_rejected() {
    let td = TestDir::new("invalid_ext");
    let bad_ext = td.write_file(
        "module.txt",
        r#"fn foo() -> void {
bb0:
    ret
}
"#,
    );

    let res = load_module_from_path(&bad_ext);
    match res {
        Err(ParseError::InvalidSyntax { line, msg }) => {
            assert_eq!(line, 1);
            assert!(msg.contains("invalid extension; expected '.ssa' or '.ssa.schema'"));
        }
        other => panic!("Expected InvalidSyntax for invalid extension, got {other:?}"),
    }
}

#[test]
fn test_diamond_import_code_files_idempotent() {
    let td = TestDir::new("diamond_code");

    td.write_file(
        "shared.ssa",
        r#"fn shared_calc() -> void {
bb0:
    ret
}
"#,
    );

    td.write_file(
        "left.ssa",
        r#"import "shared.ssa"

fn left_calc() -> void {
bb0:
    ret
}
"#,
    );

    td.write_file(
        "right.ssa",
        r#"import "shared.ssa"

fn right_calc() -> void {
bb0:
    ret
}
"#,
    );

    let main_path = td.write_file(
        "main.ssa",
        r#"import "left.ssa"
import "right.ssa"

fn main() -> void {
bb0:
    ret
}
"#,
    );

    let module = load_module_from_path(&main_path).expect("diamond import of code files must deduplicate cleanly");
    assert_eq!(module.functions.len(), 4);
    assert!(module.get_function("shared_calc").is_some());
    assert!(module.get_function("left_calc").is_some());
    assert!(module.get_function("right_calc").is_some());
    assert!(module.get_function("main").is_some());
}

#[test]
fn test_schema_file_can_import_other_schema_and_declare_schemas() {
    let td = TestDir::new("schema_import_schema");

    td.write_file(
        "base.ssa.schema",
        r#"schema Base {
    0: id i64,
}
"#,
    );

    let derived_path = td.write_file(
        "derived.ssa.schema",
        r#"import "base.ssa.schema"

schema Derived {
    0: base struct[Base],
    1: name str,
}
"#,
    );

    let module = load_module_from_path(&derived_path).expect("schema file importing another schema file should load successfully");
    assert_eq!(module.schemas.len(), 2);
    assert!(module.get_schema("Base").is_some());
    assert!(module.get_schema("Derived").is_some());
}

#[test]
fn test_schema_role_format_and_reparse_with_imports() {
    let src = r#"import "base.ssa.schema"

schema Derived {
    0: name str,
}
"#;

    let module = parse_ssa_with_role(src, FileRole::Schema).expect("parse schema with import");
    assert_eq!(module.imports, vec!["base.ssa.schema"]);
    assert_eq!(module.schemas.len(), 1);

    let formatted = format_ssa(&module);
    assert_eq!(formatted, src);

    let reparsed = parse_ssa_with_role(&formatted, FileRole::Schema).expect("re-parse schema with import");
    assert_eq!(module, reparsed);
}

#[test]
fn test_knrbin_roundtrip_module_from_loader() {
    let td = TestDir::new("knrbin_loader_roundtrip");

    td.write_file(
        "schemas/point.ssa.schema",
        r#"schema Point {
    0: x i64,
}
"#,
    );

    let main_path = td.write_file(
        "main.ssa",
        r#"import "schemas/point.ssa.schema"

fn main() -> i64 {
bb0:
    %0 = const_i64 42
    ret %0
}
"#,
    );

    let module = load_module_from_path(&main_path).expect("load module");
    assert_eq!(module.imports, vec!["schemas/point.ssa.schema"]);

    let bin = encode_knrbin(&module).expect("encode knrbin");
    let decoded = decode_knrbin(&bin).expect("decode knrbin");

    // Schemas and function logic are preserved identically in bytecode:
    assert_eq!(module.schemas, decoded.schemas);
    assert_eq!(module.functions.len(), decoded.functions.len());
    for (m_fn, d_fn) in module.functions.iter().zip(decoded.functions.iter()) {
        assert_eq!(m_fn.name, d_fn.name);
        assert_eq!(m_fn.params, d_fn.params);
        assert_eq!(m_fn.ret_ty, d_fn.ret_ty);
        assert_eq!(m_fn.effects, d_fn.effects);
        assert_eq!(m_fn.blocks.len(), d_fn.blocks.len());
        for (m_blk, d_blk) in m_fn.blocks.iter().zip(d_fn.blocks.iter()) {
            assert_eq!(m_blk.id, d_blk.id);
            assert_eq!(m_blk.terminator, d_blk.terminator);
            assert_eq!(m_blk.instructions.len(), d_blk.instructions.len());
            for (m_inst, d_inst) in m_blk.instructions.iter().zip(d_blk.instructions.iter()) {
                assert_eq!(m_inst.dst, d_inst.dst);
                assert_eq!(m_inst.ty, d_inst.ty);
                assert_eq!(m_inst.op, d_inst.op);
            }
        }
    }

    // Documented distinction: .knrbin is a self-contained assembled binary, so decoded.imports is empty
    assert_eq!(decoded.imports, Vec::<String>::new());
}

