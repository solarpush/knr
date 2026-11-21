//! High-Quality Anti-Aliased 2D Vector & TrueType/OpenType UI Renderer
//! backed by `tiny-skia` (rounded rects, borders, shadows) and `ab_glyph` (system fonts)
//! with graceful fallback to the embedded 8x8 bitmap font.

use crate::gui::font::get_glyph;
use crate::gui::ui_tree::{compute_ui_layout, HitBox, LayoutNode, LayoutRect, TextMeasurer, UiKind, UiNode};
use ab_glyph::{Font, FontVec, PxScale, ScaleFont};
use std::path::Path;
use tiny_skia::{Color as SkColor, FillRule, Paint, PathBuilder, Pixmap, Stroke, Transform};

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

/// Anti-aliased vector & TrueType/OpenType UI renderer.
pub struct UiRenderer {
    font: Option<FontVec>,
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
        Self { font }
    }

    pub fn has_truetype_font(&self) -> bool {
        self.font.is_some()
    }

    /// Renders `root` into a new `tiny_skia::Pixmap` and returns `(pixmap, hit_boxes)`.
    pub fn render_ui(
        &self,
        root: &UiNode,
        width: u32,
        height: u32,
        hovered_action: Option<&str>,
    ) -> (Pixmap, Vec<HitBox>) {
        let w = width.max(1);
        let h = height.max(1);
        let mut pixmap = Pixmap::new(w, h).expect("create tiny-skia pixmap");

        // Clear with root background or clean dark slate canvas (#0b1120)
        let (br, bg, bb, ba) = parse_hex_color(&root.bg).unwrap_or((11, 17, 32, 255));
        pixmap.fill(SkColor::from_rgba8(br, bg, bb, ba));

        let (layout_root, hit_boxes) = compute_ui_layout(root, w as f32, h as f32, self);
        self.paint_layout_node(&mut pixmap, &layout_root, hovered_action);
        (pixmap, hit_boxes)
    }

    fn paint_layout_node(
        &self,
        pixmap: &mut Pixmap,
        laid: &LayoutNode,
        hovered_action: Option<&str>,
    ) {
        let node = &laid.node;
        let rect = laid.rect;
        let is_hovered = !node.action.is_empty() && hovered_action == Some(node.action.as_str());

        // 1. Subtle drop shadow for Cards and Buttons
        if matches!(node.kind, UiKind::Card | UiKind::Button) && !node.bg.is_empty() {
            let shadow_rect = LayoutRect::new(rect.x, rect.y + 3.0, rect.w, rect.h);
            fill_rounded_rect(pixmap, shadow_rect, node.radius, (0, 0, 0, 72));
        }

        // 2. Background fill (with gentle brighten on button hover)
        if let Some((mut r, mut g, mut b, a)) = parse_hex_color(&node.bg) {
            if is_hovered {
                r = r.saturating_add(24);
                g = g.saturating_add(24);
                b = b.saturating_add(24);
            }
            fill_rounded_rect(pixmap, rect, node.radius, (r, g, b, a));
        }

        // 3. Border stroke
        if let Some((r, g, b, a)) = parse_hex_color(&node.border) {
            stroke_rounded_rect(pixmap, rect, node.radius, (r, g, b, a), if is_hovered { 2.0 } else { 1.0 });
        } else if is_hovered && matches!(node.kind, UiKind::Button) {
            stroke_rounded_rect(pixmap, rect, node.radius, (255, 255, 255, 96), 1.5);
        }

        // 4. Text rendering
        if !node.text.is_empty() {
            let color = parse_hex_color(&node.color).unwrap_or((248, 250, 252, 255));
            let (tw, th) = self.measure_text_size(&node.text, node.font_size);
            let (tx, ty) = match node.kind {
                UiKind::Button | UiKind::Badge => (
                    rect.x + ((rect.w - tw) * 0.5).max(0.0),
                    rect.y + ((rect.h - th) * 0.5).max(0.0),
                ),
                _ => (rect.x + node.pad, rect.y + node.pad),
            };
            self.draw_text(pixmap, &node.text, tx, ty, node.font_size, color);
        }

        // 5. Children
        for child in &laid.children {
            self.paint_layout_node(pixmap, child, hovered_action);
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
            let scale = PxScale::from(font_size.max(8.0));
            let scaled = font.as_scaled(scale);
            let ascent = scaled.ascent();
            let mut caret_x = x;
            let baseline_y = y + ascent;
            let mut prev_glyph = None;

            let pw = pixmap.width() as i32;
            let ph = pixmap.height() as i32;
            let data = pixmap.data_mut();

            for ch in text.chars() {
                let glyph_id = scaled.glyph_id(ch);
                if let Some(prev) = prev_glyph {
                    caret_x += scaled.kern(prev, glyph_id);
                }
                let glyph = glyph_id.with_scale_and_position(scale, ab_glyph::point(caret_x, baseline_y));
                if let Some(outlined) = font.outline_glyph(glyph) {
                    let bounds = outlined.px_bounds();
                    outlined.draw(|gx, gy, cov| {
                        let px = bounds.min.x as i32 + gx as i32;
                        let py = bounds.min.y as i32 + gy as i32;
                        if px >= 0 && px < pw && py >= 0 && py < ph {
                            let alpha = (cov * (color.3 as f32 / 255.0)).clamp(0.0, 1.0);
                            if alpha > 0.004 {
                                let idx = ((py as usize) * (pw as usize) + (px as usize)) * 4;
                                blend_premul_rgba(&mut data[idx..idx + 4], color.0, color.1, color.2, alpha);
                            }
                        }
                    });
                }
                caret_x += scaled.h_advance(glyph_id);
                prev_glyph = Some(glyph_id);
            }
        } else {
            // Deterministic 8x8 bitmap fallback
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
                                        let idx = ((py as usize) * (pw as usize) + (px as usize)) * 4;
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

fn build_rounded_rect_path(rect: LayoutRect, radius: f32) -> Option<tiny_skia::Path> {
    if rect.w <= 0.0 || rect.h <= 0.0 {
        return None;
    }
    let r = radius.min(rect.w * 0.5).min(rect.h * 0.5).max(0.0);
    let x = rect.x;
    let y = rect.y;
    let w = rect.w;
    let h = rect.h;

    let mut pb = PathBuilder::new();
    if r <= 0.5 {
        pb.push_rect(tiny_skia::Rect::from_xywh(x, y, w, h)?);
        return pb.finish();
    }

    // Standard cubic Bezier approximation for quarter-circle (k = 0.55228475)
    let k = r * 0.55228475;
    pb.move_to(x + r, y);
    pb.line_to(x + w - r, y);
    pb.cubic_to(x + w - r + k, y, x + w, y + r - k, x + w, y + r);
    pb.line_to(x + w, y + h - r);
    pb.cubic_to(x + w, y + h - r + k, x + w - r + k, y + h, x + w - r, y + h);
    pb.line_to(x + r, y + h);
    pb.cubic_to(x + r - k, y + h, x, y + h - r + k, x, y + h - r);
    pb.line_to(x, y + r);
    pb.cubic_to(x, y + r - k, x + r - k, y, x + r, y);
    pb.close();
    pb.finish()
}

fn fill_rounded_rect(pixmap: &mut Pixmap, rect: LayoutRect, radius: f32, color: (u8, u8, u8, u8)) {
    if let Some(path) = build_rounded_rect_path(rect, radius) {
        let mut paint = Paint::default();
        paint.set_color_rgba8(color.0, color.1, color.2, color.3);
        paint.anti_alias = true;
        pixmap.fill_path(&path, &paint, FillRule::Winding, Transform::identity(), None);
    }
}

fn stroke_rounded_rect(
    pixmap: &mut Pixmap,
    rect: LayoutRect,
    radius: f32,
    color: (u8, u8, u8, u8),
    width: f32,
) {
    let inset = width * 0.5;
    let r = LayoutRect::new(
        rect.x + inset,
        rect.y + inset,
        (rect.w - width).max(0.0),
        (rect.h - width).max(0.0),
    );
    if let Some(path) = build_rounded_rect_path(r, (radius - inset).max(0.0)) {
        let mut paint = Paint::default();
        paint.set_color_rgba8(color.0, color.1, color.2, color.3);
        paint.anti_alias = true;
        let stroke = Stroke {
            width,
            ..Stroke::default()
        };
        pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
    }
}

/// Converts a `tiny_skia::Pixmap` (RGBA premultiplied) into `softbuffer` (`0x00RRGGBB`) pixels.
pub fn pixmap_to_softbuffer_u32(pixmap: &Pixmap, dst: &mut [u32]) {
    let src = pixmap.data();
    let count = dst.len().min(src.len() / 4);
    for i in 0..count {
        let base = i * 4;
        let r = src[base] as u32;
        let g = src[base + 1] as u32;
        let b = src[base + 2] as u32;
        dst[i] = (r << 16) | (g << 8) | b;
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
