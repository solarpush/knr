use std::fs;
use std::path::{Path, PathBuf};

use knr_core::manifest::{Manifest, PackageManifest, WorkspaceManifest};
use crate::error::CliError;

pub const TEMPLATE_MINIMAL: &str = r#"// Knr Minimal Starter Application
fn main() -> i64 {
    let code = 0;
    return code;
}
"#;

pub const TEMPLATE_APP: &str = r#"// Knr Application Starter
schema AppState {
    0: count i64,
    1: active bool,
}

fn main() -> i64 effects(Ui) {
    let state = AppState {
        count: 0,
        active: true,
    };
    return state.count;
}
"#;

pub const TEMPLATE_UI: &str = r#"// Knr UI Application Starter
schema WindowConfig {
    0: width i64,
    1: height i64,
}

fn main() -> i64 effects(Ui) {
    let config = WindowConfig {
        width: 800,
        height: 600,
    };
    return config.width;
}
"#;

pub const TEMPLATE_SERVICE: &str = r#"// Knr Network Service Starter
schema ServiceConfig {
    0: port i64,
    1: workers i64,
}

fn start_service(cfg: ServiceConfig) -> i64 effects(Net) {
    return cfg.port;
}

fn main() -> i64 effects(Net) {
    let config = ServiceConfig {
        port: 8080,
        workers: 4,
    };
    let _status = start_service(config);
    return 0;
}
"#;

pub const TEMPLATE_LIB: &str = r#"// Knr Library Starter
schema Point {
    0: x i64,
    1: y i64,
}

fn add_points(a: Point, b: Point) -> Point {
    let res = Point {
        x: a.x + b.x,
        y: a.y + b.y,
    };
    return res;
}

fn main() -> i64 {
    let p1 = Point { x: 10, y: 20 };
    let p2 = Point { x: 30, y: 40 };
    let p3 = add_points(p1, p2);
    return p3.x;
}
"#;

pub const DEFAULT_GITIGNORE: &str = r#"/target
*.knrbin
*.o
*.wasm
.DS_Store
"#;

pub fn get_template_content(template_name: &str) -> Result<&'static str, CliError> {
    match template_name.to_ascii_lowercase().as_str() {
        "minimal" => Ok(TEMPLATE_MINIMAL),
        "app" => Ok(TEMPLATE_APP),
        "ui" => Ok(TEMPLATE_UI),
        "service" => Ok(TEMPLATE_SERVICE),
        "lib" => Ok(TEMPLATE_LIB),
        other => Err(CliError::Args(format!(
            "Unknown template '{other}'. Available templates: minimal, app, ui, service, lib."
        ))),
    }
}

/// Executes `knr init` in the specified directory or current working directory.
pub fn execute(
    dir: Option<&Path>,
    name: Option<&str>,
    workspace: bool,
    template: &str,
    force: bool,
) -> Result<(), CliError> {
    let target_dir = dir
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

    if !target_dir.exists() {
        fs::create_dir_all(&target_dir).map_err(|e| {
            CliError::Other(format!("Failed to create directory '{}': {e}", target_dir.display()))
        })?;
    }

    let manifest_path = target_dir.join("knr.toml");
    if manifest_path.exists() && !force {
        return Err(CliError::Other(format!(
            "Project manifest 'knr.toml' already exists in '{}'. Use --force to overwrite.",
            target_dir.display()
        )));
    }

    let template_source = get_template_content(template)?;

    if workspace {
        // Initialize multi-project workspace
        let ws_manifest = Manifest::Workspace(WorkspaceManifest {
            members: vec!["apps/*".to_string()],
            default_members: Some(vec!["apps/demo".to_string()]),
            dependencies: Default::default(),
        });
        fs::write(&manifest_path, ws_manifest.to_toml()).map_err(|e| {
            CliError::Other(format!("Failed to write workspace manifest: {e}"))
        })?;

        // Create default starter package in apps/demo
        let demo_dir = target_dir.join("apps").join("demo");
        let demo_src_dir = demo_dir.join("src");
        fs::create_dir_all(&demo_src_dir).map_err(|e| {
            CliError::Other(format!("Failed to create demo app directories: {e}"))
        })?;

        let demo_manifest = Manifest::Package(PackageManifest::new("demo", "0.1.0"));
        fs::write(demo_dir.join("knr.toml"), demo_manifest.to_toml()).map_err(|e| {
            CliError::Other(format!("Failed to write demo manifest: {e}"))
        })?;

        fs::write(demo_src_dir.join("main.knr"), template_source).map_err(|e| {
            CliError::Other(format!("Failed to write demo main.knr: {e}"))
        })?;

        // Write .gitignore and README.md at workspace root
        let gitignore_path = target_dir.join(".gitignore");
        if !gitignore_path.exists() || force {
            let _ = fs::write(gitignore_path, DEFAULT_GITIGNORE);
        }

        let readme_path = target_dir.join("README.md");
        if !readme_path.exists() || force {
            let ws_name = name.unwrap_or("Knr");
            let readme_content = format!(
                "# {ws_name} Workspace\n\nA multi-project Knr workspace.\n\n## Members\n- `apps/demo`\n\n## Running\n```bash\nknr run apps/demo/src/main.knr\n```\n"
            );
            let _ = fs::write(readme_path, readme_content);
        }

        println!("Initialized Knr workspace in '{}'", target_dir.display());
    } else {
        // Infer package name
        let inferred_name = if let Some(n) = name {
            n.to_string()
        } else {
            target_dir
                .canonicalize()
                .ok()
                .and_then(|c| c.file_name().map(|f| f.to_string_lossy().to_string()))
                .filter(|n| !n.is_empty())
                .unwrap_or_else(|| "knr_app".to_string())
        };

        // Initialize single package
        let pkg_manifest = Manifest::Package(PackageManifest::new(&inferred_name, "0.1.0"));
        fs::write(&manifest_path, pkg_manifest.to_toml()).map_err(|e| {
            CliError::Other(format!("Failed to write knr.toml: {e}"))
        })?;

        let src_dir = target_dir.join("src");
        fs::create_dir_all(&src_dir).map_err(|e| {
            CliError::Other(format!("Failed to create 'src' directory: {e}"))
        })?;

        let entry_file = src_dir.join("main.knr");
        if !entry_file.exists() || force {
            fs::write(&entry_file, template_source).map_err(|e| {
                CliError::Other(format!("Failed to write main.knr: {e}"))
            })?;
        }

        let gitignore_path = target_dir.join(".gitignore");
        if !gitignore_path.exists() || force {
            let _ = fs::write(gitignore_path, DEFAULT_GITIGNORE);
        }

        let readme_path = target_dir.join("README.md");
        if !readme_path.exists() || force {
            let readme_content = format!(
                "# {inferred_name}\n\nA verifiable linear application built with Knr.\n\n## Running\n```bash\nknr run src/main.knr\nknr dev\n```\n"
            );
            let _ = fs::write(readme_path, readme_content);
        }

        println!("Initialized Knr project '{inferred_name}' in '{}'", target_dir.display());
    }

    Ok(())
}
