#[test]
fn test_e2e_suite_initialization() {
    println!("Initializing knr E2E Test Suite...");
    let loader = knr_e2e::harness::FixtureLoader::new();
    let valids = loader.load_valid_fixtures();
    let invalids = loader.load_invalid_fixtures();

    println!("Loaded {} valid fixtures", valids.len());
    println!("Loaded {} invalid fixtures", invalids.len());

    assert!(valids.len() >= 5, "Must have at least 5 valid fixtures");
    assert_eq!(invalids.len(), 20, "Must have exactly 20 invalid fixtures");
}
