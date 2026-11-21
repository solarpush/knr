//! Pure-Rust 2D software framebuffer canvas and rasterizer.
//!
//! Provides a 32-bit ARGB software canvas (`Vec<u32>`), alpha blending,
//! primitive rendering (rectangles, rounded rectangles, borders, shadows,
//! lines, arrows, clipping, text), and deterministic headless snapshotting.

use crate::gui::font::get_glyph;
use crate::gui::style::Rect;


/// Pure-Rust software framebuffer canvas.
#[derive(Debug, Clone)]
pub struct Canvas {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u32>,
    clip_stack: Vec<Rect>,
}

impl Canvas {
    /// Creates a new canvas of given dimensions, initialized to transparent (0x00000000).
    pub fn new(width: u32, height: u32) -> Self {
        let size = (width as usize).saturating_mul(height as usize);
        Self {
            width,
            height,
            pixels: vec![0; size],
            clip_stack: Vec::new(),
        }
    }

    /// Returns canvas dimensions `(width, height)`.
    #[inline]
    pub fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// Returns a slice of the 32-bit ARGB pixel buffer.
    #[inline]
    pub fn pixels(&self) -> &[u32] {
        &self.pixels
    }

    /// Returns a mutable slice of the 32-bit ARGB pixel buffer.
    #[inline]
    pub fn pixels_mut(&mut self) -> &mut [u32] {
        &mut self.pixels
    }

    /// Pushes a clipping rectangle onto the stack.
    /// The active clip is the intersection of the new rectangle and the previous active clip.
    pub fn push_clip(&mut self, rect: Rect) {
        let active = if let Some(current) = self.clip_stack.last() {
            current.intersect(&rect).unwrap_or(Rect::new(0, 0, 0, 0))
        } else {
            let canvas_rect = Rect::new(0, 0, self.width, self.height);
            canvas_rect.intersect(&rect).unwrap_or(Rect::new(0, 0, 0, 0))
        };
        self.clip_stack.push(active);
    }

    /// Pops the top clipping rectangle from the stack.
    pub fn pop_clip(&mut self) {
        self.clip_stack.pop();
    }

    /// Returns the currently active clipping rectangle.
    #[inline]
    pub fn current_clip(&self) -> Rect {
        if let Some(r) = self.clip_stack.last() {
            *r
        } else {
            Rect::new(0, 0, self.width, self.height)
        }
    }

    /// Clears the entire canvas with the specified color.
    pub fn clear(&mut self, color: u32) {
        self.pixels.fill(color);
    }

    /// Retrieves the pixel at `(x, y)`, or None if out of bounds.
    #[inline]
    pub fn get_pixel(&self, x: i32, y: i32) -> Option<u32> {
        if x >= 0 && x < self.width as i32 && y >= 0 && y < self.height as i32 {
            let idx = (y as usize) * (self.width as usize) + (x as usize);
            Some(self.pixels[idx])
        } else {
            None
        }
    }

    /// Sets a pixel at `(x, y)` with alpha blending over the existing pixel.
    #[inline]
    pub fn set_pixel(&mut self, x: i32, y: i32, color: u32) {
        let clip = self.current_clip();
        if x < clip.x
            || x >= clip.x + clip.w as i32
            || y < clip.y
            || y >= clip.y + clip.h as i32
        {
            return;
        }

        let sa = (color >> 24) & 0xFF;
        if sa == 0 {

            return;
        }

        let idx = (y as usize) * (self.width as usize) + (x as usize);
        if sa == 255 {
            self.pixels[idx] = color;
            return;
        }

        let dst = self.pixels[idx];
        let da = (dst >> 24) & 0xFF;
        let sr = (color >> 16) & 0xFF;
        let sg = (color >> 8) & 0xFF;
        let sb = color & 0xFF;

        let dr = (dst >> 16) & 0xFF;
        let dg = (dst >> 8) & 0xFF;
        let db = dst & 0xFF;

        let inv_sa = 255 - sa;
        let da_inv_sa = (da * inv_sa + 127) / 255;
        let out_a = sa + da_inv_sa;
        if out_a == 0 {
            self.pixels[idx] = 0;
            return;
        }

        let out_r = ((sr * sa + dr * da_inv_sa) + out_a / 2) / out_a;
        let out_g = ((sg * sa + dg * da_inv_sa) + out_a / 2) / out_a;
        let out_b = ((sb * sa + db * da_inv_sa) + out_a / 2) / out_a;

        self.pixels[idx] = (out_a.min(255) << 24)
            | (out_r.min(255) << 16)
            | (out_g.min(255) << 8)
            | out_b.min(255);
    }

    /// Fills an axis-aligned rectangle with a solid or alpha color.
    pub fn fill_rect(&mut self, x: i32, y: i32, w: u32, h: u32, color: u32) {
        if w == 0 || h == 0 {
            return;
        }

        let clip = self.current_clip();
        let x_start = x.max(clip.x);
        let y_start = y.max(clip.y);
        let x_end = (x + w as i32).min(clip.x + clip.w as i32);
        let y_end = (y + h as i32).min(clip.y + clip.h as i32);

        if x_end <= x_start || y_end <= y_start {
            return;
        }

        let sa = (color >> 24) & 0xFF;
        if sa == 255 {
            for py in y_start..y_end {
                let row_start = (py as usize) * (self.width as usize);
                let idx_start = row_start + (x_start as usize);
                let idx_end = row_start + (x_end as usize);
                self.pixels[idx_start..idx_end].fill(color);
            }
        } else {
            for py in y_start..y_end {
                for px in x_start..x_end {
                    self.set_pixel(px, py, color);
                }
            }
        }
    }

    /// Strokes the border of an axis-aligned rectangle.
    pub fn stroke_rect(
        &mut self,
        x: i32,
        y: i32,
        w: u32,
        h: u32,
        color: u32,
        stroke_width: u32,
    ) {
        if w == 0 || h == 0 || stroke_width == 0 {
            return;
        }
        let max_sw = (w / 2).min(h / 2);
        if max_sw == 0 {
            self.fill_rect(x, y, w, h, color);
            return;
        }
        let sw = stroke_width.min(max_sw).max(1);

        // Top border
        self.fill_rect(x, y, w, sw, color);
        // Bottom border
        self.fill_rect(x, y + h as i32 - sw as i32, w, sw, color);
        // Left border
        self.fill_rect(x, y + sw as i32, sw, h - 2 * sw, color);
        // Right border
        self.fill_rect(x + w as i32 - sw as i32, y + sw as i32, sw, h - 2 * sw, color);
    }

    /// Fills a rounded rectangle with corner radius `radius`.
    pub fn fill_rounded_rect(
        &mut self,
        x: i32,
        y: i32,
        w: u32,
        h: u32,
        radius: u32,
        color: u32,
    ) {
        if w == 0 || h == 0 {
            return;
        }
        let max_r = (w / 2).min(h / 2);
        let r = radius.min(max_r) as i32;

        if r <= 0 {
            self.fill_rect(x, y, w, h, color);
            return;
        }

        let clip = self.current_clip();
        let x_start = x.max(clip.x);
        let y_start = y.max(clip.y);
        let x_end = (x + w as i32).min(clip.x + clip.w as i32);
        let y_end = (y + h as i32).min(clip.y + clip.h as i32);

        if x_end <= x_start || y_end <= y_start {
            return;
        }

        let r_sq = r * r;
        let c_top_y = y + r;
        let c_bot_y = y + h as i32 - r - 1;
        let c_left_x = x + r;
        let c_right_x = x + w as i32 - r - 1;

        for py in y_start..y_end {
            for px in x_start..x_end {
                let in_corner = if px < c_left_x && py < c_top_y {
                    let dx = px - c_left_x;
                    let dy = py - c_top_y;
                    dx * dx + dy * dy <= r_sq
                } else if px > c_right_x && py < c_top_y {
                    let dx = px - c_right_x;
                    let dy = py - c_top_y;
                    dx * dx + dy * dy <= r_sq
                } else if px < c_left_x && py > c_bot_y {
                    let dx = px - c_left_x;
                    let dy = py - c_bot_y;
                    dx * dx + dy * dy <= r_sq
                } else if px > c_right_x && py > c_bot_y {
                    let dx = px - c_right_x;
                    let dy = py - c_bot_y;
                    dx * dx + dy * dy <= r_sq
                } else {
                    true
                };

                if in_corner {
                    self.set_pixel(px, py, color);
                }
            }
        }
    }

    /// Strokes a rounded rectangle with corner radius `radius` and thickness `stroke_width`.
    #[allow(clippy::too_many_arguments)]
    pub fn stroke_rounded_rect(

        &mut self,
        x: i32,
        y: i32,
        w: u32,
        h: u32,
        radius: u32,
        color: u32,
        stroke_width: u32,
    ) {
        if w == 0 || h == 0 || stroke_width == 0 {
            return;
        }
        let max_r = (w / 2).min(h / 2);
        let r = radius.min(max_r) as i32;
        let sw = stroke_width.min(w / 2).min(h / 2).max(1) as i32;

        if r <= 0 {
            self.stroke_rect(x, y, w, h, color, stroke_width);
            return;
        }

        let clip = self.current_clip();
        let x_start = x.max(clip.x);
        let y_start = y.max(clip.y);
        let x_end = (x + w as i32).min(clip.x + clip.w as i32);
        let y_end = (y + h as i32).min(clip.y + clip.h as i32);

        let outer_r_sq = r * r;
        let inner_r = (r - sw).max(0);
        let inner_r_sq = inner_r * inner_r;

        let c_top_y = y + r;
        let c_bot_y = y + h as i32 - r - 1;
        let c_left_x = x + r;
        let c_right_x = x + w as i32 - r - 1;

        for py in y_start..y_end {
            for px in x_start..x_end {
                let draw = if px < c_left_x && py < c_top_y {
                    let dx = px - c_left_x;
                    let dy = py - c_top_y;
                    let d2 = dx * dx + dy * dy;
                    d2 <= outer_r_sq && d2 >= inner_r_sq
                } else if px > c_right_x && py < c_top_y {
                    let dx = px - c_right_x;
                    let dy = py - c_top_y;
                    let d2 = dx * dx + dy * dy;
                    d2 <= outer_r_sq && d2 >= inner_r_sq
                } else if px < c_left_x && py > c_bot_y {
                    let dx = px - c_left_x;
                    let dy = py - c_bot_y;
                    let d2 = dx * dx + dy * dy;
                    d2 <= outer_r_sq && d2 >= inner_r_sq
                } else if px > c_right_x && py > c_bot_y {
                    let dx = px - c_right_x;
                    let dy = py - c_bot_y;
                    let d2 = dx * dx + dy * dy;
                    d2 <= outer_r_sq && d2 >= inner_r_sq
                } else {
                    // Straight edges
                    px < x + sw
                        || px >= x + w as i32 - sw
                        || py < y + sw
                        || py >= y + h as i32 - sw
                };

                if draw {
                    self.set_pixel(px, py, color);
                }
            }
        }
    }

    /// Renders a soft drop shadow behind a rectangular area.
    pub fn draw_shadow(
        &mut self,
        x: i32,
        y: i32,
        w: u32,
        h: u32,
        blur: u32,
        color: u32,
    ) {
        if blur == 0 || w == 0 || h == 0 {
            return;
        }

        let base_alpha = ((color >> 24) & 0xFF) as f32;
        if base_alpha <= 0.0 {
            return;
        }

        let b = blur as i32;
        let rgb = color & 0x00FFFFFF;

        let left = x - b;
        let top = y - b;
        let right = x + w as i32 + b;
        let bottom = y + h as i32 + b;

        let clip = self.current_clip();
        let x_start = left.max(clip.x);
        let y_start = top.max(clip.y);
        let x_end = right.min(clip.x + clip.w as i32);
        let y_end = bottom.min(clip.y + clip.h as i32);

        let blur_f = blur as f32;

        for py in y_start..y_end {
            for px in x_start..x_end {
                // Compute distance to closest point on the rect (x, y, w, h)
                let dx = if px < x {
                    x - px
                } else if px >= x + w as i32 {
                    px - (x + w as i32 - 1)
                } else {
                    0
                };

                let dy = if py < y {
                    y - py
                } else if py >= y + h as i32 {
                    py - (y + h as i32 - 1)
                } else {
                    0
                };

                let dist = ((dx * dx + dy * dy) as f32).sqrt();
                if dist <= blur_f {
                    let factor = (1.0 - (dist / blur_f)).powi(2);
                    let alpha = ((base_alpha * factor).clamp(0.0, 255.0)) as u32;
                    if alpha > 0 {
                        self.set_pixel(px, py, (alpha << 24) | rgb);
                    }
                }
            }
        }
    }

    /// Draws a straight line with arbitrary stroke thickness.
    pub fn draw_line(
        &mut self,
        x0: i32,
        y0: i32,
        x1: i32,
        y1: i32,
        color: u32,
        stroke_width: u32,
    ) {
        let sw = stroke_width.max(1) as i32;
        let dx = (x1 - x0).abs();
        let dy = (y1 - y0).abs();
        let sx = if x0 < x1 { 1 } else { -1 };
        let sy = if y0 < y1 { 1 } else { -1 };
        let mut err = dx - dy;

        let mut cur_x = x0;
        let mut cur_y = y0;

        let half_sw = sw / 2;

        loop {
            if sw == 1 {
                self.set_pixel(cur_x, cur_y, color);
            } else {
                for ox in -half_sw..=half_sw {
                    for oy in -half_sw..=half_sw {
                        if ox * ox + oy * oy <= half_sw * half_sw + half_sw {
                            self.set_pixel(cur_x + ox, cur_y + oy, color);
                        }
                    }
                }
            }

            if cur_x == x1 && cur_y == y1 {
                break;
            }

            let e2 = 2 * err;
            if e2 > -dy {
                err -= dy;
                cur_x += sx;
            }
            if e2 < dx {
                err += dx;
                cur_y += sy;
            }
        }
    }

    /// Draws a directed arrow from `(x0, y0)` pointing to `(x1, y1)`.
    #[allow(clippy::too_many_arguments)]
    pub fn draw_arrow(

        &mut self,
        x0: i32,
        y0: i32,
        x1: i32,
        y1: i32,
        color: u32,
        stroke_width: u32,
        arrow_size: u32,
    ) {
        let asz = arrow_size.max(4) as f32;
        let dx = (x1 - x0) as f32;
        let dy = (y1 - y0) as f32;
        let len = (dx * dx + dy * dy).sqrt();

        if len < 1.0 {
            return;
        }

        let ux = dx / len;
        let uy = dy / len;

        // Draw shaft up to arrow base
        let base_x = (x1 as f32 - ux * asz) as i32;
        let base_y = (y1 as f32 - uy * asz) as i32;
        self.draw_line(x0, y0, base_x, base_y, color, stroke_width);

        // Perpendicular unit vector
        let px = -uy;
        let py = ux;

        let wing_w = asz * 0.6;
        let w1_x = (base_x as f32 + px * wing_w).round() as i32;
        let w1_y = (base_y as f32 + py * wing_w).round() as i32;
        let w2_x = (base_x as f32 - px * wing_w).round() as i32;
        let w2_y = (base_y as f32 - py * wing_w).round() as i32;

        // Fill triangular arrowhead using barycentric check
        let min_x = x1.min(w1_x).min(w2_x);
        let max_x = x1.max(w1_x).max(w2_x);
        let min_y = y1.min(w1_y).min(w2_y);
        let max_y = y1.max(w1_y).max(w2_y);

        let v0x = w2_x - x1;
        let v0y = w2_y - y1;
        let v1x = w1_x - x1;
        let v1y = w1_y - y1;

        let denom = v0x * v1y - v1x * v0y;
        if denom == 0 {
            self.draw_line(x1, y1, w1_x, w1_y, color, stroke_width);
            self.draw_line(x1, y1, w2_x, w2_y, color, stroke_width);
            return;
        }

        for py in min_y..=max_y {
            for px in min_x..=max_x {
                let v2x = px - x1;
                let v2y = py - y1;

                let u = (v2x * v1y - v1x * v2y) as f32 / denom as f32;
                let v = (v0x * v2y - v2x * v0y) as f32 / denom as f32;

                if u >= 0.0 && v >= 0.0 && (u + v) <= 1.0 {
                    self.set_pixel(px, py, color);
                }
            }
        }
    }

    /// Renders monospace text using the embedded 8x8 font.
    pub fn draw_text(
        &mut self,
        x: i32,
        y: i32,
        text: &str,
        color: u32,
        scale: u32,
    ) {
        let s = scale.max(1);
        let mut cur_x = x;
        let mut cur_y = y;

        for ch in text.chars() {
            if ch == '\r' {
                continue;
            }
            if ch == '\n' {
                cur_x = x;
                cur_y += 8 * s as i32;
                continue;
            }

            let glyph = get_glyph(ch);
            for (row, &byte) in glyph.iter().enumerate() {
                if byte == 0 {
                    continue;
                }
                for col in 0..8i32 {
                    if (byte & (1 << (7 - col))) != 0 {
                        let px = cur_x + col * (s as i32);
                        let py = cur_y + (row as i32) * (s as i32);
                        if s == 1 {
                            self.set_pixel(px, py, color);
                        } else {
                            self.fill_rect(px, py, s, s, color);
                        }
                    }
                }
            }
            cur_x += 8 * s as i32;
        }
    }

    /// Helper that draws a complete styled card: shadow, rounded fill, and rounded border.
    #[allow(clippy::too_many_arguments)]
    pub fn draw_card(

        &mut self,
        x: i32,
        y: i32,
        w: u32,
        h: u32,
        radius: u32,
        bg: u32,
        border: u32,
        border_w: u32,
        shadow_blur: u32,
        shadow_color: u32,
    ) {
        if shadow_blur > 0 && ((shadow_color >> 24) & 0xFF) > 0 {
            self.draw_shadow(x, y + 2, w, h, shadow_blur, shadow_color);
        }
        self.fill_rounded_rect(x, y, w, h, radius, bg);
        if border_w > 0 && ((border >> 24) & 0xFF) > 0 {
            self.stroke_rounded_rect(x, y, w, h, radius, border, border_w);
        }
    }
}

/// Headless canvas for deterministic CI snapshot testing and zero-display environments.
#[derive(Debug, Clone)]
pub struct HeadlessCanvas {
    canvas: Canvas,
}

impl HeadlessCanvas {
    /// Creates a new headless canvas of given dimensions.
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            canvas: Canvas::new(width, height),
        }
    }

    /// Access the underlying `Canvas` immutably.
    #[inline]
    pub fn canvas(&self) -> &Canvas {
        &self.canvas
    }

    /// Access the underlying `Canvas` mutably.
    #[inline]
    pub fn canvas_mut(&mut self) -> &mut Canvas {
        &mut self.canvas
    }

    /// Dimensions of the headless canvas.
    #[inline]
    pub fn dimensions(&self) -> (u32, u32) {
        self.canvas.dimensions()
    }

    /// Computes a deterministic 64-bit FNV-1a checksum of the entire framebuffer.
    /// Used for deterministic snapshot verification across CI runs.
    pub fn pixel_hash(&self) -> u64 {
        const FNV_OFFSET_BASIS: u64 = 0xcbf29ce484222325;
        const FNV_PRIME: u64 = 0x100000001b3;

        let mut hash = FNV_OFFSET_BASIS;
        for &px in self.canvas.pixels() {
            for b in px.to_le_bytes() {
                hash ^= b as u64;
                hash = hash.wrapping_mul(FNV_PRIME);
            }
        }
        hash
    }

    /// Exports the framebuffer as a standard Netpbm PPM (P6 binary) image.
    pub fn render_to_ppm(&self) -> Vec<u8> {
        let (w, h) = self.canvas.dimensions();
        let header = format!("P6\n{} {}\n255\n", w, h);
        let mut out = Vec::with_capacity(header.len() + (w as usize * h as usize * 3));
        out.extend_from_slice(header.as_bytes());

        for &px in self.canvas.pixels() {
            let r = ((px >> 16) & 0xFF) as u8;
            let g = ((px >> 8) & 0xFF) as u8;
            let b = (px & 0xFF) as u8;
            out.push(r);
            out.push(g);
            out.push(b);
        }
        out
    }

    /// Exports the framebuffer as a standard uncompressed 32-bit BMP image.
    pub fn render_to_bmp(&self) -> Vec<u8> {
        let (w, h) = self.canvas.dimensions();
        let image_size = (w as usize * h as usize * 4) as u32;
        let file_size = 54 + image_size;

        let mut out = Vec::with_capacity(file_size as usize);

        // BITMAPFILEHEADER (14 bytes)
        out.extend_from_slice(b"BM");
        out.extend_from_slice(&file_size.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes()); // Reserved 1
        out.extend_from_slice(&0u16.to_le_bytes()); // Reserved 2
        out.extend_from_slice(&54u32.to_le_bytes()); // Offset to pixel data

        // BITMAPINFOHEADER (40 bytes)
        out.extend_from_slice(&40u32.to_le_bytes()); // Header size
        out.extend_from_slice(&(w as i32).to_le_bytes()); // Width
        out.extend_from_slice(&(-(h as i32)).to_le_bytes()); // Negative height for top-down DIB
        out.extend_from_slice(&1u16.to_le_bytes()); // Planes
        out.extend_from_slice(&32u16.to_le_bytes()); // Bits per pixel (32-bit BGRA)
        out.extend_from_slice(&0u32.to_le_bytes()); // Compression (BI_RGB)
        out.extend_from_slice(&image_size.to_le_bytes()); // Image size
        out.extend_from_slice(&2835u32.to_le_bytes()); // X pixels per meter (~72 DPI)
        out.extend_from_slice(&2835u32.to_le_bytes()); // Y pixels per meter
        out.extend_from_slice(&0u32.to_le_bytes()); // Colors in table
        out.extend_from_slice(&0u32.to_le_bytes()); // Important colors

        // Pixel data (BGRA order)
        for &px in self.canvas.pixels() {
            let a = ((px >> 24) & 0xFF) as u8;
            let r = ((px >> 16) & 0xFF) as u8;
            let g = ((px >> 8) & 0xFF) as u8;
            let b = (px & 0xFF) as u8;
            out.push(b);
            out.push(g);
            out.push(r);
            out.push(a);
        }

        out
    }
}
