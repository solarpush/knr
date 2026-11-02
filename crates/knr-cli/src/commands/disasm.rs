use std::fs;
use std::path::Path;

use crate::error::CliError;

pub fn execute(file: &Path, output: Option<&Path>) -> Result<(), CliError> {
    let bin_bytes = fs::read(file).map_err(|e| {
        eprintln!("Error reading file '{}': {e}", file.display());
        CliError::Io(e)
    })?;

    let module = knr_core::decode_knrbin(&bin_bytes).map_err(|err| {
        eprintln!("{err}");
        CliError::Binary(err)
    })?;

    let text = knr_core::format_ssa(&module);

    if let Some(out_path) = output {
        if let Some(parent) = out_path.parent() {
            if !parent.as_os_str().is_empty() && !parent.exists() {
                fs::create_dir_all(parent).map_err(CliError::Io)?;
            }
        }
        fs::write(out_path, text).map_err(CliError::Io)?;
    } else {
        print!("{text}");
    }

    Ok(())
}
