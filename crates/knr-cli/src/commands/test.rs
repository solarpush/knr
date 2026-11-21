use std::path::{Path, PathBuf};
use std::time::Instant;

use knr_interp::Value;
use knr_runtime::mock::MockPlatform;
use serde::Serialize;

use crate::error::CliError;

#[derive(Debug, Serialize)]
struct TestCaseReport {
    file: String,
    name: String,
    passed: bool,
    duration_us: u128,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

#[derive(Debug, Serialize)]
struct TestSuiteReport {
    status: String,
    total: usize,
    passed: usize,
    failed: usize,
    results: Vec<TestCaseReport>,
}

pub fn execute(path: &Path, json: bool, gas: u64) -> Result<(), CliError> {
    let files = collect_source_files(path)?;
    if files.is_empty() {
        return Err(CliError::Other(format!(
            "No .knr or .ssa files found under '{}'",
            path.display()
        )));
    }

    let mut results = Vec::new();
    let mut passed_count = 0usize;
    let mut failed_count = 0usize;

    for file in &files {
        let file_label = file.display().to_string();
        let module = if file.to_string_lossy().ends_with(".knr") {
            let src = std::fs::read_to_string(file).map_err(CliError::Io)?;
            knr_core::syntax::parse_knr(&src).map_err(|err| {
                if !json {
                    eprintln!("{file_label}: {err}");
                }
                CliError::Parse(err)
            })?;
            knr_verify::verify_program_from_file(file).map_err(|diags| {
                if !json {
                    for d in &diags {
                        eprintln!("{file_label}: {d}");
                    }
                }
                CliError::Verify(diags.len())
            })?
        } else {
            let m = knr_core::load_module_from_path(file).map_err(|err| {
                if !json {
                    eprintln!("{file_label}: {err}");
                }
                CliError::Parse(err)
            })?;
            if let Err(diags) = knr_verify::verify_module(&m) {
                if !json {
                    for d in &diags {
                        eprintln!("{file_label}: {d}");
                    }
                }
                return Err(CliError::Verify(diags.len()));
            }
            m
        };

        // Discover zero-arg `test_*` functions declared in this file
        let test_fns: Vec<String> = module
            .functions
            .iter()
            .filter(|f| f.name.starts_with("test_") && f.params.is_empty())
            .map(|f| f.name.clone())
            .collect();

        for fn_name in test_fns {
            let start = Instant::now();
            let mut mock = MockPlatform::new();
            let exec_res = knr_interp::execute_module(&module, &fn_name, vec![], &mut mock, gas, false);
            let duration_us = start.elapsed().as_micros();

            match exec_res {
                Ok((Value::Bool(true), _)) => {
                    passed_count += 1;
                    results.push(TestCaseReport {
                        file: file_label.clone(),
                        name: fn_name,
                        passed: true,
                        duration_us,
                        error: None,
                    });
                }
                Ok((Value::I64(n), _)) if n != 0 => {
                    passed_count += 1;
                    results.push(TestCaseReport {
                        file: file_label.clone(),
                        name: fn_name,
                        passed: true,
                        duration_us,
                        error: None,
                    });
                }
                Ok((val, _)) => {
                    failed_count += 1;
                    results.push(TestCaseReport {
                        file: file_label.clone(),
                        name: fn_name,
                        passed: false,
                        duration_us,
                        error: Some(format!("Assertion failed (returned {val})")),
                    });
                }
                Err(vm_err) => {
                    failed_count += 1;
                    results.push(TestCaseReport {
                        file: file_label.clone(),
                        name: fn_name,
                        passed: false,
                        duration_us,
                        error: Some(vm_err.to_string()),
                    });
                }
            }
        }
    }

    let total = passed_count + failed_count;
    if json {
        let report = TestSuiteReport {
            status: if failed_count == 0 { "ok" } else { "failed" }.to_string(),
            total,
            passed: passed_count,
            failed: failed_count,
            results,
        };
        let s = serde_json::to_string_pretty(&report)
            .map_err(|e| CliError::Other(e.to_string()))?;
        println!("{s}");
    } else {
        for r in &results {
            if r.passed {
                println!("  PASS  {}::{} ({} us)", r.file, r.name, r.duration_us);
            } else {
                println!(
                    "  FAIL  {}::{} ({} us) — {}",
                    r.file,
                    r.name,
                    r.duration_us,
                    r.error.as_deref().unwrap_or("failed")
                );
            }
        }
        println!(
            "\ntest result: {}. {} passed; {} failed; {} total",
            if failed_count == 0 { "ok" } else { "FAILED" },
            passed_count,
            failed_count,
            total
        );
    }

    if failed_count > 0 {
        return Err(CliError::Other(format!("{failed_count} test(s) failed")));
    }

    Ok(())
}

fn collect_source_files(path: &Path) -> Result<Vec<PathBuf>, CliError> {
    if !path.exists() {
        return Err(CliError::Other(format!(
            "Path does not exist: '{}'",
            path.display()
        )));
    }
    if path.is_file() {
        return Ok(vec![path.to_path_buf()]);
    }
    let mut out = Vec::new();
    visit_dir(path, &mut out)?;
    out.sort();
    Ok(out)
}

fn visit_dir(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), CliError> {
    for entry in std::fs::read_dir(dir).map_err(CliError::Io)? {
        let entry = entry.map_err(CliError::Io)?;
        let p = entry.path();
        if p.is_dir() {
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if !name.starts_with('.') && name != "target" {
                visit_dir(&p, out)?;
            }
        } else if let Some(s) = p.to_str() {
            if s.ends_with(".knr") || s.ends_with(".ssa") {
                out.push(p);
            }
        }
    }
    Ok(())
}
