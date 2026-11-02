use std::path::Path;

use knr_interp::Value;

use crate::error::CliError;
use crate::platform::CliHostPlatform;

pub fn execute(file: &Path, gas: u64, trace: bool) -> Result<(), CliError> {
    let module = knr_core::load_module_from_path(file).map_err(|err| {
        eprintln!("{err}");
        CliError::Parse(err)
    })?;

    if let Err(diagnostics) = knr_verify::verify_module(&module) {
        for diag in &diagnostics {
            eprintln!("{diag}");
        }
        return Err(CliError::Verify(diagnostics.len()));
    }

    let entry_func = module
        .get_function("main")
        .or_else(|| module.functions.first())
        .ok_or_else(|| CliError::Other("Module contains no functions to execute".to_string()))?;

    let entry_name = entry_func.name.clone();

    let args: Vec<Value> = entry_func
        .params
        .iter()
        .map(|(_, ty)| knr_interp::default_value_for_type_with_schemas(ty, &module.schemas))
        .collect();

    let mut platform = CliHostPlatform::new();

    let (val, trace_records) = knr_interp::execute_module(
        &module,
        &entry_name,
        args,
        &mut platform,
        gas,
        trace,
    )
    .map_err(|err| {
        eprintln!("{err}");
        CliError::Vm(err)
    })?;

    if trace {
        let json_output = serde_json::to_string_pretty(&trace_records)
            .map_err(|e| CliError::Other(e.to_string()))?;
        println!("{json_output}");
    } else {
        println!("{val}");
    }

    Ok(())
}
