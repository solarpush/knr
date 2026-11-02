use std::fs;
use std::path::Path;

use knr_core::ParseError;
use knr_verify::Diagnostic;

use crate::error::CliError;

pub fn execute(file: &Path, json: bool) -> Result<(), CliError> {
    let content = fs::read_to_string(file).map_err(|e| {
        eprintln!("Error reading file '{}': {e}", file.display());
        CliError::Io(e)
    })?;

    let module = match knr_core::parse_ssa(&content) {
        Ok(m) => m,
        Err(err) => {
            if json {
                let (error_kind, line) = match &err {
                    ParseError::UnterminatedString { line } => ("UnterminatedString", *line),
                    ParseError::InvalidRegister { line, .. } => ("InvalidRegister", *line),
                    ParseError::InvalidBlockLabel { line, .. } => ("InvalidBlockLabel", *line),
                    ParseError::MissingTerminator { line, .. } => ("MissingTerminator", *line),
                    ParseError::InvalidSyntax { line, .. } => ("InvalidSyntax", *line),
                    ParseError::UnexpectedEof { line } => ("UnexpectedEof", *line),
                    ParseError::UnknownOpcode { line, .. } => ("UnknownOpcode", *line),
                    ParseError::InvalidType { line, .. } => ("InvalidType", *line),
                    ParseError::InvalidEffect { line, .. } => ("InvalidEffect", *line),
                    ParseError::InvalidLiteral { line, .. } => ("InvalidLiteral", *line),
                    ParseError::MissingClosingBrace { line } => ("MissingClosingBrace", *line),
                };
                let diag = Diagnostic::new(error_kind, "<parse>", line, err.to_string());
                let json_output = serde_json::to_string_pretty(&vec![diag])
                    .map_err(|e| CliError::Other(e.to_string()))?;
                println!("{json_output}");
            } else {
                eprintln!("{err}");
            }
            return Err(CliError::Parse(err));
        }
    };

    match knr_verify::verify_module(&module) {
        Ok(()) => {
            if json {
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
