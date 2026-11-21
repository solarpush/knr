use std::fs;
use std::path::Path;

use crate::commands::init;
use crate::error::CliError;

/// Executes `knr new` to create a new project or workspace directory.
pub fn execute(
    path: &Path,
    name: Option<&str>,
    workspace: bool,
    template: &str,
    force: bool,
) -> Result<(), CliError> {
    if path.exists() {
        let is_empty = fs::read_dir(path)
            .map(|mut it| it.next().is_none())
            .unwrap_or(false);
        if !is_empty && !force {
            return Err(CliError::Other(format!(
                "Destination directory '{}' already exists and is not empty. Use --force to proceed.",
                path.display()
            )));
        }
    } else {
        fs::create_dir_all(path).map_err(|e| {
            CliError::Other(format!("Failed to create directory '{}': {e}", path.display()))
        })?;
    }

    let default_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .filter(|n| !n.is_empty())
        .unwrap_or("knr_app");

    let final_name = name.unwrap_or(default_name);

    init::execute(Some(path), Some(final_name), workspace, template, force)
}
