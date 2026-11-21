use knr_core::lower::lower_program_from_file;
use knr_core::parse_knr;
use knr_verify::{verify_module, verify_program};
use std::fs;
use std::path::Path;

#[test]
fn test_reference_exec_graph_component_verifies_cleanly() {
    let component_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("site/components/exec_graph.knr");

    assert!(component_path.exists(), "site/components/exec_graph.knr must exist");

    // 1. Test loading and lowering from file (which resolves ../schemas/vdom.ssa.schema)
    let module = lower_program_from_file(&component_path)
        .unwrap_or_else(|e| panic!("Failed to load/lower exec_graph.knr: {e}"));

    // Verify schemas are present
    assert!(
        module.schemas.iter().any(|s| s.name == "Section"),
        "Section schema must be present"
    );
    assert!(
        module.schemas.iter().any(|s| s.name == "Card"),
        "Card schema must be present"
    );
    assert!(
        module.schemas.iter().any(|s| s.name == "Leaf"),
        "Leaf schema must be present"
    );

    // Verify function build_exec_graph exists
    assert!(
        module.functions.iter().any(|f| f.name == "build_exec_graph"),
        "build_exec_graph function must be present"
    );

    // 2. Run full 4-pass verification on the lowered Module
    let result = verify_module(&module);
    assert!(
        result.is_ok(),
        "Reference execution graph component failed static verification: {:?}",
        result.err()
    );

    // 3. Verify via verify_program directly
    let src = fs::read_to_string(&component_path).expect("read exec_graph.knr");
    let prog = parse_knr(&src).expect("parse exec_graph.knr");
    let prog_result = verify_program(&prog);
    assert!(
        prog_result.is_ok(),
        "verify_program on exec_graph.knr failed: {:?}",
        prog_result.err()
    );
}
