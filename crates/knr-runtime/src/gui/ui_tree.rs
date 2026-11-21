//! Typed Declarative UI Tree, Flexbox Layout Engine, Hit-Testing, and SSR HTML Renderer.
//!
//! Matches the canonical 11-field `.knr` `UiNode` struct layout:
//! ```knr
//! struct UiNode {
//!     0: kind str,         // "col" | "row" | "card" | "text" | "button" | "badge" | "spacer"
//!     1: text str,
//!     2: action str,
//!     3: bg str,
//!     4: color str,
//!     5: border str,
//!     6: radius i64,
//!     7: pad i64,
//!     8: gap i64,
//!     9: font_size i64,
//!     10: children vec[UiNode],
//! }
//! ```

use crate::platform::{RuntimeError, Value};
use crate::ui::{escape_html_attr, escape_html_text};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UiKind {
    Col,
    Row,
    Card,
    Text,
    Button,
    Badge,
    Spacer,
}

impl UiKind {
    pub fn from_str_kind(s: &str) -> Self {
        match s.to_ascii_lowercase().as_str() {
            "row" => UiKind::Row,
            "card" => UiKind::Card,
            "text" | "h1" | "h2" | "p" | "span" => UiKind::Text,
            "button" | "btn" => UiKind::Button,
            "badge" | "pill" => UiKind::Badge,
            "spacer" => UiKind::Spacer,
            _ => UiKind::Col,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            UiKind::Col => "col",
            UiKind::Row => "row",
            UiKind::Card => "card",
            UiKind::Text => "text",
            UiKind::Button => "button",
            UiKind::Badge => "badge",
            UiKind::Spacer => "spacer",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct UiNode {
    pub kind: UiKind,
    pub text: String,
    pub action: String,
    pub bg: String,
    pub color: String,
    pub border: String,
    pub radius: f32,
    pub pad: f32,
    pub gap: f32,
    pub font_size: f32,
    pub children: Vec<UiNode>,
}

impl Default for UiNode {
    fn default() -> Self {
        Self {
            kind: UiKind::Col,
            text: String::new(),
            action: String::new(),
            bg: String::new(),
            color: "#f8fafc".to_string(),
            border: String::new(),
            radius: 0.0,
            pad: 0.0,
            gap: 0.0,
            font_size: 16.0,
            children: Vec::new(),
        }
    }
}

impl UiNode {
    /// Parses a `knr_runtime::Value` (canonical 11-field `UiNode` struct, Map, or Str) into a `UiNode`.
    pub fn from_value(val: &Value) -> Result<Self, RuntimeError> {
        match val {
            Value::Str(s) => Ok(UiNode {
                kind: UiKind::Text,
                text: s.clone(),
                ..Default::default()
            }),
            Value::I64(n) => Ok(UiNode {
                kind: UiKind::Text,
                text: n.to_string(),
                ..Default::default()
            }),
            Value::Struct { fields, .. } if fields.len() >= 11 => {
                let kind = fields[0]
                    .as_str()
                    .map(UiKind::from_str_kind)
                    .unwrap_or(UiKind::Col);
                let text = value_to_string(&fields[1]);
                let action = value_to_string(&fields[2]);
                let bg = value_to_string(&fields[3]);
                let color = value_to_string(&fields[4]);
                let border = value_to_string(&fields[5]);
                let radius = fields[6].as_i64().unwrap_or(0).max(0) as f32;
                let pad = fields[7].as_i64().unwrap_or(0).max(0) as f32;
                let gap = fields[8].as_i64().unwrap_or(0).max(0) as f32;
                let fs_raw = fields[9].as_i64().unwrap_or(0);
                let font_size = if fs_raw > 0 { fs_raw as f32 } else { 16.0 };

                let mut children = Vec::new();
                if let Value::Vec(items) = &fields[10] {
                    for item in items {
                        children.push(UiNode::from_value(item)?);
                    }
                }

                Ok(UiNode {
                    kind,
                    text,
                    action,
                    bg,
                    color,
                    border,
                    radius,
                    pad,
                    gap,
                    font_size,
                    children,
                })
            }
            Value::Struct { fields, .. } if fields.len() >= 3 => {
                // Graceful fallback for legacy 3..5-field VDOM structs (tag, class, text, [action/children])
                let kind = fields[0]
                    .as_str()
                    .map(UiKind::from_str_kind)
                    .unwrap_or(UiKind::Col);
                let text = value_to_string(&fields[2]);
                let mut action = String::new();
                let mut children = Vec::new();
                for f in fields.iter().skip(3) {
                    match f {
                        Value::Str(s) if action.is_empty() && !s.is_empty() => action = s.clone(),
                        Value::Vec(items) => {
                            for item in items {
                                children.push(UiNode::from_value(item)?);
                            }
                        }
                        _ => {}
                    }
                }
                Ok(UiNode {
                    kind,
                    text,
                    action,
                    pad: 12.0,
                    gap: 8.0,
                    ..Default::default()
                })
            }
            Value::Map(m) => {
                let kind = m
                    .get("kind")
                    .or_else(|| m.get("tag"))
                    .and_then(Value::as_str)
                    .map(UiKind::from_str_kind)
                    .unwrap_or(UiKind::Col);
                let text = m.get("text").map(value_to_string).unwrap_or_default();
                let action = m.get("action").map(value_to_string).unwrap_or_default();
                let bg = m.get("bg").map(value_to_string).unwrap_or_default();
                let color = m.get("color").map(value_to_string).unwrap_or_default();
                let border = m.get("border").map(value_to_string).unwrap_or_default();
                let radius = m.get("radius").and_then(Value::as_i64).unwrap_or(0) as f32;
                let pad = m.get("pad").and_then(Value::as_i64).unwrap_or(0) as f32;
                let gap = m.get("gap").and_then(Value::as_i64).unwrap_or(0) as f32;
                let fs = m.get("font_size").and_then(Value::as_i64).unwrap_or(16) as f32;
                let mut children = Vec::new();
                if let Some(Value::Vec(items)) = m.get("children") {
                    for item in items {
                        children.push(UiNode::from_value(item)?);
                    }
                }
                Ok(UiNode {
                    kind,
                    text,
                    action,
                    bg,
                    color,
                    border,
                    radius,
                    pad,
                    gap,
                    font_size: if fs > 0.0 { fs } else { 16.0 },
                    children,
                })
            }
            other => Err(RuntimeError::InvalidArgument(format!(
                "Cannot convert {other:?} into UiNode"
            ))),
        }
    }

    /// Renders this `UiNode` tree to clean semantic HTML5 with matching Flexbox inline styles.
    pub fn render_html(&self) -> String {
        let mut out = String::new();
        self.write_html(&mut out);
        out
    }

    /// Renders this `UiNode` tree into a complete responsive dark-mode HTML5 document
    /// with a generic SSR action bridge (clicks on `[data-knr-action]` POST to the server
    /// and hot-swap `#knr-app` seamlessly without full-page reload).
    pub fn render_document_html(&self, title: &str) -> String {
        let escaped_title = escape_html_text(title);
        let body_html = self.render_html();
        format!(
            r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8" />
<meta name="viewport" content="width=device-width, initial-scale=1" />
<title>{escaped_title}</title>
<style>
  *, *::before, *::after {{ box-sizing: border-box; margin: 0; padding: 0; }}
  html, body {{
    min-height: 100vh;
    background: radial-gradient(ellipse 80% 50% at 50% -15%, rgba(56, 189, 248, 0.16), transparent), #0b1120;
    color: #f8fafc;
    font-family: Inter, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif;
    -webkit-font-smoothing: antialiased;
  }}
  #knr-app {{ max-width: 780px; margin: 0 auto; padding: 24px 16px; }}
  button {{ transition: filter 120ms ease, transform 80ms ease; }}
  button:hover {{ filter: brightness(1.14); }}
  button:active {{ transform: scale(0.97); }}
</style>
</head>
<body>
<div id="knr-app">{body_html}</div>
<script data-knr-ssr="v1">
(function() {{
  const root = document.getElementById("knr-app");
  if (!root) return;
  root.addEventListener("click", async function(e) {{
    const btn = e.target && e.target.closest ? e.target.closest("[data-knr-action]") : null;
    if (!btn) return;
    const act = btn.getAttribute("data-knr-action");
    if (!act) return;
    try {{
      const res = await fetch(act, {{ method: "POST" }});
      if (!res.ok) return;
      const html = await res.text();
      const doc = new DOMParser().parseFromString(html, "text/html");
      const next = doc.getElementById("knr-app");
      if (next) {{
        root.innerHTML = next.innerHTML;
      }} else {{
        root.innerHTML = html;
      }}
    }} catch (err) {{
      console.error("knr SSR action failed:", err);
    }}
  }});
}})();
</script>
</body>
</html>"#
        )
    }

    fn write_html(&self, out: &mut String) {
        let tag = match self.kind {
            UiKind::Button => "button",
            UiKind::Badge => "span",
            UiKind::Text => "div",
            _ => "div",
        };

        let mut styles: Vec<String> = Vec::new();
        match self.kind {
            UiKind::Col | UiKind::Card => {
                styles.push("display:flex".into());
                styles.push("flex-direction:column".into());
                styles.push("box-sizing:border-box".into());
            }
            UiKind::Row => {
                styles.push("display:flex".into());
                styles.push("flex-direction:row".into());
                styles.push("flex-wrap:wrap".into());
                styles.push("align-items:center".into());
                styles.push("box-sizing:border-box".into());
            }
            UiKind::Button => {
                styles.push("display:inline-flex".into());
                styles.push("align-items:center".into());
                styles.push("justify-content:center".into());
                styles.push("cursor:pointer".into());
                styles.push("font-weight:600".into());
                styles.push("font-family:inherit".into());
                styles.push("transition:filter 120ms ease,transform 80ms ease".into());
                if self.border.is_empty() {
                    styles.push("border:none".into());
                }
            }
            UiKind::Badge => {
                styles.push("display:inline-flex".into());
                styles.push("align-items:center".into());
                styles.push("align-self:flex-start".into());
                styles.push("font-weight:600".into());
            }
            UiKind::Text => {
                styles.push("line-height:1.35".into());
            }
            UiKind::Spacer => {
                styles.push(format!("min-height:{}px", self.pad.max(8.0) as i32));
            }
        }

        if !self.bg.is_empty() {
            styles.push(format!("background:{}", escape_html_attr(&self.bg)));
        }
        if !self.color.is_empty() {
            styles.push(format!("color:{}", escape_html_attr(&self.color)));
        }
        if !self.border.is_empty() {
            styles.push(format!("border:1px solid {}", escape_html_attr(&self.border)));
        }
        if self.radius > 0.0 {
            styles.push(format!("border-radius:{}px", self.radius as i32));
        }
        if self.pad > 0.0 {
            if matches!(self.kind, UiKind::Button | UiKind::Badge) {
                let pv = (self.pad * 0.6).max(6.0) as i32;
                let ph = (self.pad * 1.3).max(12.0) as i32;
                styles.push(format!("padding:{pv}px {ph}px"));
            } else {
                styles.push(format!("padding:{}px", self.pad as i32));
            }
        }
        if self.gap > 0.0 && matches!(self.kind, UiKind::Col | UiKind::Row | UiKind::Card) {
            styles.push(format!("gap:{}px", self.gap as i32));
        }
        if self.font_size > 0.0 {
            styles.push(format!("font-size:{}px", self.font_size as i32));
        }

        out.push('<');
        out.push_str(tag);
        if !styles.is_empty() {
            out.push_str(" style=\"");
            out.push_str(&styles.join(";"));
            out.push('"');
        }
        if !self.action.is_empty() {
            out.push_str(" data-knr-action=\"");
            out.push_str(&escape_html_attr(&self.action));
            out.push('"');
        }
        out.push('>');

        if !self.text.is_empty() {
            out.push_str(&escape_html_text(&self.text));
        }
        for child in &self.children {
            child.write_html(out);
        }

        out.push_str("</");
        out.push_str(tag);
        out.push('>');
    }
}

fn value_to_string(v: &Value) -> String {
    match v {
        Value::Str(s) => s.clone(),
        Value::I64(n) => n.to_string(),
        Value::F64(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        _ => String::new(),
    }
}

/// Floating-point bounding box produced by the Flexbox layout solver.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LayoutRect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl LayoutRect {
    pub fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }

    pub fn contains(&self, px: f32, py: f32) -> bool {
        px >= self.x && px <= self.x + self.w && py >= self.y && py <= self.y + self.h
    }
}

/// Interactive hit-testing target collected during layout.
#[derive(Debug, Clone, PartialEq)]
pub struct HitBox {
    pub rect: LayoutRect,
    pub action: String,
}

/// Laid-out UI tree node with exact screen-space coordinates.
#[derive(Debug, Clone)]
pub struct LayoutNode {
    pub rect: LayoutRect,
    pub node: UiNode,
    pub children: Vec<LayoutNode>,
}

/// Trait abstracting text measurement so layout works identically with `ab_glyph` or bitmap fonts.
pub trait TextMeasurer {
    fn measure_text_size(&self, text: &str, font_size: f32) -> (f32, f32);
}

/// Computes a full two-pass Flexbox layout for `root` inside `(viewport_w, viewport_h)`,
/// returning the positioned `LayoutNode` tree and interactive `HitBox` list.
pub fn compute_ui_layout<M: TextMeasurer>(
    root: &UiNode,
    viewport_w: f32,
    viewport_h: f32,
    measurer: &M,
) -> (LayoutNode, Vec<HitBox>) {
    let mut hit_boxes = Vec::new();
    let laid_out = layout_node(
        root,
        0.0,
        0.0,
        viewport_w,
        Some(viewport_h),
        measurer,
        &mut hit_boxes,
    );
    (laid_out, hit_boxes)
}

fn measure_intrinsic<M: TextMeasurer>(node: &UiNode, max_w: f32, measurer: &M) -> (f32, f32) {
    let pad = node.pad;
    let gap = node.gap;
    match node.kind {
        UiKind::Text => {
            let (tw, th) = measurer.measure_text_size(&node.text, node.font_size);
            ((tw + pad * 2.0).min(max_w.max(tw + pad * 2.0)), th + pad * 2.0)
        }
        UiKind::Button | UiKind::Badge => {
            let (tw, th) = measurer.measure_text_size(&node.text, node.font_size);
            let pv = (pad * 0.6).max(6.0);
            let ph = (pad * 1.3).max(12.0);
            (tw + ph * 2.0, th + pv * 2.0)
        }
        UiKind::Spacer => (pad.max(8.0), pad.max(8.0)),
        UiKind::Col | UiKind::Card => {
            let inner_max = (max_w - pad * 2.0).max(40.0);
            let mut max_child_w: f32 = 0.0;
            let mut total_h: f32 = 0.0;
            if !node.text.is_empty() {
                let (tw, th) = measurer.measure_text_size(&node.text, node.font_size);
                max_child_w = max_child_w.max(tw);
                total_h += th;
                if !node.children.is_empty() {
                    total_h += gap;
                }
            }
            for (idx, child) in node.children.iter().enumerate() {
                let (cw, ch) = measure_intrinsic(child, inner_max, measurer);
                max_child_w = max_child_w.max(cw);
                total_h += ch;
                if idx + 1 < node.children.len() {
                    total_h += gap;
                }
            }
            let w = if matches!(node.kind, UiKind::Card | UiKind::Col) {
                max_w.max(max_child_w + pad * 2.0)
            } else {
                max_child_w + pad * 2.0
            };
            (w.min(max_w), total_h + pad * 2.0)
        }
        UiKind::Row => {
            let inner_max = (max_w - pad * 2.0).max(40.0);
            let mut row_x: f32 = 0.0;
            let mut row_h: f32 = 0.0;
            let mut total_h: f32 = 0.0;
            let mut max_row_w: f32 = 0.0;

            for child in &node.children {
                let (cw, ch) = measure_intrinsic(child, inner_max, measurer);
                if row_x > 0.0 && row_x + gap + cw > inner_max {
                    // Wrap to next line
                    max_row_w = max_row_w.max(row_x);
                    total_h += row_h + gap;
                    row_x = cw;
                    row_h = ch;
                } else {
                    if row_x > 0.0 {
                        row_x += gap;
                    }
                    row_x += cw;
                    row_h = row_h.max(ch);
                }
            }
            max_row_w = max_row_w.max(row_x);
            total_h += row_h;
            (max_row_w + pad * 2.0, total_h + pad * 2.0)
        }
    }
}

fn layout_node<M: TextMeasurer>(
    node: &UiNode,
    x: f32,
    y: f32,
    avail_w: f32,
    min_h: Option<f32>,
    measurer: &M,
    hit_boxes: &mut Vec<HitBox>,
) -> LayoutNode {
    let pad = node.pad;
    let gap = node.gap;

    match node.kind {
        UiKind::Text | UiKind::Button | UiKind::Badge | UiKind::Spacer => {
            let (iw, ih) = measure_intrinsic(node, avail_w, measurer);
            let w = if matches!(node.kind, UiKind::Text) {
                iw.min(avail_w)
            } else {
                iw
            };
            let h = min_h.map(|m| ih.max(m)).unwrap_or(ih);
            let rect = LayoutRect::new(x, y, w, h);
            if !node.action.is_empty() {
                hit_boxes.push(HitBox {
                    rect,
                    action: node.action.clone(),
                });
            }
            LayoutNode {
                rect,
                node: node.clone(),
                children: Vec::new(),
            }
        }
        UiKind::Col | UiKind::Card => {
            let inner_w = (avail_w - pad * 2.0).max(20.0);
            let mut cur_y = y + pad;
            if !node.text.is_empty() {
                let (_, th) = measurer.measure_text_size(&node.text, node.font_size);
                cur_y += th + if node.children.is_empty() { 0.0 } else { gap };
            }
            let mut laid_children = Vec::with_capacity(node.children.len());
            for (idx, child) in node.children.iter().enumerate() {
                let child_avail = inner_w;
                let laid = layout_node(child, x + pad, cur_y, child_avail, None, measurer, hit_boxes);
                cur_y += laid.rect.h;
                if idx + 1 < node.children.len() {
                    cur_y += gap;
                }
                laid_children.push(laid);
            }
            let content_h = (cur_y - y) + pad;
            let h = min_h.map(|m| content_h.max(m)).unwrap_or(content_h);
            let rect = LayoutRect::new(x, y, avail_w, h);
            if !node.action.is_empty() {
                hit_boxes.push(HitBox {
                    rect,
                    action: node.action.clone(),
                });
            }
            LayoutNode {
                rect,
                node: node.clone(),
                children: laid_children,
            }
        }
        UiKind::Row => {
            let inner_w = (avail_w - pad * 2.0).max(20.0);
            let mut cur_x = x + pad;
            let mut cur_y = y + pad;
            let mut line_h: f32 = 0.0;
            let mut laid_children = Vec::with_capacity(node.children.len());

            for child in &node.children {
                let (cw, ch) = measure_intrinsic(child, inner_w, measurer);
                let child_w = match child.kind {
                    UiKind::Card | UiKind::Col => cw.min(inner_w),
                    _ => cw,
                };
                if cur_x > x + pad && (cur_x - (x + pad)) + child_w > inner_w {
                    cur_x = x + pad;
                    cur_y += line_h + gap;
                    line_h = 0.0;
                }
                let laid = layout_node(child, cur_x, cur_y, child_w, None, measurer, hit_boxes);
                cur_x += laid.rect.w + gap;
                line_h = line_h.max(laid.rect.h.max(ch));
                laid_children.push(laid);
            }

            let content_h = (cur_y - y) + line_h + pad;
            let h = min_h.map(|m| content_h.max(m)).unwrap_or(content_h);
            let rect = LayoutRect::new(x, y, avail_w, h);
            if !node.action.is_empty() {
                hit_boxes.push(HitBox {
                    rect,
                    action: node.action.clone(),
                });
            }
            LayoutNode {
                rect,
                node: node.clone(),
                children: laid_children,
            }
        }
    }
}

/// Finds the topmost hit-box containing `(x, y)`.
pub fn hit_test(hit_boxes: &[HitBox], x: f32, y: f32) -> Option<&str> {
    hit_boxes
        .iter()
        .rev()
        .find(|hb| hb.rect.contains(x, y))
        .map(|hb| hb.action.as_str())
}
