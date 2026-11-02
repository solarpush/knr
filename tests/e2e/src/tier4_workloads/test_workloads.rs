use crate::harness::*;

/// Tier 4: Real-World Application Workload Tests
/// End-to-end verification of macro workloads: HTTP Microservice, VDOM UI, Packet Filter, Compression, Pipeline.

#[test]
fn test_t4_w01_http_microservice_fixture() {
    let fixture = FixtureLoader::new()
        .load_valid_fixtures()
        .into_iter()
        .find(|f| f.filename == "workload_http_service.ssa")
        .expect("workload_http_service.ssa exists");

    assert!(validate_ssa_grammar(&fixture.content).is_ok());
    assert!(fixture.content.contains("effects(Net, Async)"));
    assert!(fixture.content.contains("syscall Net"));
}

#[test]
fn test_t4_w02_vdom_ui_fixture() {
    let fixture = FixtureLoader::new()
        .load_valid_fixtures()
        .into_iter()
        .find(|f| f.filename == "workload_vdom_ui.ssa")
        .expect("workload_vdom_ui.ssa exists");

    assert!(validate_ssa_grammar(&fixture.content).is_ok());
    assert!(fixture.content.contains("effects(Ui)"));
    assert!(fixture.content.contains("map_new str str"));
}

#[test]
fn test_t4_w03_packet_filter_fixture() {
    let fixture = FixtureLoader::new()
        .load_valid_fixtures()
        .into_iter()
        .find(|f| f.filename == "workload_packet_filter.ssa")
        .expect("workload_packet_filter.ssa exists");

    assert!(validate_ssa_grammar(&fixture.content).is_ok());
    assert!(fixture.content.contains("effects(Hw)"));
    assert!(fixture.content.contains("buf_alloc"));
}

#[test]
fn test_t4_w04_compression_fixture() {
    let fixture = FixtureLoader::new()
        .load_valid_fixtures()
        .into_iter()
        .find(|f| f.filename == "workload_compression.ssa")
        .expect("workload_compression.ssa exists");

    assert!(validate_ssa_grammar(&fixture.content).is_ok());
    assert!(fixture.content.contains("vec_new i64"));
    assert!(fixture.content.contains("vec_push"));
}

#[test]
fn test_t4_w05_compiler_pipeline_fixture() {
    let fixture = FixtureLoader::new()
        .load_valid_fixtures()
        .into_iter()
        .find(|f| f.filename == "workload_compiler_pipeline.ssa")
        .expect("workload_compiler_pipeline.ssa exists");

    assert!(validate_ssa_grammar(&fixture.content).is_ok());
    assert!(fixture.content.contains("br_if"));
}
