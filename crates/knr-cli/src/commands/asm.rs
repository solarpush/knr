use std::fs;
use std::path::Path;

use crate::error::CliError;

pub fn execute(file: &Path, output: &Path) -> Result<(), CliError> {
    let module = knr_core::load_module_from_path(file).map_err(|err| {
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
