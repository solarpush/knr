use std::path::PathBuf;

use crate::error::CliError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuildTarget {
    Native,
    Wasm,
    Web,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestArgs {
    pub path: PathBuf,
    pub json: bool,
    pub gas: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ExploreFormat {
    #[default]
    Ai,
    Json,
    Mermaid,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckArgs {
    pub file: PathBuf,
    pub json: bool,
    pub graph: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExploreArgs {
    pub path: PathBuf,
    pub format: ExploreFormat,
    pub symbol: Option<String>,
    pub callers: Option<String>,
    pub callees: Option<String>,
    pub effects: Option<String>,
    pub orphans: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SkillArgs {
    pub json: bool,
    pub output: Option<PathBuf>,
    pub install_dir: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunArgs {
    pub file: PathBuf,
    pub gas: u64,
    pub trace: bool,
    pub profile: bool,
    pub watch: bool,
    pub once: bool,
    pub gui: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DevArgs {
    pub path: Option<PathBuf>,
    pub debounce_ms: u64,
    pub poll_ms: u64,
    pub once: bool,
    pub gas: u64,
    pub trace: bool,
    pub clear: bool,
    pub gui: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InitArgs {
    pub dir: Option<PathBuf>,
    pub template: String,
    pub name: Option<String>,
    pub workspace: bool,
    pub force: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewArgs {
    pub path: PathBuf,
    pub workspace: bool,
    pub template: String,
    pub name: Option<String>,
    pub force: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum RenderStep {
    Action(String),
    Click(String),
    Input { key: String, value: String },
    Slider { key: String, value: f64 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewportSpec {
    pub name: String,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RenderArgs {
    pub file: PathBuf,
    pub output: Option<PathBuf>,
    pub width: u32,
    pub height: u32,
    pub scale: f32,
    pub time: f32,
    pub actions: Vec<String>,
    pub steps: Vec<RenderStep>,
    pub viewports: Vec<ViewportSpec>,
    pub diff_out: Option<PathBuf>,
    pub flow: Option<PathBuf>,
    pub audit: bool,
    pub assert_audit: bool,
    pub baseline: Option<PathBuf>,
    pub max_diff: f64,
    pub json: bool,
    pub gas: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DoctorArgs {
    pub json: bool,
    pub dir: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BenchArgs {
    pub file: PathBuf,
    pub fn_name: Option<String>,
    pub iters: usize,
    pub json: bool,
    pub gas: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FmtArgs {
    pub paths: Vec<PathBuf>,
    pub check: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FixArgs {
    pub file: PathBuf,
    pub write: bool,
    pub json: bool,
}

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Clone, PartialEq)]
pub enum CliCommand {
    Check(CheckArgs),
    Run(RunArgs),
    Render(RenderArgs),
    Build {
        file: PathBuf,
        target: BuildTarget,
        output: PathBuf,
        gui: bool,
        standalone: bool,
    },
    Asm {
        file: PathBuf,
        output: PathBuf,
    },
    Disasm {
        file: PathBuf,
        output: Option<PathBuf>,
    },
    Update {
        check: bool,
        force: bool,
        version: Option<String>,
    },
    Explore(ExploreArgs),
    Fmt(FmtArgs),
    Fix(FixArgs),
    Skill(SkillArgs),
    Dev(DevArgs),
    Test(TestArgs),
    Lsp,
    Mcp,
    Doctor(DoctorArgs),
    Bench(BenchArgs),
    Init(InitArgs),
    New(NewArgs),
    Help,
    Version,
}

pub type Command = CliCommand;

pub fn print_main_help() {
    println!(
        "knr {VERSION} - Knr Strict TSX/TS & Linear SSA compiler, virtual machine, and unified GPU UI toolchain

USAGE:
    knr <SUBCOMMAND> [OPTIONS]

SUBCOMMANDS:
    check     Statically verify a .tsk, .knr, or .ssa module
    run       Execute a .tsk, .knr, or .ssa module in the reference interpreter (or --gui desktop app)
    render    Render a headless HiDPI PNG snapshot, multi-viewport audit, visual diff, or --flow E2E journey
    test      Discover and run deterministic test_*() suites in .tsk/.knr/.ssa files
    bench     Benchmark VM gas/steps, Cranelift JIT compile & native latency, and binary sizes
    doctor    Check toolchain environment, native linker, runtime, display server, and project health
    mcp       Run the built-in Model Context Protocol (MCP 2024-11-05) JSON-RPC 2.0 server over stdio
    build     Compile a module to native executable, WebAssembly (.wasm), or unified Canvas+WebGPU Web app
    asm       Assemble source or textual SSA (.ssa) to compact binary bytecode (.knrbin)
    disasm    Disassemble binary bytecode (.knrbin) to canonical textual SSA (.ssa)
    update    Check for updates and self-update from GitHub Releases
    explore   Explore project structure, call graphs, schemas, and dependencies
    fmt       Canonically format .tsk / .knr source files
    fix       Automatically propose or apply compiler-verified QuickFixes to a .tsk file
    skill     Output or install the official AI Agent SKILL.md and VDOM helper for knr
    dev       Watch project files and automatically re-verify/re-run (or --gui hot-reload) on changes
    lsp       Run the built-in Language Server Protocol (LSP) server over stdio
    init      Initialize a knr project in an existing directory
    new       Create a new knr project or workspace directory

FLAGS:
    -g, --gui        Launch native desktop GUI window with execution graph visualization
    -h, --help       Print help information
    -V, --version    Print version information"
    );
}

pub fn print_version() {
    println!("knr {VERSION}");
}

pub fn parse_args<I: IntoIterator<Item = String>>(args: I) -> Result<CliCommand, CliError> {
    let args: Vec<String> = args.into_iter().collect();
    if args.len() <= 1 {
        return Err(CliError::Args(
            "Usage: knr <check|run|render|test|bench|doctor|mcp|build|asm|disasm|update|explore|fmt|fix|skill|dev|lsp|init|new> [options]\nUse 'knr --help' for usage."
                .to_string(),
        ));
    }

    match args[1].as_str() {
        "--help" | "-h" => {
            print_main_help();
            Ok(CliCommand::Help)
        }
        "--version" | "-V" => {
            print_version();
            Ok(CliCommand::Version)
        }
        "check" => parse_check(&args[2..]),
        "run" => parse_run(&args[2..]),
        "render" => parse_render(&args[2..]),
        "test" => parse_test(&args[2..]),
        "bench" => parse_bench(&args[2..]),
        "doctor" => parse_doctor(&args[2..]),
        "mcp" => parse_mcp(&args[2..]),
        "build" => parse_build(&args[2..]),
        "asm" => parse_asm(&args[2..]),
        "disasm" => parse_disasm(&args[2..]),
        "update" => parse_update(&args[2..]),
        "explore" => parse_explore(&args[2..]),
        "fmt" | "format" => parse_fmt(&args[2..]),
        "fix" => parse_fix(&args[2..]),
        "skill" | "skills" => parse_skill(&args[2..]),
        "dev" => parse_dev(&args[2..]),
        "lsp" => parse_lsp(&args[2..]),
        "init" => parse_init(&args[2..]),
        "new" => parse_new(&args[2..]),
        cmd => Err(CliError::Args(format!(
            "Unknown subcommand: '{cmd}'. Use 'knr --help' for usage."
        ))),
    }
}

fn parse_check(args: &[String]) -> Result<CliCommand, CliError> {
    let mut file: Option<PathBuf> = None;
    let mut json = false;
    let mut graph = false;

    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if arg == "--help" || arg == "-h" {
            println!(
                "knr check - Statically verify a .tsk, .knr, or .ssa module

USAGE:
    knr check <FILE> [OPTIONS]

ARGS:
    <FILE>    Path to .tsk, .knr, or .ssa source file

OPTIONS:
    --graph       Print ProjectGraph after static verification succeeds
    --json        Output diagnostics as structured JSON array
    -h, --help    Print help information"
            );
            return Ok(CliCommand::Help);
        } else if arg == "--json" {
            json = true;
        } else if arg == "--graph" {
            graph = true;
        } else if arg.starts_with('-') {
            return Err(CliError::Args(format!("Unknown option for 'check': {arg}")));
        } else if file.is_none() {
            file = Some(PathBuf::from(arg));
        } else {
            return Err(CliError::Args(format!(
                "Unexpected argument for 'check': {arg}"
            )));
        }
        i += 1;
    }

    let file = file.ok_or_else(|| {
        CliError::Args(
            "Missing input file for 'knr check'. Usage: knr check <file.ssa> [--json] [--graph]"
                .to_string(),
        )
    })?;

    Ok(CliCommand::Check(CheckArgs { file, json, graph }))
}

fn parse_explore(args: &[String]) -> Result<CliCommand, CliError> {
    let mut path: Option<PathBuf> = None;
    let mut format: Option<ExploreFormat> = None;
    let mut json = false;
    let mut symbol: Option<String> = None;
    let mut callers: Option<String> = None;
    let mut callees: Option<String> = None;
    let mut effects: Option<String> = None;
    let mut orphans = false;

    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if arg == "--help" || arg == "-h" {
            println!(
                "knr explore - Structural exploration and semantic query engine

USAGE:
    knr explore <PATH> [OPTIONS]

ARGS:
    <PATH>    Path to .ssa file, .ssa.schema file, or project directory

OPTIONS:
    --format <ai|json|mermaid>  Output format (default: ai)
    --json                      Output as JSON (shorthand for --format json)
    -s, --symbol <NAME>         Show definition and cross-references for schema or function
    --callers <FN>              Show direct and transitive callers of function
    --callees <FN>              Show direct and transitive callees and syscalls of function
    --effects <EFF>             Filter functions declaring or invoking effect (ui, net, hw, async)
    --orphans                   Show only orphan schemas and uncalled functions
    -h, --help                  Print help information"
            );
            return Ok(CliCommand::Help);
        } else if arg == "--json" {
            json = true;
        } else if arg == "--orphans" {
            orphans = true;
        } else if arg == "--format" {
            i += 1;
            if i >= args.len() {
                return Err(CliError::Args("Missing value for '--format'".to_string()));
            }
            format = Some(parse_explore_format(&args[i])?);
        } else if let Some(val) = arg.strip_prefix("--format=") {
            format = Some(parse_explore_format(val)?);
        } else if arg == "-s" || arg == "--symbol" {
            i += 1;
            if i >= args.len() {
                return Err(CliError::Args("Missing value for '--symbol'".to_string()));
            }
            symbol = Some(args[i].clone());
        } else if let Some(val) = arg.strip_prefix("-s=") {
            symbol = Some(val.to_string());
        } else if let Some(val) = arg.strip_prefix("--symbol=") {
            symbol = Some(val.to_string());
        } else if arg == "--callers" {
            i += 1;
            if i >= args.len() {
                return Err(CliError::Args("Missing value for '--callers'".to_string()));
            }
            callers = Some(args[i].clone());
        } else if let Some(val) = arg.strip_prefix("--callers=") {
            callers = Some(val.to_string());
        } else if arg == "--callees" {
            i += 1;
            if i >= args.len() {
                return Err(CliError::Args("Missing value for '--callees'".to_string()));
            }
            callees = Some(args[i].clone());
        } else if let Some(val) = arg.strip_prefix("--callees=") {
            callees = Some(val.to_string());
        } else if arg == "--effects" {
            i += 1;
            if i >= args.len() {
                return Err(CliError::Args("Missing value for '--effects'".to_string()));
            }
            effects = Some(args[i].clone());
        } else if let Some(val) = arg.strip_prefix("--effects=") {
            effects = Some(val.to_string());
        } else if arg.starts_with('-') {
            return Err(CliError::Args(format!("Unknown option for 'explore': {arg}")));
        } else if path.is_none() {
            path = Some(PathBuf::from(arg));
        } else {
            return Err(CliError::Args(format!(
                "Unexpected argument for 'explore': {arg}"
            )));
        }
        i += 1;
    }

    let path = path.ok_or_else(|| {
        CliError::Args(
            "Missing input path for 'knr explore'. Usage: knr explore <path> [options]".to_string(),
        )
    })?;

    let final_format = match (format, json) {
        (Some(ExploreFormat::Json), _) => ExploreFormat::Json,
        (Some(f), false) => f,
        (Some(f), true) => {
            if f != ExploreFormat::Json {
                return Err(CliError::Args(
                    "Cannot specify conflicting options '--json' and '--format'".to_string(),
                ));
            }
            ExploreFormat::Json
        }
        (None, true) => ExploreFormat::Json,
        (None, false) => ExploreFormat::Ai,
    };

    Ok(CliCommand::Explore(ExploreArgs {
        path,
        format: final_format,
        symbol,
        callers,
        callees,
        effects,
        orphans,
    }))
}

fn parse_explore_format(val: &str) -> Result<ExploreFormat, CliError> {
    match val.to_ascii_lowercase().as_str() {
        "ai" => Ok(ExploreFormat::Ai),
        "json" => Ok(ExploreFormat::Json),
        "mermaid" => Ok(ExploreFormat::Mermaid),
        other => Err(CliError::Args(format!(
            "Invalid format '{other}' for 'knr explore'. Expected 'ai', 'json', or 'mermaid'."
        ))),
    }
}

fn parse_gas_value(val: &str) -> Result<u64, CliError> {
    if val.eq_ignore_ascii_case("unlimited") || val.eq_ignore_ascii_case("max") {
        Ok(u64::MAX)
    } else {
        val.parse::<u64>()
            .map_err(|e| CliError::Args(format!("Invalid integer value for '--gas': {e}")))
    }
}

fn parse_run(args: &[String]) -> Result<CliCommand, CliError> {
    let mut file: Option<PathBuf> = None;
    let mut gas: u64 = 1_000_000;
    let mut trace = false;
    let mut profile = false;
    let mut watch = false;
    let mut once = false;
    let mut gui = false;

    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if arg == "--help" || arg == "-h" {
            println!(
                "knr run - Execute a .tsk, .knr, or .ssa module in the reference VM

USAGE:
    knr run <FILE> [OPTIONS]

ARGS:
    <FILE>    Path to source file (.tsk, .knr, or .ssa)

OPTIONS:
    --gas <N|unlimited>  Maximum execution steps limit (default: 1000000)
    --unlimited-gas      Disable execution step limits (alias for --gas unlimited)
    --trace              Output execution trace as structured JSON array
    --profile            Print deterministic VM profile (instructions, gas, allocs, top functions)
    -g, --gui            Launch native desktop GUI window with execution graph visualization
    -w, --watch          Watch project files and automatically re-run on modifications
    --once, --test-once  When running with --watch, execute one cycle and exit
    -h, --help           Print help information"
            );
            return Ok(CliCommand::Help);
        } else if arg == "--trace" {
            trace = true;
        } else if arg == "--profile" {
            profile = true;
        } else if arg == "--gui" || arg == "-g" {
            gui = true;
        } else if arg == "--watch" || arg == "-w" {
            watch = true;
        } else if arg == "--once" || arg == "--test-once" {
            once = true;
        } else if arg == "--unlimited-gas" {
            gas = u64::MAX;
        } else if arg == "--gas" {
            i += 1;
            if i >= args.len() {
                return Err(CliError::Args("Missing value for '--gas'".to_string()));
            }
            gas = parse_gas_value(&args[i])?;
        } else if let Some(val) = arg.strip_prefix("--gas=") {
            gas = parse_gas_value(val)?;
        } else if arg.starts_with('-') {
            return Err(CliError::Args(format!("Unknown option for 'run': {arg}")));
        } else if file.is_none() {
            file = Some(PathBuf::from(arg));
        } else {
            return Err(CliError::Args(format!(
                "Unexpected argument for 'run': {arg}"
            )));
        }
        i += 1;
    }

    let file = file.ok_or_else(|| {
        CliError::Args("Missing input file for 'knr run'. Usage: knr run <file.ssa> [--gas <N|unlimited>] [--trace] [--profile]".to_string())
    })?;

    Ok(CliCommand::Run(RunArgs { file, gas, trace, profile, watch, once, gui }))
}

fn parse_render(args: &[String]) -> Result<CliCommand, CliError> {
    let mut file: Option<PathBuf> = None;
    let mut output: Option<PathBuf> = None;
    let mut width: u32 = 1280;
    let mut height: u32 = 800;
    let mut scale: f32 = 2.0;
    let mut time: f32 = 1.2;
    let mut actions: Vec<String> = Vec::new();
    let mut steps: Vec<RenderStep> = Vec::new();
    let mut viewports: Vec<ViewportSpec> = Vec::new();
    let mut diff_out: Option<PathBuf> = None;
    let mut flow: Option<PathBuf> = None;
    let mut audit = false;
    let mut assert_audit = false;
    let mut baseline: Option<PathBuf> = None;
    let mut max_diff: f64 = 0.0;
    let mut json = false;
    let mut gas: u64 = 2_000_000;

    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if arg == "--help" || arg == "-h" {
            println!(
                "knr render - Render a headless HiDPI PNG snapshot, multi-viewport audit, visual diff & E2E flow runner

USAGE:
    knr render <FILE> [OPTIONS]

ARGS:
    <FILE>    Path to .tsk, .knr, or .ssa UI module

OPTIONS:
    -o, --output <OUT.png>  Save rendered HiDPI PNG snapshot to path
    --width <W>             Logical viewport width in pixels (default: 1280)
    --height <H>            Logical viewport height in pixels (default: 800)
    --scale <S>             HiDPI pixel scale factor (default: 2.0)
    --time <SEC>            Animation & shader evaluation time in seconds (default: 1.2)
    --viewports <LIST>      Comma-separated viewports (mobile, tablet, desktop, or WxH, e.g. mobile,desktop)
    --action <ACT>          Dispatch action via update(state, act) before rendering (repeatable)
    --click <KEY>           Simulate clicking widget with matching key= (or action) (repeatable)
    --input <KEY=TEXT>      Simulate typing TEXT into <Input key=KEY> (repeatable)
    --slider <KEY=0..1>     Simulate dragging <Slider key=KEY> to value in 0..1 (repeatable)
    --flow <FLOW.json>      Run a declarative JSON E2E UI journey with assertions & snapshots
    --audit                 Run automated visual layout, clipping, overlap, glyph & shader audit
    --assert-audit          Exit non-zero if visual audit reports any error
    --baseline <REF.png>    Compare rendered frame against reference PNG snapshot
    --diff-out <DIFF.png>   Write side-by-side magenta visual diff PNG when --baseline pixels differ
    --max-diff <PCT>        Max allowed differing pixel percentage vs --baseline (default: 0.0)
    --json                  Output structured JSON visual audit report
    --gas <N|unlimited>     Maximum VM execution steps (default: 2000000)
    -h, --help              Print help information"
            );
            return Ok(CliCommand::Help);
        } else if arg == "--audit" {
            audit = true;
        } else if arg == "--assert-audit" {
            audit = true;
            assert_audit = true;
        } else if arg == "--json" {
            json = true;
        } else if arg == "-o" || arg == "--output" || arg == "--png" {
            i += 1;
            if i >= args.len() {
                return Err(CliError::Args("Missing value for '-o' / '--output'".to_string()));
            }
            output = Some(PathBuf::from(&args[i]));
        } else if let Some(val) = arg.strip_prefix("-o=") {
            output = Some(PathBuf::from(val));
        } else if let Some(val) = arg.strip_prefix("--output=") {
            output = Some(PathBuf::from(val));
        } else if let Some(val) = arg.strip_prefix("--png=") {
            output = Some(PathBuf::from(val));
        } else if arg == "--viewports" {
            i += 1;
            if i >= args.len() {
                return Err(CliError::Args("Missing value for '--viewports'".to_string()));
            }
            viewports.extend(parse_viewports_list(&args[i])?);
        } else if let Some(val) = arg.strip_prefix("--viewports=") {
            viewports.extend(parse_viewports_list(val)?);
        } else if arg == "--diff-out" {
            i += 1;
            if i >= args.len() {
                return Err(CliError::Args("Missing value for '--diff-out'".to_string()));
            }
            diff_out = Some(PathBuf::from(&args[i]));
        } else if let Some(val) = arg.strip_prefix("--diff-out=") {
            diff_out = Some(PathBuf::from(val));
        } else if arg == "--flow" {
            i += 1;
            if i >= args.len() {
                return Err(CliError::Args("Missing value for '--flow'".to_string()));
            }
            flow = Some(PathBuf::from(&args[i]));
        } else if let Some(val) = arg.strip_prefix("--flow=") {
            flow = Some(PathBuf::from(val));
        } else if arg == "--baseline" {
            i += 1;
            if i >= args.len() {
                return Err(CliError::Args("Missing value for '--baseline'".to_string()));
            }
            baseline = Some(PathBuf::from(&args[i]));
        } else if let Some(val) = arg.strip_prefix("--baseline=") {
            baseline = Some(PathBuf::from(val));
        } else if arg == "--max-diff" {
            i += 1;
            if i >= args.len() {
                return Err(CliError::Args("Missing value for '--max-diff'".to_string()));
            }
            max_diff = args[i]
                .parse::<f64>()
                .map_err(|e| CliError::Args(format!("Invalid float for '--max-diff': {e}")))?;
        } else if let Some(val) = arg.strip_prefix("--max-diff=") {
            max_diff = val
                .parse::<f64>()
                .map_err(|e| CliError::Args(format!("Invalid float for '--max-diff': {e}")))?;
        } else if arg == "--width" {
            i += 1;
            if i >= args.len() {
                return Err(CliError::Args("Missing value for '--width'".to_string()));
            }
            width = args[i]
                .parse::<u32>()
                .map_err(|e| CliError::Args(format!("Invalid integer for '--width': {e}")))?;
        } else if let Some(val) = arg.strip_prefix("--width=") {
            width = val
                .parse::<u32>()
                .map_err(|e| CliError::Args(format!("Invalid integer for '--width': {e}")))?;
        } else if arg == "--height" {
            i += 1;
            if i >= args.len() {
                return Err(CliError::Args("Missing value for '--height'".to_string()));
            }
            height = args[i]
                .parse::<u32>()
                .map_err(|e| CliError::Args(format!("Invalid integer for '--height': {e}")))?;
        } else if let Some(val) = arg.strip_prefix("--height=") {
            height = val
                .parse::<u32>()
                .map_err(|e| CliError::Args(format!("Invalid integer for '--height': {e}")))?;
        } else if arg == "--scale" {
            i += 1;
            if i >= args.len() {
                return Err(CliError::Args("Missing value for '--scale'".to_string()));
            }
            scale = args[i]
                .parse::<f32>()
                .map_err(|e| CliError::Args(format!("Invalid float for '--scale': {e}")))?;
        } else if let Some(val) = arg.strip_prefix("--scale=") {
            scale = val
                .parse::<f32>()
                .map_err(|e| CliError::Args(format!("Invalid float for '--scale': {e}")))?;
        } else if arg == "--time" {
            i += 1;
            if i >= args.len() {
                return Err(CliError::Args("Missing value for '--time'".to_string()));
            }
            time = args[i]
                .parse::<f32>()
                .map_err(|e| CliError::Args(format!("Invalid float for '--time': {e}")))?;
        } else if let Some(val) = arg.strip_prefix("--time=") {
            time = val
                .parse::<f32>()
                .map_err(|e| CliError::Args(format!("Invalid float for '--time': {e}")))?;
        } else if arg == "--action" {
            i += 1;
            if i >= args.len() {
                return Err(CliError::Args("Missing value for '--action'".to_string()));
            }
            actions.push(args[i].clone());
            steps.push(RenderStep::Action(args[i].clone()));
        } else if let Some(val) = arg.strip_prefix("--action=") {
            actions.push(val.to_string());
            steps.push(RenderStep::Action(val.to_string()));
        } else if arg == "--click" {
            i += 1;
            if i >= args.len() {
                return Err(CliError::Args("Missing value for '--click'".to_string()));
            }
            steps.push(RenderStep::Click(args[i].clone()));
        } else if let Some(val) = arg.strip_prefix("--click=") {
            steps.push(RenderStep::Click(val.to_string()));
        } else if arg == "--input" {
            i += 1;
            if i >= args.len() {
                return Err(CliError::Args("Missing value for '--input'".to_string()));
            }
            let (k, v) = args[i].split_once('=').ok_or_else(|| {
                CliError::Args(format!(
                    "Invalid '--input' value '{}': expected KEY=TEXT",
                    args[i]
                ))
            })?;
            steps.push(RenderStep::Input {
                key: k.to_string(),
                value: v.to_string(),
            });
        } else if let Some(val) = arg.strip_prefix("--input=") {
            let (k, v) = val.split_once('=').ok_or_else(|| {
                CliError::Args(format!("Invalid '--input' value '{val}': expected KEY=TEXT"))
            })?;
            steps.push(RenderStep::Input {
                key: k.to_string(),
                value: v.to_string(),
            });
        } else if arg == "--slider" {
            i += 1;
            if i >= args.len() {
                return Err(CliError::Args("Missing value for '--slider'".to_string()));
            }
            let (k, v_str) = args[i].split_once('=').ok_or_else(|| {
                CliError::Args(format!(
                    "Invalid '--slider' value '{}': expected KEY=FLOAT",
                    args[i]
                ))
            })?;
            let v = v_str
                .parse::<f64>()
                .map_err(|e| CliError::Args(format!("Invalid float in '--slider': {e}")))?;
            steps.push(RenderStep::Slider {
                key: k.to_string(),
                value: v.clamp(0.0, 1.0),
            });
        } else if let Some(val) = arg.strip_prefix("--slider=") {
            let (k, v_str) = val.split_once('=').ok_or_else(|| {
                CliError::Args(format!("Invalid '--slider' value '{val}': expected KEY=FLOAT"))
            })?;
            let v = v_str
                .parse::<f64>()
                .map_err(|e| CliError::Args(format!("Invalid float in '--slider': {e}")))?;
            steps.push(RenderStep::Slider {
                key: k.to_string(),
                value: v.clamp(0.0, 1.0),
            });
        } else if arg == "--gas" {
            i += 1;
            if i >= args.len() {
                return Err(CliError::Args("Missing value for '--gas'".to_string()));
            }
            gas = parse_gas_value(&args[i])?;
        } else if let Some(val) = arg.strip_prefix("--gas=") {
            gas = parse_gas_value(val)?;
        } else if arg.starts_with('-') {
            return Err(CliError::Args(format!("Unknown option for 'render': {arg}")));
        } else if file.is_none() {
            file = Some(PathBuf::from(arg));
        } else {
            return Err(CliError::Args(format!(
                "Unexpected argument for 'render': {arg}"
            )));
        }
        i += 1;
    }

    let file = file.ok_or_else(|| {
        CliError::Args(
            "Missing input file for 'knr render'. Usage: knr render <file.tsk> [-o out.png] [--viewports mobile,desktop] [--flow flow.json] [--click KEY] [--input KEY=TEXT] [--slider KEY=0..1] [--audit] [--assert-audit] [--baseline REF.png] [--diff-out DIFF.png] [--json]"
                .to_string(),
        )
    })?;

    if width == 0 || width > 8192 {
        return Err(CliError::Args(format!(
            "Invalid '--width' value {width}: expected 1..=8192"
        )));
    }
    if height == 0 || height > 8192 {
        return Err(CliError::Args(format!(
            "Invalid '--height' value {height}: expected 1..=8192"
        )));
    }
    if !scale.is_finite() || scale <= 0.0 || scale > 4.0 {
        return Err(CliError::Args(format!(
            "Invalid '--scale' value {scale}: expected finite value in (0.0, 4.0]"
        )));
    }
    if !time.is_finite() || time < 0.0 {
        return Err(CliError::Args(format!(
            "Invalid '--time' value {time}: expected finite non-negative seconds"
        )));
    }
    if !max_diff.is_finite() || !(0.0..=100.0).contains(&max_diff) {
        return Err(CliError::Args(format!(
            "Invalid '--max-diff' value {max_diff}: expected percentage in 0.0..=100.0"
        )));
    }

    Ok(CliCommand::Render(RenderArgs {
        file,
        output,
        width,
        height,
        scale,
        time,
        actions,
        steps,
        viewports,
        diff_out,
        flow,
        audit,
        assert_audit,
        baseline,
        max_diff,
        json,
        gas,
    }))
}

fn parse_viewports_list(raw: &str) -> Result<Vec<ViewportSpec>, CliError> {
    let mut specs = Vec::new();
    for item in raw.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        let lower = item.to_ascii_lowercase();
        let spec = match lower.as_str() {
            "mobile" => ViewportSpec {
                name: "mobile".to_string(),
                width: 390,
                height: 844,
            },
            "tablet" => ViewportSpec {
                name: "tablet".to_string(),
                width: 768,
                height: 1024,
            },
            "desktop" => ViewportSpec {
                name: "desktop".to_string(),
                width: 1280,
                height: 900,
            },
            _ => {
                let (custom_name, dims) = match item.split_once('=') {
                    Some((n, d)) => (Some(n.trim()), d.trim()),
                    None => (None, item),
                };
                let (w_str, h_str) = dims
                    .split_once('x')
                    .or_else(|| dims.split_once('X'))
                    .ok_or_else(|| {
                        CliError::Args(format!(
                            "Invalid viewport '{item}': expected preset (mobile, tablet, desktop) or WxH (e.g. 390x844)"
                        ))
                    })?;
                let w = w_str.trim().parse::<u32>().map_err(|e| {
                    CliError::Args(format!("Invalid width in viewport '{item}': {e}"))
                })?;
                let h = h_str.trim().parse::<u32>().map_err(|e| {
                    CliError::Args(format!("Invalid height in viewport '{item}': {e}"))
                })?;
                if w == 0 || w > 8192 || h == 0 || h > 8192 {
                    return Err(CliError::Args(format!(
                        "Viewport '{item}' out of range: width and height must be in 1..=8192"
                    )));
                }
                ViewportSpec {
                    name: custom_name
                        .filter(|n| !n.is_empty())
                        .map(ToString::to_string)
                        .unwrap_or_else(|| format!("{w}x{h}")),
                    width: w,
                    height: h,
                }
            }
        };
        specs.push(spec);
    }
    if specs.is_empty() {
        return Err(CliError::Args(
            "Empty '--viewports' list: expected e.g. mobile,tablet,desktop or 390x844,1440x900"
                .to_string(),
        ));
    }
    Ok(specs)
}

fn parse_build(args: &[String]) -> Result<CliCommand, CliError> {
    let mut file: Option<PathBuf> = None;
    let mut target: Option<BuildTarget> = None;
    let mut output: Option<PathBuf> = None;
    let mut gui = false;
    let mut standalone = false;

    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if arg == "--help" || arg == "-h" {
            println!(
                "knr build - Compile a .tsk, .knr, or .ssa module to native executable, WebAssembly, or unified Canvas+WebGPU Web app

USAGE:
    knr build <FILE> --target <native|wasm|web> -o <OUTPUT> [OPTIONS]

ARGS:
    <FILE>    Path to .tsk, .knr, or .ssa source file

OPTIONS:
    --target <TARGET>    Compilation target: 'native', 'wasm', or 'web'
    -o, --output <PATH>  Output artifact file or directory path
    -g, --gui            Build standalone native desktop GUI executable (with --target native)
    --standalone         Embed base64-encoded WASM inline in index.html for offline file:// opening (with --target web)
    -h, --help           Print help information"
            );
            return Ok(CliCommand::Help);
        } else if arg == "--gui" || arg == "-g" {
            gui = true;
        } else if arg == "--standalone" {
            standalone = true;
        } else if arg == "--target" {
            i += 1;
            if i >= args.len() {
                return Err(CliError::Args("Missing value for '--target'".to_string()));
            }
            target = Some(parse_build_target(&args[i])?);
        } else if let Some(val) = arg.strip_prefix("--target=") {
            target = Some(parse_build_target(val)?);
        } else if arg == "-o" || arg == "--output" {
            i += 1;
            if i >= args.len() {
                return Err(CliError::Args("Missing value for output '-o'".to_string()));
            }
            output = Some(PathBuf::from(&args[i]));
        } else if let Some(val) = arg.strip_prefix("-o=") {
            output = Some(PathBuf::from(val));
        } else if let Some(val) = arg.strip_prefix("--output=") {
            output = Some(PathBuf::from(val));
        } else if arg.starts_with('-') {
            return Err(CliError::Args(format!("Unknown option for 'build': {arg}")));
        } else if file.is_none() {
            file = Some(PathBuf::from(arg));
        } else {
            return Err(CliError::Args(format!(
                "Unexpected argument for 'build': {arg}"
            )));
        }
        i += 1;
    }

    let file = file.ok_or_else(|| {
        CliError::Args("Missing input file for 'knr build'. Usage: knr build <file.knr> --target <native|wasm|web> -o <out> [--gui] [--standalone]".to_string())
    })?;

    let target = target.ok_or_else(|| {
        CliError::Args("Missing '--target' for 'knr build'. Supported targets: 'native', 'wasm', 'web'".to_string())
    })?;

    let output = output.ok_or_else(|| {
        CliError::Args("Missing output path '-o' for 'knr build'".to_string())
    })?;

    Ok(CliCommand::Build {
        file,
        target,
        output,
        gui,
        standalone,
    })
}

fn parse_build_target(target: &str) -> Result<BuildTarget, CliError> {
    match target {
        "native" => Ok(BuildTarget::Native),
        "wasm" => Ok(BuildTarget::Wasm),
        "web" => Ok(BuildTarget::Web),
        other => Err(CliError::Args(format!(
            "Invalid target '{other}' for 'knr build'. Expected 'native', 'wasm', or 'web'."
        ))),
    }
}

fn parse_test(args: &[String]) -> Result<CliCommand, CliError> {
    let mut path: Option<PathBuf> = None;
    let mut json = false;
    let mut gas: u64 = 1_000_000;

    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if arg == "--help" || arg == "-h" {
            println!(
                "knr test - Run deterministic test_*() functions in .tsk, .knr, or .ssa files

USAGE:
    knr test [PATH] [OPTIONS]

ARGS:
    [PATH]    File or directory to test (default: .)

OPTIONS:
    --json               Output structured JSON test report
    --gas <N|unlimited>  Maximum execution steps per test (default: 1000000)
    -h, --help           Print help information"
            );
            return Ok(CliCommand::Help);
        } else if arg == "--json" {
            json = true;
        } else if arg == "--gas" {
            i += 1;
            if i >= args.len() {
                return Err(CliError::Args("Missing value for '--gas'".to_string()));
            }
            gas = parse_gas_value(&args[i])?;
        } else if let Some(val) = arg.strip_prefix("--gas=") {
            gas = parse_gas_value(val)?;
        } else if arg.starts_with('-') {
            return Err(CliError::Args(format!("Unknown option for 'test': {arg}")));
        } else if path.is_none() {
            path = Some(PathBuf::from(arg));
        } else {
            return Err(CliError::Args(format!(
                "Unexpected argument for 'test': {arg}"
            )));
        }
        i += 1;
    }

    Ok(CliCommand::Test(TestArgs {
        path: path.unwrap_or_else(|| PathBuf::from(".")),
        json,
        gas,
    }))
}

fn parse_asm(args: &[String]) -> Result<CliCommand, CliError> {
    let mut file: Option<PathBuf> = None;
    let mut output: Option<PathBuf> = None;

    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if arg == "--help" || arg == "-h" {
            println!(
                "knr asm - Assemble textual SSA to binary bytecode (.knrbin)

USAGE:
    knr asm <FILE> -o <OUTPUT>

ARGS:
    <FILE>    Path to .ssa source file

OPTIONS:
    -o, --output <PATH>  Output .knrbin file path
    -h, --help           Print help information"
            );
            return Ok(CliCommand::Help);
        } else if arg == "-o" || arg == "--output" {
            i += 1;
            if i >= args.len() {
                return Err(CliError::Args("Missing value for output '-o'".to_string()));
            }
            output = Some(PathBuf::from(&args[i]));
        } else if let Some(val) = arg.strip_prefix("-o=") {
            output = Some(PathBuf::from(val));
        } else if let Some(val) = arg.strip_prefix("--output=") {
            output = Some(PathBuf::from(val));
        } else if arg.starts_with('-') {
            return Err(CliError::Args(format!("Unknown option for 'asm': {arg}")));
        } else if file.is_none() {
            file = Some(PathBuf::from(arg));
        } else {
            return Err(CliError::Args(format!(
                "Unexpected argument for 'asm': {arg}"
            )));
        }
        i += 1;
    }

    let file = file.ok_or_else(|| {
        CliError::Args("Missing input file for 'knr asm'. Usage: knr asm <file.ssa> -o <out.knrbin>".to_string())
    })?;

    let output = output.ok_or_else(|| {
        CliError::Args("Missing output file path '-o' for 'knr asm'".to_string())
    })?;

    Ok(CliCommand::Asm { file, output })
}

fn parse_disasm(args: &[String]) -> Result<CliCommand, CliError> {
    let mut file: Option<PathBuf> = None;
    let mut output: Option<PathBuf> = None;

    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if arg == "--help" || arg == "-h" {
            println!(
                "knr disasm - Disassemble binary bytecode (.knrbin) to canonical textual SSA

USAGE:
    knr disasm <FILE> [OPTIONS]

ARGS:
    <FILE>    Path to .knrbin file

OPTIONS:
    -o, --output <PATH>  Output .ssa file path (default: stdout)
    -h, --help           Print help information"
            );
            return Ok(CliCommand::Help);
        } else if arg == "-o" || arg == "--output" {
            i += 1;
            if i >= args.len() {
                return Err(CliError::Args("Missing value for output '-o'".to_string()));
            }
            output = Some(PathBuf::from(&args[i]));
        } else if let Some(val) = arg.strip_prefix("-o=") {
            output = Some(PathBuf::from(val));
        } else if let Some(val) = arg.strip_prefix("--output=") {
            output = Some(PathBuf::from(val));
        } else if arg.starts_with('-') {
            return Err(CliError::Args(format!("Unknown option for 'disasm': {arg}")));
        } else if file.is_none() {
            file = Some(PathBuf::from(arg));
        } else {
            return Err(CliError::Args(format!(
                "Unexpected argument for 'disasm': {arg}"
            )));
        }
        i += 1;
    }

    let file = file.ok_or_else(|| {
        CliError::Args("Missing input file for 'knr disasm'. Usage: knr disasm <file.knrbin> [-o <out.ssa>]".to_string())
    })?;

    Ok(CliCommand::Disasm { file, output })
}

fn parse_update(args: &[String]) -> Result<CliCommand, CliError> {
    let mut check = false;
    let mut force = false;
    let mut version: Option<String> = None;

    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if arg == "--help" || arg == "-h" {
            println!(
                "knr update - Check for updates and self-update from GitHub Releases

USAGE:
    knr update [OPTIONS]

OPTIONS:
    --check              Only check if a newer release is available without installing
    --force              Force update even if the current version matches
    --version <TAG>      Install a specific release tag (e.g. v0.2.0)
    -h, --help           Print help information"
            );
            return Ok(CliCommand::Help);
        } else if arg == "--check" || arg == "-c" {
            check = true;
        } else if arg == "--force" || arg == "-f" {
            force = true;
        } else if arg == "--version" || arg == "-v" {
            i += 1;
            if i >= args.len() {
                return Err(CliError::Args("Missing value for '--version'".to_string()));
            }
            version = Some(args[i].clone());
        } else if let Some(val) = arg.strip_prefix("--version=") {
            version = Some(val.to_string());
        } else if arg.starts_with('-') {
            return Err(CliError::Args(format!("Unknown option for 'update': {arg}")));
        } else {
            return Err(CliError::Args(format!(
                "Unexpected argument for 'update': {arg}"
            )));
        }
        i += 1;
    }

    Ok(CliCommand::Update {
        check,
        force,
        version,
    })
}

fn parse_skill(args: &[String]) -> Result<CliCommand, CliError> {
    let mut json = false;
    let mut output: Option<PathBuf> = None;
    let mut install_dir: Option<PathBuf> = None;

    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if arg == "--help" || arg == "-h" {
            println!(
                "knr skill - Output or install the official AI Agent SKILL.md and VDOM helper for knr

USAGE:
    knr skill [get|install] [OPTIONS]

SUBCOMMANDS / ACTIONS:
    get                  Print the official knr-ssa SKILL.md to stdout (default)
    install [DIR]        Install .agents/skills/knr-ssa/SKILL.md (+ scripts/) into DIR (default: .)

OPTIONS:
    --json               Output skill metadata, SKILL.md, and helper script as JSON
    --install [DIR]      Install .agents/skills/knr-ssa/SKILL.md (+ scripts/) into DIR (default: .)
    -o, --output <PATH>  Write output to file instead of stdout
    -h, --help           Print help information"
            );
            return Ok(CliCommand::Help);
        } else if arg == "--json" {
            json = true;
        } else if arg == "get" {
            // default action, no-op
        } else if arg == "install" || arg == "--install" {
            if i + 1 < args.len() && !args[i + 1].starts_with('-') {
                i += 1;
                install_dir = Some(PathBuf::from(&args[i]));
            } else {
                install_dir = Some(PathBuf::from("."));
            }
        } else if let Some(val) = arg.strip_prefix("--install=") {
            install_dir = Some(PathBuf::from(val));
        } else if arg == "-o" || arg == "--output" {
            i += 1;
            if i >= args.len() {
                return Err(CliError::Args("Missing value for output '-o'".to_string()));
            }
            output = Some(PathBuf::from(&args[i]));
        } else if let Some(val) = arg.strip_prefix("-o=") {
            output = Some(PathBuf::from(val));
        } else if let Some(val) = arg.strip_prefix("--output=") {
            output = Some(PathBuf::from(val));
        } else if arg.starts_with('-') {
            return Err(CliError::Args(format!("Unknown option for 'skill': {arg}")));
        } else {
            return Err(CliError::Args(format!(
                "Unexpected argument for 'skill': {arg}"
            )));
        }
        i += 1;
    }

    Ok(CliCommand::Skill(SkillArgs {
        json,
        output,
        install_dir,
    }))
}

fn parse_lsp(args: &[String]) -> Result<CliCommand, CliError> {
    if let Some(arg) = args.first() {
        if arg == "--help" || arg == "-h" {
            println!(
                "knr lsp - Run the built-in Language Server Protocol (JSON-RPC 2.0 over stdio)

USAGE:
    knr lsp [OPTIONS]

OPTIONS:
    -h, --help    Print help information"
            );
            return Ok(CliCommand::Help);
        } else {
            return Err(CliError::Args(format!("Unknown option for 'lsp': {arg}")));
        }
    }
    Ok(CliCommand::Lsp)
}

fn parse_dev(args: &[String]) -> Result<CliCommand, CliError> {
    let mut path: Option<PathBuf> = None;
    let mut debounce_ms: u64 = 150;
    let mut poll_ms: u64 = 200;
    let mut gas: u64 = 1_000_000;
    let mut trace = false;
    let mut once = false;
    let mut clear = true;
    let mut gui = false;

    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if arg == "--help" || arg == "-h" {
            println!(
                "knr dev - Watch project files and automatically re-verify/re-run on changes

USAGE:
    knr dev [PATH] [OPTIONS]

ARGS:
    [PATH]    Path to project directory, knr.toml manifest, or source file (default: .)

OPTIONS:
    -g, --gui            Launch live hot-reloading GUI + GPU shader window
    --debounce <MS>      Debounce quiet period in milliseconds (default: 150)
    --poll <MS>          Polling interval in milliseconds (default: 200)
    --gas <N|unlimited>  Maximum execution steps limit (default: 1000000)
    --trace              Output execution trace as structured JSON array
    --once, --test-once  Run a single cycle and exit
    --no-clear           Do not clear the terminal before re-executing
    -h, --help           Print help information"
            );
            return Ok(CliCommand::Help);
        } else if arg == "--gui" || arg == "-g" {
            gui = true;
        } else if arg == "--trace" {
            trace = true;
        } else if arg == "--once" || arg == "--test-once" {
            once = true;
        } else if arg == "--no-clear" {
            clear = false;
        } else if arg == "--clear" {
            clear = true;
        } else if arg == "--debounce" {
            i += 1;
            if i >= args.len() {
                return Err(CliError::Args("Missing value for '--debounce'".to_string()));
            }
            debounce_ms = args[i]
                .parse()
                .map_err(|e| CliError::Args(format!("Invalid integer for '--debounce': {e}")))?;
        } else if let Some(val) = arg.strip_prefix("--debounce=") {
            debounce_ms = val
                .parse()
                .map_err(|e| CliError::Args(format!("Invalid integer for '--debounce': {e}")))?;
        } else if arg == "--poll" {
            i += 1;
            if i >= args.len() {
                return Err(CliError::Args("Missing value for '--poll'".to_string()));
            }
            poll_ms = args[i]
                .parse()
                .map_err(|e| CliError::Args(format!("Invalid integer for '--poll': {e}")))?;
        } else if let Some(val) = arg.strip_prefix("--poll=") {
            poll_ms = val
                .parse()
                .map_err(|e| CliError::Args(format!("Invalid integer for '--poll': {e}")))?;
        } else if arg == "--gas" {
            i += 1;
            if i >= args.len() {
                return Err(CliError::Args("Missing value for '--gas'".to_string()));
            }
            gas = parse_gas_value(&args[i])?;
        } else if let Some(val) = arg.strip_prefix("--gas=") {
            gas = parse_gas_value(val)?;
        } else if arg.starts_with('-') {
            return Err(CliError::Args(format!("Unknown option for 'dev': {arg}")));
        } else if path.is_none() {
            path = Some(PathBuf::from(arg));
        } else {
            return Err(CliError::Args(format!("Unexpected argument for 'dev': {arg}")));
        }
        i += 1;
    }

    Ok(CliCommand::Dev(DevArgs {
        path,
        debounce_ms,
        poll_ms,
        once,
        gas,
        trace,
        clear,
        gui,
    }))
}

fn parse_init(args: &[String]) -> Result<CliCommand, CliError> {
    let mut dir: Option<PathBuf> = None;
    let mut template = "minimal".to_string();
    let mut name: Option<String> = None;
    let mut workspace = false;
    let mut force = false;

    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if arg == "--help" || arg == "-h" {
            println!(
                "knr init - Initialize a knr project in an existing directory

USAGE:
    knr init [DIR] [OPTIONS]

ARGS:
    [DIR]    Target directory (default: current directory)

OPTIONS:
    --template <minimal|app|ui|dashboard|shader|service|lib>  Starter template (default: minimal)
    --name <NAME>                                             Explicit package name
    --workspace                                               Initialize a multi-project workspace
    --force                                                   Overwrite existing files if already present
    -h, --help                                                Print help information"
            );
            return Ok(CliCommand::Help);
        } else if arg == "--workspace" {
            workspace = true;
        } else if arg == "--force" {
            force = true;
        } else if arg == "--name" {
            i += 1;
            if i >= args.len() {
                return Err(CliError::Args("Missing value for '--name'".to_string()));
            }
            name = Some(args[i].clone());
        } else if let Some(val) = arg.strip_prefix("--name=") {
            name = Some(val.to_string());
        } else if arg == "--template" {
            i += 1;
            if i >= args.len() {
                return Err(CliError::Args("Missing value for '--template'".to_string()));
            }
            template = args[i].clone();
        } else if let Some(val) = arg.strip_prefix("--template=") {
            template = val.to_string();
        } else if arg.starts_with('-') {
            return Err(CliError::Args(format!("Unknown option for 'init': {arg}")));
        } else if dir.is_none() {
            dir = Some(PathBuf::from(arg));
        } else {
            return Err(CliError::Args(format!("Unexpected argument for 'init': {arg}")));
        }
        i += 1;
    }

    Ok(CliCommand::Init(InitArgs {
        dir,
        template,
        name,
        workspace,
        force,
    }))
}

fn parse_new(args: &[String]) -> Result<CliCommand, CliError> {
    let mut path: Option<PathBuf> = None;
    let mut template = "minimal".to_string();
    let mut name: Option<String> = None;
    let mut workspace = false;
    let mut force = false;

    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if arg == "--help" || arg == "-h" {
            println!(
                "knr new - Create a new knr project or workspace directory

USAGE:
    knr new <PATH> [OPTIONS]

ARGS:
    <PATH>    Path of the new directory to create

OPTIONS:
    --template <minimal|app|ui|dashboard|shader|service|lib>  Starter template (default: minimal)
    --name <NAME>                                             Explicit package name
    --workspace                                               Create as a multi-project workspace
    --force                                                   Allow existing non-empty directory
    -h, --help                                                Print help information"
            );
            return Ok(CliCommand::Help);
        } else if arg == "--workspace" {
            workspace = true;
        } else if arg == "--force" {
            force = true;
        } else if arg == "--name" {
            i += 1;
            if i >= args.len() {
                return Err(CliError::Args("Missing value for '--name'".to_string()));
            }
            name = Some(args[i].clone());
        } else if let Some(val) = arg.strip_prefix("--name=") {
            name = Some(val.to_string());
        } else if arg == "--template" {
            i += 1;
            if i >= args.len() {
                return Err(CliError::Args("Missing value for '--template'".to_string()));
            }
            template = args[i].clone();
        } else if let Some(val) = arg.strip_prefix("--template=") {
            template = val.to_string();
        } else if arg.starts_with('-') {
            return Err(CliError::Args(format!("Unknown option for 'new': {arg}")));
        } else if path.is_none() {
            path = Some(PathBuf::from(arg));
        } else {
            return Err(CliError::Args(format!("Unexpected argument for 'new': {arg}")));
        }
        i += 1;
    }

    let path = path.ok_or_else(|| {
        CliError::Args(
            "Missing path argument for 'knr new'. Usage: knr new <path> [options]".to_string(),
        )
    })?;

    Ok(CliCommand::New(NewArgs {
        path,
        workspace,
        template,
        name,
        force,
    }))
}

fn parse_fmt(args: &[String]) -> Result<CliCommand, CliError> {
    let mut paths = Vec::new();
    let mut check = false;

    for arg in args {
        if arg == "--help" || arg == "-h" {
            println!(
                "knr fmt - Canonically format .tsk / .knr source files

USAGE:
    knr fmt [PATHS...] [OPTIONS]

ARGS:
    [PATHS...]    Files or directories to format (default: current directory)

OPTIONS:
    --check       Check formatting without modifying files (exits non-zero if unformatted)
    -h, --help    Print help information"
            );
            return Ok(CliCommand::Help);
        } else if arg == "--check" {
            check = true;
        } else if arg.starts_with('-') {
            return Err(CliError::Args(format!("Unknown option for 'fmt': {arg}")));
        } else {
            paths.push(PathBuf::from(arg));
        }
    }

    Ok(CliCommand::Fmt(FmtArgs { paths, check }))
}

fn parse_fix(args: &[String]) -> Result<CliCommand, CliError> {
    let mut file: Option<PathBuf> = None;
    let mut write = false;
    let mut json = false;

    for arg in args {
        if arg == "--help" || arg == "-h" {
            println!(
                "knr fix - Deterministically propose or apply compiler-verified QuickFixes

USAGE:
    knr fix <FILE> [OPTIONS]

ARGS:
    <FILE>    Path to .tsk or .knr source file

OPTIONS:
    -w, --write   Apply verified QuickFixes directly to the file on disk
    --json        Output structured JSON report of fixes and patched source
    -h, --help    Print help information"
            );
            return Ok(CliCommand::Help);
        } else if arg == "--write" || arg == "-w" {
            write = true;
        } else if arg == "--json" {
            json = true;
        } else if arg.starts_with('-') {
            return Err(CliError::Args(format!("Unknown option for 'fix': {arg}")));
        } else if file.is_none() {
            file = Some(PathBuf::from(arg));
        } else {
            return Err(CliError::Args(format!(
                "Unexpected argument for 'fix': {arg}"
            )));
        }
    }

    let file = file.ok_or_else(|| {
        CliError::Args("Missing input file for 'knr fix'. Usage: knr fix <file.tsk> [--write] [--json]".to_string())
    })?;

    Ok(CliCommand::Fix(FixArgs { file, write, json }))
}

fn parse_mcp(args: &[String]) -> Result<CliCommand, CliError> {
    if let Some(arg) = args.first() {
        if arg == "--help" || arg == "-h" {
            println!(
                "knr mcp - Run the built-in Model Context Protocol (MCP 2024-11-05) JSON-RPC 2.0 server over stdio

USAGE:
    knr mcp [OPTIONS]

TOOLS EXPOSED:
    knr_check         Statically verify .tsk/.knr/.ssa files & validate @shader WGSL/GLSL
    knr_explore       Query ProjectGraph schemas, call graph, effects, and orphans
    knr_test          Run deterministic test_*() suites
    knr_render_audit  Render headless HiDPI PNG, simulate clicks/inputs/sliders & run visual audit
    knr_fmt           Canonically format .tsk/.knr source files
    knr_fix           Propose or apply compiler-verified QuickFixes
    knr_bench         Benchmark VM gas/steps, Cranelift JIT speed & artifact sizes
    knr_get_skill     Return the official knr AI Agent SKILL.md reference

OPTIONS:
    -h, --help    Print help information"
            );
            return Ok(CliCommand::Help);
        }
        return Err(CliError::Args(format!("Unknown option for 'mcp': {arg}")));
    }
    Ok(CliCommand::Mcp)
}

fn parse_doctor(args: &[String]) -> Result<CliCommand, CliError> {
    let mut json = false;
    let mut dir: Option<PathBuf> = None;

    for arg in args {
        if arg == "--help" || arg == "-h" {
            println!(
                "knr doctor - Check toolchain environment, native linker, runtime, display server & project health

USAGE:
    knr doctor [DIR] [OPTIONS]

ARGS:
    [DIR]         Project directory to inspect for knr.toml and .tsk files (default: .)

OPTIONS:
    --json        Output structured JSON health report
    -h, --help    Print help information"
            );
            return Ok(CliCommand::Help);
        } else if arg == "--json" {
            json = true;
        } else if arg.starts_with('-') {
            return Err(CliError::Args(format!(
                "Unknown option for 'doctor': {arg}"
            )));
        } else if dir.is_none() {
            dir = Some(PathBuf::from(arg));
        } else {
            return Err(CliError::Args(format!(
                "Unexpected argument for 'doctor': {arg}"
            )));
        }
    }

    Ok(CliCommand::Doctor(DoctorArgs { json, dir }))
}

fn parse_bench(args: &[String]) -> Result<CliCommand, CliError> {
    let mut file: Option<PathBuf> = None;
    let mut fn_name: Option<String> = None;
    let mut iters: usize = 20;
    let mut json = false;
    let mut gas: u64 = 2_000_000;

    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if arg == "--help" || arg == "-h" {
            println!(
                "knr bench - Deterministic VM, Cranelift JIT & bytecode size benchmark

USAGE:
    knr bench <FILE> [OPTIONS]

ARGS:
    <FILE>    Path to .tsk, .knr, or .ssa file

OPTIONS:
    --fn <NAME>          Target function name (default: main, view(init()), or init)
    --iters <N>          Number of measured iterations (default: 20)
    --gas <N|unlimited>  Maximum VM steps per iteration (default: 2000000)
    --json               Output structured JSON benchmark report
    -h, --help           Print help information"
            );
            return Ok(CliCommand::Help);
        } else if arg == "--json" {
            json = true;
        } else if arg == "--fn" {
            i += 1;
            if i >= args.len() {
                return Err(CliError::Args("Missing value for '--fn'".to_string()));
            }
            fn_name = Some(args[i].clone());
        } else if let Some(val) = arg.strip_prefix("--fn=") {
            fn_name = Some(val.to_string());
        } else if arg == "--iters" {
            i += 1;
            if i >= args.len() {
                return Err(CliError::Args("Missing value for '--iters'".to_string()));
            }
            iters = args[i]
                .parse::<usize>()
                .map_err(|e| CliError::Args(format!("Invalid integer for '--iters': {e}")))?;
        } else if let Some(val) = arg.strip_prefix("--iters=") {
            iters = val
                .parse::<usize>()
                .map_err(|e| CliError::Args(format!("Invalid integer for '--iters': {e}")))?;
        } else if arg == "--gas" {
            i += 1;
            if i >= args.len() {
                return Err(CliError::Args("Missing value for '--gas'".to_string()));
            }
            gas = parse_gas_value(&args[i])?;
        } else if let Some(val) = arg.strip_prefix("--gas=") {
            gas = parse_gas_value(val)?;
        } else if arg.starts_with('-') {
            return Err(CliError::Args(format!(
                "Unknown option for 'bench': {arg}"
            )));
        } else if file.is_none() {
            file = Some(PathBuf::from(arg));
        } else {
            return Err(CliError::Args(format!(
                "Unexpected argument for 'bench': {arg}"
            )));
        }
        i += 1;
    }

    let file = file.ok_or_else(|| {
        CliError::Args(
            "Missing input file for 'knr bench'. Usage: knr bench <file.tsk> [--fn <name>] [--iters N] [--json]"
                .to_string(),
        )
    })?;

    if iters == 0 || iters > 100_000 {
        return Err(CliError::Args(format!(
            "Invalid '--iters' value {iters}: expected 1..=100000"
        )));
    }

    Ok(CliCommand::Bench(BenchArgs {
        file,
        fn_name,
        iters,
        json,
        gas,
    }))
}
