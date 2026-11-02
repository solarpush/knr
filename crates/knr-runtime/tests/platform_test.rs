use knr_core::Effect;
use knr_runtime::{RuntimeError, Value};
use std::collections::BTreeMap;

#[test]
fn test_value_variants_and_helpers() {
    let vi = Value::I64(42);
    assert_eq!(vi.as_i64(), Some(42));
    assert_eq!(vi.type_name(), "i64");

    let vf = Value::F64(3.25);
    assert_eq!(vf.as_f64(), Some(3.25));
    assert_eq!(vf.type_name(), "f64");

    let vb = Value::Bool(true);
    assert_eq!(vb.as_bool(), Some(true));
    assert_eq!(vb.type_name(), "bool");

    let vs = Value::Str("hello".into());
    assert_eq!(vs.as_str(), Some("hello"));
    assert_eq!(vs.type_name(), "str");

    let mut vbuf = Value::Buf(vec![1, 2, 3]);
    assert_eq!(vbuf.as_buf(), Some(&[1, 2, 3][..]));
    if let Some(buf_mut) = vbuf.as_buf_mut() {
        buf_mut.push(4);
    }
    assert_eq!(vbuf.as_buf(), Some(&[1, 2, 3, 4][..]));
    assert_eq!(vbuf.type_name(), "buf");

    let mut vv = Value::Vec(vec![Value::I64(1), Value::I64(2)]);
    assert_eq!(vv.as_vec().map(|v| v.len()), Some(2));
    if let Some(vec_mut) = vv.as_vec_mut() {
        vec_mut.push(Value::I64(3));
    }
    assert_eq!(vv.as_vec().map(|v| v.len()), Some(3));
    assert_eq!(vv.type_name(), "vec");

    let mut map = BTreeMap::new();
    map.insert("key".to_string(), Value::Str("val".into()));
    let mut vm = Value::Map(map);
    assert_eq!(vm.as_map().and_then(|m| m.get("key")), Some(&Value::Str("val".into())));
    if let Some(map_mut) = vm.as_map_mut() {
        map_mut.insert("key2".to_string(), Value::I64(100));
    }
    assert_eq!(vm.as_map().and_then(|m| m.get("key2")), Some(&Value::I64(100)));
    assert_eq!(vm.type_name(), "map");

    let vc = Value::Chan(7);
    assert_eq!(vc.as_chan(), Some(7));
    assert_eq!(vc.type_name(), "chan");

    let vvoid = Value::Void;
    assert!(vvoid.is_void());
    assert_eq!(vvoid.type_name(), "void");
}

#[test]
fn test_nan_equality() {
    let nan1 = Value::F64(f64::NAN);
    let nan2 = Value::F64(f64::NAN);
    assert_eq!(nan1, nan2);

    let norm = Value::F64(1.0);
    assert_ne!(nan1, norm);
}

#[test]
fn test_deeply_nested_structures() {
    let mut inner_map = BTreeMap::new();
    inner_map.insert("deep_str".to_string(), Value::Str("level3".into()));
    inner_map.insert("deep_num".to_string(), Value::I64(999));

    let nested = Value::Vec(vec![
        Value::I64(1),
        Value::Vec(vec![
            Value::Map(inner_map.clone()),
            Value::Buf(vec![10, 20, 30]),
        ]),
    ]);

    let clone = nested.clone();
    assert_eq!(nested, clone);

    // JSON serialization roundtrip
    let json = nested.to_json();
    let reconstructed = Value::from_json(&json);
    assert_eq!(reconstructed.as_vec().unwrap().len(), 2);
}

#[test]
fn test_display_formatting() {
    assert_eq!(format!("{}", Value::I64(123)), "123");
    assert_eq!(format!("{}", Value::Bool(false)), "false");
    assert_eq!(format!("{}", Value::Str("test".into())), "\"test\"");
    assert_eq!(format!("{}", Value::Void), "()");
    assert_eq!(format!("{}", Value::Chan(42)), "chan(42)");
    assert_eq!(format!("{}", Value::Buf(vec![0; 5])), "<buf len=5>");
    assert_eq!(format!("{}", Value::Vec(vec![Value::I64(1), Value::I64(2)])), "[1, 2]");
}

#[test]
fn test_host_platform_unsupported_effect() {
    let err = RuntimeError::UnsupportedEffect(Effect::Net);
    assert_eq!(format!("{err}"), "Effect not supported: Net");
}
