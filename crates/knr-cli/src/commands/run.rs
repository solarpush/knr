use std::path::Path;

use knr_interp::Value;

use crate::error::CliError;
use crate::platform::CliHostPlatform;

pub fn execute(file: &Path, gas: u64, trace: bool, gui: bool) -> Result<(), CliError> {
    let module = if file.to_string_lossy().ends_with(".knr") {
        let src = std::fs::read_to_string(file).map_err(|e| {
            eprintln!("{e}");
            CliError::Other(e.to_string())
        })?;
        let _prog = knr_core::syntax::parse_knr(&src).map_err(|err| {
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
    // with optional `update`, `tick`, `on_slider`, `on_input`, and `shader fn`),
    // launch the hardware-accelerated GPU (`wgpu 23`) desktop runtime with automatic CPU fallback!
    if gui && module.get_function("init").is_some() && module.get_function("view").is_some() {
        use knr_runtime::gui::{
            launch_ui_app, UiEvent, UiNode, WindowBackend, WindowConfig,
        };
        use std::cell::RefCell;
        use std::collections::HashMap;

        // Precompile all `shader fn` declarations in the module to validated WGSL
        let compiled_shaders = knr_codegen::WgslBackend::new()
            .compile_all_shaders(&module)
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

        let mut init_platform = CliHostPlatform::new();
        let (init_state, _) =
            knr_interp::execute_module(&module, "init", vec![], &mut init_platform, gas, false)
                .map_err(CliError::Vm)?;

        let state_cell = RefCell::new(init_state);
        let plat_cell = RefCell::new(init_platform);
        let file_label = file
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("knr app");

        let config = WindowConfig::interactive(format!("knr - {file_label}"), 1120, 800);
        let module_ref = &module;

        let launch_res = launch_ui_app(
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
                let res = match ev {
                    UiEvent::Click(action) | UiEvent::Toggle { action, .. } if has_update => {
                        knr_interp::execute_module(
                            module_ref,
                            "update",
                            vec![st, Value::Str(action)],
                            &mut *plat,
                            gas,
                            false,
                        )
                    }
                    UiEvent::SliderChange { action, value } => {
                        if has_on_slider {
                            knr_interp::execute_module(
                                module_ref,
                                "on_slider",
                                vec![st, Value::Str(action), Value::F64(value)],
                                &mut *plat,
                                gas,
                                false,
                            )
                        } else if has_update {
                            knr_interp::execute_module(
                                module_ref,
                                "update",
                                vec![st, Value::Str(format!("{action}:{value:.3}"))],
                                &mut *plat,
                                gas,
                                false,
                            )
                        } else {
                            return;
                        }
                    }
                    UiEvent::TextInput { action, value } => {
                        if has_on_input {
                            knr_interp::execute_module(
                                module_ref,
                                "on_input",
                                vec![st, Value::Str(action), Value::Str(value)],
                                &mut *plat,
                                gas,
                                false,
                            )
                        } else if has_update {
                            knr_interp::execute_module(
                                module_ref,
                                "update",
                                vec![st, Value::Str(format!("{action}:{value}"))],
                                &mut *plat,
                                gas,
                                false,
                            )
                        } else {
                            return;
                        }
                    }
                    _ => return,
                };
                if let Ok((next_state, _)) = res {
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
    let (val, trace_records) = knr_interp::execute_module(
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

        return Ok(());
    }

    if trace {
        let json_output = serde_json::to_string_pretty(&trace_records)
            .map_err(|e| CliError::Other(e.to_string()))?;
        println!("{json_output}");
    } else {
        println!("{val}");
    }

    Ok(())
}
