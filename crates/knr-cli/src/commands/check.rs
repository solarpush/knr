use std::path::Path;

use knr_core::{ExploreFilter, ParseError};
use knr_verify::Diagnostic;

use crate::error::CliError;

pub fn execute(file: &Path, json: bool, graph: bool) -> Result<(), CliError> {
    let module = match knr_core::load_module_from_path(file) {
        Ok(m) => m,
        Err(err) => {
            let (error_kind, line, fix_hint) = parse_error_details(&err);
            let diag = Diagnostic::new(error_kind, "<parse>", line, err.to_string())
                .with_fix_hint(fix_hint);
            if json {
                let json_output = serde_json::to_string_pretty(&vec![diag])
                    .map_err(|e| CliError::Other(e.to_string()))?;
                println!("{json_output}");
            } else {
                eprintln!("{diag}");
            }
            return Err(CliError::Parse(err));
        }
    };

    match knr_verify::verify_module(&module) {
        Ok(()) => {
            if graph {
                let project_graph = knr_core::explore_path(file).map_err(|err| {
                    eprintln!("{err}");
                    CliError::Parse(err)
                })?;
                let filter = ExploreFilter::default();
                let output = if json {
                    project_graph.format_json(&filter)
                } else {
                    project_graph.format_ai(&filter)
                };
                if output.ends_with('\n') {
                    print!("{output}");
                } else {
                    println!("{output}");
                }
            } else if json {
                println!("[]");
            }
            Ok(())
        }
        Err(diagnostics) => {
            if json {
                let json_output = serde_json::to_string_pretty(&diagnostics)
                    .map_err(|e| CliError::Other(e.to_string()))?;
                println!("{json_output}");
            } else {
                for diag in &diagnostics {
                    eprintln!("{diag}");
                }
            }
            Err(CliError::Verify(diagnostics.len()))
        }
    }
}

fn parse_error_details(err: &ParseError) -> (&'static str, usize, String) {
    match err {
        ParseError::UnexpectedEof { line } => (
            "UnexpectedEof",
            *line,
            "Ensure all blocks, braces, and instructions are fully completed".to_string(),
        ),
        ParseError::InvalidSyntax { line, msg } => (
            "InvalidSyntax",
            *line,
            format!("Fix syntax error: {msg}"),
        ),
        ParseError::UnknownOpcode { line, opcode } => (
            "UnknownOpcode",
            *line,
            format!("Check spelling of opcode '{opcode}'; verify it is a valid knr opcode"),
        ),
        ParseError::InvalidRegister { line, token } => (
            "InvalidRegister",
            *line,
            format!("Register token '{token}' must be in the format %<digits> (e.g., %0, %1)"),
        ),
        ParseError::InvalidBlockLabel { line, token } => (
            "InvalidBlockLabel",
            *line,
            format!("Block label '{token}' must be in the format bb<digits>: (e.g., bb0:, bb1:)"),
        ),
        ParseError::InvalidType { line, token } => (
            "InvalidType",
            *line,
            format!("Type '{token}' is invalid; use i64, f64, bool, str, buf, vec[T], map[K, V], chan[T], or struct[Name]"),
        ),
        ParseError::InvalidEffect { line, token } => (
            "InvalidEffect",
            *line,
            format!("Effect '{token}' is invalid; declare Net, Ui, Hw, or Async"),
        ),
        ParseError::UnterminatedString { line } => (
            "UnterminatedString",
            *line,
            "Add closing quotation mark '\"'".to_string(),
        ),
        ParseError::InvalidLiteral { line, token: _, expected } => (
            "InvalidLiteral",
            *line,
            format!("Provide a valid literal: {expected}"),
        ),
        ParseError::MissingTerminator { line, block_id } => (
            "MissingTerminator",
            *line,
            format!("Add a terminator instruction (ret, br, or br_if) at the end of block bb{block_id}"),
        ),
        ParseError::MissingClosingBrace { line } => (
            "MissingClosingBrace",
            *line,
            "Add closing brace '}'".to_string(),
        ),
        ParseError::InlineSchemaInCodeFile { line, name: _, fix_hint } => (
            "InlineSchemaInCodeFile",
            *line,
            fix_hint.clone(),
        ),
        ParseError::InvalidConstructInSchemaFile { line, msg } => (
            "InvalidConstructInSchemaFile",
            *line,
            format!("Schema files (.ssa.schema) may only contain 'import' and 'schema' declarations; {msg}"),
        ),
        ParseError::ImportAfterDeclaration { line } => (
            "ImportAfterDeclaration",
            *line,
            "Move all 'import' directives to the top of the file before any schema or fn declarations".to_string(),
        ),
        ParseError::CircularImport { cycle } => (
            "CircularImport",
            1,
            format!("Break circular dependency cycle: {}", cycle.join(" -> ")),
        ),
        ParseError::DuplicateSymbol { symbol, first_file, second_file } => (
            "DuplicateSymbol",
            1,
            format!("Rename or remove duplicate symbol '{symbol}' defined in both '{first_file}' and '{second_file}'"),
        ),
        ParseError::FileNotFound { line, path } => (
            "FileNotFound",
            *line,
            format!("Check that file '{path}' exists and the path is correct"),
        ),
        ParseError::IoError { path, msg } => (
            "IoError",
            1,
            format!("Verify file '{path}' permissions and accessibility: {msg}"),
        ),
    }
}
