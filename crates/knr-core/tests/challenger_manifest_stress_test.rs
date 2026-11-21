//! Adversarial empirical stress testing for `Manifest` parser, serializer, and workspace resolver.
//! Milestone 3 Challenger 1: Empirical stress harness.

use knr_core::manifest::{
    find_manifest_root, resolve_workspace_members, BuildConfig, DependencySpec, Manifest,
    ManifestError, PackageManifest, UiConfig, WorkspaceManifest,
};
use proptest::prelude::*;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

/// RAII helper for creating isolated temporary directories that are cleaned up on drop.
struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(prefix: &str) -> Self {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let id = COUNTER.fetch_add(1, Ordering::SeqCst);
        let pid = std::process::id();
        let dir = std::env::temp_dir().join(format!("knr_challenger_{prefix}_{pid}_{id}"));
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
// Section 1: Malformed TOML, Boundary Values & Missing Fields
// ============================================================================

#[test]
fn test_malformed_toml_syntax_errors() {
    let malformed_cases = [
        ("unclosed_string", "[package]\nname = \"unclosed\nversion = \"0.1.0\""),
        ("invalid_table_header", "[package\nname = \"app\"\nversion = \"0.1.0\""),
        ("duplicate_keys", "[package]\nname = \"app1\"\nname = \"app2\"\nversion = \"0.1.0\""),
        ("unexpected_tokens", "[package]\nname = @@@\nversion = \"0.1.0\""),
        ("unclosed_array", "[package]\nname = \"app\"\nversion = \"0.1.0\"\nauthors = [\"Alice\",\n"),
        ("unclosed_inline_table", "[package]\nname = \"app\"\nversion = \"0.1.0\"\n[dependencies]\nfoo = { version = \"1.0\"\n"),
    ];

    for (label, toml) in malformed_cases {
        let res = Manifest::from_toml(toml);
        assert!(
            res.is_err(),
            "Expected parse error for malformed case '{}', got Ok: {:?}",
            label,
            res
        );
        match res.unwrap_err() {
            ManifestError::Parse(msg) => {
                assert!(!msg.is_empty(), "Parse error message should not be empty for '{}'", label);
            }
            other => panic!("Expected ManifestError::Parse for '{}', got {:?}", label, other),
        }
    }
}

#[test]
fn test_missing_root_table_rejection() {
    let missing_root_cases = [
        ("empty_string", ""),
        ("whitespace_only", "   \n\t\n  "),
        ("comments_only", "# Just a comment\n# Another comment\n"),
        ("arbitrary_table", "[foo]\nbar = 123\n"),
        ("root_dependencies_only", "[dependencies]\nserde = \"1.0\"\n"),
        ("build_table_only", "[build]\ntarget = \"native\"\n"),
        ("ui_table_only", "[ui]\ntitle = \"Demo\"\n"),
    ];

    for (label, toml) in missing_root_cases {
        let res = Manifest::from_toml(toml);
        assert!(
            res.is_err(),
            "Expected MissingRootTable for case '{}', got Ok: {:?}",
            label,
            res
        );
        match res.unwrap_err() {
            ManifestError::MissingRootTable => {}
            other => panic!("Expected ManifestError::MissingRootTable for '{}', got {:?}", label, other),
        }
    }
}

#[test]
fn test_missing_and_unexpected_types_in_required_fields() {
    // Missing package.name
    let no_name = "[package]\nversion = \"1.0.0\"\n";
    let err = Manifest::from_toml(no_name).unwrap_err();
    match err {
        ManifestError::MissingField("package.name") => {}
        other => panic!("Expected MissingField('package.name'), got {:?}", other),
    }

    // Missing package.version
    let no_ver = "[package]\nname = \"my-app\"\n";
    let err = Manifest::from_toml(no_ver).unwrap_err();
    match err {
        ManifestError::MissingField("package.version") => {}
        other => panic!("Expected MissingField('package.version'), got {:?}", other),
    }

    // Unexpected type: package.name as integer
    // Defect finding: parser treats non-string type as missing field rather than type error
    let int_name = "[package]\nname = 12345\nversion = \"1.0.0\"\n";
    let err = Manifest::from_toml(int_name).unwrap_err();
    match err {
        ManifestError::MissingField("package.name") => {}
        other => panic!("Expected MissingField for numeric package.name, got {:?}", other),
    }

    // Unexpected type: package.version as array
    let arr_ver = "[package]\nname = \"my-app\"\nversion = [\"0\", \"1\", \"0\"]\n";
    let err = Manifest::from_toml(arr_ver).unwrap_err();
    match err {
        ManifestError::MissingField("package.version") => {}
        other => panic!("Expected MissingField for array package.version, got {:?}", other),
    }

    // Unexpected type: package as non-table (e.g. string or int)
    let pkg_not_table = "package = \"not a table\"\n";
    let err = Manifest::from_toml(pkg_not_table).unwrap_err();
    match err {
        ManifestError::MissingRootTable => {}
        other => panic!("Expected MissingRootTable when package is a string, got {:?}", other),
    }

    // Unexpected type: workspace as non-table
    let ws_not_table = "workspace = 42\n";
    let err = Manifest::from_toml(ws_not_table).unwrap_err();
    match err {
        ManifestError::MissingRootTable => {}
        other => panic!("Expected MissingRootTable when workspace is an int, got {:?}", other),
    }
}

// ============================================================================
// Section 2: Type Confusion, Overflow & Semantic Validation Edge Cases
// ============================================================================

#[test]
fn test_unexpected_types_in_optional_fields_fallback_behavior() {
    let toml = r#"
[package]
name = "resilient-app"
version = "0.1.0"
entry = 9999
edition = false
description = ["Not", "a", "string"]
authors = "Not an array, just a string"
"#;

    let manifest = Manifest::from_toml(toml).expect("Parser should gracefully fall back for optional fields");
    let pkg = manifest.as_package().expect("Expected PackageManifest");

    // Defect/Behavior verification:
    // entry falls back to "src/main.knr" when non-string
    assert_eq!(pkg.entry, PathBuf::from("src/main.knr"));
    // edition falls back to "2026" when non-string
    assert_eq!(pkg.edition, "2026");
    // description is None when non-string
    assert!(pkg.description.is_none());
    // authors is empty vector when non-array
    assert!(pkg.authors.is_empty());
}

#[test]
fn test_defect_numeric_overflow_in_build_and_ui_config() {
    let toml = r#"
[package]
name = "overflow-app"
version = "0.1.0"

[build]
opt_level = 258

[ui]
width = -100
height = -50
"#;

    let manifest = Manifest::from_toml(toml).expect("Should parse TOML with out-of-range numbers");
    let pkg = manifest.as_package().unwrap();

    // Remediation: opt_level 258 is rejected by u8::try_from, resulting in None
    let build = pkg.build.as_ref().unwrap();
    assert_eq!(
        build.opt_level,
        None,
        "Out-of-range opt_level 258 is safely rejected as None"
    );

    // Remediation: negative dimensions are rejected by u32::try_from, resulting in None
    let ui = pkg.ui.as_ref().unwrap();
    assert_eq!(
        ui.width,
        None,
        "Negative width -100 is safely rejected as None"
    );
    assert_eq!(
        ui.height,
        None,
        "Negative height -50 is safely rejected as None"
    );
}

#[test]
fn test_defect_dependency_spec_invalid_types_silently_dropped() {
    let toml = r#"
[package]
name = "deps-app"
version = "0.1.0"

[dependencies]
valid_simple = "1.0.0"
valid_detailed = { version = "2.0.0", path = "../shared" }
invalid_int = 42
invalid_bool = true
invalid_array = ["foo", "bar"]
empty_table = {}
unknown_keys_only = { unsupported_key = "value" }
"#;

    let manifest = Manifest::from_toml(toml).expect("Should parse TOML with mixed dependencies");
    let pkg = manifest.as_package().unwrap();

    // Valid dependencies are present
    assert!(pkg.dependencies.contains_key("valid_simple"));
    assert!(pkg.dependencies.contains_key("valid_detailed"));

    // Defect Finding: invalid types (integer, boolean, array) are silently dropped without error
    assert!(!pkg.dependencies.contains_key("invalid_int"));
    assert!(!pkg.dependencies.contains_key("invalid_bool"));
    assert!(!pkg.dependencies.contains_key("invalid_array"));

    // Defect Finding: empty_table and unknown_keys_only produce Detailed with all fields None
    assert_eq!(
        pkg.dependencies.get("empty_table"),
        Some(&DependencySpec::Detailed {
            path: None,
            version: None,
            git: None,
            tag: None,
        })
    );
    assert_eq!(
        pkg.dependencies.get("unknown_keys_only"),
        Some(&DependencySpec::Detailed {
            path: None,
            version: None,
            git: None,
            tag: None,
        })
    );
}

#[test]
fn test_defect_dual_package_and_workspace_manifest_precedence() {
    let toml = r#"
[package]
name = "hybrid-app"
version = "0.1.0"

[workspace]
members = ["crates/*"]
"#;

    let manifest = Manifest::from_toml(toml).expect("Should parse dual manifest");

    // Defect Finding: When both [package] and [workspace] are present,
    // [package] unconditionally takes precedence and [workspace] is silently discarded.
    assert!(manifest.is_package());
    assert!(!manifest.is_workspace());

    // When serialized back, [workspace] is completely lost
    let serialized = manifest.to_toml();
    assert!(!serialized.contains("[workspace]"));
}

// ============================================================================
// Section 3: Workspace Member Resolution & Deep Nested Globs
// ============================================================================

#[test]
fn test_resolve_empty_workspace_members() {
    let temp = TempDir::new("ws_empty");
    let members: Vec<String> = vec![];
    let resolved = resolve_workspace_members(temp.path(), &members).expect("Empty members should succeed");
    assert!(resolved.is_empty());
}

#[test]
fn test_defect_resolve_member_empty_string_and_dot_resolves_root() {
    let temp = TempDir::new("ws_root_member");
    let root_manifest = temp.path().join("knr.toml");
    fs::write(&root_manifest, "[workspace]\nmembers = [\"\"]\n").unwrap();

    // Remediation: Passing an empty string `""` or `"."` or `"./"` is skipped,
    // ensuring the workspace root is never falsely included as a member of itself.
    let resolved_empty = resolve_workspace_members(temp.path(), &["".to_string()]).unwrap();
    assert!(resolved_empty.is_empty(), "Empty pattern should not resolve root");

    let resolved_dot = resolve_workspace_members(temp.path(), &[".".to_string()]).unwrap();
    assert!(resolved_dot.is_empty(), "Dot pattern should not resolve root");

    let resolved_dot_slash = resolve_workspace_members(temp.path(), &["./".to_string()]).unwrap();
    assert!(resolved_dot_slash.is_empty(), "Dot-slash pattern should not resolve root");
}

#[test]
fn test_resolve_nonexistent_glob_returns_empty_results() {
    let temp = TempDir::new("ws_nonexistent_glob");
    // Glob for directory that does not exist at all
    let resolved = resolve_workspace_members(temp.path(), &["nonexistent_apps/*".to_string()])
        .expect("Non-matching glob pattern should return Ok(empty), not an error");
    assert!(resolved.is_empty());
}

#[test]
fn test_resolve_deep_nested_globs_and_intermediate_subdirs() {
    let temp = TempDir::new("ws_deep_globs");
    let pkg_l3 = temp.path().join("level1").join("level2").join("level3");
    fs::create_dir_all(&pkg_l3).unwrap();
    fs::write(
        pkg_l3.join("knr.toml"),
        "[package]\nname = \"deep-pkg\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();

    // Test segment-by-segment glob "*/*/*"
    let resolved_3star = resolve_workspace_members(temp.path(), &["*/*/*".to_string()]).unwrap();
    assert_eq!(resolved_3star, vec![pkg_l3.clone()]);

    // Test intermediate glob "level1/*/level3"
    let resolved_middle = resolve_workspace_members(temp.path(), &["level1/*/level3".to_string()]).unwrap();
    assert_eq!(resolved_middle, vec![pkg_l3.clone()]);

    // Test recursive glob "**"
    let resolved_recursive = resolve_workspace_members(temp.path(), &["**".to_string()]).unwrap();
    assert!(resolved_recursive.contains(&pkg_l3));

    // Remediation: In `expand_glob`, `pattern.split_once("/**")` precedes `pattern.split_once("/*")`.
    // When the pattern is "level1/**", it triggers recursive glob expansion and finds level1/level2/level3!
    let resolved_prefix = resolve_workspace_members(temp.path(), &["level1/**".to_string()]).unwrap();
    assert!(
        resolved_prefix.contains(&pkg_l3),
        "Recursive glob 'level1/**' successfully resolves deeply nested package"
    );
}

#[test]
fn test_resolve_unicode_and_emoji_directory_paths() {
    let temp = TempDir::new("ws_unicode");
    let emoji_pkg = temp.path().join("apps").join("🚀_launcher_📦");
    let cjk_pkg = temp.path().join("apps").join("日本語_サービス");
    let accent_pkg = temp.path().join("apps").join("café_app");

    fs::create_dir_all(&emoji_pkg).unwrap();
    fs::create_dir_all(&cjk_pkg).unwrap();
    fs::create_dir_all(&accent_pkg).unwrap();

    fs::write(emoji_pkg.join("knr.toml"), "[package]\nname = \"rocket\"\nversion = \"0.1.0\"\n").unwrap();
    fs::write(cjk_pkg.join("knr.toml"), "[package]\nname = \"nihongo\"\nversion = \"0.1.0\"\n").unwrap();
    fs::write(accent_pkg.join("knr.toml"), "[package]\nname = \"cafe\"\nversion = \"0.1.0\"\n").unwrap();

    // Test wildcard glob resolution with unicode directory names
    let resolved = resolve_workspace_members(temp.path(), &["apps/*".to_string()]).unwrap();
    assert_eq!(resolved.len(), 3);
    assert!(resolved.contains(&emoji_pkg));
    assert!(resolved.contains(&cjk_pkg));
    assert!(resolved.contains(&accent_pkg));

    // Test explicit unicode member path resolution
    let resolved_explicit = resolve_workspace_members(
        temp.path(),
        &["apps/🚀_launcher_📦".to_string(), "apps/日本語_サービス".to_string()],
    )
    .unwrap();
    assert_eq!(resolved_explicit.len(), 2);
    assert!(resolved_explicit.contains(&emoji_pkg));
    assert!(resolved_explicit.contains(&cjk_pkg));
}

#[test]
#[cfg(unix)]
fn test_resolve_permission_denied_directory_handling() {
    use std::os::unix::fs::PermissionsExt;

    let temp = TempDir::new("ws_perms");
    let accessible = temp.path().join("accessible");
    let forbidden = temp.path().join("forbidden");

    fs::create_dir_all(&accessible).unwrap();
    fs::create_dir_all(&forbidden).unwrap();

    fs::write(accessible.join("knr.toml"), "[package]\nname = \"ok\"\nversion = \"0.1.0\"\n").unwrap();
    fs::write(forbidden.join("knr.toml"), "[package]\nname = \"secret\"\nversion = \"0.1.0\"\n").unwrap();

    // Remove all permissions from the forbidden directory
    fs::set_permissions(&forbidden, fs::Permissions::from_mode(0o000)).unwrap();

    // Expand glob: read_dir on forbidden will return Err(PermissionDenied)
    // The resolver should skip unreadable directories without crashing
    let resolved = resolve_workspace_members(temp.path(), &["*".to_string()]).unwrap();

    assert!(resolved.contains(&accessible));
    assert!(!resolved.contains(&forbidden));

    // Restore permissions for clean TempDir teardown
    let _ = fs::set_permissions(&forbidden, fs::Permissions::from_mode(0o755));
}

// ============================================================================
// Section 4: Symlinks & Circular Symlink Infinite Recursion Analysis
// ============================================================================

#[test]
#[cfg(unix)]
fn test_defect_circular_symlink_phantom_multiplication() {
    let temp = TempDir::new("ws_circ_loop");
    let manifest_path = temp.path().join("knr.toml");
    fs::write(
        &manifest_path,
        "[package]\nname = \"loop-pkg\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();

    let link_path = temp.path().join("loop_link");
    std::os::unix::fs::symlink(temp.path(), &link_path).unwrap();

    // Remediation: When resolving "**", scan_recursive_for_manifests tracks visited canonical paths.
    // The circular symlink loop_link points back to temp.path(), which is already visited,
    // so it is skipped immediately. No phantom duplicate members or infinite loops occur.
    let resolved = resolve_workspace_members(temp.path(), &["**".to_string()]).unwrap();

    assert_eq!(
        resolved.len(),
        0,
        "Circular symlink cycle detected and skipped; zero phantom duplicates"
    );
}

#[test]
#[cfg(unix)]
fn test_valid_acyclic_symlink_member_resolution() {
    let temp = TempDir::new("ws_symlink_acyclic");
    let real_pkg = temp.path().join("real_pkg");
    let link_pkg = temp.path().join("link_pkg");

    fs::create_dir_all(&real_pkg).unwrap();
    fs::write(
        real_pkg.join("knr.toml"),
        "[package]\nname = \"sym-app\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();

    // Create valid non-circular symlink to the package directory
    std::os::unix::fs::symlink(&real_pkg, &link_pkg).unwrap();

    let resolved = resolve_workspace_members(temp.path(), &["link_pkg".to_string()]).unwrap();
    assert_eq!(resolved, vec![link_pkg]);
}

// ============================================================================
// Section 5: Roundtrip Stability & Serialization Defect Proofs
// ============================================================================

#[test]
fn test_defect_roundtrip_empty_default_members_dropped() {
    let ws = WorkspaceManifest {
        members: vec!["crates/*".to_string()],
        // Explicitly set to Some(vec![])
        default_members: Some(vec![]),
        dependencies: BTreeMap::new(),
    };

    let manifest = Manifest::Workspace(ws.clone());
    let serialized = manifest.to_toml();

    // Remediation: When default_members is Some(vec![]), to_toml emits `default_members = []\n`.
    // On parse, default_members is preserved as `Some(vec![])`.
    assert!(
        serialized.contains("default_members = []"),
        "to_toml preserves empty default_members"
    );

    let parsed = Manifest::from_toml(&serialized).expect("Should parse serialized workspace");
    let parsed_ws = parsed.as_workspace().unwrap();

    assert_eq!(parsed_ws.default_members, Some(vec![]));
    assert_eq!(
        Manifest::Workspace(ws),
        parsed,
        "Roundtrip stability holds when default_members is Some(vec![])"
    );
}

#[test]
fn test_defect_roundtrip_dotted_dependency_name_corrupts_spec() {
    let mut deps = BTreeMap::new();
    deps.insert("serde.json".to_string(), DependencySpec::Simple("1.0.100".to_string()));

    let pkg = PackageManifest {
        name: "dotted-dep-app".to_string(),
        version: "0.1.0".to_string(),
        entry: PathBuf::from("src/main.knr"),
        edition: "2026".to_string(),
        dependencies: deps,
        description: None,
        authors: vec![],
        build: None,
        ui: None,
    };

    let manifest = Manifest::Package(pkg.clone());
    let serialized = manifest.to_toml();

    // In to_toml, dotted dependency keys are quoted: `"serde.json" = "1.0.100"`
    assert!(serialized.contains("\"serde.json\" = "));

    let parsed = Manifest::from_toml(&serialized).expect("Should parse serialized TOML");
    let parsed_pkg = parsed.as_package().unwrap();

    // Remediation: The key "serde.json" is properly preserved
    assert!(
        parsed_pkg.dependencies.contains_key("serde.json"),
        "Original dependency key 'serde.json' was preserved"
    );
    assert_eq!(
        Manifest::Package(pkg),
        parsed,
        "Roundtrip stability holds for dotted dependency keys"
    );
}

#[test]
fn test_defect_roundtrip_unescaped_control_characters_fail_parse() {
    let pkg = PackageManifest {
        name: "control-app".to_string(),
        version: "0.1.0".to_string(),
        entry: PathBuf::from("src/main.knr"),
        edition: "2026".to_string(),
        dependencies: BTreeMap::new(),
        // Control characters: \x0C (form feed), \x08 (backspace), \x1B (escape)
        description: Some("Text with \x0C formfeed and \x1B escape".to_string()),
        authors: vec![],
        build: None,
        ui: None,
    };

    let manifest = Manifest::Package(pkg.clone());
    let serialized = manifest.to_toml();

    // Remediation: escape_toml_str properly escapes all control characters \x00..=\x1F per TOML 1.0.
    let parse_res = Manifest::from_toml(&serialized);

    assert!(
        parse_res.is_ok(),
        "to_toml produces valid TOML with properly escaped control characters"
    );
    assert_eq!(
        parse_res.unwrap(),
        Manifest::Package(pkg),
        "Manifest with control characters roundtrips cleanly"
    );
}

#[test]
fn test_roundtrip_comprehensive_clean_manifest() {
    let mut deps = BTreeMap::new();
    deps.insert("lib_core".to_string(), DependencySpec::Simple("0.5.0".to_string()));
    deps.insert("lib_gui".to_string(), DependencySpec::Detailed {
        path: Some(PathBuf::from("../gui")),
        version: Some("0.2.0".to_string()),
        git: Some("https://github.com/example/gui.git".to_string()),
        tag: Some("v0.2.0".to_string()),
    });
    deps.insert("lib_empty_spec".to_string(), DependencySpec::Detailed {
        path: None,
        version: None,
        git: None,
        tag: None,
    });

    let pkg = PackageManifest {
        name: "clean-app".to_string(),
        version: "2.1.0-alpha.1".to_string(),
        entry: PathBuf::from("custom/entry.knr"),
        edition: "2026".to_string(),
        dependencies: deps,
        description: Some("Multi-line description\nwith special characters: \\ and \" and \t tabs.".to_string()),
        authors: vec!["Alice <alice@test.org>".to_string(), "Bob <bob@test.org>".to_string()],
        build: Some(BuildConfig {
            target: Some("wasm".to_string()),
            opt_level: Some(3),
            out_dir: Some(PathBuf::from("dist/bin")),
        }),
        ui: Some(UiConfig {
            title: Some("Clean UI Title: \"Super App\"".to_string()),
            width: Some(1920),
            height: Some(1080),
            theme: Some("nord".to_string()),
        }),
    };

    let manifest = Manifest::Package(pkg.clone());
    let serialized = manifest.to_toml();
    let parsed = Manifest::from_toml(&serialized).expect("Clean manifest should roundtrip cleanly");

    assert_eq!(Manifest::Package(pkg), parsed);
}

// ============================================================================
// Section 6: Manifest Root Discovery Edge Cases
// ============================================================================

#[test]
fn test_find_manifest_root_boundary_cases() {
    let temp = TempDir::new("root_boundary");
    let manifest_path = temp.path().join("knr.toml");
    fs::write(&manifest_path, "[package]\nname = \"root\"\nversion = \"0.1.0\"\n").unwrap();

    // 1. Direct directory match
    assert_eq!(find_manifest_root(temp.path()), Some(temp.path().to_path_buf()));

    // 2. Direct manifest file match
    assert_eq!(find_manifest_root(&manifest_path), Some(temp.path().to_path_buf()));

    // 3. Deeply nested non-existent path
    let deep_nonexistent = temp.path().join("does").join("not").join("exist").join("file.knr");
    assert_eq!(find_manifest_root(&deep_nonexistent), Some(temp.path().to_path_buf()));

    // 4. File in root directory
    let root_file = temp.path().join("main.knr");
    fs::write(&root_file, "// code").unwrap();
    assert_eq!(find_manifest_root(&root_file), Some(temp.path().to_path_buf()));
}

#[test]
fn test_find_manifest_root_returns_none_when_outside_project() {
    let temp = TempDir::new("isolated_no_manifest");
    let sub = temp.path().join("a").join("b");
    fs::create_dir_all(&sub).unwrap();

    let found = find_manifest_root(&sub);
    assert_eq!(found, None);
}

// ============================================================================
// Section 7: Proptest Property Testing for Roundtrip Invariants
// ============================================================================

// Strategy generating valid identifier strings (ASCII alphanumeric and hyphens/underscores)
fn valid_name_strategy() -> impl Strategy<Value = String> {
    "[a-z][a-z0-9_-]{1,15}"
}

fn valid_version_strategy() -> impl Strategy<Value = String> {
    "[0-9]{1,2}\\.[0-9]{1,2}\\.[0-9]{1,2}"
}

// Strategy generating clean strings with standard printable characters
fn clean_string_strategy() -> impl Strategy<Value = String> {
    "[a-zA-Z0-9 .,:;!?_/'\"-]{0,30}"
}

proptest! {
    #[test]
    fn prop_package_manifest_roundtrip_stability(
        name in valid_name_strategy(),
        version in valid_version_strategy(),
        edition in "[0-9]{4}",
        description in proptest::option::of(clean_string_strategy()),
        target in proptest::option::of(valid_name_strategy()),
        opt_level in proptest::option::of(0u8..4u8),
        width in proptest::option::of(100u32..3840u32),
        height in proptest::option::of(100u32..2160u32),
        theme in proptest::option::of(valid_name_strategy()),
    ) {
        let pkg = PackageManifest {
            name,
            version,
            entry: PathBuf::from("src/main.knr"),
            edition,
            dependencies: BTreeMap::new(),
            description,
            authors: vec![],
            build: Some(BuildConfig {
                target,
                opt_level,
                out_dir: None,
            }),
            ui: Some(UiConfig {
                title: None,
                width,
                height,
                theme,
            }),
        };

        let manifest = Manifest::Package(pkg.clone());
        let toml_str = manifest.to_toml();
        let parsed = Manifest::from_toml(&toml_str).expect("Valid generated manifest should parse");

        prop_assert_eq!(Manifest::Package(pkg), parsed);
    }

    #[test]
    fn prop_workspace_manifest_roundtrip_stability(
        members in proptest::collection::vec(valid_name_strategy(), 1..5),
        default_members in proptest::option::of(proptest::collection::vec(valid_name_strategy(), 1..3)),
    ) {
        let ws = WorkspaceManifest {
            members,
            default_members,
            dependencies: BTreeMap::new(),
        };

        let manifest = Manifest::Workspace(ws.clone());
        let toml_str = manifest.to_toml();
        let parsed = Manifest::from_toml(&toml_str).expect("Valid generated workspace should parse");

        prop_assert_eq!(Manifest::Workspace(ws), parsed);
    }
}
