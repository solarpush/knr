use knr_core::Effect;
use knr_runtime::{HostPlatform, UiPlatform, VNode, Value};
use std::collections::BTreeMap;

#[test]
fn test_vdom_schema_and_html_ssr() {
    let mut props = BTreeMap::new();
    props.insert("class".to_string(), "container".to_string());
    props.insert("id".to_string(), "main".to_string());

    let node = VNode::element(
        "div",
        props,
        vec![
            VNode::element("h1", BTreeMap::new(), vec![], Some("Title".into())),
            VNode::element(
                "p",
                BTreeMap::new(),
                vec![VNode::text("Paragraph with "), VNode::element("b", BTreeMap::new(), vec![], Some("bold".into()))],
                None,
            ),
        ],
        None,
    );

    let html = node.render_html();
    assert_eq!(
        html,
        "<div class=\"container\" id=\"main\"><h1>Title</h1><p>Paragraph with <b>bold</b></p></div>"
    );
}

#[test]
fn test_void_elements_self_closing() {
    let mut img_props = BTreeMap::new();
    img_props.insert("src".to_string(), "avatar.png".to_string());
    img_props.insert("alt".to_string(), "User Avatar".to_string());
    let img = VNode::element("img", img_props, vec![], None);
    assert_eq!(img.render_html(), "<img alt=\"User Avatar\" src=\"avatar.png\" />");

    let mut input_props = BTreeMap::new();
    input_props.insert("type".to_string(), "text".to_string());
    input_props.insert("value".to_string(), "knr".to_string());
    let input = VNode::element("input", input_props, vec![], None);
    assert_eq!(input.render_html(), "<input type=\"text\" value=\"knr\" />");

    let br = VNode::element("br", BTreeMap::new(), vec![], None);
    assert_eq!(br.render_html(), "<br />");
}

#[test]
fn test_html_entity_escaping_xss_protection() {
    let mut props = BTreeMap::new();
    props.insert("data-payload".to_string(), "\"><script>alert('xss')</script>".to_string());

    let node = VNode::element(
        "div",
        props,
        vec![],
        Some("<script>alert('xss & attack')</script>".into()),
    );

    let html = node.render_html();
    // Verify no unescaped script tag or attribute breakouts exist
    assert!(!html.contains("<script>"));
    assert!(!html.contains("</script>"));
    assert!(html.contains("&lt;script&gt;alert(&#39;xss &amp; attack&#39;)&lt;/script&gt;"));
    assert!(html.contains("&quot;&gt;&lt;script&gt;"));
}

#[test]
fn test_deeply_nested_vdom_tree() {
    // 12 levels of nesting (>= 10 levels required by spec)
    let mut current = VNode::element("span", BTreeMap::new(), vec![], Some("deepest".into()));
    for i in (0..12).rev() {
        let mut props = BTreeMap::new();
        props.insert("level".to_string(), i.to_string());
        current = VNode::element("div", props, vec![current], None);
    }

    let html = current.render_html();
    assert!(html.starts_with("<div level=\"0\">"));
    assert!(html.contains("deepest"));
    assert!(html.ends_with("</div>"));
}

#[test]
fn test_vdom_with_50_children() {
    let children: Vec<VNode> = (0..50)
        .map(|i| {
            let mut props = BTreeMap::new();
            props.insert("key".to_string(), i.to_string());
            VNode::element("li", props, vec![], Some(format!("Item {i}")))
        })
        .collect();

    let list = VNode::element("ul", BTreeMap::new(), children, None);
    let html = list.render_html();
    assert!(html.starts_with("<ul><li key=\"0\">Item 0</li>"));
    assert!(html.ends_with("<li key=\"49\">Item 49</li></ul>"));
}

#[test]
fn test_ui_platform_mount_update_render_and_events() {
    let mut ui = UiPlatform::new();

    let mut map = BTreeMap::new();
    map.insert("tag".to_string(), Value::Str("button".into()));
    let mut props = BTreeMap::new();
    props.insert("id".to_string(), Value::Str("btn-1".into()));
    map.insert("props".to_string(), Value::Map(props));
    map.insert("text".to_string(), Value::Str("Click Me".into()));
    map.insert("children".to_string(), Value::Vec(vec![]));

    // ui.mount
    let mount_res = ui
        .host_call(Effect::Ui, "ui.mount", Value::Map(map.clone()))
        .expect("ui.mount should succeed");
    assert_eq!(mount_res, Value::Chan(1));

    // ui.render
    let render_res = ui
        .host_call(Effect::Ui, "ui.render", Value::Void)
        .expect("ui.render should succeed");
    assert_eq!(
        render_res,
        Value::Str("<button id=\"btn-1\">Click Me</button>".into())
    );

    // ui.update
    let mut updated_map = map;
    updated_map.insert("text".to_string(), Value::Str("Clicked!".into()));
    ui.host_call(Effect::Ui, "ui.update", Value::Map(updated_map))
        .expect("ui.update should succeed");

    let updated_render = ui
        .host_call(Effect::Ui, "ui.render", Value::Void)
        .expect("ui.render should succeed");
    assert_eq!(
        updated_render,
        Value::Str("<button id=\"btn-1\">Clicked!</button>".into())
    );

    // ui.dispatch_event & ui.poll_event
    let mut event = BTreeMap::new();
    event.insert("type".to_string(), Value::Str("click".into()));
    event.insert("target".to_string(), Value::Str("btn-1".into()));

    ui.host_call(Effect::Ui, "ui.dispatch_event", Value::Map(event.clone()))
        .expect("dispatch_event should succeed");

    let polled = ui
        .host_call(Effect::Ui, "ui.poll_event", Value::Void)
        .expect("poll_event should succeed");
    assert_eq!(polled, Value::Map(event));
}
