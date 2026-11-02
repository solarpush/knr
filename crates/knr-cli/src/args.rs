use std::path::PathBuf;

use crate::error::CliError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuildTarget {
    Native,
    Wasm,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CliCommand {
    Check {
        file: PathBuf,
        json: bool,
    },
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
    Help,
    Version,
}

pub fn print_main_help() {
    println!(
        "knr 0.1.0 - Knr SSA compiler, virtual machine, and bytecode toolchain

USAGE:
    knr <SUBCOMMAND> [OPTIONS]

SUBCOMMANDS:
    check     Statically verify an SSA module (.ssa)
    run       Execute an SSA module in the reference interpreter
    build     Compile an SSA module to native executable or WebAssembly
    asm       Assemble textual SSA (.ssa) to compact binary bytecode (.knrbin)
    disasm    Disassemble binary bytecode (.knrbin) to canonical textual SSA (.ssa)

FLAGS:
    -h, --help       Print help information
    -V, --version    Print version information"
    );
}

pub fn print_version() {
    println!("knr 0.1.0");
}

pub fn parse_args<I: IntoIterator<Item = String>>(args: I) -> Result<CliCommand, CliError> {
    let args: Vec<String> = args.into_iter().collect();
    if args.len() <= 1 {
        return Err(CliError::Args(
            "Usage: knr <check|run|build|asm|disasm> [options]\nUse 'knr --help' for usage."
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
        cmd => Err(CliError::Args(format!(
            "Unknown subcommand: '{cmd}'. Use 'knr --help' for usage."
        ))),
    }
}

fn parse_check(args: &[String]) -> Result<CliCommand, CliError> {
    let mut file: Option<PathBuf> = None;
    let mut json = false;

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
    --json        Output diagnostics as structured JSON array
    -h, --help    Print help information"
            );
            return Ok(CliCommand::Help);
        } else if arg == "--json" {
            json = true;
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
        CliError::Args("Missing input file for 'knr check'. Usage: knr check <file.ssa> [--json]".to_string())
    })?;

    Ok(CliCommand::Check { file, json })
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
    --gas <N>     Maximum execution steps limit (default: 1000000)
    --trace       Output execution trace as structured JSON array
    -h, --help    Print help information"
            );
            return Ok(CliCommand::Help);
        } else if arg == "--trace" {
            trace = true;
        } else if arg == "--gas" {
            i += 1;
            if i >= args.len() {
                return Err(CliError::Args("Missing value for '--gas'".to_string()));
            }
            gas = args[i].parse::<u64>().map_err(|e| {
                CliError::Args(format!("Invalid integer value for '--gas': {e}"))
            })?;
        } else if let Some(val) = arg.strip_prefix("--gas=") {
            gas = val
                .parse::<u64>()
                .map_err(|e| CliError::Args(format!("Invalid integer value for '--gas': {e}")))?;
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
        CliError::Args("Missing input file for 'knr run'. Usage: knr run <file.ssa> [--gas <N>] [--trace]".to_string())
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
