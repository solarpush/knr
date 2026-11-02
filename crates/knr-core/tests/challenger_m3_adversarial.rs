use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use knr_core::{explore_path, ExploreFilter, ExploreOutputFormat, ParseError};

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(prefix: &str) -> Self {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let id = COUNTER.fetch_add(1, Ordering::SeqCst);
        let pid = std::process::id();
        let dir = std::env::temp_dir().join(format!("knr_test_adv_{prefix}_{pid}_{id}"));
        fs::create_dir_all(&dir).expect("failed to create temp dir");
        Self { path: dir }
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

// ============================================================================
// 1. Complex Diamond & Multi-file Dependencies
// ============================================================================

#[test]
fn test_diamond_imports_in_file_mode() {
    let temp = TempDir::new("diamond");
    let schema_file = temp.path().join("base.ssa.schema");
    let net_file = temp.path().join("net_mod.ssa");
    let app_file = temp.path().join("app_mod.ssa");
    let root_file = temp.path().join("root.ssa");

    fs::write(
        &schema_file,
        r#"
schema Config {
    0: port i64,
    1: host str,
}
"#,
    )
    .unwrap();

    fs::write(
        &net_file,
        r#"
import "base.ssa.schema"

fn make_config() -> struct[Config] {
bb0:
    %0 = struct_new Config
    %1 = const_i64 8080
    %2 = struct_set %0 0 %1
    ret %2
}
"#,
    )
    .unwrap();

    fs::write(
        &app_file,
        r#"
import "base.ssa.schema"

fn read_config(%0: struct[Config]) -> i64 {
bb0:
    %1 = struct_get %0 0
    ret %1
}
"#,
    )
    .unwrap();

    fs::write(
        &root_file,
        r#"
import "net_mod.ssa"
import "app_mod.ssa"

fn main() -> i64 {
bb0:
    %0 = call make_config
    %1 = call read_config %0
    ret %1
}
"#,
    )
    .unwrap();

    let graph = explore_path(&root_file).expect("diamond import explore_path should succeed");

    // Must discover all 4 files
    assert_eq!(graph.files.len(), 4, "Expected 4 files in diamond closure");
    let file_paths: Vec<&str> = graph.files.iter().map(|f| f.path.as_str()).collect();
    assert!(file_paths.iter().any(|p| p.contains("root.ssa")));
    assert!(file_paths.iter().any(|p| p.contains("net_mod.ssa")));
    assert!(file_paths.iter().any(|p| p.contains("app_mod.ssa")));
    assert!(file_paths.iter().any(|p| p.contains("base.ssa.schema")));

    // Config schema must appear once with 2 fields
    let config_schemas: Vec<&knr_core::SchemaNode> =
        graph.schemas.iter().filter(|s| s.name == "Config").collect();
    assert_eq!(config_schemas.len(), 1, "Config schema must be defined exactly once");
    let cfg = config_schemas[0];
    assert_eq!(cfg.fields.len(), 2);

    // Cross-reference usages of Config
    let op_kinds: Vec<&str> = cfg.usages.iter().map(|u| u.op_kind.as_str()).collect();
    assert!(op_kinds.contains(&"ret"), "make_config ret type usage");
    assert!(op_kinds.contains(&"param"), "read_config param type usage");
    assert!(op_kinds.contains(&"struct_new"), "struct_new Config usage");
    assert!(op_kinds.contains(&"struct_set#0"), "struct_set Config#0 usage");
    assert!(op_kinds.contains(&"struct_get#0"), "struct_get Config#0 usage");

    // Calls in main()
    let main_fn = graph.functions.iter().find(|f| f.name == "main").expect("main fn");
    let callees: Vec<&str> = main_fn.calls.iter().map(|c| c.callee.as_str()).collect();
    assert!(callees.contains(&"make_config"));
    assert!(callees.contains(&"read_config"));
}

// ============================================================================
// 2. Circular Import Prevention & Detection
// ============================================================================

#[test]
fn test_circular_imports_detection_exact_cycle() {
    let temp = TempDir::new("cycle3");
    let a_file = temp.path().join("a.ssa");
    let b_file = temp.path().join("b.ssa");
    let c_file = temp.path().join("c.ssa");

    fs::write(&a_file, "import \"b.ssa\"\nfn a() -> i64 {\nbb0:\nret 0\n}\n").unwrap();
    fs::write(&b_file, "import \"c.ssa\"\nfn b() -> i64 {\nbb0:\nret 0\n}\n").unwrap();
    fs::write(&c_file, "import \"a.ssa\"\nfn c() -> i64 {\nbb0:\nret 0\n}\n").unwrap();

    match explore_path(&a_file) {
        Err(ParseError::CircularImport { cycle }) => {
            assert!(
                cycle.len() >= 3,
                "Expected circular import cycle chain, got: {:?}",
                cycle
            );
            assert_eq!(cycle.first(), cycle.last(), "Cycle must loop back to origin");
        }
        other => panic!("Expected CircularImport error, got {:?}", other),
    }
}

#[test]
fn test_self_import_detection() {
    let temp = TempDir::new("self_import");
    let self_file = temp.path().join("self_ref.ssa");
    fs::write(
        &self_file,
        "import \"self_ref.ssa\"\nfn main() -> i64 {\nbb0:\nret 0\n}\n",
    )
    .unwrap();

    match explore_path(&self_file) {
        Err(ParseError::CircularImport { cycle }) => {
            assert_eq!(cycle, vec!["self_ref.ssa", "self_ref.ssa"]);
        }
        other => panic!("Expected CircularImport on self-import, got {:?}", other),
    }
}

#[test]
fn test_schema_cycle_import_detection() {
    let temp = TempDir::new("schema_cycle");
    let s1_file = temp.path().join("s1.ssa.schema");
    let s2_file = temp.path().join("s2.ssa.schema");

    fs::write(&s1_file, "import \"s2.ssa.schema\"\nschema S1 { 0: x i64 }\n").unwrap();
    fs::write(&s2_file, "import \"s1.ssa.schema\"\nschema S2 { 0: y i64 }\n").unwrap();

    match explore_path(&s1_file) {
        Err(ParseError::CircularImport { cycle }) => {
            assert_eq!(cycle.first(), cycle.last());
        }
        other => panic!("Expected CircularImport between schemas, got {:?}", other),
    }
}

// ============================================================================
// 3. Cycle Prevention in BFS Caller/Callee Traversal
// ============================================================================

#[test]
fn test_transitive_callers_complex_mutual_and_self_recursion() {
    let temp = TempDir::new("transitive_callers_complex");
    let file = temp.path().join("graph.ssa");

    // Call topology:
    // root -> f1
    // f1 -> f2, f1 (self-recursion)
    // f2 -> f3, f1 (mutual cycle)
    // f3 -> f2, leaf (mutual cycle)
    // leaf (terminal)
    let ssa = r#"
fn root() -> i64 {
bb0:
    %0 = call f1
    ret %0
}

fn f1() -> i64 {
bb0:
    %0 = call f2
    %1 = call f1
    ret %0
}

fn f2() -> i64 {
bb0:
    %0 = call f3
    %1 = call f1
    ret %0
}

fn f3() -> i64 {
bb0:
    %0 = call f2
    %1 = call leaf
    ret %1
}

fn leaf() -> i64 {
bb0:
    %0 = const_i64 42
    ret %0
}
"#;
    fs::write(&file, ssa).unwrap();

    let graph = explore_path(temp.path()).expect("explore graph.ssa");

    // 1. Transitive callers of 'leaf'
    let callers = graph.get_transitive_callers("leaf");
    assert!(!callers.is_empty());
    let caller_names: Vec<&str> = callers.iter().map(|c| c.caller.as_str()).collect();

    // Must find: f3 (depth 1), f2 (depth 2), f1 (depth 3), root (depth 4)
    assert!(caller_names.contains(&"f3"));
    assert!(caller_names.contains(&"f2"));
    assert!(caller_names.contains(&"f1"));
    assert!(caller_names.contains(&"root"));

    let f3_c = callers.iter().find(|c| c.caller == "f3").unwrap();
    assert_eq!(f3_c.depth, 1);

    let f2_c = callers.iter().find(|c| c.caller == "f2").unwrap();
    assert_eq!(f2_c.depth, 2);

    let f1_c = callers.iter().find(|c| c.caller == "f1").unwrap();
    assert_eq!(f1_c.depth, 3);

    let root_c = callers.iter().find(|c| c.caller == "root").unwrap();
    assert_eq!(root_c.depth, 4);

    // 2. Transitive callees of 'root'
    let report = graph.get_transitive_callees("root");
    let callee_names: Vec<&str> = report.callees.iter().map(|c| c.callee.as_str()).collect();
    assert!(callee_names.contains(&"f1"));
    assert!(callee_names.contains(&"f2"));
    assert!(callee_names.contains(&"f3"));
    assert!(callee_names.contains(&"leaf"));

    let f1_edge = report.callees.iter().find(|c| c.callee == "f1").unwrap();
    assert_eq!(f1_edge.depth, 1);

    let f2_edge = report.callees.iter().find(|c| c.callee == "f2").unwrap();
    assert_eq!(f2_edge.depth, 2);

    let f3_edge = report.callees.iter().find(|c| c.callee == "f3").unwrap();
    assert_eq!(f3_edge.depth, 3);

    let leaf_edge = report.callees.iter().find(|c| c.callee == "leaf").unwrap();
    assert_eq!(leaf_edge.depth, 4);
}

#[test]
fn test_transitive_callees_unresolved_external_call() {
    let temp = TempDir::new("external_callee");
    let file = temp.path().join("main.ssa");
    let ssa = r#"
fn main() -> i64 {
bb0:
    %0 = call unknown_external_fn
    ret %0
}
"#;
    fs::write(&file, ssa).unwrap();

    let graph = explore_path(temp.path()).expect("explore main.ssa");
    let report = graph.get_transitive_callees("main");
    assert_eq!(report.callees.len(), 1);
    let edge = &report.callees[0];
    assert_eq!(edge.caller, "main");
    assert_eq!(edge.callee, "unknown_external_fn");
    assert_eq!(edge.def_file, "<external>");
    assert_eq!(edge.def_line, 0);
    assert_eq!(edge.depth, 1);
}

// ============================================================================
// 4. Deep Call Graph Stress Test
// ============================================================================

#[test]
fn test_deep_transitive_call_chain_50_levels() {
    let temp = TempDir::new("deep_call_chain");
    let file = temp.path().join("deep.ssa");

    let mut ssa = String::new();
    for i in 0..50 {
        if i == 49 {
            ssa.push_str(&format!(
                "fn f{i}() -> i64 {{\nbb0:\n    %0 = const_i64 42\n    ret %0\n}}\n\n"
            ));
        } else {
            let next = i + 1;
            ssa.push_str(&format!(
                "fn f{i}() -> i64 {{\nbb0:\n    %0 = call f{next}\n    ret %0\n}}\n\n"
            ));
        }
    }
    fs::write(&file, ssa).unwrap();

    let graph = explore_path(&file).expect("explore deep.ssa");

    // Callees from f0
    let report = graph.get_transitive_callees("f0");
    assert_eq!(report.callees.len(), 49, "f0 must reach all 49 callees");
    for (idx, edge) in report.callees.iter().enumerate() {
        let expected_callee = format!("f{}", idx + 1);
        assert_eq!(edge.callee, expected_callee);
        assert_eq!(edge.depth, idx + 1);
    }

    // Callers of f49
    let callers = graph.get_transitive_callers("f49");
    assert_eq!(callers.len(), 49, "f49 must have all 49 callers");
    for (idx, caller) in callers.iter().enumerate() {
        let expected_caller = format!("f{}", 48 - idx);
        assert_eq!(caller.caller, expected_caller);
        assert_eq!(caller.depth, idx + 1);
    }
}

// ============================================================================
// 5. Mermaid Escaping and Valid Syntax
// ============================================================================

#[test]
fn test_mermaid_special_characters_escaping() {
    let temp = TempDir::new("mermaid_special");
    let sub = temp.path().join("sub-module.v1_alpha");
    fs::create_dir_all(&sub).unwrap();

    let schema_file = sub.join("data-schema.ssa.schema");
    let code_file = sub.join("proc-work.ssa");

    fs::write(
        &schema_file,
        r#"
schema ComplexData {
    0: id i64,
    1: tags vec[str],
}
"#,
    )
    .unwrap();

    fs::write(
        &code_file,
        r#"
import "data-schema.ssa.schema"

fn process_complex_data(%0: struct[ComplexData]) -> vec[str] {
bb0:
    %1 = struct_get %0 1
    ret %1
}
"#,
    )
    .unwrap();

    let graph = explore_path(temp.path()).expect("explore mermaid project");
    let filter = ExploreFilter::default();
    let mermaid = graph.format_mermaid(&filter);

    assert!(mermaid.starts_with("flowchart TD\n"));

    // Verify subgraphs
    assert!(mermaid.contains("subgraph file_"));
    // Verify node labels enclosed in ["..."]
    assert!(mermaid.contains("[\""));
    assert!(mermaid.contains("\"]"));

    // Verify brackets inside types are safely quoted inside ["..."]
    assert!(mermaid.contains("[\"process_complex_data() -> vec[str]\"]"));

    // Verify no unquoted raw brackets outside node labels
    let lines: Vec<&str> = mermaid.lines().collect();
    for line in lines {
        let trimmed = line.trim();
        if trimmed.starts_with("fn_") || trimmed.starts_with("schema_") {
            assert!(
                trimmed.contains("[\"") && trimmed.contains("\"]"),
                "Node definition must use [\"...\"]: {trimmed}"
            );
        }
    }
}

// ============================================================================
// 6. UI Actions & Struct Operations Tracking
// ============================================================================

#[test]
fn test_ui_actions_extraction_and_route_detection() {
    let temp = TempDir::new("ui_actions");
    let schema_file = temp.path().join("vdom.ssa.schema");
    let code_file = temp.path().join("actions.ssa");

    fs::write(
        &schema_file,
        r#"
schema Leaf {
    0: tag str,
    1: class str,
    2: text str,
    3: action str,
    4: target str,
}
"#,
    )
    .unwrap();

    fs::write(
        &code_file,
        r#"
import "vdom.ssa.schema"

fn build_ui_handlers(%0: str) -> struct[Leaf] {
bb0:
    %1 = struct_new Leaf
    %2 = const_str "button"
    %3 = struct_set %1 0 %2
    %4 = const_str "/api/v2/submit_form"
    %5 = struct_set %3 3 %4

    // Route comparison
    %6 = const_str "/api/v2/status"
    %7 = cmp_eq %0 %6
    br_if %7 bb1 bb2

bb1:
    ret %5

bb2:
    ret %5
}
"#,
    )
    .unwrap();

    let graph = explore_path(&code_file).expect("explore actions.ssa");
    let func = graph
        .functions
        .iter()
        .find(|f| f.name == "build_ui_handlers")
        .expect("build_ui_handlers");

    assert!(
        func.ui_actions.iter().any(|a| a == "/api/v2/submit_form"),
        "Leaf action field 3 string must be captured as UI action"
    );
    assert!(
        func.ui_actions.iter().any(|a| a == "/api/v2/status"),
        "Route compared with cmp_eq must be captured as UI action"
    );
    // Non-action string "button" at field 0 must NOT be in ui_actions
    assert!(
        !func.ui_actions.iter().any(|a| a == "button"),
        "Field 0 'button' must not be recorded as a UI action"
    );
}

#[test]
fn test_struct_ops_tracking_deduplication() {
    let temp = TempDir::new("struct_ops");
    let schema_file = temp.path().join("items.ssa.schema");
    let code_file = temp.path().join("ops.ssa");

    fs::write(
        &schema_file,
        r#"
schema Item {
    0: id i64,
    1: value i64,
}
"#,
    )
    .unwrap();

    fs::write(
        &code_file,
        r#"
import "items.ssa.schema"

fn mutate_item() -> struct[Item] {
bb0:
    %0 = struct_new Item
    %1 = const_i64 1
    %2 = struct_set %0 0 %1
    %3 = struct_get %2 0
    %4 = struct_get %2 0
    %5 = struct_get %2 0
    %6 = const_i64 2
    %7 = struct_set %2 1 %6
    ret %7
}
"#,
    )
    .unwrap();

    let graph = explore_path(&code_file).expect("explore ops.ssa");
    let func = graph.functions.iter().find(|f| f.name == "mutate_item").expect("mutate_item");

    // Deduplication test: struct_get %2 0 executed 3 times, must appear once
    assert_eq!(
        func.struct_ops,
        vec!["new Item", "set Item#0", "get Item#0", "set Item#1"]
    );
}

// ============================================================================
// 7. Complete Orphans Detection
// ============================================================================

#[test]
fn test_orphans_complete_detection() {
    let temp = TempDir::new("orphans_complete");
    let schema_file = temp.path().join("all_schemas.ssa.schema");
    let code_file = temp.path().join("all_code.ssa");

    fs::write(
        &schema_file,
        r#"
schema CompletelyUnused {
    0: dummy i64,
}

schema UsedAsField {
    0: val i64,
}

schema EnclosingSchema {
    0: child struct[UsedAsField],
}

schema UsedInParam {
    0: x i64,
}

schema UsedInRet {
    0: y i64,
}

schema UsedInVec {
    0: z i64,
}
"#,
    )
    .unwrap();

    fs::write(
        &code_file,
        r#"
import "all_schemas.ssa.schema"

fn main() -> i64 {
bb0:
    %0 = call called_by_main
    ret %0
}

fn knr_dispatch(%0: i64) -> i64 {
bb0:
    ret %0
}

fn called_by_main() -> i64 {
bb0:
    %0 = vec_new struct[UsedInVec]
    %1 = const_i64 0
    ret %1
}

fn takes_param(%0: struct[UsedInParam]) -> i64 {
bb0:
    %1 = const_i64 0
    ret %1
}

fn returns_schema() -> struct[UsedInRet] {
bb0:
    %0 = struct_new UsedInRet
    ret %0
}

fn dead_function() -> i64 {
bb0:
    %0 = const_i64 999
    ret %0
}
"#,
    )
    .unwrap();

    let graph = explore_path(temp.path()).expect("explore orphans project");

    // 1. Unused schemas
    assert!(
        graph.orphans.unused_schemas.contains(&"CompletelyUnused".to_string()),
        "CompletelyUnused must be in unused_schemas"
    );
    assert!(
        !graph.orphans.unused_schemas.contains(&"UsedAsField".to_string()),
        "UsedAsField is referenced inside EnclosingSchema, must NOT be orphan"
    );
    assert!(
        !graph.orphans.unused_schemas.contains(&"UsedInParam".to_string()),
        "UsedInParam is used in parameter type, must NOT be orphan"
    );
    assert!(
        !graph.orphans.unused_schemas.contains(&"UsedInRet".to_string()),
        "UsedInRet is used in return type, must NOT be orphan"
    );
    assert!(
        !graph.orphans.unused_schemas.contains(&"UsedInVec".to_string()),
        "UsedInVec is used in vec_new, must NOT be orphan"
    );

    // 2. Uncalled functions
    assert!(
        graph.orphans.uncalled_functions.contains(&"dead_function".to_string()),
        "dead_function has 0 callers, must be in uncalled_functions"
    );
    assert!(
        graph.orphans.uncalled_functions.contains(&"takes_param".to_string()),
        "takes_param has 0 callers, must be in uncalled_functions"
    );
    assert!(
        graph.orphans.uncalled_functions.contains(&"returns_schema".to_string()),
        "returns_schema has 0 callers, must be in uncalled_functions"
    );

    // Entry points must NEVER be reported as orphans
    assert!(
        !graph.orphans.uncalled_functions.contains(&"main".to_string()),
        "'main' must NEVER be reported as uncalled_function"
    );
    assert!(
        !graph.orphans.uncalled_functions.contains(&"knr_dispatch".to_string()),
        "'knr_dispatch' must NEVER be reported as uncalled_function"
    );
    assert!(
        !graph.orphans.uncalled_functions.contains(&"called_by_main".to_string()),
        "'called_by_main' is called by main, must NOT be orphan"
    );
}

// ============================================================================
// 8. Output Formats Serialization Fidelity
// ============================================================================

#[test]
fn test_explore_output_formats_consistency() {
    let temp = TempDir::new("formats");
    let file = temp.path().join("simple.ssa");
    let ssa = r#"
fn main() -> i64 {
bb0:
    %0 = const_i64 0
    ret %0
}
"#;
    fs::write(&file, ssa).unwrap();

    let graph = explore_path(&file).expect("explore simple.ssa");
    let filter = ExploreFilter::default();

    let ai = graph.format(ExploreOutputFormat::Ai, &filter);
    assert!(ai.contains("[TOPOLOGY & IMPORTS]"));
    assert!(ai.contains("[CALL GRAPH & SIGNATURES]"));

    let json = graph.format(ExploreOutputFormat::Json, &filter);
    let val: serde_json::Value = serde_json::from_str(&json).expect("valid json");
    assert!(val.is_object());

    let mermaid = graph.format(ExploreOutputFormat::Mermaid, &filter);
    assert!(mermaid.starts_with("flowchart TD\n"));
}
