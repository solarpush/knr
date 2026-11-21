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

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CliCommand {
    Check(CheckArgs),
    Run(RunArgs),
    Build {
        file: PathBuf,
        target: BuildTarget,
        output: PathBuf,
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
    Skill(SkillArgs),
    Dev(DevArgs),
    Test(TestArgs),
    Lsp,
    Init(InitArgs),
    New(NewArgs),
    Help,
    Version,
}

pub type Command = CliCommand;

pub fn print_main_help() {
    println!(
        "knr {VERSION} - Knr SSA compiler, virtual machine, and bytecode toolchain

USAGE:
    knr <SUBCOMMAND> [OPTIONS]

SUBCOMMANDS:
    check     Statically verify a .knr or .ssa module
    run       Execute a .knr or .ssa module in the reference interpreter (or --gui desktop app)
    test      Discover and run deterministic fn test_*() suites in .knr/.ssa files
    build     Compile a module to native executable, WebAssembly (.wasm), or standalone Web app
    asm       Assemble textual SSA (.ssa) to compact binary bytecode (.knrbin)
    disasm    Disassemble binary bytecode (.knrbin) to canonical textual SSA (.ssa)
    update    Check for updates and self-update from GitHub Releases
    explore   Explore project structure, call graphs, schemas, and dependencies
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
            "Usage: knr <check|run|build|asm|disasm|update|explore|skill|dev|init|new> [options]\nUse 'knr --help' for usage."
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
        "test" => parse_test(&args[2..]),
        "build" => parse_build(&args[2..]),
        "asm" => parse_asm(&args[2..]),
        "disasm" => parse_disasm(&args[2..]),
        "update" => parse_update(&args[2..]),
        "explore" => parse_explore(&args[2..]),
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
                "knr check - Statically verify an SSA module

USAGE:
    knr check <FILE> [OPTIONS]

ARGS:
    <FILE>    Path to .ssa source file

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
    let mut watch = false;
    let mut once = false;
    let mut gui = false;

    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if arg == "--help" || arg == "-h" {
            println!(
                "knr run - Execute an SSA module in the reference VM

USAGE:
    knr run <FILE> [OPTIONS]

ARGS:
    <FILE>    Path to source file (.ssa or .knr)

OPTIONS:
    --gas <N|unlimited>  Maximum execution steps limit (default: 1000000)
    --unlimited-gas      Disable execution step limits (alias for --gas unlimited)
    --trace              Output execution trace as structured JSON array
    -g, --gui            Launch native desktop GUI window with execution graph visualization
    -w, --watch          Watch project files and automatically re-run on modifications
    --once, --test-once  When running with --watch, execute one cycle and exit
    -h, --help           Print help information"
            );
            return Ok(CliCommand::Help);
        } else if arg == "--trace" {
            trace = true;
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
        CliError::Args("Missing input file for 'knr run'. Usage: knr run <file.ssa> [--gas <N|unlimited>] [--trace]".to_string())
    })?;

    Ok(CliCommand::Run(RunArgs { file, gas, trace, watch, once, gui }))
}

fn parse_build(args: &[String]) -> Result<CliCommand, CliError> {
    let mut file: Option<PathBuf> = None;
    let mut target: Option<BuildTarget> = None;
    let mut output: Option<PathBuf> = None;

    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if arg == "--help" || arg == "-h" {
            println!(
                "knr build - Compile an SSA module to native executable or WebAssembly

USAGE:
    knr build <FILE> --target <native|wasm> -o <OUTPUT>

ARGS:
    <FILE>    Path to .ssa source file

OPTIONS:
    --target <TARGET>    Compilation target: 'native' or 'wasm'
    -o, --output <PATH>  Output artifact file path
    -h, --help           Print help information"
            );
            return Ok(CliCommand::Help);
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
        CliError::Args("Missing input file for 'knr build'. Usage: knr build <file.ssa> --target <native|wasm> -o <out>".to_string())
    })?;

    let target = target.ok_or_else(|| {
        CliError::Args("Missing '--target' for 'knr build'. Supported targets: 'native', 'wasm'".to_string())
    })?;

    let output = output.ok_or_else(|| {
        CliError::Args("Missing output path '-o' for 'knr build'".to_string())
    })?;

    Ok(CliCommand::Build {
        file,
        target,
        output,
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
                "knr test - Run deterministic fn test_*() functions in .knr or .ssa files

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
    --template <minimal|app|ui|service|lib>  Starter template (default: minimal)
    --name <NAME>                           Explicit package name
    --workspace                             Initialize a multi-project workspace
    --force                                 Overwrite existing files if already present
    -h, --help                              Print help information"
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
    --template <minimal|app|ui|service|lib>  Starter template (default: minimal)
    --name <NAME>                           Explicit package name
    --workspace                             Create as a multi-project workspace
    --force                                 Allow existing non-empty directory
    -h, --help                              Print help information"
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
