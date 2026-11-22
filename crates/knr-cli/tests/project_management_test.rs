use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(suffix: &str) -> Self {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let id = COUNTER.fetch_add(1, Ordering::SeqCst);
        let pid = std::process::id();
        let dir_name = format!("knr_pm_test_{pid}_{id}_{suffix}");
        let path = std::env::temp_dir().join(dir_name);
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("failed to create temp dir");
        Self { path }
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

#[test]
fn test_init_in_empty_dir() {
    let temp = TempDir::new("init_empty");
    let output = knr_cmd()
        .args(["init", temp.path().to_str().unwrap()])
        .output()
        .expect("failed to execute knr init");

    assert!(output.status.success(), "knr init should exit 0");

    let manifest_path = temp.path().join("knr.toml");
    let entry_path = temp.path().join("src/main.tsk");
    let gitignore_path = temp.path().join(".gitignore");
    let readme_path = temp.path().join("README.md");

    assert!(manifest_path.is_file(), "knr.toml must be created");
    assert!(entry_path.is_file(), "src/main.tsk must be created");
    assert!(gitignore_path.is_file(), ".gitignore must be created");
    assert!(readme_path.is_file(), "README.md must be created");

    let manifest_content = fs::read_to_string(manifest_path).unwrap();
    assert!(manifest_content.contains("[package]"));
    assert!(manifest_content.contains("version = \"0.1.0\""));
    assert!(manifest_content.contains("edition = \"2026\""));
}

#[test]
fn test_init_preserves_or_infers_name() {
    let temp = TempDir::new("custom_project");
    let output = knr_cmd()
        .args(["init", temp.path().to_str().unwrap(), "--name", "my_special_app"])
        .output()
        .expect("failed to execute knr init");

    assert!(output.status.success());
    let manifest_content = fs::read_to_string(temp.path().join("knr.toml")).unwrap();
    assert!(manifest_content.contains("name = \"my_special_app\""));
}

#[test]
fn test_init_fails_on_existing_without_force() {
    let temp = TempDir::new("init_fail_existing");
    fs::write(temp.path().join("knr.toml"), "[package]\nname = \"existing\"\nversion = \"0.1.0\"\n").unwrap();

    let output = knr_cmd()
        .args(["init", temp.path().to_str().unwrap()])
        .output()
        .expect("failed to execute knr init");

    assert!(!output.status.success(), "init without --force on existing knr.toml must fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let combined = format!("{stdout}\n{stderr}");
    assert!(combined.contains("already exists"), "Expected already exists error: {combined}");
}

#[test]
fn test_init_succeeds_with_force() {
    let temp = TempDir::new("init_force");
    fs::write(temp.path().join("knr.toml"), "[package]\nname = \"old\"\nversion = \"0.0.1\"\n").unwrap();

    let output = knr_cmd()
        .args(["init", temp.path().to_str().unwrap(), "--force", "--name", "overwritten"])
        .output()
        .expect("failed to execute knr init with force");

    assert!(output.status.success());
    let manifest_content = fs::read_to_string(temp.path().join("knr.toml")).unwrap();
    assert!(manifest_content.contains("name = \"overwritten\""));
}

#[test]
fn test_new_creates_directory_structure() {
    let temp_parent = TempDir::new("new_parent");
    let app_dir = temp_parent.path().join("brand_new_app");

    let output = knr_cmd()
        .args(["new", app_dir.to_str().unwrap()])
        .output()
        .expect("failed to execute knr new");

    assert!(output.status.success(), "knr new should succeed");
    assert!(app_dir.is_dir());
    assert!(app_dir.join("knr.toml").is_file());
    assert!(app_dir.join("src/main.tsk").is_file());
}

#[test]
fn test_new_all_templates_and_immediate_verification() {
    let templates = ["minimal", "app", "ui", "service", "lib"];

    for tmpl in &templates {
        let temp = TempDir::new(&format!("tmpl_{tmpl}"));
        let app_dir = temp.path().join(format!("test_{tmpl}"));

        let new_output = knr_cmd()
            .args(["new", app_dir.to_str().unwrap(), "--template", tmpl])
            .output()
            .expect("knr new with template");
        assert!(new_output.status.success(), "knr new --template {tmpl} must succeed");

        let entry_rel = if *tmpl == "ui" { "src/app.tsk" } else { "src/main.tsk" };
        let entry = app_dir.join(entry_rel);
        assert!(entry.is_file(), "{entry_rel} must exist for template {tmpl}");

        // Statically check using knr check
        let check_output = knr_cmd()
            .args(["check", entry.to_str().unwrap()])
            .output()
            .expect("knr check template");
        let check_err = String::from_utf8_lossy(&check_output.stderr);
        assert!(check_output.status.success(), "knr check must succeed for template {tmpl}: {check_err}");

        // Execute using knr run
        let run_output = knr_cmd()
            .args(["run", entry.to_str().unwrap()])
            .output()
            .expect("knr run template");
        let run_err = String::from_utf8_lossy(&run_output.stderr);
        assert!(run_output.status.success(), "knr run must succeed for template {tmpl}: {run_err}");
    }
}

#[test]
fn test_workspace_initialization() {
    let temp = TempDir::new("ws_init");
    let output = knr_cmd()
        .args(["init", temp.path().to_str().unwrap(), "--workspace", "--name", "mega_corp"])
        .output()
        .expect("knr init --workspace");

    assert!(output.status.success());

    let ws_manifest_path = temp.path().join("knr.toml");
    let demo_manifest_path = temp.path().join("apps/demo/knr.toml");
    let demo_entry_path = temp.path().join("apps/demo/src/main.tsk");

    assert!(ws_manifest_path.is_file(), "Workspace knr.toml must exist");
    assert!(demo_manifest_path.is_file(), "apps/demo/knr.toml must exist");
    assert!(demo_entry_path.is_file(), "apps/demo/src/main.tsk must exist");

    let ws_content = fs::read_to_string(ws_manifest_path).unwrap();
    assert!(ws_content.contains("[workspace]"));
    assert!(ws_content.contains("\"apps/*\""));

    // Check that demo app passes check and run
    let check_out = knr_cmd()
        .args(["check", demo_entry_path.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(check_out.status.success());

    let run_out = knr_cmd()
        .args(["run", demo_entry_path.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(run_out.status.success());
}

#[test]
fn test_new_workspace_command() {
    let temp_parent = TempDir::new("new_ws_parent");
    let ws_dir = temp_parent.path().join("my_workspace");

    let output = knr_cmd()
        .args(["new", ws_dir.to_str().unwrap(), "--workspace"])
        .output()
        .expect("knr new --workspace");

    assert!(output.status.success());
    assert!(ws_dir.join("knr.toml").is_file());
    assert!(ws_dir.join("apps/demo/knr.toml").is_file());
    assert!(ws_dir.join("apps/demo/src/main.tsk").is_file());
}
