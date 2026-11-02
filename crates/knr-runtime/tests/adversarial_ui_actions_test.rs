use knr_core::Effect;
use knr_runtime::{HostPlatform, RuntimeError, UiPlatform, VNode, Value, KNR_RUNTIME_BRIDGE_SCRIPT};
use std::collections::BTreeMap;

#[test]
fn test_edge_case_empty_vs_non_empty_action_target() {
    // 1. action is empty, target is non-empty -> target set, action NOT set, bridge NOT injected
    let mut map_empty_act = BTreeMap::new();
    map_empty_act.insert("tag".to_string(), Value::Str("button".into()));
    map_empty_act.insert("action".to_string(), Value::Str("".into()));
    map_empty_act.insert("target".to_string(), Value::Str(".output".into()));
    let node_empty_act = VNode::from_value(&Value::Map(map_empty_act)).expect("valid VDOM map");

    match &node_empty_act {
        VNode::Element { props, .. } => {
            assert!(!props.contains_key("data-knr-action"));
            assert_eq!(props.get("data-knr-target").map(|s| s.as_str()), Some(".output"));
        }
        _ => panic!("Expected element"),
    }
    assert!(!node_empty_act.has_action_attrs());
    assert!(!node_empty_act.has_declarative_actions());
    let html_empty_act = node_empty_act.render_html();
    assert!(!html_empty_act.contains("data-knr-runtime=\"v1\""));

    // 2. action is non-empty, target is empty -> action set, target NOT set, bridge IS injected
    let mut map_empty_tgt = BTreeMap::new();
    map_empty_tgt.insert("tag".to_string(), Value::Str("button".into()));
    map_empty_tgt.insert("action".to_string(), Value::Str("/api/trigger".into()));
    map_empty_tgt.insert("target".to_string(), Value::Str("".into()));
    let node_empty_tgt = VNode::from_value(&Value::Map(map_empty_tgt)).expect("valid VDOM map");

    match &node_empty_tgt {
        VNode::Element { props, .. } => {
            assert_eq!(props.get("data-knr-action").map(|s| s.as_str()), Some("/api/trigger"));
            assert!(!props.contains_key("data-knr-target"));
        }
        _ => panic!("Expected element"),
    }
    assert!(node_empty_tgt.has_action_attrs());
    assert!(node_empty_tgt.has_declarative_actions());
    let html_empty_tgt = node_empty_tgt.render_html();
    assert!(html_empty_tgt.contains("data-knr-runtime=\"v1\""));

    // 3. Both action and target empty in Struct
    let fields_both_empty = vec![
        Value::Str("a".into()),
        Value::Str("btn".into()),
        Value::Str("Click".into()),
        Value::Str("".into()),
        Value::Str("".into()),
    ];
    let struct_empty = Value::Struct {
        schema: "Leaf".into(),
        fields: fields_both_empty,
    };
    let node_struct_empty = VNode::from_value(&struct_empty).expect("valid struct");
    assert!(!node_struct_empty.has_action_attrs());
    assert!(!node_struct_empty.has_declarative_actions());
    assert!(!node_struct_empty.render_html().contains("data-knr-runtime=\"v1\""));
}

#[test]
fn test_edge_case_struct_field3_vs_field4_children_and_actions() {
    let child1 = Value::Struct {
        schema: "Leaf".into(),
        fields: vec![
            Value::Str("span".into()),
            Value::Str("c1".into()),
            Value::Str("Child1".into()),
        ],
    };
    let child2 = Value::Struct {
        schema: "Leaf".into(),
        fields: vec![
            Value::Str("span".into()),
            Value::Str("c2".into()),
            Value::Str("Child2".into()),
        ],
    };

    // Scenario A: Field 3 is Vec(children), Field 4 is Vec(children) -> both parsed into children
    let fields_double_vec = vec![
        Value::Str("div".into()),
        Value::Str("container".into()),
        Value::Str("".into()),
        Value::Vec(vec![child1.clone()]),
        Value::Vec(vec![child2.clone()]),
    ];
    let struct_double_vec = Value::Struct {
        schema: "Container".into(),
        fields: fields_double_vec,
    };
    let node_double_vec = VNode::from_value(&struct_double_vec).expect("valid double vec struct");
    match &node_double_vec {
        VNode::Element { children, props, .. } => {
            assert_eq!(children.len(), 2);
            assert!(!props.contains_key("data-knr-action"));
            assert!(!props.contains_key("data-knr-target"));
        }
        _ => panic!("Expected element"),
    }

    // Scenario B: Field 3 is Str (action), Field 4 is Vec(children) -> action AND children present
    let fields_act_and_children = vec![
        Value::Str("form".into()),
        Value::Str("box".into()),
        Value::Str("".into()),
        Value::Str("/api/submit_form".into()),
        Value::Vec(vec![child1]),
    ];
    let struct_act_and_children = Value::Struct {
        schema: "Form".into(),
        fields: fields_act_and_children,
    };
    let node_act_and_children =
        VNode::from_value(&struct_act_and_children).expect("valid action+children struct");
    match &node_act_and_children {
        VNode::Element { children, props, .. } => {
            assert_eq!(children.len(), 1);
            assert_eq!(
                props.get("data-knr-action").map(|s| s.as_str()),
                Some("/api/submit_form")
            );
            assert!(!props.contains_key("data-knr-target"));
        }
        _ => panic!("Expected element"),
    }
    assert!(node_act_and_children.has_declarative_actions());

    // Scenario C: Non-string non-vec in fields 3 and 4 -> gracefully ignored
    let fields_non_str_vec = vec![
        Value::Str("div".into()),
        Value::Str("c".into()),
        Value::Str("t".into()),
        Value::I64(9999),
        Value::Bool(false),
    ];
    let struct_non_str_vec = Value::Struct {
        schema: "Test".into(),
        fields: fields_non_str_vec,
    };
    let node_non_str_vec = VNode::from_value(&struct_non_str_vec).expect("valid struct with int fields");
    match &node_non_str_vec {
        VNode::Element { children, props, .. } => {
            assert!(children.is_empty());
            assert!(!props.contains_key("data-knr-action"));
            assert!(!props.contains_key("data-knr-target"));
        }
        _ => panic!("Expected element"),
    }

    // Scenario D: Struct with fewer than 3 fields (e.g. 1 field tag only)
    let struct_1_field = Value::Struct {
        schema: "TagOnly".into(),
        fields: vec![Value::Str("hr".into())],
    };
    let node_1_field = VNode::from_value(&struct_1_field).expect("valid 1-field struct");
    match &node_1_field {
        VNode::Element { tag, props, children, text } => {
            assert_eq!(tag, "hr");
            assert!(props.is_empty());
            assert!(children.is_empty());
            assert!(text.is_none());
        }
        _ => panic!("Expected element"),
    }

    // Scenario E: Struct with 0 fields -> InvalidArgument
    let struct_0_fields = Value::Struct {
        schema: "Empty".into(),
        fields: vec![],
    };
    assert!(matches!(
        VNode::from_value(&struct_0_fields),
        Err(RuntimeError::InvalidArgument(_))
    ));
}

#[test]
fn test_edge_case_map_string_vs_non_string_action_target() {
    let mut map = BTreeMap::new();
    map.insert("tag".to_string(), Value::Str("div".into()));
    // Non-string action and target
    map.insert("action".to_string(), Value::I64(42));
    map.insert("target".to_string(), Value::Bool(true));

    let node = VNode::from_value(&Value::Map(map)).expect("valid map with non-string action/target");
    match &node {
        VNode::Element { props, .. } => {
            assert!(!props.contains_key("data-knr-action"));
            assert!(!props.contains_key("data-knr-target"));
        }
        _ => panic!("Expected element"),
    }
    assert!(!node.has_action_attrs());
    assert!(!node.has_declarative_actions());

    // Map with Vec as action and Map as target
    let mut complex_map = BTreeMap::new();
    complex_map.insert("tag".to_string(), Value::Str("div".into()));
    complex_map.insert("action".to_string(), Value::Vec(vec![]));
    complex_map.insert("target".to_string(), Value::Map(BTreeMap::new()));
    let complex_node = VNode::from_value(&Value::Map(complex_map)).expect("valid map with complex action/target");
    assert!(!complex_node.has_action_attrs());
    assert!(!complex_node.has_declarative_actions());
}

#[test]
fn test_edge_case_deep_nested_trees_single_bridge_injection() {
    // Build a 25-level deep nested tree
    let leaf_action = VNode::element(
        "button",
        {
            let mut m = BTreeMap::new();
            m.insert("data-knr-action".to_string(), "/api/deep_click".to_string());
            m.insert("data-knr-target".to_string(), "#status".to_string());
            m
        },
        vec![],
        Some("Deep Button".into()),
    );

    let mut current = leaf_action;
    for level in 0..25 {
        let mut props = BTreeMap::new();
        props.insert("data-level".to_string(), level.to_string());
        current = VNode::element("div", props, vec![current], None);
    }

    assert!(current.has_action_attrs());
    assert!(!current.has_explicit_script());
    assert!(current.has_declarative_actions());

    let html = current.render_html();

    // Verify bridge script is present
    assert!(html.contains("data-knr-runtime=\"v1\""));

    // Verify bridge script is injected EXACTLY ONCE across the entire HTML output
    let occurrences = html.matches("data-knr-runtime=\"v1\"").count();
    assert_eq!(occurrences, 1, "Bridge script must be injected exactly once");

    // Verify it is placed right before the closing tag of the root element: ...</script></div>
    assert!(
        html.ends_with(&format!("{}</div>", KNR_RUNTIME_BRIDGE_SCRIPT)),
        "Bridge script must be directly before root element closing tag"
    );
}

#[test]
fn test_edge_case_explicit_script_suppression_variations() {
    let declarative_child = VNode::element(
        "button",
        {
            let mut m = BTreeMap::new();
            m.insert("data-knr-action".to_string(), "/api/click".to_string());
            m
        },
        vec![],
        Some("Click".into()),
    );

    // Case 1: Uppercase <SCRIPT> tag
    let upper_script = VNode::element(
        "SCRIPT",
        BTreeMap::new(),
        vec![],
        Some("console.log('UPPERCASE');".into()),
    );
    let root1 = VNode::element(
        "div",
        BTreeMap::new(),
        vec![declarative_child.clone(), upper_script],
        None,
    );
    assert!(root1.has_action_attrs());
    assert!(root1.has_explicit_script(), "Uppercase SCRIPT must be detected");
    assert!(!root1.has_declarative_actions(), "Bridge must be suppressed by uppercase SCRIPT");
    let html1 = root1.render_html();
    assert!(!html1.contains("data-knr-runtime=\"v1\""));
    assert!(html1.contains("console.log('UPPERCASE');"));

    // Case 2: Mixed-case <sCrIpt> tag
    let mixed_script = VNode::element(
        "sCrIpt",
        BTreeMap::new(),
        vec![],
        Some("console.log('mixed');".into()),
    );
    let root2 = VNode::element(
        "div",
        BTreeMap::new(),
        vec![declarative_child.clone(), mixed_script],
        None,
    );
    assert!(root2.has_explicit_script());
    assert!(!root2.has_declarative_actions());
    let html2 = root2.render_html();
    assert!(!html2.contains("data-knr-runtime=\"v1\""));

    // Case 3: Script deeply nested in a distant branch
    let deep_script = VNode::element(
        "script",
        {
            let mut p = BTreeMap::new();
            p.insert("src".to_string(), "/vendor/bundle.js".to_string());
            p
        },
        vec![],
        None,
    );
    let distant_branch = VNode::element(
        "footer",
        BTreeMap::new(),
        vec![VNode::element("div", BTreeMap::new(), vec![deep_script], None)],
        None,
    );
    let root3 = VNode::element(
        "html",
        BTreeMap::new(),
        vec![declarative_child, distant_branch],
        None,
    );
    assert!(root3.has_action_attrs());
    assert!(root3.has_explicit_script(), "Deeply nested script must be detected");
    assert!(!root3.has_declarative_actions());
    let html3 = root3.render_html();
    assert!(!html3.contains("data-knr-runtime=\"v1\""));
    assert!(html3.contains("/vendor/bundle.js"));
}

#[test]
fn test_edge_case_multiple_declarative_elements_and_xss_escaping() {
    // Multiple buttons with actions
    let b1 = VNode::element(
        "button",
        {
            let mut m = BTreeMap::new();
            m.insert("data-knr-action".to_string(), "/api/inc".to_string());
            m
        },
        vec![],
        Some("+1".into()),
    );
    let b2 = VNode::element(
        "button",
        {
            let mut m = BTreeMap::new();
            m.insert("data-knr-action".to_string(), "/api/dec".to_string());
            m
        },
        vec![],
        Some("-1".into()),
    );
    let b3 = VNode::element(
        "button",
        {
            let mut m = BTreeMap::new();
            // Adversarial payload in action attribute
            m.insert(
                "data-knr-action".to_string(),
                "/api/test?x=1&y=2\" onmouseover=\"alert('xss')".to_string(),
            );
            m.insert(
                "data-knr-target".to_string(),
                "#output<script>evil()</script>".to_string(),
            );
            m
        },
        vec![],
        Some("Reset".into()),
    );

    let container = VNode::element("div", BTreeMap::new(), vec![b1, b2, b3], None);
    let html = container.render_html();

    // Verify bridge script is injected exactly once despite 3 action elements
    assert_eq!(
        html.matches("data-knr-runtime=\"v1\"").count(),
        1,
        "Bridge script must be injected exactly once for multiple declarative elements"
    );

    // Verify attribute escaping prevented attribute breakout or script tag injection in attributes
    assert!(!html.contains("onmouseover=\"alert('xss')"));
    assert!(html.contains("&quot; onmouseover=&quot;alert(&#39;xss&#39;)"));
    assert!(html.contains("#output&lt;script&gt;evil()&lt;/script&gt;"));
}

#[test]
fn test_ui_platform_host_call_edge_cases() {
    let mut ui = UiPlatform::new();

    // ui.render with Void when nothing is mounted returns empty string
    let empty_render = ui
        .host_call(Effect::Ui, "ui.render", Value::Void)
        .expect("empty render should succeed");
    assert_eq!(empty_render, Value::Str("".into()));

    // Invalid effect rejected
    let err_eff = ui.host_call(Effect::Net, "ui.render", Value::Void);
    assert!(matches!(err_eff, Err(RuntimeError::UnsupportedEffect(_))));

    // Unknown op rejected
    let err_op = ui.host_call(Effect::Ui, "ui.unknown_op", Value::Void);
    assert!(matches!(err_op, Err(RuntimeError::InvalidOperation(_))));
}
