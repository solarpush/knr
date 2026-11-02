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

/// Standard knr runtime bridge script with dual SSR (fetch POST) and WASM Client Reducer support.
pub const KNR_RUNTIME_BRIDGE_SCRIPT: &str = "<script data-knr-runtime=\"v1\">\
(function(){\
let w=window.__knr_wasm;\
const B64='AGFzbQEAAAABBwFgAn5+AX4DAwIAAAUDAQABByYDBm1lbW9yeQIACmFwcGx5X3N0ZXAAAAxrbnJfZGlzcGF0Y2gAAQp4Ag4BAX4gACABfCECIAIPC2cCBn4Cf0EAIQkDQCAJQQBGBEBCACECIAEgAlEhCCAIBEBBASEJBUECIQkLDAELIAlBAUYEQCACDwsgCUECRgRAQgMhA0ICIQQgASAEfiEFIAMgBX0hBiAAIAYQACEHIAcPCwwACwAL';\
async function getWasm(){\
if(w)return w;\
if(typeof location!=='undefined'&&location.protocol&&location.protocol.startsWith('http')){\
try{\
const r=await fetch('site.wasm');\
if(r.ok){\
const b=await r.arrayBuffer();\
const u=new Uint8Array(b);\
if(u.length>4&&u[0]===0&&u[1]===97&&u[2]===115&&u[3]===109){\
const m=await WebAssembly.instantiate(b,{});\
w=window.__knr_wasm=m.instance.exports;\
return w;\
}\
}\
}catch(_){}\
}\
try{\
const bin=atob(window.__knr_wasm_bytes||B64);\
const u=new Uint8Array(bin.length);\
for(let i=0;i<bin.length;i++)u[i]=bin.charCodeAt(i);\
const m=await WebAssembly.instantiate(u.buffer,{});\
w=window.__knr_wasm=m.instance.exports;\
return w;\
}catch(_){}\
return null;\
}\
document.addEventListener('click',async function(e){\
const b=e.target.closest('[data-knr-action]');\
if(!b)return;\
const act=b.getAttribute('data-knr-action');\
if(!act)return;\
const tgt=b.getAttribute('data-knr-target')||'.counter-val';\
const el=document.querySelector(tgt);\
const box=b.closest('.counter-box,.wasm-box')||b.parentElement||document;\
const st=box.querySelector('.counter-status')||document.querySelector('.counter-status');\
const cur=BigInt(parseInt((el?el.textContent:'0')||'0',10)||0);\
if(act.startsWith('/')&&typeof location!=='undefined'&&location.protocol&&location.protocol.startsWith('http')){\
try{\
const r=await fetch(act,{method:'POST'});\
if(r.ok){\
const t=(await r.text()).trim();\
if(act.startsWith('/nav/')||/^-?\\d+$/.test(t)){\
if(el)el.textContent=t;\
if(st)st.textContent=act.startsWith('/nav/')?('Routeur Multi-Pages SSR (site/server.ssa) -> route '+act):('Syscall Net + Ui SSR (site/server.ssa) -> reg muté en VM (val = '+t+')');\
return;\
}\
}\
}catch(_){}\
}\
if(act.startsWith('/nav/')){\
const p=act.slice(5);\
const all=['home','pillars','learn','isa','examples'];\
const hide=p==='all'?[]:all.filter(x=>x!==p).map(x=>'.page-'+x);\
const css=(hide.length?hide.join(',')+'{display:none!important}':'')+'.tab-'+p+'{background:#38bdf8!important;color:#050811!important;border-color:#38bdf8!important}';\
if(el)el.textContent=css;\
if(st)st.textContent='Routeur VDOM Client -> vue active : '+p;\
return;\
}\
const op=act.endsWith('inc')?1n:(act.endsWith('dec')?2n:0n);\
try{\
const wasm=await getWasm();\
if(wasm&&typeof wasm.knr_dispatch==='function'){\
const next=wasm.knr_dispatch(cur,op);\
if(el)el.textContent=next.toString();\
if(st)st.textContent='WASM Client Reducer (site.wasm -> knr_dispatch('+cur+'n, '+op+'n)) = '+next;\
return;\
}\
}catch(_){}\
const next=op===1n?cur+1n:(op===2n?cur-1n:0n);\
if(el)el.textContent=next.toString();\
if(st)st.textContent='Bridge VDOM local -> struct_set %state 0 '+next+' (0 allocation GC)';\
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
