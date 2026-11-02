use knr_core::parse_ssa;
use knr_verify::verify_module;
use std::collections::HashMap;
use std::fs;
use std::path::Path;

fn normalize_syscall_fixtures(content: &str) -> String {
    let mut str_constants: HashMap<String, String> = HashMap::new();
    let mut lines = Vec::new();

    for line in content.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix('%') {
            if let Some((reg_part, op_part)) = rest.split_once('=') {
                let reg = format!("%{}", reg_part.trim());
                let op = op_part.trim();
                if let Some(str_val) = op.strip_prefix("const_str ") {
                    let s = str_val.trim();
                    if s.starts_with('"') && s.ends_with('"') && s.len() >= 2 {
                        str_constants.insert(reg, s[1..s.len() - 1].to_string());
                    }
                }
            }
        }

        let mut new_line = line.to_string();
        for (reg, val) in &str_constants {
            for eff in &["Net", "Ui", "Hw", "Async"] {
                let pattern = format!("syscall {eff} {reg}");
                let replacement = format!("syscall {eff} \"{val}\"");
                if new_line.contains(&pattern) {
                    new_line = new_line.replace(&pattern, &replacement);
                }
            }
        }
        lines.push(new_line);
    }

    lines.join("\n")
}

#[test]
fn test_all_20_invalid_fixtures() {
    let invalid_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("tests/fixtures/invalid");

    let mut entries: Vec<_> = fs::read_dir(&invalid_dir)
        .expect("Failed to read invalid fixtures directory")
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "ssa"))
        .collect();

    entries.sort_by_key(|e| e.file_name());

    assert_eq!(
        entries.len(),
        20,
        "Expected exactly 20 canonical invalid fixtures in tests/fixtures/invalid/"
    );

    for entry in &entries {
        let path = entry.path();
        let filename = entry.file_name().into_string().unwrap();
        let raw_content = fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("Failed to read {filename}: {e}"));

        let content = normalize_syscall_fixtures(&raw_content);

        // Attempt to parse
        match parse_ssa(&content) {
            Ok(module) => {
                // If it parses into AST, verification MUST reject it!
                let res = verify_module(&module);
                assert!(
                    res.is_err(),
                    "Invalid fixture {filename} parsed successfully but was accepted by knr-verify!"
                );
                let diags = res.err().unwrap();
                assert!(
                    !diags.is_empty(),
                    "Expected diagnostics for invalid fixture {filename}"
                );
                let diag = &diags[0];
                println!(
                    "Fixture {filename} rejected with error_kind={:?}, line={}, inst={:?}",
                    diag.error_kind, diag.line, diag.instruction
                );

                // Verify diagnostic serializes to valid JSON matching schema
                let json_str = serde_json::to_string(&diag).unwrap();
                let deserialized: serde_json::Value = serde_json::from_str(&json_str).unwrap();
                assert_eq!(deserialized["status"], "error");
                assert_eq!(deserialized["error_kind"], diag.error_kind);

                // Validate specific error kind for known fixtures
                if filename.starts_with("err01") {
                    assert_eq!(diag.error_kind, "NonMonotonicRegisters");
                } else if filename.starts_with("err02") {
                    assert_eq!(diag.error_kind, "DominanceViolation");
                } else if filename.starts_with("err03") {
                    assert_eq!(diag.error_kind, "MultipleAssignment");
                } else if filename.starts_with("err04")
                    || filename.starts_with("err05")
                    || filename.starts_with("err06")
                    || filename.starts_with("err16")
                {
                    assert_eq!(diag.error_kind, "UseAfterMove");
                } else if filename.starts_with("err07") || filename.starts_with("err08") {
                    assert_eq!(diag.error_kind, "LinearLeak");
                } else if filename.starts_with("err09")
                    || filename.starts_with("err10")
                    || filename.starts_with("err19")
                    || filename.starts_with("err20")
                {
                    assert_eq!(diag.error_kind, "TypeMismatch");
                } else if filename.starts_with("err11") || filename.starts_with("err12") {
                    assert_eq!(diag.error_kind, "UndeclaredEffect");
                } else if filename.starts_with("err17") {
                    assert_eq!(diag.error_kind, "DominanceViolation");
                } else if filename.starts_with("err18") {
                    assert_eq!(diag.error_kind, "InvalidBranchTarget");
                }
            }
            Err(parse_err) => {
                // err13, err14, err15 are syntax/grammar level errors caught by parser
                println!("Fixture {filename} correctly rejected at parse level: {parse_err:?}");
                assert!(
                    filename.starts_with("err13")
                        || filename.starts_with("err14")
                        || filename.starts_with("err15"),
                    "Fixture {filename} was rejected at parse level unexpectedly: {parse_err:?}"
                );
            }
        }
    }
}

#[test]
fn test_valid_fixtures() {
    let valid_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("tests/fixtures/valid");

    let valid_files = [
        "01_minimal.ssa",
        "02_arithmetic.ssa",
        "03_linear_containers.ssa",
        "04_branching.ssa",
        "canonical_15_lines.ssa",
        "workload_compiler_pipeline.ssa",
        "workload_compression.ssa",
        "workload_http_service.ssa",
        "workload_packet_filter.ssa",
        "workload_vdom_ui.ssa",
    ];

    for file in &valid_files {
        let path = valid_dir.join(file);
        let raw_content = fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("Failed to read {file}: {e}"));

        let content = normalize_syscall_fixtures(&raw_content);

        let module = parse_ssa(&content)
            .unwrap_or_else(|e| panic!("Failed to parse valid fixture {file}: {e}"));

        let res = verify_module(&module);
        assert!(
            res.is_ok(),
            "Valid fixture {file} failed verification: {:?}",
            res.err()
        );
    }
}
