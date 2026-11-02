use std::path::PathBuf;

use crate::error::CliError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuildTarget {
    Native,
    Wasm,
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

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CliCommand {
    Check(CheckArgs),
    Run {
        file: PathBuf,
        gas: u64,
        trace: bool,
    },
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
    check     Statically verify an SSA module (.ssa)
    run       Execute an SSA module in the reference interpreter
    build     Compile an SSA module to native executable or WebAssembly
    asm       Assemble textual SSA (.ssa) to compact binary bytecode (.knrbin)
    disasm    Disassemble binary bytecode (.knrbin) to canonical textual SSA (.ssa)
    update    Check for updates and self-update from GitHub Releases
    explore   Explore project structure, call graphs, schemas, and dependencies

FLAGS:
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
            "Usage: knr <check|run|build|asm|disasm|update|explore> [options]\nUse 'knr --help' for usage."
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
        "build" => parse_build(&args[2..]),
        "asm" => parse_asm(&args[2..]),
        "disasm" => parse_disasm(&args[2..]),
        "update" => parse_update(&args[2..]),
        "explore" => parse_explore(&args[2..]),
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

    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if arg == "--help" || arg == "-h" {
            println!(
                "knr run - Execute an SSA module in the reference VM

USAGE:
    knr run <FILE> [OPTIONS]

ARGS:
    <FILE>    Path to .ssa source file

OPTIONS:
    --gas <N|unlimited>  Maximum execution steps limit (default: 1000000)
    --unlimited-gas      Disable execution step limits (alias for --gas unlimited)
    --trace              Output execution trace as structured JSON array
    -h, --help           Print help information"
            );
            return Ok(CliCommand::Help);
        } else if arg == "--trace" {
            trace = true;
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

    Ok(CliCommand::Run { file, gas, trace })
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
        other => Err(CliError::Args(format!(
            "Invalid target '{other}' for 'knr build'. Expected 'native' or 'wasm'."
        ))),
    }
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
