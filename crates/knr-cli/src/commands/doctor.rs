use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use knr_core::manifest::Manifest;
use serde::Serialize;

use crate::args::{DoctorArgs, VERSION};
use crate::error::CliError;

#[derive(Debug, Clone, Serialize)]
pub struct DoctorCheck {
    pub name: String,
    pub status: String,
    pub detail: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DoctorReport {
    pub status: String,
    pub version: String,
    pub executable: String,
    pub tsk_file_count: usize,
    pub ok_count: usize,
    pub warn_count: usize,
    pub error_count: usize,
    pub checks: Vec<DoctorCheck>,
}

/// Runs all environment and project health checks rooted at `dir`.
pub fn collect_doctor_report(dir: &Path) -> DoctorReport {
    let mut checks = Vec::new();

    // 1. knr CLI version & executable path
    let exe_path = std::env::current_exe()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "<unknown>".to_string());
    checks.push(DoctorCheck {
        name: "knr_cli".to_string(),
        status: "ok".to_string(),
        detail: format!("knr v{VERSION} ({exe_path})"),
        hint: None,
    });

    // 2. C linker (cc / gcc / clang) availability for --target native
    checks.push(check_c_linker());

    // 3. libknr_runtime.a availability for --target native --gui
    checks.push(check_runtime_staticlib());

    // 4. Display server (WAYLAND_DISPLAY / DISPLAY / headless fallback)
    checks.push(check_display_server());

    // 5. Node.js availability for WASM testing
    checks.push(check_nodejs());

    // 6. VS Code .tsk extension install status (~/.vscode/extensions/knr-lang)
    checks.push(check_vscode_extension());

    // 7. Current directory knr.toml manifest validity + .tsk file count
    let tsk_file_count = count_tsk_files(dir);
    checks.push(check_project_manifest(dir, tsk_file_count));

    let ok_count = checks.iter().filter(|c| c.status == "ok").count();
    let warn_count = checks.iter().filter(|c| c.status == "warn").count();
    let error_count = checks.iter().filter(|c| c.status == "error").count();
    let status = if error_count > 0 {
        "error"
    } else if warn_count > 0 {
        "warn"
    } else {
        "ok"
    }
    .to_string();

    DoctorReport {
        status,
        version: VERSION.to_string(),
        executable: exe_path,
        tsk_file_count,
        ok_count,
        warn_count,
        error_count,
        checks,
    }
}

pub fn execute(args: &DoctorArgs) -> Result<(), CliError> {
    let cwd = args
        .dir
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    let report = collect_doctor_report(&cwd);

    if args.json {
        let pretty = serde_json::to_string_pretty(&report)
            .map_err(|e| CliError::Other(e.to_string()))?;
        println!("{pretty}");
    } else {
        println!("knr doctor (v{})", report.version);
        for c in &report.checks {
            let badge = match c.status.as_str() {
                "ok" => "[OK]  ",
                "warn" => "[WARN]",
                _ => "[ERR] ",
            };
            println!("  {badge} {}: {}", c.name, c.detail);
            if let Some(ref hint) = c.hint {
                println!("         hint: {hint}");
            }
        }
        println!(
            "\nSummary: {} ok, {} warn, {} error (status: {})",
            report.ok_count, report.warn_count, report.error_count, report.status
        );
    }

    if report.error_count > 0 {
        return Err(CliError::Other(format!(
            "knr doctor reported {} error(s)",
            report.error_count
        )));
    }

    Ok(())
}

fn check_c_linker() -> DoctorCheck {
    for compiler in ["cc", "gcc", "clang"] {
        if let Ok(out) = Command::new(compiler).arg("--version").output() {
            if out.status.success() {
                let first_line = String::from_utf8_lossy(&out.stdout)
                    .lines()
                    .next()
                    .or_else(|| {
                        let s = String::from_utf8_lossy(&out.stderr);
                        if s.is_empty() {
                            None
                        } else {
                            Some("")
                        }
                    })
                    .unwrap_or("")
                    .trim()
                    .to_string();
                let detail = if first_line.is_empty() {
                    format!("{compiler} available")
                } else {
                    format!("{compiler} ({first_line})")
                };
                return DoctorCheck {
                    name: "c_linker".to_string(),
                    status: "ok".to_string(),
                    detail,
                    hint: None,
                };
            }
        }
    }

    DoctorCheck {
        name: "c_linker".to_string(),
        status: "warn".to_string(),
        detail: "No C compiler/linker (cc, gcc, clang) found in PATH".to_string(),
        hint: Some("Install gcc or clang to enable `knr build --target native`".to_string()),
    }
}

fn find_runtime_staticlib() -> Option<PathBuf> {
    let mut candidates = Vec::new();

    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join("libknr_runtime.a"));
            if let Some(parent) = dir.parent() {
                candidates.push(parent.join("libknr_runtime.a"));
            }
        }
    }

    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd.join("target/release/libknr_runtime.a"));
        candidates.push(cwd.join("target/debug/libknr_runtime.a"));
        if let Some(parent) = cwd.parent() {
            candidates.push(parent.join("target/release/libknr_runtime.a"));
            candidates.push(parent.join("target/debug/libknr_runtime.a"));
            if let Some(gp) = parent.parent() {
                candidates.push(gp.join("target/release/libknr_runtime.a"));
                candidates.push(gp.join("target/debug/libknr_runtime.a"));
            }
        }
    }

    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    if let Some(ws_root) = manifest_dir.parent().and_then(|p| p.parent()) {
        candidates.push(ws_root.join("target/release/libknr_runtime.a"));
        candidates.push(ws_root.join("target/debug/libknr_runtime.a"));
    }

    candidates.into_iter().find(|p| p.is_file())
}

fn check_runtime_staticlib() -> DoctorCheck {
    if let Some(path) = find_runtime_staticlib() {
        let size_kb = fs::metadata(&path).map(|m| m.len() / 1024).unwrap_or(0);
        DoctorCheck {
            name: "runtime_staticlib".to_string(),
            status: "ok".to_string(),
            detail: format!("{} ({size_kb} KB)", path.display()),
            hint: None,
        }
    } else {
        DoctorCheck {
            name: "runtime_staticlib".to_string(),
            status: "warn".to_string(),
            detail: "libknr_runtime.a not found".to_string(),
            hint: Some(
                "Build with `cargo build -p knr-runtime --release` for `--target native --gui`"
                    .to_string(),
            ),
        }
    }
}

fn check_display_server() -> DoctorCheck {
    let forced_headless = std::env::var("KNR_HEADLESS")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);
    if forced_headless {
        return DoctorCheck {
            name: "display_server".to_string(),
            status: "ok".to_string(),
            detail: "Headless mode active (KNR_HEADLESS=1); CPU/headless rendering available"
                .to_string(),
            hint: None,
        };
    }

    let wayland = std::env::var("WAYLAND_DISPLAY").ok().filter(|v| !v.is_empty());
    let x11 = std::env::var("DISPLAY").ok().filter(|v| !v.is_empty());

    if let Some(w) = wayland {
        return DoctorCheck {
            name: "display_server".to_string(),
            status: "ok".to_string(),
            detail: format!("Wayland display active (WAYLAND_DISPLAY={w})"),
            hint: None,
        };
    }
    if let Some(d) = x11 {
        return DoctorCheck {
            name: "display_server".to_string(),
            status: "ok".to_string(),
            detail: format!("X11 display active (DISPLAY={d})"),
            hint: None,
        };
    }

    #[cfg(any(target_os = "macos", target_os = "windows"))]
    {
        return DoctorCheck {
            name: "display_server".to_string(),
            status: "ok".to_string(),
            detail: "Native OS windowing server available".to_string(),
            hint: None,
        };
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    DoctorCheck {
        name: "display_server".to_string(),
        status: "warn".to_string(),
        detail: "Neither WAYLAND_DISPLAY nor DISPLAY is set; headless fallback active".to_string(),
        hint: Some(
            "Headless `knr render` and `knr test` work without a display; set DISPLAY or WAYLAND_DISPLAY for `--gui`"
                .to_string(),
        ),
    }
}

fn check_nodejs() -> DoctorCheck {
    if let Ok(out) = Command::new("node").arg("--version").output() {
        if out.status.success() {
            let ver = String::from_utf8_lossy(&out.stdout).trim().to_string();
            return DoctorCheck {
                name: "nodejs".to_string(),
                status: "ok".to_string(),
                detail: format!("node {ver}"),
                hint: None,
            };
        }
    }

    DoctorCheck {
        name: "nodejs".to_string(),
        status: "warn".to_string(),
        detail: "node not found in PATH".to_string(),
        hint: Some("Install Node.js to execute WASM smoke tests".to_string()),
    }
}

fn check_vscode_extension() -> DoctorCheck {
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from);

    if let Some(home_dir) = home {
        let exact = home_dir.join(".vscode").join("extensions").join("knr-lang");
        if exact.is_dir() {
            return DoctorCheck {
                name: "vscode_extension".to_string(),
                status: "ok".to_string(),
                detail: format!("Installed at {}", exact.display()),
                hint: None,
            };
        }
        // Also detect versioned directories like knr-lang-0.4.0
        for ext_root in [
            home_dir.join(".vscode").join("extensions"),
            home_dir.join(".vscode-insiders").join("extensions"),
        ] {
            if let Ok(entries) = fs::read_dir(&ext_root) {
                for entry in entries.flatten() {
                    let p = entry.path();
                    if p.is_dir() {
                        if let Some(name) = p.file_name().and_then(|n| n.to_str()) {
                            if name.starts_with("knr-lang") || name.contains(".knr-lang") {
                                return DoctorCheck {
                                    name: "vscode_extension".to_string(),
                                    status: "ok".to_string(),
                                    detail: format!("Installed at {}", p.display()),
                                    hint: None,
                                };
                            }
                        }
                    }
                }
            }
        }
    }

    DoctorCheck {
        name: "vscode_extension".to_string(),
        status: "warn".to_string(),
        detail: "VS Code .tsk extension not found in ~/.vscode/extensions/knr-lang".to_string(),
        hint: Some(
            "Install editors/vscode into ~/.vscode/extensions/knr-lang for .tsk highlighting & LSP"
                .to_string(),
        ),
    }
}

fn check_project_manifest(dir: &Path, tsk_count: usize) -> DoctorCheck {
    let manifest_path = dir.join("knr.toml");
    if !manifest_path.is_file() {
        return DoctorCheck {
            name: "project_manifest".to_string(),
            status: "ok".to_string(),
            detail: format!(
                "No knr.toml in '{}' (standalone mode; {tsk_count} .tsk file(s) found)",
                dir.display()
            ),
            hint: Some("Run `knr init` to create a knr.toml project manifest".to_string()),
        };
    }

    match fs::read_to_string(&manifest_path) {
        Ok(content) => match Manifest::from_toml(&content) {
            Ok(Manifest::Package(pkg)) => DoctorCheck {
                name: "project_manifest".to_string(),
                status: "ok".to_string(),
                detail: format!(
                    "Valid package '{}' v{} (entry: {}, {tsk_count} .tsk file(s))",
                    pkg.name,
                    pkg.version,
                    pkg.entry.display()
                ),
                hint: None,
            },
            Ok(Manifest::Workspace(ws)) => DoctorCheck {
                name: "project_manifest".to_string(),
                status: "ok".to_string(),
                detail: format!(
                    "Valid workspace ({} member pattern(s), {tsk_count} .tsk file(s))",
                    ws.members.len()
                ),
                hint: None,
            },
            Err(err) => DoctorCheck {
                name: "project_manifest".to_string(),
                status: "error".to_string(),
                detail: format!("Invalid knr.toml at '{}': {err}", manifest_path.display()),
                hint: Some("Fix syntax or required [package]/[workspace] fields in knr.toml".to_string()),
            },
        },
        Err(err) => DoctorCheck {
            name: "project_manifest".to_string(),
            status: "error".to_string(),
            detail: format!("Failed to read '{}': {err}", manifest_path.display()),
            hint: None,
        },
    }
}

fn count_tsk_files(dir: &Path) -> usize {
    fn walk(path: &Path, depth: usize, count: &mut usize) {
        if depth > 8 {
            return;
        }
        let Ok(entries) = fs::read_dir(path) else {
            return;
        };
        for entry in entries.flatten() {
            let p = entry.path();
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name.starts_with('.') || name == "target" || name == "node_modules" || name == "dist" {
                continue;
            }
            if p.is_dir() {
                walk(&p, depth + 1, count);
            } else if p.extension().and_then(|e| e.to_str()) == Some("tsk") {
                *count += 1;
            }
        }
    }

    let mut count = 0usize;
    if dir.is_dir() {
        walk(dir, 0, &mut count);
    }
    count
}
