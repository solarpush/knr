//! Adversarial empirical stress testing suite for Canvas, HeadlessCanvas,
//! Font, Scissor Clipping, and Porter-Duff Alpha Compositing.
//!
//! Milestone 2 Empirical Challenger Suite.

use knr_runtime::gui::canvas::{Canvas, HeadlessCanvas};
use knr_runtime::gui::font::{get_glyph, measure_text, FONT_8X8};
use knr_runtime::gui::style::{Color, Rect};
use std::panic::{catch_unwind, AssertUnwindSafe};

// =========================================================================
// 1. BOUNDARY CONDITIONS, MASSIVE DIMENSIONS, AND PANIC EDGE CASES
// =========================================================================

#[test]
fn test_adversarial_0x0_canvas_and_headless() {
    let mut canvas = Canvas::new(0, 0);
    assert_eq!(canvas.dimensions(), (0, 0));
    assert_eq!(canvas.pixels().len(), 0);

    // Operations on 0x0 canvas must be safe no-ops and not panic
    canvas.clear(0xFFFFFFFF);
    assert_eq!(canvas.get_pixel(0, 0), None);
    assert_eq!(canvas.get_pixel(-1, -1), None);
    canvas.set_pixel(0, 0, 0xFFFFFFFF);
    canvas.fill_rect(0, 0, 10, 10, 0xFFFFFFFF);
    canvas.stroke_rect(0, 0, 10, 10, 0xFFFFFFFF, 1);
    canvas.fill_rounded_rect(0, 0, 10, 10, 2, 0xFFFFFFFF);
    canvas.draw_line(0, 0, 5, 5, 0xFFFFFFFF, 1);
    canvas.draw_arrow(0, 0, 5, 5, 0xFFFFFFFF, 1, 4);
    canvas.draw_shadow(0, 0, 10, 10, 4, 0xFFFFFFFF);
    canvas.draw_text(0, 0, "Test", 0xFFFFFFFF, 1);

    // Headless 0x0 canvas exports
    let headless = HeadlessCanvas::new(0, 0);
    let ppm = headless.render_to_ppm();
    assert_eq!(ppm, b"P6\n0 0\n255\n");

    let bmp = headless.render_to_bmp();
    assert_eq!(&bmp[0..2], b"BM");
    assert_eq!(bmp.len(), 54); // Exact 54-byte BMP header with 0 pixel bytes
    assert_eq!(headless.pixel_hash(), 0xcbf29ce484222325); // FNV-1a offset basis
}

#[test]
fn test_adversarial_negative_and_out_of_bounds_coordinates() {
    let mut canvas = Canvas::new(50, 50);
    canvas.clear(Color::BLACK.to_u32());

    // Out of bounds accesses should return None and not panic
    assert_eq!(canvas.get_pixel(-1, 0), None);
    assert_eq!(canvas.get_pixel(0, -1), None);
    assert_eq!(canvas.get_pixel(50, 50), None);
    assert_eq!(canvas.get_pixel(100, 100), None);
    assert_eq!(canvas.get_pixel(i32::MIN, i32::MIN), None);

    // Negative coordinate writes must safely clip without modifying canvas
    canvas.set_pixel(-10, -10, Color::WHITE.to_u32());
    canvas.set_pixel(100, 100, Color::WHITE.to_u32());
    canvas.fill_rect(-20, -20, 10, 10, Color::WHITE.to_u32());
    canvas.fill_rect(60, 60, 20, 20, Color::WHITE.to_u32());
    canvas.draw_line(-50, -50, -10, -10, Color::WHITE.to_u32(), 1);
    canvas.draw_arrow(-50, -50, -10, -10, Color::WHITE.to_u32(), 1, 4);
    canvas.draw_shadow(-30, -30, 10, 10, 5, Color::WHITE.to_u32());

    // Ensure entire canvas remains pure black
    for &px in canvas.pixels() {
        assert_eq!(px, Color::BLACK.to_u32());
    }
}

/// EMPIRICAL BUG CONFIRMATION:
/// `stroke_rect` panics with arithmetic underflow when `w = 1` or `h = 1`
/// because `sw = stroke_width.min(w/2).min(h/2).max(1)` evaluates to `1`,
/// and then `h - 2 * sw` executes `1u32 - 2u32` which panics with attempt to subtract with overflow!
#[test]
fn test_reproduce_bug_stroke_rect_underflow_panic() {
    let canvas = Canvas::new(20, 20);

    // Call stroke_rect with w = 10, h = 1
    // When h = 1, h/2 = 0, sw = 1.
    // Then h - 2 * sw evaluates to 1u32 - 2u32 which panics with attempt to subtract with overflow!
    let res_h1 = catch_unwind(AssertUnwindSafe(|| {
        let mut c = canvas.clone();
        c.stroke_rect(0, 0, 10, 1, 0xFFFFFFFF, 1);
    }));
    assert!(
        res_h1.is_ok(),
        "REMEDIATED: stroke_rect does not panic with u32 underflow on h = 1"
    );

    // Call stroke_rect with w = 1, h = 1
    let res_w1_h1 = catch_unwind(AssertUnwindSafe(|| {
        let mut c = canvas.clone();
        c.stroke_rect(0, 0, 1, 1, 0xFFFFFFFF, 1);
    }));
    assert!(
        res_w1_h1.is_ok(),
        "REMEDIATED: stroke_rect does not panic with u32 underflow on w = 1, h = 1"
    );

    // Call draw_card with w = 10, h = 1, which cascades to stroke_rounded_rect -> stroke_rect
    let res_card = catch_unwind(AssertUnwindSafe(|| {
        let mut c = canvas.clone();
        c.draw_card(0, 0, 10, 1, 0, 0xFF000000, 0xFFFFFFFF, 1, 0, 0);
    }));
    assert!(
        res_card.is_ok(),
        "REMEDIATED: draw_card does not panic when h = 1 due to stroke_rect underflow"
    );
}

/// EMPIRICAL BUG CONFIRMATION:
/// `Color::parse_hex` panics on multi-byte UTF-8 strings because it slices byte indices
/// directly (`&clean[0..1]`, `&clean[0..2]`) when `clean.len()` is 3, 6, or 8 bytes.
#[test]
fn test_reproduce_bug_color_parse_hex_utf8_slice_panic() {
    // Euro symbol '€' is 3 UTF-8 bytes: [0xE2, 0x82, 0xAC].
    // clean.len() == 3, so clean[0..1] slices inside the multi-byte char and panics!
    let res_3bytes = catch_unwind(AssertUnwindSafe(|| {
        Color::parse_hex("€")
    }));
    assert!(
        res_3bytes.is_ok(),
        "REMEDIATED: Color::parse_hex does not panic on 3-byte UTF-8 string '€'"
    );
    assert_eq!(res_3bytes.unwrap(), None);

    // 6-byte UTF-8 string: "€123" is 3 + 3 = 6 bytes.
    let res_6bytes = catch_unwind(AssertUnwindSafe(|| {
        Color::parse_hex("€123")
    }));
    assert!(
        res_6bytes.is_ok(),
        "REMEDIATED: Color::parse_hex does not panic on 6-byte UTF-8 string '€123'"
    );
    assert_eq!(res_6bytes.unwrap(), None);
}

/// EMPIRICAL BUG CONFIRMATION:
/// Integer addition overflow in Rect intersection and coordinate calculations.
#[test]
fn test_reproduce_bug_rect_coordinate_overflow() {
    // When coordinates are near i32::MAX, (x + w as i32) overflows in debug mode
    let res_rect = catch_unwind(AssertUnwindSafe(|| {
        let r1 = Rect::new(i32::MAX - 5, 0, 10, 10);
        let r2 = Rect::new(0, 0, 100, 100);
        r1.intersect(&r2)
    }));
    assert!(
        res_rect.is_err(),
        "CONFIRMED BUG: Rect::intersect panics with addition overflow near i32::MAX"
    );

    let res_contains = catch_unwind(AssertUnwindSafe(|| {
        let r = Rect::new(i32::MAX - 5, 0, 10, 10);
        r.contains(i32::MAX - 2, 5)
    }));
    assert!(
        res_contains.is_err(),
        "CONFIRMED BUG: Rect::contains panics with addition overflow near i32::MAX"
    );
}

// =========================================================================
// 2. PORTER-DUFF ALPHA COMPOSITING & MATH PRECISION
// =========================================================================

/// EMPIRICAL BUG CONFIRMATION:
/// Porter-Duff math in `set_pixel` completely ignores `da` (destination alpha)
/// for color channels, assuming destination is always 100% opaque.
/// On a transparent background (da = 0), drawing semi-transparent red (alpha=128, red=255)
/// corrupts the red channel to 128 instead of preserving 255.
#[test]
fn test_empirical_alpha_compositing_transparent_dest_darkening_bug() {
    let mut canvas = Canvas::new(10, 10);
    // Initial canvas is transparent 0x00000000 (da = 0, dr = 0, dg = 0, db = 0)
    assert_eq!(canvas.get_pixel(0, 0), Some(0));

    // Paint semi-transparent pure white (alpha 128, RGB 255) on transparent background
    let semi_white = Color::rgba(255, 255, 255, 128).to_u32();
    canvas.set_pixel(0, 0, semi_white);

    let result = canvas.get_pixel(0, 0).unwrap();
    let r = (result >> 16) & 0xFF;
    let g = (result >> 8) & 0xFF;
    let b = result & 0xFF;
    let a = (result >> 24) & 0xFF;

    assert_eq!(a, 128, "Alpha channel is 128");

    // Under mathematically correct Porter-Duff Source-Over onto a fully transparent pixel:
    // Output RGB must equal source RGB (255, 255, 255).
    // But knr canvas attenuates RGB by sa / 255: (255 * 128 + 0 * 127 + 127) / 255 = 128!
    // Result is dark grey (128, 128, 128) instead of white (255, 255, 255)!
    assert_eq!(
        r, 255,
        "REMEDIATED: RGB channels preserved as 255 on transparent dest"
    );
    assert_eq!(g, 255);
    assert_eq!(b, 255);
}

#[test]
fn test_alpha_compositing_extreme_values() {
    let mut canvas = Canvas::new(10, 10);
    canvas.clear(Color::BLACK.to_u32()); // Fully opaque black

    // Alpha = 0 (fully transparent) must be a no-op
    canvas.set_pixel(0, 0, Color::rgba(255, 0, 0, 0).to_u32());
    assert_eq!(canvas.get_pixel(0, 0), Some(Color::BLACK.to_u32()));

    // Alpha = 255 (fully opaque) must completely overwrite
    canvas.set_pixel(0, 0, Color::rgba(0, 255, 0, 255).to_u32());
    assert_eq!(canvas.get_pixel(0, 0), Some(Color::rgba(0, 255, 0, 255).to_u32()));

    // Alpha = 1 (minimal positive alpha) over black
    canvas.set_pixel(1, 1, Color::rgba(255, 255, 255, 1).to_u32());
    let px_1 = canvas.get_pixel(1, 1).unwrap();
    let r_1 = (px_1 >> 16) & 0xFF;
    assert_eq!(r_1, 1, "Minimal alpha blends 1 unit of red");

    // Alpha = 254 (maximal semi-transparent alpha) over black
    canvas.set_pixel(2, 2, Color::rgba(255, 255, 255, 254).to_u32());
    let px_254 = canvas.get_pixel(2, 2).unwrap();
    let r_254 = (px_254 >> 16) & 0xFF;
    assert_eq!(r_254, 254, "Alpha 254 blends 254 units of red");
}

// =========================================================================
// 3. DEEP SCISSOR CLIPPING STACK & NESTED INTERSECTIONS
// =========================================================================

#[test]
fn test_deep_scissor_clip_stack_stress() {
    let mut canvas = Canvas::new(100, 100);

    // Push 100 progressively shrinking nested clips
    for i in 0..50 {
        canvas.push_clip(Rect::new(i, i, 100 - 2 * i as u32, 100 - 2 * i as u32));
    }

    assert_eq!(canvas.current_clip(), Rect::new(49, 49, 2, 2));

    // Fill rect on the deeply clipped canvas
    canvas.fill_rect(0, 0, 100, 100, Color::WHITE.to_u32());

    // Only the center 2x2 pixels (49, 49) and (50, 50) should be modified
    assert_eq!(canvas.get_pixel(49, 49), Some(Color::WHITE.to_u32()));
    assert_eq!(canvas.get_pixel(50, 50), Some(Color::WHITE.to_u32()));
    assert_eq!(canvas.get_pixel(48, 48), Some(0));
    assert_eq!(canvas.get_pixel(51, 51), Some(0));

    // Pop all 50 clips and verify restoration
    for _ in 0..50 {
        canvas.pop_clip();
    }
    assert_eq!(canvas.current_clip(), Rect::new(0, 0, 100, 100));

    // Excessive popping past empty stack must not panic
    for _ in 0..10 {
        canvas.pop_clip();
    }
    assert_eq!(canvas.current_clip(), Rect::new(0, 0, 100, 100));
}

#[test]
fn test_disjoint_and_nested_scissor_clips() {
    let mut canvas = Canvas::new(100, 100);

    // Push clip A: [10..50, 10..50]
    canvas.push_clip(Rect::new(10, 10, 40, 40));
    // Push disjoint clip B: [60..90, 60..90]
    canvas.push_clip(Rect::new(60, 60, 30, 30));

    // Intersection is empty
    assert_eq!(canvas.current_clip(), Rect::new(0, 0, 0, 0));

    // Drawing with empty clip must be a no-op
    canvas.fill_rect(0, 0, 100, 100, Color::WHITE.to_u32());
    for &px in canvas.pixels() {
        assert_eq!(px, 0);
    }

    // Pop disjoint clip; clip A [10..50, 10..50] is restored
    canvas.pop_clip();
    assert_eq!(canvas.current_clip(), Rect::new(10, 10, 40, 40));
}

// =========================================================================
// 4. FONT RENDERING, UNMAPPED CHARS, CONTROL CHARS & MULTI-LINE
// =========================================================================

#[test]
fn test_font_empty_string_and_measurement_anomaly() {
    let (w0, h0) = measure_text("", 1);
    // EMPIRICAL OBSERVATION: measure_text on empty string returns (0, 8) rather than (0, 0)
    assert_eq!(w0, 0);
    assert_eq!(
        h0, 8,
        "ANOMALY: measure_text on empty string reports 1 full line height (8px)"
    );

    let mut canvas = Canvas::new(20, 20);
    canvas.draw_text(0, 0, "", Color::WHITE.to_u32(), 1);
    for &px in canvas.pixels() {
        assert_eq!(px, 0);
    }
}

#[test]
fn test_font_control_characters_and_crlf_bug() {
    // Printable ASCII 32..=126
    for code in 32..=126u8 {
        let ch = code as char;
        let glyph = get_glyph(ch);
        assert_eq!(glyph, &FONT_8X8[(code - 32) as usize]);
    }

    // Characters outside 32..=126 map to '?' (ASCII 63, index 31)
    let qmark_glyph = &FONT_8X8[63 - 32];
    assert_eq!(get_glyph('\0'), qmark_glyph);
    assert_eq!(get_glyph('\t'), qmark_glyph);
    assert_eq!(get_glyph('\x1b'), qmark_glyph);
    assert_eq!(get_glyph('\r'), qmark_glyph); // Carriage return maps to '?'
    assert_eq!(get_glyph('🦀'), qmark_glyph);
    assert_eq!(get_glyph('é'), qmark_glyph);

    // EMPIRICAL BUG CONFIRMATION:
    // Windows CRLF "\r\n" draws a '?' glyph at the end of the line
    let mut canvas = Canvas::new(32, 16);
    canvas.draw_text(0, 0, "A\r\nB", Color::WHITE.to_u32(), 1);

    // Position of '\r' is x in 8..16 on line 0 (y in 0..8)
    let mut r_glyph_pixels = 0;
    for y in 0..8 {
        for x in 8..16 {
            if canvas.get_pixel(x, y) == Some(Color::WHITE.to_u32()) {
                r_glyph_pixels += 1;
            }
        }
    }
    assert_eq!(
        r_glyph_pixels, 0,
        "REMEDIATED: Carriage return '\\r' is ignored and does not render '?' glyph"
    );
}

#[test]
fn test_font_multi_line_and_consecutive_newlines() {
    let text = "\n\nHello\n\n";
    let (w, h) = measure_text(text, 1);
    assert_eq!(w, 5 * 8); // "Hello" is 5 chars = 40px
    assert_eq!(h, 5 * 8); // 5 segments from split('\n') = 40px

    let mut canvas = Canvas::new(50, 50);
    canvas.draw_text(0, 0, text, Color::WHITE.to_u32(), 1);

    // Line 0 (y 0..8) and Line 1 (y 8..16) are empty
    for y in 0..16 {
        for x in 0..50 {
            assert_eq!(canvas.get_pixel(x, y), Some(0));
        }
    }

    // Line 2 (y 16..24) contains "Hello"
    let mut hello_pixels = 0;
    for y in 16..24 {
        for x in 0..40 {
            if canvas.get_pixel(x, y) == Some(Color::WHITE.to_u32()) {
                hello_pixels += 1;
            }
        }
    }
    assert!(hello_pixels > 20, "Line 2 renders 'Hello'");
}

// =========================================================================
// 5. DETERMINISM & HASH COLLISION VULNERABILITIES
// =========================================================================

#[test]
fn test_pixel_hash_determinism_across_identical_draws() {
    let mut h1 = HeadlessCanvas::new(64, 64);
    let mut h2 = HeadlessCanvas::new(64, 64);

    let operations = |c: &mut Canvas| {
        c.clear(Color::BG_VOID.to_u32());
        c.fill_rounded_rect(5, 5, 50, 50, 6, Color::CARD_SURFACE.to_u32());
        c.stroke_rounded_rect(5, 5, 50, 50, 6, Color::CARD_BORDER.to_u32(), 2);
        c.draw_shadow(10, 10, 40, 40, 4, Color::rgba(255, 255, 255, 50).to_u32());
        c.draw_line(10, 10, 40, 40, Color::ACCENT_CYAN.to_u32(), 2);
        c.draw_arrow(40, 10, 10, 40, Color::ACCENT_EMERALD.to_u32(), 2, 6);
        c.draw_text(12, 12, "DET", Color::TEXT_PRIMARY.to_u32(), 1);
    };

    operations(h1.canvas_mut());
    operations(h2.canvas_mut());

    assert_eq!(h1.pixel_hash(), h2.pixel_hash());
    assert_eq!(h1.render_to_ppm(), h2.render_to_ppm());
    assert_eq!(h1.render_to_bmp(), h2.render_to_bmp());
}

/// EMPIRICAL VULNERABILITY:
/// `pixel_hash()` does not hash canvas dimensions. Canvases of different dimensions
/// (e.g. 10x10 vs 1x100) with identical pixel counts and pixel values produce the exact same hash!
#[test]
fn test_empirical_pixel_hash_dimension_blindness_vulnerability() {
    let h1 = HeadlessCanvas::new(10, 10); // 100 pixels
    let h2 = HeadlessCanvas::new(1, 100); // 100 pixels
    let h3 = HeadlessCanvas::new(100, 1); // 100 pixels

    // All three have 100 identical zero pixels
    assert_eq!(
        h1.pixel_hash(),
        h2.pixel_hash(),
        "CONFIRMED VULNERABILITY: pixel_hash does not include width/height"
    );
    assert_eq!(h1.pixel_hash(), h3.pixel_hash());

    // However, BMP headers correctly distinguish them
    assert_ne!(h1.render_to_bmp(), h2.render_to_bmp());
}

// =========================================================================
// 6. MASSIVE DIMENSIONS, OVERFLOW & INTEGER CAST INVERSION
// =========================================================================

/// EMPIRICAL BUG CONFIRMATION:
/// When `w >= 2^31` (e.g. `u32::MAX`), `w as i32` converts to a negative signed integer.
/// In `fill_rect`, `x_end = (x + w as i32).min(...)` becomes negative, resulting in `x_end <= x_start`
/// and causing `fill_rect` to silently drop the draw operation completely!
#[test]
fn test_reproduce_bug_massive_u32_dimension_sign_inversion() {
    let mut canvas = Canvas::new(50, 50);
    canvas.clear(Color::BLACK.to_u32());

    // Attempt to fill canvas with a rectangle of size u32::MAX x u32::MAX
    canvas.fill_rect(0, 0, u32::MAX, u32::MAX, Color::WHITE.to_u32());

    // Because u32::MAX as i32 is -1, x_end becomes -1, and nothing is drawn!
    let mut modified_pixels = 0;
    for &px in canvas.pixels() {
        if px != Color::BLACK.to_u32() {
            modified_pixels += 1;
        }
    }
    assert_eq!(
        modified_pixels, 0,
        "CONFIRMED BUG: fill_rect with u32::MAX silently draws 0 pixels due to sign inversion"
    );

    // Rect::contains also fails due to sign inversion:
    let r = Rect::new(0, 0, 3_000_000_000, 3_000_000_000);
    assert!(
        !r.contains(10, 10),
        "CONFIRMED BUG: Rect::contains returns false for points inside a massive rectangle"
    );
}

/// EMPIRICAL BUG CONFIRMATION:
/// `draw_line` panics with arithmetic overflow when delta `x1 - x0` exceeds `i32::MAX`.
#[test]
fn test_reproduce_bug_draw_line_overflow_panic() {
    let canvas = Canvas::new(50, 50);

    let res_line = catch_unwind(AssertUnwindSafe(|| {
        let mut c = canvas.clone();
        c.draw_line(-1_500_000_000, 0, 1_500_000_000, 0, Color::WHITE.to_u32(), 1);
    }));
    assert!(
        res_line.is_err(),
        "CONFIRMED BUG: draw_line panics on extreme coordinates due to i32 subtraction overflow"
    );
}

// =========================================================================
// 7. DEEP STRESS, BLENDING CONVERGENCE & FORMAT CONFORMANCE
// =========================================================================

#[test]
fn test_deep_scissor_stack_1000_levels() {
    let mut canvas = Canvas::new(100, 100);

    // Push 1000 identical clips
    for _ in 0..1000 {
        canvas.push_clip(Rect::new(10, 10, 80, 80));
    }
    assert_eq!(canvas.current_clip(), Rect::new(10, 10, 80, 80));

    // Draw inside clip
    canvas.fill_rect(0, 0, 100, 100, Color::ACCENT_CYAN.to_u32());
    assert_eq!(canvas.get_pixel(15, 15), Some(Color::ACCENT_CYAN.to_u32()));
    assert_eq!(canvas.get_pixel(5, 5), Some(0));

    // Pop all 1000 clips
    for _ in 0..1000 {
        canvas.pop_clip();
    }
    assert_eq!(canvas.current_clip(), Rect::new(0, 0, 100, 100));
}

#[test]
fn test_alpha_compositing_accumulation_precision() {
    let mut canvas = Canvas::new(1, 1);
    canvas.clear(Color::BLACK.to_u32()); // Opaque black (255, 0, 0, 0)

    // Blend semi-transparent white (alpha = 32, RGB = 255) 30 times
    let blend_col = Color::rgba(255, 255, 255, 32).to_u32();
    for _ in 0..30 {
        canvas.set_pixel(0, 0, blend_col);
    }

    let px = canvas.get_pixel(0, 0).unwrap();
    let r = (px >> 16) & 0xFF;
    let g = (px >> 8) & 0xFF;
    let b = px & 0xFF;
    let a = (px >> 24) & 0xFF;

    assert_eq!(a, 255, "Alpha remains fully saturated at 255");
    // With alpha=32 applied 30 times, color should approach white (> 250)
    assert!(r >= 250, "Red channel converged to white: {r}");
    assert!(g >= 250, "Green channel converged to white: {g}");
    assert!(b >= 250, "Blue channel converged to white: {b}");
}

#[test]
fn test_bmp_and_ppm_binary_format_conformance() {
    let mut headless = HeadlessCanvas::new(2, 2);
    // (0,0) = Red, (1,0) = Green, (0,1) = Blue, (1,1) = White
    headless.canvas_mut().set_pixel(0, 0, Color::rgb(255, 0, 0).to_u32());
    headless.canvas_mut().set_pixel(1, 0, Color::rgb(0, 255, 0).to_u32());
    headless.canvas_mut().set_pixel(0, 1, Color::rgb(0, 0, 255).to_u32());
    headless.canvas_mut().set_pixel(1, 1, Color::rgb(255, 255, 255).to_u32());

    // 1. Verify Netpbm PPM (P6) binary format
    let ppm = headless.render_to_ppm();
    assert_eq!(&ppm[0..3], b"P6\n");
    assert_eq!(&ppm[3..7], b"2 2\n");
    assert_eq!(&ppm[7..11], b"255\n");
    assert_eq!(ppm.len(), 11 + 2 * 2 * 3); // 11 header bytes + 12 pixel bytes

    // Check pixel order in PPM: RGB RGB RGB RGB
    let pixels_ppm = &ppm[11..];
    assert_eq!(&pixels_ppm[0..3], &[255, 0, 0]); // (0,0) Red
    assert_eq!(&pixels_ppm[3..6], &[0, 255, 0]); // (1,0) Green
    assert_eq!(&pixels_ppm[6..9], &[0, 0, 255]); // (0,1) Blue
    assert_eq!(&pixels_ppm[9..12], &[255, 255, 255]); // (1,1) White

    // 2. Verify Windows BMP 32-bit BGRA format
    let bmp = headless.render_to_bmp();
    assert_eq!(&bmp[0..2], b"BM");
    let file_size = u32::from_le_bytes(bmp[2..6].try_into().unwrap());
    assert_eq!(file_size, 54 + 2 * 2 * 4);
    assert_eq!(bmp.len(), file_size as usize);

    // BITMAPINFOHEADER width & height
    let w = i32::from_le_bytes(bmp[18..22].try_into().unwrap());
    let h = i32::from_le_bytes(bmp[22..26].try_into().unwrap());
    assert_eq!(w, 2);
    assert_eq!(h, -2); // Top-down DIB negative height

    let bpp = u16::from_le_bytes(bmp[28..30].try_into().unwrap());
    assert_eq!(bpp, 32); // 32 bits per pixel

    // Check pixel order in BMP: BGRA
    let pixels_bmp = &bmp[54..];
    assert_eq!(&pixels_bmp[0..4], &[0, 0, 255, 255]); // (0,0) Red: B=0, G=0, R=255, A=255
    assert_eq!(&pixels_bmp[4..8], &[0, 255, 0, 255]); // (1,0) Green: B=0, G=255, R=0, A=255
    assert_eq!(&pixels_bmp[8..12], &[255, 0, 0, 255]); // (0,1) Blue: B=255, G=0, R=0, A=255
    assert_eq!(&pixels_bmp[12..16], &[255, 255, 255, 255]); // (1,1) White
}

#[test]
fn test_font_stress_long_lines_and_scaling() {
    let long_text = "A".repeat(1000);
    let (w, h) = measure_text(&long_text, 1);
    assert_eq!(w, 1000 * 8);
    assert_eq!(h, 8);

    let (w4, h4) = measure_text(&long_text, 4);
    assert_eq!(w4, 1000 * 32);
    assert_eq!(h4, 32);

    // Draw on small canvas, must clip cleanly without crashing
    let mut canvas = Canvas::new(50, 20);
    canvas.draw_text(0, 0, &long_text, Color::WHITE.to_u32(), 4);
    assert_ne!(canvas.get_pixel(0, 0), None);
}
