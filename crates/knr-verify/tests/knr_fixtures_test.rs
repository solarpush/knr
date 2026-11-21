use knr_core::parse_knr;
use knr_verify::verify_program;
use std::fs;
use std::path::Path;

#[test]
fn test_all_knr_valid_fixtures() {
    let fixtures_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("tests/fixtures/knr_valid");

    let entries = fs::read_dir(&fixtures_dir).expect("knr_valid fixtures dir exists");
    let mut count = 0;

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "knr") {
            count += 1;
            let filename = path.file_name().unwrap().to_string_lossy();
            let source = fs::read_to_string(&path).expect("read fixture");

            let prog = parse_knr(&source)
                .unwrap_or_else(|e| panic!("Failed to parse valid fixture {filename}: {e:?}"));

            let res = verify_program(&prog);
            assert!(
                res.is_ok(),
                "Valid fixture {filename} failed verification: {:?}",
                res.err()
            );
        }
    }
    assert!(count >= 8, "Expected at least 8 valid knr fixtures, found {count}");
}

#[test]
fn test_all_knr_invalid_fixtures() {
    let fixtures_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("tests/fixtures/knr_invalid");

    let entries = fs::read_dir(&fixtures_dir).expect("knr_invalid fixtures dir exists");
    let mut count = 0;

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "knr") {
            count += 1;
            let filename = path.file_name().unwrap().to_string_lossy();
            let source = fs::read_to_string(&path).expect("read fixture");

            let expected_error = source
                .lines()
                .find(|l| l.starts_with("// Error:"))
                .map(|l| l.trim_start_matches("// Error:").trim().to_string())
                .unwrap_or_else(|| panic!("Fixture {filename} must declare '// Error: <Kind>'"));

            match parse_knr(&source) {
                Ok(prog) => {
                    let res = verify_program(&prog);
                    assert!(
                        res.is_err(),
                        "Invalid fixture {filename} unexpectedly passed verification!"
                    );
                    let diags = res.err().unwrap();
                    assert!(!diags.is_empty(), "Expected diagnostics for {filename}");
                    let actual_error = &diags[0].error_kind;
                    assert_eq!(
                        actual_error, &expected_error,
                        "Fixture {filename}: expected error '{expected_error}', got '{actual_error}'"
                    );
                }
                Err(parse_err) => {
                    assert_eq!(
                        expected_error, "ParseError",
                        "Fixture {filename} failed parsing ({parse_err:?}) but expected {expected_error}"
                    );
                }
            }
        }
    }
    assert!(count >= 12, "Expected at least 12 invalid knr fixtures, found {count}");
}
