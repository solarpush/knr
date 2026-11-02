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

                let text = m.get("text").and_then(Value::as_str).map(|s| s.to_string());

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
                if let Some(Value::Vec(cv)) = fields.get(3) {
                    for child in cv {
                        children.push(VNode::from_value(child)?);
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
            VNode::Text(s) => Value::Str(s.clone()),
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

                if let Some(t) = text {
                    map.insert("text".to_string(), Value::Str(t.clone()));
                }

                let cval: Vec<Value> = children.iter().map(VNode::to_value).collect();
                map.insert("children".to_string(), Value::Vec(cval));

                Value::Map(map)
            }
        }
    }

    /// Renders this VNode and its subtree to an HTML string.
    pub fn render_html(&self) -> String {
        let mut buf = String::new();
        self.render_into(&mut buf);
        buf
    }

    fn render_into(&self, buf: &mut String) {
        match self {
            VNode::Text(s) => {
                buf.push_str(&escape_html_text(s));
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

                if is_void_element(tag) && children.is_empty() && text.is_none() {
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
                        child.render_into(buf);
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

    /// Returns the currently mounted root VNode, if any.
    pub fn get_mounted(&self) -> Option<VNode> {
        self.mounted_root.lock().unwrap().clone()
    }

    /// Renders the currently mounted VDOM tree to an HTML string.
    pub fn render(&self) -> Result<String, RuntimeError> {
        let root = self.mounted_root.lock().unwrap();
        match &*root {
            Some(vnode) => Ok(vnode.render_html()),
            None => Ok(String::new()),
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

            "ui.update" => {
                let node = VNode::from_value(&payload)?;
                *self.mounted_root.lock().unwrap() = Some(node);
                Ok(Value::Bool(true))
            }

            "ui.render" => {
                if payload.is_void() {
                    let html = self.render()?;
                    Ok(Value::Str(html))
                } else {
                    let node = VNode::from_value(&payload)?;
                    Ok(Value::Str(node.render_html()))
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
