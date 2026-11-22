use std::fs;
use std::path::{Path, PathBuf};

use crate::error::CliError;

pub fn execute(file: &Path, output: Option<&Path>) -> Result<(), CliError> {
    let module = if knr_core::is_source_file(file) {
        let src = fs::read_to_string(file).map_err(|e| {
            eprintln!("Error reading file '{}': {e}", file.display());
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
        let bin_bytes = fs::read(file).map_err(|e| {
            eprintln!("Error reading file '{}': {e}", file.display());
            CliError::Io(e)
        })?;

        knr_core::decode_knrbin(&bin_bytes).map_err(|err| {
            eprintln!("{err}");
            CliError::Binary(err)
        })?
    };

    if let Some(out_path) = output {
        if let Some(parent) = out_path.parent() {
            if !parent.as_os_str().is_empty() && !parent.exists() {
                fs::create_dir_all(parent).map_err(CliError::Io)?;
            }
        }

        if !module.schemas.is_empty() && !out_path.to_string_lossy().ends_with(".ssa.schema") {
            // Split schemas into accompanying .ssa.schema file
            let filename = out_path.file_name().unwrap_or_default().to_string_lossy();
            let stem = if let Some(stripped) = filename.strip_suffix(".ssa") {
                stripped
            } else if let Some(dot_pos) = filename.rfind('.') {
                &filename[..dot_pos]
            } else {
                &filename
            };
            let schema_filename = format!("{stem}.ssa.schema");
            let schema_path = if let Some(parent) = out_path.parent() {
                if !parent.as_os_str().is_empty() {
                    parent.join(&schema_filename)
                } else {
                    PathBuf::from(&schema_filename)
                }
            } else {
                PathBuf::from(&schema_filename)
            };

            let mut schema_text = String::new();
            for (i, schema) in module.schemas.iter().enumerate() {
                if i > 0 {
                    schema_text.push('\n');
                }
                schema_text.push_str(&knr_core::format_schema(schema));
            }
            fs::write(&schema_path, schema_text).map_err(CliError::Io)?;

            let mut code_text = String::new();
            code_text.push_str(&format!("import \"{schema_filename}\"\n"));
            for imp in &module.imports {
                if imp != &schema_filename {
                    code_text.push_str(&format!("import \"{imp}\"\n"));
                }
            }
            if !module.functions.is_empty() {
                code_text.push('\n');
            }
            for (i, func) in module.functions.iter().enumerate() {
                if i > 0 {
                    code_text.push('\n');
                }
                code_text.push_str(&knr_core::format_function(func));
            }
            fs::write(out_path, code_text).map_err(CliError::Io)?;
        } else {
            let text = knr_core::format_ssa(&module);
            fs::write(out_path, text).map_err(CliError::Io)?;
        }
    } else {
        let text = knr_core::format_ssa(&module);
        print!("{text}");
    }

    Ok(())
}
