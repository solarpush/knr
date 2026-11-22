use std::path::Path;

use knr_interp::{ExecutionStats, Value};

use crate::error::CliError;
use crate::platform::CliHostPlatform;

pub fn execute(file: &Path, gas: u64, trace: bool, gui: bool) -> Result<(), CliError> {
    execute_with_profile(file, gas, trace, gui, false)
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

fn print_profile_table(stats: &ExecutionStats, to_stderr: bool) {
    let mut top_fns: Vec<(&str, u64, u64)> = stats
        .function_instructions
        .iter()
        .map(|(name, &instrs)| {
            let calls = stats.function_calls.get(name).copied().unwrap_or(0);
            (name.as_str(), calls, instrs)
        })
        .collect();
    for (name, &calls) in &stats.function_calls {
        if !stats.function_instructions.contains_key(name) {
            top_fns.push((name.as_str(), calls, 0));
        }
    }
    top_fns.sort_by(|a, b| b.2.cmp(&a.2).then_with(|| a.0.cmp(b.0)));

    let mut out = String::new();
    use std::fmt::Write as _;
    let _ = writeln!(out, "\n--- VM Profile ---");
    let _ = writeln!(out, "  total_instructions : {}", stats.total_instructions);
    let _ = writeln!(out, "  gas_used           : {}", stats.gas_used);
    let _ = writeln!(out, "  allocations        : {}", stats.allocations);
    let _ = writeln!(out, "  frees              : {}", stats.frees);
    if !top_fns.is_empty() {
        let _ = writeln!(out, "  Top functions:");
        let _ = writeln!(out, "    {:<28} {:>10} {:>14}", "Function", "Calls", "Instructions");
        let _ = writeln!(out, "    {}", "-".repeat(54));
        for (name, calls, instrs) in top_fns.into_iter().take(15) {
            let _ = writeln!(out, "    {name:<28} {calls:>10} {instrs:>14}");
        }
    }
    if to_stderr {
        eprint!("{out}");
    } else {
        print!("{out}");
    }
}

pub fn execute_with_profile(
    file: &Path,
    gas: u64,
    trace: bool,
    gui: bool,
    profile: bool,
) -> Result<(), CliError> {
    let is_source = knr_core::is_source_file(file);
    let module = if is_source {
        let src = std::fs::read_to_string(file).map_err(|e| {
            eprintln!("{e}");
            CliError::Other(e.to_string())
        })?;
        let _prog = knr_core::syntax::parse_tsx(&src).map_err(|err| {
            eprintln!("{err}");
            CliError::Parse(err)
        })?;
        knr_verify::verify_program_from_file(file).map_err(|diagnostics| {
            for diag in &diagnostics {
                eprintln!("{diag}");
            }
            CliError::Verify(diagnostics.len())
        })?
    } else {
        let module = knr_core::load_module_from_path(file).map_err(|err| {
            eprintln!("{err}");
            CliError::Parse(err)
        })?;

        if let Err(diagnostics) = knr_verify::verify_module(&module) {
            for diag in &diagnostics {
                eprintln!("{diag}");
            }
            return Err(CliError::Verify(diagnostics.len()));
        }

        module
    };

    // If --gui is requested and the module is a typed MVU / Animated Shader app (`init` + `view`,
    // with optional `update`, `tick`, `on_slider`, `on_input`, `on_key`, `on_resize`, and `shader fn`),
    // launch the hardware-accelerated GPU (`wgpu 23`) desktop runtime with automatic CPU fallback!
    if gui && module.get_function("init").is_some() && module.get_function("view").is_some() {
        use knr_runtime::gui::{
            launch_ui_app_with_reload_and_effects, spawn_ui_cmd, UiCmd, UiEvent, UiNode,
            WindowBackend, WindowConfig,
        };
        use std::cell::RefCell;
        use std::collections::HashMap;
        use std::sync::mpsc;

        // Precompile all `shader fn` declarations in the module to validated WGSL
        // using SourceMap param names when available (auto-binding `resolution`, `dt`, `mouse`, etc.)
        let source_map = if is_source && module.functions.iter().any(|f| f.is_shader) {
            knr_core::lower::lower_program_from_file_with_source_map(file)
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
        .map_err(|err| {
            eprintln!("{err}");
            CliError::Codegen(err)
        })?;
        let mut shaders_wgsl = HashMap::new();
        for cs in compiled_shaders {
            shaders_wgsl.insert(cs.name, cs.wgsl);
        }

        let tick_arity = module.get_function("tick").map(|f| f.params.len()).unwrap_or(0);
        let has_tick = tick_arity >= 2;
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

        let mut init_platform = CliHostPlatform::new();
        let (init_state, _) =
            knr_interp::execute_module(&module, "init", vec![], &mut init_platform, gas, false)
                .map_err(CliError::Vm)?;

        let state_cell = RefCell::new(init_state);
        let plat_cell = RefCell::new(init_platform);
        let (cmd_tx, cmd_rx) = mpsc::channel::<UiEvent>();
        let file_label = file
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("knr app");

        let config = WindowConfig::interactive(format!("knr - {file_label}"), 1120, 800);
        let module_ref = &module;

        let launch_res = launch_ui_app_with_reload_and_effects(
            config,
            shaders_wgsl,
            has_tick,
            || {
                let st = state_cell.borrow().clone();
                let mut plat = plat_cell.borrow_mut();
                match knr_interp::execute_module(module_ref, "view", vec![st], &mut *plat, gas, false)
                {
                    Ok((val, _)) => UiNode::from_value(&val).unwrap_or_default(),
                    Err(e) => UiNode {
                        text: format!("view() error: {e}"),
                        ..Default::default()
                    },
                }
            },
            |ev| {
                let st = state_cell.borrow().clone();
                let mut plat = plat_cell.borrow_mut();
                let (action_label, res) = match ev {
                    UiEvent::Click(action) | UiEvent::Toggle { action, .. } if has_update => {
                        let r = knr_interp::execute_module(
                            module_ref,
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
                                module_ref,
                                "on_slider",
                                vec![st, Value::Str(action.clone()), Value::F64(value)],
                                &mut *plat,
                                gas,
                                false,
                            );
                            (action, r)
                        } else if has_update {
                            let r = knr_interp::execute_module(
                                module_ref,
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
                                module_ref,
                                "on_input",
                                vec![st, Value::Str(action.clone()), Value::Str(value)],
                                &mut *plat,
                                gas,
                                false,
                            );
                            (action, r)
                        } else if has_update {
                            let r = knr_interp::execute_module(
                                module_ref,
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
                            module_ref,
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
                                module_ref,
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
                        if let Ok((cmd_val, _)) = knr_interp::execute_module(
                            module_ref,
                            "effect",
                            vec![next_state.clone(), Value::Str(action_label)],
                            &mut *plat,
                            gas,
                            false,
                        ) {
                            if let Some(cmd) = UiCmd::from_value(&cmd_val) {
                                spawn_ui_cmd(cmd, cmd_tx.clone());
                            }
                        }
                    }
                    *state_cell.borrow_mut() = next_state;
                }
            },
            |dt, elapsed| {
                if !has_tick {
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
                    knr_interp::execute_module(module_ref, "tick", args, &mut *plat, gas, false)
                {
                    *state_cell.borrow_mut() = next_state;
                }
            },
            || None,
            Some(cmd_rx),
        );

        match launch_res {
            Ok(res) => {
                if res.backend == WindowBackend::Headless {
                    println!(
                        "[GUI] Headless environment detected: rendered MVU UI snapshot (hash=0x{:016x})",
                        res.final_pixel_hash
                    );
                }
            }
            Err(err) => {
                eprintln!("[GUI] Desktop window error: {err}");
            }
        }
        return Ok(());
    }

    let entry_func = module
        .get_function("main")
        .or_else(|| module.functions.first())
        .ok_or_else(|| CliError::Other("Module contains no functions to execute".to_string()))?;

    let entry_name = entry_func.name.clone();

    let args: Vec<Value> = entry_func
        .params
        .iter()
        .map(|(_, ty)| knr_interp::default_value_for_type_with_schemas(ty, &module.schemas))
        .collect();

    let mut platform = CliHostPlatform::new();

    let should_trace = trace || gui;
    let (val, trace_records, profile_stats) = if profile {
        let (v, tr, st) = knr_interp::execute_module_with_stats(
            &module,
            &entry_name,
            args,
            &mut platform,
            gas,
            should_trace,
        )
        .map_err(|err| {
            match &err {
                knr_interp::VmError::GasExhausted { .. } => eprintln!("{err} (gas limit)"),
                _ => eprintln!("{err}"),
            }
            CliError::Vm(err)
        })?;
        (v, tr, Some(st))
    } else {
        let (v, tr) = knr_interp::execute_module(
            &module,
            &entry_name,
            args,
            &mut platform,
            gas,
            should_trace,
        )
        .map_err(|err| {
            match &err {
                knr_interp::VmError::GasExhausted { .. } => eprintln!("{err} (gas limit)"),
                _ => eprintln!("{err}"),
            }
            CliError::Vm(err)
        })?;
        (v, tr, None)
    };

    if gui {
        use knr_runtime::gui::{
            launch_desktop_window, EdgeType, ExecutionGraph, GraphEdge, GraphNode, HeadlessCanvas,
            PipelineStage, WindowBackend, WindowConfig,
        };

        let file_label = file
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("knr execution");
        let mut graph = ExecutionGraph::new(format!("Execution Graph - {file_label}"));

        // Pipeline stage nodes: AST frontend and knr-verify judge
        let ast_node = GraphNode::new("ast", "Frontend AST", PipelineStage::FrontendAst)
            .with_subtitle("Source parsing & lowering");
        let verify_node = GraphNode::new("verify", "knr-verify", PipelineStage::VerifyPass)
            .with_subtitle("Static verification");
        graph.add_node(ast_node);
        graph.add_node(verify_node);
        graph.add_edge(GraphEdge::new("ast", "verify", EdgeType::ControlFlow));

        // Connect verify to entry block bb0 if present
        if entry_func.blocks.iter().any(|b| b.id == 0) {
            graph.add_edge(GraphEdge::new("verify", "bb0", EdgeType::ControlFlow));
        }

        // Construct nodes for BasicBlocks of the executing function
        for block in &entry_func.blocks {
            let node_id = format!("bb{}", block.id);
            let label = format!("bb{} ({} instrs)", block.id, block.instructions.len());

            // Build code snippet
            let mut snippet = String::new();
            for (idx, inst) in block.instructions.iter().take(3).enumerate() {
                if idx > 0 {
                    snippet.push('\n');
                }
                snippet.push_str(&knr_core::format_instruction(inst));
            }
            if !block.instructions.is_empty() {
                snippet.push('\n');
            }
            snippet.push_str(&knr_core::format_terminator(&block.terminator));

            let mut node = GraphNode::new(&node_id, label, PipelineStage::BasicBlock)
                .with_subtitle(format!("{} instructions", block.instructions.len()))
                .with_code(snippet);

            node.metadata.insert(
                "instruction_count".to_string(),
                block.instructions.len().to_string(),
            );
            graph.add_node(node);

            // Connect branch / jump terminator edges with edge labels
            match &block.terminator {
                knr_core::Terminator::Ret(_) => {}
                knr_core::Terminator::Br(target)
                | knr_core::Terminator::BrArgs { target, .. } => {
                    let edge = GraphEdge::new(&node_id, format!("bb{target}"), EdgeType::ControlFlow)
                        .with_label("jump");
                    graph.add_edge(edge);
                }
                knr_core::Terminator::BrIf {
                    cond,
                    then_blk,
                    else_blk,
                }
                | knr_core::Terminator::BrIfArgs {
                    cond,
                    then_blk,
                    else_blk,
                    ..
                } => {
                    let edge_then = GraphEdge::new(
                        &node_id,
                        format!("bb{then_blk}"),
                        EdgeType::BranchTrue,
                    )
                    .with_label(format!("%{cond} == true"));
                    graph.add_edge(edge_then);

                    let edge_else = GraphEdge::new(
                        &node_id,
                        format!("bb{else_blk}"),
                        EdgeType::BranchFalse,
                    )
                    .with_label(format!("%{cond} == false"));
                    graph.add_edge(edge_else);
                }
            }
        }

        // Add additional module functions if present
        for func in &module.functions {
            if func.name != entry_name {
                let fn_node = GraphNode::new(
                    &func.name,
                    format!("fn {}", func.name),
                    PipelineStage::InterpOracle,
                )
                .with_subtitle(format!("{} blocks", func.blocks.len()));
                graph.add_node(fn_node);
            }
        }

        // Compute automated topological layout
        let config = WindowConfig::new(format!("knr - {file_label}"), 900, 600);
        graph.compute_layout(config.width, config.height);

        // Step the graph using graph.update_from_trace(&trace)
        let final_step = trace_records.len().saturating_sub(1);
        let runtime_traces: Vec<knr_runtime::gui::TraceRecord> = trace_records
            .iter()
            .map(|r| knr_runtime::gui::TraceRecord {
                step: r.step,
                line: r.line,
                block: r.block,
                opcode: r.opcode.clone(),
                dst_reg: r.dst_reg.clone(),
                new_value: r.new_value.clone(),
            })
            .collect();
        graph.update_from_trace(&runtime_traces, final_step);

        // Launch native desktop window
        let launch_res = launch_desktop_window(config, |canvas, _frame| {
            graph.render_to_canvas(canvas);
            false
        });

        match launch_res {
            Ok(res) => {
                if res.backend == WindowBackend::Headless {
                    println!("[GUI] Headless environment detected: rendered off-screen canvas snapshot");
                }
            }
            Err(_) => {
                println!("[GUI] Headless environment detected: rendered off-screen canvas snapshot");
                let mut headless = HeadlessCanvas::new(900, 600);
                graph.render_to_canvas(headless.canvas_mut());
            }
        }

        if trace {
            let json_output = serde_json::to_string_pretty(&trace_records)
                .map_err(|e| CliError::Other(e.to_string()))?;
            println!("{json_output}");
        }
        if let Some(ref stats) = profile_stats {
            print_profile_table(stats, trace);
        }

        return Ok(());
    }

    if trace {
        let json_output = serde_json::to_string_pretty(&trace_records)
            .map_err(|e| CliError::Other(e.to_string()))?;
        println!("{json_output}");
        if let Some(ref stats) = profile_stats {
            print_profile_table(stats, true);
        }
    } else {
        println!("{val}");
        if let Some(ref stats) = profile_stats {
            print_profile_table(stats, false);
        }
    }

    Ok(())
}
