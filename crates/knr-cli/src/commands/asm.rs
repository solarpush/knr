use std::fs;
use std::path::Path;

use crate::error::CliError;

pub fn execute(file: &Path, output: &Path) -> Result<(), CliError> {
    let module = if file.to_string_lossy().ends_with(".knr") {
        let src = std::fs::read_to_string(file).map_err(|e| {
            eprintln!("{e}");
            CliError::Other(e.to_string())
        })?;
        let prog = knr_core::syntax::parse_knr(&src).map_err(|err| {
            eprintln!("{err}");
            CliError::Parse(err)
        })?;
        if let Err(diagnostics) = knr_verify::verify_program(&prog) {
            for diag in &diagnostics {
                eprintln!("{diag}");
            }
            return Err(CliError::Verify(diagnostics.len()));
        }
        knr_core::lower::lower_program_from_file(file).map_err(|err| {
            eprintln!("{err}");
            CliError::Other(err.to_string())
        })?
    } else {
        knr_core::load_module_from_path(file).map_err(|err| {
            eprintln!("{err}");
            CliError::Parse(err)
        })?
    };

    let bin_bytes = knr_core::encode_knrbin(&module).map_err(|err| {
        eprintln!("{err}");
        CliError::Binary(err)
    })?;

    if let Some(parent) = output.parent() {
        if !parent.as_os_str().is_empty() && !parent.exists() {
            fs::create_dir_all(parent).map_err(CliError::Io)?;
        }
    }

    fs::write(output, bin_bytes).map_err(CliError::Io)?;
    Ok(())
}
