use knr_core::manifest::{
    find_manifest_root, resolve_workspace_members, BuildConfig, DependencySpec, Manifest,
    ManifestError, PackageManifest, UiConfig, WorkspaceManifest,
};
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

#[test]
fn test_manifest_package_minimal() {
    let toml = r#"
[package]
name = "my-minimal-app"
version = "0.1.0"
"#;

    let manifest = Manifest::from_toml(toml).expect("Should parse minimal package manifest");
    assert!(manifest.is_package());
    assert!(!manifest.is_workspace());

    let pkg = manifest.as_package().expect("Expected package manifest");
    assert_eq!(pkg.name, "my-minimal-app");
    assert_eq!(pkg.version, "0.1.0");
    assert_eq!(pkg.entry, PathBuf::from("src/main.knr"));
    assert_eq!(pkg.edition, "2026");
    assert!(pkg.dependencies.is_empty());
    assert!(pkg.description.is_none());
    assert!(pkg.authors.is_empty());
}

#[test]
fn test_manifest_package_roundtrip() {
    let mut deps = BTreeMap::new();
    deps.insert("common".to_string(), DependencySpec::Detailed {
        path: Some(PathBuf::from("../common")),
        version: Some("0.2.0".to_string()),
        git: None,
        tag: None,
    });
    deps.insert("serde_json".to_string(), DependencySpec::Simple("1.0.100".to_string()));

    let pkg = PackageManifest {
        name: "test-app".to_string(),
        version: "1.2.3".to_string(),
        entry: PathBuf::from("src/entry.knr"),
        edition: "2026".to_string(),
        dependencies: deps,
        description: Some("A test application for knr".to_string()),
        authors: vec!["Alice <alice@example.com>".to_string(), "Bob <bob@example.com>".to_string()],
        build: Some(BuildConfig {
            target: Some("native".to_string()),
            opt_level: Some(2),
            out_dir: Some(PathBuf::from("target/knr")),
        }),
        ui: Some(UiConfig {
            title: Some("My Super App".to_string()),
            width: Some(1024),
            height: Some(768),
            theme: Some("dark".to_string()),
        }),
    };

    let manifest = Manifest::Package(pkg.clone());
    let serialized = manifest.to_toml();

    let parsed = Manifest::from_toml(&serialized).expect("Should parse serialized manifest");
    assert_eq!(Manifest::Package(pkg), parsed);
}

#[test]
fn test_manifest_workspace_roundtrip() {
    let mut ws_deps = BTreeMap::new();
    ws_deps.insert("shared_lib".to_string(), DependencySpec::Detailed {
        path: Some(PathBuf::from("packages/shared")),
        version: None,
        git: None,
        tag: None,
    });

    let ws = WorkspaceManifest {
        members: vec!["apps/*".to_string(), "packages/core".to_string()],
        default_members: Some(vec!["apps/demo".to_string()]),
        dependencies: ws_deps,
    };

    let manifest = Manifest::Workspace(ws.clone());
    let serialized = manifest.to_toml();

    let parsed = Manifest::from_toml(&serialized).expect("Should parse serialized workspace manifest");
    assert!(parsed.is_workspace());
    assert_eq!(Manifest::Workspace(ws), parsed);
}

#[test]
fn test_manifest_workspace_single_default_member() {
    let toml = r#"
[workspace]
members = ["crates/*"]
default_member = "crates/cli"
"#;

    let manifest = Manifest::from_toml(toml).expect("Should parse workspace manifest");
    let ws = manifest.as_workspace().expect("Workspace expected");
    assert_eq!(ws.members, vec!["crates/*"]);
    assert_eq!(ws.default_members, Some(vec!["crates/cli".to_string()]));
}

#[test]
fn test_manifest_missing_root_table() {
    let toml = r#"
foo = "bar"
"#;
    let err = Manifest::from_toml(toml).unwrap_err();
    match err {
        ManifestError::MissingRootTable => {}
        other => panic!("Unexpected error: {:?}", other),
    }
}

#[test]
fn test_manifest_missing_required_fields() {
    let toml_no_name = r#"
[package]
version = "0.1.0"
"#;
    let err = Manifest::from_toml(toml_no_name).unwrap_err();
    match err {
        ManifestError::MissingField("package.name") => {}
        other => panic!("Expected MissingField('package.name'), got: {:?}", other),
    }

    let toml_no_ver = r#"
[package]
name = "my-app"
"#;
    let err = Manifest::from_toml(toml_no_ver).unwrap_err();
    match err {
        ManifestError::MissingField("package.version") => {}
        other => panic!("Expected MissingField('package.version'), got: {:?}", other),
    }
}

#[test]
fn test_find_manifest_root() {
    let temp_dir = std::env::temp_dir().join(format!("knr_test_root_{}", std::process::id()));
    let sub_dir = temp_dir.join("a").join("b").join("c");
    fs::create_dir_all(&sub_dir).unwrap();

    let manifest_path = temp_dir.join("knr.toml");
    fs::write(&manifest_path, "[package]\nname = \"root-app\"\nversion = \"0.1.0\"\n").unwrap();

    let found = find_manifest_root(&sub_dir);
    assert_eq!(found, Some(temp_dir.clone()));

    let deep_file = sub_dir.join("deep.knr");
    fs::write(&deep_file, "// test").unwrap();
    let found_file = find_manifest_root(&deep_file);
    assert_eq!(found_file, Some(temp_dir.clone()));

    // Clean up
    let _ = fs::remove_dir_all(&temp_dir);
}

#[test]
fn test_resolve_workspace_members() {
    let temp_root = std::env::temp_dir().join(format!("knr_test_ws_{}", std::process::id()));
    let app1 = temp_root.join("apps").join("app1");
    let app2 = temp_root.join("apps").join("app2");
    let pkg_core = temp_root.join("packages").join("core");
    let ignored = temp_root.join("apps").join("not_a_pkg");

    fs::create_dir_all(&app1).unwrap();
    fs::create_dir_all(&app2).unwrap();
    fs::create_dir_all(&pkg_core).unwrap();
    fs::create_dir_all(&ignored).unwrap();

    fs::write(app1.join("knr.toml"), "[package]\nname = \"app1\"\nversion = \"0.1.0\"\n").unwrap();
    fs::write(app2.join("knr.toml"), "[package]\nname = \"app2\"\nversion = \"0.1.0\"\n").unwrap();
    fs::write(pkg_core.join("knr.toml"), "[package]\nname = \"core\"\nversion = \"0.1.0\"\n").unwrap();
    // note: ignored does NOT have knr.toml

    let members = vec!["apps/*".to_string(), "packages/core".to_string()];
    let resolved = resolve_workspace_members(&temp_root, &members).expect("Resolve should succeed");

    assert_eq!(resolved.len(), 3);
    assert!(resolved.contains(&app1));
    assert!(resolved.contains(&app2));
    assert!(resolved.contains(&pkg_core));
    assert!(!resolved.contains(&ignored));

    // Test missing explicit member fails
    let bad_members = vec!["packages/nonexistent".to_string()];
    let bad_res = resolve_workspace_members(&temp_root, &bad_members);
    assert!(bad_res.is_err());

    // Clean up
    let _ = fs::remove_dir_all(&temp_root);
}
