use knr_core::Effect;
use knr_runtime::{HostPlatform, UiPlatform, VNode, Value, KNR_RUNTIME_BRIDGE_SCRIPT};
use std::collections::BTreeMap;

#[test]
fn test_struct_declarative_action_and_target() {
    let fields = vec![
        Value::Str("button".into()),
        Value::Str("btn act-inc".into()),
        Value::Str("+1".into()),
        Value::Str("/api/inc".into()),
        Value::Str(".counter-val".into()),
    ];
    let val = Value::Struct {
        schema: "Leaf".into(),
        fields,
    };
    let node = VNode::from_value(&val).expect("Valid Leaf struct");
    match node {
        VNode::Element {
            tag,
            props,
            children,
            text,
        } => {
            assert_eq!(tag, "button");
            assert_eq!(props.get("class").map(|s| s.as_str()), Some("btn act-inc"));
            assert_eq!(props.get("data-knr-action").map(|s| s.as_str()), Some("/api/inc"));
            assert_eq!(props.get("data-knr-target").map(|s| s.as_str()), Some(".counter-val"));
            assert_eq!(text.as_deref(), Some("+1"));
            assert!(children.is_empty());
        }
        _ => panic!("Expected VNode::Element"),
    }
}

#[test]
fn test_struct_empty_action_and_target_ignored() {
    let fields = vec![
        Value::Str("span".into()),
        Value::Str("badge".into()),
        Value::Str("v1.0".into()),
        Value::Str("".into()),
        Value::Str("".into()),
    ];
    let val = Value::Struct {
        schema: "Leaf".into(),
        fields,
    };
    let node = VNode::from_value(&val).expect("Valid Leaf struct");
    match node {
        VNode::Element { props, .. } => {
            assert!(!props.contains_key("data-knr-action"));
            assert!(!props.contains_key("data-knr-target"));
        }
        _ => panic!("Expected VNode::Element"),
    }
}

#[test]
fn test_struct_vec_children_backwards_compatible() {
    let child_fields = vec![
        Value::Str("span".into()),
        Value::Str("label".into()),
        Value::Str("Item".into()),
    ];
    let child_val = Value::Struct {
        schema: "Leaf".into(),
        fields: child_fields,
    };
    let fields = vec![
        Value::Str("div".into()),
        Value::Str("card".into()),
        Value::Str("".into()),
        Value::Vec(vec![child_val]),
    ];
    let val = Value::Struct {
        schema: "Card".into(),
        fields,
    };
    let node = VNode::from_value(&val).expect("Valid Card struct");
    match node {
        VNode::Element { tag, props, children, .. } => {
            assert_eq!(tag, "div");
            assert!(!props.contains_key("data-knr-action"));
            assert_eq!(children.len(), 1);
            match &children[0] {
                VNode::Element { tag, text, .. } => {
                    assert_eq!(tag, "span");
                    assert_eq!(text.as_deref(), Some("Item"));
                }
                _ => panic!("Expected child VNode::Element"),
            }
        }
        _ => panic!("Expected VNode::Element"),
    }
}

#[test]
fn test_struct_action_and_vec_children() {
    let child_fields = vec![
        Value::Str("span".into()),
        Value::Str("btn-text".into()),
        Value::Str("Submit".into()),
    ];
    let child_val = Value::Struct {
        schema: "Leaf".into(),
        fields: child_fields,
    };
    let fields = vec![
        Value::Str("form".into()),
        Value::Str("my-form".into()),
        Value::Str("".into()),
        Value::Str("/api/submit".into()),
        Value::Vec(vec![child_val]),
    ];
    let val = Value::Struct {
        schema: "FormNode".into(),
        fields,
    };
    let node = VNode::from_value(&val).expect("Valid struct with action and children");
    match node {
        VNode::Element { tag, props, children, .. } => {
            assert_eq!(tag, "form");
            assert_eq!(props.get("data-knr-action").map(|s| s.as_str()), Some("/api/submit"));
            assert_eq!(children.len(), 1);
        }
        _ => panic!("Expected VNode::Element"),
    }
}

#[test]
fn test_map_declarative_action_and_target() {
    let mut map = BTreeMap::new();
    map.insert("tag".to_string(), Value::Str("button".into()));
    map.insert("action".to_string(), Value::Str("/api/reset".into()));
    map.insert("target".to_string(), Value::Str(".counter-val".into()));
    map.insert("text".to_string(), Value::Str("Reset".into()));

    let node = VNode::from_value(&Value::Map(map)).expect("Valid Map VNode");
    match &node {
        VNode::Element { tag, props, text, .. } => {
            assert_eq!(tag, "button");
            assert_eq!(props.get("data-knr-action").map(|s| s.as_str()), Some("/api/reset"));
            assert_eq!(props.get("data-knr-target").map(|s| s.as_str()), Some(".counter-val"));
            assert_eq!(text.as_deref(), Some("Reset"));
        }
        _ => panic!("Expected VNode::Element"),
    }

    // Verify to_value roundtrip preserves action and target
    let roundtrip_val = node.to_value();
    match roundtrip_val {
        Value::Map(m) => {
            assert_eq!(m.get("action"), Some(&Value::Str("/api/reset".into())));
            assert_eq!(m.get("target"), Some(&Value::Str(".counter-val".into())));
        }
        _ => panic!("Expected Value::Map from to_value"),
    }
}

#[test]
fn test_has_declarative_actions_deep_tree() {
    let mut btn_props = BTreeMap::new();
    btn_props.insert("data-knr-action".to_string(), "/api/inc".to_string());
    let leaf_btn = VNode::element("button", btn_props, vec![], Some("+1".into()));

    // Nest 5 levels deep
    let l4 = VNode::element("div", BTreeMap::new(), vec![leaf_btn], None);
    let l3 = VNode::element("div", BTreeMap::new(), vec![l4], None);
    let l2 = VNode::element("section", BTreeMap::new(), vec![l3], None);
    let l1 = VNode::element("main", BTreeMap::new(), vec![l2], None);
    let root = VNode::element("body", BTreeMap::new(), vec![l1], None);

    assert!(root.has_action_attrs());
    assert!(!root.has_explicit_script());
    assert!(root.has_declarative_actions());
}

#[test]
fn test_has_declarative_actions_suppressed_by_explicit_script() {
    let mut btn_props = BTreeMap::new();
    btn_props.insert("data-knr-action".to_string(), "/api/dec".to_string());
    let leaf_btn = VNode::element("button", btn_props, vec![], Some("-1".into()));

    let script_node = VNode::element("script", BTreeMap::new(), vec![], Some("console.log('custom');".into()));

    let root = VNode::element(
        "div",
        BTreeMap::new(),
        vec![leaf_btn, script_node],
        None,
    );

    assert!(root.has_action_attrs());
    assert!(root.has_explicit_script());
    // Injection must be suppressed when explicit script tag is present
    assert!(!root.has_declarative_actions());

    let html = root.render_html();
    assert!(!html.contains("data-knr-runtime=\"v1\""));
    assert!(html.contains("console.log('custom');"));
}

#[test]
fn test_render_html_bridge_injection() {
    let mut btn_props = BTreeMap::new();
    btn_props.insert("data-knr-action".to_string(), "/api/inc".to_string());
    btn_props.insert("data-knr-target".to_string(), ".counter-val".to_string());
    let btn = VNode::element("button", btn_props, vec![], Some("Increment".into()));

    let main = VNode::element("main", BTreeMap::new(), vec![btn], None);
    let html = main.render_html();

    assert!(html.contains("<script data-knr-runtime=\"v1\">"));
    assert!(html.contains(KNR_RUNTIME_BRIDGE_SCRIPT));
    assert!(html.ends_with(&format!("{}</main>", KNR_RUNTIME_BRIDGE_SCRIPT)));

    // Tree without declarative actions must NOT inject bridge script
    let plain_p = VNode::element("p", BTreeMap::new(), vec![], Some("Plain text".into()));
    let plain_main = VNode::element("main", BTreeMap::new(), vec![plain_p], None);
    let plain_html = plain_main.render_html();
    assert!(!plain_html.contains("data-knr-runtime"));
    assert_eq!(plain_html, "<main><p>Plain text</p></main>");
}

#[test]
fn test_ui_platform_host_call_declarative_ssr() {
    let mut ui = UiPlatform::new();

    let fields = vec![
        Value::Str("button".into()),
        Value::Str("btn act-inc".into()),
        Value::Str("+1".into()),
        Value::Str("/api/inc".into()),
        Value::Str(".counter-val".into()),
    ];
    let btn_struct = Value::Struct {
        schema: "Leaf".into(),
        fields,
    };

    let card_fields = vec![
        Value::Str("div".into()),
        Value::Str("counter-box".into()),
        Value::Str("".into()),
        Value::Vec(vec![btn_struct]),
    ];
    let card_struct = Value::Struct {
        schema: "Card".into(),
        fields: card_fields,
    };

    let render_res = ui
        .host_call(Effect::Ui, "ui.render", card_struct)
        .expect("ui.render should succeed with declarative struct");

    match render_res {
        Value::Str(html) => {
            assert!(html.contains("data-knr-action=\"/api/inc\""));
            assert!(html.contains("data-knr-target=\".counter-val\""));
            assert!(html.contains("<script data-knr-runtime=\"v1\">"));
            assert!(html.ends_with(&format!("{}</div>", KNR_RUNTIME_BRIDGE_SCRIPT)));
        }
        other => panic!("Expected Value::Str, got {:?}", other),
    }
}
