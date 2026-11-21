//! High-Quality Anti-Aliased 2D Vector & TrueType/OpenType UI Renderer (CPU Fallback & Headless Engine)
//! backed by `tiny-skia` (4-corner rounded rects, linear/radial gradients, soft shadows, transforms,
//! interactive widgets, procedural shader preview, images) and `ab_glyph` (system TrueType/OpenType fonts)
//! with deterministic 8x8 bitmap fallback.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;

use ab_glyph::{Font, FontVec, PxScale, ScaleFont};
use tiny_skia::{
    Color as SkColor, FillRule, GradientStop, LinearGradient, Paint, PathBuilder, Pixmap,
    PixmapPaint, Point, RadialGradient, SpreadMode, Stroke, Transform,
};

use crate::gui::font::get_glyph;
use crate::gui::ui_tree::{
    compute_ui_layout, parse_fill_spec, HitBox, LayoutNode, LayoutRect, ParsedFill, TextMeasurer,
    UiKind, UiNode,
};

const FONT_CANDIDATE_PATHS: &[&str] = &[
    // Linux (Fedora / Ubuntu / Arch / Debian)
    "/usr/share/fonts/rsms-inter-fonts/Inter-Regular.ttf",
    "/usr/share/fonts/rsms-inter-fonts/Inter-Medium.ttf",
    "/usr/share/fonts/adwaita-sans-fonts/AdwaitaSans-Regular.ttf",
    "/usr/share/fonts/abattis-cantarell-fonts/Cantarell-Regular.otf",
    "/usr/share/fonts/abattis-cantarell-vf-fonts/Cantarell-VF.otf",
    "/usr/share/fonts/open-sans/OpenSans-Regular.ttf",
    "/usr/share/fonts/liberation-sans-fonts/LiberationSans-Regular.ttf",
    "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
    "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf",
    "/usr/share/fonts/aajohan-comfortaa-fonts/Comfortaa-Bold.otf",
    // macOS
    "/System/Library/Fonts/SFNS.ttf",
    "/System/Library/Fonts/Helvetica.ttc",
    // Windows
    "C:\\Windows\\Fonts\\segoeui.ttf",
    "C:\\Windows\\Fonts\\arial.ttf",
];

/// Interactive render state passed to both GPU and CPU renderers.
#[derive(Debug, Default, Clone)]
pub struct RenderInteractionState<'a> {
    pub hovered_action: Option<&'a str>,
    pub focused_action: Option<&'a str>,
    pub elapsed_sec: f32,
}

/// Parses hex colors (`#RGB`, `#RRGGBB`, `#RRGGBBAA`) into `(r, g, b, a)`.
pub fn parse_hex_color(hex: &str) -> Option<(u8, u8, u8, u8)> {
    let s = hex.trim().strip_prefix('#')?;
    match s.len() {
        3 => {
            let r = u8::from_str_radix(&s[0..1], 16).ok()? * 17;
            let g = u8::from_str_radix(&s[1..2], 16).ok()? * 17;
            let b = u8::from_str_radix(&s[2..3], 16).ok()? * 17;
            Some((r, g, b, 255))
        }
        6 => {
            let r = u8::from_str_radix(&s[0..2], 16).ok()?;
            let g = u8::from_str_radix(&s[2..4], 16).ok()?;
            let b = u8::from_str_radix(&s[4..6], 16).ok()?;
            Some((r, g, b, 255))
        }
        8 => {
            let r = u8::from_str_radix(&s[0..2], 16).ok()?;
            let g = u8::from_str_radix(&s[2..4], 16).ok()?;
            let b = u8::from_str_radix(&s[4..6], 16).ok()?;
            let a = u8::from_str_radix(&s[6..8], 16).ok()?;
            Some((r, g, b, a))
        }
        _ => None,
    }
}

/// Cached rasterized glyph coverage bitmap (avoids re-outlining every character every frame).
struct CachedGlyph {
    w: i32,
    h: i32,
    off_x: i32,
    off_y: i32,
    alpha: Vec<u8>,
}

/// Anti-aliased vector & TrueType/OpenType UI renderer (CPU + Headless engine).
pub struct UiRenderer {
    font: Option<FontVec>,
    image_cache: RefCell<HashMap<String, Option<Pixmap>>>,
    glyph_cache: RefCell<HashMap<(char, u32), Option<CachedGlyph>>>,
}

impl Default for UiRenderer {
    fn default() -> Self {
        Self::new()
    }
}

impl UiRenderer {
    pub fn new() -> Self {
        let mut font = None;
        for path_str in FONT_CANDIDATE_PATHS {
            let p = Path::new(path_str);
            if p.is_file() {
                if let Ok(bytes) = std::fs::read(p) {
                    if let Ok(fv) = FontVec::try_from_vec(bytes) {
                        font = Some(fv);
                        break;
                    }
                }
            }
        }
        Self {
            font,
            image_cache: RefCell::new(HashMap::new()),
            glyph_cache: RefCell::new(HashMap::new()),
        }
    }

    pub fn has_truetype_font(&self) -> bool {
        self.font.is_some()
    }

    pub fn font(&self) -> Option<&FontVec> {
        self.font.as_ref()
    }

    /// Renders `root` into a new `tiny_skia::Pixmap` and returns `(pixmap, hit_boxes)`.
    pub fn render_ui(
        &self,
        root: &UiNode,
        width: u32,
        height: u32,
        hovered_action: Option<&str>,
    ) -> (Pixmap, Vec<HitBox>) {
        self.render_ui_with_state(
            root,
            width,
            height,
            &RenderInteractionState {
                hovered_action,
                focused_action: None,
                elapsed_sec: 0.0,
            },
        )
    }

    /// Renders `root` with full interaction & animation state into a new `tiny_skia::Pixmap`.
    pub fn render_ui_with_state(
        &self,
        root: &UiNode,
        width: u32,
        height: u32,
        state: &RenderInteractionState<'_>,
    ) -> (Pixmap, Vec<HitBox>) {
        let w = width.max(1);
        let h = height.max(1);
        let (layout_root, hit_boxes) = compute_ui_layout(root, w as f32, h as f32, self);
        let pixmap = self.render_layout(&layout_root, w, h, state);
        (pixmap, hit_boxes)
    }

    /// Renders a pre-computed (and optionally `UiAnimator`-stepped) `LayoutNode` tree into a `Pixmap`.
    pub fn render_layout(
        &self,
        layout_root: &LayoutNode,
        width: u32,
        height: u32,
        state: &RenderInteractionState<'_>,
    ) -> Pixmap {
        let w = width.max(1);
        let h = height.max(1);
        let mut pixmap = Pixmap::new(w, h).expect("create tiny-skia pixmap");
        self.render_layout_into(&mut pixmap, layout_root, state);
        pixmap
    }

    /// Renders a pre-computed `LayoutNode` tree into an existing reusable `Pixmap` (zero frame alloc!).
    pub fn render_layout_into(
        &self,
        pixmap: &mut Pixmap,
        layout_root: &LayoutNode,
        state: &RenderInteractionState<'_>,
    ) {
        self.render_layout_into_ex(pixmap, layout_root, state, None, false);
    }

    /// Renders a pre-computed `LayoutNode` tree into an existing reusable `Pixmap` with optional
    /// visual scrollbar `(scroll_y, content_height)` and input select-all highlight.
    pub fn render_layout_into_ex(
        &self,
        pixmap: &mut Pixmap,
        layout_root: &LayoutNode,
        state: &RenderInteractionState<'_>,
        scroll_state: Option<(f32, f32)>,
        input_selected_all: bool,
    ) {
        let (br, bg, bb, ba) = parse_hex_color(&layout_root.node.bg).unwrap_or((11, 17, 32, 255));
        pixmap.fill(SkColor::from_rgba8(br, bg, bb, ba));
        self.paint_layout_node(
            pixmap,
            layout_root,
            state,
            1.0,
            Transform::identity(),
            input_selected_all,
        );
        if let Some((scroll_y, content_height)) = scroll_state {
            self.paint_scrollbar(pixmap, scroll_y, content_height);
        }
    }

    /// Draws a subtle translucent vertical pill scrollbar along the right edge when content
    /// overflows the viewport height.
    pub fn paint_scrollbar(&self, pixmap: &mut Pixmap, scroll_y: f32, content_height: f32) {
        let logical_w = pixmap.width() as f32;
        let logical_h = pixmap.height() as f32;
        if content_height <= logical_h + 2.0 || logical_h <= 24.0 || logical_w <= 16.0 {
            return;
        }
        let max_thumb = (logical_h - 16.0).max(4.0);
        let min_thumb = 28.0f32.min(max_thumb);
        let thumb_h = (logical_h / content_height * logical_h).clamp(min_thumb, max_thumb);
        let max_scroll = (content_height - logical_h).max(1.0);
        let ratio = (scroll_y / max_scroll).clamp(0.0, 1.0);
        let thumb_y = ratio * (logical_h - thumb_h - 8.0).max(0.0) + 4.0;
        let track_x = logical_w - 8.0;

        let track_rect = LayoutRect::new(track_x, 4.0, 5.0, (logical_h - 8.0).max(4.0));
        fill_rounded_rect_solid(
            pixmap,
            track_rect,
            [2.5; 4],
            (15, 23, 42, 72),
            Transform::identity(),
        );
        let thumb_rect = LayoutRect::new(track_x, thumb_y, 5.0, thumb_h);
        fill_rounded_rect_solid(
            pixmap,
            thumb_rect,
            [2.5; 4],
            (148, 163, 184, 133),
            Transform::identity(),
        );
        stroke_rounded_rect_corners(
            pixmap,
            thumb_rect,
            [2.5; 4],
            (56, 189, 248, 64),
            0.75,
            Transform::identity(),
        );
    }

    /// Fast incremental CPU update: repaints ONLY `UiKind::Shader` viewports (and their border)
    /// onto an already-rendered UI `Pixmap`, skipping background gradients, shadows, and text.
    pub fn render_shaders_only(
        &self,
        pixmap: &mut Pixmap,
        layout_root: &LayoutNode,
        elapsed_sec: f32,
    ) {
        self.paint_shaders_only_node(pixmap, layout_root, elapsed_sec, 1.0);
    }

    fn paint_shaders_only_node(
        &self,
        pixmap: &mut Pixmap,
        laid: &LayoutNode,
        elapsed_sec: f32,
        parent_opacity: f32,
    ) {
        let node = &laid.node;
        let rect = laid.rect;
        let opacity = (parent_opacity * node.style.opacity).clamp(0.0, 1.0);
        if opacity <= 0.004 || rect.w <= 0.0 || rect.h <= 0.0 {
            return;
        }
        if rect.y + rect.h < -64.0 || rect.y > pixmap.height() as f32 + 64.0 {
            return;
        }
        if node.kind == UiKind::Shader {
            let radii = node.effective_radii();
            paint_cpu_shader_preview(
                pixmap,
                rect,
                radii,
                &node.text,
                elapsed_sec,
                &node.uniforms,
                opacity,
            );
            if let Some((r, g, b, a)) = parse_hex_color(&node.border) {
                let bw = node.style.border_w.max(1.0);
                let aa = (a as f32 * opacity).round().clamp(0.0, 255.0) as u8;
                stroke_rounded_rect_corners(
                    pixmap,
                    rect,
                    radii,
                    (r, g, b, aa),
                    bw,
                    Transform::identity(),
                );
            }
        }
        for child in &laid.children {
            self.paint_shaders_only_node(pixmap, child, elapsed_sec, opacity);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn paint_layout_node(
        &self,
        pixmap: &mut Pixmap,
        laid: &LayoutNode,
        state: &RenderInteractionState<'_>,
        parent_opacity: f32,
        parent_xform: Transform,
        input_selected_all: bool,
    ) {
        let node = &laid.node;
        let rect = laid.rect;
        let opacity = (parent_opacity * node.style.opacity).clamp(0.0, 1.0);
        if opacity <= 0.004 || rect.w <= 0.0 || rect.h <= 0.0 {
            return;
        }
        // Cull completely off-screen subtrees when scrolled
        if rect.y + rect.h < -64.0 || rect.y > pixmap.height() as f32 + 64.0 {
            return;
        }

        let is_hovered =
            !node.action.is_empty() && state.hovered_action == Some(node.action.as_str());
        let is_focused =
            !node.action.is_empty() && state.focused_action == Some(node.action.as_str());

        let local_xform = compute_node_transform(rect, &node.style);
        let xform = parent_xform.post_concat(local_xform);
        let raw_radii = node.effective_radii();
        let radii = if raw_radii.iter().all(|&r| r <= 0.0) {
            match node.kind {
                UiKind::Progress => [rect.h * 0.5; 4],
                UiKind::Divider => [(rect.h * 0.5).min(2.0); 4],
                _ => raw_radii,
            }
        } else {
            raw_radii
        };

        // 1. Soft drop shadow
        if node.style.shadow_blur > 0.0 && !node.style.shadow_color.is_empty() {
            if let Some((sr, sg, sb, sa)) = parse_hex_color(&node.style.shadow_color) {
                let steps = 3usize;
                for i in (0..steps).rev() {
                    let spread = (node.style.shadow_blur * (i as f32 + 1.0) / steps as f32) * 0.45;
                    let step_a =
                        ((sa as f32 * opacity) / (steps as f32 + 0.6)).round().clamp(0.0, 255.0) as u8;
                    let srect = LayoutRect::new(
                        rect.x + node.style.shadow_x - spread,
                        rect.y + node.style.shadow_y - spread,
                        rect.w + spread * 2.0,
                        rect.h + spread * 2.0,
                    );
                    let sradii = [
                        radii[0] + spread,
                        radii[1] + spread,
                        radii[2] + spread,
                        radii[3] + spread,
                    ];
                    fill_rounded_rect_solid(pixmap, srect, sradii, (sr, sg, sb, step_a), xform);
                }
            }
        } else if matches!(node.kind, UiKind::Card | UiKind::Button) && !node.bg.is_empty() {
            let srect = LayoutRect::new(rect.x, rect.y + 3.0, rect.w, rect.h);
            let sa = (72.0 * opacity).round().clamp(0.0, 255.0) as u8;
            fill_rounded_rect_solid(pixmap, srect, radii, (0, 0, 0, sa), xform);
        }

        // 2. Background fill (Solid / LinearGradient / RadialGradient + hover override)
        let active_bg = if is_hovered && !node.style.hover_bg.is_empty() {
            node.style.hover_bg.as_str()
        } else {
            node.bg.as_str()
        };
        let fill = parse_fill_spec(active_bg);
        match fill {
            ParsedFill::None => {}
            ParsedFill::Solid(mut rgba) => {
                if is_hovered && node.style.hover_bg.is_empty() {
                    rgba[0] = (rgba[0] + 0.08).min(1.0);
                    rgba[1] = (rgba[1] + 0.08).min(1.0);
                    rgba[2] = (rgba[2] + 0.08).min(1.0);
                }
                rgba[3] *= opacity;
                fill_rounded_rect_solid(pixmap, rect, radii, f32_to_rgba8(rgba), xform);
            }
            ParsedFill::LinearGradient {
                angle_deg,
                mut start,
                mut end,
            } => {
                start[3] *= opacity;
                end[3] *= opacity;
                fill_rounded_rect_linear_grad(pixmap, rect, radii, angle_deg, start, end, xform);
            }
            ParsedFill::RadialGradient {
                mut center,
                mut edge,
            } => {
                center[3] *= opacity;
                edge[3] *= opacity;
                fill_rounded_rect_radial_grad(pixmap, rect, radii, center, edge, xform);
            }
        }

        // 3. Specialized interactive widget surfaces
        match node.kind {
            UiKind::Shader => {
                paint_cpu_shader_preview(
                    pixmap,
                    rect,
                    radii,
                    &node.text,
                    state.elapsed_sec,
                    &node.uniforms,
                    opacity,
                );
            }
            UiKind::Image => {
                self.paint_image_widget(pixmap, rect, &node.text, opacity, xform);
            }
            UiKind::Slider => {
                let t = node.value.clamp(0.0, 1.0) as f32;
                let track_h = 8.0f32;
                let track_y = rect.y + (rect.h - track_h) * 0.5;
                let track = LayoutRect::new(rect.x, track_y, rect.w, track_h);
                fill_rounded_rect_solid(
                    pixmap,
                    track,
                    [4.0; 4],
                    (30, 41, 59, (230.0 * opacity) as u8),
                    xform,
                );
                let fill_w = (rect.w * t).max(8.0).min(rect.w);
                let filled = LayoutRect::new(rect.x, track_y, fill_w, track_h);
                fill_rounded_rect_linear_grad(
                    pixmap,
                    filled,
                    [4.0; 4],
                    90.0,
                    [0.22, 0.74, 0.97, opacity],
                    [0.51, 0.55, 0.97, opacity],
                    xform,
                );
                let thumb_r = 9.0f32;
                let thumb_x = rect.x + (rect.w - thumb_r * 2.0) * t;
                let thumb_y = rect.y + (rect.h - thumb_r * 2.0) * 0.5;
                let thumb = LayoutRect::new(thumb_x, thumb_y, thumb_r * 2.0, thumb_r * 2.0);
                fill_rounded_rect_solid(
                    pixmap,
                    thumb,
                    [thumb_r; 4],
                    (248, 250, 252, (255.0 * opacity) as u8),
                    xform,
                );
                stroke_rounded_rect_corners(
                    pixmap,
                    thumb,
                    [thumb_r; 4],
                    (56, 189, 248, (255.0 * opacity) as u8),
                    2.0,
                    xform,
                );
            }
            UiKind::Toggle => {
                let checked = node.value > 0.5;
                let pill_w = 42.0f32;
                let pill_h = 24.0f32;
                let pill_x = rect.x + node.effective_pad()[3];
                let pill_y = rect.y + (rect.h - pill_h) * 0.5;
                let pill = LayoutRect::new(pill_x, pill_y, pill_w, pill_h);
                let pill_col = if checked {
                    (56, 189, 248, (255.0 * opacity) as u8)
                } else {
                    (51, 65, 85, (255.0 * opacity) as u8)
                };
                fill_rounded_rect_solid(pixmap, pill, [12.0; 4], pill_col, xform);
                let knob_x = if checked { pill_x + pill_w - 20.0 } else { pill_x + 4.0 };
                let knob = LayoutRect::new(knob_x, pill_y + 4.0, 16.0, 16.0);
                fill_rounded_rect_solid(
                    pixmap,
                    knob,
                    [8.0; 4],
                    (248, 250, 252, (255.0 * opacity) as u8),
                    xform,
                );
            }
            UiKind::Divider => {
                if node.bg.is_empty() {
                    let (r, g, b, a) = if !node.border.is_empty() {
                        parse_hex_color(&node.border).unwrap_or((51, 65, 85, 170))
                    } else {
                        (51, 65, 85, 170)
                    };
                    let aa = (a as f32 * opacity).round().clamp(0.0, 255.0) as u8;
                    fill_rounded_rect_solid(pixmap, rect, radii, (r, g, b, aa), xform);
                }
            }
            UiKind::Progress => {
                if node.bg.is_empty() {
                    fill_rounded_rect_solid(
                        pixmap,
                        rect,
                        radii,
                        (30, 41, 59, (235.0 * opacity) as u8),
                        xform,
                    );
                }
                let t = node.value.clamp(0.0, 1.0) as f32;
                if t > 0.001 {
                    let fill_w = (rect.w * t).max(2.0).min(rect.w);
                    let filled = LayoutRect::new(rect.x, rect.y, fill_w, rect.h);
                    fill_rounded_rect_linear_grad(
                        pixmap,
                        filled,
                        radii,
                        90.0,
                        [0.22, 0.74, 0.97, opacity],
                        [0.51, 0.55, 0.97, opacity],
                        xform,
                    );
                }
            }
            UiKind::Input if node.bg.is_empty() => {
                fill_rounded_rect_solid(
                    pixmap,
                    rect,
                    radii,
                    (15, 23, 42, (235.0 * opacity) as u8),
                    xform,
                );
            }
            _ => {}
        }

        // 4. Border stroke (with focus ring for Input and hover_border support)
        if node.kind != UiKind::Divider {
            let active_border = if is_focused {
                if input_selected_all && node.kind == UiKind::Input {
                    "#7dd3fc"
                } else {
                    "#38bdf8"
                }
            } else if is_hovered && !node.style.hover_border.is_empty() {
                node.style.hover_border.as_str()
            } else {
                node.border.as_str()
            };
            if let Some((r, g, b, a)) = parse_hex_color(active_border) {
                let bw = if is_focused {
                    if input_selected_all && node.kind == UiKind::Input {
                        2.25
                    } else {
                        2.0
                    }
                } else if is_hovered {
                    node.style.border_w.max(1.5)
                } else {
                    node.style.border_w.max(1.0)
                };
                let aa = (a as f32 * opacity).round().clamp(0.0, 255.0) as u8;
                stroke_rounded_rect_corners(pixmap, rect, radii, (r, g, b, aa), bw, xform);
            } else if is_hovered && matches!(node.kind, UiKind::Button) {
                let aa = (96.0 * opacity).round().clamp(0.0, 255.0) as u8;
                stroke_rounded_rect_corners(pixmap, rect, radii, (255, 255, 255, aa), 1.5, xform);
            }
        }

        // 5. Text rendering (uses pre-wrapped multi-line layout so text never overflows!)
        if !laid.wrapped_lines.is_empty()
            && !matches!(
                node.kind,
                UiKind::Shader
                    | UiKind::Image
                    | UiKind::Slider
                    | UiKind::Divider
                    | UiKind::Progress
            )
        {
            let (tr, tg, tb, ta) = parse_hex_color(&node.color).unwrap_or((248, 250, 252, 255));
            let color = (tr, tg, tb, (ta as f32 * opacity).round().clamp(0.0, 255.0) as u8);
            let [pt, _pr, _pb, pl] = node.effective_pad();
            let first_line = &laid.wrapped_lines[0];
            let (tw, th) = self.measure_text_size(first_line, node.font_size);
            let (tx, ty) = match node.kind {
                UiKind::Button | UiKind::Badge => (
                    rect.x + ((rect.w - tw) * 0.5).max(0.0),
                    rect.y + ((rect.h - th) * 0.5).max(0.0),
                ),
                UiKind::Input => (
                    rect.x + pl.max(12.0),
                    rect.y + ((rect.h - th) * 0.5).max(0.0),
                ),
                UiKind::Toggle => (
                    rect.x + pl + 52.0,
                    rect.y + ((rect.h - th) * 0.5).max(0.0),
                ),
                _ => (rect.x + pl, rect.y + pt),
            };
            let tx_final = tx + node.style.translate_x;
            let ty_final = ty + node.style.translate_y;

            // Subtle cyan selection highlight behind input text when Ctrl+A is active
            if node.kind == UiKind::Input && is_focused && input_selected_all && tw > 0.0 {
                let sel_pad_x = 3.0f32;
                let sel_pad_y = 2.0f32;
                let max_sel_w = (rect.w - (tx_final - rect.x) - 8.0).max(0.0);
                let sel_w = (tw + sel_pad_x * 2.0).min(max_sel_w);
                if sel_w > 0.0 {
                    let sel_rect = LayoutRect::new(
                        tx_final - sel_pad_x,
                        ty_final - sel_pad_y,
                        sel_w,
                        th.max(14.0) + sel_pad_y * 2.0,
                    );
                    let sa = (84.0 * opacity).round().clamp(0.0, 255.0) as u8;
                    fill_rounded_rect_solid(pixmap, sel_rect, [4.0; 4], (56, 189, 248, sa), xform);
                }
            }

            for (line_idx, line) in laid.wrapped_lines.iter().enumerate() {
                if !line.is_empty() {
                    let ly = ty_final + (line_idx as f32) * laid.line_height;
                    self.draw_text(pixmap, line, tx_final, ly, node.font_size, color);
                }
            }

            // Focused text input caret
            if node.kind == UiKind::Input && is_focused {
                let caret_rect = LayoutRect::new(tx_final + tw + 2.0, ty_final, 2.0, th.max(14.0));
                fill_rounded_rect_solid(pixmap, caret_rect, [1.0; 4], (56, 189, 248, 255), xform);
            }
        } else if node.kind == UiKind::Input && is_focused {
            let [_, _, _, pl] = node.effective_pad();
            let (_, th) = self.measure_text_size("Mg", node.font_size);
            let tx_final = rect.x + pl.max(12.0) + node.style.translate_x;
            let ty_final = rect.y + ((rect.h - th) * 0.5).max(0.0) + node.style.translate_y;
            let caret_rect = LayoutRect::new(tx_final, ty_final, 2.0, th.max(14.0));
            fill_rounded_rect_solid(pixmap, caret_rect, [1.0; 4], (56, 189, 248, 255), xform);
        }

        // 6. Children
        for child in &laid.children {
            self.paint_layout_node(pixmap, child, state, opacity, xform, input_selected_all);
        }
    }

    fn paint_image_widget(
        &self,
        pixmap: &mut Pixmap,
        rect: LayoutRect,
        path: &str,
        opacity: f32,
        xform: Transform,
    ) {
        if path.is_empty() {
            return;
        }
        let mut cache = self.image_cache.borrow_mut();
        let entry = cache.entry(path.to_string()).or_insert_with(|| {
            let img = image::open(path).ok()?.to_rgba8();
            let (iw, ih) = img.dimensions();
            let mut pm = Pixmap::new(iw.max(1), ih.max(1))?;
            let dst = pm.data_mut();
            for (i, px) in img.pixels().enumerate() {
                let a = px[3] as f32 / 255.0;
                let base = i * 4;
                dst[base] = (px[0] as f32 * a).round() as u8;
                dst[base + 1] = (px[1] as f32 * a).round() as u8;
                dst[base + 2] = (px[2] as f32 * a).round() as u8;
                dst[base + 3] = px[3];
            }
            Some(pm)
        });
        if let Some(src_pm) = entry.as_ref() {
            let sx = rect.w / (src_pm.width().max(1) as f32);
            let sy = rect.h / (src_pm.height().max(1) as f32);
            let img_xform = xform.pre_translate(rect.x, rect.y).pre_scale(sx, sy);
            let paint = PixmapPaint {
                opacity,
                ..PixmapPaint::default()
            };
            pixmap.draw_pixmap(0, 0, src_pm.as_ref(), &paint, img_xform, None);
        }
    }

    fn draw_text(
        &self,
        pixmap: &mut Pixmap,
        text: &str,
        x: f32,
        y: f32,
        font_size: f32,
        color: (u8, u8, u8, u8),
    ) {
        if let Some(ref font) = self.font {
            let px_size = (font_size.round() as u32).clamp(8, 96);
            let scale = PxScale::from(px_size as f32);
            let scaled = font.as_scaled(scale);
            let ascent = scaled.ascent();
            let mut caret_x = x;
            let mut prev_glyph = None;

            let pw = pixmap.width() as i32;
            let ph = pixmap.height() as i32;
            let data = pixmap.data_mut();
            let mut cache = self.glyph_cache.borrow_mut();

            for ch in text.chars() {
                let glyph_id = scaled.glyph_id(ch);
                if let Some(prev) = prev_glyph {
                    caret_x += scaled.kern(prev, glyph_id);
                }

                let cached = cache.entry((ch, px_size)).or_insert_with(|| {
                    let glyph = glyph_id
                        .with_scale_and_position(scale, ab_glyph::point(0.0, ascent));
                    let outlined = font.outline_glyph(glyph)?;
                    let bounds = outlined.px_bounds();
                    let w = (bounds.width().ceil() as i32).max(1);
                    let h = (bounds.height().ceil() as i32).max(1);
                    let mut alpha = vec![0u8; (w * h) as usize];
                    outlined.draw(|gx, gy, cov| {
                        let ix = gx as i32;
                        let iy = gy as i32;
                        if ix >= 0 && ix < w && iy >= 0 && iy < h {
                            alpha[(iy * w + ix) as usize] =
                                (cov.clamp(0.0, 1.0) * 255.0).round() as u8;
                        }
                    });
                    Some(CachedGlyph {
                        w,
                        h,
                        off_x: bounds.min.x.round() as i32,
                        off_y: bounds.min.y.round() as i32,
                        alpha,
                    })
                });

                if let Some(g) = cached.as_ref() {
                    let base_x = caret_x.round() as i32 + g.off_x;
                    let base_y = y.round() as i32 + g.off_y;
                    let col_a = color.3 as f32 / 255.0;
                    for gy in 0..g.h {
                        let py = base_y + gy;
                        if py < 0 || py >= ph {
                            continue;
                        }
                        let row_off = (gy * g.w) as usize;
                        let dst_row = (py as usize) * (pw as usize);
                        for gx in 0..g.w {
                            let cov_u8 = g.alpha[row_off + gx as usize];
                            if cov_u8 == 0 {
                                continue;
                            }
                            let px = base_x + gx;
                            if px >= 0 && px < pw {
                                let alpha = (cov_u8 as f32 / 255.0) * col_a;
                                let idx = (dst_row + px as usize) * 4;
                                blend_premul_rgba(
                                    &mut data[idx..idx + 4],
                                    color.0,
                                    color.1,
                                    color.2,
                                    alpha,
                                );
                            }
                        }
                    }
                }
                caret_x += scaled.h_advance(glyph_id);
                prev_glyph = Some(glyph_id);
            }
        } else {
            let scale_i = (font_size / 8.0).round().max(1.0) as i32;
            let pw = pixmap.width() as i32;
            let ph = pixmap.height() as i32;
            let data = pixmap.data_mut();
            let mut cx = x as i32;
            let cy = y as i32;
            for ch in text.chars() {
                let glyph = get_glyph(ch);
                for (row, &bits) in glyph.iter().enumerate() {
                    for col in 0..8 {
                        if (bits & (1 << (7 - col))) != 0 {
                            for sy in 0..scale_i {
                                for sx in 0..scale_i {
                                    let px = cx + col * scale_i + sx;
                                    let py = cy + (row as i32) * scale_i + sy;
                                    if px >= 0 && px < pw && py >= 0 && py < ph {
                                        let idx =
                                            ((py as usize) * (pw as usize) + (px as usize)) * 4;
                                        blend_premul_rgba(
                                            &mut data[idx..idx + 4],
                                            color.0,
                                            color.1,
                                            color.2,
                                            color.3 as f32 / 255.0,
                                        );
                                    }
                                }
                            }
                        }
                    }
                }
                cx += 8 * scale_i;
            }
        }
    }
}

impl TextMeasurer for UiRenderer {
    fn measure_text_size(&self, text: &str, font_size: f32) -> (f32, f32) {
        if text.is_empty() {
            return (0.0, font_size.max(8.0) * 1.25);
        }
        if let Some(ref font) = self.font {
            let scale = PxScale::from(font_size.max(8.0));
            let scaled = font.as_scaled(scale);
            let mut w: f32 = 0.0;
            let mut prev = None;
            for ch in text.chars() {
                let id = scaled.glyph_id(ch);
                if let Some(p) = prev {
                    w += scaled.kern(p, id);
                }
                w += scaled.h_advance(id);
                prev = Some(id);
            }
            let h = (scaled.ascent() - scaled.descent()).max(font_size * 1.2);
            (w.ceil(), h.ceil())
        } else {
            let s = (font_size / 8.0).round().max(1.0);
            ((text.chars().count() as f32) * 8.0 * s, 8.0 * s * 1.25)
        }
    }
}

fn compute_node_transform(rect: LayoutRect, st: &crate::gui::ui_tree::UiStyle) -> Transform {
    let has_tx = st.translate_x.abs() > 0.01 || st.translate_y.abs() > 0.01;
    let has_sc = (st.scale - 1.0).abs() > 0.002;
    let has_rot = st.rotate_deg.abs() > 0.01;
    if !has_tx && !has_sc && !has_rot {
        return Transform::identity();
    }
    let cx = rect.x + rect.w * 0.5;
    let cy = rect.y + rect.h * 0.5;
    let mut t = Transform::from_translate(cx + st.translate_x, cy + st.translate_y);
    if has_rot {
        t = t.pre_rotate(st.rotate_deg);
    }
    if has_sc {
        t = t.pre_scale(st.scale, st.scale);
    }
    t.pre_translate(-cx, -cy)
}

#[inline]
fn f32_to_rgba8(c: [f32; 4]) -> (u8, u8, u8, u8) {
    (
        (c[0].clamp(0.0, 1.0) * 255.0).round() as u8,
        (c[1].clamp(0.0, 1.0) * 255.0).round() as u8,
        (c[2].clamp(0.0, 1.0) * 255.0).round() as u8,
        (c[3].clamp(0.0, 1.0) * 255.0).round() as u8,
    )
}

#[inline]
fn blend_premul_rgba(dst: &mut [u8], sr: u8, sg: u8, sb: u8, sa: f32) {
    let inv = 1.0 - sa;
    let nr = (sr as f32 * sa + dst[0] as f32 * inv).round().clamp(0.0, 255.0) as u8;
    let ng = (sg as f32 * sa + dst[1] as f32 * inv).round().clamp(0.0, 255.0) as u8;
    let nb = (sb as f32 * sa + dst[2] as f32 * inv).round().clamp(0.0, 255.0) as u8;
    let na = (255.0 * sa + dst[3] as f32 * inv).round().clamp(0.0, 255.0) as u8;
    dst[0] = nr;
    dst[1] = ng;
    dst[2] = nb;
    dst[3] = na;
}

/// Fast deterministic CPU fallback preview for `UiKind::Shader` widgets.
/// Evaluates the procedural shader on a 4x4 tile grid (16x faster than per-pixel trig!)
/// and applies exact rounded-corner clipping only along the corners.
fn paint_cpu_shader_preview(
    pixmap: &mut Pixmap,
    rect: LayoutRect,
    radii: [f32; 4],
    shader_name: &str,
    elapsed: f32,
    uniforms: &[f64],
    opacity: f32,
) {
    let pw = pixmap.width() as i32;
    let ph = pixmap.height() as i32;
    let x0 = (rect.x.floor() as i32).max(0);
    let y0 = (rect.y.floor() as i32).max(0);
    let x1 = ((rect.x + rect.w).ceil() as i32).min(pw);
    let y1 = ((rect.y + rect.h).ceil() as i32).min(ph);
    if x1 <= x0 || y1 <= y0 {
        return;
    }

    let speed = uniforms.first().copied().unwrap_or(1.0) as f32;
    let intensity = uniforms.get(1).copied().unwrap_or(0.85) as f32;
    let zoom = uniforms.get(2).copied().unwrap_or(1.0).max(0.25) as f32;
    let t = elapsed * speed;
    let is_plasma = shader_name.contains("plasma");

    let half_w = rect.w * 0.5;
    let half_h = rect.h * 0.5;
    let max_rad = radii.iter().copied().fold(0.0f32, f32::max).min(half_w.min(half_h));
    let inv_w = 1.0 / rect.w.max(1.0);
    let inv_h = 1.0 / rect.h.max(1.0);
    let opaque_fast = opacity >= 0.995;
    let data = pixmap.data_mut();

    const STEP: i32 = 4;
    let mut ty = y0;
    while ty < y1 {
        let by_end = (ty + STEP).min(y1);
        let v = ((ty as f32 + 1.5) - rect.y) * inv_h;
        let sin_v3 = (v * 3.0).sin();
        let near_y_edge = (ty as f32 - rect.y) < max_rad + 4.0
            || (rect.y + rect.h - by_end as f32) < max_rad + 4.0;

        let mut tx = x0;
        while tx < x1 {
            let bx_end = (tx + STEP).min(x1);
            let u = ((tx as f32 + 1.5) - rect.x) * inv_w;

            let (rf, gf, bf) = if is_plasma {
                let cx = (u - 0.5) * zoom * 4.0;
                let cy = (v - 0.5) * zoom * 4.0;
                let dist = (cx * cx + cy * cy).sqrt();
                let w = ((dist * 3.0 - t * 2.2).sin()
                    + (cx * 2.0 + t).cos()
                    + (cy * 2.0 - t).sin())
                    * 0.333;
                let n = (w * 0.5 + 0.5).clamp(0.0, 1.0) * intensity;
                (
                    (0.18 + 0.72 * n).clamp(0.0, 1.0),
                    (0.12 + 0.45 * (1.0 - n) + 0.3 * (t + u * 3.0).sin().abs()).clamp(0.0, 1.0),
                    (0.35 + 0.65 * (0.5 + 0.5 * (dist - t).cos())).clamp(0.0, 1.0),
                )
            } else {
                let w1 = (u * 5.0 * zoom + t * 1.4 + sin_v3).sin();
                let w2 = ((u * 3.0 - v * 4.0) * zoom - t * 1.1).cos();
                let band = (0.5 + 0.5 * (w1 * 0.55 + w2 * 0.45)) * intensity;
                (
                    (0.06 + 0.32 * band * (1.0 - v)).clamp(0.0, 1.0),
                    (0.14 + 0.68 * band).clamp(0.0, 1.0),
                    (0.28 + 0.68 * (0.4 + 0.6 * band)).clamp(0.0, 1.0),
                )
            };

            let r8 = (rf * 255.0) as u8;
            let g8 = (gf * 255.0) as u8;
            let b8 = (bf * 255.0) as u8;

            let near_corner = near_y_edge
                && ((tx as f32 - rect.x) < max_rad + 4.0
                    || (rect.x + rect.w - bx_end as f32) < max_rad + 4.0);

            if !near_corner && opaque_fast {
                // Fast interior block fill — zero trig, zero SDF, zero alpha math!
                for py in ty..by_end {
                    let row = (py as usize) * (pw as usize);
                    for px in tx..bx_end {
                        let idx = (row + px as usize) * 4;
                        data[idx] = r8;
                        data[idx + 1] = g8;
                        data[idx + 2] = b8;
                        data[idx + 3] = 255;
                    }
                }
            } else {
                for py in ty..by_end {
                    let ly = (py as f32 - rect.y) - half_h;
                    let row = (py as usize) * (pw as usize);
                    for px in tx..bx_end {
                        let lx = (px as f32 - rect.x) - half_w;
                        let r = if lx > 0.0 {
                            if ly > 0.0 { radii[2] } else { radii[1] }
                        } else if ly > 0.0 {
                            radii[3]
                        } else {
                            radii[0]
                        }
                        .clamp(0.0, half_w.min(half_h));
                        let qx = lx.abs() - half_w + r;
                        let qy = ly.abs() - half_h + r;
                        let d = (qx.max(0.0).hypot(qy.max(0.0))) + qx.max(qy).min(0.0) - r;
                        let mask = (0.5 - d).clamp(0.0, 1.0) * opacity;
                        if mask <= 0.004 {
                            continue;
                        }
                        let idx = (row + px as usize) * 4;
                        blend_premul_rgba(&mut data[idx..idx + 4], r8, g8, b8, mask);
                    }
                }
            }
            tx += STEP;
        }
        ty += STEP;
    }
}

fn build_rounded_rect_path_corners(rect: LayoutRect, radii: [f32; 4]) -> Option<tiny_skia::Path> {
    if rect.w <= 0.0 || rect.h <= 0.0 {
        return None;
    }
    let max_r = (rect.w * 0.5).min(rect.h * 0.5);
    let r_tl = radii[0].clamp(0.0, max_r);
    let r_tr = radii[1].clamp(0.0, max_r);
    let r_br = radii[2].clamp(0.0, max_r);
    let r_bl = radii[3].clamp(0.0, max_r);

    let x = rect.x;
    let y = rect.y;
    let w = rect.w;
    let h = rect.h;

    let mut pb = PathBuilder::new();
    if r_tl <= 0.5 && r_tr <= 0.5 && r_br <= 0.5 && r_bl <= 0.5 {
        pb.push_rect(tiny_skia::Rect::from_xywh(x, y, w, h)?);
        return pb.finish();
    }

    const K: f32 = 0.552_284_8;
    pb.move_to(x + r_tl, y);
    pb.line_to(x + w - r_tr, y);
    if r_tr > 0.5 {
        pb.cubic_to(
            x + w - r_tr + r_tr * K,
            y,
            x + w,
            y + r_tr - r_tr * K,
            x + w,
            y + r_tr,
        );
    }
    pb.line_to(x + w, y + h - r_br);
    if r_br > 0.5 {
        pb.cubic_to(
            x + w,
            y + h - r_br + r_br * K,
            x + w - r_br + r_br * K,
            y + h,
            x + w - r_br,
            y + h,
        );
    }
    pb.line_to(x + r_bl, y + h);
    if r_bl > 0.5 {
        pb.cubic_to(
            x + r_bl - r_bl * K,
            y + h,
            x,
            y + h - r_bl + r_bl * K,
            x,
            y + h - r_bl,
        );
    }
    pb.line_to(x, y + r_tl);
    if r_tl > 0.5 {
        pb.cubic_to(
            x,
            y + r_tl - r_tl * K,
            x + r_tl - r_tl * K,
            y,
            x + r_tl,
            y,
        );
    }
    pb.close();
    pb.finish()
}

fn fill_rounded_rect_solid(
    pixmap: &mut Pixmap,
    rect: LayoutRect,
    radii: [f32; 4],
    color: (u8, u8, u8, u8),
    xform: Transform,
) {
    if let Some(path) = build_rounded_rect_path_corners(rect, radii) {
        let mut paint = Paint::default();
        paint.set_color_rgba8(color.0, color.1, color.2, color.3);
        paint.anti_alias = true;
        pixmap.fill_path(&path, &paint, FillRule::Winding, xform, None);
    }
}

fn fill_rounded_rect_linear_grad(
    pixmap: &mut Pixmap,
    rect: LayoutRect,
    radii: [f32; 4],
    angle_deg: f32,
    start: [f32; 4],
    end: [f32; 4],
    xform: Transform,
) {
    if let Some(path) = build_rounded_rect_path_corners(rect, radii) {
        let rad = (angle_deg - 90.0).to_radians();
        let dx = rad.cos();
        let dy = rad.sin();
        let cx = rect.x + rect.w * 0.5;
        let cy = rect.y + rect.h * 0.5;
        let half = (rect.w.abs() * dx.abs() + rect.h.abs() * dy.abs()) * 0.5;
        let p0 = Point::from_xy(cx - dx * half, cy - dy * half);
        let p1 = Point::from_xy(cx + dx * half, cy + dy * half);
        let stops = vec![
            GradientStop::new(
                0.0,
                SkColor::from_rgba(start[0], start[1], start[2], start[3])
                    .unwrap_or(SkColor::BLACK),
            ),
            GradientStop::new(
                1.0,
                SkColor::from_rgba(end[0], end[1], end[2], end[3]).unwrap_or(SkColor::BLACK),
            ),
        ];
        if let Some(shader) =
            LinearGradient::new(p0, p1, stops, SpreadMode::Pad, Transform::identity())
        {
            let paint = Paint {
                shader,
                anti_alias: true,
                ..Paint::default()
            };
            pixmap.fill_path(&path, &paint, FillRule::Winding, xform, None);
        }
    }
}

fn fill_rounded_rect_radial_grad(
    pixmap: &mut Pixmap,
    rect: LayoutRect,
    radii: [f32; 4],
    center: [f32; 4],
    edge: [f32; 4],
    xform: Transform,
) {
    if let Some(path) = build_rounded_rect_path_corners(rect, radii) {
        let cx = rect.x + rect.w * 0.5;
        let cy = rect.y + rect.h * 0.5;
        let rad = (rect.w.max(rect.h) * 0.6).max(1.0);
        let stops = vec![
            GradientStop::new(
                0.0,
                SkColor::from_rgba(center[0], center[1], center[2], center[3])
                    .unwrap_or(SkColor::BLACK),
            ),
            GradientStop::new(
                1.0,
                SkColor::from_rgba(edge[0], edge[1], edge[2], edge[3]).unwrap_or(SkColor::BLACK),
            ),
        ];
        if let Some(shader) = RadialGradient::new(
            Point::from_xy(cx, cy),
            Point::from_xy(cx, cy),
            rad,
            stops,
            SpreadMode::Pad,
            Transform::identity(),
        ) {
            let paint = Paint {
                shader,
                anti_alias: true,
                ..Paint::default()
            };
            pixmap.fill_path(&path, &paint, FillRule::Winding, xform, None);
        }
    }
}

fn stroke_rounded_rect_corners(
    pixmap: &mut Pixmap,
    rect: LayoutRect,
    radii: [f32; 4],
    color: (u8, u8, u8, u8),
    width: f32,
    xform: Transform,
) {
    let inset = width * 0.5;
    let r = LayoutRect::new(
        rect.x + inset,
        rect.y + inset,
        (rect.w - width).max(0.0),
        (rect.h - width).max(0.0),
    );
    let inner_radii = [
        (radii[0] - inset).max(0.0),
        (radii[1] - inset).max(0.0),
        (radii[2] - inset).max(0.0),
        (radii[3] - inset).max(0.0),
    ];
    if let Some(path) = build_rounded_rect_path_corners(r, inner_radii) {
        let mut paint = Paint::default();
        paint.set_color_rgba8(color.0, color.1, color.2, color.3);
        paint.anti_alias = true;
        let stroke = Stroke {
            width,
            ..Stroke::default()
        };
        pixmap.stroke_path(&path, &paint, &stroke, xform, None);
    }
}

/// Converts a `tiny_skia::Pixmap` (RGBA premultiplied) into `softbuffer` (`0x00RRGGBB`) pixels,
/// scaling cleanly when the physical window buffer (`dst_w x dst_h`) differs from the logical pixmap
/// (e.g. Wayland / Retina HiDPI scaling). Uses precomputed X LUT + row duplication for high speed.
pub fn pixmap_to_softbuffer_u32_scaled(
    pixmap: &Pixmap,
    dst: &mut [u32],
    dst_w: u32,
    dst_h: u32,
) {
    let src_w = pixmap.width();
    let src_h = pixmap.height();
    let src = pixmap.data();
    if src_w == dst_w && src_h == dst_h {
        pixmap_to_softbuffer_u32(pixmap, dst);
        return;
    }
    let sw = src_w.max(1) as usize;
    let sh = src_h.max(1) as usize;
    let dw = dst_w.max(1) as usize;
    let dh = dst_h.max(1) as usize;
    let max_rows = (dst.len() / dw).min(dh);
    if max_rows == 0 {
        return;
    }

    // Precompute source byte offsets along X once (eliminates millions of integer divisions!)
    let mut sx_lut = Vec::with_capacity(dw);
    for dx in 0..dw {
        sx_lut.push(((dx * sw) / dw) * 4);
    }

    let mut prev_sy = usize::MAX;
    let mut prev_dst_row = 0usize;
    for dy in 0..max_rows {
        let sy = (dy * sh) / dh;
        let dst_start = dy * dw;
        if sy == prev_sy {
            // Identical source scanline (common on HiDPI 1.25x..2x) -> fast memcpy!
            dst.copy_within(prev_dst_row..prev_dst_row + dw, dst_start);
            continue;
        }
        let src_row_off = sy * sw * 4;
        let dst_row = &mut dst[dst_start..dst_start + dw];
        for (px_out, &sx_off) in dst_row.iter_mut().zip(sx_lut.iter()) {
            let base = src_row_off + sx_off;
            let r = src[base] as u32;
            let g = src[base + 1] as u32;
            let b = src[base + 2] as u32;
            *px_out = (r << 16) | (g << 8) | b;
        }
        prev_sy = sy;
        prev_dst_row = dst_start;
    }
}

/// Converts a `tiny_skia::Pixmap` (RGBA premultiplied) into `softbuffer` (`0x00RRGGBB`) pixels.
pub fn pixmap_to_softbuffer_u32(pixmap: &Pixmap, dst: &mut [u32]) {
    let src = pixmap.data();
    for (rgba, pixel) in src.chunks_exact(4).zip(dst.iter_mut()) {
        *pixel = ((rgba[0] as u32) << 16) | ((rgba[1] as u32) << 8) | (rgba[2] as u32);
    }
}

/// Computes a deterministic 64-bit FNV-1a hash of the rendered pixmap bytes (for CI snapshot tests).
pub fn pixmap_fnv_hash(pixmap: &Pixmap) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for &byte in pixmap.data() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}
