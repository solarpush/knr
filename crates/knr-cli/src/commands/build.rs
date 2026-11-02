use std::fs;
use std::path::Path;
use std::process::Command;

use crate::args::BuildTarget;
use crate::c_runtime::KNR_RT_C;
use crate::error::CliError;

pub fn execute(file: &Path, target: BuildTarget, output: &Path) -> Result<(), CliError> {
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

    if let Some(parent) = output.parent() {
        if !parent.as_os_str().is_empty() && !parent.exists() {
            fs::create_dir_all(parent).map_err(CliError::Io)?;
        }
    }

    match target {
        BuildTarget::Wasm => {
            let backend = knr_codegen::WasmBackend::new();
            let wasm_bytes = backend.compile(&module).map_err(CliError::Codegen)?;
            fs::write(output, wasm_bytes).map_err(CliError::Io)?;
            Ok(())
        }
        BuildTarget::Native => {
            let mut backend = knr_codegen::CraneliftBackend::new().map_err(CliError::Codegen)?;
            let obj_bytes = backend
                .compile_object(&module)
                .map_err(CliError::Codegen)?;

            let timestamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            let temp_dir = std::env::temp_dir().join(format!("knr_build_{}_{}", std::process::id(), timestamp));
            fs::create_dir_all(&temp_dir).map_err(CliError::Io)?;

            let obj_path = temp_dir.join("module.o");
            let c_rt_path = temp_dir.join("knr_rt.c");

            fs::write(&obj_path, obj_bytes).map_err(CliError::Io)?;
            fs::write(&c_rt_path, KNR_RT_C).map_err(CliError::Io)?;

            let compiler = find_c_compiler().ok_or_else(|| {
                CliError::Linker("No C compiler (cc, gcc, clang) found in PATH for native linking".to_string())
            })?;

            let mut cmd = Command::new(compiler);
            cmd.arg("-O2")
                .arg(&obj_path)
                .arg(&c_rt_path)
                .arg("-o")
                .arg(output)
                .arg("-lpthread")
                .arg("-lm");

            // If the module does not define main, provide ENTRY_FN definition
            if module.get_function("main").is_none() {
                if let Some(entry_func) = module.functions.first() {
                    cmd.arg(format!("-DENTRY_FN={}", entry_func.name));
                }
            }

            let compile_res = cmd.output();
            let _ = fs::remove_dir_all(&temp_dir);

            let res = compile_res.map_err(CliError::Io)?;
            if !res.status.success() {
                let err_msg = String::from_utf8_lossy(&res.stderr).to_string();
                eprintln!("Linker error: {err_msg}");
                return Err(CliError::Linker(err_msg));
            }

            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if let Ok(metadata) = fs::metadata(output) {
                    let mut perms = metadata.permissions();
                    perms.set_mode(0o755);
                    let _ = fs::set_permissions(output, perms);
                }
            }

            Ok(())
        }
    }
}

fn find_c_compiler() -> Option<&'static str> {
    ["cc", "gcc", "clang"]
        .iter()
        .copied()
        .find(|&compiler| Command::new(compiler).arg("--version").output().is_ok())
}
