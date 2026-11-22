use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, SystemTime};

use knr_core::manifest::{find_manifest_root, resolve_workspace_members, Manifest};
use knr_interp::Value;
use knr_runtime::gui::{
    has_display_server, launch_ui_app, launch_ui_app_with_reload_and_effects, spawn_ui_cmd,
    HotReloadUpdate, UiCmd, UiEvent, UiNode, WindowBackend, WindowConfig,
};

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

        let mut changed_files = detect_changes(&last_snapshot, &current_snapshot);
        if !changed_files.is_empty() {
            // True trailing-edge debounce: wait until the filesystem stays quiet
            // for `debounce_ms`, resetting the quiet timer on every burst write.
            if debounce_ms > 0 {
                let mut settled_snapshot = current_snapshot;
                let target_quiet = Duration::from_millis(debounce_ms);
                let step = Duration::from_millis(poll_ms.min(debounce_ms).clamp(10, 40));
                let mut last_change_at = std::time::Instant::now();
                let max_wait_start = last_change_at;
                let max_total_wait = Duration::from_millis(debounce_ms.saturating_mul(10).max(1500));

                while last_change_at.elapsed() < target_quiet
                    && max_wait_start.elapsed() < max_total_wait
                {
                    std::thread::sleep(step);
                    let probe = take_snapshot(&watch_root);
                    let more_changes = detect_changes(&settled_snapshot, &probe);
                    if !more_changes.is_empty() {
                        for p in more_changes {
                            if !changed_files.contains(&p) {
                                changed_files.push(p);
                            }
                        }
                        settled_snapshot = probe;
                        last_change_at = std::time::Instant::now();
                    }
                }
                last_snapshot = settled_snapshot;
            } else {
                last_snapshot = take_snapshot(&watch_root);
            }

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

struct ReloadPayload {
    module: knr_core::Module,
    shaders_wgsl: HashMap<String, String>,
    changed_label: String,
}

/// Executes `knr dev --gui` with live hot-reload for MVU desktop UI apps and `.knr` GPU shaders,
/// reloading the UI tree and recompiling WGSL pipelines live without closing the desktop window.
pub fn execute_gui(
    target: Option<&Path>,
    debounce_ms: u64,
    poll_ms: u64,
    gas: u64,
    once: bool,
) -> Result<(), CliError> {
    let start_path = target.unwrap_or_else(|| Path::new("."));
    let (watch_root, entry_file) = resolve_watch_and_entry(start_path)?;

    let (initial_module, initial_shaders) =
        load_verified_gui_bundle(&entry_file).map_err(CliError::Other)?;

    // Fall back cleanly to standard GUI execution graph runner for non-MVU modules.
    if initial_module.get_function("init").is_none()
        || initial_module.get_function("view").is_none()
    {
        return crate::commands::run::execute(&entry_file, gas, false, true);
    }

    let mut init_platform = CliHostPlatform::new();
    let (init_state, _) = knr_interp::execute_module(
        &initial_module,
        "init",
        vec![],
        &mut init_platform,
        gas,
        false,
    )
    .map_err(CliError::Vm)?;

    let file_label = entry_file
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("knr app")
        .to_string();

    let initial_has_tick = initial_module
        .get_function("tick")
        .is_some_and(|f| f.params.len() >= 2);

    let module_cell = RefCell::new(initial_module);
    let state_cell = RefCell::new(init_state);
    let plat_cell = RefCell::new(init_platform);

    if once || !has_display_server() {
        let config = WindowConfig::interactive(format!("knr dev - {file_label}"), 1120, 800)
            .with_max_frames(1);
        let launch_res = launch_ui_app(
            config,
            initial_shaders,
            initial_has_tick,
            || eval_gui_view(&module_cell, &state_cell, &plat_cell, gas),
            |ev| dispatch_gui_event(&module_cell, &state_cell, &plat_cell, gas, ev, None),
            |dt, elapsed| {
                dispatch_gui_tick(&module_cell, &state_cell, &plat_cell, gas, dt, elapsed);
            },
        );

        match launch_res {
            Ok(res) => {
                println!(
                    "[DEV-GUI] Headless hot-reload check ok (hash=0x{:016x})",
                    res.final_pixel_hash
                );
            }
            Err(err) => {
                eprintln!("[DEV-GUI] Desktop window error: {err}");
            }
        }
        return Ok(());
    }

    eprintln!(
        "[DEV-GUI] Watching '{}' for live GUI & shader hot-reload (entrypoint: '{}')",
        watch_root.display(),
        entry_file.display()
    );

    let stop_flag = Arc::new(AtomicBool::new(false));
    let (tx, rx) = mpsc::channel::<ReloadPayload>();

    let watcher_stop = Arc::clone(&stop_flag);
    let watcher_root = watch_root.clone();
    let watcher_start = start_path.to_path_buf();
    let mut watcher_entry = entry_file.clone();

    let watcher_handle = std::thread::spawn(move || {
        let mut last_snapshot = take_snapshot(&watcher_root);
        let poll_dur = Duration::from_millis(poll_ms.max(20));

        while !watcher_stop.load(Ordering::Relaxed) {
            std::thread::sleep(poll_dur);
            if watcher_stop.load(Ordering::Relaxed) {
                break;
            }

            let current_snapshot = take_snapshot(&watcher_root);
            let mut changed_files = detect_changes(&last_snapshot, &current_snapshot);
            if changed_files.is_empty() {
                continue;
            }

            // True trailing-edge debounce, responsive to window shutdown.
            if debounce_ms > 0 {
                let mut settled_snapshot = current_snapshot;
                let target_quiet = Duration::from_millis(debounce_ms);
                let step = Duration::from_millis(poll_ms.min(debounce_ms).clamp(10, 40));
                let mut last_change_at = std::time::Instant::now();
                let max_wait_start = last_change_at;
                let max_total_wait =
                    Duration::from_millis(debounce_ms.saturating_mul(10).max(1500));

                while !watcher_stop.load(Ordering::Relaxed)
                    && last_change_at.elapsed() < target_quiet
                    && max_wait_start.elapsed() < max_total_wait
                {
                    std::thread::sleep(step);
                    let probe = take_snapshot(&watcher_root);
                    let more_changes = detect_changes(&settled_snapshot, &probe);
                    if !more_changes.is_empty() {
                        for p in more_changes {
                            if !changed_files.contains(&p) {
                                changed_files.push(p);
                            }
                        }
                        settled_snapshot = probe;
                        last_change_at = std::time::Instant::now();
                    }
                }
                last_snapshot = settled_snapshot;
            } else {
                last_snapshot = take_snapshot(&watcher_root);
            }

            if watcher_stop.load(Ordering::Relaxed) {
                break;
            }

            // If knr.toml changed, re-resolve entrypoint.
            if changed_files
                .iter()
                .any(|p| p.file_name().and_then(|n| n.to_str()) == Some("knr.toml"))
            {
                if let Ok((_, new_entry)) = resolve_watch_and_entry(&watcher_start) {
                    watcher_entry = new_entry;
                }
            }

            match load_verified_gui_bundle(&watcher_entry) {
                Ok((module, shaders_wgsl)) => {
                    if module.get_function("init").is_none()
                        || module.get_function("view").is_none()
                    {
                        eprintln!(
                            "[DEV-GUI] Hot-reload skipped: '{}' must define `init` and `view` (keeping last valid UI)",
                            watcher_entry.display()
                        );
                        continue;
                    }
                    let changed_label = watcher_entry
                        .strip_prefix(&watcher_root)
                        .unwrap_or(&watcher_entry)
                        .display()
                        .to_string();
                    if tx
                        .send(ReloadPayload {
                            module,
                            shaders_wgsl,
                            changed_label,
                        })
                        .is_err()
                    {
                        break;
                    }
                }
                Err(err) => {
                    eprintln!("[DEV-GUI] Hot-reload error (keeping last valid UI): {err}");
                }
            }
        }
    });

    let (cmd_tx, cmd_rx) = mpsc::channel::<UiEvent>();
    let config = WindowConfig::interactive(format!("knr dev - {file_label}"), 1120, 800);
    let launch_res = launch_ui_app_with_reload_and_effects(
        config,
        initial_shaders,
        initial_has_tick,
        || eval_gui_view(&module_cell, &state_cell, &plat_cell, gas),
        |ev| dispatch_gui_event(&module_cell, &state_cell, &plat_cell, gas, ev, Some(&cmd_tx)),
        |dt, elapsed| {
            dispatch_gui_tick(&module_cell, &state_cell, &plat_cell, gas, dt, elapsed);
        },
        || {
            let payload = rx.try_iter().last()?;
            apply_reload_payload(payload, &module_cell, &state_cell, &plat_cell, gas)
        },
        Some(cmd_rx),
    );

    stop_flag.store(true, Ordering::Relaxed);
    let _ = watcher_handle.join();

    match launch_res {
        Ok(res) => {
            if res.backend == WindowBackend::Headless {
                println!(
                    "[DEV-GUI] Headless hot-reload check ok (hash=0x{:016x})",
                    res.final_pixel_hash
                );
            }
        }
        Err(err) => {
            eprintln!("[DEV-GUI] Desktop window error: {err}");
        }
    }

    Ok(())
}

/// Parses, verifies, and compiles all `@shader` declarations for a GUI `.tsk`, `.knr`, or `.ssa` entrypoint.
fn load_verified_gui_bundle(
    entry: &Path,
) -> Result<(knr_core::Module, HashMap<String, String>), String> {
    if !entry.is_file() {
        return Err(format!("Entrypoint file '{}' not found", entry.display()));
    }

    let is_source = knr_core::is_source_file(entry);
    let module = if is_source {
        let src = fs::read_to_string(entry)
            .map_err(|e| format!("IO Error reading '{}': {e}", entry.display()))?;
        let _prog = knr_core::syntax::parse_tsx(&src).map_err(|e| format!("Syntax Error: {e}"))?;

        match knr_verify::verify_program_from_file(entry) {
            Ok(m) => m,
            Err(diagnostics) => {
                for diag in &diagnostics {
                    eprintln!("{diag}");
                }
                return Err(format!(
                    "Verification failed with {} diagnostic(s)",
                    diagnostics.len()
                ));
            }
        }
    } else {
        let module =
            knr_core::load_module_from_path(entry).map_err(|e| format!("Parse Error: {e}"))?;

        if let Err(diagnostics) = knr_verify::verify_module(&module) {
            for diag in &diagnostics {
                eprintln!("{diag}");
            }
            return Err(format!(
                "Verification failed with {} diagnostic(s)",
                diagnostics.len()
            ));
        }

        module
    };

    let source_map = if is_source && module.functions.iter().any(|f| f.is_shader) {
        knr_core::lower::lower_program_from_file_with_source_map(entry)
            .ok()
            .map(|(_, sm)| sm)
    } else {
        None
    };
    let wgsl_backend = knr_codegen::WgslBackend::new();
    let compiled_shaders = match source_map.as_ref() {
        Some(sm) => wgsl_backend.compile_all_shaders_with_source_map(&module, sm),
        None => wgsl_backend.compile_all_shaders(&module),
    }
    .map_err(|err| format!("WGSL Shader Error: {err}"))?;
    let mut shaders_wgsl = HashMap::with_capacity(compiled_shaders.len());
    for cs in compiled_shaders {
        shaders_wgsl.insert(cs.name, cs.wgsl);
    }

    Ok((module, shaders_wgsl))
}

fn collect_referenced_schema_names(
    module: &knr_core::Module,
    ty: &knr_core::Type,
    out: &mut BTreeSet<String>,
) {
    match ty {
        knr_core::Type::Struct(name) => {
            if out.insert(name.clone()) {
                if let Some(schema) = module.get_schema(name) {
                    for field in &schema.fields {
                        collect_referenced_schema_names(module, &field.ty, out);
                    }
                }
            }
        }
        knr_core::Type::Vec(inner) | knr_core::Type::Chan(inner) => {
            collect_referenced_schema_names(module, inner, out);
        }
        knr_core::Type::Map(k, v) => {
            collect_referenced_schema_names(module, k, out);
            collect_referenced_schema_names(module, v, out);
        }
        knr_core::Type::I64
        | knr_core::Type::F64
        | knr_core::Type::Bool
        | knr_core::Type::Str
        | knr_core::Type::Buf
        | knr_core::Type::Void => {}
    }
}

fn state_schema_changed(old_mod: &knr_core::Module, new_mod: &knr_core::Module) -> bool {
    let old_ret = old_mod.get_function("init").map(|f| &f.ret_ty);
    let new_ret = new_mod.get_function("init").map(|f| &f.ret_ty);
    if old_ret != new_ret {
        return true;
    }
    let Some(ret_ty) = new_ret else {
        return true;
    };
    let mut names = BTreeSet::new();
    collect_referenced_schema_names(new_mod, ret_ty, &mut names);
    for name in names {
        if old_mod.get_schema(&name) != new_mod.get_schema(&name) {
            return true;
        }
    }
    false
}

fn eval_gui_view(
    module_cell: &RefCell<knr_core::Module>,
    state_cell: &RefCell<Value>,
    plat_cell: &RefCell<CliHostPlatform>,
    gas: u64,
) -> UiNode {
    let module = module_cell.borrow();
    let st = state_cell.borrow().clone();
    let mut plat = plat_cell.borrow_mut();
    match knr_interp::execute_module(&module, "view", vec![st], &mut *plat, gas, false) {
        Ok((val, _)) => UiNode::from_value(&val).unwrap_or_default(),
        Err(e) => UiNode {
            text: format!("view() error: {e}"),
            ..Default::default()
        },
    }
}

fn format_key_event_combo(key: String, ctrl: bool, shift: bool, alt: bool) -> String {
    if (ctrl && key.contains("Ctrl+"))
        || (alt && key.contains("Alt+"))
        || (shift && key.contains("Shift+"))
    {
        key
    } else {
        knr_runtime::gui::format_key_combo(&key, ctrl, shift, alt)
    }
}

fn resize_dim_value(dim: u32, ty: &knr_core::Type) -> Value {
    match ty {
        knr_core::Type::I64 => Value::I64(dim as i64),
        _ => Value::F64(dim as f64),
    }
}

fn dispatch_gui_event(
    module_cell: &RefCell<knr_core::Module>,
    state_cell: &RefCell<Value>,
    plat_cell: &RefCell<CliHostPlatform>,
    gas: u64,
    ev: UiEvent,
    cmd_tx: Option<&mpsc::Sender<UiEvent>>,
) {
    let module = module_cell.borrow();
    let has_update = module.get_function("update").is_some();
    let has_on_slider = module.get_function("on_slider").is_some();
    let has_on_input = module.get_function("on_input").is_some();
    let has_on_key = module
        .get_function("on_key")
        .is_some_and(|f| f.params.len() == 2);
    let on_resize_types = module
        .get_function("on_resize")
        .filter(|f| f.params.len() == 3)
        .map(|f| (f.params[1].1.clone(), f.params[2].1.clone()));
    let has_effect = module.get_function("effect").is_some();
    let st = state_cell.borrow().clone();
    let mut plat = plat_cell.borrow_mut();
    let (action_label, res) = match ev {
        UiEvent::Click(action) | UiEvent::Toggle { action, .. } if has_update => {
            let r = knr_interp::execute_module(
                &module,
                "update",
                vec![st, Value::Str(action.clone())],
                &mut *plat,
                gas,
                false,
            );
            (action, r)
        }
        UiEvent::SliderChange { action, value } => {
            if has_on_slider {
                let r = knr_interp::execute_module(
                    &module,
                    "on_slider",
                    vec![st, Value::Str(action.clone()), Value::F64(value)],
                    &mut *plat,
                    gas,
                    false,
                );
                (action, r)
            } else if has_update {
                let r = knr_interp::execute_module(
                    &module,
                    "update",
                    vec![st, Value::Str(format!("{action}:{value:.3}"))],
                    &mut *plat,
                    gas,
                    false,
                );
                (action, r)
            } else {
                return;
            }
        }
        UiEvent::TextInput { action, value } => {
            if has_on_input {
                let r = knr_interp::execute_module(
                    &module,
                    "on_input",
                    vec![st, Value::Str(action.clone()), Value::Str(value)],
                    &mut *plat,
                    gas,
                    false,
                );
                (action, r)
            } else if has_update {
                let r = knr_interp::execute_module(
                    &module,
                    "update",
                    vec![st, Value::Str(format!("{action}:{value}"))],
                    &mut *plat,
                    gas,
                    false,
                );
                (action, r)
            } else {
                return;
            }
        }
        UiEvent::KeyPress {
            key,
            ctrl,
            shift,
            alt,
        } if has_on_key => {
            let combo = format_key_event_combo(key, ctrl, shift, alt);
            if combo.is_empty() {
                return;
            }
            let r = knr_interp::execute_module(
                &module,
                "on_key",
                vec![st, Value::Str(combo.clone())],
                &mut *plat,
                gas,
                false,
            );
            (format!("key:{combo}"), r)
        }
        UiEvent::Resize { width, height } => {
            if let Some((ref w_ty, ref h_ty)) = on_resize_types {
                let r = knr_interp::execute_module(
                    &module,
                    "on_resize",
                    vec![
                        st,
                        resize_dim_value(width, w_ty),
                        resize_dim_value(height, h_ty),
                    ],
                    &mut *plat,
                    gas,
                    false,
                );
                (format!("resize:{width}x{height}"), r)
            } else {
                return;
            }
        }
        _ => return,
    };
    if let Ok((next_state, _)) = res {
        if has_effect {
            if let Some(tx) = cmd_tx {
                if let Ok((cmd_val, _)) = knr_interp::execute_module(
                    &module,
                    "effect",
                    vec![next_state.clone(), Value::Str(action_label)],
                    &mut *plat,
                    gas,
                    false,
                ) {
                    if let Some(cmd) = UiCmd::from_value(&cmd_val) {
                        spawn_ui_cmd(cmd, tx.clone());
                    }
                }
            }
        }
        *state_cell.borrow_mut() = next_state;
    }
}

fn dispatch_gui_tick(
    module_cell: &RefCell<knr_core::Module>,
    state_cell: &RefCell<Value>,
    plat_cell: &RefCell<CliHostPlatform>,
    gas: u64,
    dt: f64,
    elapsed: f64,
) {
    let module = module_cell.borrow();
    let tick_arity = module
        .get_function("tick")
        .map_or(0, |f| f.params.len());
    if tick_arity < 2 {
        return;
    }
    let st = state_cell.borrow().clone();
    let mut plat = plat_cell.borrow_mut();
    let args = if tick_arity >= 3 {
        vec![st, Value::F64(dt), Value::F64(elapsed)]
    } else {
        vec![st, Value::F64(dt)]
    };
    if let Ok((next_state, _)) =
        knr_interp::execute_module(&module, "tick", args, &mut *plat, gas, false)
    {
        *state_cell.borrow_mut() = next_state;
    }
}

fn apply_reload_payload(
    payload: ReloadPayload,
    module_cell: &RefCell<knr_core::Module>,
    state_cell: &RefCell<Value>,
    plat_cell: &RefCell<CliHostPlatform>,
    gas: u64,
) -> Option<HotReloadUpdate> {
    let schema_changed = {
        let old_mod = module_cell.borrow();
        state_schema_changed(&old_mod, &payload.module)
    };
    let old_state = state_cell.borrow().clone();
    let mut plat = plat_cell.borrow_mut();

    // Preserve live state across UI/shader edits whenever the state schema is unchanged
    // and the new `view(old_state)` evaluates cleanly; otherwise re-run `init()`.
    let preserved_tree = if !schema_changed {
        knr_interp::execute_module(
            &payload.module,
            "view",
            vec![old_state],
            &mut *plat,
            gas,
            false,
        )
        .ok()
        .and_then(|(val, _)| UiNode::from_value(&val).ok())
    } else {
        None
    };

    let (new_state_opt, tree) = if let Some(tree) = preserved_tree {
        (None, tree)
    } else {
        let (fresh_state, _) = match knr_interp::execute_module(
            &payload.module,
            "init",
            vec![],
            &mut *plat,
            gas,
            false,
        ) {
            Ok(res) => res,
            Err(err) => {
                eprintln!("[DEV-GUI] Hot-reload init() error (keeping last valid UI): {err}");
                return None;
            }
        };
        let (val, _) = match knr_interp::execute_module(
            &payload.module,
            "view",
            vec![fresh_state.clone()],
            &mut *plat,
            gas,
            false,
        ) {
            Ok(res) => res,
            Err(err) => {
                eprintln!("[DEV-GUI] Hot-reload view() error (keeping last valid UI): {err}");
                return None;
            }
        };
        let tree = match UiNode::from_value(&val) {
            Ok(tree) => tree,
            Err(err) => {
                eprintln!(
                    "[DEV-GUI] Hot-reload view() returned invalid UI node (keeping last valid UI): {err}"
                );
                return None;
            }
        };
        (Some(fresh_state), tree)
    };
    drop(plat);

    let has_tick = payload
        .module
        .get_function("tick")
        .is_some_and(|f| f.params.len() >= 2);
    let shader_count = payload.shaders_wgsl.len();

    *module_cell.borrow_mut() = payload.module;
    if let Some(new_state) = new_state_opt {
        *state_cell.borrow_mut() = new_state;
    }

    eprintln!(
        "[DEV-GUI] Hot-reloaded {} ({} shader(s))",
        payload.changed_label, shader_count
    );

    Some(HotReloadUpdate {
        tree,
        shaders_wgsl: payload.shaders_wgsl,
        has_tick,
    })
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
        canonical.join("src/app.tsk"),
        canonical.join("src/main.tsk"),
        canonical.join("src/main.knr"),
        canonical.join("src/main.ssa"),
        canonical.join("app.tsk"),
        canonical.join("main.tsk"),
        canonical.join("main.knr"),
        canonical.join("main.ssa"),
    ];

    for candidate in &candidates {
        if candidate.is_file() {
            return Ok((canonical, candidate.clone()));
        }
    }

    // Search directory for any .tsk/.knr/.ssa file
    if let Some(found) = find_first_source_file(&canonical) {
        return Ok((canonical, found));
    }

    Err(CliError::Other(format!(
        "No entrypoint source file or knr.toml found in '{}'. Expected 'src/app.tsk' or 'src/main.tsk'.",
        canonical.display()
    )))
}

fn find_first_source_file(dir: &Path) -> Option<PathBuf> {
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() {
                if knr_core::is_source_file(&path) || path.to_string_lossy().ends_with(".ssa") {
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

    let is_source = knr_core::is_source_file(entry);

    let module = if is_source {
        let src = fs::read_to_string(entry).map_err(|e| format!("IO Error reading '{}': {e}", entry.display()))?;
        let _prog = knr_core::syntax::parse_tsx(&src).map_err(|e| format!("Syntax Error: {e}"))?;

        match knr_verify::verify_program_from_file(entry) {
            Ok(m) => m,
            Err(diagnostics) => {
                for diag in &diagnostics {
                    eprintln!("{diag}");
                }
                return Err(format!("Verification failed with {} diagnostic(s)", diagnostics.len()));
            }
        }
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
        || knr_core::is_source_file(Path::new(name))
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
