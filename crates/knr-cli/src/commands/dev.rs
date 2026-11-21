use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use knr_core::manifest::{find_manifest_root, resolve_workspace_members, Manifest};
use crate::error::CliError;
use crate::platform::CliHostPlatform;

/// Executes `knr dev` file watcher with debouncing and resilient error recovery.
pub fn execute(
    target: Option<&Path>,
    debounce_ms: u64,
    poll_ms: u64,
    gas: u64,
    trace: bool,
    clear: bool,
    once: bool,
) -> Result<(), CliError> {
    let start_path = target.unwrap_or_else(|| Path::new("."));
    let (watch_root, mut entry_file) = resolve_watch_and_entry(start_path)?;

    // In single-cycle mode (--once or --test-once), execute once and exit
    if once {
        return run_cycle(&entry_file, gas, trace).map_err(CliError::Other);
    }

    println!("knr dev — Monitoring '{}' for changes", watch_root.display());
    println!("Target entrypoint: '{}'", entry_file.display());

    // Initial cycle
    match run_cycle(&entry_file, gas, trace) {
        Ok(()) => {
            println!("Ready. Watching for changes... (Press Ctrl+C to exit)");
        }
        Err(err) => {
            eprintln!("{err}");
            eprintln!("Watching for changes... (fix errors to re-run)");
        }
    }

    let mut last_snapshot = take_snapshot(&watch_root);

    loop {
        std::thread::sleep(Duration::from_millis(poll_ms.max(20)));
        let current_snapshot = take_snapshot(&watch_root);

        let changed_files = detect_changes(&last_snapshot, &current_snapshot);
        if !changed_files.is_empty() {
            // Debounce window to let editor finish writing buffers
            if debounce_ms > 0 {
                std::thread::sleep(Duration::from_millis(debounce_ms));
            }
            last_snapshot = take_snapshot(&watch_root);

            // If knr.toml changed, re-resolve entrypoint
            if changed_files.iter().any(|p| p.file_name().and_then(|n| n.to_str()) == Some("knr.toml")) {
                if let Ok((_, new_entry)) = resolve_watch_and_entry(start_path) {
                    entry_file = new_entry;
                }
            }

            if clear {
                print!("\x1B[2J\x1B[1;1H");
            }

            let first_rel = changed_files[0]
                .strip_prefix(&watch_root)
                .unwrap_or(&changed_files[0]);

            println!("============================================================");
            println!("knr dev — File changed: {}", first_rel.display());
            println!("============================================================");

            match run_cycle(&entry_file, gas, trace) {
                Ok(()) => {
                    println!("Ready. Watching for changes... (Press Ctrl+C to exit)");
                }
                Err(err) => {
                    eprintln!("{err}");
                    eprintln!("Watching for changes... (fix errors to re-run)");
                }
            }
        }
    }
}

/// Resolves the watch directory root and the target source entrypoint file.
fn resolve_watch_and_entry(start: &Path) -> Result<(PathBuf, PathBuf), CliError> {
    let canonical = start.canonicalize().unwrap_or_else(|_| start.to_path_buf());

    if canonical.is_file() {
        if canonical.file_name().and_then(|n| n.to_str()) == Some("knr.toml") {
            let watch_dir = canonical.parent().unwrap_or(Path::new(".")).to_path_buf();
            let manifest = Manifest::load_from_path(&canonical)
                .map_err(|e| CliError::Other(format!("Failed to load manifest: {e}")))?;
            if let Some(entry_rel) = manifest.entry() {
                let entry_file = watch_dir.join(entry_rel);
                return Ok((watch_dir, entry_file));
            } else if let Manifest::Workspace(ws) = &manifest {
                if let Some(first_default) = ws.default_members.as_ref().and_then(|d| d.first()) {
                    let member_dir = watch_dir.join(first_default);
                    if let Ok(Manifest::Package(pkg)) = Manifest::load_from_path(&member_dir) {
                        return Ok((watch_dir, member_dir.join(pkg.entry)));
                    }
                }
                if let Ok(members) = resolve_workspace_members(&watch_dir, &ws.members) {
                    if let Some(first_member) = members.first() {
                        if let Ok(Manifest::Package(pkg)) = Manifest::load_from_path(first_member) {
                            return Ok((watch_dir, first_member.join(pkg.entry)));
                        }
                    }
                }
            }
        }
        let watch_dir = canonical.parent().unwrap_or(Path::new(".")).to_path_buf();
        return Ok((watch_dir, canonical));
    }

    // Try finding manifest root in directory or ancestors
    if let Some(manifest_root) = find_manifest_root(&canonical) {
        if let Ok(manifest) = Manifest::load_from_path(&manifest_root) {
            match manifest {
                Manifest::Package(pkg) => {
                    let entry = manifest_root.join(&pkg.entry);
                    return Ok((manifest_root, entry));
                }
                Manifest::Workspace(ws) => {
                    if let Some(first_default) = ws.default_members.as_ref().and_then(|d| d.first()) {
                        let member_dir = manifest_root.join(first_default);
                        if let Ok(Manifest::Package(pkg)) = Manifest::load_from_path(&member_dir) {
                            return Ok((manifest_root, member_dir.join(pkg.entry)));
                        }
                    }
                    if let Ok(members) = resolve_workspace_members(&manifest_root, &ws.members) {
                        if let Some(first_member) = members.first() {
                            if let Ok(Manifest::Package(pkg)) = Manifest::load_from_path(first_member) {
                                return Ok((manifest_root, first_member.join(pkg.entry)));
                            }
                        }
                    }
                }
            }
        }
    }

    // Fallbacks if no manifest found
    let candidates = [
        canonical.join("src/main.knr"),
        canonical.join("src/main.ssa"),
        canonical.join("main.knr"),
        canonical.join("main.ssa"),
    ];

    for candidate in &candidates {
        if candidate.is_file() {
            return Ok((canonical, candidate.clone()));
        }
    }

    // Search directory for any knr/ssa file
    if let Some(found) = find_first_source_file(&canonical) {
        return Ok((canonical, found));
    }

    Err(CliError::Other(format!(
        "No entrypoint source file or knr.toml found in '{}'. Expected 'src/main.knr'.",
        canonical.display()
    )))
}

fn find_first_source_file(dir: &Path) -> Option<PathBuf> {
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() {
                let s = path.to_string_lossy();
                if s.ends_with(".knr") || s.ends_with(".ssa") {
                    return Some(path);
                }
            } else if path.is_dir() {
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if !name.starts_with('.') && name != "target" {
                    if let Some(sub) = find_first_source_file(&path) {
                        return Some(sub);
                    }
                }
            }
        }
    }
    None
}

/// Executes one re-parse, re-verify, and re-run cycle against the entry file.
fn run_cycle(entry: &Path, gas: u64, trace: bool) -> Result<(), String> {
    if !entry.is_file() {
        return Err(format!("Entrypoint file '{}' not found", entry.display()));
    }

    let is_knr = entry.to_string_lossy().ends_with(".knr");

    let module = if is_knr {
        let src = fs::read_to_string(entry).map_err(|e| format!("IO Error reading '{}': {e}", entry.display()))?;
        let prog = knr_core::syntax::parse_knr(&src).map_err(|e| format!("Syntax Error: {e}"))?;

        if let Err(diagnostics) = knr_verify::verify_program(&prog) {
            for diag in &diagnostics {
                eprintln!("{diag}");
            }
            return Err(format!("Verification failed with {} diagnostic(s)", diagnostics.len()));
        }

        knr_core::lower::lower_program_from_file(entry).map_err(|e| format!("Lowering Error: {e}"))?
    } else {
        let module = knr_core::load_module_from_path(entry).map_err(|e| format!("Parse Error: {e}"))?;

        if let Err(diagnostics) = knr_verify::verify_module(&module) {
            for diag in &diagnostics {
                eprintln!("{diag}");
            }
            return Err(format!("Verification failed with {} diagnostic(s)", diagnostics.len()));
        }

        module
    };

    let entry_func = module
        .get_function("main")
        .or_else(|| module.functions.first())
        .ok_or_else(|| "Module contains no functions to execute".to_string())?;

    let entry_name = entry_func.name.clone();

    let args: Vec<knr_interp::Value> = entry_func
        .params
        .iter()
        .map(|(_, ty)| knr_interp::default_value_for_type_with_schemas(ty, &module.schemas))
        .collect();

    let mut platform = CliHostPlatform::new();

    let (val, trace_records) = knr_interp::execute_module(
        &module,
        &entry_name,
        args,
        &mut platform,
        gas,
        trace,
    )
    .map_err(|err| format!("VM Error: {err}"))?;

    if trace {
        let json_output = serde_json::to_string_pretty(&trace_records)
            .map_err(|e| format!("Serialization Error: {e}"))?;
        println!("{json_output}");
    } else {
        println!("{val}");
    }

    Ok(())
}

type FileSnapshot = BTreeMap<PathBuf, (SystemTime, u64)>;

fn take_snapshot(root: &Path) -> FileSnapshot {
    let mut snapshot = BTreeMap::new();
    scan_dir(root, &mut snapshot);
    snapshot
}

fn scan_dir(dir: &Path, snapshot: &mut FileSnapshot) {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");

        if path.is_dir() {
            if file_name.starts_with('.')
                || file_name == "target"
                || file_name == "build"
                || file_name == "dist"
                || file_name == "node_modules"
            {
                continue;
            }
            scan_dir(&path, snapshot);
        } else if path.is_file() && is_watched_file(file_name) {
            if let Ok(meta) = fs::metadata(&path) {
                let mtime = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
                let len = meta.len();
                snapshot.insert(path, (mtime, len));
            }
        }
    }
}

fn is_watched_file(name: &str) -> bool {
    if name.starts_with('.') || name.ends_with('~') || name.ends_with(".swp") || name.ends_with(".tmp") {
        return false;
    }

    name == "knr.toml"
        || name.ends_with(".knr")
        || name.ends_with(".ssa")
        || name.ends_with(".schema")
}

fn detect_changes(prev: &FileSnapshot, curr: &FileSnapshot) -> Vec<PathBuf> {
    let mut changed = Vec::new();

    for (path, (curr_mtime, curr_len)) in curr {
        match prev.get(path) {
            Some((prev_mtime, prev_len)) => {
                if curr_mtime != prev_mtime || curr_len != prev_len {
                    changed.push(path.clone());
                }
            }
            None => {
                changed.push(path.clone());
            }
        }
    }

    for path in prev.keys() {
        if !curr.contains_key(path) {
            changed.push(path.clone());
        }
    }

    changed
}
