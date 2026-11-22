//! Unconstrained Typed Declarative UI Tree, Flexbox + CSS Grid + Stack Layout Engine,
//! Interactive Hit-Testing (Buttons, TextInputs, Sliders, Toggles, ScrollViews, Shaders),
//! and Semantic HTML5/CSS Renderer.
//!
//! Supports BOTH:
//! - Canonical 8-field `Widget { key, kind, text, action, value, uniforms, style: Style(40), children }`
//! - Backward-compatible 11-field `UiNode` & legacy 3..5-field VDOM structs.

use std::collections::HashMap;

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
    Divider,
    Progress,
    Code,
    Icon,
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
            "divider" | "hr" => UiKind::Divider,
            "progress" | "progressbar" => UiKind::Progress,
            "code" | "codeblock" | "pre" => UiKind::Code,
            "icon" | "svg" => UiKind::Icon,
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
            UiKind::Divider => "divider",
            UiKind::Progress => "progress",
            UiKind::Code => "code",
            UiKind::Icon => "icon",
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

/// Full unconstrained style descriptor matching the 45-field `Style` schema in `.knr`
/// (gracefully backward-compatible with legacy 40-field `Style` structs).
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
    pub font_family: String,
    pub text_align: String,
    pub line_height: f32,
    pub col_span: i64,
    pub letter_spacing: f32,
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
            font_family: "sans".to_string(),
            text_align: "left".to_string(),
            line_height: 0.0,
            col_span: 1,
            letter_spacing: 0.0,
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

    /// Returns true when this container clips overflowing descendants
    /// (`<Scroll>` or `overflow="hidden"|"scroll"|"scroll-x"|"scroll-xy"`).
    pub fn clips_children(&self) -> bool {
        self.kind == UiKind::Scroll
            || self.style.overflow.eq_ignore_ascii_case("hidden")
            || self.style.overflow.eq_ignore_ascii_case("scroll")
            || self.style.overflow.eq_ignore_ascii_case("scroll-x")
            || self.style.overflow.eq_ignore_ascii_case("scroll-xy")
    }

    /// Returns true when this container supports vertical scrolling
    /// (`<Scroll>` unless `overflow="scroll-x"`, or `overflow="scroll"|"scroll-xy"`).
    pub fn is_scrollable_y(&self) -> bool {
        (self.kind == UiKind::Scroll && !self.style.overflow.eq_ignore_ascii_case("scroll-x"))
            || self.style.overflow.eq_ignore_ascii_case("scroll")
            || self.style.overflow.eq_ignore_ascii_case("scroll-xy")
    }

    /// Returns true when this container supports horizontal scrolling
    /// (`overflow="scroll-x"` or `overflow="scroll-xy"`).
    pub fn is_scrollable_x(&self) -> bool {
        self.style.overflow.eq_ignore_ascii_case("scroll-x")
            || self.style.overflow.eq_ignore_ascii_case("scroll-xy")
    }

    /// Returns true when this container supports vertical or horizontal scrolling.
    pub fn is_scrollable(&self) -> bool {
        self.is_scrollable_y() || self.is_scrollable_x()
    }

    /// Returns the explicit scroll key for this node (`key`, falling back to `action`, or empty).
    pub fn scroll_key(&self) -> &str {
        if !self.key.is_empty() {
            self.key.as_str()
        } else {
            self.action.as_str()
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
            UiKind::Badge | UiKind::Icon => "span",
            UiKind::Input => "input",
            UiKind::Slider => "input",
            UiKind::Image => "img",
            UiKind::Shader => "canvas",
            UiKind::Code => "pre",
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
                if !self.children.is_empty() {
                    let g = if st.gap_x > 0.0 { st.gap_x } else { 8.0 };
                    styles.push(format!("gap:{}px", g as i32));
                }
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
                let lh = effective_line_height_mult(self.font_size, st.line_height);
                styles.push(format!("line-height:{lh:.2}"));
                styles.push("white-space:pre-wrap".into());
            }
            UiKind::Code => {
                let lh = if st.line_height > 0.0 { st.line_height } else { 1.55 };
                styles.push("font-family:ui-monospace,SFMono-Regular,Menlo,Monaco,Consolas,monospace".into());
                styles.push(format!("line-height:{lh:.2}"));
                styles.push("white-space:pre".into());
                styles.push("overflow-x:auto".into());
                styles.push("box-sizing:border-box".into());
            }
            UiKind::Icon => {
                let sz = self.font_size.max(14.0) as i32;
                styles.push("display:inline-flex".into());
                styles.push("align-items:center".into());
                styles.push("justify-content:center".into());
                styles.push(format!("width:{sz}px"));
                styles.push(format!("height:{sz}px"));
            }
            UiKind::Spacer => {
                styles.push(format!("min-height:{}px", self.pad.max(8.0) as i32));
            }
            UiKind::Divider => {
                if st.w <= 0.0 {
                    styles.push("width:100%".into());
                }
                if st.h <= 0.0 {
                    let dh = if st.border_w > 1.0 {
                        st.border_w.max(2.0) as i32
                    } else {
                        1
                    };
                    styles.push(format!("height:{dh}px"));
                }
                if self.bg.is_empty() {
                    let div_bg = if !self.border.is_empty() {
                        escape_html_attr(&self.border)
                    } else {
                        "#334155aa".to_string()
                    };
                    styles.push(format!("background:{div_bg}"));
                }
            }
            UiKind::Progress => {
                if st.w <= 0.0 {
                    styles.push("width:100%".into());
                }
                if st.h <= 0.0 {
                    styles.push("height:10px".into());
                }
                if self.bg.is_empty() {
                    styles.push("background:#1e293b".into());
                }
                let r = self.effective_radii();
                if r.iter().all(|&c| c <= 0.0) {
                    styles.push("border-radius:9999px".into());
                }
                if st.overflow != "hidden" {
                    styles.push("overflow:hidden".into());
                }
                styles.push("box-sizing:border-box".into());
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
        if !self.border.is_empty() && self.kind != UiKind::Divider {
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
        if st.overflow.eq_ignore_ascii_case("hidden") {
            styles.push("overflow:hidden".into());
        } else if st.overflow.eq_ignore_ascii_case("scroll-x") {
            styles.push("overflow-x:auto".into());
            styles.push("overflow-y:hidden".into());
        } else if st.overflow.eq_ignore_ascii_case("scroll")
            || st.overflow.eq_ignore_ascii_case("scroll-xy")
        {
            styles.push("overflow:auto".into());
        }
        if st.z_index != 0 {
            styles.push(format!("z-index:{}", st.z_index));
        }
        if st.font_family == "mono" && self.kind != UiKind::Code {
            styles.push("font-family:ui-monospace,SFMono-Regular,Menlo,Monaco,Consolas,monospace".into());
        }
        if !st.text_align.is_empty() && st.text_align != "left" {
            styles.push(format!("text-align:{}", escape_html_attr(&st.text_align)));
        }
        if st.col_span > 1 {
            styles.push(format!("grid-column:span {}", st.col_span));
        }
        if st.letter_spacing.abs() > 0.001 {
            styles.push(format!("letter-spacing:{:.2}px", st.letter_spacing));
        }

        out.push('<');
        out.push_str(tag);
        out.push_str(" data-knr-kind=\"");
        out.push_str(self.kind.as_str());
        out.push('"');
        if self.kind == UiKind::Grid {
            out.push_str(&format!(" data-knr-cols=\"{}\"", st.grid_cols.max(1)));
        }
        if self.kind == UiKind::Icon && !self.text.is_empty() {
            out.push_str(" data-knr-icon=\"");
            out.push_str(&escape_html_attr(&self.text));
            out.push('"');
        }
        if self.kind == UiKind::Code && !self.action.is_empty() {
            out.push_str(" data-knr-lang=\"");
            out.push_str(&escape_html_attr(&self.action));
            out.push('"');
        }
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
            UiKind::Divider => {
                out.push_str("></div>");
                return;
            }
            UiKind::Icon => {
                out.push_str("></span>");
                return;
            }
            UiKind::Progress => {
                let pct = self.value.clamp(0.0, 1.0) * 100.0;
                let fill_css = if !self.color.is_empty() && self.color != "#f8fafc" {
                    fill_to_css(&self.color)
                        .unwrap_or_else(|| "linear-gradient(90deg,#38bdf8,#818cf8)".to_string())
                } else {
                    "linear-gradient(90deg,#38bdf8,#818cf8)".to_string()
                };
                out.push_str(&format!(
                    "><div style=\"width:{pct:.1}%;height:100%;background:{fill_css};border-radius:inherit;transition:width 150ms ease\"></div></div>"
                ));
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
        if fields.len() > 40 {
            let ff = s(40);
            if !ff.is_empty() {
                style.font_family = ff;
            }
        }
        if fields.len() > 41 {
            let ta = s(41);
            if !ta.is_empty() {
                style.text_align = ta;
            }
        }
        if fields.len() > 42 {
            let lh = f(42);
            if lh > 0.0 {
                style.line_height = lh;
            }
        }
        if fields.len() > 43 {
            let cs = i(43);
            if cs > 0 {
                style.col_span = cs;
            }
        }
        if fields.len() > 44 {
            style.letter_spacing = f(44);
        }
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

    /// Returns the non-empty intersection of `self` and `other`, or `None` if disjoint.
    pub fn intersect(&self, other: LayoutRect) -> Option<LayoutRect> {
        let x0 = self.x.max(other.x);
        let y0 = self.y.max(other.y);
        let x1 = (self.x + self.w).min(other.x + other.w);
        let y1 = (self.y + self.h).min(other.y + other.h);
        if x1 > x0 && y1 > y0 {
            Some(LayoutRect::new(x0, y0, x1 - x0, y1 - y0))
        } else {
            None
        }
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

/// Scrollable container hit-testing descriptor collected from a laid-out UI tree.
#[derive(Debug, Clone, PartialEq)]
pub struct ScrollTarget {
    pub id: String,
    pub rect: LayoutRect,
    pub content_height: f32,
    pub max_scroll_y: f32,
    pub scroll_offset_y: f32,
    pub content_width: f32,
    pub max_scroll_x: f32,
    pub scroll_offset_x: f32,
    pub horizontal: bool,
}

/// Laid-out UI tree node with exact screen-space coordinates and pre-wrapped text lines.
#[derive(Debug, Clone)]
pub struct LayoutNode {
    pub rect: LayoutRect,
    pub node: UiNode,
    pub wrapped_lines: Vec<String>,
    pub line_height: f32,
    pub content_height: f32,
    pub scroll_offset_y: f32,
    pub content_width: f32,
    pub scroll_offset_x: f32,
    pub children: Vec<LayoutNode>,
}

impl Default for LayoutNode {
    fn default() -> Self {
        Self {
            rect: LayoutRect::new(0.0, 0.0, 0.0, 0.0),
            node: UiNode::default(),
            wrapped_lines: Vec::new(),
            line_height: 20.0,
            content_height: 0.0,
            scroll_offset_y: 0.0,
            content_width: 0.0,
            scroll_offset_x: 0.0,
            children: Vec::new(),
        }
    }
}

/// Returns `true` when `children` are not already sorted in non-decreasing `z_index` order.
#[inline]
pub(crate) fn children_need_z_sort(children: &[LayoutNode]) -> bool {
    children
        .windows(2)
        .any(|w| w[0].node.style.z_index > w[1].node.style.z_index)
}

/// Returns child indices `0..children.len()` stably sorted in ascending `z_index` order
/// (preserving document order on ties).
pub(crate) fn sorted_child_indices_by_z(children: &[LayoutNode]) -> Vec<usize> {
    let mut indices: Vec<usize> = (0..children.len()).collect();
    if children_need_z_sort(children) {
        indices.sort_by_key(|&i| children[i].node.style.z_index);
    }
    indices
}

/// Returns the deterministic identifier for a scrollable container (`key` -> `action` -> structural `path`).
pub fn scroll_container_id(node: &UiNode, path: &str) -> String {
    let explicit = node.scroll_key();
    if !explicit.is_empty() {
        explicit.to_string()
    } else {
        path.to_string()
    }
}

/// Trait abstracting text measurement so layout works identically with `ab_glyph` or bitmap fonts.
pub trait TextMeasurer {
    fn measure_text_size(&self, text: &str, font_size: f32) -> (f32, f32);

    fn measure_text_styled(
        &self,
        text: &str,
        font_size: f32,
        _font_family: &str,
        _font_weight: i64,
    ) -> (f32, f32) {
        self.measure_text_size(text, font_size)
    }
}

/// Computes effective line-height multiplier for `UiKind::Text` when `style.line_height == 0.0`.
pub fn effective_line_height_mult(font_size: f32, style_line_height: f32) -> f32 {
    if style_line_height > 0.0 {
        style_line_height
    } else if font_size >= 28.0 {
        1.2
    } else if font_size >= 20.0 {
        1.35
    } else {
        1.5
    }
}

fn measure_styled_with_spacing<M: TextMeasurer>(
    measurer: &M,
    text: &str,
    font_size: f32,
    font_family: &str,
    font_weight: i64,
    letter_spacing: f32,
) -> (f32, f32) {
    let (w, h) = measurer.measure_text_styled(text, font_size, font_family, font_weight);
    if letter_spacing.abs() > 0.001 && !text.is_empty() {
        let extra = (text.chars().count().saturating_sub(1) as f32) * letter_spacing;
        ((w + extra).max(0.0), h)
    } else {
        (w, h)
    }
}

/// Word-wraps `text` cleanly to fit within `max_w` logical pixels (preserving explicit `\n`,
/// breaking long unspaced tokens by character so text NEVER overflows its container card).
/// Returns `(lines, max_line_w, total_height, line_height)`.
pub fn wrap_text_lines<M: TextMeasurer>(
    text: &str,
    font_size: f32,
    max_w: f32,
    measurer: &M,
) -> (Vec<String>, f32, f32, f32) {
    wrap_text_lines_styled(text, font_size, "sans", 400, 0.0, 0.0, max_w, measurer)
}

/// Styled version of `wrap_text_lines` respecting `font_family`, `font_weight`,
/// `line_height_mult` (`0.0` = auto), and `letter_spacing`.
#[allow(clippy::too_many_arguments)]
pub fn wrap_text_lines_styled<M: TextMeasurer>(
    text: &str,
    font_size: f32,
    font_family: &str,
    font_weight: i64,
    line_height_mult: f32,
    letter_spacing: f32,
    max_w: f32,
    measurer: &M,
) -> (Vec<String>, f32, f32, f32) {
    let (_, base_h) = measurer.measure_text_styled("Mg", font_size, font_family, font_weight);
    let lh_mult = effective_line_height_mult(font_size, line_height_mult);
    let line_h = (font_size * lh_mult).ceil().max(base_h);
    if text.is_empty() {
        return (Vec::new(), 0.0, base_h, line_h);
    }
    let limit = max_w.max(16.0);
    let mut lines: Vec<String> = Vec::new();
    let mut max_line_w: f32 = 0.0;

    for raw_para in text.split('\n') {
        if raw_para.is_empty() {
            lines.push(String::new());
            continue;
        }
        let (full_w, _) = measure_styled_with_spacing(
            measurer,
            raw_para,
            font_size,
            font_family,
            font_weight,
            letter_spacing,
        );
        if full_w <= limit {
            max_line_w = max_line_w.max(full_w);
            lines.push(raw_para.to_string());
            continue;
        }

        let mut cur = String::new();
        for word in raw_para.split_whitespace() {
            let candidate = if cur.is_empty() {
                word.to_string()
            } else {
                format!("{cur} {word}")
            };
            let (cw, _) = measure_styled_with_spacing(
                measurer,
                &candidate,
                font_size,
                font_family,
                font_weight,
                letter_spacing,
            );
            if cw <= limit {
                cur = candidate;
            } else {
                if !cur.is_empty() {
                    let (w_cur, _) = measure_styled_with_spacing(
                        measurer,
                        &cur,
                        font_size,
                        font_family,
                        font_weight,
                        letter_spacing,
                    );
                    max_line_w = max_line_w.max(w_cur);
                    lines.push(cur);
                }
                let (word_w, _) = measure_styled_with_spacing(
                    measurer,
                    word,
                    font_size,
                    font_family,
                    font_weight,
                    letter_spacing,
                );
                if word_w <= limit {
                    cur = word.to_string();
                } else {
                    // Break oversized unspaced token char-by-char so it never overflows
                    let mut chunk = String::new();
                    for ch in word.chars() {
                        let mut next = chunk.clone();
                        next.push(ch);
                        let (nw, _) = measure_styled_with_spacing(
                            measurer,
                            &next,
                            font_size,
                            font_family,
                            font_weight,
                            letter_spacing,
                        );
                        if nw <= limit || chunk.is_empty() {
                            chunk = next;
                        } else {
                            let (cw2, _) = measure_styled_with_spacing(
                                measurer,
                                &chunk,
                                font_size,
                                font_family,
                                font_weight,
                                letter_spacing,
                            );
                            max_line_w = max_line_w.max(cw2);
                            lines.push(chunk);
                            chunk = ch.to_string();
                        }
                    }
                    cur = chunk;
                }
            }
        }
        if !cur.is_empty() {
            let (w_cur, _) = measure_styled_with_spacing(
                measurer,
                &cur,
                font_size,
                font_family,
                font_weight,
                letter_spacing,
            );
            max_line_w = max_line_w.max(w_cur);
            lines.push(cur);
        }
    }

    if lines.is_empty() {
        lines.push(String::new());
    }
    let total_h = if lines.len() == 1 {
        base_h
    } else {
        base_h + ((lines.len() - 1) as f32) * line_h
    };
    (lines, max_line_w, total_h, line_h)
}

/// Keeps the visible suffix of a single-line text input that fits within `max_w`.
pub fn fit_input_text_tail<M: TextMeasurer>(
    text: &str,
    font_size: f32,
    max_w: f32,
    measurer: &M,
) -> String {
    let limit = max_w.max(20.0);
    if measurer.measure_text_size(text, font_size).0 <= limit {
        return text.to_string();
    }
    let chars: Vec<char> = text.chars().collect();
    let mut start = chars.len();
    let mut cur = String::new();
    while start > 0 {
        start -= 1;
        let cand: String = chars[start..].iter().collect();
        if measurer.measure_text_size(&cand, font_size).0 <= limit {
            cur = cand;
        } else {
            break;
        }
    }
    cur
}

/// Computes a full two-pass Flexbox + CSS Grid + Stack layout for `root` inside `(viewport_w, viewport_h)`.
pub fn compute_ui_layout<M: TextMeasurer>(
    root: &UiNode,
    viewport_w: f32,
    viewport_h: f32,
    measurer: &M,
) -> (LayoutNode, Vec<HitBox>) {
    let mut hit_boxes = Vec::new();
    let laid_out = layout_node_inner(
        root,
        0.0,
        0.0,
        viewport_w,
        Some(viewport_h),
        measurer,
        None,
        "root",
        &mut hit_boxes,
    );
    (laid_out, hit_boxes)
}

/// Computes a full two-pass Flexbox + CSS Grid + Stack layout while applying per-container
/// vertical scroll offsets (keyed by `<Scroll key="...">` / `scroll_container_id`) and
/// clipping descendant hit-boxes to clipped container bounds.
pub fn compute_ui_layout_with_scroll<M: TextMeasurer>(
    root: &UiNode,
    viewport_w: f32,
    viewport_h: f32,
    scroll_offsets: &HashMap<String, f32>,
    measurer: &M,
) -> (LayoutNode, Vec<HitBox>) {
    let mut hit_boxes = Vec::new();
    let laid_out = layout_node_inner(
        root,
        0.0,
        0.0,
        viewport_w,
        Some(viewport_h),
        measurer,
        Some(scroll_offsets),
        "root",
        &mut hit_boxes,
    );
    (laid_out, hit_boxes)
}

/// Applies per-container vertical scroll offsets onto an existing `LayoutNode` tree in-place,
/// shifting each scrollable container's children by the delta from its currently baked
/// `scroll_offset_y` (idempotent if called repeatedly with the same offsets).
pub fn apply_scroll_offsets(layout: &mut LayoutNode, scroll_offsets: &HashMap<String, f32>) {
    apply_scroll_offsets_rec(layout, scroll_offsets, "root");
}

fn lookup_scroll_x(node: &UiNode, id: &str, offsets: &HashMap<String, f32>) -> f32 {
    if let Some(&v) = offsets.get(&format!("{id}:x")) {
        return v;
    }
    if !node.is_scrollable_y() {
        if let Some(&v) = offsets.get(id) {
            return v;
        }
    }
    0.0
}

fn apply_scroll_offsets_rec(
    laid: &mut LayoutNode,
    scroll_offsets: &HashMap<String, f32>,
    path: &str,
) {
    if laid.node.is_scrollable() {
        let id = scroll_container_id(&laid.node, path);
        if laid.node.is_scrollable_y() {
            let requested = scroll_offsets.get(&id).copied().unwrap_or(0.0);
            let max_scroll = (laid.content_height - laid.rect.h).max(0.0);
            let target = requested.clamp(0.0, max_scroll);
            let delta = target - laid.scroll_offset_y;
            if delta.abs() > 0.001 {
                laid.scroll_offset_y = target;
                for ch in &mut laid.children {
                    shift_layout_y(ch, -delta);
                }
            }
        }
        if laid.node.is_scrollable_x() {
            let requested = lookup_scroll_x(&laid.node, &id, scroll_offsets);
            let max_scroll = (laid.content_width - laid.rect.w).max(0.0);
            let target = requested.clamp(0.0, max_scroll);
            let delta = target - laid.scroll_offset_x;
            if delta.abs() > 0.001 {
                laid.scroll_offset_x = target;
                for ch in &mut laid.children {
                    shift_layout_x(ch, -delta);
                }
            }
        }
    }
    for (idx, child) in laid.children.iter_mut().enumerate() {
        let child_path = format!("{path}/{idx}");
        apply_scroll_offsets_rec(child, scroll_offsets, &child_path);
    }
}

/// Returns true if any scrollable node in `layout` has a different desired offset in `scroll_offsets`
/// than its currently baked `scroll_offset_y` / `scroll_offset_x`.
pub(crate) fn layout_needs_scroll_offsets(
    laid: &LayoutNode,
    scroll_offsets: &HashMap<String, f32>,
) -> bool {
    layout_needs_scroll_offsets_rec(laid, scroll_offsets, "root")
}

fn layout_needs_scroll_offsets_rec(
    laid: &LayoutNode,
    scroll_offsets: &HashMap<String, f32>,
    path: &str,
) -> bool {
    if laid.node.is_scrollable() {
        let id = scroll_container_id(&laid.node, path);
        if laid.node.is_scrollable_y() {
            let requested = scroll_offsets.get(&id).copied().unwrap_or(0.0);
            let max_scroll = (laid.content_height - laid.rect.h).max(0.0);
            let target = requested.clamp(0.0, max_scroll);
            if (target - laid.scroll_offset_y).abs() > 0.01 {
                return true;
            }
        }
        if laid.node.is_scrollable_x() {
            let requested = lookup_scroll_x(&laid.node, &id, scroll_offsets);
            let max_scroll = (laid.content_width - laid.rect.w).max(0.0);
            let target = requested.clamp(0.0, max_scroll);
            if (target - laid.scroll_offset_x).abs() > 0.01 {
                return true;
            }
        }
    }
    for (idx, child) in laid.children.iter().enumerate() {
        let child_path = format!("{path}/{idx}");
        if layout_needs_scroll_offsets_rec(child, scroll_offsets, &child_path) {
            return true;
        }
    }
    false
}

/// Returns all scrollable containers containing `(x, y)` that have vertical or horizontal overflow,
/// ordered from topmost/deepest (reverse `z_index` paint order) to outermost for native scroll chaining.
pub fn hit_test_scroll_targets(layout: &LayoutNode, x: f32, y: f32) -> Vec<ScrollTarget> {
    let mut out = Vec::new();
    collect_scroll_targets_rec(layout, x, y, "root", &mut out);
    out
}

fn collect_scroll_targets_rec(
    laid: &LayoutNode,
    x: f32,
    y: f32,
    path: &str,
    out: &mut Vec<ScrollTarget>,
) {
    if laid.node.clips_children() && !laid.rect.contains(x, y) {
        return;
    }
    if children_need_z_sort(&laid.children) {
        let order = sorted_child_indices_by_z(&laid.children);
        for idx in order.into_iter().rev() {
            let child = &laid.children[idx];
            let child_path = format!("{path}/{idx}");
            collect_scroll_targets_rec(child, x, y, &child_path, out);
        }
    } else {
        for (idx, child) in laid.children.iter().enumerate().rev() {
            let child_path = format!("{path}/{idx}");
            collect_scroll_targets_rec(child, x, y, &child_path, out);
        }
    }
    let has_v_overflow = laid.node.is_scrollable_y() && laid.content_height > laid.rect.h + 0.5;
    let has_h_overflow = laid.node.is_scrollable_x() && laid.content_width > laid.rect.w + 0.5;
    if (has_v_overflow || has_h_overflow) && laid.rect.contains(x, y) {
        out.push(ScrollTarget {
            id: scroll_container_id(&laid.node, path),
            rect: laid.rect,
            content_height: laid.content_height,
            max_scroll_y: if has_v_overflow {
                (laid.content_height - laid.rect.h).max(0.0)
            } else {
                0.0
            },
            scroll_offset_y: laid.scroll_offset_y,
            content_width: laid.content_width,
            max_scroll_x: if has_h_overflow {
                (laid.content_width - laid.rect.w).max(0.0)
            } else {
                0.0
            },
            scroll_offset_x: laid.scroll_offset_x,
            horizontal: has_h_overflow,
        });
    }
}

/// Finds the deepest (innermost) scrollable container containing `(x, y)` with vertical overflow.
pub fn hit_test_scroll_target(layout: &LayoutNode, x: f32, y: f32) -> Option<ScrollTarget> {
    hit_test_scroll_targets(layout, x, y).into_iter().next()
}

fn effective_grid_cols(requested: i64, avail_w: f32) -> usize {
    let req = requested.max(1) as usize;
    if req >= 2 && avail_w < 500.0 {
        1
    } else if req >= 3 && avail_w < 760.0 {
        2
    } else {
        req
    }
}

/// Packs Grid children into rows respecting each child's `col_span` (clamped to `1..=cols`).
/// Returns rows of `(child_index, start_col, span)`.
fn pack_grid_rows(children: &[UiNode], cols: usize) -> Vec<Vec<(usize, usize, usize)>> {
    let mut rows: Vec<Vec<(usize, usize, usize)>> = Vec::new();
    let mut cur_row: Vec<(usize, usize, usize)> = Vec::new();
    let mut cur_col = 0usize;

    for (idx, child) in children.iter().enumerate() {
        let span = (child.style.col_span.max(1) as usize).min(cols);
        if cur_col > 0 && cur_col + span > cols {
            rows.push(std::mem::take(&mut cur_row));
            cur_col = 0;
        }
        cur_row.push((idx, cur_col, span));
        cur_col += span;
        if cur_col >= cols {
            rows.push(std::mem::take(&mut cur_row));
            cur_col = 0;
        }
    }
    if !cur_row.is_empty() {
        rows.push(cur_row);
    }
    rows
}

fn button_badge_padding(pad: [f32; 4]) -> (f32, f32, f32, f32) {
    let [pt, pr, pb, pl] = pad;
    if pt > 0.0 || pr > 0.0 || pb > 0.0 || pl > 0.0 {
        if (pt - pl).abs() < 0.01 && (pt - pr).abs() < 0.01 && (pt - pb).abs() < 0.01 {
            let pv = (pt * 0.65).max(6.0);
            let ph = (pl * 1.3).max(12.0);
            (pv, ph, pv, ph)
        } else {
            (pt, pr, pb, pl)
        }
    } else {
        (8.0, 14.0, 8.0, 14.0)
    }
}

fn measure_code_lines<M: TextMeasurer>(
    node: &UiNode,
    measurer: &M,
) -> (Vec<String>, f32, f32, f32) {
    let lh_mult = if node.style.line_height > 0.0 {
        node.style.line_height
    } else {
        1.55
    };
    let (_, base_h) = measurer.measure_text_styled("Mg", node.font_size, "mono", node.style.font_weight);
    let line_h = (node.font_size * lh_mult).ceil().max(base_h);
    if node.text.is_empty() {
        return (Vec::new(), 0.0, base_h, line_h);
    }
    let mut lines = Vec::new();
    let mut max_lw = 0.0f32;
    for raw in node.text.split('\n') {
        let line = raw.trim_end_matches('\r').to_string();
        let (lw, _) = measure_styled_with_spacing(
            measurer,
            &line,
            node.font_size,
            "mono",
            node.style.font_weight,
            node.style.letter_spacing,
        );
        max_lw = max_lw.max(lw);
        lines.push(line);
    }
    let total_h = if lines.len() <= 1 {
        base_h
    } else {
        base_h + ((lines.len() - 1) as f32) * line_h
    };
    (lines, max_lw, total_h, line_h)
}

fn shift_layout_y(node: &mut LayoutNode, dy: f32) {
    node.rect.y += dy;
    for ch in &mut node.children {
        shift_layout_y(ch, dy);
    }
}

fn shift_layout_x(node: &mut LayoutNode, dx: f32) {
    node.rect.x += dx;
    for ch in &mut node.children {
        shift_layout_x(ch, dx);
    }
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
            let inner_avail = (avail - pl - pr).max(16.0);
            let (_, max_lw, total_h, _) = wrap_text_lines_styled(
                &node.text,
                node.font_size,
                &node.style.font_family,
                node.style.font_weight,
                node.style.line_height,
                node.style.letter_spacing,
                inner_avail,
                measurer,
            );
            ((max_lw + pl + pr).min(avail), total_h + pt + pb)
        }
        UiKind::Code => {
            let (_, max_lw, total_h, _) = measure_code_lines(node, measurer);
            ((max_lw + pl + pr).min(avail), total_h + pt + pb)
        }
        UiKind::Icon => {
            let sz = node.font_size.max(14.0);
            (sz + pl + pr, sz + pt + pb)
        }
        UiKind::Button if !node.children.is_empty() => {
            let (pv_t, ph_r, pv_b, ph_l) = button_badge_padding([pt, pr, pb, pl]);
            let btn_gap = if gx > 0.0 { gx } else { 8.0 };
            let inner_max = (avail - ph_l - ph_r).max(16.0);
            let mut total_w = 0.0f32;
            let mut max_h = 0.0f32;
            let mut item_count = 0usize;
            for child in &node.children {
                let (cw, ch) = measure_intrinsic(child, inner_max, measurer);
                if item_count > 0 {
                    total_w += btn_gap;
                }
                total_w += cw;
                max_h = max_h.max(ch);
                item_count += 1;
            }
            if !node.text.is_empty() {
                let (tw, th) = measure_styled_with_spacing(
                    measurer,
                    &node.text,
                    node.font_size,
                    &node.style.font_family,
                    node.style.font_weight,
                    node.style.letter_spacing,
                );
                if item_count > 0 {
                    total_w += btn_gap;
                }
                total_w += tw;
                max_h = max_h.max(th);
            }
            (total_w + ph_l + ph_r, max_h + pv_t + pv_b)
        }
        UiKind::Button | UiKind::Badge => {
            let (tw, th) = measure_styled_with_spacing(
                measurer,
                &node.text,
                node.font_size,
                &node.style.font_family,
                node.style.font_weight,
                node.style.letter_spacing,
            );
            let (pv_t, ph_r, pv_b, ph_l) = button_badge_padding([pt, pr, pb, pl]);
            (tw + ph_l + ph_r, th + pv_t + pv_b)
        }
        UiKind::Toggle => {
            let (tw, th) = measure_styled_with_spacing(
                measurer,
                &node.text,
                node.font_size,
                &node.style.font_family,
                node.style.font_weight,
                node.style.letter_spacing,
            );
            (tw + 52.0 + pl + pr, th.max(26.0) + pt + pb)
        }
        UiKind::Input => {
            let (_, th) = measurer.measure_text_styled(
                "Mg",
                node.font_size,
                &node.style.font_family,
                node.style.font_weight,
            );
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
        UiKind::Divider => {
            let default_h = if node.style.border_w > 1.0 {
                node.style.border_w.max(2.0)
            } else {
                1.0
            };
            (avail, node.style.h.max(default_h))
        }
        UiKind::Progress => {
            let h = if node.style.h > 0.0 {
                node.style.h
            } else {
                10.0
            };
            (avail, h)
        }
        UiKind::Col | UiKind::Card | UiKind::Scroll => {
            let inner_max = (avail - pl - pr).max(40.0);
            let mut max_child_w: f32 = 0.0;
            let mut total_h: f32 = 0.0;
            if !node.text.is_empty() {
                let (_, tw, th, _) = wrap_text_lines_styled(
                    &node.text,
                    node.font_size,
                    &node.style.font_family,
                    node.style.font_weight,
                    node.style.line_height,
                    node.style.letter_spacing,
                    inner_max,
                    measurer,
                );
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
            let cols = effective_grid_cols(node.style.grid_cols, avail);
            let inner_w = (avail - pl - pr).max(40.0);
            let col_w = ((inner_w - gx * (cols.saturating_sub(1) as f32)) / cols as f32).max(20.0);
            let rows = pack_grid_rows(&node.children, cols);
            let mut total_h: f32 = 0.0;
            for row in rows {
                let mut row_h: f32 = 0.0;
                for (child_idx, _start_col, span) in row {
                    let item_w =
                        (span as f32) * col_w + (span.saturating_sub(1) as f32) * gx;
                    let (_, ch) = measure_intrinsic(&node.children[child_idx], item_w, measurer);
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
            let scroll_x = node.is_scrollable_x();
            let mut row_x: f32 = 0.0;
            let mut row_h: f32 = 0.0;
            let mut total_h: f32 = 0.0;
            let mut max_row_w: f32 = 0.0;
            for child in &node.children {
                let child_max = if scroll_x {
                    if child.style.w > 0.0 {
                        child.style.w
                    } else {
                        inner_max.max(2000.0)
                    }
                } else {
                    inner_max
                };
                let (cw, ch) = measure_intrinsic(child, child_max, measurer);
                if !scroll_x && row_x > 0.0 && row_x + gx + cw > inner_max {
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
            let w_out = if scroll_x {
                (max_row_w + pl + pr).min(avail)
            } else {
                max_row_w + pl + pr
            };
            (w_out, total_h + pt + pb)
        }
    };

    let mut final_w = if node.style.w > 0.0 {
        node.style.w
    } else if node.style.w < 0.0 {
        avail
    } else {
        raw_w.max(node.style.min_w)
    };
    if node.style.max_w > 0.0 {
        final_w = final_w.min(node.style.max_w);
    }
    let final_h = if node.style.h > 0.0 { node.style.h } else { raw_h };
    (final_w, final_h)
}

fn clip_hit_boxes_from(hit_boxes: &mut Vec<HitBox>, start: usize, clip_rect: LayoutRect) {
    let mut idx = start;
    while idx < hit_boxes.len() {
        if let Some(clipped) = hit_boxes[idx].rect.intersect(clip_rect) {
            hit_boxes[idx].rect = clipped;
            idx += 1;
        } else {
            hit_boxes.remove(idx);
        }
    }
}

/// Stably reorders child hit-box slices within `hit_boxes[children_hb_start..]` into ascending
/// `z_index` paint order (preserving document order on ties). Because `hit_test` / `hit_test_box`
/// scan in reverse, higher `z_index` widgets (and later siblings on ties) always intercept clicks first.
fn reorder_child_hit_boxes_by_z(
    hit_boxes: &mut [HitBox],
    children_hb_start: usize,
    laid_children: &[LayoutNode],
    child_hb_ranges: &[(usize, usize)],
) {
    if !children_need_z_sort(laid_children) || children_hb_start >= hit_boxes.len() {
        return;
    }
    let order = sorted_child_indices_by_z(laid_children);
    let mut reordered: Vec<HitBox> = Vec::with_capacity(hit_boxes.len() - children_hb_start);
    for idx in order {
        let (s, e) = child_hb_ranges[idx];
        if s < e && e <= hit_boxes.len() {
            reordered.extend_from_slice(&hit_boxes[s..e]);
        }
    }
    if reordered.len() == hit_boxes.len() - children_hb_start {
        hit_boxes[children_hb_start..].clone_from_slice(&reordered);
    }
}

fn compute_children_content_width(
    x: f32,
    box_w: f32,
    pl: f32,
    pr: f32,
    laid_children: &[LayoutNode],
) -> f32 {
    let mut max_right = x + pl;
    for ch in laid_children {
        max_right = max_right.max(ch.rect.x + ch.rect.w);
    }
    ((max_right - x) + pr).max(box_w)
}

#[allow(clippy::too_many_arguments)]
fn apply_container_scroll_and_clip(
    node: &UiNode,
    rect: LayoutRect,
    content_h: f32,
    content_w: f32,
    path: &str,
    scroll_offsets: Option<&HashMap<String, f32>>,
    laid_children: &mut [LayoutNode],
    hit_boxes: &mut Vec<HitBox>,
    children_hb_start: usize,
) -> (f32, f32) {
    let mut scroll_offset_y = 0.0f32;
    let mut scroll_offset_x = 0.0f32;
    if node.is_scrollable() {
        if let Some(offsets) = scroll_offsets {
            let id = scroll_container_id(node, path);
            if node.is_scrollable_y() {
                if let Some(&req) = offsets.get(&id) {
                    let max_scroll = (content_h - rect.h).max(0.0);
                    scroll_offset_y = req.clamp(0.0, max_scroll);
                }
            }
            if node.is_scrollable_x() {
                let req = lookup_scroll_x(node, &id, offsets);
                let max_scroll = (content_w - rect.w).max(0.0);
                scroll_offset_x = req.clamp(0.0, max_scroll);
            }
        }
    }
    if scroll_offset_y > 0.001 {
        for ch in laid_children.iter_mut() {
            shift_layout_y(ch, -scroll_offset_y);
        }
        for hb in &mut hit_boxes[children_hb_start..] {
            hb.rect.y -= scroll_offset_y;
        }
    }
    if scroll_offset_x > 0.001 {
        for ch in laid_children.iter_mut() {
            shift_layout_x(ch, -scroll_offset_x);
        }
        for hb in &mut hit_boxes[children_hb_start..] {
            hb.rect.x -= scroll_offset_x;
        }
    }
    if node.clips_children() {
        clip_hit_boxes_from(hit_boxes, children_hb_start, rect);
    }
    (scroll_offset_y, scroll_offset_x)
}

#[allow(clippy::too_many_arguments)]
fn layout_node_inner<M: TextMeasurer>(
    node: &UiNode,
    x: f32,
    y: f32,
    avail_w: f32,
    min_h: Option<f32>,
    measurer: &M,
    scroll_offsets: Option<&HashMap<String, f32>>,
    path: &str,
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

    let insert_container_hit = |idx: usize, rect: LayoutRect, hit_boxes: &mut Vec<HitBox>| {
        if !node.action.is_empty() {
            hit_boxes.insert(
                idx,
                HitBox {
                    rect,
                    action: node.action.clone(),
                    kind: node.kind,
                    value: node.value,
                    text: node.text.clone(),
                },
            );
        }
    };

    match node.kind {
        UiKind::Button if !node.children.is_empty() => {
            let (pv_t, ph_r, pv_b, ph_l) = button_badge_padding([pt, pr, pb, pl]);
            let btn_gap = if gx > 0.0 { gx } else { 8.0 };
            let (iw, ih) = measure_intrinsic(node, box_w, measurer);
            let w = if node.style.w > 0.0 {
                node.style.w
            } else if node.style.w < 0.0 {
                box_w
            } else {
                iw.min(box_w)
            };
            let h = if node.style.h > 0.0 {
                node.style.h
            } else {
                min_h.map(|m| ih.max(m)).unwrap_or(ih)
            };
            let rect = LayoutRect::new(x, y, w, h);
            let inner_w = (w - ph_l - ph_r).max(16.0);

            let mut child_nodes: Vec<UiNode> = node.children.clone();
            if !node.text.is_empty() {
                let label_style = UiStyle {
                    font_family: node.style.font_family.clone(),
                    font_weight: node.style.font_weight,
                    letter_spacing: node.style.letter_spacing,
                    ..UiStyle::default()
                };
                child_nodes.push(UiNode {
                    kind: UiKind::Text,
                    text: node.text.clone(),
                    color: node.color.clone(),
                    font_size: node.font_size,
                    style: label_style,
                    ..UiNode::default()
                });
            }

            let mut sizes: Vec<(f32, f32)> = Vec::with_capacity(child_nodes.len());
            let mut content_w = 0.0f32;
            for (i, ch) in child_nodes.iter().enumerate() {
                let (cw, ch_h) = measure_intrinsic(ch, inner_w, measurer);
                if i > 0 {
                    content_w += btn_gap;
                }
                content_w += cw;
                sizes.push((cw, ch_h));
            }

            let children_hb_start = hit_boxes.len();
            let mut cur_x = x + ((w - content_w) * 0.5).max(ph_l);
            let mut laid_children = Vec::with_capacity(child_nodes.len());
            let mut child_hb_ranges: Vec<(usize, usize)> = Vec::with_capacity(child_nodes.len());
            for (c_idx, (ch, (cw, ch_h))) in child_nodes.iter().zip(sizes).enumerate() {
                let cy = y + ((h - ch_h) * 0.5).max(pv_t);
                let avail_c = cw.min(inner_w);
                let hb_start = hit_boxes.len();
                let child_path = format!("{path}/{c_idx}");
                let laid = layout_node_inner(
                    ch,
                    cur_x,
                    cy,
                    avail_c,
                    None,
                    measurer,
                    scroll_offsets,
                    &child_path,
                    hit_boxes,
                );
                let hb_end = hit_boxes.len();
                cur_x += laid.rect.w + btn_gap;
                laid_children.push(laid);
                child_hb_ranges.push((hb_start, hb_end));
            }
            let _ = pv_b;
            reorder_child_hit_boxes_by_z(
                hit_boxes,
                children_hb_start,
                &laid_children,
                &child_hb_ranges,
            );
            // Push Button hit-box AFTER children so clicking any child hits the button action.
            push_hit(rect, hit_boxes);
            LayoutNode {
                rect,
                node: node.clone(),
                wrapped_lines: Vec::new(),
                line_height: node.font_size * 1.3,
                content_height: h,
                scroll_offset_y: 0.0,
                content_width: w,
                scroll_offset_x: 0.0,
                children: laid_children,
            }
        }
        UiKind::Text
        | UiKind::Code
        | UiKind::Icon
        | UiKind::Button
        | UiKind::Badge
        | UiKind::Input
        | UiKind::Slider
        | UiKind::Toggle
        | UiKind::Image
        | UiKind::Shader
        | UiKind::Spacer
        | UiKind::Divider
        | UiKind::Progress => {
            let (iw, ih) = measure_intrinsic(node, box_w, measurer);
            let w = match node.kind {
                UiKind::Input
                | UiKind::Slider
                | UiKind::Shader
                | UiKind::Divider
                | UiKind::Progress
                | UiKind::Code => {
                    if node.style.w > 0.0 {
                        node.style.w
                    } else {
                        box_w
                    }
                }
                UiKind::Text => {
                    if node.style.w > 0.0 {
                        node.style.w
                    } else if node.style.w < 0.0
                        || matches!(node.style.text_align.as_str(), "center" | "right")
                    {
                        box_w
                    } else {
                        iw.min(box_w)
                    }
                }
                _ => iw,
            };
            let h = if node.style.h > 0.0 {
                node.style.h
            } else {
                min_h.map(|m| ih.max(m)).unwrap_or(ih)
            };
            let rect = LayoutRect::new(x, y, w, h);
            let (wrapped_lines, line_height) = match node.kind {
                UiKind::Text if !node.text.is_empty() => {
                    let inner_w = (w - pl - pr).max(16.0);
                    let (lines, _, _, lh) = wrap_text_lines_styled(
                        &node.text,
                        node.font_size,
                        &node.style.font_family,
                        node.style.font_weight,
                        node.style.line_height,
                        node.style.letter_spacing,
                        inner_w,
                        measurer,
                    );
                    (lines, lh)
                }
                UiKind::Code => {
                    let (lines, _, _, lh) = measure_code_lines(node, measurer);
                    (lines, lh)
                }
                UiKind::Icon | UiKind::Divider | UiKind::Progress => {
                    (Vec::new(), node.font_size * 1.3)
                }
                UiKind::Input if !node.text.is_empty() => {
                    let inner_w = (w - pl.max(12.0) - pr.max(12.0)).max(20.0);
                    let fitted = fit_input_text_tail(&node.text, node.font_size, inner_w, measurer);
                    let (_, th) = measurer.measure_text_styled(
                        "Mg",
                        node.font_size,
                        &node.style.font_family,
                        node.style.font_weight,
                    );
                    (vec![fitted], th)
                }
                _ if !node.text.is_empty() => {
                    let (_, th) = measurer.measure_text_styled(
                        "Mg",
                        node.font_size,
                        &node.style.font_family,
                        node.style.font_weight,
                    );
                    (vec![node.text.clone()], th)
                }
                _ => (Vec::new(), node.font_size * 1.3),
            };
            push_hit(rect, hit_boxes);
            LayoutNode {
                rect,
                node: node.clone(),
                wrapped_lines,
                line_height,
                content_height: h,
                scroll_offset_y: 0.0,
                content_width: w,
                scroll_offset_x: 0.0,
                children: Vec::new(),
            }
        }
        UiKind::Col | UiKind::Card | UiKind::Scroll => {
            let inner_w = (box_w - pl - pr).max(20.0);
            let mut cur_y = y + pt;
            let (wrapped_lines, line_height) = if !node.text.is_empty() {
                let (lines, _, th, lh) = wrap_text_lines_styled(
                    &node.text,
                    node.font_size,
                    &node.style.font_family,
                    node.style.font_weight,
                    node.style.line_height,
                    node.style.letter_spacing,
                    inner_w,
                    measurer,
                );
                cur_y += th + if node.children.is_empty() { 0.0 } else { gy };
                (lines, lh)
            } else {
                (Vec::new(), node.font_size * 1.3)
            };
            let children_hb_start = hit_boxes.len();
            let mut laid_children = Vec::with_capacity(node.children.len());
            let mut child_hb_ranges: Vec<(usize, usize)> = Vec::with_capacity(node.children.len());
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
                let hb_start = hit_boxes.len();
                let child_path = format!("{path}/{idx}");
                let laid = layout_node_inner(
                    child,
                    child_x,
                    cur_y,
                    child_avail,
                    None,
                    measurer,
                    scroll_offsets,
                    &child_path,
                    hit_boxes,
                );
                let hb_end = hit_boxes.len();
                cur_y += laid.rect.h;
                if idx + 1 < node.children.len() {
                    cur_y += gy;
                }
                laid_children.push(laid);
                child_hb_ranges.push((hb_start, hb_end));
            }
            let content_h = (cur_y - y) + pb;
            let content_w = compute_children_content_width(x, box_w, pl, pr, &laid_children);
            let h = if node.style.h > 0.0 {
                node.style.h
            } else if node.style.h < 0.0 && node.clips_children() {
                min_h.unwrap_or(content_h)
            } else {
                min_h.map(|m| content_h.max(m)).unwrap_or(content_h)
            };
            let extra_h = (h - content_h).max(0.0);
            if extra_h > 0.5 && !laid_children.is_empty() {
                match node.style.justify.as_str() {
                    "center" => {
                        let dy = extra_h * 0.5;
                        for (child, &(hb_s, hb_e)) in laid_children.iter_mut().zip(&child_hb_ranges) {
                            shift_layout_y(child, dy);
                            for hb in &mut hit_boxes[hb_s..hb_e] {
                                hb.rect.y += dy;
                            }
                        }
                    }
                    "end" => {
                        for (child, &(hb_s, hb_e)) in laid_children.iter_mut().zip(&child_hb_ranges) {
                            shift_layout_y(child, extra_h);
                            for hb in &mut hit_boxes[hb_s..hb_e] {
                                hb.rect.y += extra_h;
                            }
                        }
                    }
                    "between" if laid_children.len() > 1 => {
                        let step = extra_h / ((laid_children.len() - 1) as f32);
                        for (i, (child, &(hb_s, hb_e))) in
                            laid_children.iter_mut().zip(&child_hb_ranges).enumerate()
                        {
                            let dy = step * (i as f32);
                            if dy > 0.01 {
                                shift_layout_y(child, dy);
                                for hb in &mut hit_boxes[hb_s..hb_e] {
                                    hb.rect.y += dy;
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
            reorder_child_hit_boxes_by_z(
                hit_boxes,
                children_hb_start,
                &laid_children,
                &child_hb_ranges,
            );
            let rect = LayoutRect::new(x, y, box_w, h);
            let (scroll_offset_y, scroll_offset_x) = apply_container_scroll_and_clip(
                node,
                rect,
                content_h,
                content_w,
                path,
                scroll_offsets,
                &mut laid_children,
                hit_boxes,
                children_hb_start,
            );
            insert_container_hit(children_hb_start, rect, hit_boxes);
            LayoutNode {
                rect,
                node: node.clone(),
                wrapped_lines,
                line_height,
                content_height: content_h,
                scroll_offset_y,
                content_width: content_w,
                scroll_offset_x,
                children: laid_children,
            }
        }
        UiKind::Grid => {
            let cols = effective_grid_cols(node.style.grid_cols, box_w);
            let inner_w = (box_w - pl - pr).max(20.0);
            let col_w = ((inner_w - gx * (cols.saturating_sub(1) as f32)) / cols as f32).max(20.0);
            let mut cur_y = y + pt;
            let children_hb_start = hit_boxes.len();
            let mut laid_children = Vec::with_capacity(node.children.len());
            let mut child_hb_ranges: Vec<(usize, usize)> = Vec::with_capacity(node.children.len());
            let rows = pack_grid_rows(&node.children, cols);

            for (row_idx, row) in rows.iter().enumerate() {
                let mut row_h: f32 = 0.0;
                for &(child_idx, _start_col, span) in row {
                    let item_w =
                        (span as f32) * col_w + (span.saturating_sub(1) as f32) * gx;
                    let (_, ch) = measure_intrinsic(&node.children[child_idx], item_w, measurer);
                    row_h = row_h.max(ch);
                }
                for &(child_idx, start_col, span) in row {
                    let item_w =
                        (span as f32) * col_w + (span.saturating_sub(1) as f32) * gx;
                    let cx = x + pl + (start_col as f32) * (col_w + gx);
                    let hb_start = hit_boxes.len();
                    let child_path = format!("{path}/{child_idx}");
                    let laid = layout_node_inner(
                        &node.children[child_idx],
                        cx,
                        cur_y,
                        item_w,
                        Some(row_h),
                        measurer,
                        scroll_offsets,
                        &child_path,
                        hit_boxes,
                    );
                    let hb_end = hit_boxes.len();
                    laid_children.push(laid);
                    child_hb_ranges.push((hb_start, hb_end));
                }
                cur_y += row_h;
                if row_idx + 1 < rows.len() {
                    cur_y += gy;
                }
            }
            let content_h = (cur_y - y) + pb;
            let content_w = compute_children_content_width(x, box_w, pl, pr, &laid_children);
            let h = if node.style.h > 0.0 {
                node.style.h
            } else if node.style.h < 0.0 && node.clips_children() {
                min_h.unwrap_or(content_h)
            } else {
                min_h.map(|m| content_h.max(m)).unwrap_or(content_h)
            };
            reorder_child_hit_boxes_by_z(
                hit_boxes,
                children_hb_start,
                &laid_children,
                &child_hb_ranges,
            );
            let rect = LayoutRect::new(x, y, box_w, h);
            let (scroll_offset_y, scroll_offset_x) = apply_container_scroll_and_clip(
                node,
                rect,
                content_h,
                content_w,
                path,
                scroll_offsets,
                &mut laid_children,
                hit_boxes,
                children_hb_start,
            );
            insert_container_hit(children_hb_start, rect, hit_boxes);
            LayoutNode {
                rect,
                node: node.clone(),
                wrapped_lines: Vec::new(),
                line_height: node.font_size * 1.3,
                content_height: content_h,
                scroll_offset_y,
                content_width: content_w,
                scroll_offset_x,
                children: laid_children,
            }
        }
        UiKind::Stack => {
            let inner_w = (box_w - pl - pr).max(20.0);
            let mut max_h: f32 = 0.0;
            let children_hb_start = hit_boxes.len();
            let mut laid_children = Vec::with_capacity(node.children.len());
            let mut child_hb_ranges: Vec<(usize, usize)> = Vec::with_capacity(node.children.len());
            for (idx, child) in node.children.iter().enumerate() {
                let hb_start = hit_boxes.len();
                let child_path = format!("{path}/{idx}");
                let laid = layout_node_inner(
                    child,
                    x + pl,
                    y + pt,
                    inner_w,
                    None,
                    measurer,
                    scroll_offsets,
                    &child_path,
                    hit_boxes,
                );
                let hb_end = hit_boxes.len();
                max_h = max_h.max(laid.rect.h);
                laid_children.push(laid);
                child_hb_ranges.push((hb_start, hb_end));
            }
            let content_h = max_h + pt + pb;
            let content_w = compute_children_content_width(x, box_w, pl, pr, &laid_children);
            let h = if node.style.h > 0.0 {
                node.style.h
            } else if node.style.h < 0.0 && node.clips_children() {
                min_h.unwrap_or(content_h)
            } else {
                min_h.map(|m| content_h.max(m)).unwrap_or(content_h)
            };
            reorder_child_hit_boxes_by_z(
                hit_boxes,
                children_hb_start,
                &laid_children,
                &child_hb_ranges,
            );
            let rect = LayoutRect::new(x, y, box_w, h);
            let (scroll_offset_y, scroll_offset_x) = apply_container_scroll_and_clip(
                node,
                rect,
                content_h,
                content_w,
                path,
                scroll_offsets,
                &mut laid_children,
                hit_boxes,
                children_hb_start,
            );
            insert_container_hit(children_hb_start, rect, hit_boxes);
            LayoutNode {
                rect,
                node: node.clone(),
                wrapped_lines: Vec::new(),
                line_height: node.font_size * 1.3,
                content_height: content_h,
                scroll_offset_y,
                content_width: content_w,
                scroll_offset_x,
                children: laid_children,
            }
        }
        UiKind::Row => {
            let inner_w = (box_w - pl - pr).max(20.0);
            let scroll_x = node.is_scrollable_x();
            let total_grow: f32 = node.children.iter().map(|c| c.style.grow.max(0.0)).sum();
            let fixed_sum: f32 = node
                .children
                .iter()
                .filter(|c| c.style.grow <= 0.0)
                .map(|c| {
                    let c_max = if scroll_x {
                        if c.style.w > 0.0 {
                            c.style.w
                        } else {
                            inner_w.max(2000.0)
                        }
                    } else {
                        inner_w
                    };
                    measure_intrinsic(c, c_max, measurer).0
                })
                .sum();
            let gaps_total = gx * (node.children.len().saturating_sub(1) as f32);
            let rem_for_grow = (inner_w - fixed_sum - gaps_total).max(0.0);

            let mut cur_x = x + pl;
            let mut cur_y = y + pt;
            let mut line_h: f32 = 0.0;
            let children_hb_start = hit_boxes.len();
            let mut laid_children: Vec<LayoutNode> = Vec::with_capacity(node.children.len());
            let mut all_child_hb_ranges: Vec<(usize, usize)> =
                Vec::with_capacity(node.children.len());
            // Track `(child_idx, hb_start, hb_end)` for the current visual row line to align vertically
            let mut line_entries: Vec<(usize, usize, usize)> = Vec::new();

            let flush_row_align = |entries: &mut Vec<(usize, usize, usize)>,
                                   lh: f32,
                                   align: &str,
                                   children: &mut [LayoutNode],
                                   hbs: &mut [HitBox]| {
                if matches!(align, "center" | "end") {
                    for &(c_idx, hb_start, hb_end) in entries.iter() {
                        let ch_h = children[c_idx].rect.h;
                        let dy = match align {
                            "center" => ((lh - ch_h) * 0.5).max(0.0),
                            "end" => (lh - ch_h).max(0.0),
                            _ => 0.0,
                        };
                        if dy > 0.01 {
                            shift_layout_y(&mut children[c_idx], dy);
                            for hb in &mut hbs[hb_start..hb_end] {
                                hb.rect.y += dy;
                            }
                        }
                    }
                }
                entries.clear();
            };

            // Compute justify offset for single-line non-growing rows
            let justify_extra_gap = if !scroll_x
                && total_grow == 0.0
                && node.style.justify == "between"
                && node.children.len() > 1
            {
                (inner_w - fixed_sum).max(0.0) / ((node.children.len() - 1) as f32)
            } else {
                gx
            };
            if !scroll_x && total_grow == 0.0 && node.style.justify == "center" {
                let row_w = fixed_sum + gaps_total;
                cur_x += ((inner_w - row_w) * 0.5).max(0.0);
            } else if !scroll_x && total_grow == 0.0 && node.style.justify == "end" {
                let row_w = fixed_sum + gaps_total;
                cur_x += (inner_w - row_w).max(0.0);
            }

            for (idx, child) in node.children.iter().enumerate() {
                let c_max = if scroll_x {
                    if child.style.w > 0.0 {
                        child.style.w
                    } else {
                        inner_w.max(2000.0)
                    }
                } else {
                    inner_w
                };
                let (cw, ch) = measure_intrinsic(child, c_max, measurer);
                let child_w = if total_grow > 0.0 && child.style.grow > 0.0 {
                    rem_for_grow * (child.style.grow / total_grow)
                } else if !scroll_x
                    && matches!(child.kind, UiKind::Card | UiKind::Col | UiKind::Grid | UiKind::Code)
                {
                    cw.min(inner_w)
                } else {
                    cw
                };
                if !scroll_x
                    && total_grow == 0.0
                    && cur_x > x + pl
                    && (cur_x - (x + pl)) + child_w > inner_w + 0.5
                {
                    flush_row_align(
                        &mut line_entries,
                        line_h,
                        &node.style.align,
                        &mut laid_children,
                        hit_boxes,
                    );
                    cur_x = x + pl;
                    cur_y += line_h + gy;
                    line_h = 0.0;
                }
                let hb_start = hit_boxes.len();
                let child_path = format!("{path}/{idx}");
                let laid = layout_node_inner(
                    child,
                    cur_x,
                    cur_y,
                    child_w,
                    None,
                    measurer,
                    scroll_offsets,
                    &child_path,
                    hit_boxes,
                );
                let hb_end = hit_boxes.len();
                cur_x += laid.rect.w + justify_extra_gap;
                line_h = line_h.max(laid.rect.h.max(ch));
                let c_idx = laid_children.len();
                laid_children.push(laid);
                line_entries.push((c_idx, hb_start, hb_end));
                all_child_hb_ranges.push((hb_start, hb_end));
            }
            flush_row_align(
                &mut line_entries,
                line_h,
                &node.style.align,
                &mut laid_children,
                hit_boxes,
            );

            let content_h = (cur_y - y) + line_h + pb;
            let content_w = compute_children_content_width(x, box_w, pl, pr, &laid_children);
            let h = if node.style.h > 0.0 {
                node.style.h
            } else if node.style.h < 0.0 && node.clips_children() {
                min_h.unwrap_or(content_h)
            } else {
                min_h.map(|m| content_h.max(m)).unwrap_or(content_h)
            };
            reorder_child_hit_boxes_by_z(
                hit_boxes,
                children_hb_start,
                &laid_children,
                &all_child_hb_ranges,
            );
            let rect = LayoutRect::new(x, y, box_w, h);
            let (scroll_offset_y, scroll_offset_x) = apply_container_scroll_and_clip(
                node,
                rect,
                content_h,
                content_w,
                path,
                scroll_offsets,
                &mut laid_children,
                hit_boxes,
                children_hb_start,
            );
            insert_container_hit(children_hb_start, rect, hit_boxes);
            LayoutNode {
                rect,
                node: node.clone(),
                wrapped_lines: Vec::new(),
                line_height: node.font_size * 1.3,
                content_height: content_h,
                scroll_offset_y,
                content_width: content_w,
                scroll_offset_x,
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
