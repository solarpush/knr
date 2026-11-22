//! High-Quality Anti-Aliased 2D Vector & TrueType/OpenType UI Renderer (CPU Fallback & Headless Engine)
//! backed by `tiny-skia` (4-corner rounded rects, linear/radial gradients, soft shadows, specular
//! depth highlights, 31 crisp vector icons, syntax-highlighted `<Code>`, transforms, interactive
//! widgets, bilinear rounded-rect-clipped shaders, images) and `ab_glyph` (4 system TrueType/OpenType
//! font faces: sans regular, sans bold, display bold, mono) with deterministic 8x8 bitmap fallback.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use ab_glyph::{Font, FontVec, PxScale, ScaleFont};
use tiny_skia::{
    Color as SkColor, FillRule, FilterQuality, GradientStop, IntSize, LineCap, LineJoin,
    LinearGradient, Mask, Paint, PathBuilder, Pixmap, PixmapPaint, Point, RadialGradient,
    SpreadMode, Stroke, Transform,
};

use crate::gui::font::get_glyph;
use crate::gui::ui_tree::{
    apply_scroll_offsets, children_need_z_sort, compute_ui_layout, compute_ui_layout_with_scroll,
    layout_needs_scroll_offsets, parse_fill_spec, sorted_child_indices_by_z, HitBox, LayoutNode,
    LayoutRect, ParsedFill, TextMeasurer, UiKind, UiNode,
};

pub const FONT_ID_SANS_REGULAR: u8 = 0;
pub const FONT_ID_SANS_BOLD: u8 = 1;
pub const FONT_ID_DISPLAY_BOLD: u8 = 2;
pub const FONT_ID_MONO: u8 = 3;

const SANS_REGULAR_PATHS: &[&str] = &[
    // Linux (Fedora / Ubuntu / Arch / Debian)
    "/usr/share/fonts/rsms-inter-fonts/Inter-Regular.ttf",
    "/usr/share/fonts/rsms-inter-fonts/Inter-Medium.ttf",
    "/usr/share/fonts/adwaita-sans-fonts/AdwaitaSans-Regular.ttf",
    "/usr/share/fonts/google-noto/NotoSans-Regular.ttf",
    "/usr/share/fonts/google-noto-vf/NotoSans[wght].ttf",
    "/usr/share/fonts/abattis-cantarell-fonts/Cantarell-Regular.otf",
    "/usr/share/fonts/abattis-cantarell-vf-fonts/Cantarell-VF.otf",
    "/usr/share/fonts/open-sans/OpenSans-Regular.ttf",
    "/usr/share/fonts/liberation-sans-fonts/LiberationSans-Regular.ttf",
    "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
    "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf",
    "/usr/share/fonts/google-carlito-fonts/Carlito-Regular.ttf",
    "/usr/share/fonts/google-droid-sans-fonts/DroidSans.ttf",
    "/usr/share/fonts/aajohan-comfortaa-fonts/Comfortaa-Regular.otf",
    // macOS
    "/System/Library/Fonts/SFNS.ttf",
    "/System/Library/Fonts/Helvetica.ttc",
    // Windows
    "C:\\Windows\\Fonts\\segoeui.ttf",
    "C:\\Windows\\Fonts\\arial.ttf",
];

const SANS_BOLD_PATHS: &[&str] = &[
    // Linux (Fedora / Ubuntu / Arch / Debian)
    "/usr/share/fonts/rsms-inter-fonts/Inter-SemiBold.ttf",
    "/usr/share/fonts/rsms-inter-fonts/Inter-Bold.ttf",
    "/usr/share/fonts/abattis-cantarell-fonts/Cantarell-Bold.otf",
    "/usr/share/fonts/liberation-sans-fonts/LiberationSans-Bold.ttf",
    "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf",
    "/usr/share/fonts/truetype/liberation/LiberationSans-Bold.ttf",
    "/usr/share/fonts/google-noto/NotoSans-Bold.ttf",
    "/usr/share/fonts/google-carlito-fonts/Carlito-Bold.ttf",
    "/usr/share/fonts/google-droid-sans-fonts/DroidSans-Bold.ttf",
    "/usr/share/fonts/aajohan-comfortaa-fonts/Comfortaa-Bold.otf",
    // macOS
    "/System/Library/Fonts/SFNS.ttf",
    // Windows
    "C:\\Windows\\Fonts\\segoeuib.ttf",
    "C:\\Windows\\Fonts\\arialbd.ttf",
];

const DISPLAY_BOLD_PATHS: &[&str] = &[
    // Linux (Fedora / Ubuntu / Arch / Debian)
    "/usr/share/fonts/rsms-inter-fonts/InterDisplay-Bold.ttf",
    "/usr/share/fonts/rsms-inter-fonts/InterDisplay-SemiBold.ttf",
    "/usr/share/fonts/rsms-inter-fonts/Inter-ExtraBold.ttf",
    "/usr/share/fonts/abattis-cantarell-fonts/Cantarell-ExtraBold.otf",
    "/usr/share/fonts/aajohan-comfortaa-fonts/Comfortaa-Bold.otf",
    // macOS
    "/System/Library/Fonts/SFNS.ttf",
    // Windows
    "C:\\Windows\\Fonts\\segoeuib.ttf",
];

const MONO_PATHS: &[&str] = &[
    // Linux (Fedora / Ubuntu / Arch / Debian)
    "/usr/share/fonts/adwaita-mono-fonts/AdwaitaMono-Regular.ttf",
    "/usr/share/fonts/google-noto/NotoSansMono-Regular.ttf",
    "/usr/share/fonts/google-noto-vf/NotoSansMono[wght].ttf",
    "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf",
    "/usr/share/fonts/liberation-mono-fonts/LiberationMono-Regular.ttf",
    "/usr/share/fonts/truetype/liberation/LiberationMono-Regular.ttf",
    // macOS
    "/System/Library/Fonts/SFNSMono.ttf",
    "/System/Library/Fonts/Menlo.ttc",
    // Windows
    "C:\\Windows\\Fonts\\consola.ttf",
    "C:\\Windows\\Fonts\\cour.ttf",
];

fn load_first_font(paths: &[&str]) -> Option<FontVec> {
    for path_str in paths {
        let p = Path::new(path_str);
        if p.is_file() {
            if let Ok(bytes) = std::fs::read(p) {
                if let Ok(fv) = FontVec::try_from_vec(bytes) {
                    return Some(fv);
                }
            }
        }
    }
    None
}

/// Optional callback invoked when painting `UiKind::Shader` widgets to obtain a rendered pixmap.
pub type ShaderPixmapProvider<'a> =
    &'a dyn Fn(&str, u32, u32, f32, &[f64]) -> Option<tiny_skia::Pixmap>;

/// Interactive render state passed to both GPU and CPU renderers.
#[derive(Debug, Default, Clone)]
pub struct RenderInteractionState<'a> {
    pub hovered_action: Option<&'a str>,
    pub focused_action: Option<&'a str>,
    /// Optional keyboard-focused widget `key` (or `action`) for drawing the crisp 2px rounded
    /// `#38bdf8` accessibility focus ring in both CPU and GPU renderers.
    pub focused_widget_key: Option<String>,
    pub elapsed_sec: f32,
    /// Optional character index of the caret inside the focused `<Input>` (`None` = end of text).
    pub input_caret: Option<usize>,
    /// Optional selected character range `(start, end)` inside the focused `<Input>`.
    pub input_selection: Option<(usize, usize)>,
    /// Optional per-container vertical/horizontal scroll offsets keyed by `<Scroll>` container id.
    pub scroll_offsets: Option<&'a HashMap<String, f32>>,
    /// Optional identifier of the currently actively scrolling container (highlights thumb `#38bdf888`).
    pub active_scroll_id: Option<&'a str>,
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

/// Pure-Rust Base64 decoder supporting standard (`+/`) and URL-safe (`-_`) alphabets.
fn decode_base64_bytes(input: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity((input.len() * 3) / 4);
    let mut buf: u32 = 0;
    let mut bits: u8 = 0;
    let mut count: usize = 0;
    let mut saw_pad = false;
    for b in input.bytes() {
        if b.is_ascii_whitespace() {
            continue;
        }
        if b == b'=' {
            saw_pad = true;
            continue;
        }
        if saw_pad {
            return None;
        }
        let val: u32 = match b {
            b'A'..=b'Z' => (b - b'A') as u32,
            b'a'..=b'z' => (b - b'a' + 26) as u32,
            b'0'..=b'9' => (b - b'0' + 52) as u32,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            _ => return None,
        };
        buf = (buf << 6) | val;
        bits += 6;
        count += 1;
        if bits >= 8 {
            bits -= 8;
            out.push(((buf >> bits) & 0xFF) as u8);
            buf &= (1u32 << bits) - 1;
        }
    }
    if count % 4 == 1 {
        return None;
    }
    Some(out)
}

/// Decodes a local image path (PNG/JPEG/WebP) or `data:image/...;base64,...` URI into
/// straight (non-premultiplied) RGBA8 pixel data `(width, height, rgba_bytes)`.
pub(crate) fn decode_image_rgba(src: &str) -> Option<(u32, u32, Vec<u8>)> {
    let trimmed = src.trim();
    if trimmed.is_empty() {
        return None;
    }
    let dyn_img = if let Some(rest) = trimmed.strip_prefix("data:") {
        let (meta, data_part) = rest.split_once(',')?;
        let meta_lower = meta.trim().to_ascii_lowercase();
        if !meta_lower.starts_with("image/") || !meta_lower.ends_with(";base64") {
            return None;
        }
        let raw = decode_base64_bytes(data_part)?;
        if raw.is_empty() {
            return None;
        }
        image::load_from_memory(&raw).ok()?
    } else {
        image::open(trimmed).ok()?
    };
    let rgba = dyn_img.to_rgba8();
    let (w, h) = rgba.dimensions();
    if w == 0 || h == 0 {
        return None;
    }
    Some((w, h, rgba.into_raw()))
}

/// Returns true if `src` is a non-empty local PNG/JPEG/WebP path or `data:image/...;base64,...`
/// URI that decodes cleanly.
pub(crate) fn can_decode_image_src(src: &str) -> bool {
    decode_image_rgba(src).is_some()
}

/// Anti-aliased vector & TrueType/OpenType UI renderer (CPU + Headless engine).
pub struct UiRenderer {
    sans_regular: Option<FontVec>,
    sans_bold: Option<FontVec>,
    display_bold: Option<FontVec>,
    mono: Option<FontVec>,
    image_cache: RefCell<HashMap<String, Option<Arc<Pixmap>>>>,
    glyph_cache: RefCell<HashMap<(u8, char, u32), Option<CachedGlyph>>>,
}

impl Default for UiRenderer {
    fn default() -> Self {
        Self::new()
    }
}

impl UiRenderer {
    pub fn new() -> Self {
        let sans_regular = load_first_font(SANS_REGULAR_PATHS);
        let sans_bold = load_first_font(SANS_BOLD_PATHS);
        let display_bold = load_first_font(DISPLAY_BOLD_PATHS);
        let mono = load_first_font(MONO_PATHS);
        Self {
            sans_regular,
            sans_bold,
            display_bold,
            mono,
            image_cache: RefCell::new(HashMap::new()),
            glyph_cache: RefCell::new(HashMap::new()),
        }
    }

    pub fn has_truetype_font(&self) -> bool {
        self.sans_regular.is_some()
            || self.sans_bold.is_some()
            || self.display_bold.is_some()
            || self.mono.is_some()
    }

    pub fn font(&self) -> Option<&FontVec> {
        self.sans_regular.as_ref()
    }

    /// Selects the best loaded font face `(font_id, Option<&FontVec>)` for `(font_family, font_weight)`.
    pub fn font_for(&self, font_family: &str, font_weight: i64) -> (u8, Option<&FontVec>) {
        if font_family.eq_ignore_ascii_case("mono") {
            if let Some(ref f) = self.mono {
                return (FONT_ID_MONO, Some(f));
            }
            if let Some(ref f) = self.sans_regular {
                return (FONT_ID_SANS_REGULAR, Some(f));
            }
            return (FONT_ID_MONO, None);
        }
        if font_family.eq_ignore_ascii_case("display") {
            if let Some(ref f) = self.display_bold {
                return (FONT_ID_DISPLAY_BOLD, Some(f));
            }
            if let Some(ref f) = self.sans_bold {
                return (FONT_ID_SANS_BOLD, Some(f));
            }
            if let Some(ref f) = self.sans_regular {
                return (FONT_ID_SANS_REGULAR, Some(f));
            }
            return (FONT_ID_DISPLAY_BOLD, None);
        }
        if font_weight >= 600 {
            if let Some(ref f) = self.sans_bold {
                return (FONT_ID_SANS_BOLD, Some(f));
            }
        }
        if let Some(ref f) = self.sans_regular {
            return (FONT_ID_SANS_REGULAR, Some(f));
        }
        if let Some(ref f) = self.sans_bold {
            return (FONT_ID_SANS_BOLD, Some(f));
        }
        (FONT_ID_SANS_REGULAR, None)
    }

    /// Returns true if `ch` can be rendered by the selected font face (or printable ASCII fallback).
    pub fn has_glyph(&self, ch: char, font_family: &str, font_weight: i64) -> bool {
        if ch.is_whitespace() || ch.is_ascii_control() {
            return true;
        }
        let (_, font_opt) = self.font_for(font_family, font_weight);
        if let Some(font) = font_opt {
            font.glyph_id(ch).0 != 0
        } else {
            let code = ch as u32;
            (32..=126).contains(&code)
        }
    }

    /// Returns true if `src` (a local PNG/JPEG/WebP path or `data:image/...;base64,...` URI)
    /// decodes cleanly into an image. Results are cached inside `UiRenderer`.
    pub fn can_load_image(&self, src: &str) -> bool {
        self.get_or_decode_image(src).is_some()
    }

    fn get_or_decode_image(&self, src: &str) -> Option<Arc<Pixmap>> {
        let trimmed = src.trim();
        if trimmed.is_empty() {
            return None;
        }
        if let Some(cached) = self.image_cache.borrow().get(trimmed) {
            return cached.clone();
        }
        let decoded = decode_image_rgba(trimmed).and_then(|(iw, ih, raw)| {
            let mut pm = Pixmap::new(iw.max(1), ih.max(1))?;
            let dst = pm.data_mut();
            for (src_px, dst_px) in raw
                .as_chunks::<4>()
                .0
                .iter()
                .zip(dst.as_chunks_mut::<4>().0.iter_mut())
            {
                let a = src_px[3] as f32 / 255.0;
                dst_px[0] = (src_px[0] as f32 * a).round() as u8;
                dst_px[1] = (src_px[1] as f32 * a).round() as u8;
                dst_px[2] = (src_px[2] as f32 * a).round() as u8;
                dst_px[3] = src_px[3];
            }
            Some(Arc::new(pm))
        });
        self.image_cache
            .borrow_mut()
            .insert(trimmed.to_string(), decoded.clone());
        decoded
    }

    /// Measures exact horizontal advance (in logical pixels) of the first `char_count` characters
    /// of `text`, matching `draw_text_styled` glyph positioning.
    pub fn measure_text_prefix_x(
        &self,
        text: &str,
        char_count: usize,
        font_size: f32,
        font_family: &str,
        font_weight: i64,
        letter_spacing: f32,
    ) -> f32 {
        if char_count == 0 || text.is_empty() {
            return 0.0;
        }
        let (_, font_opt) = self.font_for(font_family, font_weight);
        if let Some(font) = font_opt {
            let scale = PxScale::from(font_size.max(8.0));
            let scaled = font.as_scaled(scale);
            let mut x = 0.0f32;
            let mut prev = None;
            for ch in text.chars().take(char_count) {
                let id = scaled.glyph_id(ch);
                if let Some(p) = prev {
                    x += scaled.kern(p, id);
                }
                x += scaled.h_advance(id) + letter_spacing;
                prev = Some(id);
            }
            x.max(0.0)
        } else {
            let s = (font_size / 8.0).round().max(1.0);
            let count = text.chars().take(char_count).count();
            ((count as f32) * (8.0 * s + letter_spacing)).max(0.0)
        }
    }

    /// Computes the character index closest to `local_x` inside `text` (for mouse click caret placement).
    pub fn hit_test_text_char_idx(
        &self,
        text: &str,
        local_x: f32,
        font_size: f32,
        font_family: &str,
        font_weight: i64,
        letter_spacing: f32,
    ) -> usize {
        if local_x <= 0.0 || text.is_empty() {
            return 0;
        }
        let (_, font_opt) = self.font_for(font_family, font_weight);
        if let Some(font) = font_opt {
            let scale = PxScale::from(font_size.max(8.0));
            let scaled = font.as_scaled(scale);
            let mut x = 0.0f32;
            let mut prev = None;
            for (idx, ch) in text.chars().enumerate() {
                let id = scaled.glyph_id(ch);
                let kern = prev.map(|p| scaled.kern(p, id)).unwrap_or(0.0);
                let adv = scaled.h_advance(id) + letter_spacing;
                let mid = x + kern + adv * 0.5;
                if local_x < mid {
                    return idx;
                }
                x += kern + adv;
                prev = Some(id);
            }
            text.chars().count()
        } else {
            let s = (font_size / 8.0).round().max(1.0);
            let step = (8.0 * s + letter_spacing).max(1.0);
            ((local_x / step).round() as usize).min(text.chars().count())
        }
    }

    /// Computes the visible text slice `(visible_text, start_char_idx)` for an `<Input>` field
    /// so the caret at `caret_idx` always remains visible inside `max_w`.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn compute_visible_input_window(
        &self,
        full_text: &str,
        fallback_line: &str,
        caret_idx: Option<usize>,
        font_size: f32,
        font_family: &str,
        font_weight: i64,
        letter_spacing: f32,
        max_w: f32,
    ) -> (String, usize) {
        let chars: Vec<char> = full_text.chars().collect();
        let total = chars.len();
        if total == 0 {
            return (String::new(), 0);
        }
        let full_w = self.measure_text_prefix_x(
            full_text,
            total,
            font_size,
            font_family,
            font_weight,
            letter_spacing,
        );
        let limit = max_w.max(20.0);
        if full_w <= limit {
            return (full_text.to_string(), 0);
        }
        let Some(raw_caret) = caret_idx else {
            let vis_len = fallback_line.chars().count();
            return (fallback_line.to_string(), total.saturating_sub(vis_len));
        };
        let caret = raw_caret.min(total);
        // Find the largest prefix ending at `end >= caret` that fits in `limit`
        let mut start = caret;
        let mut end = caret;
        while start > 0 {
            let cand: String = chars[start - 1..end].iter().collect();
            let w = self.measure_text_prefix_x(
                &cand,
                end - (start - 1),
                font_size,
                font_family,
                font_weight,
                letter_spacing,
            );
            if w <= limit {
                start -= 1;
            } else {
                break;
            }
        }
        while end < total {
            let cand: String = chars[start..end + 1].iter().collect();
            let w = self.measure_text_prefix_x(
                &cand,
                (end + 1) - start,
                font_size,
                font_family,
                font_weight,
                letter_spacing,
            );
            if w <= limit {
                end += 1;
            } else {
                break;
            }
        }
        (chars[start..end].iter().collect(), start)
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
                ..Default::default()
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
        let (pixmap, _layout, hit_boxes) =
            self.render_ui_with_shaders(root, width, height, 1.0, state, None);
        (pixmap, hit_boxes)
    }

    /// Renders `root` at logical `(width, height)` into a crisp HiDPI `Pixmap` of dimensions
    /// `(width * scale, height * scale)` (clamped to `1.0..=3.0`), invoking `shader_provider`
    /// for `UiKind::Shader` widgets and returning `(pixmap, layout_root, hit_boxes)`.
    pub fn render_ui_with_shaders(
        &self,
        root: &UiNode,
        width: u32,
        height: u32,
        scale: f32,
        state: &RenderInteractionState<'_>,
        shader_provider: Option<ShaderPixmapProvider<'_>>,
    ) -> (tiny_skia::Pixmap, LayoutNode, Vec<HitBox>) {
        let w = width.max(1);
        let h = height.max(1);
        let sc = scale.clamp(1.0, 3.0);
        let (layout_root, hit_boxes) = if let Some(offsets) = state.scroll_offsets {
            compute_ui_layout_with_scroll(root, w as f32, h as f32, offsets, self)
        } else {
            compute_ui_layout(root, w as f32, h as f32, self)
        };
        let phys_w = ((w as f32 * sc).round() as u32).max(1);
        let phys_h = ((h as f32 * sc).round() as u32).max(1);
        let mut pixmap = Pixmap::new(phys_w, phys_h).expect("create tiny-skia pixmap");
        let (br, bg, bb, ba) = parse_hex_color(&layout_root.node.bg).unwrap_or((11, 17, 32, 255));
        pixmap.fill(SkColor::from_rgba8(br, bg, bb, ba));
        self.paint_layout_node(
            &mut pixmap,
            &layout_root,
            state,
            1.0,
            Transform::from_scale(sc, sc),
            sc,
            false,
            shader_provider,
            None,
            None,
        );
        (pixmap, layout_root, hit_boxes)
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
        let scrolled_owned;
        let active_root = if let Some(offsets) = state.scroll_offsets {
            if !offsets.is_empty() && layout_needs_scroll_offsets(layout_root, offsets) {
                let mut cloned = layout_root.clone();
                apply_scroll_offsets(&mut cloned, offsets);
                scrolled_owned = cloned;
                &scrolled_owned
            } else {
                layout_root
            }
        } else {
            layout_root
        };
        let (br, bg, bb, ba) = parse_hex_color(&active_root.node.bg).unwrap_or((11, 17, 32, 255));
        pixmap.fill(SkColor::from_rgba8(br, bg, bb, ba));
        self.paint_layout_node(
            pixmap,
            active_root,
            state,
            1.0,
            Transform::identity(),
            1.0,
            input_selected_all,
            None,
            None,
            None,
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
            None,
        );
        let thumb_rect = LayoutRect::new(track_x, thumb_y, 5.0, thumb_h);
        fill_rounded_rect_solid(
            pixmap,
            thumb_rect,
            [2.5; 4],
            (148, 163, 184, 133),
            Transform::identity(),
            None,
        );
        stroke_rounded_rect_corners(
            pixmap,
            thumb_rect,
            [2.5; 4],
            (56, 189, 248, 64),
            0.75,
            Transform::identity(),
            None,
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
        self.paint_shaders_only_node(
            pixmap,
            layout_root,
            elapsed_sec,
            1.0,
            Transform::identity(),
            None,
        );
    }

    fn paint_shaders_only_node(
        &self,
        pixmap: &mut Pixmap,
        laid: &LayoutNode,
        elapsed_sec: f32,
        parent_opacity: f32,
        parent_xform: Transform,
        clip_mask: Option<&Mask>,
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
        let local_xform = compute_node_transform(rect, &node.style);
        let xform = parent_xform.pre_concat(local_xform);
        let radii = node.effective_radii();
        if node.kind == UiKind::Shader {
            paint_cpu_shader_preview(
                pixmap,
                rect,
                radii,
                &node.text,
                elapsed_sec,
                &node.uniforms,
                opacity,
                clip_mask,
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
                    xform,
                    clip_mask,
                );
            }
        }
        let owned_clip = if node.clips_children() {
            push_clip_mask(pixmap.width(), pixmap.height(), clip_mask, rect, radii, xform)
        } else {
            None
        };
        let child_clip = owned_clip.as_ref().or(clip_mask);
        if children_need_z_sort(&laid.children) {
            for idx in sorted_child_indices_by_z(&laid.children) {
                self.paint_shaders_only_node(
                    pixmap,
                    &laid.children[idx],
                    elapsed_sec,
                    opacity,
                    xform,
                    child_clip,
                );
            }
        } else {
            for child in &laid.children {
                self.paint_shaders_only_node(pixmap, child, elapsed_sec, opacity, xform, child_clip);
            }
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
        render_scale: f32,
        input_selected_all: bool,
        shader_provider: Option<ShaderPixmapProvider<'_>>,
        clip_mask: Option<&Mask>,
        clip_bounds: Option<LayoutRect>,
    ) {
        let node = &laid.node;
        let rect = laid.rect;
        let opacity = (parent_opacity * node.style.opacity).clamp(0.0, 1.0);
        if opacity <= 0.004 || rect.w <= 0.0 || rect.h <= 0.0 {
            return;
        }
        // Cull completely off-screen or outside-clip-bounds subtrees
        let logical_h = pixmap.height() as f32 / render_scale.max(0.5);
        if rect.y + rect.h < -64.0 || rect.y > logical_h + 64.0 {
            return;
        }
        if let Some(cb) = clip_bounds {
            let margin = node.style.shadow_blur.max(8.0);
            if rect.x + rect.w + margin < cb.x
                || rect.x - margin > cb.x + cb.w
                || rect.y + rect.h + margin < cb.y
                || rect.y - margin > cb.y + cb.h
            {
                return;
            }
        }

        let is_hovered =
            !node.action.is_empty() && state.hovered_action == Some(node.action.as_str());
        let is_key_focused = state.focused_widget_key.as_deref().is_some_and(|fk| {
            !fk.is_empty()
                && ((!node.key.is_empty() && node.key == fk)
                    || (!node.action.is_empty() && node.action == fk))
        });
        let is_focused =
            (!node.action.is_empty() && state.focused_action == Some(node.action.as_str()))
                || is_key_focused;

        let local_xform = compute_node_transform(rect, &node.style);
        let xform = parent_xform.pre_concat(local_xform);
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
                    fill_rounded_rect_solid(
                        pixmap,
                        srect,
                        sradii,
                        (sr, sg, sb, step_a),
                        xform,
                        clip_mask,
                    );
                }
            }
        } else if matches!(node.kind, UiKind::Card | UiKind::Button) && !node.bg.is_empty() {
            let srect = LayoutRect::new(rect.x, rect.y + 3.0, rect.w, rect.h);
            let sa = (72.0 * opacity).round().clamp(0.0, 255.0) as u8;
            fill_rounded_rect_solid(pixmap, srect, radii, (0, 0, 0, sa), xform, clip_mask);
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
                fill_rounded_rect_solid(pixmap, rect, radii, f32_to_rgba8(rgba), xform, clip_mask);
            }
            ParsedFill::LinearGradient {
                angle_deg,
                mut start,
                mut end,
            } => {
                start[3] *= opacity;
                end[3] *= opacity;
                fill_rounded_rect_linear_grad(
                    pixmap, rect, radii, angle_deg, start, end, xform, clip_mask,
                );
            }
            ParsedFill::RadialGradient {
                mut center,
                mut edge,
            } => {
                center[3] *= opacity;
                edge[3] *= opacity;
                fill_rounded_rect_radial_grad(
                    pixmap, rect, radii, center, edge, xform, clip_mask,
                );
            }
        }

        // 3. Specialized interactive widget surfaces
        match node.kind {
            UiKind::Shader => {
                let req_w = (rect.w * render_scale).round().max(1.0) as u32;
                let req_h = (rect.h * render_scale).round().max(1.0) as u32;
                let provided = shader_provider
                    .and_then(|p| p(&node.text, req_w, req_h, state.elapsed_sec, &node.uniforms));
                if let Some(shader_pm) = provided {
                    blit_pixmap_rounded(
                        pixmap, &shader_pm, rect, radii, opacity, xform, clip_mask,
                    );
                } else {
                    let phys_rect = LayoutRect::new(
                        rect.x * render_scale,
                        rect.y * render_scale,
                        rect.w * render_scale,
                        rect.h * render_scale,
                    );
                    let phys_radii = [
                        radii[0] * render_scale,
                        radii[1] * render_scale,
                        radii[2] * render_scale,
                        radii[3] * render_scale,
                    ];
                    paint_cpu_shader_preview(
                        pixmap,
                        phys_rect,
                        phys_radii,
                        &node.text,
                        state.elapsed_sec,
                        &node.uniforms,
                        opacity,
                        clip_mask,
                    );
                }
            }
            UiKind::Image => {
                self.paint_image_widget(
                    pixmap, rect, radii, &node.text, opacity, xform, clip_mask,
                );
            }
            UiKind::Icon => {
                let (ir, ig, ib, ia) = parse_hex_color(&node.color).unwrap_or((148, 163, 184, 255));
                let col = (
                    ir,
                    ig,
                    ib,
                    (ia as f32 * opacity).round().clamp(0.0, 255.0) as u8,
                );
                let [pt, pr, pb, pl] = node.effective_pad();
                let inner_w = (rect.w - pl - pr).max(1.0);
                let inner_h = (rect.h - pt - pb).max(1.0);
                let sz = inner_w.min(inner_h);
                let icon_rect = LayoutRect::new(
                    rect.x + pl + (inner_w - sz) * 0.5,
                    rect.y + pt + (inner_h - sz) * 0.5,
                    sz,
                    sz,
                );
                paint_vector_icon_clipped(pixmap, icon_rect, &node.text, col, xform, clip_mask);
            }
            UiKind::Code => {
                if node.bg.is_empty() {
                    fill_rounded_rect_solid(
                        pixmap,
                        rect,
                        radii,
                        (7, 11, 20, (245.0 * opacity).round().clamp(0.0, 255.0) as u8),
                        xform,
                        clip_mask,
                    );
                }
                self.paint_code_block(pixmap, laid, opacity, render_scale, clip_mask);
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
                    clip_mask,
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
                    clip_mask,
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
                    clip_mask,
                );
                stroke_rounded_rect_corners(
                    pixmap,
                    thumb,
                    [thumb_r; 4],
                    (56, 189, 248, (255.0 * opacity) as u8),
                    2.0,
                    xform,
                    clip_mask,
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
                fill_rounded_rect_solid(pixmap, pill, [12.0; 4], pill_col, xform, clip_mask);
                let knob_x = if checked { pill_x + pill_w - 20.0 } else { pill_x + 4.0 };
                let knob = LayoutRect::new(knob_x, pill_y + 4.0, 16.0, 16.0);
                fill_rounded_rect_solid(
                    pixmap,
                    knob,
                    [8.0; 4],
                    (248, 250, 252, (255.0 * opacity) as u8),
                    xform,
                    clip_mask,
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
                    fill_rounded_rect_solid(pixmap, rect, radii, (r, g, b, aa), xform, clip_mask);
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
                        clip_mask,
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
                        clip_mask,
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
                    clip_mask,
                );
            }
            _ => {}
        }

        // 4. Border stroke + subtle 1px top inner specular highlight on bordered Cards
        let has_input_sel = is_focused
            && node.kind == UiKind::Input
            && (input_selected_all
                || state
                    .input_selection
                    .is_some_and(|(a, b)| a != b));
        if node.kind != UiKind::Divider {
            let active_border = if is_focused {
                if has_input_sel {
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
                    if has_input_sel {
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
                stroke_rounded_rect_corners(
                    pixmap,
                    rect,
                    radii,
                    (r, g, b, aa),
                    bw,
                    xform,
                    clip_mask,
                );

                // Subtle 1px top inner specular highlight (`rgba(255,255,255,0.08)`) on bordered Cards
                if node.kind == UiKind::Card && rect.w > 4.0 && rect.h > 4.0 {
                    let left_in = radii[0].min(rect.w * 0.4).max(1.0);
                    let right_in = radii[1].min(rect.w * 0.4).max(1.0);
                    if rect.w > left_in + right_in + 1.0 {
                        let mut pb = PathBuilder::new();
                        let hy = rect.y + 1.0;
                        pb.move_to(rect.x + left_in, hy);
                        pb.line_to(rect.x + rect.w - right_in, hy);
                        if let Some(path) = pb.finish() {
                            let mut paint = Paint::default();
                            let ha = (20.4 * opacity).round().clamp(0.0, 255.0) as u8;
                            paint.set_color_rgba8(255, 255, 255, ha);
                            paint.anti_alias = true;
                            let stroke = Stroke {
                                width: 1.0,
                                ..Stroke::default()
                            };
                            pixmap.stroke_path(&path, &paint, &stroke, xform, clip_mask);
                        }
                    }
                }
            } else if is_hovered && matches!(node.kind, UiKind::Button) {
                let aa = (96.0 * opacity).round().clamp(0.0, 255.0) as u8;
                stroke_rounded_rect_corners(
                    pixmap,
                    rect,
                    radii,
                    (255, 255, 255, aa),
                    1.5,
                    xform,
                    clip_mask,
                );
            }
        }

        // 4b. Crisp 2px rounded `#38bdf8` accessibility focus ring when keyboard-focused
        if is_key_focused {
            let ring_rect = LayoutRect::new(
                rect.x - 2.5,
                rect.y - 2.5,
                rect.w + 5.0,
                rect.h + 5.0,
            );
            let ring_radii = [
                if radii[0] > 0.0 { radii[0] + 2.5 } else { 6.0 },
                if radii[1] > 0.0 { radii[1] + 2.5 } else { 6.0 },
                if radii[2] > 0.0 { radii[2] + 2.5 } else { 6.0 },
                if radii[3] > 0.0 { radii[3] + 2.5 } else { 6.0 },
            ];
            let ring_a = (255.0 * opacity).round().clamp(0.0, 255.0) as u8;
            stroke_rounded_rect_corners(
                pixmap,
                ring_rect,
                ring_radii,
                (56, 189, 248, ring_a),
                2.0,
                xform,
                clip_mask,
            );
        }

        // Compute intersected clip mask for this container's interior content & children
        let owned_child_clip = if node.clips_children() {
            push_clip_mask(pixmap.width(), pixmap.height(), clip_mask, rect, radii, xform)
        } else {
            None
        };
        let child_clip_mask = owned_child_clip.as_ref().or(clip_mask);
        let child_clip_bounds = if node.clips_children() {
            Some(match clip_bounds {
                Some(parent_cb) => parent_cb
                    .intersect(rect)
                    .unwrap_or_else(|| LayoutRect::new(rect.x, rect.y, 0.0, 0.0)),
                None => rect,
            })
        } else {
            clip_bounds
        };

        // 5. Text rendering (uses pre-wrapped multi-line layout + 4 font faces + text_align)
        let text_clip = if node.clips_children() {
            child_clip_mask
        } else {
            clip_mask
        };
        if node.kind == UiKind::Input {
            let (tr, tg, tb, ta) = parse_hex_color(&node.color).unwrap_or((248, 250, 252, 255));
            let color = (
                tr,
                tg,
                tb,
                (ta as f32 * opacity).round().clamp(0.0, 255.0) as u8,
            );
            let [_, pr, _, pl] = node.effective_pad();
            let pad_l = pl.max(12.0);
            let pad_r = pr.max(12.0);
            let inner_w = (rect.w - pad_l - pad_r).max(20.0);
            let (_, th) = self.measure_text_styled(
                "Mg",
                node.font_size,
                &node.style.font_family,
                node.style.font_weight,
            );
            let tx_final = rect.x + pad_l + node.style.translate_x;
            let ty_final = rect.y + ((rect.h - th) * 0.5).max(0.0) + node.style.translate_y;
            let fallback_line = laid
                .wrapped_lines
                .first()
                .map(String::as_str)
                .unwrap_or("");
            let full_len = node.text.chars().count();
            let caret_opt = if is_focused {
                Some(state.input_caret.unwrap_or(full_len).min(full_len))
            } else {
                None
            };
            let (vis_text, vis_start) = self.compute_visible_input_window(
                &node.text,
                fallback_line,
                caret_opt,
                node.font_size,
                &node.style.font_family,
                node.style.font_weight,
                node.style.letter_spacing,
                inner_w,
            );
            let vis_len = vis_text.chars().count();

            // Selection highlight behind selected characters (`#38bdf844`)
            if is_focused && full_len > 0 {
                let eff_sel = state
                    .input_selection
                    .map(|(a, b)| if a <= b { (a, b) } else { (b, a) })
                    .filter(|(a, b)| a < b)
                    .or_else(|| input_selected_all.then_some((0, full_len)));
                if let Some((sel_s, sel_e)) = eff_sel {
                    let vs = sel_s.saturating_sub(vis_start).min(vis_len);
                    let ve = sel_e.saturating_sub(vis_start).min(vis_len);
                    if ve > vs {
                        let x0 = tx_final
                            + self.measure_text_prefix_x(
                                &vis_text,
                                vs,
                                node.font_size,
                                &node.style.font_family,
                                node.style.font_weight,
                                node.style.letter_spacing,
                            );
                        let x1 = tx_final
                            + self.measure_text_prefix_x(
                                &vis_text,
                                ve,
                                node.font_size,
                                &node.style.font_family,
                                node.style.font_weight,
                                node.style.letter_spacing,
                            );
                        let max_x = rect.x + rect.w - pad_r.min(8.0);
                        let sx0 = x0.clamp(tx_final - 2.0, max_x);
                        let sx1 = x1.clamp(sx0, max_x);
                        if sx1 > sx0 + 0.5 {
                            let sel_rect = LayoutRect::new(
                                sx0,
                                ty_final - 2.0,
                                sx1 - sx0,
                                th.max(14.0) + 4.0,
                            );
                            let sa = (68.0 * opacity).round().clamp(0.0, 255.0) as u8;
                            fill_rounded_rect_solid(
                                pixmap,
                                sel_rect,
                                [3.0; 4],
                                (56, 189, 248, sa),
                                xform,
                                text_clip,
                            );
                        }
                    }
                }
            }

            if !vis_text.is_empty() {
                let clip_max_x = Some(rect.x + rect.w - pad_r.min(8.0));
                self.draw_text_styled(
                    pixmap,
                    &vis_text,
                    tx_final,
                    ty_final,
                    node.font_size,
                    &node.style.font_family,
                    node.style.font_weight,
                    node.style.letter_spacing,
                    render_scale,
                    color,
                    clip_max_x,
                    text_clip,
                );
            }

            if let Some(caret_idx) = caret_opt {
                let vis_caret = caret_idx.saturating_sub(vis_start).min(vis_len);
                let caret_x_off = self.measure_text_prefix_x(
                    &vis_text,
                    vis_caret,
                    node.font_size,
                    &node.style.font_family,
                    node.style.font_weight,
                    node.style.letter_spacing,
                );
                let max_caret_x = (rect.x + rect.w - pad_r.min(8.0) - 2.0).max(tx_final);
                let cx = (tx_final + caret_x_off).min(max_caret_x);
                let caret_rect = LayoutRect::new(cx, ty_final, 2.0, th.max(14.0));
                let ca = (255.0 * opacity).round().clamp(0.0, 255.0) as u8;
                fill_rounded_rect_solid(
                    pixmap,
                    caret_rect,
                    [1.0; 4],
                    (56, 189, 248, ca),
                    xform,
                    text_clip,
                );
            }
        } else if !laid.wrapped_lines.is_empty()
            && !matches!(
                node.kind,
                UiKind::Shader
                    | UiKind::Image
                    | UiKind::Slider
                    | UiKind::Divider
                    | UiKind::Progress
                    | UiKind::Code
                    | UiKind::Icon
            )
        {
            let (tr, tg, tb, ta) = parse_hex_color(&node.color).unwrap_or((248, 250, 252, 255));
            let color = (tr, tg, tb, (ta as f32 * opacity).round().clamp(0.0, 255.0) as u8);
            let [pt, pr, _pb, pl] = node.effective_pad();
            let first_line = &laid.wrapped_lines[0];
            let (tw, th) = self.measure_text_styled(
                first_line,
                node.font_size,
                &node.style.font_family,
                node.style.font_weight,
            );
            let (base_tx, ty) = match node.kind {
                UiKind::Button | UiKind::Badge => (
                    rect.x + ((rect.w - tw) * 0.5).max(0.0),
                    rect.y + ((rect.h - th) * 0.5).max(0.0),
                ),
                UiKind::Toggle => (
                    rect.x + pl + 52.0,
                    rect.y + ((rect.h - th) * 0.5).max(0.0),
                ),
                _ => (rect.x + pl, rect.y + pt),
            };
            let tx_final = base_tx + node.style.translate_x;
            let ty_final = ty + node.style.translate_y;

            let inner_w = (rect.w - pl - pr).max(0.0);
            for (line_idx, line) in laid.wrapped_lines.iter().enumerate() {
                if !line.is_empty() {
                    let ly = ty_final + (line_idx as f32) * laid.line_height;
                    let lx = if matches!(node.kind, UiKind::Text | UiKind::Col | UiKind::Card | UiKind::Scroll) {
                        match node.style.text_align.as_str() {
                            "center" => {
                                let (lw, _) = self.measure_text_styled(
                                    line,
                                    node.font_size,
                                    &node.style.font_family,
                                    node.style.font_weight,
                                );
                                rect.x + pl + ((inner_w - lw) * 0.5).max(0.0) + node.style.translate_x
                            }
                            "right" => {
                                let (lw, _) = self.measure_text_styled(
                                    line,
                                    node.font_size,
                                    &node.style.font_family,
                                    node.style.font_weight,
                                );
                                rect.x + pl + (inner_w - lw).max(0.0) + node.style.translate_x
                            }
                            _ => tx_final,
                        }
                    } else {
                        tx_final
                    };
                    self.draw_text_styled(
                        pixmap,
                        line,
                        lx,
                        ly,
                        node.font_size,
                        &node.style.font_family,
                        node.style.font_weight,
                        node.style.letter_spacing,
                        render_scale,
                        color,
                        None,
                        text_clip,
                    );
                }
            }
        }

        // 6. Children (painted in ascending z_index order, stable on ties, clipped if container clips!)
        let skip_children = child_clip_bounds.is_some_and(|cb| cb.w <= 0.0 || cb.h <= 0.0);
        if !skip_children {
            if children_need_z_sort(&laid.children) {
                for idx in sorted_child_indices_by_z(&laid.children) {
                    self.paint_layout_node(
                        pixmap,
                        &laid.children[idx],
                        state,
                        opacity,
                        xform,
                        render_scale,
                        input_selected_all,
                        shader_provider,
                        child_clip_mask,
                        child_clip_bounds,
                    );
                }
            } else {
                for child in &laid.children {
                    self.paint_layout_node(
                        pixmap,
                        child,
                        state,
                        opacity,
                        xform,
                        render_scale,
                        input_selected_all,
                        shader_provider,
                        child_clip_mask,
                        child_clip_bounds,
                    );
                }
            }
        }

        // 7. Sleek rounded vertical & horizontal scrollbar thumbs for overflowing `<Scroll>` /
        // `overflow="scroll"|"scroll-x"|"scroll-xy"` containers (`#94a3b855` idle, `#38bdf888` active)
        let has_v_scroll = node.is_scrollable_y()
            && laid.content_height > rect.h + 2.0
            && rect.h > 24.0
            && rect.w > 16.0;
        let has_h_scroll = node.is_scrollable_x()
            && laid.content_width > rect.w + 2.0
            && rect.w > 24.0
            && rect.h > 16.0;
        if has_v_scroll || has_h_scroll {
            let is_scroll_active = is_hovered
                || is_focused
                || state.active_scroll_id.is_some_and(|aid| {
                    !aid.is_empty() && (aid == node.key || aid == node.action)
                });
            let thumb_col = if is_scroll_active {
                (56, 189, 248, (136.0 * opacity).round().clamp(0.0, 255.0) as u8)
            } else {
                (148, 163, 184, (85.0 * opacity).round().clamp(0.0, 255.0) as u8)
            };
            if has_v_scroll {
                let corner_reserve = if has_h_scroll { 8.0 } else { 0.0 };
                let track_h = (rect.h - 8.0 - corner_reserve).max(4.0);
                let max_thumb = (track_h - 4.0).max(4.0);
                let min_thumb = 20.0f32.min(max_thumb);
                let thumb_h = (rect.h / laid.content_height * track_h).clamp(min_thumb, max_thumb);
                let max_scroll = (laid.content_height - rect.h).max(1.0);
                let ratio = (laid.scroll_offset_y / max_scroll).clamp(0.0, 1.0);
                let thumb_y = rect.y + 4.0 + ratio * (track_h - thumb_h).max(0.0);
                let thumb_rect = LayoutRect::new(rect.x + rect.w - 7.0, thumb_y, 5.0, thumb_h);
                fill_rounded_rect_solid(
                    pixmap,
                    thumb_rect,
                    [2.5; 4],
                    thumb_col,
                    xform,
                    child_clip_mask,
                );
            }
            if has_h_scroll {
                let corner_reserve = if has_v_scroll { 8.0 } else { 0.0 };
                let track_w = (rect.w - 8.0 - corner_reserve).max(4.0);
                let max_thumb = (track_w - 4.0).max(4.0);
                let min_thumb = 20.0f32.min(max_thumb);
                let thumb_w = (rect.w / laid.content_width * track_w).clamp(min_thumb, max_thumb);
                let max_scroll = (laid.content_width - rect.w).max(1.0);
                let ratio = (laid.scroll_offset_x / max_scroll).clamp(0.0, 1.0);
                let thumb_x = rect.x + 4.0 + ratio * (track_w - thumb_w).max(0.0);
                let thumb_rect = LayoutRect::new(thumb_x, rect.y + rect.h - 7.0, thumb_w, 5.0);
                fill_rounded_rect_solid(
                    pixmap,
                    thumb_rect,
                    [2.5; 4],
                    thumb_col,
                    xform,
                    child_clip_mask,
                );
            }
        }
    }

    fn paint_code_block(
        &self,
        pixmap: &mut Pixmap,
        laid: &LayoutNode,
        opacity: f32,
        render_scale: f32,
        clip_mask: Option<&Mask>,
    ) {
        let node = &laid.node;
        let rect = laid.rect;
        let [pt, pr, _pb, pl] = node.effective_pad();
        let start_x = rect.x + pl + node.style.translate_x;
        let start_y = rect.y + pt + node.style.translate_y;
        let clip_max_x = Some(rect.x + rect.w - pr);
        let alpha = (255.0 * opacity).round().clamp(0.0, 255.0) as u8;

        for (line_idx, line) in laid.wrapped_lines.iter().enumerate() {
            if line.is_empty() {
                continue;
            }
            let ly = start_y + (line_idx as f32) * laid.line_height;
            let mut cx = start_x;
            for (span, (r, g, b)) in tokenize_code_line(line) {
                cx = self.draw_text_styled(
                    pixmap,
                    span,
                    cx,
                    ly,
                    node.font_size,
                    "mono",
                    node.style.font_weight,
                    node.style.letter_spacing,
                    render_scale,
                    (r, g, b, alpha),
                    clip_max_x,
                    clip_mask,
                );
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn paint_image_widget(
        &self,
        pixmap: &mut Pixmap,
        rect: LayoutRect,
        radii: [f32; 4],
        src: &str,
        opacity: f32,
        xform: Transform,
        clip_mask: Option<&Mask>,
    ) {
        if let Some(src_pm) = self.get_or_decode_image(src) {
            blit_pixmap_cover_rounded(pixmap, &src_pm, rect, radii, opacity, xform, clip_mask);
            return;
        }
        // Subtle broken-image frame fallback when decode fails or src is empty
        let bg_a = (140.0 * opacity).round().clamp(0.0, 255.0) as u8;
        let br_a = (180.0 * opacity).round().clamp(0.0, 255.0) as u8;
        fill_rounded_rect_solid(pixmap, rect, radii, (30, 41, 59, bg_a), xform, clip_mask);
        stroke_rounded_rect_corners(
            pixmap,
            rect,
            radii,
            (71, 85, 105, br_a),
            1.25,
            xform,
            clip_mask,
        );
        let pad = (rect.w.min(rect.h) * 0.28).clamp(4.0, 20.0);
        if rect.w > pad * 2.0 + 2.0 && rect.h > pad * 2.0 + 2.0 {
            let mut pb = PathBuilder::new();
            pb.move_to(rect.x + pad, rect.y + pad);
            pb.line_to(rect.x + rect.w - pad, rect.y + rect.h - pad);
            pb.move_to(rect.x + rect.w - pad, rect.y + pad);
            pb.line_to(rect.x + pad, rect.y + rect.h - pad);
            if let Some(path) = pb.finish() {
                let mut paint = Paint::default();
                paint.set_color_rgba8(100, 116, 139, br_a);
                paint.anti_alias = true;
                let stroke = Stroke {
                    width: 1.5,
                    line_cap: LineCap::Round,
                    ..Stroke::default()
                };
                pixmap.stroke_path(&path, &paint, &stroke, xform, clip_mask);
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_text_styled(
        &self,
        pixmap: &mut Pixmap,
        text: &str,
        x: f32,
        y: f32,
        font_size: f32,
        font_family: &str,
        font_weight: i64,
        letter_spacing: f32,
        render_scale: f32,
        color: (u8, u8, u8, u8),
        clip_max_x: Option<f32>,
        clip_mask: Option<&Mask>,
    ) -> f32 {
        let sc = render_scale.max(0.5);
        let (font_id, font_opt) = self.font_for(font_family, font_weight);
        let clip_phys_x = clip_max_x.map(|cx| (cx * sc).floor() as i32);
        let mask_data = clip_mask.map(Mask::data);

        if let Some(font) = font_opt {
            let scaled_fs = (font_size * sc).clamp(6.0, 288.0);
            let size_tenths = (scaled_fs * 10.0).round() as u32;
            let px_scale = PxScale::from(size_tenths as f32 * 0.1);
            let scaled = font.as_scaled(px_scale);
            let ascent = scaled.ascent();
            let mut caret_x = x * sc;
            let base_phys_y = y * sc;
            let phys_spacing = letter_spacing * sc;
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

                let cached = cache.entry((font_id, ch, size_tenths)).or_insert_with(|| {
                    let glyph = glyph_id
                        .with_scale_and_position(px_scale, ab_glyph::point(0.0, ascent));
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
                    let base_y = base_phys_y.round() as i32 + g.off_y;
                    let max_x = clip_phys_x.map(|cx| cx.min(pw)).unwrap_or(pw);
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
                            if px >= 0 && px < max_x {
                                let px_idx = dst_row + px as usize;
                                let mask_scale = if let Some(md) = mask_data {
                                    let ma = md[px_idx];
                                    if ma == 0 {
                                        continue;
                                    }
                                    ma as f32 / 255.0
                                } else {
                                    1.0
                                };
                                let alpha = (cov_u8 as f32 / 255.0) * col_a * mask_scale;
                                let idx = px_idx * 4;
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
                caret_x += scaled.h_advance(glyph_id) + phys_spacing;
                prev_glyph = Some(glyph_id);
            }
            caret_x / sc
        } else {
            let scaled_fs = font_size * sc;
            let scale_i = (scaled_fs / 8.0).round().max(1.0) as i32;
            let pw = pixmap.width() as i32;
            let ph = pixmap.height() as i32;
            let max_x = clip_phys_x.map(|cx| cx.min(pw)).unwrap_or(pw);
            let data = pixmap.data_mut();
            let mut cx = (x * sc).round() as i32;
            let cy = (y * sc).round() as i32;
            let extra_px = (letter_spacing * sc).round() as i32;
            for ch in text.chars() {
                let glyph = get_glyph(ch);
                for (row, &bits) in glyph.iter().enumerate() {
                    for col in 0..8 {
                        if (bits & (1 << (7 - col))) != 0 {
                            for sy in 0..scale_i {
                                for sx in 0..scale_i {
                                    let px = cx + col * scale_i + sx;
                                    let py = cy + (row as i32) * scale_i + sy;
                                    if px >= 0 && px < max_x && py >= 0 && py < ph {
                                        let px_idx =
                                            (py as usize) * (pw as usize) + (px as usize);
                                        let mask_scale = if let Some(md) = mask_data {
                                            let ma = md[px_idx];
                                            if ma == 0 {
                                                continue;
                                            }
                                            ma as f32 / 255.0
                                        } else {
                                            1.0
                                        };
                                        let idx = px_idx * 4;
                                        blend_premul_rgba(
                                            &mut data[idx..idx + 4],
                                            color.0,
                                            color.1,
                                            color.2,
                                            (color.3 as f32 / 255.0) * mask_scale,
                                        );
                                    }
                                }
                            }
                        }
                    }
                }
                cx += 8 * scale_i + extra_px;
            }
            cx as f32 / sc
        }
    }
}

impl TextMeasurer for UiRenderer {
    fn measure_text_size(&self, text: &str, font_size: f32) -> (f32, f32) {
        self.measure_text_styled(text, font_size, "sans", 400)
    }

    fn measure_text_styled(
        &self,
        text: &str,
        font_size: f32,
        font_family: &str,
        font_weight: i64,
    ) -> (f32, f32) {
        if text.is_empty() {
            return (0.0, font_size.max(8.0) * 1.25);
        }
        let (_, font_opt) = self.font_for(font_family, font_weight);
        if let Some(font) = font_opt {
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

// ============================================================================
// Syntax Tokenizer for `<Code>` Blocks (shared between CPU and GPU engines)
// ============================================================================

const COL_COMMENT: (u8, u8, u8) = (100, 116, 139); // #64748b
const COL_DECORATOR: (u8, u8, u8) = (244, 114, 182); // #f472b6
const COL_STRING: (u8, u8, u8) = (52, 211, 153); // #34d399
const COL_KEYWORD: (u8, u8, u8) = (192, 132, 252); // #c084fc
const COL_TYPE: (u8, u8, u8) = (56, 189, 248); // #38bdf8
const COL_NUMBER: (u8, u8, u8) = (251, 191, 36); // #fbbf24
const COL_PUNCT: (u8, u8, u8) = (148, 163, 184); // #94a3b8
const COL_IDENT: (u8, u8, u8) = (226, 232, 240); // #e2e8f0

fn is_code_keyword(word: &str) -> bool {
    matches!(
        word,
        "import"
            | "from"
            | "export"
            | "type"
            | "interface"
            | "enum"
            | "function"
            | "const"
            | "let"
            | "return"
            | "if"
            | "else"
            | "while"
            | "for"
            | "switch"
            | "case"
            | "default"
            | "break"
            | "continue"
            | "readonly"
            | "true"
            | "false"
            | "as"
    )
}

fn is_code_type(word: &str) -> bool {
    if matches!(
        word,
        "i64"
            | "f64"
            | "number"
            | "string"
            | "bool"
            | "boolean"
            | "void"
            | "Widget"
            | "Style"
            | "State"
            | "Color"
            | "Vec2"
            | "Vec3"
            | "Buffer"
    ) {
        return true;
    }
    word.bytes().next().is_some_and(|b| b.is_ascii_uppercase())
}

/// Tokenizes a single line of `.knr` / TSX / TypeScript / JSON / SSA code into `(slice, rgb)` spans,
/// preserving exact leading indentation and whitespace.
pub fn tokenize_code_line(line: &str) -> Vec<(&str, (u8, u8, u8))> {
    let mut out = Vec::new();
    let bytes = line.as_bytes();
    let len = bytes.len();
    let mut i = 0usize;

    while i < len {
        let b = bytes[i];
        // Whitespace (preserve verbatim)
        if b == b' ' || b == b'\t' {
            let start = i;
            while i < len && (bytes[i] == b' ' || bytes[i] == b'\t') {
                i += 1;
            }
            out.push((&line[start..i], COL_IDENT));
            continue;
        }
        // Line comment `//...`
        if b == b'/' && i + 1 < len && bytes[i + 1] == b'/' {
            out.push((&line[i..], COL_COMMENT));
            break;
        }
        // Decorator `@shader`, `@effects`, etc.
        if b == b'@' && i + 1 < len && (bytes[i + 1].is_ascii_alphabetic() || bytes[i + 1] == b'_') {
            let start = i;
            i += 1;
            while i < len && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                i += 1;
            }
            out.push((&line[start..i], COL_DECORATOR));
            continue;
        }
        // String literal `"..."`, `'...'`, `` `...` ``
        if b == b'"' || b == b'\'' || b == b'`' {
            let quote = b;
            let start = i;
            i += 1;
            while i < len {
                if bytes[i] == b'\\' && i + 1 < len {
                    i += 2;
                    continue;
                }
                if bytes[i] == quote {
                    i += 1;
                    break;
                }
                i += 1;
            }
            out.push((&line[start..i], COL_STRING));
            continue;
        }
        // Number literal
        if b.is_ascii_digit() {
            let start = i;
            i += 1;
            while i < len
                && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'.' || bytes[i] == b'_')
            {
                i += 1;
            }
            out.push((&line[start..i], COL_NUMBER));
            continue;
        }
        // Identifier / keyword / type
        if b.is_ascii_alphabetic() || b == b'_' || b == b'$' {
            let start = i;
            i += 1;
            while i < len
                && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_' || bytes[i] == b'$')
            {
                i += 1;
            }
            let word = &line[start..i];
            let col = if is_code_keyword(word) {
                COL_KEYWORD
            } else if is_code_type(word) {
                COL_TYPE
            } else {
                COL_IDENT
            };
            out.push((word, col));
            continue;
        }
        // Punctuation or UTF-8 symbol (step safely by char boundary)
        let start = i;
        let ch_len = line[i..].chars().next().map(char::len_utf8).unwrap_or(1);
        i += ch_len;
        out.push((&line[start..i], COL_PUNCT));
    }

    out
}

// ============================================================================
// 67 Built-in Crisp Analytical Anti-Aliased Vector Icons (Normalized 0..24 space)
// ============================================================================

/// Canonical list of all 67 built-in analytical vector icon names supported by both CPU and GPU engines.
pub const SUPPORTED_ICON_NAMES: &[&str] = &[
    "activity",
    "alert",
    "arrow-left",
    "arrow-right",
    "bell",
    "bookmark",
    "box",
    "calendar",
    "chart",
    "check",
    "chevron-down",
    "chevron-left",
    "chevron-right",
    "chevron-up",
    "clock",
    "code",
    "command",
    "copy",
    "cpu",
    "cube",
    "database",
    "diamond",
    "download",
    "edit",
    "external",
    "eye",
    "file",
    "filter",
    "folder",
    "git",
    "git-branch",
    "globe",
    "heart",
    "home",
    "inbox",
    "info",
    "layers",
    "lock",
    "mail",
    "menu",
    "minus",
    "moon",
    "more",
    "palette",
    "pause",
    "play",
    "plus",
    "refresh",
    "rocket",
    "search",
    "send",
    "settings",
    "shield",
    "sliders",
    "sparkles",
    "star",
    "sun",
    "tag",
    "terminal",
    "trash",
    "unlock",
    "upload",
    "user",
    "users",
    "wifi",
    "x",
    "zap",
];

/// Returns all supported built-in analytical vector icon names (for tests, auditor, and LSP).
pub fn supported_icon_names() -> &'static [&'static str] {
    SUPPORTED_ICON_NAMES
}

fn push_circle_path(pb: &mut PathBuilder, cx: f32, cy: f32, r: f32) {
    const K: f32 = 0.552_284_8;
    let kr = r * K;
    pb.move_to(cx, cy - r);
    pb.cubic_to(cx + kr, cy - r, cx + r, cy - kr, cx + r, cy);
    pb.cubic_to(cx + r, cy + kr, cx + kr, cy + r, cx, cy + r);
    pb.cubic_to(cx - kr, cy + r, cx - r, cy + kr, cx - r, cy);
    pb.cubic_to(cx - r, cy - kr, cx - kr, cy - r, cx, cy - r);
    pb.close();
}

fn push_ellipse_path(pb: &mut PathBuilder, cx: f32, cy: f32, rx: f32, ry: f32) {
    const K: f32 = 0.552_284_8;
    let krx = rx * K;
    let kry = ry * K;
    pb.move_to(cx, cy - ry);
    pb.cubic_to(cx + krx, cy - ry, cx + rx, cy - kry, cx + rx, cy);
    pb.cubic_to(cx + rx, cy + kry, cx + krx, cy + ry, cx, cy + ry);
    pb.cubic_to(cx - krx, cy + ry, cx - rx, cy + kry, cx - rx, cy);
    pb.cubic_to(cx - rx, cy - kry, cx - krx, cy - ry, cx, cy - ry);
    pb.close();
}

fn push_round_rect_24(pb: &mut PathBuilder, x: f32, y: f32, w: f32, h: f32, r: f32) {
    let rc = r.min(w * 0.5).min(h * 0.5);
    const K: f32 = 0.552_284_8;
    let kr = rc * K;
    pb.move_to(x + rc, y);
    pb.line_to(x + w - rc, y);
    pb.cubic_to(x + w - rc + kr, y, x + w, y + rc - kr, x + w, y + rc);
    pb.line_to(x + w, y + h - rc);
    pb.cubic_to(x + w, y + h - rc + kr, x + w - rc + kr, y + h, x + w - rc, y + h);
    pb.line_to(x + rc, y + h);
    pb.cubic_to(x + rc - kr, y + h, x, y + h - rc + kr, x, y + h - rc);
    pb.line_to(x, y + rc);
    pb.cubic_to(x, y + rc - kr, x + rc - kr, y, x + rc, y);
    pb.close();
}

/// Paints one of the 67 crisp anti-aliased analytical vector icons (or a geometric fallback ring)
/// scaled cleanly from normalized `0..24` coordinates into `rect`.
pub fn paint_vector_icon(
    pixmap: &mut Pixmap,
    rect: LayoutRect,
    name: &str,
    color: (u8, u8, u8, u8),
    xform: Transform,
) {
    paint_vector_icon_clipped(pixmap, rect, name, color, xform, None);
}

fn paint_vector_icon_clipped(
    pixmap: &mut Pixmap,
    rect: LayoutRect,
    name: &str,
    color: (u8, u8, u8, u8),
    xform: Transform,
    clip_mask: Option<&Mask>,
) {
    if rect.w <= 0.5 || rect.h <= 0.5 || color.3 == 0 {
        return;
    }
    let norm_name = name.trim().to_ascii_lowercase().replace('_', "-");
    let icon_xform = xform
        .pre_translate(rect.x, rect.y)
        .pre_scale(rect.w / 24.0, rect.h / 24.0);

    let mut stroke_pb = PathBuilder::new();
    let mut fill_pb = PathBuilder::new();

    match norm_name.as_str() {
        "zap" => {
            stroke_pb.move_to(13.0, 2.0);
            stroke_pb.line_to(3.5, 13.5);
            stroke_pb.line_to(12.0, 13.5);
            stroke_pb.line_to(11.0, 22.0);
            stroke_pb.line_to(20.5, 10.5);
            stroke_pb.line_to(12.0, 10.5);
            stroke_pb.close();
        }
        "cpu" => {
            push_round_rect_24(&mut stroke_pb, 5.5, 5.5, 13.0, 13.0, 2.0);
            push_round_rect_24(&mut stroke_pb, 9.0, 9.0, 6.0, 6.0, 1.0);
            for p in [9.0f32, 15.0] {
                stroke_pb.move_to(p, 2.0);
                stroke_pb.line_to(p, 5.5);
                stroke_pb.move_to(p, 18.5);
                stroke_pb.line_to(p, 22.0);
                stroke_pb.move_to(2.0, p);
                stroke_pb.line_to(5.5, p);
                stroke_pb.move_to(18.5, p);
                stroke_pb.line_to(22.0, p);
            }
        }
        "layers" => {
            stroke_pb.move_to(12.0, 2.5);
            stroke_pb.line_to(21.5, 7.2);
            stroke_pb.line_to(12.0, 12.0);
            stroke_pb.line_to(2.5, 7.2);
            stroke_pb.close();
            stroke_pb.move_to(2.5, 12.0);
            stroke_pb.line_to(12.0, 16.8);
            stroke_pb.line_to(21.5, 12.0);
            stroke_pb.move_to(2.5, 16.8);
            stroke_pb.line_to(12.0, 21.5);
            stroke_pb.line_to(21.5, 16.8);
        }
        "shield" => {
            stroke_pb.move_to(12.0, 2.5);
            stroke_pb.line_to(20.0, 5.5);
            stroke_pb.line_to(20.0, 11.5);
            stroke_pb.cubic_to(20.0, 16.5, 16.5, 19.8, 12.0, 21.8);
            stroke_pb.cubic_to(7.5, 19.8, 4.0, 16.5, 4.0, 11.5);
            stroke_pb.line_to(4.0, 5.5);
            stroke_pb.close();
        }
        "terminal" => {
            stroke_pb.move_to(4.0, 17.0);
            stroke_pb.line_to(10.0, 11.0);
            stroke_pb.line_to(4.0, 5.0);
            stroke_pb.move_to(12.0, 19.0);
            stroke_pb.line_to(20.0, 19.0);
        }
        "code" => {
            stroke_pb.move_to(8.0, 6.0);
            stroke_pb.line_to(2.5, 12.0);
            stroke_pb.line_to(8.0, 18.0);
            stroke_pb.move_to(16.0, 6.0);
            stroke_pb.line_to(21.5, 12.0);
            stroke_pb.line_to(16.0, 18.0);
            stroke_pb.move_to(13.8, 4.5);
            stroke_pb.line_to(10.2, 19.5);
        }
        "play" => {
            fill_pb.move_to(6.0, 4.0);
            fill_pb.line_to(20.0, 12.0);
            fill_pb.line_to(6.0, 20.0);
            fill_pb.close();
        }
        "pause" => {
            push_round_rect_24(&mut fill_pb, 5.5, 4.0, 4.2, 16.0, 1.2);
            push_round_rect_24(&mut fill_pb, 14.3, 4.0, 4.2, 16.0, 1.2);
        }
        "refresh" => {
            // Top-right curved arrow + bottom-left curved arrow
            stroke_pb.move_to(20.5, 4.5);
            stroke_pb.line_to(20.5, 9.5);
            stroke_pb.line_to(15.5, 9.5);
            stroke_pb.move_to(3.5, 19.5);
            stroke_pb.line_to(3.5, 14.5);
            stroke_pb.line_to(8.5, 14.5);
            stroke_pb.move_to(4.5, 9.5);
            stroke_pb.cubic_to(6.0, 5.0, 11.2, 3.2, 15.8, 5.0);
            stroke_pb.cubic_to(17.8, 5.8, 19.4, 7.4, 20.4, 9.4);
            stroke_pb.move_to(19.5, 14.5);
            stroke_pb.cubic_to(18.0, 19.0, 12.8, 20.8, 8.2, 19.0);
            stroke_pb.cubic_to(6.2, 18.2, 4.6, 16.6, 3.6, 14.6);
        }
        "sparkles" => {
            stroke_pb.move_to(12.0, 2.5);
            stroke_pb.quad_to(13.2, 10.8, 21.5, 12.0);
            stroke_pb.quad_to(13.2, 13.2, 12.0, 21.5);
            stroke_pb.quad_to(10.8, 13.2, 2.5, 12.0);
            stroke_pb.quad_to(10.8, 10.8, 12.0, 2.5);
            stroke_pb.close();
            stroke_pb.move_to(19.0, 3.5);
            stroke_pb.line_to(19.0, 7.0);
            stroke_pb.move_to(17.25, 5.25);
            stroke_pb.line_to(20.75, 5.25);
            stroke_pb.move_to(5.0, 17.0);
            stroke_pb.line_to(5.0, 20.0);
            stroke_pb.move_to(3.5, 18.5);
            stroke_pb.line_to(6.5, 18.5);
        }
        "box" | "cube" => {
            stroke_pb.move_to(12.0, 2.5);
            stroke_pb.line_to(20.5, 7.2);
            stroke_pb.line_to(20.5, 16.8);
            stroke_pb.line_to(12.0, 21.5);
            stroke_pb.line_to(3.5, 16.8);
            stroke_pb.line_to(3.5, 7.2);
            stroke_pb.close();
            stroke_pb.move_to(3.5, 7.2);
            stroke_pb.line_to(12.0, 12.0);
            stroke_pb.line_to(20.5, 7.2);
            stroke_pb.move_to(12.0, 12.0);
            stroke_pb.line_to(12.0, 21.5);
            if norm_name == "cube" {
                push_circle_path(&mut fill_pb, 12.0, 12.0, 1.4);
            }
        }
        "check" => {
            stroke_pb.move_to(4.0, 12.5);
            stroke_pb.line_to(9.5, 18.0);
            stroke_pb.line_to(20.0, 6.5);
        }
        "x" => {
            stroke_pb.move_to(5.0, 5.0);
            stroke_pb.line_to(19.0, 19.0);
            stroke_pb.move_to(19.0, 5.0);
            stroke_pb.line_to(5.0, 19.0);
        }
        "alert" => {
            stroke_pb.move_to(12.0, 3.0);
            stroke_pb.line_to(21.2, 19.8);
            stroke_pb.line_to(2.8, 19.8);
            stroke_pb.close();
            stroke_pb.move_to(12.0, 9.2);
            stroke_pb.line_to(12.0, 13.8);
            push_circle_path(&mut fill_pb, 12.0, 16.8, 1.15);
        }
        "arrow-right" => {
            stroke_pb.move_to(4.0, 12.0);
            stroke_pb.line_to(20.0, 12.0);
            stroke_pb.move_to(13.5, 5.5);
            stroke_pb.line_to(20.0, 12.0);
            stroke_pb.line_to(13.5, 18.5);
        }
        "sliders" => {
            for (y, kx) in [(6.0f32, 9.0f32), (12.0, 16.0), (18.0, 8.0)] {
                stroke_pb.move_to(3.0, y);
                stroke_pb.line_to(kx - 2.4, y);
                stroke_pb.move_to(kx + 2.4, y);
                stroke_pb.line_to(21.0, y);
                push_circle_path(&mut stroke_pb, kx, y, 2.2);
            }
        }
        "eye" => {
            stroke_pb.move_to(2.0, 12.0);
            stroke_pb.cubic_to(5.5, 5.5, 18.5, 5.5, 22.0, 12.0);
            stroke_pb.cubic_to(18.5, 18.5, 5.5, 18.5, 2.0, 12.0);
            stroke_pb.close();
            push_circle_path(&mut stroke_pb, 12.0, 12.0, 3.0);
        }
        "globe" => {
            push_circle_path(&mut stroke_pb, 12.0, 12.0, 9.2);
            stroke_pb.move_to(2.8, 12.0);
            stroke_pb.line_to(21.2, 12.0);
            stroke_pb.move_to(12.0, 2.8);
            stroke_pb.cubic_to(16.0, 6.5, 16.0, 17.5, 12.0, 21.2);
            stroke_pb.move_to(12.0, 2.8);
            stroke_pb.cubic_to(8.0, 6.5, 8.0, 17.5, 12.0, 21.2);
        }
        "lock" => {
            push_round_rect_24(&mut stroke_pb, 4.5, 11.0, 15.0, 10.0, 2.2);
            stroke_pb.move_to(8.0, 11.0);
            stroke_pb.line_to(8.0, 7.5);
            stroke_pb.cubic_to(8.0, 4.2, 16.0, 4.2, 16.0, 7.5);
            stroke_pb.line_to(16.0, 11.0);
            push_circle_path(&mut fill_pb, 12.0, 16.0, 1.4);
        }
        "rocket" => {
            stroke_pb.move_to(12.0, 15.0);
            stroke_pb.line_to(9.0, 12.0);
            stroke_pb.cubic_to(11.5, 7.2, 16.0, 3.5, 20.5, 3.5);
            stroke_pb.cubic_to(20.5, 8.0, 16.8, 12.5, 12.0, 15.0);
            stroke_pb.close();
            stroke_pb.move_to(9.0, 12.0);
            stroke_pb.line_to(5.0, 12.5);
            stroke_pb.line_to(7.5, 16.5);
            stroke_pb.move_to(12.0, 15.0);
            stroke_pb.line_to(11.5, 19.0);
            stroke_pb.line_to(7.5, 16.5);
            stroke_pb.move_to(5.5, 18.5);
            stroke_pb.line_to(3.5, 20.5);
            push_circle_path(&mut fill_pb, 15.2, 8.8, 1.3);
        }
        "command" => {
            push_round_rect_24(&mut stroke_pb, 9.0, 9.0, 6.0, 6.0, 0.5);
            // Top-left, top-right, bottom-right, bottom-left loops
            push_round_rect_24(&mut stroke_pb, 3.5, 3.5, 5.5, 5.5, 2.75);
            push_round_rect_24(&mut stroke_pb, 15.0, 3.5, 5.5, 5.5, 2.75);
            push_round_rect_24(&mut stroke_pb, 15.0, 15.0, 5.5, 5.5, 2.75);
            push_round_rect_24(&mut stroke_pb, 3.5, 15.0, 5.5, 5.5, 2.75);
        }
        "git-branch" => {
            stroke_pb.move_to(6.0, 8.5);
            stroke_pb.line_to(6.0, 15.5);
            push_circle_path(&mut stroke_pb, 6.0, 6.0, 2.4);
            push_circle_path(&mut stroke_pb, 6.0, 18.0, 2.4);
            push_circle_path(&mut stroke_pb, 18.0, 6.0, 2.4);
            stroke_pb.move_to(18.0, 8.5);
            stroke_pb.cubic_to(18.0, 13.5, 11.0, 13.5, 6.0, 15.5);
        }
        "palette" => {
            stroke_pb.move_to(12.0, 2.8);
            stroke_pb.cubic_to(17.5, 2.8, 21.2, 6.6, 21.2, 11.5);
            stroke_pb.cubic_to(21.2, 14.3, 19.2, 15.5, 17.0, 15.5);
            stroke_pb.line_to(15.2, 15.5);
            stroke_pb.cubic_to(13.6, 15.5, 13.0, 16.5, 13.8, 18.0);
            stroke_pb.cubic_to(14.5, 19.5, 13.6, 21.2, 11.8, 21.2);
            stroke_pb.cubic_to(6.4, 21.2, 2.8, 17.2, 2.8, 12.0);
            stroke_pb.cubic_to(2.8, 6.8, 6.8, 2.8, 12.0, 2.8);
            stroke_pb.close();
            push_circle_path(&mut fill_pb, 7.5, 10.0, 1.3);
            push_circle_path(&mut fill_pb, 10.5, 6.8, 1.3);
            push_circle_path(&mut fill_pb, 15.0, 7.5, 1.3);
            push_circle_path(&mut fill_pb, 17.2, 11.2, 1.3);
        }
        "activity" => {
            stroke_pb.move_to(2.0, 12.0);
            stroke_pb.line_to(6.0, 12.0);
            stroke_pb.line_to(9.0, 4.0);
            stroke_pb.line_to(15.0, 20.0);
            stroke_pb.line_to(18.0, 12.0);
            stroke_pb.line_to(22.0, 12.0);
        }
        "plus" => {
            stroke_pb.move_to(12.0, 4.5);
            stroke_pb.line_to(12.0, 19.5);
            stroke_pb.move_to(4.5, 12.0);
            stroke_pb.line_to(19.5, 12.0);
        }
        "minus" => {
            stroke_pb.move_to(4.5, 12.0);
            stroke_pb.line_to(19.5, 12.0);
        }
        "copy" => {
            push_round_rect_24(&mut stroke_pb, 8.8, 8.8, 11.4, 11.4, 2.0);
            stroke_pb.move_to(5.5, 15.0);
            stroke_pb.line_to(4.5, 15.0);
            stroke_pb.cubic_to(3.8, 15.0, 3.5, 14.6, 3.5, 14.0);
            stroke_pb.line_to(3.5, 4.5);
            stroke_pb.cubic_to(3.5, 3.8, 3.8, 3.5, 4.5, 3.5);
            stroke_pb.line_to(14.0, 3.5);
            stroke_pb.cubic_to(14.6, 3.5, 15.0, 3.8, 15.0, 4.5);
            stroke_pb.line_to(15.0, 5.5);
        }
        "sun" => {
            push_circle_path(&mut stroke_pb, 12.0, 12.0, 4.2);
            for i in 0..8 {
                let ang = (i as f32) * std::f32::consts::FRAC_PI_4;
                let (s, c) = ang.sin_cos();
                stroke_pb.move_to(12.0 + c * 6.8, 12.0 + s * 6.8);
                stroke_pb.line_to(12.0 + c * 9.8, 12.0 + s * 9.8);
            }
        }
        "moon" => {
            stroke_pb.move_to(20.2, 13.8);
            stroke_pb.cubic_to(18.8, 18.2, 14.2, 21.0, 9.6, 20.0);
            stroke_pb.cubic_to(5.0, 19.0, 2.4, 14.2, 3.8, 9.6);
            stroke_pb.cubic_to(4.8, 6.2, 7.6, 3.8, 11.0, 3.4);
            stroke_pb.cubic_to(8.8, 7.4, 9.6, 12.2, 13.2, 14.2);
            stroke_pb.cubic_to(15.4, 15.4, 18.0, 15.2, 20.2, 13.8);
            stroke_pb.close();
        }
        "diamond" => {
            stroke_pb.move_to(12.0, 2.5);
            stroke_pb.line_to(21.2, 12.0);
            stroke_pb.line_to(12.0, 21.5);
            stroke_pb.line_to(2.8, 12.0);
            stroke_pb.close();
            stroke_pb.move_to(7.5, 12.0);
            stroke_pb.line_to(12.0, 7.5);
            stroke_pb.line_to(16.5, 12.0);
            stroke_pb.line_to(12.0, 16.5);
            stroke_pb.close();
        }
        "search" => {
            push_circle_path(&mut stroke_pb, 10.5, 10.5, 6.8);
            stroke_pb.move_to(15.6, 15.6);
            stroke_pb.line_to(21.0, 21.0);
        }
        "trash" => {
            stroke_pb.move_to(3.5, 6.0);
            stroke_pb.line_to(20.5, 6.0);
            stroke_pb.move_to(9.0, 6.0);
            stroke_pb.line_to(9.0, 3.8);
            stroke_pb.line_to(15.0, 3.8);
            stroke_pb.line_to(15.0, 6.0);
            stroke_pb.move_to(5.5, 6.0);
            stroke_pb.line_to(6.6, 19.8);
            stroke_pb.cubic_to(6.7, 20.6, 7.3, 21.2, 8.2, 21.2);
            stroke_pb.line_to(15.8, 21.2);
            stroke_pb.cubic_to(16.7, 21.2, 17.3, 20.6, 17.4, 19.8);
            stroke_pb.line_to(18.5, 6.0);
            stroke_pb.move_to(10.0, 10.2);
            stroke_pb.line_to(10.0, 17.2);
            stroke_pb.move_to(14.0, 10.2);
            stroke_pb.line_to(14.0, 17.2);
        }
        "edit" => {
            stroke_pb.move_to(16.2, 3.8);
            stroke_pb.line_to(20.2, 7.8);
            stroke_pb.line_to(8.8, 19.2);
            stroke_pb.line_to(3.8, 20.2);
            stroke_pb.line_to(4.8, 15.2);
            stroke_pb.close();
            stroke_pb.move_to(13.8, 6.2);
            stroke_pb.line_to(17.8, 10.2);
        }
        "download" => {
            stroke_pb.move_to(12.0, 3.0);
            stroke_pb.line_to(12.0, 15.0);
            stroke_pb.move_to(7.5, 10.8);
            stroke_pb.line_to(12.0, 15.3);
            stroke_pb.line_to(16.5, 10.8);
            stroke_pb.move_to(4.0, 16.5);
            stroke_pb.line_to(4.0, 19.5);
            stroke_pb.cubic_to(4.0, 20.3, 4.7, 21.0, 5.5, 21.0);
            stroke_pb.line_to(18.5, 21.0);
            stroke_pb.cubic_to(19.3, 21.0, 20.0, 20.3, 20.0, 19.5);
            stroke_pb.line_to(20.0, 16.5);
        }
        "upload" => {
            stroke_pb.move_to(12.0, 15.5);
            stroke_pb.line_to(12.0, 3.5);
            stroke_pb.move_to(7.5, 7.8);
            stroke_pb.line_to(12.0, 3.3);
            stroke_pb.line_to(16.5, 7.8);
            stroke_pb.move_to(4.0, 16.5);
            stroke_pb.line_to(4.0, 19.5);
            stroke_pb.cubic_to(4.0, 20.3, 4.7, 21.0, 5.5, 21.0);
            stroke_pb.line_to(18.5, 21.0);
            stroke_pb.cubic_to(19.3, 21.0, 20.0, 20.3, 20.0, 19.5);
            stroke_pb.line_to(20.0, 16.5);
        }
        "folder" => {
            stroke_pb.move_to(3.0, 6.5);
            stroke_pb.cubic_to(3.0, 5.5, 3.8, 4.8, 4.8, 4.8);
            stroke_pb.line_to(9.4, 4.8);
            stroke_pb.line_to(11.4, 7.2);
            stroke_pb.line_to(19.2, 7.2);
            stroke_pb.cubic_to(20.2, 7.2, 21.0, 8.0, 21.0, 9.0);
            stroke_pb.line_to(21.0, 18.2);
            stroke_pb.cubic_to(21.0, 19.2, 20.2, 20.0, 19.2, 20.0);
            stroke_pb.line_to(4.8, 20.0);
            stroke_pb.cubic_to(3.8, 20.0, 3.0, 19.2, 3.0, 18.2);
            stroke_pb.close();
        }
        "file" => {
            stroke_pb.move_to(6.0, 2.8);
            stroke_pb.line_to(14.0, 2.8);
            stroke_pb.line_to(19.0, 7.8);
            stroke_pb.line_to(19.0, 19.6);
            stroke_pb.cubic_to(19.0, 20.5, 18.3, 21.2, 17.4, 21.2);
            stroke_pb.line_to(6.6, 21.2);
            stroke_pb.cubic_to(5.7, 21.2, 5.0, 20.5, 5.0, 19.6);
            stroke_pb.line_to(5.0, 4.4);
            stroke_pb.cubic_to(5.0, 3.5, 5.5, 2.8, 6.0, 2.8);
            stroke_pb.close();
            stroke_pb.move_to(14.0, 2.8);
            stroke_pb.line_to(14.0, 7.8);
            stroke_pb.line_to(19.0, 7.8);
            stroke_pb.move_to(8.5, 12.5);
            stroke_pb.line_to(15.5, 12.5);
            stroke_pb.move_to(8.5, 16.5);
            stroke_pb.line_to(13.5, 16.5);
        }
        "home" => {
            stroke_pb.move_to(3.0, 10.8);
            stroke_pb.line_to(12.0, 3.2);
            stroke_pb.line_to(21.0, 10.8);
            stroke_pb.move_to(5.2, 9.2);
            stroke_pb.line_to(5.2, 19.6);
            stroke_pb.cubic_to(5.2, 20.4, 5.8, 21.0, 6.6, 21.0);
            stroke_pb.line_to(17.4, 21.0);
            stroke_pb.cubic_to(18.2, 21.0, 18.8, 20.4, 18.8, 19.6);
            stroke_pb.line_to(18.8, 9.2);
            stroke_pb.move_to(9.6, 21.0);
            stroke_pb.line_to(9.6, 13.8);
            stroke_pb.line_to(14.4, 13.8);
            stroke_pb.line_to(14.4, 21.0);
        }
        "bell" => {
            stroke_pb.move_to(18.2, 14.5);
            stroke_pb.line_to(18.2, 9.6);
            stroke_pb.cubic_to(18.2, 6.0, 15.5, 3.4, 12.0, 3.4);
            stroke_pb.cubic_to(8.5, 3.4, 5.8, 6.0, 5.8, 9.6);
            stroke_pb.line_to(5.8, 14.5);
            stroke_pb.line_to(3.8, 17.2);
            stroke_pb.line_to(20.2, 17.2);
            stroke_pb.close();
            stroke_pb.move_to(10.0, 19.8);
            stroke_pb.cubic_to(10.7, 21.2, 13.3, 21.2, 14.0, 19.8);
        }
        "mail" => {
            push_round_rect_24(&mut stroke_pb, 3.0, 5.0, 18.0, 14.0, 2.2);
            stroke_pb.move_to(3.6, 6.2);
            stroke_pb.line_to(12.0, 12.8);
            stroke_pb.line_to(20.4, 6.2);
        }
        "user" => {
            push_circle_path(&mut stroke_pb, 12.0, 7.6, 3.8);
            stroke_pb.move_to(4.4, 20.6);
            stroke_pb.cubic_to(4.4, 15.8, 7.8, 14.0, 12.0, 14.0);
            stroke_pb.cubic_to(16.2, 14.0, 19.6, 15.8, 19.6, 20.6);
        }
        "users" => {
            push_circle_path(&mut stroke_pb, 9.2, 8.0, 3.2);
            stroke_pb.move_to(2.8, 20.2);
            stroke_pb.cubic_to(2.8, 16.0, 5.6, 14.4, 9.2, 14.4);
            stroke_pb.cubic_to(12.8, 14.4, 15.6, 16.0, 15.6, 20.2);
            push_circle_path(&mut stroke_pb, 16.4, 8.6, 2.4);
            stroke_pb.move_to(16.5, 14.6);
            stroke_pb.cubic_to(19.2, 14.8, 21.2, 16.4, 21.2, 20.2);
        }
        "unlock" => {
            push_round_rect_24(&mut stroke_pb, 4.5, 11.0, 15.0, 10.0, 2.2);
            stroke_pb.move_to(8.0, 11.0);
            stroke_pb.line_to(8.0, 7.2);
            stroke_pb.cubic_to(8.0, 3.8, 14.8, 3.4, 16.2, 6.4);
            push_circle_path(&mut fill_pb, 12.0, 16.0, 1.4);
        }
        "wifi" => {
            stroke_pb.move_to(2.6, 9.2);
            stroke_pb.cubic_to(7.8, 4.2, 16.2, 4.2, 21.4, 9.2);
            stroke_pb.move_to(5.6, 12.6);
            stroke_pb.cubic_to(9.2, 9.2, 14.8, 9.2, 18.4, 12.6);
            stroke_pb.move_to(8.8, 15.8);
            stroke_pb.cubic_to(10.6, 14.2, 13.4, 14.2, 15.2, 15.8);
            push_circle_path(&mut fill_pb, 12.0, 19.2, 1.5);
        }
        "heart" => {
            stroke_pb.move_to(12.0, 20.4);
            stroke_pb.cubic_to(12.0, 20.4, 3.2, 14.8, 3.2, 8.8);
            stroke_pb.cubic_to(3.2, 5.5, 5.7, 3.6, 8.4, 3.6);
            stroke_pb.cubic_to(10.2, 3.6, 11.4, 4.6, 12.0, 6.0);
            stroke_pb.cubic_to(12.6, 4.6, 13.8, 3.6, 15.6, 3.6);
            stroke_pb.cubic_to(18.3, 3.6, 20.8, 5.5, 20.8, 8.8);
            stroke_pb.cubic_to(20.8, 14.8, 12.0, 20.4, 12.0, 20.4);
            stroke_pb.close();
        }
        "star" => {
            let cx = 12.0f32;
            let cy = 12.2f32;
            let r_outer = 9.4f32;
            let r_inner = 4.1f32;
            for i in 0..10 {
                let r = if i % 2 == 0 { r_outer } else { r_inner };
                let ang = -std::f32::consts::FRAC_PI_2
                    + (i as f32) * std::f32::consts::PI / 5.0;
                let (s, c) = ang.sin_cos();
                let px = cx + c * r;
                let py = cy + s * r;
                if i == 0 {
                    stroke_pb.move_to(px, py);
                } else {
                    stroke_pb.line_to(px, py);
                }
            }
            stroke_pb.close();
        }
        "bookmark" => {
            stroke_pb.move_to(6.0, 4.2);
            stroke_pb.cubic_to(6.0, 3.4, 6.7, 2.8, 7.5, 2.8);
            stroke_pb.line_to(16.5, 2.8);
            stroke_pb.cubic_to(17.3, 2.8, 18.0, 3.4, 18.0, 4.2);
            stroke_pb.line_to(18.0, 21.0);
            stroke_pb.line_to(12.0, 16.8);
            stroke_pb.line_to(6.0, 21.0);
            stroke_pb.close();
        }
        "filter" => {
            stroke_pb.move_to(3.0, 4.2);
            stroke_pb.line_to(21.0, 4.2);
            stroke_pb.line_to(14.0, 12.6);
            stroke_pb.line_to(14.0, 19.4);
            stroke_pb.line_to(10.0, 21.2);
            stroke_pb.line_to(10.0, 12.6);
            stroke_pb.close();
        }
        "external" => {
            stroke_pb.move_to(14.5, 3.5);
            stroke_pb.line_to(20.5, 3.5);
            stroke_pb.line_to(20.5, 9.5);
            stroke_pb.move_to(10.5, 13.5);
            stroke_pb.line_to(20.5, 3.5);
            stroke_pb.move_to(18.5, 13.5);
            stroke_pb.line_to(18.5, 19.0);
            stroke_pb.cubic_to(18.5, 19.9, 17.8, 20.5, 16.9, 20.5);
            stroke_pb.line_to(5.1, 20.5);
            stroke_pb.cubic_to(4.2, 20.5, 3.5, 19.9, 3.5, 19.0);
            stroke_pb.line_to(3.5, 7.1);
            stroke_pb.cubic_to(3.5, 6.2, 4.2, 5.5, 5.1, 5.5);
            stroke_pb.line_to(10.5, 5.5);
        }
        "chevron-left" => {
            stroke_pb.move_to(15.2, 5.2);
            stroke_pb.line_to(8.4, 12.0);
            stroke_pb.line_to(15.2, 18.8);
        }
        "chevron-right" => {
            stroke_pb.move_to(8.8, 5.2);
            stroke_pb.line_to(15.6, 12.0);
            stroke_pb.line_to(8.8, 18.8);
        }
        "chevron-up" => {
            stroke_pb.move_to(5.2, 15.2);
            stroke_pb.line_to(12.0, 8.4);
            stroke_pb.line_to(18.8, 15.2);
        }
        "chevron-down" => {
            stroke_pb.move_to(5.2, 8.8);
            stroke_pb.line_to(12.0, 15.6);
            stroke_pb.line_to(18.8, 8.8);
        }
        "arrow-left" => {
            stroke_pb.move_to(20.0, 12.0);
            stroke_pb.line_to(4.0, 12.0);
            stroke_pb.move_to(10.5, 5.5);
            stroke_pb.line_to(4.0, 12.0);
            stroke_pb.line_to(10.5, 18.5);
        }
        "menu" => {
            stroke_pb.move_to(3.5, 6.2);
            stroke_pb.line_to(20.5, 6.2);
            stroke_pb.move_to(3.5, 12.0);
            stroke_pb.line_to(20.5, 12.0);
            stroke_pb.move_to(3.5, 17.8);
            stroke_pb.line_to(20.5, 17.8);
        }
        "more" => {
            push_circle_path(&mut fill_pb, 5.5, 12.0, 1.8);
            push_circle_path(&mut fill_pb, 12.0, 12.0, 1.8);
            push_circle_path(&mut fill_pb, 18.5, 12.0, 1.8);
        }
        "info" => {
            push_circle_path(&mut stroke_pb, 12.0, 12.0, 9.2);
            stroke_pb.move_to(12.0, 11.2);
            stroke_pb.line_to(12.0, 16.6);
            push_circle_path(&mut fill_pb, 12.0, 7.8, 1.25);
        }
        "calendar" => {
            push_round_rect_24(&mut stroke_pb, 3.2, 5.2, 17.6, 15.8, 2.2);
            stroke_pb.move_to(3.2, 10.2);
            stroke_pb.line_to(20.8, 10.2);
            stroke_pb.move_to(8.0, 2.8);
            stroke_pb.line_to(8.0, 6.8);
            stroke_pb.move_to(16.0, 2.8);
            stroke_pb.line_to(16.0, 6.8);
            push_circle_path(&mut fill_pb, 8.2, 14.2, 1.1);
            push_circle_path(&mut fill_pb, 12.0, 14.2, 1.1);
            push_circle_path(&mut fill_pb, 15.8, 14.2, 1.1);
        }
        "clock" => {
            push_circle_path(&mut stroke_pb, 12.0, 12.0, 9.2);
            stroke_pb.move_to(12.0, 6.6);
            stroke_pb.line_to(12.0, 12.0);
            stroke_pb.line_to(16.2, 14.2);
        }
        "chart" => {
            stroke_pb.move_to(3.5, 4.0);
            stroke_pb.line_to(3.5, 20.0);
            stroke_pb.line_to(20.5, 20.0);
            push_round_rect_24(&mut fill_pb, 6.8, 13.0, 2.8, 4.8, 0.8);
            push_round_rect_24(&mut fill_pb, 11.4, 9.2, 2.8, 8.6, 0.8);
            push_round_rect_24(&mut fill_pb, 16.0, 5.6, 2.8, 12.2, 0.8);
        }
        "database" => {
            push_ellipse_path(&mut stroke_pb, 12.0, 5.8, 8.0, 2.8);
            stroke_pb.move_to(4.0, 5.8);
            stroke_pb.line_to(4.0, 18.2);
            stroke_pb.cubic_to(4.0, 20.2, 20.0, 20.2, 20.0, 18.2);
            stroke_pb.line_to(20.0, 5.8);
            stroke_pb.move_to(4.0, 12.0);
            stroke_pb.cubic_to(4.0, 14.0, 20.0, 14.0, 20.0, 12.0);
        }
        "git" => {
            stroke_pb.move_to(12.0, 2.8);
            stroke_pb.line_to(21.2, 12.0);
            stroke_pb.line_to(12.0, 21.2);
            stroke_pb.line_to(2.8, 12.0);
            stroke_pb.close();
            stroke_pb.move_to(9.5, 7.2);
            stroke_pb.line_to(14.8, 12.5);
            stroke_pb.move_to(11.4, 9.2);
            stroke_pb.line_to(11.4, 15.8);
            push_circle_path(&mut fill_pb, 11.4, 15.8, 1.3);
            push_circle_path(&mut fill_pb, 14.8, 12.5, 1.3);
        }
        "settings" => {
            push_circle_path(&mut stroke_pb, 12.0, 12.0, 3.2);
            push_circle_path(&mut stroke_pb, 12.0, 12.0, 6.8);
            for i in 0..8 {
                let ang = (i as f32) * std::f32::consts::FRAC_PI_4;
                let (s, c) = ang.sin_cos();
                stroke_pb.move_to(12.0 + c * 6.8, 12.0 + s * 6.8);
                stroke_pb.line_to(12.0 + c * 9.8, 12.0 + s * 9.8);
            }
        }
        "send" => {
            stroke_pb.move_to(21.0, 3.2);
            stroke_pb.line_to(14.2, 20.8);
            stroke_pb.line_to(10.6, 13.4);
            stroke_pb.line_to(3.2, 9.8);
            stroke_pb.close();
            stroke_pb.move_to(21.0, 3.2);
            stroke_pb.line_to(10.6, 13.4);
        }
        "inbox" => {
            stroke_pb.move_to(5.5, 4.5);
            stroke_pb.line_to(18.5, 4.5);
            stroke_pb.line_to(21.0, 13.0);
            stroke_pb.line_to(21.0, 18.8);
            stroke_pb.cubic_to(21.0, 19.7, 20.3, 20.4, 19.4, 20.4);
            stroke_pb.line_to(4.6, 20.4);
            stroke_pb.cubic_to(3.7, 20.4, 3.0, 19.7, 3.0, 18.8);
            stroke_pb.line_to(3.0, 13.0);
            stroke_pb.close();
            stroke_pb.move_to(3.2, 13.0);
            stroke_pb.line_to(8.4, 13.0);
            stroke_pb.line_to(9.8, 15.8);
            stroke_pb.line_to(14.2, 15.8);
            stroke_pb.line_to(15.6, 13.0);
            stroke_pb.line_to(20.8, 13.0);
        }
        "tag" => {
            stroke_pb.move_to(11.6, 3.4);
            stroke_pb.line_to(4.8, 3.4);
            stroke_pb.cubic_to(4.0, 3.4, 3.4, 4.0, 3.4, 4.8);
            stroke_pb.line_to(3.4, 11.6);
            stroke_pb.line_to(12.2, 20.4);
            stroke_pb.cubic_to(12.9, 21.1, 14.0, 21.1, 14.7, 20.4);
            stroke_pb.line_to(20.4, 14.7);
            stroke_pb.cubic_to(21.1, 14.0, 21.1, 12.9, 20.4, 12.2);
            stroke_pb.close();
            push_circle_path(&mut fill_pb, 7.6, 7.6, 1.4);
        }
        _ => {
            // Clean geometric fallback ring + center dot
            push_circle_path(&mut stroke_pb, 12.0, 12.0, 7.8);
            push_circle_path(&mut fill_pb, 12.0, 12.0, 1.8);
        }
    }

    let mut paint = Paint::default();
    paint.set_color_rgba8(color.0, color.1, color.2, color.3);
    paint.anti_alias = true;

    if let Some(fill_path) = fill_pb.finish() {
        pixmap.fill_path(&fill_path, &paint, FillRule::Winding, icon_xform, clip_mask);
    }
    if let Some(stroke_path) = stroke_pb.finish() {
        let stroke = Stroke {
            width: 1.9,
            line_cap: LineCap::Round,
            line_join: LineJoin::Round,
            ..Stroke::default()
        };
        pixmap.stroke_path(&stroke_path, &paint, &stroke, icon_xform, clip_mask);
    }
}

fn clone_mask(mask: &Mask) -> Option<Mask> {
    let size = IntSize::from_wh(mask.width(), mask.height())?;
    Mask::from_vec(mask.data().to_vec(), size)
}

fn push_clip_mask(
    pixmap_w: u32,
    pixmap_h: u32,
    parent: Option<&Mask>,
    rect: LayoutRect,
    radii: [f32; 4],
    xform: Transform,
) -> Option<Mask> {
    let path = build_rounded_rect_path_corners(rect, radii)?;
    if let Some(pm) = parent {
        let mut combined = clone_mask(pm)?;
        combined.intersect_path(&path, FillRule::Winding, true, xform);
        Some(combined)
    } else {
        let mut mask = Mask::new(pixmap_w, pixmap_h)?;
        mask.fill_path(&path, FillRule::Winding, true, xform);
        Some(mask)
    }
}

fn blit_pixmap_rounded(
    dst: &mut Pixmap,
    src: &Pixmap,
    rect: LayoutRect,
    radii: [f32; 4],
    opacity: f32,
    xform: Transform,
    clip_mask: Option<&Mask>,
) {
    let sx = rect.w / (src.width().max(1) as f32);
    let sy = rect.h / (src.height().max(1) as f32);
    let img_xform = xform.pre_translate(rect.x, rect.y).pre_scale(sx, sy);
    let paint = PixmapPaint {
        opacity,
        quality: FilterQuality::Bilinear,
        ..PixmapPaint::default()
    };

    let has_rounding = radii.iter().any(|&r| r > 0.5);
    if has_rounding {
        if let Some(mask) = push_clip_mask(dst.width(), dst.height(), clip_mask, rect, radii, xform)
        {
            dst.draw_pixmap(0, 0, src.as_ref(), &paint, img_xform, Some(&mask));
            return;
        }
    }
    dst.draw_pixmap(0, 0, src.as_ref(), &paint, img_xform, clip_mask);
}

/// Blits `src` into `rect` using aspect-preserving `cover` fit clipped cleanly to the
/// rounded-rect `rect` intersected with any active ancestor `clip_mask`.
fn blit_pixmap_cover_rounded(
    dst: &mut Pixmap,
    src: &Pixmap,
    rect: LayoutRect,
    radii: [f32; 4],
    opacity: f32,
    xform: Transform,
    clip_mask: Option<&Mask>,
) {
    let sw = src.width().max(1) as f32;
    let sh = src.height().max(1) as f32;
    let scale = (rect.w / sw).max(rect.h / sh);
    let draw_w = sw * scale;
    let draw_h = sh * scale;
    let off_x = rect.x + (rect.w - draw_w) * 0.5;
    let off_y = rect.y + (rect.h - draw_h) * 0.5;
    let img_xform = xform.pre_translate(off_x, off_y).pre_scale(scale, scale);
    let paint = PixmapPaint {
        opacity,
        quality: FilterQuality::Bilinear,
        ..PixmapPaint::default()
    };

    if let Some(mask) = push_clip_mask(dst.width(), dst.height(), clip_mask, rect, radii, xform) {
        dst.draw_pixmap(0, 0, src.as_ref(), &paint, img_xform, Some(&mask));
    } else {
        dst.draw_pixmap(0, 0, src.as_ref(), &paint, img_xform, clip_mask);
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
#[allow(clippy::too_many_arguments)]
fn paint_cpu_shader_preview(
    pixmap: &mut Pixmap,
    rect: LayoutRect,
    radii: [f32; 4],
    shader_name: &str,
    elapsed: f32,
    uniforms: &[f64],
    opacity: f32,
    clip_mask: Option<&Mask>,
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
    let opaque_fast = opacity >= 0.995 && clip_mask.is_none();
    let mask_data = clip_mask.map(Mask::data);
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
                        let px_idx = row + px as usize;
                        let cm_scale = if let Some(md) = mask_data {
                            let ma = md[px_idx];
                            if ma == 0 {
                                continue;
                            }
                            ma as f32 / 255.0
                        } else {
                            1.0
                        };
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
                        let mask = (0.5 - d).clamp(0.0, 1.0) * opacity * cm_scale;
                        if mask <= 0.004 {
                            continue;
                        }
                        let idx = px_idx * 4;
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
    clip_mask: Option<&Mask>,
) {
    if let Some(path) = build_rounded_rect_path_corners(rect, radii) {
        let mut paint = Paint::default();
        paint.set_color_rgba8(color.0, color.1, color.2, color.3);
        paint.anti_alias = true;
        pixmap.fill_path(&path, &paint, FillRule::Winding, xform, clip_mask);
    }
}

#[allow(clippy::too_many_arguments)]
fn fill_rounded_rect_linear_grad(
    pixmap: &mut Pixmap,
    rect: LayoutRect,
    radii: [f32; 4],
    angle_deg: f32,
    start: [f32; 4],
    end: [f32; 4],
    xform: Transform,
    clip_mask: Option<&Mask>,
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
            pixmap.fill_path(&path, &paint, FillRule::Winding, xform, clip_mask);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn fill_rounded_rect_radial_grad(
    pixmap: &mut Pixmap,
    rect: LayoutRect,
    radii: [f32; 4],
    center: [f32; 4],
    edge: [f32; 4],
    xform: Transform,
    clip_mask: Option<&Mask>,
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
            pixmap.fill_path(&path, &paint, FillRule::Winding, xform, clip_mask);
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
    clip_mask: Option<&Mask>,
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
        pixmap.stroke_path(&path, &paint, &stroke, xform, clip_mask);
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
    for (rgba, pixel) in src.as_chunks::<4>().0.iter().zip(dst.iter_mut()) {
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
