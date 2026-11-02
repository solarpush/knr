//! Challenger M4/M5 Adversarial Verification Test Suite
//!
//! Empirical and adversarial verification of:
//! 1. Diamond and cyclic imports:
//!    - site/site.ssa and site/server.ssa diamond dependency resolution without symbol conflicts
//!    - Cyclic imports across 1, 2, 3 files and schemas detected with ParseError::CircularImport
//!    - Duplicate symbol collisions across different imported files detected
//! 2. File topology boundaries:
//!    - Inline schema in .ssa code files rejected with InlineSchemaInCodeFile (loader + CLI)
//!    - Functions in .ssa.schema files rejected with InvalidConstructInSchemaFile (loader + CLI)
//!    - Schema importing code file rejected with InvalidConstructInSchemaFile
//!    - Import after declaration rejected with ImportAfterDeclaration
//! 3. Op::Call opcode semantics:
//!    - Multi-argument passing, register isolation, primitive and struct return values
//!    - Linear ownership transfer: moved linear registers cannot be reused
//!    - Passing same linear register twice to call rejected with UseAfterMove
//!    - Callee returning linear resource marked Alive; unfreed triggers LinearLeak
//!    - Caller discarding linear return triggers LinearLeak / TypeMismatch
//!    - Transitive effects verification
//! 4. Modular site/ CLI execution:
//!    - knr check site/site.ssa and site/server.ssa
//!    - knr asm site/site.ssa -> bytecode disassembly check
//!    - knr run site/site.ssa -> valid HTML matching site/index.html
//!    - site/index.html size and structure validation

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

use knr_core::{
    load_module_from_path, parse_ssa, ParseError,
};
use knr_interp::{execute_module, Value};
use knr_runtime::MockPlatform;
use knr_verify::verify_module;

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(prefix: &str) -> Self {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let id = COUNTER.fetch_add(1, Ordering::SeqCst);
        let pid = std::process::id();
        let path = std::env::temp_dir().join(format!("knr_chal_m4m5_{pid}_{id}_{prefix}"));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("create temp dir");
        Self { path }
    }

    fn write_file(&self, rel: &str, content: &str) -> PathBuf {
        let full = self.path.join(rel);
        if let Some(parent) = full.parent() {
            fs::create_dir_all(parent).expect("create parent dir");
        }
        fs::write(&full, content).expect("write temp file");
        full
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn knr_cmd() -> Command {
    let bin = env!("CARGO_BIN_EXE_knr");
    Command::new(bin)
}

fn repo_root() -> PathBuf {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest_dir.parent().unwrap().parent().unwrap().to_path_buf()
}

// ============================================================================
// 1. Diamond and Cyclic Imports
// ============================================================================

#[test]
fn test_adv_site_site_ssa_diamond_dependency_resolution() {
    let site_ssa = repo_root().join("site").join("site.ssa");
    assert!(site_ssa.exists(), "site/site.ssa must exist");

    // Load via multi-file loader
    let module = load_module_from_path(&site_ssa).expect("load site/site.ssa with diamond imports");

    // site/site.ssa -> page.ssa -> nav.ssa/hero.ssa/pillars.ssa/examples.ssa -> vdom.ssa.schema
    // All 4 schemas must be present exactly once (no duplication)
    assert_eq!(module.schemas.len(), 4, "Must have exactly 4 VDOM schemas");
    assert!(module.get_schema("Leaf").is_some());
    assert!(module.get_schema("Card").is_some());
    assert!(module.get_schema("Section").is_some());
    assert!(module.get_schema("Page").is_some());

    // All 9 functions must be present exactly once
    assert_eq!(module.functions.len(), 9, "Must have exactly 9 functions");
    assert!(module.get_function("main").is_some());
    assert!(module.get_function("render_page").is_some());
    assert!(module.get_function("build_nav").is_some());
    assert!(module.get_function("build_hero").is_some());
    assert!(module.get_function("build_exec_graph").is_some());
    assert!(module.get_function("build_pillars").is_some());
    assert!(module.get_function("build_learn").is_some());
    assert!(module.get_function("build_opcodes").is_some());
    assert!(module.get_function("build_examples").is_some());

    // Verification must pass with 0 diagnostics
    let verify_res = verify_module(&module);
    assert!(verify_res.is_ok(), "site/site.ssa must pass static verification: {:?}", verify_res);
}

#[test]
fn test_adv_site_server_ssa_diamond_and_schemas_resolution() {
    let server_ssa = repo_root().join("site").join("server.ssa");
    assert!(server_ssa.exists(), "site/server.ssa must exist");

    let module = load_module_from_path(&server_ssa).expect("load site/server.ssa with schemas & diamond imports");

    // server.ssa imports http.ssa.schema (3 schemas) and page.ssa (4 schemas via diamond)
    assert_eq!(module.schemas.len(), 7, "Must have 3 HTTP schemas + 4 VDOM schemas");
    assert!(module.get_schema("HttpRequest").is_some());
    assert!(module.get_schema("HttpResponse").is_some());
    assert!(module.get_schema("HttpResponseInt").is_some());
    assert!(module.get_schema("Leaf").is_some());
    assert!(module.get_schema("Card").is_some());
    assert!(module.get_schema("Section").is_some());
    assert!(module.get_schema("Page").is_some());

    // Functions: main + reply_int + reply_text + render_page + 7 components = 11 functions
    assert_eq!(module.functions.len(), 11);
    assert!(module.get_function("main").is_some());
    assert!(module.get_function("render_page").is_some());

    let verify_res = verify_module(&module);
    assert!(verify_res.is_ok(), "site/server.ssa must pass static verification: {:?}", verify_res);
}

#[test]
fn test_adv_cyclic_import_single_file_self_import_cli_and_loader() {
    let td = TempDir::new("cycle_self");
    let file = td.write_file(
        "self.ssa",
        r#"import "self.ssa"
fn foo() -> void {
bb0:
    ret
}
"#,
    );

    // Loader test
    match load_module_from_path(&file) {
        Err(ParseError::CircularImport { cycle }) => {
            assert!(cycle.contains(&"self.ssa".to_string()));
        }
        other => panic!("Expected CircularImport, got {:?}", other),
    }

    // CLI test
    let out = knr_cmd().args(["check", file.to_str().unwrap()]).output().unwrap();
    assert!(!out.status.success(), "knr check on cyclic self-import must fail");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("Circular") || stderr.contains("cycle") || stderr.contains("CircularImport"));
}

#[test]
fn test_adv_cyclic_import_two_files_cli_and_loader() {
    let td = TempDir::new("cycle_two");
    td.write_file(
        "mod_a.ssa",
        r#"import "mod_b.ssa"
fn fa() -> void {
bb0:
    ret
}
"#,
    );
    let file_b = td.write_file(
        "mod_b.ssa",
        r#"import "mod_a.ssa"
fn fb() -> void {
bb0:
    ret
}
"#,
    );

    // Loader test
    match load_module_from_path(&file_b) {
        Err(ParseError::CircularImport { cycle }) => {
            let s = cycle.join(" -> ");
            assert!(s.contains("mod_a.ssa") && s.contains("mod_b.ssa"));
        }
        other => panic!("Expected CircularImport, got {:?}", other),
    }

    // CLI test
    let out = knr_cmd().args(["check", file_b.to_str().unwrap()]).output().unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("Circular") || stderr.contains("cycle") || stderr.contains("CircularImport"));
}

#[test]
fn test_adv_cyclic_import_three_files_ring_cli_and_loader() {
    let td = TempDir::new("cycle_three");
    td.write_file("r1.ssa", "import \"r2.ssa\"\nfn f1() -> void { bb0: ret }\n");
    td.write_file("r2.ssa", "import \"r3.ssa\"\nfn f2() -> void { bb0: ret }\n");
    let file_r3 = td.write_file("r3.ssa", "import \"r1.ssa\"\nfn f3() -> void { bb0: ret }\n");

    match load_module_from_path(&file_r3) {
        Err(ParseError::CircularImport { cycle }) => {
            let s = cycle.join(" -> ");
            assert!(s.contains("r1.ssa") && s.contains("r2.ssa") && s.contains("r3.ssa"));
        }
        other => panic!("Expected CircularImport, got {:?}", other),
    }

    let out = knr_cmd().args(["check", file_r3.to_str().unwrap()]).output().unwrap();
    assert!(!out.status.success());
}

#[test]
fn test_adv_cyclic_import_schemas_cli_and_loader() {
    let td = TempDir::new("cycle_schemas");
    td.write_file(
        "s1.ssa.schema",
        "import \"s2.ssa.schema\"\nschema S1 { 0: x i64, }\n",
    );
    let file_s2 = td.write_file(
        "s2.ssa.schema",
        "import \"s1.ssa.schema\"\nschema S2 { 0: y i64, }\n",
    );

    match load_module_from_path(&file_s2) {
        Err(ParseError::CircularImport { cycle }) => {
            let s = cycle.join(" -> ");
            assert!(s.contains("s1.ssa.schema") && s.contains("s2.ssa.schema"));
        }
        other => panic!("Expected CircularImport, got {:?}", other),
    }

    let out = knr_cmd().args(["check", file_s2.to_str().unwrap()]).output().unwrap();
    assert!(!out.status.success());
}

#[test]
fn test_adv_symbol_collision_duplicate_schema_cli_and_loader() {
    let td = TempDir::new("dup_schema");
    td.write_file("part1.ssa.schema", "schema Collide {\n    0: a i64,\n}\n");
    td.write_file("part2.ssa.schema", "schema Collide {\n    0: b str,\n}\n");
    let main_file = td.write_file(
        "main.ssa",
        "import \"part1.ssa.schema\"\nimport \"part2.ssa.schema\"\nfn main() -> void {\nbb0:\n    ret\n}\n",
    );

    match load_module_from_path(&main_file) {
        Err(ParseError::DuplicateSymbol { symbol, first_file, second_file }) => {
            assert_eq!(symbol, "Collide");
            assert!(first_file.contains("part1.ssa.schema"));
            assert!(second_file.contains("part2.ssa.schema"));
        }
        other => panic!("Expected DuplicateSymbol, got {:?}", other),
    }

    let out = knr_cmd().args(["check", main_file.to_str().unwrap()]).output().unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("DuplicateSymbol") || stderr.contains("Collide"));
}

#[test]
fn test_adv_symbol_collision_duplicate_function_cli_and_loader() {
    let td = TempDir::new("dup_fn");
    td.write_file(
        "worker1.ssa",
        "fn duplicate_task() -> i64 {\nbb0:\n    %0 = const_i64 1\n    ret %0\n}\n",
    );
    td.write_file(
        "worker2.ssa",
        "fn duplicate_task() -> i64 {\nbb0:\n    %0 = const_i64 2\n    ret %0\n}\n",
    );
    let main_file = td.write_file(
        "main.ssa",
        "import \"worker1.ssa\"\nimport \"worker2.ssa\"\nfn main() -> void {\nbb0:\n    ret\n}\n",
    );

    match load_module_from_path(&main_file) {
        Err(ParseError::DuplicateSymbol { symbol, first_file, second_file }) => {
            assert_eq!(symbol, "duplicate_task");
            assert!(first_file.contains("worker1.ssa"));
            assert!(second_file.contains("worker2.ssa"));
        }
        other => panic!("Expected DuplicateSymbol, got {:?}", other),
    }

    let out = knr_cmd().args(["check", main_file.to_str().unwrap()]).output().unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("DuplicateSymbol") || stderr.contains("duplicate_task"));
}

// ============================================================================
// 2. File Topology Boundaries
// ============================================================================

#[test]
fn test_adv_topology_inline_schema_in_code_file_cli_and_loader() {
    let td = TempDir::new("inline_schema_code");
    let file = td.write_file(
        "code.ssa",
        r#"schema InlineForbidden {
    0: id i64,
}

fn compute() -> void {
bb0:
    ret
}
"#,
    );

    // Loader must reject with InlineSchemaInCodeFile
    match load_module_from_path(&file) {
        Err(ParseError::InlineSchemaInCodeFile { line, name, fix_hint }) => {
            assert_eq!(line, 1);
            assert_eq!(name, "InlineForbidden");
            assert!(fix_hint.contains("Move schema 'InlineForbidden' into a '.ssa.schema' file"));
        }
        other => panic!("Expected InlineSchemaInCodeFile, got {:?}", other),
    }

    // CLI must reject with non-zero exit status and informative message
    let out = knr_cmd().args(["check", file.to_str().unwrap()]).output().unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("InlineSchemaInCodeFile") || stderr.contains("Move schema 'InlineForbidden'"));
}

#[test]
fn test_adv_topology_injected_schema_into_site_component_rejected() {
    let td = TempDir::new("inject_site_schema");
    let vdom_orig = fs::read_to_string(repo_root().join("site").join("schemas").join("vdom.ssa.schema")).unwrap();
    td.write_file("schemas/vdom.ssa.schema", &vdom_orig);

    // Copy site/components/nav.ssa and inject an inline schema
    let nav_orig = fs::read_to_string(repo_root().join("site").join("components").join("nav.ssa")).unwrap();
    let corrupted = format!("{nav_orig}\nschema InjectedSchema {{\n    0: bad i64,\n}}\n");
    let corrupted_path = td.write_file("components/nav_bad.ssa", &corrupted);

    match load_module_from_path(&corrupted_path) {
        Err(ParseError::InlineSchemaInCodeFile { name, .. }) => {
            assert_eq!(name, "InjectedSchema");
        }
        other => panic!("Expected InlineSchemaInCodeFile when injecting schema into component, got {:?}", other),
    }

    let out = knr_cmd().args(["check", corrupted_path.to_str().unwrap()]).output().unwrap();
    assert!(!out.status.success());
}

#[test]
fn test_adv_topology_function_in_schema_file_cli_and_loader() {
    let td = TempDir::new("fn_in_schema");
    let file = td.write_file(
        "types.ssa.schema",
        r#"schema GoodSchema {
    0: val i64,
}

fn bad_func() -> void {
bb0:
    ret
}
"#,
    );

    // Loader must reject with InvalidConstructInSchemaFile
    match load_module_from_path(&file) {
        Err(ParseError::InvalidConstructInSchemaFile { line, msg }) => {
            assert_eq!(line, 5);
            assert!(msg.contains("Functions cannot be declared in a .ssa.schema file"));
        }
        other => panic!("Expected InvalidConstructInSchemaFile, got {:?}", other),
    }

    // CLI must reject
    let out = knr_cmd().args(["check", file.to_str().unwrap()]).output().unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("InvalidConstructInSchemaFile") || stderr.contains("Functions cannot be declared"));
}

#[test]
fn test_adv_topology_injected_function_into_site_schema_rejected() {
    let td = TempDir::new("inject_site_fn");
    let vdom_orig = fs::read_to_string(repo_root().join("site").join("schemas").join("vdom.ssa.schema")).unwrap();
    let corrupted = format!("{vdom_orig}\nfn bad_helper() -> void {{\nbb0:\n    ret\n}}\n");
    let corrupted_path = td.write_file("vdom_bad.ssa.schema", &corrupted);

    match load_module_from_path(&corrupted_path) {
        Err(ParseError::InvalidConstructInSchemaFile { msg, .. }) => {
            assert!(msg.contains("Functions cannot be declared in a .ssa.schema file"));
        }
        other => panic!("Expected InvalidConstructInSchemaFile, got {:?}", other),
    }

    let out = knr_cmd().args(["check", corrupted_path.to_str().unwrap()]).output().unwrap();
    assert!(!out.status.success());
}

#[test]
fn test_adv_topology_schema_file_importing_code_file_rejected() {
    let td = TempDir::new("schema_imports_code");
    td.write_file("code.ssa", "fn foo() -> void { bb0: ret }\n");
    let schema_file = td.write_file(
        "bad.ssa.schema",
        "import \"code.ssa\"\nschema S { 0: a i64, }\n",
    );

    match load_module_from_path(&schema_file) {
        Err(ParseError::InvalidConstructInSchemaFile { line, msg }) => {
            assert_eq!(line, 1);
            assert!(msg.contains("Schema files may only import other '.ssa.schema' files"));
        }
        other => panic!("Expected InvalidConstructInSchemaFile, got {:?}", other),
    }

    let out = knr_cmd().args(["check", schema_file.to_str().unwrap()]).output().unwrap();
    assert!(!out.status.success());
}

#[test]
fn test_adv_topology_import_after_declaration_rejected() {
    let td = TempDir::new("import_after_decl");
    let code_file = td.write_file(
        "late_import.ssa",
        r#"fn main() -> void {
bb0:
    ret
}

import "other.ssa"
"#,
    );

    match load_module_from_path(&code_file) {
        Err(ParseError::ImportAfterDeclaration { line }) => {
            assert_eq!(line, 6);
        }
        other => panic!("Expected ImportAfterDeclaration, got {:?}", other),
    }

    let out = knr_cmd().args(["check", code_file.to_str().unwrap()]).output().unwrap();
    assert!(!out.status.success());
}

// ============================================================================
// 3. Op::Call Opcode Semantics
// ============================================================================

#[test]
fn test_adv_call_multilevel_argument_passing_and_struct_return() {
    let td = TempDir::new("call_args_struct");
    td.write_file(
        "geometry.ssa.schema",
        r#"schema Rect {
    0: width i64,
    1: height i64,
    2: area i64,
}
"#,
    );

    let main_path = td.write_file(
        "main.ssa",
        r#"import "geometry.ssa.schema"

fn compute_area(%0: i64, %1: i64) -> i64 {
bb0:
    %2 = mul %0 %1
    ret %2
}

fn make_rect(%0: i64, %1: i64) -> struct[Rect] {
bb0:
    %2 = call compute_area %0 %1
    %3 = struct_new Rect
    %4 = struct_set %3 0 %0
    %5 = struct_set %4 1 %1
    %6 = struct_set %5 2 %2
    ret %6
}

fn main() -> i64 {
bb0:
    %0 = const_i64 7
    %1 = const_i64 6
    %2 = call make_rect %0 %1
    %3 = struct_get %2 2
    free %2
    ret %3
}
"#,
    );

    let module = load_module_from_path(&main_path).expect("load call module");
    assert!(verify_module(&module).is_ok());

    let mut platform = MockPlatform::new();
    let (val, _) = execute_module(&module, "main", vec![], &mut platform, 100, false)
        .expect("execute call module");
    assert_eq!(val, Value::I64(42));
}

#[test]
fn test_adv_call_register_frame_isolation_under_clobbering() {
    let ssa = r#"
fn clobber(%0: i64, %1: i64, %2: i64) -> i64 {
bb0:
    %3 = const_i64 9999
    %4 = add %0 %3
    %5 = add %1 %4
    ret %5
}

fn main() -> i64 {
bb0:
    %0 = const_i64 100
    %1 = const_i64 200
    %2 = const_i64 300
    %3 = call clobber %0 %1 %2
    // If registers were clobbered by callee frame, %0 and %1 would be corrupt.
    // In knr-interp, %0 must still be 100 and %1 must still be 200.
    %4 = add %0 %1
    %5 = add %4 %2
    ret %5
}
"#;
    let module = parse_ssa(ssa).expect("parse SSA");
    assert!(verify_module(&module).is_ok());

    let mut platform = MockPlatform::new();
    let (val, _) = execute_module(&module, "main", vec![], &mut platform, 200, false).expect("exec isolation");
    // %0 (100) + %1 (200) + %2 (300) = 600
    assert_eq!(val, Value::I64(600));
}

#[test]
fn test_adv_call_linear_argument_consumed_use_after_move() {
    let ssa = r#"
fn consume_buffer(%0: buf) -> void {
bb0:
    free %0
    ret
}

fn main() -> void {
bb0:
    %0 = const_i64 64
    %1 = buf_alloc %0
    call consume_buffer %1
    // Second use of %1 must fail with UseAfterMove
    free %1
    ret
}
"#;
    let module = parse_ssa(ssa).expect("parse SSA");
    let errs = verify_module(&module).unwrap_err();
    assert!(
        errs.iter().any(|d| d.error_kind == "UseAfterMove"),
        "Expected UseAfterMove when using buffer after passing to call: {:?}",
        errs
    );
}

#[test]
fn test_adv_call_linear_argument_double_pass_use_after_move() {
    let ssa = r#"
fn consume_two(%0: buf, %1: buf) -> void {
bb0:
    free %0
    free %1
    ret
}

fn main() -> void {
bb0:
    %0 = const_i64 32
    %1 = buf_alloc %0
    call consume_two %1 %1
    ret
}
"#;
    let module = parse_ssa(ssa).expect("parse SSA");
    let errs = verify_module(&module).unwrap_err();
    assert!(
        errs.iter().any(|d| d.error_kind == "UseAfterMove"),
        "Passing same linear buffer twice must trigger UseAfterMove: {:?}",
        errs
    );
}

#[test]
fn test_adv_call_linear_return_leak_detection() {
    let ssa = r#"
fn allocate_buf() -> buf {
bb0:
    %0 = const_i64 128
    %1 = buf_alloc %0
    ret %1
}

fn main() -> void {
bb0:
    // %0 is received Alive as an owned linear resource, but not freed or returned -> LinearLeak
    %0 = call allocate_buf
    ret
}
"#;
    let module = parse_ssa(ssa).expect("parse SSA");
    let errs = verify_module(&module).unwrap_err();
    assert!(
        errs.iter().any(|d| d.error_kind == "LinearLeak"),
        "Unconsumed linear return value must trigger LinearLeak: {:?}",
        errs
    );
}

#[test]
fn test_adv_call_discarding_linear_return_rejected() {
    let ssa = r#"
fn allocate_buf() -> buf {
bb0:
    %0 = const_i64 128
    %1 = buf_alloc %0
    ret %1
}

fn main() -> void {
bb0:
    // Calling allocate_buf without destination discards linear return
    call allocate_buf
    ret
}
"#;
    let module = parse_ssa(ssa).expect("parse SSA");
    let errs = verify_module(&module).unwrap_err();
    assert!(
        errs.iter().any(|d| d.error_kind == "LinearLeak" || d.error_kind == "TypeMismatch"),
        "Discarding linear return without destination must be rejected: {:?}",
        errs
    );
}

#[test]
fn test_adv_call_transitive_effects_enforcement() {
    let ssa = r#"
fn effectful() -> str effects(Ui) {
bb0:
    %0 = const_str "page"
    ret %0
}

fn pure_caller() -> str {
bb0:
    %0 = call effectful
    ret %0
}
"#;
    let module = parse_ssa(ssa).expect("parse SSA");
    let errs = verify_module(&module).unwrap_err();
    assert!(
        errs.iter().any(|d| d.error_kind == "UndeclaredEffect"),
        "Calling effectful function from pure caller must trigger UndeclaredEffect: {:?}",
        errs
    );
}

// ============================================================================
// 4. Modular site/ CLI Execution
// ============================================================================

#[test]
fn test_adv_site_cli_check_site_and_server() {
    let root = repo_root();
    let site_ssa = root.join("site").join("site.ssa");
    let server_ssa = root.join("site").join("server.ssa");

    // 1. Check site.ssa
    let out_site = knr_cmd()
        .args(["check", site_ssa.to_str().unwrap()])
        .output()
        .expect("exec knr check site/site.ssa");
    assert!(
        out_site.status.success(),
        "knr check site/site.ssa must exit 0: stderr: {}",
        String::from_utf8_lossy(&out_site.stderr)
    );

    // 2. Check site.ssa --json
    let out_site_json = knr_cmd()
        .args(["check", site_ssa.to_str().unwrap(), "--json"])
        .output()
        .expect("exec knr check site/site.ssa --json");
    assert!(out_site_json.status.success());
    let stdout_json = String::from_utf8_lossy(&out_site_json.stdout);
    assert_eq!(stdout_json.trim(), "[]", "JSON check must report 0 diagnostics");

    // 3. Check server.ssa
    let out_server = knr_cmd()
        .args(["check", server_ssa.to_str().unwrap()])
        .output()
        .expect("exec knr check site/server.ssa");
    assert!(
        out_server.status.success(),
        "knr check site/server.ssa must exit 0: stderr: {}",
        String::from_utf8_lossy(&out_server.stderr)
    );

    // 4. Check server.ssa --json
    let out_server_json = knr_cmd()
        .args(["check", server_ssa.to_str().unwrap(), "--json"])
        .output()
        .expect("exec knr check site/server.ssa --json");
    assert!(out_server_json.status.success());
    let stdout_srv = String::from_utf8_lossy(&out_server_json.stdout);
    assert_eq!(stdout_srv.trim(), "[]", "JSON check must report 0 diagnostics");
}

#[test]
fn test_adv_site_cli_asm_and_disasm_site_ssa() {
    let root = repo_root();
    let site_ssa = root.join("site").join("site.ssa");

    let td = TempDir::new("site_asm");
    let out_knrbin = td.path().join("test_site.knrbin");

    // Assemble site.ssa -> test_site.knrbin
    let asm_out = knr_cmd()
        .args(["asm", site_ssa.to_str().unwrap(), "-o", out_knrbin.to_str().unwrap()])
        .output()
        .expect("exec knr asm site/site.ssa");
    assert!(
        asm_out.status.success(),
        "knr asm must exit 0: stderr: {}",
        String::from_utf8_lossy(&asm_out.stderr)
    );

    assert!(out_knrbin.exists(), "Compiled knrbin must exist on disk");
    let metadata = fs::metadata(&out_knrbin).expect("get knrbin metadata");
    assert!(metadata.len() > 1000, "knrbin bytecode must be substantial (>1000 bytes, got {})", metadata.len());

    // Disassemble test_site.knrbin
    let disasm_out = knr_cmd()
        .args(["disasm", out_knrbin.to_str().unwrap()])
        .output()
        .expect("exec knr disasm");
    assert!(
        disasm_out.status.success(),
        "knr disasm must exit 0: stderr: {}",
        String::from_utf8_lossy(&disasm_out.stderr)
    );

    let disasm_text = String::from_utf8_lossy(&disasm_out.stdout);
    // Disassembly must contain all schemas and function symbols
    assert!(disasm_text.contains("schema Leaf"), "Disasm must contain Leaf schema");
    assert!(disasm_text.contains("schema Card"), "Disasm must contain Card schema");
    assert!(disasm_text.contains("schema Section"), "Disasm must contain Section schema");
    assert!(disasm_text.contains("schema Page"), "Disasm must contain Page schema");
    assert!(disasm_text.contains("fn main"), "Disasm must contain main function");
    assert!(disasm_text.contains("fn render_page"), "Disasm must contain render_page function");
    assert!(disasm_text.contains("fn build_nav"), "Disasm must contain build_nav function");
    assert!(disasm_text.contains("fn build_hero"), "Disasm must contain build_hero function");
    assert!(disasm_text.contains("fn build_pillars"), "Disasm must contain build_pillars function");
    assert!(disasm_text.contains("fn build_examples"), "Disasm must contain build_examples function");
    assert!(disasm_text.contains("call render_page"), "Disasm must contain call opcode");
}

#[test]
fn test_adv_site_cli_run_generates_valid_html_matching_index_html() {
    let root = repo_root();
    let site_ssa = root.join("site").join("site.ssa");
    let index_html_path = root.join("site").join("index.html");

    // Execute knr run site/site.ssa
    let run_out = knr_cmd()
        .args(["run", site_ssa.to_str().unwrap()])
        .output()
        .expect("exec knr run site/site.ssa");
    assert!(
        run_out.status.success(),
        "knr run site/site.ssa must exit 0: stderr: {}",
        String::from_utf8_lossy(&run_out.stderr)
    );

    let stdout_raw = String::from_utf8_lossy(&run_out.stdout);
    let stdout_str = stdout_raw.trim();

    // Read site/index.html
    let index_content_raw = fs::read_to_string(&index_html_path).expect("read site/index.html");
    let index_str = index_content_raw.trim();

    assert_eq!(
        stdout_str, index_str,
        "knr run site/site.ssa output must match site/index.html verbatim"
    );
}

#[test]
fn test_adv_site_index_html_structural_and_content_integrity() {
    let root = repo_root();
    let index_html_path = root.join("site").join("index.html");
    assert!(index_html_path.exists(), "site/index.html must exist");

    let content = fs::read_to_string(&index_html_path).expect("read site/index.html");
    assert!(content.len() > 5000, "site/index.html must be > 5KB, got {} bytes", content.len());

    // Structural elements
    assert!(content.contains("<main class=\"page\">"));
    assert!(content.contains("<header class=\"nav\">"));
    assert!(content.contains("<title>knr — SSA Linear Language</title>"));
    assert!(content.contains("<style>"));
    assert!(content.contains("<script data-knr-runtime=\"v1\">"));

    // Nav and Branding
    assert!(content.contains("knr // SSA Linear IR"));
    assert!(content.contains("v1.0 • Zero-GC • 29 Opcodes • Self-Hosted UI"));

    // Hero Section
    assert!(content.contains("Le langage SSA à mémoire linéaire pensé pour les Agents IA et le bas niveau."));
    assert!(content.contains("DÉMO INTERACTIVE — COMPTEUR LINÉAIRE (SITE/SERVER.SSA)"));
    assert!(content.contains("counter-val"));
    assert!(content.contains("btn act-inc"));

    // 4 Pillars
    assert!(content.contains("01 / PROPRIÉTÉ LINÉAIRE"));
    assert!(content.contains("02 / JUGE STATIQUE"));
    assert!(content.contains("03 / SYSTÈME D&#39;EFFETS"));
    assert!(content.contains("04 / MULTI-CIBLES &amp; KNIR"));

    // Code Examples
    assert!(content.contains("EXEMPLE 1 — SCHÉMAS STRICTS &amp; TYPES LINÉAIRES"));
    assert!(content.contains("EXEMPLE 2 — BOUCLE HTTP SSR EN KNR (SITE/SERVER.SSA)"));
    assert!(content.contains("Généré à 100% en knr (site/site.ssa) via knr-verify + knr-interp + UiPlatform."));
}
