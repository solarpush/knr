use std::fs;
use std::path::{Path, PathBuf};

use crate::args::FmtArgs;
use crate::error::CliError;

/// Executes `knr fmt`: canonically formats `.tsk` / `.knr` source files using `knr_core::format_tsk`.
pub fn execute(args: &FmtArgs) -> Result<(), CliError> {
    let mut files = Vec::new();
    let targets = if args.paths.is_empty() {
        vec![PathBuf::from(".")]
    } else {
        args.paths.clone()
    };

    for target in &targets {
        collect_source_files(target, &mut files)?;
    }
    files.sort();
    files.dedup();

    if files.is_empty() {
        return Err(CliError::Other(
            "No .tsk or .knr source files found to format".to_string(),
        ));
    }

    let mut unformatted = Vec::new();
    let mut formatted_count = 0usize;

    for file in &files {
        let orig = fs::read_to_string(file).map_err(|e| {
            eprintln!("Error reading '{}': {e}", file.display());
            CliError::Io(e)
        })?;
        let formatted = knr_core::format_tsk(&orig).map_err(|err| {
            eprintln!("{}: {err}", file.display());
            CliError::Parse(err)
        })?;

        if formatted != orig {
            if args.check {
                unformatted.push(file.clone());
            } else {
                fs::write(file, &formatted).map_err(|e| {
                    eprintln!("Error writing '{}': {e}", file.display());
                    CliError::Io(e)
                })?;
                formatted_count += 1;
                println!("Formatted {}", file.display());
            }
        }
    }

    if args.check {
        if !unformatted.is_empty() {
            for f in &unformatted {
                eprintln!("Needs formatting: {}", f.display());
            }
            return Err(CliError::Other(format!(
                "{} file(s) not formatted canonically (run `knr fmt`)",
                unformatted.len()
            )));
        }
        println!("All {} file(s) are canonically formatted.", files.len());
    } else if formatted_count == 0 {
        println!("All {} file(s) already formatted.", files.len());
    }

    Ok(())
}

fn collect_source_files(path: &Path, out: &mut Vec<PathBuf>) -> Result<(), CliError> {
    if !path.exists() {
        return Err(CliError::Other(format!(
            "Path '{}' does not exist",
            path.display()
        )));
    }
    if path.is_file() {
        if knr_core::is_source_file(path) {
            out.push(path.to_path_buf());
        }
        return Ok(());
    }
    if path.is_dir() {
        let entries = fs::read_dir(path).map_err(CliError::Io)?;
        for entry in entries.flatten() {
            let child = entry.path();
            let name = child.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name.starts_with('.')
                || name == "target"
                || name == "node_modules"
                || name == "tests"
            {
                continue;
            }
            if child.is_dir() {
                collect_source_files(&child, out)?;
            } else if knr_core::is_source_file(&child) {
                out.push(child);
            }
        }
    }
    Ok(())
}
