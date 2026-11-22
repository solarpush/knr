use std::collections::HashSet;
use std::fs;
use std::hint::black_box;
use std::path::Path;
use std::time::Instant;

use knr_codegen::{CraneliftBackend, WasmBackend};
use knr_core::{BasicBlock, Function, Instruction, Module, Op, Terminator, Type};
use knr_interp::{ExecutionStats, Vm};
use knr_runtime::mock::MockPlatform;
use serde::Serialize;

use crate::args::BenchArgs;
use crate::error::CliError;

#[derive(Debug, Clone, Serialize)]
pub struct FunctionProfileEntry {
    pub name: String,
    pub calls: u64,
    pub instructions: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct VmBenchStats {
    pub steps: u64,
    pub instructions: u64,
    pub gas_used: u64,
    pub allocations: u64,
    pub frees: u64,
    pub top_functions: Vec<FunctionProfileEntry>,
    pub min_us: f64,
    pub mean_us: f64,
    pub p50_us: f64,
    pub max_us: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct JitBenchStats {
    pub compile_ms: f64,
    pub min_ns: f64,
    pub mean_ns: f64,
    pub p50_ns: f64,
    pub max_ns: f64,
    pub min_us: f64,
    pub mean_us: f64,
    pub p50_us: f64,
    pub max_us: f64,
    pub speedup_vs_vm: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct SizeStats {
    pub knrbin_bytes: Option<usize>,
    pub wasm_bytes: Option<usize>,
}

#[derive(Debug, Clone, Serialize)]
pub struct BenchReport {
    pub file: String,
    pub target: String,
    pub iters: usize,
    pub function_count: usize,
    pub shader_count: usize,
    pub vm: VmBenchStats,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub jit: Option<JitBenchStats>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub jit_note: Option<String>,
    pub sizes: SizeStats,
}

#[derive(Debug, Clone)]
enum BenchTarget {
    ZeroArg(String),
    InitView,
}

impl BenchTarget {
    fn label(&self) -> String {
        match self {
            BenchTarget::ZeroArg(name) => format!("{name}()"),
            BenchTarget::InitView => "view(init())".to_string(),
        }
    }
}

/// Runs deterministic VM + Cranelift JIT + artifact size benchmarks on `file`.
pub fn run_benchmark(
    file: &Path,
    fn_name: Option<&str>,
    iters: usize,
    gas: u64,
) -> Result<BenchReport, CliError> {
    if iters == 0 || iters > 100_000 {
        return Err(CliError::Args(format!(
            "Invalid iteration count {iters}: expected 1..=100000"
        )));
    }

    // 1. Parse & statically verify module
    let module = load_and_verify_module(file)?;

    // 2. Select target function
    let target = select_target(&module, fn_name)?;

    // 3. Benchmark reference VM (1 warmup + `iters` measured runs)
    let vm_stats = bench_vm(&module, &target, iters, gas)?;

    // 4. Benchmark Cranelift JIT compile + native execution (graceful fallback if ineligible)
    let (jit, jit_note) = bench_jit(&module, &target, iters, vm_stats.mean_us);

    // 5. Measure compact binary bytecode (.knrbin) and zero-import WASM sizes
    let knrbin_bytes = knr_core::encode_knrbin(&module).ok().map(|b| b.len());
    let wasm_bytes = WasmBackend::new().compile(&module).ok().map(|b| b.len());

    let shader_count = module.functions.iter().filter(|f| f.is_shader).count();

    Ok(BenchReport {
        file: file.display().to_string(),
        target: target.label(),
        iters,
        function_count: module.functions.len(),
        shader_count,
        vm: vm_stats,
        jit,
        jit_note,
        sizes: SizeStats {
            knrbin_bytes,
            wasm_bytes,
        },
    })
}

pub fn execute(args: &BenchArgs) -> Result<(), CliError> {
    let report = run_benchmark(&args.file, args.fn_name.as_deref(), args.iters, args.gas)?;

    if args.json {
        let pretty = serde_json::to_string_pretty(&report)
            .map_err(|e| CliError::Other(e.to_string()))?;
        println!("{pretty}");
    } else {
        println!(
            "knr bench: {} -> {} ({} iterations)",
            report.file, report.target, report.iters
        );
        println!(
            "  Modules: {} function(s), {} @shader kernel(s)",
            report.function_count, report.shader_count
        );
        println!();
        println!(
            "  {:<14} {:>12} {:>12} {:>12} {:>12} {:>12}",
            "Engine", "Min", "P50", "Mean", "Max", "Extra"
        );
        println!("  {}", "-".repeat(78));
        println!(
            "  {:<14} {:>9.2} us {:>9.2} us {:>9.2} us {:>9.2} us {:>9} ops",
            "VM (interp)",
            report.vm.min_us,
            report.vm.p50_us,
            report.vm.mean_us,
            report.vm.max_us,
            report.vm.steps
        );
        if let Some(ref jit) = report.jit {
            println!(
                "  {:<14} {:>9.2} us {:>9.2} us {:>9.2} us {:>9.2} us {:>8.1}x",
                "Cranelift JIT",
                jit.min_us,
                jit.p50_us,
                jit.mean_us,
                jit.max_us,
                jit.speedup_vs_vm
            );
            println!(
                "                 ({:.0} ns min | {:.0} ns p50 | {:.0} ns mean | compile: {:.2} ms)",
                jit.min_ns, jit.p50_ns, jit.mean_ns, jit.compile_ms
            );
        } else if let Some(ref note) = report.jit_note {
            println!("  {:<14} skipped ({note})", "Cranelift JIT");
        }
        println!();
        println!(
            "  VM Profile: instructions = {} | gas_used = {} | allocations = {} | frees = {}",
            report.vm.instructions, report.vm.gas_used, report.vm.allocations, report.vm.frees
        );
        if !report.vm.top_functions.is_empty() {
            println!("  Top functions:");
            for entry in &report.vm.top_functions {
                println!(
                    "    {:<24} {:>6} call(s)  {:>10} instrs",
                    entry.name, entry.calls, entry.instructions
                );
            }
        }
        println!();
        let knrbin_str = report
            .sizes
            .knrbin_bytes
            .map(|b| format!("{b} B"))
            .unwrap_or_else(|| "n/a".to_string());
        let wasm_str = report
            .sizes
            .wasm_bytes
            .map(|b| format!("{b} B"))
            .unwrap_or_else(|| "n/a".to_string());
        println!("  Sizes: .knrbin = {knrbin_str} | .wasm = {wasm_str}");
    }

    Ok(())
}

fn load_and_verify_module(file: &Path) -> Result<Module, CliError> {
    if !file.is_file() {
        return Err(CliError::Other(format!(
            "Benchmark file '{}' not found",
            file.display()
        )));
    }
    if knr_core::is_source_file(file) {
        let src = fs::read_to_string(file).map_err(CliError::Io)?;
        let _prog = knr_core::syntax::parse_tsx(&src).map_err(CliError::Parse)?;
        knr_verify::verify_program_from_file(file).map_err(|diags| {
            for d in &diags {
                eprintln!("{d}");
            }
            CliError::Verify(diags.len())
        })
    } else {
        let module = knr_core::load_module_from_path(file).map_err(CliError::Parse)?;
        if let Err(diags) = knr_verify::verify_module(&module) {
            for d in &diags {
                eprintln!("{d}");
            }
            return Err(CliError::Verify(diags.len()));
        }
        Ok(module)
    }
}

fn select_target(module: &Module, explicit_fn: Option<&str>) -> Result<BenchTarget, CliError> {
    if let Some(name) = explicit_fn {
        let func = module.get_function(name).ok_or_else(|| {
            CliError::Other(format!("Function '{name}' not found in module"))
        })?;
        if name == "view"
            && func.params.len() == 1
            && module
                .get_function("init")
                .is_some_and(|f| f.params.is_empty())
        {
            return Ok(BenchTarget::InitView);
        }
        if !func.params.is_empty() {
            return Err(CliError::Other(format!(
                "Function '{name}' expects {} parameter(s); benchmark targets must be 0-arg or `view(init())`",
                func.params.len()
            )));
        }
        return Ok(BenchTarget::ZeroArg(name.to_string()));
    }

    if module
        .get_function("main")
        .is_some_and(|f| f.params.is_empty())
    {
        return Ok(BenchTarget::ZeroArg("main".to_string()));
    }
    if module
        .get_function("init")
        .is_some_and(|f| f.params.is_empty())
        && module
            .get_function("view")
            .is_some_and(|f| f.params.len() == 1)
    {
        return Ok(BenchTarget::InitView);
    }
    if module
        .get_function("init")
        .is_some_and(|f| f.params.is_empty())
    {
        return Ok(BenchTarget::ZeroArg("init".to_string()));
    }
    if module
        .get_function("view")
        .is_some_and(|f| f.params.is_empty())
    {
        return Ok(BenchTarget::ZeroArg("view".to_string()));
    }
    if let Some(first) = module
        .functions
        .iter()
        .find(|f| !f.is_shader && f.params.is_empty())
    {
        return Ok(BenchTarget::ZeroArg(first.name.clone()));
    }

    Err(CliError::Other(
        "No benchmarkable 0-arg function (or `view(init())` pair) found in module".to_string(),
    ))
}

fn merge_execution_stats(a: &ExecutionStats, b: &ExecutionStats) -> ExecutionStats {
    let mut merged = ExecutionStats {
        total_instructions: a.total_instructions.saturating_add(b.total_instructions),
        gas_used: a.gas_used.saturating_add(b.gas_used),
        function_calls: a.function_calls.clone(),
        function_instructions: a.function_instructions.clone(),
        allocations: a.allocations.saturating_add(b.allocations),
        frees: a.frees.saturating_add(b.frees),
    };
    for (k, &v) in &b.function_calls {
        *merged.function_calls.entry(k.clone()).or_insert(0) += v;
    }
    for (k, &v) in &b.function_instructions {
        *merged.function_instructions.entry(k.clone()).or_insert(0) += v;
    }
    merged
}

fn build_top_functions(stats: &ExecutionStats) -> Vec<FunctionProfileEntry> {
    let mut entries: Vec<FunctionProfileEntry> = stats
        .function_instructions
        .iter()
        .map(|(name, &instructions)| FunctionProfileEntry {
            name: name.clone(),
            calls: stats.function_calls.get(name).copied().unwrap_or(0),
            instructions,
        })
        .collect();
    for (name, &calls) in &stats.function_calls {
        if !stats.function_instructions.contains_key(name) {
            entries.push(FunctionProfileEntry {
                name: name.clone(),
                calls,
                instructions: 0,
            });
        }
    }
    entries.sort_by(|a, b| {
        b.instructions
            .cmp(&a.instructions)
            .then_with(|| a.name.cmp(&b.name))
    });
    entries.truncate(10);
    entries
}

fn execute_vm_once_with_stats(
    module: &Module,
    target: &BenchTarget,
    gas: u64,
) -> Result<ExecutionStats, CliError> {
    let mut mock = MockPlatform::new();
    match target {
        BenchTarget::ZeroArg(name) => {
            let (val, _, stats) = knr_interp::execute_module_with_stats(
                module,
                name,
                vec![],
                &mut mock,
                gas,
                false,
            )
            .map_err(CliError::Vm)?;
            black_box(val);
            Ok(stats)
        }
        BenchTarget::InitView => {
            let (state, _, stats1) = knr_interp::execute_module_with_stats(
                module,
                "init",
                vec![],
                &mut mock,
                gas,
                false,
            )
            .map_err(CliError::Vm)?;
            let (widget, _, stats2) = knr_interp::execute_module_with_stats(
                module,
                "view",
                vec![state],
                &mut mock,
                gas,
                false,
            )
            .map_err(CliError::Vm)?;
            black_box(widget);
            Ok(merge_execution_stats(&stats1, &stats2))
        }
    }
}

fn execute_vm_once(module: &Module, target: &BenchTarget, gas: u64) -> Result<u64, CliError> {
    let mut mock = MockPlatform::new();
    match target {
        BenchTarget::ZeroArg(name) => {
            let func = module.get_function(name).ok_or_else(|| {
                CliError::Other(format!("Function '{name}' not found"))
            })?;
            let mut vm = Vm::new(&mut mock, Some(module), gas, false);
            let (val, _) = vm.run(func, vec![]).map_err(CliError::Vm)?;
            black_box(val);
            Ok(vm.step_count())
        }
        BenchTarget::InitView => {
            let init_fn = module
                .get_function("init")
                .ok_or_else(|| CliError::Other("Missing `init` function".to_string()))?;
            let view_fn = module
                .get_function("view")
                .ok_or_else(|| CliError::Other("Missing `view` function".to_string()))?;
            let mut vm1 = Vm::new(&mut mock, Some(module), gas, false);
            let (state, _) = vm1.run(init_fn, vec![]).map_err(CliError::Vm)?;
            let steps1 = vm1.step_count();

            let mut vm2 = Vm::new(&mut mock, Some(module), gas, false);
            let (widget, _) = vm2.run(view_fn, vec![state]).map_err(CliError::Vm)?;
            black_box(widget);
            Ok(steps1.saturating_add(vm2.step_count()))
        }
    }
}

fn bench_vm(
    module: &Module,
    target: &BenchTarget,
    iters: usize,
    gas: u64,
) -> Result<VmBenchStats, CliError> {
    // First (warmup) run captures deterministic VM profiler statistics
    let stats = execute_vm_once_with_stats(module, target, gas)?;
    let steps = stats.gas_used;
    let top_functions = build_top_functions(&stats);

    let mut samples_us = Vec::with_capacity(iters);
    for _ in 0..iters {
        let start = Instant::now();
        let _ = execute_vm_once(module, target, gas)?;
        samples_us.push(start.elapsed().as_nanos() as f64 / 1_000.0);
    }

    let (min_us, p50_us, mean_us, max_us) = summarize_samples(&mut samples_us);
    Ok(VmBenchStats {
        steps,
        instructions: stats.total_instructions,
        gas_used: stats.gas_used,
        allocations: stats.allocations,
        frees: stats.frees,
        top_functions,
        min_us,
        mean_us,
        p50_us,
        max_us,
    })
}

fn has_transitive_syscalls(module: &Module, entry: &str) -> bool {
    fn visit(module: &Module, name: &str, visited: &mut HashSet<String>) -> bool {
        if !visited.insert(name.to_string()) {
            return false;
        }
        let Some(func) = module.get_function(name) else {
            return false;
        };
        if !func.effects.is_empty() {
            return true;
        }
        for blk in &func.blocks {
            for inst in &blk.instructions {
                match &inst.op {
                    Op::Syscall { .. } | Op::Spawn(_, _) => return true,
                    Op::Call(callee, _) if visit(module, callee, visited) => return true,
                    _ => {}
                }
            }
        }
        false
    }

    let mut visited = HashSet::new();
    visit(module, entry, &mut visited)
}

fn bench_jit(
    module: &Module,
    target: &BenchTarget,
    iters: usize,
    vm_mean_us: f64,
) -> (Option<JitBenchStats>, Option<String>) {
    // Check host-effect / syscall safety before attempting native JIT execution
    match target {
        BenchTarget::ZeroArg(name) => {
            if has_transitive_syscalls(module, name) {
                return (
                    None,
                    Some("target uses host effects/syscalls; measured via VM with MockPlatform".to_string()),
                );
            }
        }
        BenchTarget::InitView => {
            if has_transitive_syscalls(module, "init") || has_transitive_syscalls(module, "view") {
                return (
                    None,
                    Some("target uses host effects/syscalls; measured via VM with MockPlatform".to_string()),
                );
            }
        }
    }

    // Build JIT module (wrapping `view(init())` in a synthetic 0-arg entry if needed)
    let (jit_mod_owned, entry_name, ret_ty) = match target {
        BenchTarget::ZeroArg(name) => {
            let Some(func) = module.get_function(name) else {
                return (None, Some("target function not found".to_string()));
            };
            match &func.ret_ty {
                Type::I64 | Type::F64 | Type::Bool | Type::Void | Type::Struct(_) => {
                    (module.clone(), name.clone(), func.ret_ty.clone())
                }
                other => {
                    return (
                        None,
                        Some(format!(
                            "return type {other:?} uses managed heap handle; VM-only benchmark"
                        )),
                    );
                }
            }
        }
        BenchTarget::InitView => {
            let Some(init_fn) = module.get_function("init") else {
                return (None, Some("missing init()".to_string()));
            };
            let Some(view_fn) = module.get_function("view") else {
                return (None, Some("missing view()".to_string()));
            };
            let wrapper_name = "__knr_bench_init_view".to_string();
            let mut wrapper = Function::new(&wrapper_name, view_fn.ret_ty.clone());
            let mut bb = BasicBlock::new(0, Terminator::Ret(None));
            bb.push(Instruction::new(
                Some(0),
                init_fn.ret_ty.clone(),
                Op::Call("init".to_string(), vec![]),
                1,
            ));
            bb.push(Instruction::new(
                Some(1),
                view_fn.ret_ty.clone(),
                Op::Call("view".to_string(), vec![0]),
                1,
            ));
            if view_fn.param_borrows.first().copied().unwrap_or(false) {
                bb.push(Instruction::new(None, Type::Void, Op::Free(0), 1));
            }
            bb.terminator = if view_fn.ret_ty == Type::Void {
                Terminator::Ret(None)
            } else {
                Terminator::Ret(Some(1))
            };
            wrapper.add_block(bb);

            let mut m = module.clone();
            m.add_function(wrapper);
            (m, wrapper_name, view_fn.ret_ty.clone())
        }
    };

    let compile_start = Instant::now();
    let mut backend = match CraneliftBackend::new() {
        Ok(b) => b,
        Err(e) => return (None, Some(format!("Cranelift init unavailable: {e}"))),
    };
    let code_ptr = match backend.compile_jit_module(&jit_mod_owned, &entry_name) {
        Ok(ptr) => ptr,
        Err(e) => return (None, Some(format!("Cranelift JIT fallback: {e}"))),
    };
    let compile_ms = compile_start.elapsed().as_nanos() as f64 / 1_000_000.0;

    let mut samples_ns = Vec::with_capacity(iters);

    match ret_ty {
        Type::I64 | Type::Bool | Type::Struct(_) => {
            let f: extern "C" fn() -> i64 =
                unsafe { std::mem::transmute::<*const u8, extern "C" fn() -> i64>(code_ptr) };
            // Warmup
            black_box(f());
            for _ in 0..iters {
                let start = Instant::now();
                let res = f();
                black_box(res);
                samples_ns.push(start.elapsed().as_nanos() as f64);
            }
        }
        Type::F64 => {
            let f: extern "C" fn() -> f64 =
                unsafe { std::mem::transmute::<*const u8, extern "C" fn() -> f64>(code_ptr) };
            black_box(f());
            for _ in 0..iters {
                let start = Instant::now();
                let res = f();
                black_box(res);
                samples_ns.push(start.elapsed().as_nanos() as f64);
            }
        }
        Type::Void => {
            let f: extern "C" fn() =
                unsafe { std::mem::transmute::<*const u8, extern "C" fn()>(code_ptr) };
            f();
            for _ in 0..iters {
                let start = Instant::now();
                f();
                samples_ns.push(start.elapsed().as_nanos() as f64);
            }
        }
        _ => {
            return (
                None,
                Some("unsupported JIT return ABI; VM-only benchmark".to_string()),
            );
        }
    }

    // Keep backend alive through entire execution above
    drop(backend);

    let (min_ns, p50_ns, mean_ns, max_ns) = summarize_samples(&mut samples_ns);
    let min_us = min_ns / 1_000.0;
    let p50_us = p50_ns / 1_000.0;
    let mean_us = mean_ns / 1_000.0;
    let max_us = max_ns / 1_000.0;
    let speedup_vs_vm = if mean_us > 0.0 {
        vm_mean_us / mean_us
    } else {
        0.0
    };

    (
        Some(JitBenchStats {
            compile_ms,
            min_ns,
            mean_ns,
            p50_ns,
            max_ns,
            min_us,
            mean_us,
            p50_us,
            max_us,
            speedup_vs_vm,
        }),
        None,
    )
}

fn summarize_samples(samples: &mut [f64]) -> (f64, f64, f64, f64) {
    if samples.is_empty() {
        return (0.0, 0.0, 0.0, 0.0);
    }
    samples.sort_by(|a, b| a.total_cmp(b));
    let min_v = samples[0];
    let max_v = samples[samples.len() - 1];
    let mean_v = samples.iter().sum::<f64>() / samples.len() as f64;
    let mid = samples.len() / 2;
    let p50_v = if samples.len() % 2 == 1 {
        samples[mid]
    } else {
        (samples[mid - 1] + samples[mid]) * 0.5
    };
    (min_v, p50_v, mean_v, max_v)
}
