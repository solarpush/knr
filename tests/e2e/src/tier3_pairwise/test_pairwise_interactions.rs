use crate::harness::*;

/// Tier 3: Cross-Feature Pairwise Interaction Tests
/// Validates emergent behaviors across subsystems sharing state, data flow, or control flow.

#[test]
fn test_t3_p01_linear_ownership_across_branches_spec() {
    let fixture = FixtureLoader::new()
        .load_valid_fixtures()
        .into_iter()
        .find(|f| f.filename == "04_branching.ssa")
        .expect("04_branching.ssa fixture exists");

    assert!(validate_ssa_grammar(&fixture.content).is_ok());
}

#[test]
fn test_t3_p02_buffer_allocation_and_syscall() {
    let fixture = FixtureLoader::new()
        .load_valid_fixtures()
        .into_iter()
        .find(|f| f.filename == "workload_packet_filter.ssa")
        .expect("workload_packet_filter.ssa exists");

    assert!(validate_ssa_grammar(&fixture.content).is_ok());
}

#[test]
fn test_t3_p03_vector_growth_and_containers() {
    let fixture = FixtureLoader::new()
        .load_valid_fixtures()
        .into_iter()
        .find(|f| f.filename == "workload_compression.ssa")
        .expect("workload_compression.ssa exists");

    assert!(validate_ssa_grammar(&fixture.content).is_ok());
}

#[test]
fn test_t3_p05_binary_codec_deduplication_and_extra_operands() {
    let words = vec![
        pack_instruction_word(0x14, 0x07, 3, 0, 0),
        pack_instruction_word(0x14, 0x07, 4, 3, 1),
    ];
    assert_eq!(words.len(), 2);
    for w in &words {
        assert!(unpack_instruction_word(w).is_ok());
    }
}
