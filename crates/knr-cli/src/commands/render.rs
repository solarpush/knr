use std::collections::{HashMap, HashSet};
use std::ffi::c_void;
use std::fs;
use std::path::{Path, PathBuf};

use knr_codegen::CraneliftBackend;
use knr_core::{BasicBlock, Function, Instruction, Module, Op, RegId, Terminator, Type};
use knr_interp::Value;
use knr_runtime::gui::{
    audit_layout_with_images, pixmap_fnv_hash, RenderInteractionState, UiNode, UiRenderer,
};
use knr_runtime::mock::MockPlatform;
use serde::{Deserialize, Serialize};
use tiny_skia::Pixmap;

use crate::args::{RenderArgs, RenderStep};
use crate::error::CliError;

type JitShaderEvalFn = extern "C" fn(f64, f64, f64, f64, f64, i64) -> i64;

struct CompiledJitShader {
    _backend: CraneliftBackend,
    eval_fn: JitShaderEvalFn,
}

#[derive(Debug, Deserialize)]
struct RenderFlowSpec {
    #[serde(default)]
    width: Option<u32>,
    #[serde(default)]
    height: Option<u32>,
    #[serde(default)]
    scale: Option<f32>,
    #[serde(default)]
    time: Option<f32>,
    steps: Vec<FlowStepSpec>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum FlowStepSpec {
    Action {
        action: String,
    },
    Click {
        key: String,
    },
    Input {
        key: String,
        value: String,
    },
    Slider {
        key: String,
        value: f64,
    },
    Tick {
        dt: f64,
    },
    AssertText {
        #[serde(default)]
        contains: Option<String>,
        #[serde(default)]
        not_contains: Option<String>,
    },
    AssertKey {
        key: String,
        #[serde(default)]
        absent: Option<bool>,
    },
    AssertAudit {
        #[serde(default)]
        max_errors: Option<usize>,
        #[serde(default)]
        max_warnings: Option<usize>,
    },
    Snapshot {
        output: PathBuf,
        #[serde(default)]
        width: Option<u32>,
        #[serde(default)]
        height: Option<u32>,
    },
}

#[derive(Debug, Serialize)]
struct FlowStepResult {
    index: usize,
    step_type: String,
    passed: bool,
    detail: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    output_png: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pixel_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    audit_errors: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    audit_warnings: Option<usize>,
}

/// Executes `knr render`: statically verifies the UI module, runs `init()` + `--action`/`--click`/`--input`/`--slider`
/// or `--flow` E2E steps + `view(&state)`, renders real `@shader` previews via Cranelift JIT (with automatic fast/VM fallback),
/// saves crisp HiDPI PNG(s), optionally generates a visual diff PNG against `--baseline`, and runs automated visual auditing.
pub fn execute(args: &RenderArgs) -> Result<(), CliError> {
    let (module, known_shaders) = load_verify_and_compile_shaders(&args.file)?;

    if let Some(ref flow_path) = args.flow {
        return execute_flow(args, &module, &known_shaders, flow_path);
    }

    let effective_steps: Vec<RenderStep> = if !args.steps.is_empty() {
        args.steps.clone()
    } else {
        args.actions
            .iter()
            .cloned()
            .map(RenderStep::Action)
            .collect()
    };

    let (root_node, simulated_steps) =
        evaluate_ui_tree(&module, &effective_steps, args.time, args.gas)?;

    // Pre-compile real @shader functions via Cranelift JIT (primary) + Fast zero-alloc parallel SSA evaluator (W^X-safe fallback)
    let jit_shaders = compile_shaders_jit(&module);
    let fast_shaders = FastShaderProgram::compile(&module);
    let module_ref = &module;
    let shader_provider = |shader_name: &str,
                           req_w: u32,
                           req_h: u32,
                           elapsed: f32,
                           uniforms: &[f64]|
     -> Option<Pixmap> {
        if let Some(jit) = jit_shaders.get(shader_name) {
            return render_shader_jit(jit.eval_fn, req_w, req_h, elapsed as f64, uniforms);
        }
        if let Some(pm) = fast_shaders.render(shader_name, req_w, req_h, elapsed as f64, uniforms) {
            return Some(pm);
        }
        render_shader_vm(module_ref, shader_name, req_w, req_h, elapsed as f64, uniforms)
    };

    let renderer = UiRenderer::new();
    let interaction = RenderInteractionState {
        elapsed_sec: args.time,
        ..Default::default()
    };

    if !args.viewports.is_empty() {
        return execute_multi_viewport(
            args,
            &root_node,
            &known_shaders,
            &simulated_steps,
            &renderer,
            &interaction,
            &shader_provider,
        );
    }

    // Single-viewport render + audit
    let (pixmap, layout_root, _hit_boxes) = renderer.render_ui_with_shaders(
        &root_node,
        args.width,
        args.height,
        args.scale,
        &interaction,
        Some(&shader_provider),
    );
    let pixel_hash = pixmap_fnv_hash(&pixmap);

    let output_png: Option<PathBuf> = if let Some(ref out) = args.output {
        Some(out.clone())
    } else if !args.audit && !args.json {
        let stem = args
            .file
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("render");
        Some(PathBuf::from(format!("{stem}.png")))
    } else {
        None
    };

    if let Some(ref png_path) = output_png {
        save_pixmap_png(&pixmap, png_path)?;
    }

    let report = audit_layout_with_images(
        &layout_root,
        args.width as f32,
        args.height as f32,
        &known_shaders,
        &|ch, font_family, font_weight| renderer.has_glyph(ch, font_family, font_weight),
        &|src| renderer.can_load_image(src),
    );

    // Optional baseline PNG visual regression comparison + optional --diff-out magenta diff PNG
    let mut baseline_failed = false;
    let mut baseline_summary: Option<(f64, bool)> = None;
    let mut written_diff_png: Option<PathBuf> = None;

    if let Some(ref base_path) = args.baseline {
        let base_pm = Pixmap::load_png(base_path).map_err(|e| {
            CliError::Other(format!(
                "Failed to load baseline PNG '{}': {e}",
                base_path.display()
            ))
        })?;
        let diff_pct = compare_pixmaps_diff_pct(&pixmap, &base_pm);
        let matched = diff_pct <= args.max_diff + 1e-9;
        if !matched {
            baseline_failed = true;
        }
        if diff_pct > 0.0 {
            if let Some(ref diff_path) = args.diff_out {
                if let Some(diff_pm) = generate_visual_diff_pixmap(&base_pm, &pixmap) {
                    save_pixmap_png(&diff_pm, diff_path)?;
                    written_diff_png = Some(diff_path.clone());
                }
            }
        }
        baseline_summary = Some((diff_pct, matched));
    }

    let mut report_json = serde_json::to_value(&report)
        .map_err(|e| CliError::Other(format!("Failed to serialize audit report: {e}")))?;
    if let Some(obj) = report_json.as_object_mut() {
        obj.insert(
            "output_png".to_string(),
            match &output_png {
                Some(p) => serde_json::Value::String(p.display().to_string()),
                None => serde_json::Value::Null,
            },
        );
        obj.insert("scale".to_string(), serde_json::json!(args.scale));
        obj.insert("time".to_string(), serde_json::json!(args.time));
        obj.insert(
            "pixel_hash".to_string(),
            serde_json::json!(format!("0x{pixel_hash:016x}")),
        );
        if !simulated_steps.is_empty() {
            obj.insert(
                "simulated_steps".to_string(),
                serde_json::Value::Array(simulated_steps),
            );
        }
        if let Some((diff_pct, matched)) = baseline_summary {
            obj.insert("baseline_diff_pct".to_string(), serde_json::json!(diff_pct));
            obj.insert("baseline_match".to_string(), serde_json::json!(matched));
        }
        if let Some(ref dp) = written_diff_png {
            obj.insert(
                "diff_png".to_string(),
                serde_json::Value::String(dp.display().to_string()),
            );
        }
    }

    if args.json {
        let pretty = serde_json::to_string_pretty(&report_json)
            .map_err(|e| CliError::Other(e.to_string()))?;
        println!("{pretty}");
    } else {
        if let Some(ref png_path) = output_png {
            println!(
                "Rendered {} ({}x{} @ {:.1}x -> {}x{} px, hash=0x{:016x})",
                png_path.display(),
                args.width,
                args.height,
                args.scale,
                pixmap.width(),
                pixmap.height(),
                pixel_hash
            );
        }
        if let Some((diff_pct, matched)) = baseline_summary {
            println!(
                "Baseline: {} | diff={:.4}% (max={:.4}%)",
                if matched { "match" } else { "mismatch" },
                diff_pct,
                args.max_diff
            );
        }
        if let Some(ref dp) = written_diff_png {
            println!("Visual diff PNG written to {}", dp.display());
        }
        println!(
            "Audit: {} | nodes={} | content_height={:.0}px | errors={} | warnings={}",
            report.status,
            report.node_count,
            report.content_height,
            report.error_count,
            report.warning_count
        );
        if args.audit || report.error_count > 0 || report.warning_count > 0 {
            print_audit_issues(&report_json);
        }
    }

    if args.assert_audit && report.error_count > 0 {
        return Err(CliError::Other(format!(
            "Visual audit assertion failed with {} error(s)",
            report.error_count
        )));
    }
    if baseline_failed {
        let (diff_pct, _) = baseline_summary.unwrap_or((100.0, false));
        return Err(CliError::Other(format!(
            "Visual baseline regression: {diff_pct:.4}% differing pixels exceeds --max-diff {:.4}%",
            args.max_diff
        )));
    }

    Ok(())
}

/// Programmatic headless render + audit entry point shared by the built-in MCP server (`knr_render_audit`).
#[allow(clippy::too_many_arguments)]
pub fn render_and_audit_for_mcp(
    file: &Path,
    width: u32,
    height: u32,
    scale: f32,
    time: f32,
    steps: &[RenderStep],
    output: Option<&Path>,
    gas: u64,
) -> Result<(serde_json::Value, Pixmap), CliError> {
    let (module, known_shaders) = load_verify_and_compile_shaders(file)?;
    let (root_node, simulated_steps) = evaluate_ui_tree(&module, steps, time, gas)?;

    let jit_shaders = compile_shaders_jit(&module);
    let fast_shaders = FastShaderProgram::compile(&module);
    let module_ref = &module;
    let shader_provider = |shader_name: &str,
                           req_w: u32,
                           req_h: u32,
                           elapsed: f32,
                           uniforms: &[f64]|
     -> Option<Pixmap> {
        if let Some(jit) = jit_shaders.get(shader_name) {
            return render_shader_jit(jit.eval_fn, req_w, req_h, elapsed as f64, uniforms);
        }
        if let Some(pm) = fast_shaders.render(shader_name, req_w, req_h, elapsed as f64, uniforms) {
            return Some(pm);
        }
        render_shader_vm(module_ref, shader_name, req_w, req_h, elapsed as f64, uniforms)
    };

    let renderer = UiRenderer::new();
    let interaction = RenderInteractionState {
        elapsed_sec: time,
        ..Default::default()
    };
    let (pixmap, layout_root, _hit_boxes) = renderer.render_ui_with_shaders(
        &root_node,
        width,
        height,
        scale,
        &interaction,
        Some(&shader_provider),
    );
    let pixel_hash = pixmap_fnv_hash(&pixmap);

    if let Some(png_path) = output {
        save_pixmap_png(&pixmap, png_path)?;
    }

    let report = audit_layout_with_images(
        &layout_root,
        width as f32,
        height as f32,
        &known_shaders,
        &|ch, font_family, font_weight| renderer.has_glyph(ch, font_family, font_weight),
        &|src| renderer.can_load_image(src),
    );

    let mut report_json = serde_json::to_value(&report)
        .map_err(|e| CliError::Other(format!("Failed to serialize audit report: {e}")))?;
    if let Some(obj) = report_json.as_object_mut() {
        obj.insert(
            "output_png".to_string(),
            match output {
                Some(p) => serde_json::Value::String(p.display().to_string()),
                None => serde_json::Value::Null,
            },
        );
        obj.insert("scale".to_string(), serde_json::json!(scale));
        obj.insert("time".to_string(), serde_json::json!(time));
        obj.insert(
            "pixel_hash".to_string(),
            serde_json::json!(format!("0x{pixel_hash:016x}")),
        );
        if !simulated_steps.is_empty() {
            obj.insert(
                "simulated_steps".to_string(),
                serde_json::Value::Array(simulated_steps),
            );
        }
    }

    Ok((report_json, pixmap))
}

fn load_verify_and_compile_shaders(file: &Path) -> Result<(Module, Vec<String>), CliError> {
    let module = if knr_core::is_source_file(file) {
        let src = fs::read_to_string(file).map_err(|e| {
            eprintln!("Error reading '{}': {e}", file.display());
            CliError::Io(e)
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

    let compiled_shaders = knr_codegen::WgslBackend::new()
        .compile_all_shaders(&module)
        .map_err(|err| {
            eprintln!("{err}");
            CliError::Codegen(err)
        })?;
    let known_shaders: Vec<String> = compiled_shaders.iter().map(|s| s.name.clone()).collect();
    Ok((module, known_shaders))
}

fn evaluate_ui_tree(
    module: &Module,
    steps: &[RenderStep],
    time: f32,
    gas: u64,
) -> Result<(UiNode, Vec<serde_json::Value>), CliError> {
    let mut mock = MockPlatform::new();
    let has_init = module.get_function("init").is_some();
    let view_fn = module.get_function("view").ok_or_else(|| {
        CliError::Other(
            "Module must define `view(state)` (or zero-argument `view()`) for `knr render`"
                .to_string(),
        )
    })?;
    let view_arity = view_fn.params.len();
    let mut simulated_steps = Vec::new();

    let view_val = if has_init && view_arity >= 1 {
        let (mut state, _) =
            knr_interp::execute_module(module, "init", vec![], &mut mock, gas, false)
                .map_err(CliError::Vm)?;

        for step in steps {
            let (next_state, step_meta) = apply_render_step(module, state, step, &mut mock, gas)?;
            state = next_state;
            simulated_steps.push(step_meta);
        }

        if time > 0.0 {
            state = apply_tick(module, state, time as f64, time as f64, &mut mock, gas);
        }

        let (val, _) =
            knr_interp::execute_module(module, "view", vec![state], &mut mock, gas, false)
                .map_err(CliError::Vm)?;
        val
    } else if view_arity == 0 {
        let (val, _) = knr_interp::execute_module(module, "view", vec![], &mut mock, gas, false)
            .map_err(CliError::Vm)?;
        val
    } else {
        return Err(CliError::Other(
            "Module defines `view(state)` without `init()`; add `export function init()` to render"
                .to_string(),
        ));
    };

    let root_node = UiNode::from_value(&view_val)?;
    Ok((root_node, simulated_steps))
}

fn apply_tick(
    module: &Module,
    state: Value,
    dt: f64,
    elapsed: f64,
    mock: &mut MockPlatform,
    gas: u64,
) -> Value {
    let tick_arity = module
        .get_function("tick")
        .map(|f| f.params.len())
        .unwrap_or(0);
    if tick_arity >= 3 {
        if let Ok((next_state, _)) = knr_interp::execute_module(
            module,
            "tick",
            vec![state.clone(), Value::F64(dt), Value::F64(elapsed)],
            mock,
            gas,
            false,
        ) {
            return next_state;
        }
    } else if tick_arity == 2 {
        if let Ok((next_state, _)) = knr_interp::execute_module(
            module,
            "tick",
            vec![state.clone(), Value::F64(dt)],
            mock,
            gas,
            false,
        ) {
            return next_state;
        }
    }
    state
}

fn apply_render_step(
    module: &Module,
    state: Value,
    step: &RenderStep,
    mock: &mut MockPlatform,
    gas: u64,
) -> Result<(Value, serde_json::Value), CliError> {
    let has_update = module.get_function("update").is_some();
    let has_on_input = module.get_function("on_input").is_some();
    let has_on_slider = module.get_function("on_slider").is_some();

    match step {
        RenderStep::Action(act) => {
            if !has_update {
                return Err(CliError::Other(format!(
                    "Cannot dispatch action '{act}': module does not define `update(state, action)`"
                )));
            }
            let (next_state, _) = knr_interp::execute_module(
                module,
                "update",
                vec![state, Value::Str(act.clone())],
                mock,
                gas,
                false,
            )
            .map_err(CliError::Vm)?;
            Ok((
                next_state,
                serde_json::json!({ "kind": "action", "action": act }),
            ))
        }
        RenderStep::Click(target_key) => {
            let (cur_view, _) = knr_interp::execute_module(
                module,
                "view",
                vec![state.clone()],
                mock,
                gas,
                false,
            )
            .map_err(CliError::Vm)?;
            let cur_tree = UiNode::from_value(&cur_view)?;
            let found = find_widget_by_key_or_action(&cur_tree, target_key).ok_or_else(|| {
                CliError::Other(format!(
                    "Cannot simulate click '{target_key}': no widget found with key or action '{target_key}'"
                ))
            })?;
            if found.action.is_empty() {
                return Err(CliError::Other(format!(
                    "Widget '{target_key}' has no interactive action to click"
                )));
            }
            if !has_update {
                return Err(CliError::Other(format!(
                    "Cannot simulate click '{target_key}': module does not define `update(state, action)`"
                )));
            }
            let act = found.action.clone();
            let (next_state, _) = knr_interp::execute_module(
                module,
                "update",
                vec![state, Value::Str(act.clone())],
                mock,
                gas,
                false,
            )
            .map_err(CliError::Vm)?;
            Ok((
                next_state,
                serde_json::json!({
                    "kind": "click",
                    "target": target_key,
                    "resolved_action": act
                }),
            ))
        }
        RenderStep::Input { key, value } => {
            let (cur_view, _) = knr_interp::execute_module(
                module,
                "view",
                vec![state.clone()],
                mock,
                gas,
                false,
            )
            .map_err(CliError::Vm)?;
            let cur_tree = UiNode::from_value(&cur_view)?;
            let found = find_widget_by_key_or_action(&cur_tree, key).ok_or_else(|| {
                CliError::Other(format!(
                    "Cannot simulate input '{key}=...': no widget found with key or action '{key}'"
                ))
            })?;
            let act = if found.action.is_empty() {
                key.clone()
            } else {
                found.action.clone()
            };
            let (next_state, _) = if has_on_input {
                knr_interp::execute_module(
                    module,
                    "on_input",
                    vec![state, Value::Str(act.clone()), Value::Str(value.clone())],
                    mock,
                    gas,
                    false,
                )
                .map_err(CliError::Vm)?
            } else if has_update {
                knr_interp::execute_module(
                    module,
                    "update",
                    vec![state, Value::Str(format!("{act}:{value}"))],
                    mock,
                    gas,
                    false,
                )
                .map_err(CliError::Vm)?
            } else {
                return Err(CliError::Other(format!(
                    "Cannot simulate input '{key}': module defines neither `on_input` nor `update`"
                )));
            };
            Ok((
                next_state,
                serde_json::json!({
                    "kind": "input",
                    "target": key,
                    "resolved_action": act,
                    "value": value
                }),
            ))
        }
        RenderStep::Slider { key, value } => {
            let (cur_view, _) = knr_interp::execute_module(
                module,
                "view",
                vec![state.clone()],
                mock,
                gas,
                false,
            )
            .map_err(CliError::Vm)?;
            let cur_tree = UiNode::from_value(&cur_view)?;
            let found = find_widget_by_key_or_action(&cur_tree, key).ok_or_else(|| {
                CliError::Other(format!(
                    "Cannot simulate slider '{key}=...': no widget found with key or action '{key}'"
                ))
            })?;
            let act = if found.action.is_empty() {
                key.clone()
            } else {
                found.action.clone()
            };
            let (next_state, _) = if has_on_slider {
                knr_interp::execute_module(
                    module,
                    "on_slider",
                    vec![state, Value::Str(act.clone()), Value::F64(*value)],
                    mock,
                    gas,
                    false,
                )
                .map_err(CliError::Vm)?
            } else if has_update {
                knr_interp::execute_module(
                    module,
                    "update",
                    vec![state, Value::Str(format!("{act}:{value:.3}"))],
                    mock,
                    gas,
                    false,
                )
                .map_err(CliError::Vm)?
            } else {
                return Err(CliError::Other(format!(
                    "Cannot simulate slider '{key}': module defines neither `on_slider` nor `update`"
                )));
            };
            Ok((
                next_state,
                serde_json::json!({
                    "kind": "slider",
                    "target": key,
                    "resolved_action": act,
                    "value": value
                }),
            ))
        }
    }
}

fn execute_multi_viewport<F>(
    args: &RenderArgs,
    root_node: &UiNode,
    known_shaders: &[String],
    simulated_steps: &[serde_json::Value],
    renderer: &UiRenderer,
    interaction: &RenderInteractionState,
    shader_provider: &F,
) -> Result<(), CliError>
where
    F: Fn(&str, u32, u32, f32, &[f64]) -> Option<Pixmap>,
{
    let multi = args.viewports.len() > 1;
    let mut vp_entries = Vec::with_capacity(args.viewports.len());
    let mut total_errors = 0usize;
    let mut total_warnings = 0usize;
    let mut passed_viewports = 0usize;
    let mut failed_viewports = 0usize;

    for vp in &args.viewports {
        let (pixmap, layout_root, _hit_boxes) = renderer.render_ui_with_shaders(
            root_node,
            vp.width,
            vp.height,
            args.scale,
            interaction,
            Some(shader_provider),
        );
        let pixel_hash = pixmap_fnv_hash(&pixmap);

        let vp_output_png: Option<PathBuf> = args.output.as_ref().map(|base| {
            if multi {
                viewport_output_path(base, &vp.name)
            } else {
                base.clone()
            }
        });

        if let Some(ref png_path) = vp_output_png {
            save_pixmap_png(&pixmap, png_path)?;
        }

        let report = audit_layout_with_images(
            &layout_root,
            vp.width as f32,
            vp.height as f32,
            known_shaders,
            &|ch, font_family, font_weight| renderer.has_glyph(ch, font_family, font_weight),
            &|src| renderer.can_load_image(src),
        );

        total_errors += report.error_count;
        total_warnings += report.warning_count;
        if report.error_count == 0 {
            passed_viewports += 1;
        } else {
            failed_viewports += 1;
        }

        let mut vp_json = serde_json::to_value(&report)
            .map_err(|e| CliError::Other(format!("Failed to serialize audit report: {e}")))?;
        if let Some(obj) = vp_json.as_object_mut() {
            obj.insert("viewport".to_string(), serde_json::json!(vp.name));
            obj.insert("scale".to_string(), serde_json::json!(args.scale));
            obj.insert(
                "pixel_hash".to_string(),
                serde_json::json!(format!("0x{pixel_hash:016x}")),
            );
            obj.insert(
                "output_png".to_string(),
                match &vp_output_png {
                    Some(p) => serde_json::Value::String(p.display().to_string()),
                    None => serde_json::Value::Null,
                },
            );
        }

        if !args.json {
            let png_note = vp_output_png
                .as_ref()
                .map(|p| format!(" -> {}", p.display()))
                .unwrap_or_default();
            println!(
                "Viewport {:<10} ({}x{} @ {:.1}x): {} | nodes={} | errors={} | warnings={} | hash=0x{:016x}{}",
                vp.name,
                vp.width,
                vp.height,
                args.scale,
                report.status,
                report.node_count,
                report.error_count,
                report.warning_count,
                pixel_hash,
                png_note
            );
            if args.audit || report.error_count > 0 || report.warning_count > 0 {
                print_audit_issues(&vp_json);
            }
        }

        vp_entries.push(vp_json);
    }

    let overall_status = if total_errors > 0 {
        "fail"
    } else if total_warnings > 0 {
        "warn"
    } else {
        "pass"
    };

    if args.json {
        let mut payload = serde_json::json!({
            "status": overall_status,
            "total_viewports": args.viewports.len(),
            "passed_viewports": passed_viewports,
            "failed_viewports": failed_viewports,
            "total_errors": total_errors,
            "total_warnings": total_warnings,
            "viewports": vp_entries,
        });
        if !simulated_steps.is_empty() {
            if let Some(obj) = payload.as_object_mut() {
                obj.insert(
                    "simulated_steps".to_string(),
                    serde_json::Value::Array(simulated_steps.to_vec()),
                );
            }
        }
        let pretty = serde_json::to_string_pretty(&payload)
            .map_err(|e| CliError::Other(e.to_string()))?;
        println!("{pretty}");
    } else {
        println!(
            "Multi-viewport audit: {} ({}/{} passed, {} error(s), {} warning(s))",
            overall_status,
            passed_viewports,
            args.viewports.len(),
            total_errors,
            total_warnings
        );
    }

    if args.assert_audit && total_errors > 0 {
        return Err(CliError::Other(format!(
            "Multi-viewport visual audit assertion failed: {failed_viewports}/{} viewport(s) failed with {total_errors} total error(s)",
            args.viewports.len()
        )));
    }

    Ok(())
}

fn execute_flow(
    args: &RenderArgs,
    module: &Module,
    known_shaders: &[String],
    flow_path: &Path,
) -> Result<(), CliError> {
    let raw_flow = fs::read_to_string(flow_path).map_err(|e| {
        CliError::Other(format!(
            "Failed to read flow file '{}': {e}",
            flow_path.display()
        ))
    })?;
    let flow: RenderFlowSpec = serde_json::from_str(&raw_flow).map_err(|e| {
        CliError::Other(format!(
            "Failed to parse flow JSON '{}': {e}",
            flow_path.display()
        ))
    })?;

    let width = flow.width.unwrap_or(args.width).clamp(1, 8192);
    let height = flow.height.unwrap_or(args.height).clamp(1, 8192);
    let scale = flow.scale.unwrap_or(args.scale).clamp(0.25, 4.0);
    let mut elapsed = flow.time.unwrap_or(args.time).max(0.0);

    let mut mock = MockPlatform::new();
    let has_init = module.get_function("init").is_some();
    let view_fn = module.get_function("view").ok_or_else(|| {
        CliError::Other("Module must define `view` to execute `--flow`".to_string())
    })?;
    let view_arity = view_fn.params.len();

    let mut state_opt: Option<Value> = if has_init && view_arity >= 1 {
        let (s, _) =
            knr_interp::execute_module(module, "init", vec![], &mut mock, args.gas, false)
                .map_err(CliError::Vm)?;
        Some(s)
    } else if view_arity == 0 {
        None
    } else {
        return Err(CliError::Other(
            "Module defines `view(state)` without `init()`".to_string(),
        ));
    };

    let jit_shaders = compile_shaders_jit(module);
    let fast_shaders = FastShaderProgram::compile(module);
    let shader_provider = |shader_name: &str,
                           req_w: u32,
                           req_h: u32,
                           t: f32,
                           uniforms: &[f64]|
     -> Option<Pixmap> {
        if let Some(jit) = jit_shaders.get(shader_name) {
            return render_shader_jit(jit.eval_fn, req_w, req_h, t as f64, uniforms);
        }
        if let Some(pm) = fast_shaders.render(shader_name, req_w, req_h, t as f64, uniforms) {
            return Some(pm);
        }
        render_shader_vm(module, shader_name, req_w, req_h, t as f64, uniforms)
    };
    let renderer = UiRenderer::new();

    let eval_tree = |state_opt: &Option<Value>, mock: &mut MockPlatform| -> Result<UiNode, CliError> {
        let call_args = match state_opt {
            Some(s) => vec![s.clone()],
            None => vec![],
        };
        let (val, _) =
            knr_interp::execute_module(module, "view", call_args, mock, args.gas, false)
                .map_err(CliError::Vm)?;
        UiNode::from_value(&val).map_err(CliError::Runtime)
    };

    let mut step_results: Vec<FlowStepResult> = Vec::with_capacity(flow.steps.len());

    for (index, step) in flow.steps.iter().enumerate() {
        let result = match step {
            FlowStepSpec::Action { action } => match state_opt.clone() {
                Some(s) => match apply_render_step(
                    module,
                    s,
                    &RenderStep::Action(action.clone()),
                    &mut mock,
                    args.gas,
                ) {
                    Ok((next_s, _)) => {
                        state_opt = Some(next_s);
                        FlowStepResult {
                            index,
                            step_type: "action".to_string(),
                            passed: true,
                            detail: format!("Dispatched action '{action}'"),
                            error: None,
                            output_png: None,
                            pixel_hash: None,
                            audit_errors: None,
                            audit_warnings: None,
                        }
                    }
                    Err(e) => FlowStepResult {
                        index,
                        step_type: "action".to_string(),
                        passed: false,
                        detail: format!("Action '{action}' failed"),
                        error: Some(e.to_string()),
                        output_png: None,
                        pixel_hash: None,
                        audit_errors: None,
                        audit_warnings: None,
                    },
                },
                None => FlowStepResult {
                    index,
                    step_type: "action".to_string(),
                    passed: false,
                    detail: format!("Action '{action}' requires stateful MVU app"),
                    error: Some("Module has no init() state".to_string()),
                    output_png: None,
                    pixel_hash: None,
                    audit_errors: None,
                    audit_warnings: None,
                },
            },
            FlowStepSpec::Click { key } => match state_opt.clone() {
                Some(s) => match apply_render_step(
                    module,
                    s,
                    &RenderStep::Click(key.clone()),
                    &mut mock,
                    args.gas,
                ) {
                    Ok((next_s, meta)) => {
                        state_opt = Some(next_s);
                        let resolved = meta["resolved_action"].as_str().unwrap_or(key);
                        FlowStepResult {
                            index,
                            step_type: "click".to_string(),
                            passed: true,
                            detail: format!("Clicked '{key}' -> action '{resolved}'"),
                            error: None,
                            output_png: None,
                            pixel_hash: None,
                            audit_errors: None,
                            audit_warnings: None,
                        }
                    }
                    Err(e) => FlowStepResult {
                        index,
                        step_type: "click".to_string(),
                        passed: false,
                        detail: format!("Click '{key}' failed"),
                        error: Some(e.to_string()),
                        output_png: None,
                        pixel_hash: None,
                        audit_errors: None,
                        audit_warnings: None,
                    },
                },
                None => FlowStepResult {
                    index,
                    step_type: "click".to_string(),
                    passed: false,
                    detail: format!("Click '{key}' requires stateful MVU app"),
                    error: Some("Module has no init() state".to_string()),
                    output_png: None,
                    pixel_hash: None,
                    audit_errors: None,
                    audit_warnings: None,
                },
            },
            FlowStepSpec::Input { key, value } => match state_opt.clone() {
                Some(s) => match apply_render_step(
                    module,
                    s,
                    &RenderStep::Input {
                        key: key.clone(),
                        value: value.clone(),
                    },
                    &mut mock,
                    args.gas,
                ) {
                    Ok((next_s, _)) => {
                        state_opt = Some(next_s);
                        FlowStepResult {
                            index,
                            step_type: "input".to_string(),
                            passed: true,
                            detail: format!("Typed into '{key}' = \"{value}\""),
                            error: None,
                            output_png: None,
                            pixel_hash: None,
                            audit_errors: None,
                            audit_warnings: None,
                        }
                    }
                    Err(e) => FlowStepResult {
                        index,
                        step_type: "input".to_string(),
                        passed: false,
                        detail: format!("Input '{key}' failed"),
                        error: Some(e.to_string()),
                        output_png: None,
                        pixel_hash: None,
                        audit_errors: None,
                        audit_warnings: None,
                    },
                },
                None => FlowStepResult {
                    index,
                    step_type: "input".to_string(),
                    passed: false,
                    detail: format!("Input '{key}' requires stateful MVU app"),
                    error: Some("Module has no init() state".to_string()),
                    output_png: None,
                    pixel_hash: None,
                    audit_errors: None,
                    audit_warnings: None,
                },
            },
            FlowStepSpec::Slider { key, value } => match state_opt.clone() {
                Some(s) => match apply_render_step(
                    module,
                    s,
                    &RenderStep::Slider {
                        key: key.clone(),
                        value: value.clamp(0.0, 1.0),
                    },
                    &mut mock,
                    args.gas,
                ) {
                    Ok((next_s, _)) => {
                        state_opt = Some(next_s);
                        FlowStepResult {
                            index,
                            step_type: "slider".to_string(),
                            passed: true,
                            detail: format!("Set slider '{key}' = {:.3}", value.clamp(0.0, 1.0)),
                            error: None,
                            output_png: None,
                            pixel_hash: None,
                            audit_errors: None,
                            audit_warnings: None,
                        }
                    }
                    Err(e) => FlowStepResult {
                        index,
                        step_type: "slider".to_string(),
                        passed: false,
                        detail: format!("Slider '{key}' failed"),
                        error: Some(e.to_string()),
                        output_png: None,
                        pixel_hash: None,
                        audit_errors: None,
                        audit_warnings: None,
                    },
                },
                None => FlowStepResult {
                    index,
                    step_type: "slider".to_string(),
                    passed: false,
                    detail: format!("Slider '{key}' requires stateful MVU app"),
                    error: Some("Module has no init() state".to_string()),
                    output_png: None,
                    pixel_hash: None,
                    audit_errors: None,
                    audit_warnings: None,
                },
            },
            FlowStepSpec::Tick { dt } => {
                elapsed += (*dt as f32).max(0.0);
                if let Some(s) = state_opt.take() {
                    state_opt = Some(apply_tick(module, s, *dt, elapsed as f64, &mut mock, args.gas));
                }
                FlowStepResult {
                    index,
                    step_type: "tick".to_string(),
                    passed: true,
                    detail: format!("Advanced clock by {dt:.3}s (elapsed={elapsed:.3}s)"),
                    error: None,
                    output_png: None,
                    pixel_hash: None,
                    audit_errors: None,
                    audit_warnings: None,
                }
            }
            FlowStepSpec::AssertText {
                contains,
                not_contains,
            } => match eval_tree(&state_opt, &mut mock) {
                Ok(tree) => {
                    let all_text = collect_all_text(&tree);
                    let joined = all_text.join("\n");
                    let mut failures = Vec::new();
                    if let Some(ref needle) = contains {
                        if !joined.contains(needle.as_str()) {
                            failures.push(format!("Expected rendered UI text to contain \"{needle}\""));
                        }
                    }
                    if let Some(ref forbidden) = not_contains {
                        if joined.contains(forbidden.as_str()) {
                            failures.push(format!(
                                "Expected rendered UI text NOT to contain \"{forbidden}\""
                            ));
                        }
                    }
                    if contains.is_none() && not_contains.is_none() {
                        failures.push(
                            "assert_text step requires 'contains' or 'not_contains'".to_string(),
                        );
                    }
                    if failures.is_empty() {
                        FlowStepResult {
                            index,
                            step_type: "assert_text".to_string(),
                            passed: true,
                            detail: format!(
                                "Matched text assertion ({})",
                                contains
                                    .as_deref()
                                    .map(|c| format!("contains \"{c}\""))
                                    .or_else(|| not_contains
                                        .as_deref()
                                        .map(|n| format!("not_contains \"{n}\"")))
                                    .unwrap_or_default()
                            ),
                            error: None,
                            output_png: None,
                            pixel_hash: None,
                            audit_errors: None,
                            audit_warnings: None,
                        }
                    } else {
                        FlowStepResult {
                            index,
                            step_type: "assert_text".to_string(),
                            passed: false,
                            detail: "Text assertion failed".to_string(),
                            error: Some(failures.join("; ")),
                            output_png: None,
                            pixel_hash: None,
                            audit_errors: None,
                            audit_warnings: None,
                        }
                    }
                }
                Err(e) => FlowStepResult {
                    index,
                    step_type: "assert_text".to_string(),
                    passed: false,
                    detail: "Failed to evaluate view for assert_text".to_string(),
                    error: Some(e.to_string()),
                    output_png: None,
                    pixel_hash: None,
                    audit_errors: None,
                    audit_warnings: None,
                },
            },
            FlowStepSpec::AssertKey { key, absent } => match eval_tree(&state_opt, &mut mock) {
                Ok(tree) => {
                    let found = find_widget_by_key_or_action(&tree, key).is_some();
                    let expect_absent = absent.unwrap_or(false);
                    let passed = if expect_absent { !found } else { found };
                    FlowStepResult {
                        index,
                        step_type: "assert_key".to_string(),
                        passed,
                        detail: if expect_absent {
                            format!("Verified widget key '{key}' is absent")
                        } else {
                            format!("Verified widget key '{key}' is present")
                        },
                        error: if passed {
                            None
                        } else if expect_absent {
                            Some(format!("Widget key '{key}' was unexpectedly present"))
                        } else {
                            Some(format!("Widget key '{key}' was not found in rendered UI tree"))
                        },
                        output_png: None,
                        pixel_hash: None,
                        audit_errors: None,
                        audit_warnings: None,
                    }
                }
                Err(e) => FlowStepResult {
                    index,
                    step_type: "assert_key".to_string(),
                    passed: false,
                    detail: "Failed to evaluate view for assert_key".to_string(),
                    error: Some(e.to_string()),
                    output_png: None,
                    pixel_hash: None,
                    audit_errors: None,
                    audit_warnings: None,
                },
            },
            FlowStepSpec::AssertAudit {
                max_errors,
                max_warnings,
            } => match eval_tree(&state_opt, &mut mock) {
                Ok(tree) => {
                    let interaction = RenderInteractionState {
                        elapsed_sec: elapsed,
                        ..Default::default()
                    };
                    let (_pm, layout_root, _) = renderer.render_ui_with_shaders(
                        &tree,
                        width,
                        height,
                        scale,
                        &interaction,
                        Some(&shader_provider),
                    );
                    let report = audit_layout_with_images(
                        &layout_root,
                        width as f32,
                        height as f32,
                        known_shaders,
                        &|ch, font_family, font_weight| {
                            renderer.has_glyph(ch, font_family, font_weight)
                        },
                        &|src| renderer.can_load_image(src),
                    );
                    let allowed_err = max_errors.unwrap_or(0);
                    let mut failures = Vec::new();
                    if report.error_count > allowed_err {
                        failures.push(format!(
                            "Audit reported {} error(s), exceeding max_errors={allowed_err}",
                            report.error_count
                        ));
                    }
                    if let Some(allowed_warn) = max_warnings {
                        if report.warning_count > *allowed_warn {
                            failures.push(format!(
                                "Audit reported {} warning(s), exceeding max_warnings={allowed_warn}",
                                report.warning_count
                            ));
                        }
                    }
                    let passed = failures.is_empty();
                    FlowStepResult {
                        index,
                        step_type: "assert_audit".to_string(),
                        passed,
                        detail: format!(
                            "Audit {}: {} error(s), {} warning(s)",
                            report.status, report.error_count, report.warning_count
                        ),
                        error: if passed {
                            None
                        } else {
                            Some(failures.join("; "))
                        },
                        output_png: None,
                        pixel_hash: None,
                        audit_errors: Some(report.error_count),
                        audit_warnings: Some(report.warning_count),
                    }
                }
                Err(e) => FlowStepResult {
                    index,
                    step_type: "assert_audit".to_string(),
                    passed: false,
                    detail: "Failed to evaluate view for assert_audit".to_string(),
                    error: Some(e.to_string()),
                    output_png: None,
                    pixel_hash: None,
                    audit_errors: None,
                    audit_warnings: None,
                },
            },
            FlowStepSpec::Snapshot {
                output,
                width: snap_w,
                height: snap_h,
            } => match eval_tree(&state_opt, &mut mock) {
                Ok(tree) => {
                    let sw = snap_w.unwrap_or(width).clamp(1, 8192);
                    let sh = snap_h.unwrap_or(height).clamp(1, 8192);
                    let interaction = RenderInteractionState {
                        elapsed_sec: elapsed,
                        ..Default::default()
                    };
                    let (pm, _layout_root, _) = renderer.render_ui_with_shaders(
                        &tree,
                        sw,
                        sh,
                        scale,
                        &interaction,
                        Some(&shader_provider),
                    );
                    let hash = pixmap_fnv_hash(&pm);
                    match save_pixmap_png(&pm, output) {
                        Ok(()) => FlowStepResult {
                            index,
                            step_type: "snapshot".to_string(),
                            passed: true,
                            detail: format!("Saved snapshot {} ({sw}x{sh})", output.display()),
                            error: None,
                            output_png: Some(output.display().to_string()),
                            pixel_hash: Some(format!("0x{hash:016x}")),
                            audit_errors: None,
                            audit_warnings: None,
                        },
                        Err(e) => FlowStepResult {
                            index,
                            step_type: "snapshot".to_string(),
                            passed: false,
                            detail: format!("Failed to save snapshot {}", output.display()),
                            error: Some(e.to_string()),
                            output_png: None,
                            pixel_hash: None,
                            audit_errors: None,
                            audit_warnings: None,
                        },
                    }
                }
                Err(e) => FlowStepResult {
                    index,
                    step_type: "snapshot".to_string(),
                    passed: false,
                    detail: "Failed to evaluate view for snapshot".to_string(),
                    error: Some(e.to_string()),
                    output_png: None,
                    pixel_hash: None,
                    audit_errors: None,
                    audit_warnings: None,
                },
            },
        };
        step_results.push(result);
    }

    // Optional final output PNG if -o was also passed
    if let Some(ref out_path) = args.output {
        if let Ok(tree) = eval_tree(&state_opt, &mut mock) {
            let interaction = RenderInteractionState {
                elapsed_sec: elapsed,
                ..Default::default()
            };
            let (pm, _, _) = renderer.render_ui_with_shaders(
                &tree,
                width,
                height,
                scale,
                &interaction,
                Some(&shader_provider),
            );
            save_pixmap_png(&pm, out_path)?;
        }
    }

    let passed_steps = step_results.iter().filter(|r| r.passed).count();
    let failed_steps = step_results.len() - passed_steps;
    let status = if failed_steps == 0 { "pass" } else { "fail" };

    if args.json {
        let payload = serde_json::json!({
            "status": status,
            "flow_file": flow_path.display().to_string(),
            "width": width,
            "height": height,
            "scale": scale,
            "total_steps": step_results.len(),
            "passed_steps": passed_steps,
            "failed_steps": failed_steps,
            "steps": step_results,
        });
        let pretty = serde_json::to_string_pretty(&payload)
            .map_err(|e| CliError::Other(e.to_string()))?;
        println!("{pretty}");
    } else {
        println!(
            "Flow {} ({}x{} @ {:.1}x):",
            flow_path.display(),
            width,
            height,
            scale
        );
        for r in &step_results {
            let tag = if r.passed { "PASS" } else { "FAIL" };
            println!("  [{tag}] #{:02} {}: {}", r.index + 1, r.step_type, r.detail);
            if let Some(ref err) = r.error {
                println!("         error: {err}");
            }
        }
        println!(
            "Flow result: {} ({passed_steps}/{} passed, {failed_steps} failed)",
            status,
            step_results.len()
        );
    }

    if failed_steps > 0 {
        return Err(CliError::Other(format!(
            "Declarative flow failed: {failed_steps}/{} step(s) failed",
            step_results.len()
        )));
    }

    Ok(())
}

fn save_pixmap_png(pixmap: &Pixmap, png_path: &Path) -> Result<(), CliError> {
    if let Some(parent) = png_path.parent() {
        if !parent.as_os_str().is_empty() && !parent.exists() {
            fs::create_dir_all(parent).map_err(CliError::Io)?;
        }
    }
    pixmap.save_png(png_path).map_err(|e| {
        CliError::Other(format!("Failed to save PNG '{}': {e}", png_path.display()))
    })
}

fn viewport_output_path(base: &Path, vp_name: &str) -> PathBuf {
    let safe_name: String = vp_name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let stem = base
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("render");
    let ext = base
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("png");
    let file_name = format!("{stem}_{safe_name}.{ext}");
    match base.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.join(file_name),
        _ => PathBuf::from(file_name),
    }
}

fn print_audit_issues(report_json: &serde_json::Value) {
    if let Some(issues) = report_json.get("issues").and_then(|v| v.as_array()) {
        for issue in issues {
            let sev = issue
                .get("severity")
                .and_then(|v| v.as_str())
                .unwrap_or("info");
            let code = issue
                .get("code")
                .or_else(|| issue.get("kind"))
                .and_then(|v| v.as_str())
                .unwrap_or("Issue");
            let msg = issue
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let key = issue
                .get("node_key")
                .or_else(|| issue.get("key"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if key.is_empty() {
                println!("  [{sev}] {code}: {msg}");
            } else {
                println!("  [{sev}] {code} ({key}): {msg}");
            }
        }
    }
}

fn collect_all_text(node: &UiNode) -> Vec<String> {
    let mut out = Vec::new();
    if !node.text.is_empty() {
        out.push(node.text.clone());
    }
    for ch in &node.children {
        out.extend(collect_all_text(ch));
    }
    out
}

fn find_widget_by_key_or_action<'a>(node: &'a UiNode, target: &str) -> Option<&'a UiNode> {
    if node.key == target {
        return Some(node);
    }
    for ch in &node.children {
        if let Some(found) = find_widget_by_key_or_action(ch, target) {
            return Some(found);
        }
    }
    if !node.action.is_empty() && node.action == target {
        return Some(node);
    }
    None
}

fn compare_pixmaps_diff_pct(a: &Pixmap, b: &Pixmap) -> f64 {
    if a.width() != b.width() || a.height() != b.height() {
        return 100.0;
    }
    let da = a.data();
    let db = b.data();
    let total_pixels = (a.width() as usize) * (a.height() as usize);
    if total_pixels == 0 {
        return 0.0;
    }
    let diff_pixels = da
        .as_chunks::<4>()
        .0
        .iter()
        .zip(db.as_chunks::<4>().0.iter())
        .filter(|(pa, pb)| pa != pb)
        .count();
    (diff_pixels as f64 / total_pixels as f64) * 100.0
}

/// Generates a 3-panel side-by-side visual diff PNG: `[Baseline | Actual | Magenta Diff]`.
/// Differing pixels in the third panel are highlighted in bright magenta (`#ff00dc`),
/// while unchanged pixels are rendered dimmed for immediate spatial context.
fn generate_visual_diff_pixmap(baseline: &Pixmap, actual: &Pixmap) -> Option<Pixmap> {
    let panel_w = baseline.width().max(actual.width()).max(1);
    let panel_h = baseline.height().max(actual.height()).max(1);
    let sep = 2u32;
    let total_w = panel_w.checked_mul(3)?.checked_add(sep * 2)?;
    let mut out = Pixmap::new(total_w, panel_h)?;

    let out_data = out.data_mut();
    let base_data = baseline.data();
    let act_data = actual.data();
    let bw = baseline.width();
    let bh = baseline.height();
    let aw = actual.width();
    let ah = actual.height();
    let total_w_usize = total_w as usize;

    let read_px = |data: &[u8], w: u32, h: u32, x: u32, y: u32| -> Option<[u8; 4]> {
        if x < w && y < h {
            let idx = (y as usize * w as usize + x as usize) * 4;
            Some([data[idx], data[idx + 1], data[idx + 2], data[idx + 3]])
        } else {
            None
        }
    };

    for y in 0..panel_h {
        let row_base = y as usize * total_w_usize;
        // Draw crisp divider bars between panels
        for dx in 0..sep {
            for div_x in [panel_w + dx, panel_w * 2 + sep + dx] {
                let off = (row_base + div_x as usize) * 4;
                out_data[off] = 56;
                out_data[off + 1] = 189;
                out_data[off + 2] = 248;
                out_data[off + 3] = 255;
            }
        }

        for x in 0..panel_w {
            let bp = read_px(base_data, bw, bh, x, y);
            let ap = read_px(act_data, aw, ah, x, y);

            // Panel 0: Baseline
            let off0 = (row_base + x as usize) * 4;
            let p0 = bp.unwrap_or([12, 16, 28, 255]);
            out_data[off0..off0 + 4].copy_from_slice(&p0);

            // Panel 1: Actual
            let off1 = (row_base + (panel_w + sep + x) as usize) * 4;
            let p1 = ap.unwrap_or([12, 16, 28, 255]);
            out_data[off1..off1 + 4].copy_from_slice(&p1);

            // Panel 2: Diff highlight (bright magenta on changed/missing pixels, dimmed actual on identical)
            let off2 = (row_base + (panel_w * 2 + sep * 2 + x) as usize) * 4;
            let p2 = match (bp, ap) {
                (Some(bpx), Some(apx)) if bpx == apx => [
                    ((apx[0] as u16 * 30) / 100) as u8,
                    ((apx[1] as u16 * 30) / 100) as u8,
                    ((apx[2] as u16 * 30) / 100) as u8,
                    255,
                ],
                _ => [255, 0, 220, 255],
            };
            out_data[off2..off2 + 4].copy_from_slice(&p2);
        }
    }

    Some(out)
}

// ============================================================================
// Real @shader Preview Evaluator: Cranelift JIT (Primary) + VM Fallback
// ============================================================================

fn compile_shaders_jit(module: &Module) -> HashMap<String, CompiledJitShader> {
    let mut compiled = HashMap::new();
    for func in &module.functions {
        if !func.is_shader {
            continue;
        }
        let wrapper_name = format!("__knr_render_shader_{}", func.name);
        let Some(wrapper_fn) = build_shader_jit_wrapper(module, func, &wrapper_name) else {
            continue;
        };

        // Build a minimal sub-module containing only the shader, its pure helpers, and schemas
        let mut sub_mod = Module::new();
        sub_mod.schemas = module.schemas.clone();
        let mut visited = HashSet::new();
        collect_transitive_fns(module, &func.name, &mut visited, &mut sub_mod);
        sub_mod.add_function(wrapper_fn);

        let mut backend = match CraneliftBackend::new() {
            Ok(b) => b,
            Err(e) => {
                if std::env::var_os("KNR_DEBUG_JIT").is_some() {
                    eprintln!("[JIT] backend init failed for {}: {e}", func.name);
                }
                continue;
            }
        };
        match backend.compile_jit_module(&sub_mod, &wrapper_name) {
            Ok(code_ptr) => {
                let eval_fn: JitShaderEvalFn =
                    unsafe { std::mem::transmute::<*const u8, JitShaderEvalFn>(code_ptr) };
                compiled.insert(
                    func.name.clone(),
                    CompiledJitShader {
                        _backend: backend,
                        eval_fn,
                    },
                );
            }
            Err(e) => {
                if std::env::var_os("KNR_DEBUG_JIT").is_some() {
                    eprintln!("[JIT] compile failed for {}: {e}", func.name);
                }
            }
        }
    }
    compiled
}

fn collect_transitive_fns(
    module: &Module,
    name: &str,
    visited: &mut HashSet<String>,
    out: &mut Module,
) {
    if !visited.insert(name.to_string()) {
        return;
    }
    let Some(func) = module.get_function(name) else {
        return;
    };
    for blk in &func.blocks {
        for inst in &blk.instructions {
            if let Op::Call(ref callee, _) = inst.op {
                collect_transitive_fns(module, callee, visited, out);
            }
        }
    }
    out.add_function(func.clone());
}

/// Synthesizes a fixed-ABI SSA wrapper:
/// `fn wrapper(%0: f64 u, %1: f64 v, %2: f64 time, %3: f64 mx, %4: f64 my, %5: vec[f64] uniforms) -> i64`
/// returning packed RGBA8 `(r | g<<8 | b<<16 | a<<24)` and freeing every temporary struct (zero leaks!).
fn build_shader_jit_wrapper(
    module: &Module,
    sh: &Function,
    wrapper_name: &str,
) -> Option<Function> {
    let mut f = Function::new(wrapper_name, Type::I64);
    f.add_param(0, Type::F64); // u
    f.add_param(1, Type::F64); // v
    f.add_param(2, Type::F64); // time
    f.add_param(3, Type::F64); // mouse.x
    f.add_param(4, Type::F64); // mouse.y
    f.add_param(5, Type::Vec(Box::new(Type::F64))); // uniforms

    let mut bb = BasicBlock::new(0, Terminator::Ret(None));
    let mut next_reg: RegId = 6;
    let mut alloc = |bb: &mut BasicBlock, ty: Type, op: Op| -> RegId {
        let r = next_reg;
        next_reg += 1;
        bb.push(Instruction::new(Some(r), ty, op, 1));
        r
    };

    let c_zero = alloc(&mut bb, Type::F64, Op::ConstF64(0.0));
    let c_half = alloc(&mut bb, Type::F64, Op::ConstF64(0.5));
    let c_one = alloc(&mut bb, Type::F64, Op::ConstF64(1.0));
    let c_255 = alloc(&mut bb, Type::F64, Op::ConstF64(255.0));

    let first_is_f64 = matches!(sh.params.first().map(|(_, t)| t), Some(Type::F64));
    let mut call_args = Vec::with_capacity(sh.params.len());
    let mut to_free = Vec::new();
    let mut custom_idx: i64 = 0;

    for (idx, (_, ty)) in sh.params.iter().enumerate() {
        let is_borrowed = sh.param_borrows.get(idx).copied().unwrap_or(false);
        match (idx, ty) {
            (0, Type::Struct(sname))
                if module
                    .get_schema(sname)
                    .is_some_and(|sc| sc.fields.len() == 2) =>
            {
                let sty = Type::Struct(sname.clone());
                let s0 = alloc(&mut bb, sty.clone(), Op::StructNew(sname.clone()));
                let s1 = alloc(&mut bb, sty.clone(), Op::StructSet(s0, 0, 0));
                let s2 = alloc(&mut bb, sty, Op::StructSet(s1, 1, 1));
                call_args.push(s2);
                if is_borrowed {
                    to_free.push(s2);
                }
            }
            (0, Type::F64) => call_args.push(0),
            (1, Type::F64) => {
                if first_is_f64 {
                    call_args.push(1);
                } else {
                    call_args.push(2);
                }
            }
            (2, Type::F64) if first_is_f64 => call_args.push(2),
            (_, Type::Struct(sname))
                if module
                    .get_schema(sname)
                    .is_some_and(|sc| sc.fields.len() == 2) =>
            {
                let sty = Type::Struct(sname.clone());
                let s0 = alloc(&mut bb, sty.clone(), Op::StructNew(sname.clone()));
                let s1 = alloc(&mut bb, sty.clone(), Op::StructSet(s0, 0, 3));
                let s2 = alloc(&mut bb, sty, Op::StructSet(s1, 1, 4));
                call_args.push(s2);
                if is_borrowed {
                    to_free.push(s2);
                }
            }
            (_, Type::F64) => {
                let ir = alloc(&mut bb, Type::I64, Op::ConstI64(custom_idx));
                let vr = alloc(&mut bb, Type::F64, Op::VecGet(5, ir));
                call_args.push(vr);
                custom_idx += 1;
            }
            (_, Type::I64) => {
                let ir = alloc(&mut bb, Type::I64, Op::ConstI64(custom_idx));
                let vf = alloc(&mut bb, Type::F64, Op::VecGet(5, ir));
                let vi = alloc(&mut bb, Type::I64, Op::FloatToInt(vf));
                call_args.push(vi);
                custom_idx += 1;
            }
            (_, Type::Bool) => {
                let ir = alloc(&mut bb, Type::I64, Op::ConstI64(custom_idx));
                let vf = alloc(&mut bb, Type::F64, Op::VecGet(5, ir));
                let vb = alloc(&mut bb, Type::Bool, Op::CmpLt(c_half, vf));
                call_args.push(vb);
                custom_idx += 1;
            }
            _ => return None,
        }
    }

    let ret_r = alloc(
        &mut bb,
        sh.ret_ty.clone(),
        Op::Call(sh.name.clone(), call_args),
    );

    for fr in to_free {
        bb.push(Instruction::new(None, Type::Void, Op::Free(fr), 1));
    }

    let (r8, g8, b8, a8) = match &sh.ret_ty {
        Type::Struct(sname) => {
            let sc = module.get_schema(sname)?;
            if sc.fields.len() < 3 {
                return None;
            }
            let rf = alloc(&mut bb, Type::F64, Op::StructGet(ret_r, 0));
            let gf = alloc(&mut bb, Type::F64, Op::StructGet(ret_r, 1));
            let bf = alloc(&mut bb, Type::F64, Op::StructGet(ret_r, 2));
            let af = if sc.fields.len() >= 4 {
                alloc(&mut bb, Type::F64, Op::StructGet(ret_r, 3))
            } else {
                c_one
            };
            bb.push(Instruction::new(None, Type::Void, Op::Free(ret_r), 1));
            let mut chan_to_u8 = |bb: &mut BasicBlock, v_reg: RegId| -> RegId {
                let lo = alloc(bb, Type::F64, Op::Max(v_reg, c_zero));
                let hi = alloc(bb, Type::F64, Op::Min(lo, c_one));
                let sc = alloc(bb, Type::F64, Op::Mul(hi, c_255));
                let rd = alloc(bb, Type::F64, Op::Add(sc, c_half));
                alloc(bb, Type::I64, Op::FloatToInt(rd))
            };
            let r_i = chan_to_u8(&mut bb, rf);
            let g_i = chan_to_u8(&mut bb, gf);
            let b_i = chan_to_u8(&mut bb, bf);
            let a_i = chan_to_u8(&mut bb, af);
            (r_i, g_i, b_i, a_i)
        }
        Type::F64 => {
            let lo = alloc(&mut bb, Type::F64, Op::Max(ret_r, c_zero));
            let hi = alloc(&mut bb, Type::F64, Op::Min(lo, c_one));
            let sc = alloc(&mut bb, Type::F64, Op::Mul(hi, c_255));
            let rd = alloc(&mut bb, Type::F64, Op::Add(sc, c_half));
            let lum = alloc(&mut bb, Type::I64, Op::FloatToInt(rd));
            let full_a = alloc(&mut bb, Type::I64, Op::ConstI64(255));
            (lum, lum, lum, full_a)
        }
        _ => return None,
    };

    let c256 = alloc(&mut bb, Type::I64, Op::ConstI64(256));
    let c65536 = alloc(&mut bb, Type::I64, Op::ConstI64(65_536));
    let c16m = alloc(&mut bb, Type::I64, Op::ConstI64(16_777_216));
    let g_sh = alloc(&mut bb, Type::I64, Op::Mul(g8, c256));
    let b_sh = alloc(&mut bb, Type::I64, Op::Mul(b8, c65536));
    let a_sh = alloc(&mut bb, Type::I64, Op::Mul(a8, c16m));
    let p0 = alloc(&mut bb, Type::I64, Op::Add(r8, g_sh));
    let p1 = alloc(&mut bb, Type::I64, Op::Add(p0, b_sh));
    let packed = alloc(&mut bb, Type::I64, Op::Add(p1, a_sh));

    bb.terminator = Terminator::Ret(Some(packed));
    f.add_block(bb);
    Some(f)
}

fn preview_grid_size(req_w: u32, req_h: u32, max_pixels: f32, max_w: f32, max_h: f32) -> (u32, u32) {
    let w = req_w.max(1);
    let h = req_h.max(1);
    let aspect = (w as f32) / (h as f32);
    let gw = (max_pixels * aspect).sqrt().round().clamp(24.0, max_w) as u32;
    let gh = (gw as f32 / aspect).round().clamp(16.0, max_h) as u32;
    (gw.min(w).max(1), gh.min(h).max(1))
}

fn render_shader_jit(
    eval_fn: JitShaderEvalFn,
    req_w: u32,
    req_h: u32,
    time: f64,
    uniforms: &[f64],
) -> Option<Pixmap> {
    let (gw, gh) = preview_grid_size(req_w, req_h, 96_000.0, 480.0, 280.0);
    let mut small = Pixmap::new(gw, gh)?;

    let u_vec = knr_runtime::rt_vec_new();
    for idx in 0..16 {
        let val = uniforms.get(idx).copied().unwrap_or(0.0);
        let bits_ptr = (val.to_bits() as usize) as *mut c_void;
        let _ = knr_runtime::rt_vec_push(u_vec, bits_ptr);
    }
    let u_vec_i64 = u_vec as usize as i64;

    let inv_w = 1.0 / gw as f64;
    let inv_h = 1.0 / gh as f64;
    let data = small.data_mut();

    for py in 0..gh {
        let v = (py as f64 + 0.5) * inv_h;
        let row_off = (py as usize) * (gw as usize);
        for px in 0..gw {
            let u = (px as f64 + 0.5) * inv_w;
            let packed = eval_fn(u, v, time, 0.5, 0.5, u_vec_i64) as u64;
            let r8 = (packed & 0xff) as u8;
            let g8 = ((packed >> 8) & 0xff) as u8;
            let b8 = ((packed >> 16) & 0xff) as u8;
            let a8 = ((packed >> 24) & 0xff) as u8;
            let af = a8 as f32 / 255.0;
            let off = (row_off + px as usize) * 4;
            data[off] = (r8 as f32 * af).round() as u8;
            data[off + 1] = (g8 as f32 * af).round() as u8;
            data[off + 2] = (b8 as f32 * af).round() as u8;
            data[off + 3] = a8;
        }
    }

    knr_runtime::rt_vec_free(u_vec);
    Some(small)
}

// ============================================================================
// Portable Zero-Alloc Multithreaded SSA Shader Evaluator (W^X / Sandbox Safe)
// ============================================================================

#[derive(Clone)]
enum FastOp {
    ConstBits(u64),
    AddF64(usize, usize),
    AddI64(usize, usize),
    SubF64(usize, usize),
    SubI64(usize, usize),
    MulF64(usize, usize),
    MulI64(usize, usize),
    DivF64(usize, usize),
    DivI64(usize, usize),
    RemI64(usize, usize),
    CmpEqF64(usize, usize),
    CmpEqI64(usize, usize),
    CmpLtF64(usize, usize),
    CmpLtI64(usize, usize),
    Sin(usize),
    Cos(usize),
    Tan(usize),
    Sqrt(usize),
    Abs(usize),
    Floor(usize),
    Ceil(usize),
    Pow(usize, usize),
    Atan2(usize, usize),
    Min(usize, usize),
    Max(usize, usize),
    IntToFloat(usize),
    FloatToInt(usize),
    StructNew(usize),
    StructGet(usize, usize),
    StructSet(usize, usize, usize),
    Call(usize, Vec<usize>),
    Nop,
}

#[derive(Clone)]
struct FastInst {
    dst: Option<usize>,
    op: FastOp,
}

#[derive(Clone)]
enum FastTerm {
    Ret(Option<usize>),
    Br(usize, Vec<usize>),
    BrIf {
        cond: usize,
        then_blk: usize,
        then_args: Vec<usize>,
        else_blk: usize,
        else_args: Vec<usize>,
    },
}

#[derive(Clone)]
struct FastBlock {
    params: Vec<usize>,
    insts: Vec<FastInst>,
    term: FastTerm,
}

#[derive(Clone)]
enum FastParamBinding {
    UvStruct(usize),
    MouseStruct(usize),
    U,
    V,
    Time,
    UniformF64(usize),
    UniformI64(usize),
    UniformBool(usize),
}

#[derive(Clone)]
struct FastFn {
    reg_count: usize,
    params: Vec<usize>,
    blocks: Vec<FastBlock>,
}

#[derive(Clone)]
struct FastShaderEntry {
    fn_idx: usize,
    bindings: Vec<FastParamBinding>,
    ret_is_f64: bool,
    ret_fields: usize,
}

struct FastShaderProgram {
    fns: Vec<FastFn>,
    shaders: HashMap<String, FastShaderEntry>,
}

impl FastShaderProgram {
    fn compile(module: &Module) -> Self {
        let mut fn_indices: HashMap<&str, usize> = HashMap::new();
        let mut needed_names: Vec<String> = Vec::new();
        let mut visited: HashSet<String> = HashSet::new();

        for func in &module.functions {
            if func.is_shader {
                collect_fast_names(module, &func.name, &mut visited, &mut needed_names);
            }
        }
        for (idx, name) in needed_names.iter().enumerate() {
            fn_indices.insert(name.as_str(), idx);
        }

        let schema_fields: HashMap<&str, usize> = module
            .schemas
            .iter()
            .map(|sc| {
                let count = sc
                    .fields
                    .iter()
                    .map(|f| f.index as usize + 1)
                    .max()
                    .unwrap_or(sc.fields.len())
                    .max(1);
                (sc.name.as_str(), count)
            })
            .collect();

        let mut compiled_fns: Vec<Option<FastFn>> = Vec::with_capacity(needed_names.len());
        for name in &needed_names {
            let compiled = module
                .get_function(name)
                .and_then(|f| compile_fast_fn(f, &fn_indices, &schema_fields));
            compiled_fns.push(compiled);
        }

        if compiled_fns.iter().any(|f| f.is_none()) {
            return Self {
                fns: Vec::new(),
                shaders: HashMap::new(),
            };
        }
        let fns: Vec<FastFn> = compiled_fns.into_iter().flatten().collect();

        let mut shaders = HashMap::new();
        for func in &module.functions {
            if !func.is_shader {
                continue;
            }
            let Some(&fn_idx) = fn_indices.get(func.name.as_str()) else {
                continue;
            };
            let Some(bindings) = build_fast_bindings(module, func) else {
                continue;
            };
            let (ret_is_f64, ret_fields) = match &func.ret_ty {
                Type::F64 => (true, 0),
                Type::Struct(sname) => {
                    let count = schema_fields.get(sname.as_str()).copied().unwrap_or(0);
                    if count < 3 {
                        continue;
                    }
                    (false, count)
                }
                _ => continue,
            };
            shaders.insert(
                func.name.clone(),
                FastShaderEntry {
                    fn_idx,
                    bindings,
                    ret_is_f64,
                    ret_fields,
                },
            );
        }

        Self { fns, shaders }
    }

    fn render(
        &self,
        shader_name: &str,
        req_w: u32,
        req_h: u32,
        time: f64,
        uniforms: &[f64],
    ) -> Option<Pixmap> {
        let entry = self.shaders.get(shader_name)?;
        // Crisp 320..384px wide preview grid, evaluated in parallel across CPU cores
        let (gw, gh) = preview_grid_size(req_w, req_h, 54_000.0, 384.0, 220.0);
        let mut small = Pixmap::new(gw, gh)?;
        let inv_w = 1.0 / gw as f64;
        let inv_h = 1.0 / gh as f64;

        let threads = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4)
            .min(gh as usize)
            .max(1);
        let rows_per_chunk = (gh as usize).div_ceil(threads);
        let stride = gw as usize * 4;
        let data = small.data_mut();

        std::thread::scope(|scope| {
            for (chunk_idx, chunk) in data.chunks_mut(rows_per_chunk * stride).enumerate() {
                let start_y = chunk_idx * rows_per_chunk;
                let fns = &self.fns;
                scope.spawn(move || {
                    let mut regs_stack: Vec<u64> = Vec::with_capacity(512);
                    let mut arena: Vec<u64> = Vec::with_capacity(256);
                    let mut tmp_args: Vec<u64> = Vec::with_capacity(16);
                    let mut call_args: Vec<u64> = Vec::with_capacity(16);

                    for (local_y, row) in chunk.chunks_exact_mut(stride).enumerate() {
                        let py = start_y + local_y;
                        let v = (py as f64 + 0.5) * inv_h;
                        for px in 0..gw as usize {
                            let u = (px as f64 + 0.5) * inv_w;
                            arena.clear();
                            call_args.clear();
                            for b in &entry.bindings {
                                match *b {
                                    FastParamBinding::UvStruct(fields) => {
                                        let base = arena.len();
                                        arena.resize(base + fields, 0);
                                        arena[base] = u.to_bits();
                                        if fields > 1 {
                                            arena[base + 1] = v.to_bits();
                                        }
                                        call_args.push(base as u64);
                                    }
                                    FastParamBinding::MouseStruct(fields) => {
                                        let base = arena.len();
                                        arena.resize(base + fields, 0);
                                        arena[base] = 0.5f64.to_bits();
                                        if fields > 1 {
                                            arena[base + 1] = 0.5f64.to_bits();
                                        }
                                        call_args.push(base as u64);
                                    }
                                    FastParamBinding::U => call_args.push(u.to_bits()),
                                    FastParamBinding::V => call_args.push(v.to_bits()),
                                    FastParamBinding::Time => call_args.push(time.to_bits()),
                                    FastParamBinding::UniformF64(idx) => {
                                        call_args.push(
                                            uniforms.get(idx).copied().unwrap_or(0.0).to_bits(),
                                        );
                                    }
                                    FastParamBinding::UniformI64(idx) => {
                                        call_args.push(
                                            uniforms.get(idx).copied().unwrap_or(0.0) as i64 as u64,
                                        );
                                    }
                                    FastParamBinding::UniformBool(idx) => {
                                        let flag =
                                            uniforms.get(idx).copied().unwrap_or(0.0) > 0.5;
                                        call_args.push(u64::from(flag));
                                    }
                                }
                            }

                            let ret = eval_fast_fn(
                                fns,
                                entry.fn_idx,
                                &call_args,
                                &mut regs_stack,
                                &mut arena,
                                &mut tmp_args,
                            );

                            let (rf, gf, bf, af) = if entry.ret_is_f64 {
                                let l = f64::from_bits(ret).clamp(0.0, 1.0);
                                (l, l, l, 1.0)
                            } else {
                                let base = ret as usize;
                                if base + 3 <= arena.len() {
                                    let r = f64::from_bits(arena[base]).clamp(0.0, 1.0);
                                    let g = f64::from_bits(arena[base + 1]).clamp(0.0, 1.0);
                                    let b = f64::from_bits(arena[base + 2]).clamp(0.0, 1.0);
                                    let a = if entry.ret_fields >= 4 && base + 3 < arena.len() {
                                        f64::from_bits(arena[base + 3]).clamp(0.0, 1.0)
                                    } else {
                                        1.0
                                    };
                                    (r, g, b, a)
                                } else {
                                    (0.04, 0.07, 0.13, 1.0)
                                }
                            };

                            let off = px * 4;
                            row[off] = (rf * af * 255.0).round() as u8;
                            row[off + 1] = (gf * af * 255.0).round() as u8;
                            row[off + 2] = (bf * af * 255.0).round() as u8;
                            row[off + 3] = (af * 255.0).round() as u8;
                        }
                    }
                });
            }
        });

        Some(small)
    }
}

fn collect_fast_names(
    module: &Module,
    name: &str,
    visited: &mut HashSet<String>,
    out: &mut Vec<String>,
) {
    if !visited.insert(name.to_string()) {
        return;
    }
    let Some(func) = module.get_function(name) else {
        return;
    };
    for blk in &func.blocks {
        for inst in &blk.instructions {
            if let Op::Call(ref callee, _) = inst.op {
                collect_fast_names(module, callee, visited, out);
            }
        }
    }
    out.push(name.to_string());
}

fn build_fast_bindings(module: &Module, sh: &Function) -> Option<Vec<FastParamBinding>> {
    let first_is_f64 = matches!(sh.params.first().map(|(_, t)| t), Some(Type::F64));
    let mut bindings = Vec::with_capacity(sh.params.len());
    let mut custom_idx = 0usize;

    for (idx, (_, ty)) in sh.params.iter().enumerate() {
        match (idx, ty) {
            (0, Type::Struct(sname))
                if module
                    .get_schema(sname)
                    .is_some_and(|sc| sc.fields.len() == 2) =>
            {
                bindings.push(FastParamBinding::UvStruct(2));
            }
            (0, Type::F64) => bindings.push(FastParamBinding::U),
            (1, Type::F64) => bindings.push(if first_is_f64 {
                FastParamBinding::V
            } else {
                FastParamBinding::Time
            }),
            (2, Type::F64) if first_is_f64 => bindings.push(FastParamBinding::Time),
            (_, Type::Struct(sname))
                if module
                    .get_schema(sname)
                    .is_some_and(|sc| sc.fields.len() == 2) =>
            {
                bindings.push(FastParamBinding::MouseStruct(2));
            }
            (_, Type::F64) => {
                bindings.push(FastParamBinding::UniformF64(custom_idx));
                custom_idx += 1;
            }
            (_, Type::I64) => {
                bindings.push(FastParamBinding::UniformI64(custom_idx));
                custom_idx += 1;
            }
            (_, Type::Bool) => {
                bindings.push(FastParamBinding::UniformBool(custom_idx));
                custom_idx += 1;
            }
            _ => return None,
        }
    }
    Some(bindings)
}

fn compile_fast_fn(
    func: &Function,
    fn_indices: &HashMap<&str, usize>,
    schema_fields: &HashMap<&str, usize>,
) -> Option<FastFn> {
    let mut reg_map: HashMap<RegId, usize> = HashMap::new();
    let mut reg_is_f64: HashMap<RegId, bool> = HashMap::new();
    let mut reg_count = 0usize;

    let mut map_reg = |r: RegId, ty: Option<&Type>| -> usize {
        if let Some(t) = ty {
            reg_is_f64.insert(r, *t == Type::F64);
        }
        *reg_map.entry(r).or_insert_with(|| {
            let slot = reg_count;
            reg_count += 1;
            slot
        })
    };

    let mut params = Vec::with_capacity(func.params.len());
    for (r, ty) in &func.params {
        params.push(map_reg(*r, Some(ty)));
    }
    let mut blk_map: HashMap<knr_core::BlockId, usize> = HashMap::new();
    for (idx, blk) in func.blocks.iter().enumerate() {
        blk_map.insert(blk.id, idx);
        for (r, ty) in &blk.params {
            map_reg(*r, Some(ty));
        }
        for inst in &blk.instructions {
            if let Some(d) = inst.dst {
                map_reg(d, Some(&inst.ty));
            }
        }
    }

    let mut blocks = Vec::with_capacity(func.blocks.len());
    for blk in &func.blocks {
        let b_params: Vec<usize> = blk.params.iter().map(|(r, _)| reg_map[r]).collect();
        let mut insts = Vec::with_capacity(blk.instructions.len());
        for inst in &blk.instructions {
            let dst = inst.dst.map(|d| reg_map[&d]);
            let is_f64 = inst.ty == Type::F64;
            let op = match &inst.op {
                Op::ConstI64(n) => FastOp::ConstBits(*n as u64),
                Op::ConstF64(x) => FastOp::ConstBits(x.to_bits()),
                Op::ConstBool(b) => FastOp::ConstBits(u64::from(*b)),
                Op::Add(a, b) => {
                    if is_f64 {
                        FastOp::AddF64(*reg_map.get(a)?, *reg_map.get(b)?)
                    } else {
                        FastOp::AddI64(*reg_map.get(a)?, *reg_map.get(b)?)
                    }
                }
                Op::Sub(a, b) => {
                    if is_f64 {
                        FastOp::SubF64(*reg_map.get(a)?, *reg_map.get(b)?)
                    } else {
                        FastOp::SubI64(*reg_map.get(a)?, *reg_map.get(b)?)
                    }
                }
                Op::Mul(a, b) => {
                    if is_f64 {
                        FastOp::MulF64(*reg_map.get(a)?, *reg_map.get(b)?)
                    } else {
                        FastOp::MulI64(*reg_map.get(a)?, *reg_map.get(b)?)
                    }
                }
                Op::Div(a, b) => {
                    if is_f64 {
                        FastOp::DivF64(*reg_map.get(a)?, *reg_map.get(b)?)
                    } else {
                        FastOp::DivI64(*reg_map.get(a)?, *reg_map.get(b)?)
                    }
                }
                Op::Rem(a, b) => FastOp::RemI64(*reg_map.get(a)?, *reg_map.get(b)?),
                Op::CmpEq(a, b) => {
                    if reg_is_f64.get(a).copied().unwrap_or(false) {
                        FastOp::CmpEqF64(*reg_map.get(a)?, *reg_map.get(b)?)
                    } else {
                        FastOp::CmpEqI64(*reg_map.get(a)?, *reg_map.get(b)?)
                    }
                }
                Op::CmpLt(a, b) => {
                    if reg_is_f64.get(a).copied().unwrap_or(false) {
                        FastOp::CmpLtF64(*reg_map.get(a)?, *reg_map.get(b)?)
                    } else {
                        FastOp::CmpLtI64(*reg_map.get(a)?, *reg_map.get(b)?)
                    }
                }
                Op::Sin(a) => FastOp::Sin(*reg_map.get(a)?),
                Op::Cos(a) => FastOp::Cos(*reg_map.get(a)?),
                Op::Tan(a) => FastOp::Tan(*reg_map.get(a)?),
                Op::Sqrt(a) => FastOp::Sqrt(*reg_map.get(a)?),
                Op::Abs(a) => FastOp::Abs(*reg_map.get(a)?),
                Op::Floor(a) => FastOp::Floor(*reg_map.get(a)?),
                Op::Ceil(a) => FastOp::Ceil(*reg_map.get(a)?),
                Op::Pow(a, b) => FastOp::Pow(*reg_map.get(a)?, *reg_map.get(b)?),
                Op::Atan2(a, b) => FastOp::Atan2(*reg_map.get(a)?, *reg_map.get(b)?),
                Op::Min(a, b) => FastOp::Min(*reg_map.get(a)?, *reg_map.get(b)?),
                Op::Max(a, b) => FastOp::Max(*reg_map.get(a)?, *reg_map.get(b)?),
                Op::IntToFloat(a) => FastOp::IntToFloat(*reg_map.get(a)?),
                Op::FloatToInt(a) => FastOp::FloatToInt(*reg_map.get(a)?),
                Op::StructNew(name) => FastOp::StructNew(*schema_fields.get(name.as_str())?),
                Op::StructGet(s, fi) => FastOp::StructGet(*reg_map.get(s)?, *fi as usize),
                Op::StructSet(s, fi, v) => {
                    FastOp::StructSet(*reg_map.get(s)?, *fi as usize, *reg_map.get(v)?)
                }
                Op::Free(_) => FastOp::Nop,
                Op::Call(callee, args) => {
                    let cid = *fn_indices.get(callee.as_str())?;
                    let mut mapped_args = Vec::with_capacity(args.len());
                    for a in args {
                        mapped_args.push(*reg_map.get(a)?);
                    }
                    FastOp::Call(cid, mapped_args)
                }
                _ => return None,
            };
            insts.push(FastInst { dst, op });
        }

        let term = match &blk.terminator {
            Terminator::Ret(r) => FastTerm::Ret(match r {
                Some(reg) => Some(*reg_map.get(reg)?),
                None => None,
            }),
            Terminator::Br(t) => FastTerm::Br(*blk_map.get(t)?, Vec::new()),
            Terminator::BrArgs { target, args } => {
                let mut ma = Vec::with_capacity(args.len());
                for a in args {
                    ma.push(*reg_map.get(a)?);
                }
                FastTerm::Br(*blk_map.get(target)?, ma)
            }
            Terminator::BrIf {
                cond,
                then_blk,
                else_blk,
            } => FastTerm::BrIf {
                cond: *reg_map.get(cond)?,
                then_blk: *blk_map.get(then_blk)?,
                then_args: Vec::new(),
                else_blk: *blk_map.get(else_blk)?,
                else_args: Vec::new(),
            },
            Terminator::BrIfArgs {
                cond,
                then_blk,
                then_args,
                else_blk,
                else_args,
            } => {
                let mut ta = Vec::with_capacity(then_args.len());
                for a in then_args {
                    ta.push(*reg_map.get(a)?);
                }
                let mut ea = Vec::with_capacity(else_args.len());
                for a in else_args {
                    ea.push(*reg_map.get(a)?);
                }
                FastTerm::BrIf {
                    cond: *reg_map.get(cond)?,
                    then_blk: *blk_map.get(then_blk)?,
                    then_args: ta,
                    else_blk: *blk_map.get(else_blk)?,
                    else_args: ea,
                }
            }
        };

        blocks.push(FastBlock {
            params: b_params,
            insts,
            term,
        });
    }

    Some(FastFn {
        reg_count,
        params,
        blocks,
    })
}

fn eval_fast_fn(
    fns: &[FastFn],
    fn_idx: usize,
    args: &[u64],
    regs_stack: &mut Vec<u64>,
    arena: &mut Vec<u64>,
    tmp_args: &mut Vec<u64>,
) -> u64 {
    let f = &fns[fn_idx];
    let base = regs_stack.len();
    regs_stack.resize(base + f.reg_count, 0);
    for (i, &p_slot) in f.params.iter().enumerate() {
        if let Some(&val) = args.get(i) {
            regs_stack[base + p_slot] = val;
        }
    }

    let mut cur_blk = 0usize;
    let mut fuel = 250_000usize;
    let ret_val = loop {
        if fuel == 0 {
            break 0;
        }
        fuel -= 1;
        let blk = &f.blocks[cur_blk];
        for inst in &blk.insts {
            let val = match &inst.op {
                FastOp::ConstBits(b) => *b,
                FastOp::AddF64(a, b) => {
                    (f64::from_bits(regs_stack[base + a]) + f64::from_bits(regs_stack[base + b]))
                        .to_bits()
                }
                FastOp::AddI64(a, b) => {
                    (regs_stack[base + a] as i64).wrapping_add(regs_stack[base + b] as i64) as u64
                }
                FastOp::SubF64(a, b) => {
                    (f64::from_bits(regs_stack[base + a]) - f64::from_bits(regs_stack[base + b]))
                        .to_bits()
                }
                FastOp::SubI64(a, b) => {
                    (regs_stack[base + a] as i64).wrapping_sub(regs_stack[base + b] as i64) as u64
                }
                FastOp::MulF64(a, b) => {
                    (f64::from_bits(regs_stack[base + a]) * f64::from_bits(regs_stack[base + b]))
                        .to_bits()
                }
                FastOp::MulI64(a, b) => {
                    (regs_stack[base + a] as i64).wrapping_mul(regs_stack[base + b] as i64) as u64
                }
                FastOp::DivF64(a, b) => {
                    (f64::from_bits(regs_stack[base + a]) / f64::from_bits(regs_stack[base + b]))
                        .to_bits()
                }
                FastOp::DivI64(a, b) => {
                    let denom = regs_stack[base + b] as i64;
                    if denom == 0 {
                        0
                    } else {
                        (regs_stack[base + a] as i64).wrapping_div(denom) as u64
                    }
                }
                FastOp::RemI64(a, b) => {
                    let denom = regs_stack[base + b] as i64;
                    if denom == 0 {
                        0
                    } else {
                        (regs_stack[base + a] as i64).wrapping_rem(denom) as u64
                    }
                }
                FastOp::CmpEqF64(a, b) => u64::from(
                    f64::from_bits(regs_stack[base + a]) == f64::from_bits(regs_stack[base + b]),
                ),
                FastOp::CmpEqI64(a, b) => u64::from(regs_stack[base + a] == regs_stack[base + b]),
                FastOp::CmpLtF64(a, b) => u64::from(
                    f64::from_bits(regs_stack[base + a]) < f64::from_bits(regs_stack[base + b]),
                ),
                FastOp::CmpLtI64(a, b) => {
                    u64::from((regs_stack[base + a] as i64) < (regs_stack[base + b] as i64))
                }
                FastOp::Sin(a) => f64::from_bits(regs_stack[base + a]).sin().to_bits(),
                FastOp::Cos(a) => f64::from_bits(regs_stack[base + a]).cos().to_bits(),
                FastOp::Tan(a) => f64::from_bits(regs_stack[base + a]).tan().to_bits(),
                FastOp::Sqrt(a) => f64::from_bits(regs_stack[base + a]).sqrt().to_bits(),
                FastOp::Abs(a) => f64::from_bits(regs_stack[base + a]).abs().to_bits(),
                FastOp::Floor(a) => f64::from_bits(regs_stack[base + a]).floor().to_bits(),
                FastOp::Ceil(a) => f64::from_bits(regs_stack[base + a]).ceil().to_bits(),
                FastOp::Pow(a, b) => f64::from_bits(regs_stack[base + a])
                    .powf(f64::from_bits(regs_stack[base + b]))
                    .to_bits(),
                FastOp::Atan2(a, b) => f64::from_bits(regs_stack[base + a])
                    .atan2(f64::from_bits(regs_stack[base + b]))
                    .to_bits(),
                FastOp::Min(a, b) => f64::from_bits(regs_stack[base + a])
                    .min(f64::from_bits(regs_stack[base + b]))
                    .to_bits(),
                FastOp::Max(a, b) => f64::from_bits(regs_stack[base + a])
                    .max(f64::from_bits(regs_stack[base + b]))
                    .to_bits(),
                FastOp::IntToFloat(a) => ((regs_stack[base + a] as i64) as f64).to_bits(),
                FastOp::FloatToInt(a) => (f64::from_bits(regs_stack[base + a]) as i64) as u64,
                FastOp::StructNew(nfields) => {
                    let sbase = arena.len();
                    arena.resize(sbase + *nfields, 0);
                    sbase as u64
                }
                FastOp::StructGet(s, fi) => {
                    let sbase = regs_stack[base + s] as usize;
                    arena.get(sbase + *fi).copied().unwrap_or(0)
                }
                FastOp::StructSet(s, fi, v) => {
                    let sbase = regs_stack[base + s] as usize;
                    let val = regs_stack[base + v];
                    if sbase + *fi < arena.len() {
                        arena[sbase + *fi] = val;
                    }
                    sbase as u64
                }
                FastOp::Call(cid, cargs) => {
                    let mut sub_args = [0u64; 16];
                    let n = cargs.len().min(16);
                    for (i, &slot) in cargs.iter().take(n).enumerate() {
                        sub_args[i] = regs_stack[base + slot];
                    }
                    eval_fast_fn(fns, *cid, &sub_args[..n], regs_stack, arena, tmp_args)
                }
                FastOp::Nop => continue,
            };
            if let Some(d) = inst.dst {
                regs_stack[base + d] = val;
            }
        }

        match &blk.term {
            FastTerm::Ret(r) => {
                break r.map(|slot| regs_stack[base + slot]).unwrap_or(0);
            }
            FastTerm::Br(target, bargs) => {
                if !bargs.is_empty() {
                    tmp_args.clear();
                    for &slot in bargs {
                        tmp_args.push(regs_stack[base + slot]);
                    }
                    for (i, &pslot) in f.blocks[*target].params.iter().enumerate() {
                        if let Some(&v) = tmp_args.get(i) {
                            regs_stack[base + pslot] = v;
                        }
                    }
                }
                cur_blk = *target;
            }
            FastTerm::BrIf {
                cond,
                then_blk,
                then_args,
                else_blk,
                else_args,
            } => {
                let take_then = regs_stack[base + cond] != 0;
                let (target, bargs) = if take_then {
                    (*then_blk, then_args)
                } else {
                    (*else_blk, else_args)
                };
                if !bargs.is_empty() {
                    tmp_args.clear();
                    for &slot in bargs {
                        tmp_args.push(regs_stack[base + slot]);
                    }
                    for (i, &pslot) in f.blocks[target].params.iter().enumerate() {
                        if let Some(&v) = tmp_args.get(i) {
                            regs_stack[base + pslot] = v;
                        }
                    }
                }
                cur_blk = target;
            }
        }
    };

    regs_stack.truncate(base);
    ret_val
}

fn render_shader_vm(
    module: &Module,
    shader_name: &str,
    req_w: u32,
    req_h: u32,
    time: f64,
    uniforms: &[f64],
) -> Option<Pixmap> {
    let sh = module.get_function(shader_name)?;
    if !sh.is_shader {
        return None;
    }

    let (gw, gh) = preview_grid_size(req_w, req_h, 1_600.0, 56.0, 40.0);
    let mut small = Pixmap::new(gw, gh)?;
    let mut mock = MockPlatform::new();
    let first_is_f64 = matches!(sh.params.first().map(|(_, t)| t), Some(Type::F64));
    let inv_w = 1.0 / gw as f64;
    let inv_h = 1.0 / gh as f64;
    let data = small.data_mut();

    for py in 0..gh {
        let v = (py as f64 + 0.5) * inv_h;
        let row_off = (py as usize) * (gw as usize);
        for px in 0..gw {
            let u = (px as f64 + 0.5) * inv_w;
            let mut call_args = Vec::with_capacity(sh.params.len());
            let mut custom_idx = 0usize;
            for (idx, (_, ty)) in sh.params.iter().enumerate() {
                match (idx, ty) {
                    (0, Type::Struct(sname)) => {
                        call_args.push(Value::Struct {
                            schema: sname.clone(),
                            fields: vec![Value::F64(u), Value::F64(v)],
                        });
                    }
                    (0, Type::F64) => call_args.push(Value::F64(u)),
                    (1, Type::F64) => {
                        call_args.push(Value::F64(if first_is_f64 { v } else { time }));
                    }
                    (2, Type::F64) if first_is_f64 => call_args.push(Value::F64(time)),
                    (_, Type::Struct(sname)) => {
                        call_args.push(Value::Struct {
                            schema: sname.clone(),
                            fields: vec![Value::F64(0.5), Value::F64(0.5)],
                        });
                    }
                    (_, Type::F64) => {
                        call_args.push(Value::F64(uniforms.get(custom_idx).copied().unwrap_or(0.0)));
                        custom_idx += 1;
                    }
                    (_, Type::I64) => {
                        call_args.push(Value::I64(
                            uniforms.get(custom_idx).copied().unwrap_or(0.0) as i64,
                        ));
                        custom_idx += 1;
                    }
                    (_, Type::Bool) => {
                        call_args.push(Value::Bool(
                            uniforms.get(custom_idx).copied().unwrap_or(0.0) > 0.5,
                        ));
                        custom_idx += 1;
                    }
                    _ => return None,
                }
            }

            let Ok((val, _)) =
                knr_interp::execute_module(module, shader_name, call_args, &mut mock, 250_000, false)
            else {
                return None;
            };

            let (rf, gf, bf, af) = match val {
                Value::Struct { fields, .. } if fields.len() >= 3 => {
                    let ch = |i: usize, def: f64| match fields.get(i) {
                        Some(Value::F64(x)) => x.clamp(0.0, 1.0),
                        Some(Value::I64(n)) => (*n as f64).clamp(0.0, 1.0),
                        _ => def,
                    };
                    (ch(0, 0.0), ch(1, 0.0), ch(2, 0.0), ch(3, 1.0))
                }
                Value::F64(lum) => {
                    let l = lum.clamp(0.0, 1.0);
                    (l, l, l, 1.0)
                }
                _ => (0.04, 0.07, 0.13, 1.0),
            };
            let off = (row_off + px as usize) * 4;
            data[off] = (rf * af * 255.0).round() as u8;
            data[off + 1] = (gf * af * 255.0).round() as u8;
            data[off + 2] = (bf * af * 255.0).round() as u8;
            data[off + 3] = (af * 255.0).round() as u8;
        }
    }

    Some(small)
}
