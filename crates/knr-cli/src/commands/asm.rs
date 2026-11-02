use std::fs;
use std::path::Path;

use crate::error::CliError;

pub fn execute(file: &Path, output: &Path) -> Result<(), CliError> {
    let content = fs::read_to_string(file).map_err(|e| {
        eprintln!("Error reading file '{}': {e}", file.display());
        CliError::Io(e)
    })?;

    let module = knr_core::parse_ssa(&content).map_err(|err| {
        eprintln!("{err}");
        CliError::Parse(err)
    })?;

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
