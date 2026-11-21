use crate::gui::ExecutionGraph;
use crate::platform::{HostPlatform, RuntimeError, Value};
use knr_core::Effect;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;


/// HTML self-closing void elements according to the W3C HTML5 specification.
const VOID_ELEMENTS: &[&str] = &[
    "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param", "source",
    "track", "wbr",
];

/// Checks if an HTML tag name represents a self-closing void element.
#[inline]
pub fn is_void_element(tag: &str) -> bool {
    let lower = tag.to_ascii_lowercase();
    VOID_ELEMENTS.contains(&lower.as_str())
}

/// Escapes HTML characters in text content to prevent XSS injection.
pub fn escape_html_text(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for c in input.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            other => out.push(other),
        }
    }
    out
}

/// Escapes HTML attribute values to prevent attribute breakout and XSS.
pub fn escape_html_attr(input: &str) -> String {
    escape_html_text(input)
}

/// Clean, 100% generic knr runtime bridge script for declarative `[data-knr-action]` elements.
/// Supports both SSR (`fetch(action, { method: 'POST' })`) and mounted WASM MVU hosts
/// (`window.__knr_dispatch(action)`), with zero application-specific hardcoding.
pub const KNR_RUNTIME_BRIDGE_SCRIPT: &str = "<script data-knr-runtime=\"v1\">\
(function(){\
document.addEventListener('click',async function(e){\
const btn=e.target&&e.target.closest?e.target.closest('[data-knr-action]'):null;\
if(!btn)return;\
const act=btn.getAttribute('data-knr-action');\
if(!act)return;\
if(typeof window.__knr_dispatch==='function'){\
window.__knr_dispatch(act,btn);\
return;\
}\
const tgt=btn.getAttribute('data-knr-target');\
const el=tgt?document.querySelector(tgt):null;\
if(act.startsWith('/')&&typeof location!=='undefined'&&location.protocol&&location.protocol.startsWith('http')){\
try{\
const r=await fetch(act,{method:'POST'});\
if(r.ok){\
const t=(await r.text()).trim();\
if(el)el.textContent=t;\
}\
}catch(_){}\
}\
});\
})();\
</script>";

/// In-memory Virtual DOM node representation.
#[derive(Debug, Clone, PartialEq)]
pub enum VNode {
    Element {
        tag: String,
        props: BTreeMap<String, String>,
        children: Vec<VNode>,
        text: Option<String>,
    },
    Text(String),
    RawHtml(String),
}

impl VNode {
    /// Constructs a new element VNode.
    pub fn element(
        tag: impl Into<String>,
        props: BTreeMap<String, String>,
        children: Vec<VNode>,
        text: Option<String>,
    ) -> Self {
        VNode::Element {
            tag: tag.into(),
            props,
            children,
            text,
        }
    }

    /// Constructs a new text VNode.
    pub fn text(content: impl Into<String>) -> Self {
        VNode::Text(content.into())
    }

    /// Constructs a new raw HTML VNode that bypasses entity escaping.
    pub fn raw_html(content: impl Into<String>) -> Self {
        VNode::RawHtml(content.into())
    }

    /// Converts an `ExecutionGraph` into a semantic VNode wrapping clean SVG markup.
    pub fn from_execution_graph(graph: &ExecutionGraph) -> Self {
        let svg = graph.render_to_svg();
        let mut props = BTreeMap::new();
        props.insert("class".to_string(), "knr-exec-graph-container".to_string());
        VNode::Element {
            tag: "div".to_string(),
            props,
            children: vec![VNode::RawHtml(svg)],
            text: None,
        }
    }


    /// Parses a `Value` (typically `Value::Map` or `Value::Str`) into a `VNode`.
    pub fn from_value(val: &Value) -> Result<Self, RuntimeError> {
        match val {
            Value::Str(s) => Ok(VNode::Text(s.clone())),
            Value::I64(n) => Ok(VNode::Text(n.to_string())),
            Value::F64(n) => Ok(VNode::Text(n.to_string())),
            Value::Bool(b) => Ok(VNode::Text(b.to_string())),
            Value::Map(m) => {
                let tag = m
                    .get("tag")
                    .and_then(Value::as_str)
                    .ok_or_else(|| RuntimeError::InvalidArgument("VDOM element requires a string 'tag'".into()))?
                    .to_string();

                let mut props = BTreeMap::new();
                if let Some(p) = m.get("props") {
                    match p {
                        Value::Map(pm) => {
                            for (k, v) in pm {
                                let s = match v {
                                    Value::Str(s) => s.clone(),
                                    Value::I64(n) => n.to_string(),
                                    Value::F64(n) => n.to_string(),
                                    Value::Bool(b) => b.to_string(),
                                    other => other.to_string(),
                                };
                                props.insert(k.clone(), s);
                            }
                        }
                        _ => {
                            return Err(RuntimeError::InvalidArgument(
                                "VDOM 'props' must be a Map".into(),
                            ))
                        }
                    }
                }

                // Handle declarative action and target in Map
                if let Some(Value::Str(action)) = m.get("action") {
                    if !action.is_empty() {
                        props.insert("data-knr-action".to_string(), action.clone());
                    }
                }
                if let Some(Value::Str(target)) = m.get("target") {
                    if !target.is_empty() {
                        props.insert("data-knr-target".to_string(), target.clone());
                    }
                }

                let text = match m.get("text") {
                    Some(Value::Str(s)) if !s.is_empty() => Some(s.clone()),
                    Some(Value::I64(n)) => Some(n.to_string()),
                    Some(Value::F64(n)) => Some(n.to_string()),
                    Some(Value::Bool(b)) => Some(b.to_string()),
                    _ => None,
                };

                let mut children = Vec::new();
                if let Some(c) = m.get("children") {
                    match c {
                        Value::Vec(cv) => {
                            for child in cv {
                                children.push(VNode::from_value(child)?);
                            }
                        }
                        _ => {
                            return Err(RuntimeError::InvalidArgument(
                                "VDOM 'children' must be a Vec".into(),
                            ))
                        }
                    }
                }

                Ok(VNode::Element {
                    tag,
                    props,
                    children,
                    text,
                })
            }
            Value::Struct { fields, .. } => {
                let tag = fields
                    .first()
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        RuntimeError::InvalidArgument(
                            "VDOM struct requires field 0 to be a string 'tag'".into(),
                        )
                    })?
                    .to_string();

                let mut props = BTreeMap::new();
                if let Some(p) = fields.get(1) {
                    match p {
                        Value::Map(pm) => {
                            for (k, v) in pm {
                                let s = match v {
                                    Value::Str(s) => s.clone(),
                                    Value::I64(n) => n.to_string(),
                                    Value::F64(n) => n.to_string(),
                                    Value::Bool(b) => b.to_string(),
                                    other => other.to_string(),
                                };
                                props.insert(k.clone(), s);
                            }
                        }
                        Value::Str(s) if !s.is_empty() => {
                            if s.contains(':') {
                                props.insert("style".to_string(), s.clone());
                            } else {
                                props.insert("class".to_string(), s.clone());
                            }
                        }
                        _ => {}
                    }
                }

                let text = match fields.get(2) {
                    Some(Value::Str(s)) if !s.is_empty() => Some(s.clone()),
                    Some(Value::I64(n)) => Some(n.to_string()),
                    Some(Value::F64(n)) => Some(n.to_string()),
                    Some(Value::Bool(b)) => Some(b.to_string()),
                    _ => None,
                };

                let mut children = Vec::new();

                // Field 3: Vec -> children, Str -> data-knr-action
                if let Some(f3) = fields.get(3) {
                    match f3 {
                        Value::Vec(cv) => {
                            for child in cv {
                                children.push(VNode::from_value(child)?);
                            }
                        }
                        Value::Str(action) if !action.is_empty() => {
                            props.insert("data-knr-action".to_string(), action.clone());
                        }
                        _ => {}
                    }
                }

                // Field 4: Str -> data-knr-target, Vec -> children
                if let Some(f4) = fields.get(4) {
                    match f4 {
                        Value::Str(target) if !target.is_empty() => {
                            props.insert("data-knr-target".to_string(), target.clone());
                        }
                        Value::Vec(cv) => {
                            for child in cv {
                                children.push(VNode::from_value(child)?);
                            }
                        }
                        _ => {}
                    }
                }

                Ok(VNode::Element {
                    tag,
                    props,
                    children,
                    text,
                })
            }
            _ => Err(RuntimeError::InvalidArgument(
                "VDOM node must be a Map, Struct, or String".into(),
            )),
        }
    }

    /// Converts this `VNode` into a `Value` representation.
    pub fn to_value(&self) -> Value {
        match self {
            VNode::Text(s) | VNode::RawHtml(s) => Value::Str(s.clone()),
            VNode::Element {
                tag,
                props,
                children,
                text,
            } => {
                let mut map = BTreeMap::new();
                map.insert("tag".to_string(), Value::Str(tag.clone()));

                let mut pmap = BTreeMap::new();
                for (k, v) in props {
                    pmap.insert(k.clone(), Value::Str(v.clone()));
                }
                map.insert("props".to_string(), Value::Map(pmap));

                if let Some(act) = props.get("data-knr-action") {
                    map.insert("action".to_string(), Value::Str(act.clone()));
                }
                if let Some(tgt) = props.get("data-knr-target") {
                    map.insert("target".to_string(), Value::Str(tgt.clone()));
                }

                if let Some(t) = text {
                    map.insert("text".to_string(), Value::Str(t.clone()));
                }

                let cval: Vec<Value> = children.iter().map(VNode::to_value).collect();
                map.insert("children".to_string(), Value::Vec(cval));

                Value::Map(map)
            }
        }
    }

    /// Recursively checks if any element in the VDOM subtree contains an explicit `<script>` tag.
    pub fn has_explicit_script(&self) -> bool {
        match self {
            VNode::Text(_) => false,
            VNode::RawHtml(s) => s.to_ascii_lowercase().contains("<script"),
            VNode::Element { tag, children, .. } => {
                if tag.eq_ignore_ascii_case("script") {
                    return true;
                }
                children.iter().any(|c| c.has_explicit_script())
            }
        }
    }

    /// Recursively checks if any element in the VDOM subtree has a non-empty `data-knr-action` prop.
    pub fn has_action_attrs(&self) -> bool {
        match self {
            VNode::Text(_) => false,
            VNode::RawHtml(s) => s.contains("data-knr-action"),
            VNode::Element { props, children, .. } => {
                if props.get("data-knr-action").is_some_and(|a| !a.is_empty()) {
                    return true;
                }
                children.iter().any(|c| c.has_action_attrs())
            }
        }
    }

    /// Checks if this VDOM tree requires automatic runtime bridge injection:
    /// returns `true` if any element defines declarative actions (`data-knr-action`)
    /// and NO explicit `<script>` element is present in the tree.
    pub fn has_declarative_actions(&self) -> bool {
        self.has_action_attrs() && !self.has_explicit_script()
    }

    /// Renders this VNode and its subtree to an HTML string.
    /// If declarative actions are present (`data-knr-action`) and no explicit `<script>` tag exists,
    /// automatically injects `<script data-knr-runtime="v1">...</script>` before the root closing tag.
    pub fn render_html(&self) -> String {
        let mut buf = String::new();
        let inject_bridge = self.has_declarative_actions();
        self.render_into(&mut buf, inject_bridge);
        buf
    }

    fn render_into(&self, buf: &mut String, inject_at_root: bool) {
        match self {
            VNode::Text(s) => {
                buf.push_str(&escape_html_text(s));
            }
            VNode::RawHtml(s) => {
                buf.push_str(s);
            }
            VNode::Element {
                tag,
                props,
                children,
                text,
            } => {
                buf.push('<');
                buf.push_str(tag);

                for (k, v) in props {
                    buf.push(' ');
                    buf.push_str(k);
                    buf.push_str("=\"");
                    buf.push_str(&escape_html_attr(v));
                    buf.push('"');
                }

                if is_void_element(tag) && children.is_empty() && text.is_none() && !inject_at_root {
                    buf.push_str(" />");
                } else {
                    buf.push('>');
                    if let Some(t) = text {
                        if (tag.eq_ignore_ascii_case("style")
                            && !t.to_ascii_lowercase().contains("</style"))
                            || (tag.eq_ignore_ascii_case("script")
                                && !t.to_ascii_lowercase().contains("</script"))
                        {
                            buf.push_str(t);
                        } else {
                            buf.push_str(&escape_html_text(t));
                        }
                    }
                    for child in children {
                        child.render_into(buf, false);
                    }
                    if inject_at_root {
                        buf.push_str(KNR_RUNTIME_BRIDGE_SCRIPT);
                    }
                    buf.push_str("</");
                    buf.push_str(tag);
                    buf.push('>');
                }
            }
        }
    }
}

/// UI Platform Adapter managing Virtual DOM state, native SSR rendering, and event dispatch channels.
pub struct UiPlatform {
    mounted_root: Arc<Mutex<Option<VNode>>>,
    mounted_graph: Arc<Mutex<Option<ExecutionGraph>>>,
    event_sender: mpsc::Sender<Value>,
    event_receiver: Arc<Mutex<mpsc::Receiver<Value>>>,
    channel_id: usize,
}

impl Default for UiPlatform {
    fn default() -> Self {
        Self::new()
    }
}

impl UiPlatform {
    /// Creates a new `UiPlatform` with an event channel.
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel(1024);
        Self {
            mounted_root: Arc::new(Mutex::new(None)),
            mounted_graph: Arc::new(Mutex::new(None)),
            event_sender: tx,
            event_receiver: Arc::new(Mutex::new(rx)),
            channel_id: 1,
        }
    }

    /// Mounts a root VNode and returns the event channel ID.
    pub fn mount(&mut self, root: VNode) -> usize {
        *self.mounted_root.lock().unwrap() = Some(root);
        self.channel_id
    }

    /// Mounts an execution graph and returns the event channel ID.
    pub fn mount_graph(&mut self, graph: ExecutionGraph) -> usize {
        let vnode = VNode::from_execution_graph(&graph);
        *self.mounted_graph.lock().unwrap() = Some(graph);
        *self.mounted_root.lock().unwrap() = Some(vnode);
        self.channel_id
    }

    /// Returns the currently mounted root VNode, if any.
    pub fn get_mounted(&self) -> Option<VNode> {
        self.mounted_root.lock().unwrap().clone()
    }

    /// Returns the currently mounted execution graph, if any.
    pub fn get_mounted_graph(&self) -> Option<ExecutionGraph> {
        self.mounted_graph.lock().unwrap().clone()
    }

    /// Renders the currently mounted VDOM tree to an HTML string.
    pub fn render(&self) -> Result<String, RuntimeError> {
        let root = self.mounted_root.lock().unwrap();
        match &*root {
            Some(vnode) => Ok(vnode.render_html()),
            None => Ok(String::new()),
        }
    }

    /// Renders the mounted execution graph to a clean semantic SVG string.
    pub fn render_svg(&self) -> Result<String, RuntimeError> {
        if let Some(graph) = &*self.mounted_graph.lock().unwrap() {
            Ok(graph.render_to_svg())
        } else {
            self.render()
        }
    }

    /// Dispatches a UI event into the platform's event channel.
    pub fn dispatch_event(&self, event: Value) -> Result<(), RuntimeError> {
        self.event_sender
            .try_send(event)
            .map_err(|e| RuntimeError::Channel(format!("Failed to dispatch UI event: {e}")))
    }

    /// Polls for the next UI event from the event channel (non-blocking).
    pub fn poll_event(&self) -> Option<Value> {
        self.event_receiver.lock().unwrap().try_recv().ok()
    }
}

impl HostPlatform for UiPlatform {
    fn host_call(&mut self, effect: Effect, op: &str, payload: Value) -> Result<Value, RuntimeError> {
        if effect != Effect::Ui {
            return Err(RuntimeError::UnsupportedEffect(effect));
        }

        match op {
            "ui.mount" => {
                let node = VNode::from_value(&payload)?;
                let chan_id = self.mount(node);
                Ok(Value::Chan(chan_id))
            }

            "ui.mount_graph" => {
                let graph = ExecutionGraph::new("knr Pipeline Graph");
                let chan_id = self.mount_graph(graph);
                Ok(Value::Chan(chan_id))
            }

            "ui.update" => {
                let node = VNode::from_value(&payload)?;
                *self.mounted_root.lock().unwrap() = Some(node);
                Ok(Value::Bool(true))
            }

            "ui.render" => {
                if payload.is_void() {
                    let html = self.render()?;
                    Ok(Value::Str(html))
                } else if matches!(
                    &payload,
                    Value::Struct { schema, fields } if schema == "UiNode" || fields.len() >= 11
                ) {
                    let ui = crate::gui::UiNode::from_value(&payload)?;
                    Ok(Value::Str(ui.render_document_html("knr")))
                } else {
                    let node = VNode::from_value(&payload)?;
                    Ok(Value::Str(node.render_html()))
                }
            }

            "ui.render_svg" => {
                let svg = self.render_svg()?;
                Ok(Value::Str(svg))
            }

            "ui.step_graph" => {
                if let Some(mut graph) = self.get_mounted_graph() {
                    let step = match payload {
                        Value::I64(n) => n.max(0) as usize,
                        _ => 0,
                    };
                    graph.active_step = step;
                    *self.mounted_graph.lock().unwrap() = Some(graph.clone());
                    *self.mounted_root.lock().unwrap() = Some(VNode::from_execution_graph(&graph));
                    Ok(Value::Bool(true))
                } else {
                    Ok(Value::Bool(false))
                }
            }

            "ui.dispatch_event" => {
                self.dispatch_event(payload)?;
                Ok(Value::Bool(true))
            }

            "ui.poll_event" => {
                if let Some(event) = self.poll_event() {
                    Ok(event)
                } else {
                    Ok(Value::Void)
                }
            }

            _ => Err(RuntimeError::InvalidOperation(format!(
                "Unknown ui operation: {op}"
            ))),
        }
    }
}
