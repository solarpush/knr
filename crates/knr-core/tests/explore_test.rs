use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use knr_core::{explore_path, ExploreFilter, ParseError};

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(prefix: &str) -> Self {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let id = COUNTER.fetch_add(1, Ordering::SeqCst);
        let pid = std::process::id();
        let dir = std::env::temp_dir().join(format!("knr_test_{prefix}_{pid}_{id}"));
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

fn site_dir() -> PathBuf {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let repo_root = manifest_dir.parent().unwrap().parent().unwrap();
    repo_root.join("site")
}

#[test]
fn test_explore_directory_site() {
    let site = site_dir();
    let graph = explore_path(&site).expect("explore_path on site/ should succeed");

    // 12 files discovered
    assert_eq!(graph.files.len(), 12, "Expected 12 files in site/");

    // Check schemas discovered
    let schema_names: Vec<&str> = graph.schemas.iter().map(|s| s.name.as_str()).collect();
    assert!(schema_names.contains(&"HttpRequest"));
    assert!(schema_names.contains(&"HttpResponse"));
    assert!(schema_names.contains(&"HttpResponseInt"));
    assert!(schema_names.contains(&"Leaf"));
    assert!(schema_names.contains(&"Card"));
    assert!(schema_names.contains(&"Section"));
    assert!(schema_names.contains(&"Page"));

    // Check both main functions coexist
    let mains: Vec<&knr_core::FunctionNode> = graph.functions.iter().filter(|f| f.name == "main").collect();
    assert_eq!(mains.len(), 2, "Expected 2 main functions in site (site.ssa and server.ssa)");

    let site_main = mains.iter().find(|f| f.file.contains("site.ssa")).expect("site.ssa main");
    assert_eq!(site_main.line, 3);
    assert_eq!(site_main.calls.len(), 1);
    assert_eq!(site_main.calls[0].callee, "render_page");

    let server_main = mains.iter().find(|f| f.file.contains("server.ssa")).expect("server.ssa main");
    assert_eq!(server_main.line, 4);
    assert!(server_main.syscalls.len() >= 6, "server.ssa main has multiple syscalls");

    // Check render_page incoming callers
    let render_page = graph.functions.iter().find(|f| f.name == "render_page").expect("render_page");
    let caller_files: Vec<&str> = render_page.called_by.iter().map(|c| c.file.as_str()).collect();
    assert!(caller_files.iter().any(|f| f.contains("site.ssa")));
    assert!(caller_files.iter().any(|f| f.contains("server.ssa")));

    // Check UI actions extracted
    assert!(server_main.ui_actions.iter().any(|a| a == "/api/inc"));
    assert!(server_main.ui_actions.iter().any(|a| a == "/api/dec"));
    assert!(server_main.ui_actions.iter().any(|a| a == "/api/reset"));

    let hero = graph.functions.iter().find(|f| f.name == "build_hero").expect("build_hero");
    assert!(hero.ui_actions.iter().any(|a| a == "/api/dec"));
    assert!(hero.ui_actions.iter().any(|a| a == "/api/inc"));
    assert!(hero.ui_actions.iter().any(|a| a == "/api/reset"));

    // Check schema usages: HttpRequest is used in server.ssa, Leaf/Card/Section/Page are used in components
    let http_req = graph.schemas.iter().find(|s| s.name == "HttpRequest").expect("HttpRequest");
    assert!(!http_req.usages.is_empty(), "HttpRequest is used in server.ssa via net.wait_req struct_get");
    assert!(!graph.orphans.unused_schemas.contains(&"Leaf".to_string()));
    assert!(!graph.orphans.unused_schemas.contains(&"Card".to_string()));
    assert!(!graph.orphans.unused_schemas.contains(&"Section".to_string()));
    assert!(!graph.orphans.unused_schemas.contains(&"Page".to_string()));

    // When exploring http.ssa.schema in isolation, HttpRequest IS an orphan
    let schema_graph = explore_path(&site.join("schemas").join("http.ssa.schema")).expect("explore http.ssa.schema");
    assert!(schema_graph.orphans.unused_schemas.contains(&"HttpRequest".to_string()));

    // main and knr_dispatch must not be reported as uncalled orphans
    assert!(!graph.orphans.uncalled_functions.contains(&"main".to_string()));
    assert!(!graph.orphans.uncalled_functions.contains(&"knr_dispatch".to_string()));
}

#[test]
fn test_explore_single_file_closure() {
    let site_ssa = site_dir().join("site.ssa");
    let graph = explore_path(&site_ssa).expect("explore_path on site.ssa should succeed");

    // Only site.ssa and its imported dependency tree
    let file_paths: Vec<&str> = graph.files.iter().map(|f| f.path.as_str()).collect();
    assert!(file_paths.iter().any(|p| p.contains("site.ssa")));
    assert!(file_paths.iter().any(|p| p.contains("page.ssa")));
    assert!(file_paths.iter().any(|p| p.contains("vdom.ssa.schema")));

    // server.ssa and http.ssa.schema must NOT be included in site.ssa import closure
    assert!(!file_paths.iter().any(|p| p.contains("server.ssa")));
    assert!(!file_paths.iter().any(|p| p.contains("http.ssa.schema")));
}

#[test]
fn test_transitive_callers_with_cycle_protection() {
    let temp = TempDir::new("transitive_callers");
    let file_a = temp.path().join("prog.ssa");

    // a -> b -> c -> b (cycle between b and c)
    let ssa = r#"
fn a() -> i64 {
bb0:
    %0 = call b
    ret %0
}

fn b() -> i64 {
bb0:
    %0 = call c
    ret %0
}

fn c() -> i64 {
bb0:
    %0 = call b
    ret %0
}
"#;
    fs::write(&file_a, ssa).unwrap();

    let graph = explore_path(temp.path()).expect("explore_path on prog.ssa");
    let callers = graph.get_transitive_callers("c");

    // Direct caller: b at depth 1. Transitive caller: a at depth 2.
    assert!(!callers.is_empty());
    let caller_names: Vec<&str> = callers.iter().map(|c| c.caller.as_str()).collect();
    assert!(caller_names.contains(&"b"));
    assert!(caller_names.contains(&"a"));

    let b_caller = callers.iter().find(|c| c.caller == "b").unwrap();
    assert_eq!(b_caller.depth, 1);

    let a_caller = callers.iter().find(|c| c.caller == "a").unwrap();
    assert_eq!(a_caller.depth, 2);
}

#[test]
fn test_transitive_callees_and_syscalls() {
    let site = site_dir();
    let graph = explore_path(&site).expect("explore site");

    let report = graph.get_transitive_callees("main");
    assert!(!report.callees.is_empty());
    let callee_names: Vec<&str> = report.callees.iter().map(|c| c.callee.as_str()).collect();
    assert!(callee_names.contains(&"render_page"));

    // Check syscalls reachable from server.ssa main
    assert!(!report.syscalls.is_empty());
    let op_names: Vec<&str> = report.syscalls.iter().map(|s| s.op.as_str()).collect();
    assert!(op_names.contains(&"net.listen"));
    assert!(op_names.contains(&"net.wait_req"));
    assert!(op_names.contains(&"net.respond"));
    assert!(op_names.contains(&"ui.render"));
}

#[test]
fn test_explore_filter_effects() {
    let site = site_dir();
    let graph = explore_path(&site).expect("explore site");

    let filter = ExploreFilter {
        effects: Some("net".to_string()),
        ..Default::default()
    };
    let ai_out = graph.format_ai(&filter);
    assert!(ai_out.contains("server.ssa"));
    assert!(!ai_out.contains("site.ssa"));

    let json_out = graph.format_json(&filter);
    assert!(json_out.contains("net"));
}

#[test]
fn test_explore_filter_symbol() {
    let site = site_dir();
    let graph = explore_path(&site).expect("explore site");

    let filter = ExploreFilter {
        symbol: Some("Card".to_string()),
        ..Default::default()
    };
    let ai_out = graph.format_ai(&filter);
    assert!(ai_out.contains("SCHEMA Card"));
    assert!(!ai_out.contains("SCHEMA HttpRequest"));
}

#[test]
fn test_explore_filter_orphans_only() {
    let http_schema = site_dir().join("schemas").join("http.ssa.schema");
    let graph = explore_path(&http_schema).expect("explore http.ssa.schema");

    let filter = ExploreFilter {
        orphans_only: true,
        ..Default::default()
    };
    let ai_out = graph.format_ai(&filter);
    assert!(ai_out.contains("[ORPHANS]"));
    assert!(ai_out.contains("HttpRequest"));
    assert!(!ai_out.contains("[TOPOLOGY & IMPORTS]"));
}

#[test]
fn test_explore_format_json_determinism() {
    let site = site_dir();
    let graph = explore_path(&site).expect("explore site");
    let filter = ExploreFilter::default();

    let json1 = graph.format_json(&filter);
    let json2 = graph.format_json(&filter);
    assert_eq!(json1, json2, "JSON formatting must be byte-for-byte deterministic");

    let parsed: serde_json::Value = serde_json::from_str(&json1).expect("Valid JSON");
    assert!(parsed.get("files").is_some());
    assert!(parsed.get("schemas").is_some());
    assert!(parsed.get("functions").is_some());
    assert!(parsed.get("orphans").is_some());
}

#[test]
fn test_explore_format_mermaid_validity() {
    let site = site_dir();
    let graph = explore_path(&site).expect("explore site");
    let filter = ExploreFilter::default();

    let mermaid = graph.format_mermaid(&filter);
    assert!(mermaid.starts_with("flowchart TD\n"));
    assert!(mermaid.contains("subgraph"));
    assert!(mermaid.contains("[\""));
    assert!(mermaid.contains("\"]"));
}

#[test]
fn test_explore_error_on_nonexistent_and_invalid() {
    let non_existent = Path::new("non_existent_dir_12345");
    match explore_path(non_existent) {
        Err(ParseError::FileNotFound { line: 1, .. }) => {}
        other => panic!("Expected FileNotFound, got: {:?}", other),
    }

    let temp = TempDir::new("invalid_ext");
    let bad_file = temp.path().join("readme.txt");
    fs::write(&bad_file, "hello").unwrap();
    match explore_path(&bad_file) {
        Err(ParseError::InvalidSyntax { line: 1, .. }) => {}
        other => panic!("Expected InvalidSyntax for invalid extension, got: {:?}", other),
    }
}
