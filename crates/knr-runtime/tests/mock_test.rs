use knr_core::Effect;
use knr_runtime::{HostPlatform, MockPlatform, Value};

#[test]
fn test_mock_platform_recording_and_responses() {
    let mut mock = MockPlatform::new();
    assert_eq!(mock.call_count(), 0);

    // Call without preset response returns default Void
    let res = mock
        .host_call(Effect::Net, "net.listen", Value::I64(8080))
        .expect("host_call should succeed");
    assert_eq!(res, Value::Void);
    assert_eq!(mock.call_count(), 1);
    assert!(mock.has_called(Effect::Net, "net.listen"));

    let last = mock.last_call().expect("should have last call");
    assert_eq!(last.effect, Effect::Net);
    assert_eq!(last.op, "net.listen");
    assert_eq!(last.payload, Value::I64(8080));

    // Preset response
    mock.set_response(Effect::Ui, "ui.mount", Value::Chan(42));
    let mount_res = mock
        .host_call(Effect::Ui, "ui.mount", Value::Str("div".into()))
        .expect("host_call should succeed");
    assert_eq!(mount_res, Value::Chan(42));
    assert_eq!(mock.call_count(), 2);

    // Default response change
    mock.set_default_response(Value::Bool(true));
    let other_res = mock
        .host_call(Effect::Hw, "hw.read_reg", Value::I64(0x1000))
        .expect("host_call should succeed");
    assert_eq!(other_res, Value::Bool(true));

    // Verify calls history
    let calls = mock.calls();
    assert_eq!(calls.len(), 3);
    assert_eq!(calls[1].op, "ui.mount");
    assert_eq!(calls[2].op, "hw.read_reg");

    // Clear
    mock.clear();
    assert_eq!(mock.call_count(), 0);
    assert!(!mock.has_called(Effect::Net, "net.listen"));
}
