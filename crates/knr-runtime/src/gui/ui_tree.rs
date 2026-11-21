//! Unconstrained Typed Declarative UI Tree, Flexbox + CSS Grid + Stack Layout Engine,
//! Interactive Hit-Testing (Buttons, TextInputs, Sliders, Toggles, ScrollViews, Shaders),
//! and Semantic HTML5/CSS Renderer.
//!
//! Supports BOTH:
//! - Canonical 8-field `Widget { key, kind, text, action, value, uniforms, style: Style(40), children }`
//! - Backward-compatible 11-field `UiNode` & legacy 3..5-field VDOM structs.

use crate::platform::{RuntimeError, Value};
use crate::ui::{escape_html_attr, escape_html_text};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UiKind {
    Col,
    Row,
    Grid,
    Stack,
    Card,
    Text,
    Button,
    Badge,
    Input,
    Slider,
    Toggle,
    Scroll,
    Image,
    Shader,
    Spacer,
}

impl UiKind {
    pub fn from_str_kind(s: &str) -> Self {
        match s.to_ascii_lowercase().as_str() {
            "row" => UiKind::Row,
            "grid" => UiKind::Grid,
            "stack" | "zstack" => UiKind::Stack,
            "card" | "box" => UiKind::Card,
            "text" | "h1" | "h2" | "p" | "span" => UiKind::Text,
            "button" | "btn" => UiKind::Button,
            "badge" | "pill" => UiKind::Badge,
            "input" | "textinput" | "text_input" => UiKind::Input,
            "slider" | "range" => UiKind::Slider,
            "toggle" | "switch" | "checkbox" => UiKind::Toggle,
            "scroll" | "scrollview" => UiKind::Scroll,
            "image" | "img" => UiKind::Image,
            "shader" | "shaderview" | "canvas" => UiKind::Shader,
            "spacer" => UiKind::Spacer,
            _ => UiKind::Col,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            UiKind::Col => "col",
            UiKind::Row => "row",
            UiKind::Grid => "grid",
            UiKind::Stack => "stack",
            UiKind::Card => "card",
            UiKind::Text => "text",
            UiKind::Button => "button",
            UiKind::Badge => "badge",
            UiKind::Input => "input",
            UiKind::Slider => "slider",
            UiKind::Toggle => "toggle",
            UiKind::Scroll => "scroll",
            UiKind::Image => "image",
            UiKind::Shader => "shader",
            UiKind::Spacer => "spacer",
        }
    }
}

/// Parsed fill descriptor: solid hex color, linear gradient, or radial gradient.
#[derive(Debug, Clone, PartialEq)]
pub enum ParsedFill {
    None,
    Solid([f32; 4]),
    LinearGradient {
        angle_deg: f32,
        start: [f32; 4],
        end: [f32; 4],
    },
    RadialGradient {
        center: [f32; 4],
        edge: [f32; 4],
    },
}

/// Parses `#RGB`, `#RRGGBB`, `#RRGGBBAA` into normalized `[r, g, b, a]` in `0.0..=1.0`.
pub fn parse_hex_rgba_f32(hex: &str) -> Option<[f32; 4]> {
    let s = hex.trim().strip_prefix('#')?;
    let (r, g, b, a) = match s.len() {
        3 => (
            u8::from_str_radix(&s[0..1], 16).ok()? * 17,
            u8::from_str_radix(&s[1..2], 16).ok()? * 17,
            u8::from_str_radix(&s[2..3], 16).ok()? * 17,
            255u8,
        ),
        6 => (
            u8::from_str_radix(&s[0..2], 16).ok()?,
            u8::from_str_radix(&s[2..4], 16).ok()?,
            u8::from_str_radix(&s[4..6], 16).ok()?,
            255u8,
        ),
        8 => (
            u8::from_str_radix(&s[0..2], 16).ok()?,
            u8::from_str_radix(&s[2..4], 16).ok()?,
            u8::from_str_radix(&s[4..6], 16).ok()?,
            u8::from_str_radix(&s[6..8], 16).ok()?,
        ),
        _ => return None,
    };
    Some([
        r as f32 / 255.0,
        g as f32 / 255.0,
        b as f32 / 255.0,
        a as f32 / 255.0,
    ])
}

/// Parses `"#rrggbb"`, `"linear(135, #a, #b)"`, or `"radial(#a, #b)"` into a `ParsedFill`.
pub fn parse_fill_spec(spec: &str) -> ParsedFill {
    let trimmed = spec.trim();
    if trimmed.is_empty() {
        return ParsedFill::None;
    }
    if let Some(rgba) = parse_hex_rgba_f32(trimmed) {
        return ParsedFill::Solid(rgba);
    }
    if let Some(inner) = trimmed
        .strip_prefix("linear(")
        .and_then(|s| s.strip_suffix(')'))
    {
        let parts: Vec<&str> = inner.split(',').map(str::trim).collect();
        if parts.len() >= 3 {
            let angle = parts[0].trim_end_matches("deg").parse::<f32>().unwrap_or(135.0);
            let start = parse_hex_rgba_f32(parts[1]).unwrap_or([0.07, 0.1, 0.18, 1.0]);
            let end = parse_hex_rgba_f32(parts[2]).unwrap_or([0.12, 0.16, 0.28, 1.0]);
            return ParsedFill::LinearGradient {
                angle_deg: angle,
                start,
                end,
            };
        }
    }
    if let Some(inner) = trimmed
        .strip_prefix("radial(")
        .and_then(|s| s.strip_suffix(')'))
    {
        let parts: Vec<&str> = inner.split(',').map(str::trim).collect();
        if parts.len() >= 2 {
            let center = parse_hex_rgba_f32(parts[0]).unwrap_or([0.22, 0.74, 0.97, 0.35]);
            let edge = parse_hex_rgba_f32(parts[1]).unwrap_or([0.04, 0.07, 0.13, 1.0]);
            return ParsedFill::RadialGradient { center, edge };
        }
    }
    ParsedFill::None
}

/// Converts a knr fill spec into valid CSS `background`.
pub fn fill_to_css(spec: &str) -> Option<String> {
    let trimmed = spec.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.starts_with('#') {
        return Some(escape_html_attr(trimmed));
    }
    if let Some(inner) = trimmed
        .strip_prefix("linear(")
        .and_then(|s| s.strip_suffix(')'))
    {
        let parts: Vec<&str> = inner.split(',').map(str::trim).collect();
        if parts.len() >= 3 {
            let angle = parts[0].trim_end_matches("deg");
            return Some(format!(
                "linear-gradient({}deg, {}, {})",
                escape_html_attr(angle),
                escape_html_attr(parts[1]),
                escape_html_attr(parts[2])
            ));
        }
    }
    if let Some(inner) = trimmed
        .strip_prefix("radial(")
        .and_then(|s| s.strip_suffix(')'))
    {
        let parts: Vec<&str> = inner.split(',').map(str::trim).collect();
        if parts.len() >= 2 {
            return Some(format!(
                "radial-gradient(circle, {}, {})",
                escape_html_attr(parts[0]),
                escape_html_attr(parts[1])
            ));
        }
    }
    Some(escape_html_attr(trimmed))
}

/// Full unconstrained style descriptor matching the 40-field `Style` schema in `.knr`.
#[derive(Debug, Clone, PartialEq)]
pub struct UiStyle {
    pub w: f32,            // >0 px, 0 auto, <0 fill (100%)
    pub h: f32,            // >0 px, 0 auto, <0 fill (100%)
    pub min_w: f32,
    pub max_w: f32,
    pub border_w: f32,
    pub radius_corners: [f32; 4], // [tl, tr, br, bl]
    pub pad_edges: [f32; 4],      // [top, right, bottom, left]
    pub gap_x: f32,
    pub gap_y: f32,
    pub font_weight: i64,
    pub align: String,
    pub justify: String,
    pub grid_cols: i64,
    pub shadow_x: f32,
    pub shadow_y: f32,
    pub shadow_blur: f32,
    pub shadow_color: String,
    pub backdrop_blur: f32,
    pub opacity: f32,
    pub translate_x: f32,
    pub translate_y: f32,
    pub scale: f32,
    pub rotate_deg: f32,
    pub hover_bg: String,
    pub hover_border: String,
    pub hover_scale: f32,
    pub transition_ms: f32,
    pub overflow: String,
    pub z_index: i64,
    pub grow: f32,
}

impl Default for UiStyle {
    fn default() -> Self {
        Self {
            w: 0.0,
            h: 0.0,
            min_w: 0.0,
            max_w: 0.0,
            border_w: 1.0,
            radius_corners: [0.0; 4],
            pad_edges: [0.0; 4],
            gap_x: 0.0,
            gap_y: 0.0,
            font_weight: 400,
            align: "stretch".to_string(),
            justify: "start".to_string(),
            grid_cols: 0,
            shadow_x: 0.0,
            shadow_y: 0.0,
            shadow_blur: 0.0,
            shadow_color: String::new(),
            backdrop_blur: 0.0,
            opacity: 1.0,
            translate_x: 0.0,
            translate_y: 0.0,
            scale: 1.0,
            rotate_deg: 0.0,
            hover_bg: String::new(),
            hover_border: String::new(),
            hover_scale: 1.0,
            transition_ms: 0.0,
            overflow: "visible".to_string(),
            z_index: 0,
            grow: 0.0,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct UiNode {
    pub key: String,
    pub kind: UiKind,
    pub text: String,
    pub action: String,
    pub value: f64,
    pub uniforms: Vec<f64>,
    pub bg: String,
    pub color: String,
    pub border: String,
    pub radius: f32,
    pub pad: f32,
    pub gap: f32,
    pub font_size: f32,
    pub style: UiStyle,
    pub children: Vec<UiNode>,
}

impl Default for UiNode {
    fn default() -> Self {
        Self {
            key: String::new(),
            kind: UiKind::Col,
            text: String::new(),
            action: String::new(),
            value: 0.0,
            uniforms: Vec::new(),
            bg: String::new(),
            color: "#f8fafc".to_string(),
            border: String::new(),
            radius: 0.0,
            pad: 0.0,
            gap: 0.0,
            font_size: 16.0,
            style: UiStyle::default(),
            children: Vec::new(),
        }
    }
}

impl UiNode {
    /// Returns uniform or per-corner radii `[tl, tr, br, bl]`.
    pub fn effective_radii(&self) -> [f32; 4] {
        let c = self.style.radius_corners;
        if c[0] > 0.0 || c[1] > 0.0 || c[2] > 0.0 || c[3] > 0.0 {
            c
        } else {
            [self.radius; 4]
        }
    }

    /// Returns `[top, right, bottom, left]` padding.
    pub fn effective_pad(&self) -> [f32; 4] {
        let p = self.style.pad_edges;
        if p[0] > 0.0 || p[1] > 0.0 || p[2] > 0.0 || p[3] > 0.0 {
            p
        } else {
            [self.pad; 4]
        }
    }

    pub fn effective_gap_x(&self) -> f32 {
        if self.style.gap_x > 0.0 {
            self.style.gap_x
        } else {
            self.gap
        }
    }

    pub fn effective_gap_y(&self) -> f32 {
        if self.style.gap_y > 0.0 {
            self.style.gap_y
        } else {
            self.gap
        }
    }

    /// Parses a `knr_runtime::Value` into a `UiNode`.
    /// Supports:
    /// 1. New 8-field `Widget` struct (`schema == "Widget"` with nested 40-field `Style`)
    /// 2. Legacy 11-field `UiNode` struct (100% backward compatible!)
    /// 3. Legacy 3..5-field VDOM structs / `Value::Map` / scalars.
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
            Value::Struct { schema, fields } if schema == "Widget" && fields.len() >= 8 => {
                let key = value_to_string(&fields[0]);
                let kind = fields[1]
                    .as_str()
                    .map(UiKind::from_str_kind)
                    .unwrap_or(UiKind::Col);
                let text = value_to_string(&fields[2]);
                let action = value_to_string(&fields[3]);
                let value = value_to_f64(&fields[4]);

                let mut uniforms = Vec::new();
                if let Value::Vec(items) = &fields[5] {
                    for it in items {
                        uniforms.push(value_to_f64(it));
                    }
                }

                let (bg, color, border, font_size, style) = parse_style_value(&fields[6]);
                let mut children = Vec::new();
                if let Value::Vec(items) = &fields[7] {
                    for item in items {
                        children.push(UiNode::from_value(item)?);
                    }
                }

                let radius = style.radius_corners[0];
                let pad = style.pad_edges[0];
                let gap = style.gap_y.max(style.gap_x);

                Ok(UiNode {
                    key,
                    kind,
                    text,
                    action,
                    value,
                    uniforms,
                    bg,
                    color,
                    border,
                    radius,
                    pad,
                    gap,
                    font_size,
                    style,
                    children,
                })
            }
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

                let style = UiStyle {
                    radius_corners: [radius; 4],
                    pad_edges: [pad; 4],
                    gap_x: gap,
                    gap_y: gap,
                    ..UiStyle::default()
                };

                Ok(UiNode {
                    key: action.clone(),
                    kind,
                    text,
                    action,
                    value: 0.0,
                    uniforms: Vec::new(),
                    bg,
                    color,
                    border,
                    radius,
                    pad,
                    gap,
                    font_size,
                    style,
                    children,
                })
            }
            Value::Struct { fields, .. } if fields.len() >= 3 => {
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
                    ..Default::default()
                })
            }
            other => Err(RuntimeError::InvalidArgument(format!(
                "Cannot convert {other:?} into UiNode"
            ))),
        }
    }

    /// Returns true if this tree contains any `UiKind::Shader` node.
    pub fn contains_shader(&self) -> bool {
        self.kind == UiKind::Shader || self.children.iter().any(UiNode::contains_shader)
    }

    /// Renders this `UiNode` tree to clean semantic HTML5 with full Flexbox/Grid/Shader/Transform CSS.
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
  #knr-app {{ max-width: 960px; margin: 0 auto; padding: 24px 16px; }}
  button {{ transition: filter 120ms ease, transform 120ms ease; }}
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
            UiKind::Button | UiKind::Toggle => "button",
            UiKind::Badge => "span",
            UiKind::Input => "input",
            UiKind::Slider => "input",
            UiKind::Image => "img",
            UiKind::Shader => "canvas",
            _ => "div",
        };

        let mut styles: Vec<String> = Vec::new();
        let st = &self.style;

        match self.kind {
            UiKind::Col | UiKind::Card | UiKind::Scroll => {
                styles.push("display:flex".into());
                styles.push("flex-direction:column".into());
                styles.push("box-sizing:border-box".into());
                if self.kind == UiKind::Scroll {
                    styles.push("overflow-y:auto".into());
                }
            }
            UiKind::Row => {
                styles.push("display:flex".into());
                styles.push("flex-direction:row".into());
                styles.push("flex-wrap:wrap".into());
                styles.push("align-items:center".into());
                styles.push("box-sizing:border-box".into());
            }
            UiKind::Grid => {
                let cols = st.grid_cols.max(1);
                styles.push("display:grid".into());
                styles.push(format!("grid-template-columns:repeat({cols},minmax(0,1fr))"));
                styles.push("box-sizing:border-box".into());
            }
            UiKind::Stack => {
                styles.push("position:relative".into());
                styles.push("display:grid".into());
                styles.push("box-sizing:border-box".into());
            }
            UiKind::Button => {
                styles.push("display:inline-flex".into());
                styles.push("align-items:center".into());
                styles.push("justify-content:center".into());
                styles.push("cursor:pointer".into());
                styles.push("font-weight:600".into());
                styles.push("font-family:inherit".into());
                if self.border.is_empty() {
                    styles.push("border:none".into());
                }
            }
            UiKind::Toggle => {
                styles.push("display:inline-flex".into());
                styles.push("align-items:center".into());
                styles.push("gap:8px".into());
                styles.push("cursor:pointer".into());
                styles.push("font-family:inherit".into());
                styles.push("border:none".into());
            }
            UiKind::Badge => {
                styles.push("display:inline-flex".into());
                styles.push("align-items:center".into());
                styles.push("align-self:flex-start".into());
                styles.push("font-weight:600".into());
            }
            UiKind::Input => {
                styles.push("outline:none".into());
                styles.push("font-family:inherit".into());
                styles.push("width:100%".into());
            }
            UiKind::Slider => {
                styles.push("width:100%".into());
                styles.push("accent-color:#38bdf8".into());
                styles.push("cursor:pointer".into());
            }
            UiKind::Shader => {
                styles.push("display:block".into());
                if st.w <= 0.0 {
                    styles.push("width:100%".into());
                }
                if st.h <= 0.0 {
                    styles.push("height:240px".into());
                }
            }
            UiKind::Image => {
                styles.push("display:block".into());
                styles.push("object-fit:cover".into());
            }
            UiKind::Text => {
                styles.push("line-height:1.35".into());
            }
            UiKind::Spacer => {
                styles.push(format!("min-height:{}px", self.pad.max(8.0) as i32));
            }
        }

        if st.w > 0.0 {
            styles.push(format!("width:{}px", st.w as i32));
        } else if st.w < 0.0 {
            styles.push("width:100%".into());
        }
        if st.h > 0.0 {
            styles.push(format!("height:{}px", st.h as i32));
        } else if st.h < 0.0 {
            styles.push("height:100%".into());
        }
        if st.min_w > 0.0 {
            styles.push(format!("min-width:{}px", st.min_w as i32));
        }
        if st.max_w > 0.0 {
            styles.push(format!("max-width:{}px", st.max_w as i32));
        }
        if st.grow > 0.0 {
            styles.push(format!("flex-grow:{}", st.grow));
        }
        if !st.align.is_empty() && st.align != "stretch" {
            let css_align = match st.align.as_str() {
                "start" => "flex-start",
                "end" => "flex-end",
                "center" => "center",
                other => other,
            };
            styles.push(format!("align-items:{css_align}"));
        }
        if !st.justify.is_empty() && st.justify != "start" {
            let css_just = match st.justify.as_str() {
                "end" => "flex-end",
                "center" => "center",
                "between" => "space-between",
                "around" => "space-around",
                other => other,
            };
            styles.push(format!("justify-content:{css_just}"));
        }

        if let Some(bg_css) = fill_to_css(&self.bg) {
            styles.push(format!("background:{bg_css}"));
        }
        if !self.color.is_empty() {
            styles.push(format!("color:{}", escape_html_attr(&self.color)));
        }
        if !self.border.is_empty() {
            let bw = st.border_w.max(1.0);
            styles.push(format!("border:{bw}px solid {}", escape_html_attr(&self.border)));
        }
        let r = self.effective_radii();
        if r[0] > 0.0 || r[1] > 0.0 || r[2] > 0.0 || r[3] > 0.0 {
            if (r[0] - r[1]).abs() < 0.01 && (r[1] - r[2]).abs() < 0.01 && (r[2] - r[3]).abs() < 0.01 {
                styles.push(format!("border-radius:{}px", r[0] as i32));
            } else {
                styles.push(format!(
                    "border-radius:{}px {}px {}px {}px",
                    r[0] as i32, r[1] as i32, r[2] as i32, r[3] as i32
                ));
            }
        }
        let p = self.effective_pad();
        if p[0] > 0.0 || p[1] > 0.0 || p[2] > 0.0 || p[3] > 0.0 {
            if matches!(self.kind, UiKind::Button | UiKind::Badge)
                && (p[0] - p[1]).abs() < 0.01
            {
                let pv = (p[0] * 0.6).max(6.0) as i32;
                let ph = (p[0] * 1.3).max(12.0) as i32;
                styles.push(format!("padding:{pv}px {ph}px"));
            } else if (p[0] - p[1]).abs() < 0.01 && (p[1] - p[2]).abs() < 0.01 && (p[2] - p[3]).abs() < 0.01 {
                styles.push(format!("padding:{}px", p[0] as i32));
            } else {
                styles.push(format!(
                    "padding:{}px {}px {}px {}px",
                    p[0] as i32, p[1] as i32, p[2] as i32, p[3] as i32
                ));
            }
        }
        let gx = self.effective_gap_x();
        let gy = self.effective_gap_y();
        if (gx > 0.0 || gy > 0.0) && matches!(self.kind, UiKind::Col | UiKind::Row | UiKind::Grid | UiKind::Card | UiKind::Scroll) {
            if (gx - gy).abs() < 0.01 {
                styles.push(format!("gap:{}px", gy as i32));
            } else {
                styles.push(format!("gap:{}px {}px", gy as i32, gx as i32));
            }
        }
        if self.font_size > 0.0 {
            styles.push(format!("font-size:{}px", self.font_size as i32));
        }
        if st.font_weight >= 600 {
            styles.push(format!("font-weight:{}", st.font_weight));
        }
        if st.shadow_blur > 0.0 && !st.shadow_color.is_empty() {
            styles.push(format!(
                "box-shadow:{}px {}px {}px {}",
                st.shadow_x as i32,
                st.shadow_y as i32,
                st.shadow_blur as i32,
                escape_html_attr(&st.shadow_color)
            ));
        }
        if st.backdrop_blur > 0.0 {
            styles.push(format!("backdrop-filter:blur({}px)", st.backdrop_blur as i32));
        }
        if (st.opacity - 1.0).abs() > 0.005 {
            styles.push(format!("opacity:{:.3}", st.opacity.clamp(0.0, 1.0)));
        }
        let mut transforms = Vec::new();
        if st.translate_x.abs() > 0.01 || st.translate_y.abs() > 0.01 {
            transforms.push(format!("translate({:.1}px,{:.1}px)", st.translate_x, st.translate_y));
        }
        if (st.scale - 1.0).abs() > 0.005 {
            transforms.push(format!("scale({:.3})", st.scale));
        }
        if st.rotate_deg.abs() > 0.01 {
            transforms.push(format!("rotate({:.1}deg)", st.rotate_deg));
        }
        if !transforms.is_empty() {
            styles.push(format!("transform:{}", transforms.join(" ")));
        }
        if st.transition_ms > 0.0 {
            styles.push(format!("transition:all {}ms cubic-bezier(0.22,1,0.36,1)", st.transition_ms as i32));
        }
        if st.overflow == "hidden" {
            styles.push("overflow:hidden".into());
        } else if st.overflow == "scroll" {
            styles.push("overflow:auto".into());
        }
        if st.z_index != 0 {
            styles.push(format!("z-index:{}", st.z_index));
        }

        out.push('<');
        out.push_str(tag);
        if !self.key.is_empty() {
            out.push_str(" data-knr-key=\"");
            out.push_str(&escape_html_attr(&self.key));
            out.push('"');
        }
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

        match self.kind {
            UiKind::Input => {
                out.push_str(" type=\"text\" value=\"");
                out.push_str(&escape_html_attr(&self.text));
                out.push_str("\" />");
                return;
            }
            UiKind::Slider => {
                out.push_str(&format!(
                    " type=\"range\" min=\"0\" max=\"1\" step=\"0.01\" value=\"{:.4}\" />",
                    self.value.clamp(0.0, 1.0)
                ));
                return;
            }
            UiKind::Image => {
                out.push_str(" src=\"");
                out.push_str(&escape_html_attr(&self.text));
                out.push_str("\" alt=\"\" />");
                return;
            }
            UiKind::Shader => {
                out.push_str(" data-knr-shader=\"");
                out.push_str(&escape_html_attr(&self.text));
                out.push('"');
                if !self.uniforms.is_empty() {
                    let u_str = self
                        .uniforms
                        .iter()
                        .map(|v| format!("{v:.4}"))
                        .collect::<Vec<_>>()
                        .join(",");
                    out.push_str(" data-knr-uniforms=\"");
                    out.push_str(&u_str);
                    out.push('"');
                }
                out.push_str("></canvas>");
                return;
            }
            _ => {}
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

fn parse_style_value(val: &Value) -> (String, String, String, f32, UiStyle) {
    let mut style = UiStyle::default();
    let mut bg = String::new();
    let mut color = "#f8fafc".to_string();
    let mut border = String::new();
    let mut font_size = 16.0f32;

    if let Value::Struct { fields, .. } = val {
        let f = |idx: usize| -> f32 { fields.get(idx).map(value_to_f64).unwrap_or(0.0) as f32 };
        let s = |idx: usize| -> String { fields.get(idx).map(value_to_string).unwrap_or_default() };
        let i = |idx: usize| -> i64 { fields.get(idx).and_then(Value::as_i64).unwrap_or(0) };

        style.w = f(0);
        style.h = f(1);
        style.min_w = f(2);
        style.max_w = f(3);
        bg = s(4);
        let c = s(5);
        if !c.is_empty() {
            color = c;
        }
        border = s(6);
        let bw = f(7);
        style.border_w = if bw > 0.0 { bw } else { 1.0 };
        style.radius_corners = [f(8).max(0.0), f(9).max(0.0), f(10).max(0.0), f(11).max(0.0)];
        style.pad_edges = [f(12).max(0.0), f(13).max(0.0), f(14).max(0.0), f(15).max(0.0)];
        style.gap_x = f(16).max(0.0);
        style.gap_y = f(17).max(0.0);
        let fs = f(18);
        if fs > 0.0 {
            font_size = fs;
        }
        let fw = i(19);
        style.font_weight = if fw > 0 { fw } else { 400 };
        let al = s(20);
        if !al.is_empty() {
            style.align = al;
        }
        let ju = s(21);
        if !ju.is_empty() {
            style.justify = ju;
        }
        style.grid_cols = i(22);
        style.shadow_x = f(23);
        style.shadow_y = f(24);
        style.shadow_blur = f(25).max(0.0);
        style.shadow_color = s(26);
        style.backdrop_blur = f(27).max(0.0);
        let op = f(28);
        style.opacity = if fields.len() > 28 && op > 0.0 { op.clamp(0.0, 1.0) } else { 1.0 };
        style.translate_x = f(29);
        style.translate_y = f(30);
        let sc = f(31);
        style.scale = if sc > 0.0 { sc } else { 1.0 };
        style.rotate_deg = f(32);
        style.hover_bg = s(33);
        style.hover_border = s(34);
        let hs = f(35);
        style.hover_scale = if hs > 0.0 { hs } else { 1.0 };
        style.transition_ms = f(36).max(0.0);
        let ov = s(37);
        if !ov.is_empty() {
            style.overflow = ov;
        }
        style.z_index = i(38);
        style.grow = f(39).max(0.0);
    }

    (bg, color, border, font_size, style)
}

fn value_to_string(v: &Value) -> String {
    match v {
        Value::Str(s) => s.clone(),
        Value::I64(n) => n.to_string(),
        Value::F64(n) => format!("{n:.2}"),
        Value::Bool(b) => b.to_string(),
        _ => String::new(),
    }
}

fn value_to_f64(v: &Value) -> f64 {
    match v {
        Value::F64(f) => *f,
        Value::I64(n) => *n as f64,
        Value::Bool(true) => 1.0,
        _ => 0.0,
    }
}

/// Floating-point bounding box produced by the Flexbox/Grid layout solver.
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
    pub kind: UiKind,
    pub value: f64,
    pub text: String,
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

/// Computes a full two-pass Flexbox + CSS Grid + Stack layout for `root` inside `(viewport_w, viewport_h)`.
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
    let [pt, pr, pb, pl] = node.effective_pad();
    let gx = node.effective_gap_x();
    let gy = node.effective_gap_y();
    let mut avail = max_w;
    if node.style.max_w > 0.0 {
        avail = avail.min(node.style.max_w);
    }
    if node.style.w > 0.0 {
        avail = node.style.w;
    }

    let (raw_w, raw_h) = match node.kind {
        UiKind::Text => {
            let (tw, th) = measurer.measure_text_size(&node.text, node.font_size);
            ((tw + pl + pr).min(avail.max(tw + pl + pr)), th + pt + pb)
        }
        UiKind::Button | UiKind::Badge => {
            let (tw, th) = measurer.measure_text_size(&node.text, node.font_size);
            let pv = if pt > 0.0 { (pt * 0.65).max(6.0) } else { 8.0 };
            let ph = if pl > 0.0 { (pl * 1.3).max(12.0) } else { 14.0 };
            (tw + ph * 2.0, th + pv * 2.0)
        }
        UiKind::Toggle => {
            let (tw, th) = measurer.measure_text_size(&node.text, node.font_size);
            (tw + 52.0 + pl + pr, th.max(26.0) + pt + pb)
        }
        UiKind::Input => {
            let (_, th) = measurer.measure_text_size("Mg", node.font_size);
            let h = (th + 16.0).max(38.0);
            (avail.max(160.0), h)
        }
        UiKind::Slider => (avail.max(140.0), 30.0),
        UiKind::Shader | UiKind::Image => {
            let w = if node.style.w > 0.0 { node.style.w } else { avail };
            let h = if node.style.h > 0.0 { node.style.h } else { 220.0 };
            (w, h)
        }
        UiKind::Spacer => (pl.max(8.0), pt.max(8.0)),
        UiKind::Col | UiKind::Card | UiKind::Scroll => {
            let inner_max = (avail - pl - pr).max(40.0);
            let mut max_child_w: f32 = 0.0;
            let mut total_h: f32 = 0.0;
            if !node.text.is_empty() {
                let (tw, th) = measurer.measure_text_size(&node.text, node.font_size);
                max_child_w = max_child_w.max(tw);
                total_h += th;
                if !node.children.is_empty() {
                    total_h += gy;
                }
            }
            for (idx, child) in node.children.iter().enumerate() {
                let (cw, ch) = measure_intrinsic(child, inner_max, measurer);
                max_child_w = max_child_w.max(cw);
                total_h += ch;
                if idx + 1 < node.children.len() {
                    total_h += gy;
                }
            }
            let w = if node.style.w > 0.0 {
                node.style.w
            } else {
                avail.max(max_child_w + pl + pr).min(avail)
            };
            (w, total_h + pt + pb)
        }
        UiKind::Grid => {
            let cols = node.style.grid_cols.max(1) as usize;
            let inner_w = (avail - pl - pr).max(40.0);
            let col_w = ((inner_w - gx * (cols.saturating_sub(1) as f32)) / cols as f32).max(20.0);
            let mut total_h: f32 = 0.0;
            for row_chunk in node.children.chunks(cols) {
                let mut row_h: f32 = 0.0;
                for child in row_chunk {
                    let (_, ch) = measure_intrinsic(child, col_w, measurer);
                    row_h = row_h.max(ch);
                }
                if total_h > 0.0 {
                    total_h += gy;
                }
                total_h += row_h;
            }
            (avail, total_h + pt + pb)
        }
        UiKind::Stack => {
            let inner_max = (avail - pl - pr).max(40.0);
            let mut max_w_c: f32 = 0.0;
            let mut max_h_c: f32 = 0.0;
            for child in &node.children {
                let (cw, ch) = measure_intrinsic(child, inner_max, measurer);
                max_w_c = max_w_c.max(cw);
                max_h_c = max_h_c.max(ch);
            }
            (avail.min(max_w_c + pl + pr).max(max_w_c + pl + pr), max_h_c + pt + pb)
        }
        UiKind::Row => {
            let inner_max = (avail - pl - pr).max(40.0);
            let mut row_x: f32 = 0.0;
            let mut row_h: f32 = 0.0;
            let mut total_h: f32 = 0.0;
            let mut max_row_w: f32 = 0.0;
            for child in &node.children {
                let (cw, ch) = measure_intrinsic(child, inner_max, measurer);
                if row_x > 0.0 && row_x + gx + cw > inner_max {
                    max_row_w = max_row_w.max(row_x);
                    total_h += row_h + gy;
                    row_x = cw;
                    row_h = ch;
                } else {
                    if row_x > 0.0 {
                        row_x += gx;
                    }
                    row_x += cw;
                    row_h = row_h.max(ch);
                }
            }
            max_row_w = max_row_w.max(row_x);
            total_h += row_h;
            (max_row_w + pl + pr, total_h + pt + pb)
        }
    };

    let final_w = if node.style.w > 0.0 {
        node.style.w
    } else if node.style.w < 0.0 {
        max_w
    } else {
        raw_w.max(node.style.min_w)
    };
    let final_h = if node.style.h > 0.0 { node.style.h } else { raw_h };
    (final_w, final_h)
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
    let [pt, pr, pb, pl] = node.effective_pad();
    let gx = node.effective_gap_x();
    let gy = node.effective_gap_y();
    let mut box_w = avail_w;
    if node.style.max_w > 0.0 {
        box_w = box_w.min(node.style.max_w);
    }
    if node.style.w > 0.0 {
        box_w = node.style.w;
    }

    let push_hit = |rect: LayoutRect, hit_boxes: &mut Vec<HitBox>| {
        if !node.action.is_empty() {
            hit_boxes.push(HitBox {
                rect,
                action: node.action.clone(),
                kind: node.kind,
                value: node.value,
                text: node.text.clone(),
            });
        }
    };

    match node.kind {
        UiKind::Text
        | UiKind::Button
        | UiKind::Badge
        | UiKind::Input
        | UiKind::Slider
        | UiKind::Toggle
        | UiKind::Image
        | UiKind::Shader
        | UiKind::Spacer => {
            let (iw, ih) = measure_intrinsic(node, box_w, measurer);
            let w = match node.kind {
                UiKind::Input | UiKind::Slider | UiKind::Shader => {
                    if node.style.w > 0.0 { node.style.w } else { box_w }
                }
                UiKind::Text => iw.min(box_w),
                _ => iw,
            };
            let h = min_h.map(|m| ih.max(m)).unwrap_or(ih);
            let rect = LayoutRect::new(x, y, w, h);
            push_hit(rect, hit_boxes);
            LayoutNode {
                rect,
                node: node.clone(),
                children: Vec::new(),
            }
        }
        UiKind::Col | UiKind::Card | UiKind::Scroll => {
            let inner_w = (box_w - pl - pr).max(20.0);
            let mut cur_y = y + pt;
            if !node.text.is_empty() {
                let (_, th) = measurer.measure_text_size(&node.text, node.font_size);
                cur_y += th + if node.children.is_empty() { 0.0 } else { gy };
            }
            let mut laid_children = Vec::with_capacity(node.children.len());
            for (idx, child) in node.children.iter().enumerate() {
                let (cw, _) = measure_intrinsic(child, inner_w, measurer);
                let child_x = match node.style.align.as_str() {
                    "center" => x + pl + ((inner_w - cw) * 0.5).max(0.0),
                    "end" => x + pl + (inner_w - cw).max(0.0),
                    _ => x + pl,
                };
                let child_avail = if node.style.align == "center" || node.style.align == "end" {
                    cw.min(inner_w)
                } else {
                    inner_w
                };
                let laid = layout_node(child, child_x, cur_y, child_avail, None, measurer, hit_boxes);
                cur_y += laid.rect.h;
                if idx + 1 < node.children.len() {
                    cur_y += gy;
                }
                laid_children.push(laid);
            }
            let content_h = (cur_y - y) + pb;
            let h = if node.style.h > 0.0 {
                node.style.h
            } else {
                min_h.map(|m| content_h.max(m)).unwrap_or(content_h)
            };
            let rect = LayoutRect::new(x, y, box_w, h);
            push_hit(rect, hit_boxes);
            LayoutNode {
                rect,
                node: node.clone(),
                children: laid_children,
            }
        }
        UiKind::Grid => {
            let cols = node.style.grid_cols.max(1) as usize;
            let inner_w = (box_w - pl - pr).max(20.0);
            let col_w = ((inner_w - gx * (cols.saturating_sub(1) as f32)) / cols as f32).max(20.0);
            let mut cur_y = y + pt;
            let mut laid_children = Vec::with_capacity(node.children.len());

            for row_chunk in node.children.chunks(cols) {
                let mut row_h: f32 = 0.0;
                for child in row_chunk {
                    let (_, ch) = measure_intrinsic(child, col_w, measurer);
                    row_h = row_h.max(ch);
                }
                for (c_idx, child) in row_chunk.iter().enumerate() {
                    let cx = x + pl + (c_idx as f32) * (col_w + gx);
                    let laid = layout_node(child, cx, cur_y, col_w, Some(row_h), measurer, hit_boxes);
                    laid_children.push(laid);
                }
                cur_y += row_h + gy;
            }
            if !node.children.is_empty() {
                cur_y -= gy;
            }
            let content_h = (cur_y - y) + pb;
            let h = if node.style.h > 0.0 {
                node.style.h
            } else {
                min_h.map(|m| content_h.max(m)).unwrap_or(content_h)
            };
            let rect = LayoutRect::new(x, y, box_w, h);
            push_hit(rect, hit_boxes);
            LayoutNode {
                rect,
                node: node.clone(),
                children: laid_children,
            }
        }
        UiKind::Stack => {
            let inner_w = (box_w - pl - pr).max(20.0);
            let mut max_h: f32 = 0.0;
            let mut laid_children = Vec::with_capacity(node.children.len());
            for child in &node.children {
                let laid = layout_node(child, x + pl, y + pt, inner_w, None, measurer, hit_boxes);
                max_h = max_h.max(laid.rect.h);
                laid_children.push(laid);
            }
            let content_h = max_h + pt + pb;
            let h = if node.style.h > 0.0 {
                node.style.h
            } else {
                min_h.map(|m| content_h.max(m)).unwrap_or(content_h)
            };
            let rect = LayoutRect::new(x, y, box_w, h);
            push_hit(rect, hit_boxes);
            LayoutNode {
                rect,
                node: node.clone(),
                children: laid_children,
            }
        }
        UiKind::Row => {
            let inner_w = (box_w - pl - pr).max(20.0);
            let total_grow: f32 = node.children.iter().map(|c| c.style.grow.max(0.0)).sum();
            let fixed_sum: f32 = node
                .children
                .iter()
                .filter(|c| c.style.grow <= 0.0)
                .map(|c| measure_intrinsic(c, inner_w, measurer).0)
                .sum();
            let gaps_total = gx * (node.children.len().saturating_sub(1) as f32);
            let rem_for_grow = (inner_w - fixed_sum - gaps_total).max(0.0);

            let mut cur_x = x + pl;
            let mut cur_y = y + pt;
            let mut line_h: f32 = 0.0;
            let mut laid_children = Vec::with_capacity(node.children.len());

            // Compute justify offset for single-line non-growing rows
            let justify_extra_gap = if total_grow == 0.0 && node.style.justify == "between" && node.children.len() > 1 {
                (inner_w - fixed_sum).max(0.0) / ((node.children.len() - 1) as f32)
            } else {
                gx
            };
            if total_grow == 0.0 && node.style.justify == "center" {
                let row_w = fixed_sum + gaps_total;
                cur_x += ((inner_w - row_w) * 0.5).max(0.0);
            } else if total_grow == 0.0 && node.style.justify == "end" {
                let row_w = fixed_sum + gaps_total;
                cur_x += (inner_w - row_w).max(0.0);
            }

            for child in &node.children {
                let (cw, ch) = measure_intrinsic(child, inner_w, measurer);
                let child_w = if total_grow > 0.0 && child.style.grow > 0.0 {
                    rem_for_grow * (child.style.grow / total_grow)
                } else if matches!(child.kind, UiKind::Card | UiKind::Col | UiKind::Grid) {
                    cw.min(inner_w)
                } else {
                    cw
                };
                if total_grow == 0.0 && cur_x > x + pl && (cur_x - (x + pl)) + child_w > inner_w + 0.5 {
                    cur_x = x + pl;
                    cur_y += line_h + gy;
                    line_h = 0.0;
                }
                let laid = layout_node(child, cur_x, cur_y, child_w, None, measurer, hit_boxes);
                cur_x += laid.rect.w + justify_extra_gap;
                line_h = line_h.max(laid.rect.h.max(ch));
                laid_children.push(laid);
            }

            let content_h = (cur_y - y) + line_h + pb;
            let h = if node.style.h > 0.0 {
                node.style.h
            } else {
                min_h.map(|m| content_h.max(m)).unwrap_or(content_h)
            };
            let rect = LayoutRect::new(x, y, box_w, h);
            push_hit(rect, hit_boxes);
            LayoutNode {
                rect,
                node: node.clone(),
                children: laid_children,
            }
        }
    }
}

/// Finds the topmost hit-box action containing `(x, y)`.
pub fn hit_test(hit_boxes: &[HitBox], x: f32, y: f32) -> Option<&str> {
    hit_boxes
        .iter()
        .rev()
        .find(|hb| hb.rect.contains(x, y))
        .map(|hb| hb.action.as_str())
}

/// Finds the topmost full `HitBox` containing `(x, y)`.
pub fn hit_test_box(hit_boxes: &[HitBox], x: f32, y: f32) -> Option<&HitBox> {
    hit_boxes.iter().rev().find(|hb| hb.rect.contains(x, y))
}
