//! Tier 5 Adversarial Hardening Suite for knr-runtime GUI Engine
//!
//! Scope:
//! 1. Canvas Porter-Duff math with full alpha / color channel matrix
//! 2. Scissor clipping stack with nested, disjoint, and empty intersections
//! 3. HeadlessCanvas 64-bit FNV-1a checksum determinism and single-bit sensitivity
//! 4. Font ASCII boundaries, non-printable fallback to '?', and multi-line measure
//! 5. ExecutionGraph cyclic layout termination, trace update edges, and SVG rendering
//! 6. Native desktop window configuration and headless fallback presentation

use knr_runtime::gui::canvas::{Canvas, HeadlessCanvas};
use knr_runtime::gui::font::{get_glyph, measure_text, FONT_8X8};
use knr_runtime::gui::graph::{EdgeType, ExecutionGraph, GraphEdge, GraphNode, NodeExecutionState, PipelineStage, TraceRecord};
use knr_runtime::gui::style::Rect;
use knr_runtime::gui::window::{launch_desktop_window, WindowBackend, WindowConfig};

// =========================================================================
// 1. Canvas Porter-Duff Alpha Compositing & Boundary Math
// =========================================================================

#[test]
fn test_adv_canvas_porter_duff_alpha_math() {
    let mut canvas = Canvas::new(10, 10);

    // Initial canvas pixels are 0x00000000 (transparent black)
    assert_eq!(canvas.get_pixel(0, 0), Some(0));

    // Case 1: Fully transparent source (sa = 0) must be a no-op
    canvas.set_pixel(0, 0, 0x00FF0000); // 0% red
    assert_eq!(canvas.get_pixel(0, 0), Some(0));

    // Case 2: Fully opaque source (sa = 255) must completely overwrite
    canvas.set_pixel(0, 0, 0xFF0000FF); // 100% blue
    assert_eq!(canvas.get_pixel(0, 0), Some(0xFF0000FF));

    // Case 3: Blend 50% red (sa = 128, 0x80FF0000) over 100% blue (0xFF0000FF)
    // Formula:
    // sa = 128, inv_sa = 127
    // da = 255, da_inv_sa = (255 * 127 + 127) / 255 = 127
    // out_a = 128 + 127 = 255
    // out_r = ((255 * 128 + 0 * 127) + 127) / 255 = (32640 + 127) / 255 = 128
    // out_b = ((0 * 128 + 255 * 127) + 127) / 255 = (32385 + 127) / 255 = 127
    canvas.set_pixel(0, 0, 0x80FF0000);
    let px = canvas.get_pixel(0, 0).unwrap();
    let a = (px >> 24) & 0xFF;
    let r = (px >> 16) & 0xFF;
    let g = (px >> 8) & 0xFF;
    let b = px & 0xFF;

    assert_eq!(a, 255, "Alpha must remain opaque when blending over opaque dst");
    assert!((126..=130).contains(&r), "Red channel should be ~128, got {r}");
    assert_eq!(g, 0, "Green channel must be 0");
    assert!((125..=129).contains(&b), "Blue channel should be ~127, got {b}");

    // Case 4: Semi-transparent source over transparent dst (da = 0)
    canvas.clear(0);
    canvas.set_pixel(5, 5, 0x8000FF00); // 50% green over 0% dst
    let px4 = canvas.get_pixel(5, 5).unwrap();
    let a4 = (px4 >> 24) & 0xFF;
    let g4 = (px4 >> 8) & 0xFF;
    assert_eq!(a4, 128, "Alpha should be source alpha (128)");
    assert_eq!(g4, 255, "Color channel should be preserved (255)");
}

#[test]
fn test_adv_canvas_zero_and_extreme_shapes() {
    let mut canvas = Canvas::new(50, 50);

    // Zero width or height shapes must not panic and not write pixels
    canvas.fill_rect(5, 5, 0, 10, 0xFFFFFFFF);
    canvas.fill_rect(5, 5, 10, 0, 0xFFFFFFFF);
    assert_eq!(canvas.get_pixel(5, 5), Some(0));

    canvas.stroke_rect(5, 5, 0, 10, 0xFFFFFFFF, 1);
    canvas.stroke_rect(5, 5, 10, 0, 0xFFFFFFFF, 1);
    canvas.stroke_rect(5, 5, 10, 10, 0xFFFFFFFF, 0); // stroke_width 0
    assert_eq!(canvas.get_pixel(5, 5), Some(0));

    canvas.fill_rounded_rect(5, 5, 0, 10, 5, 0xFFFFFFFF);
    canvas.fill_rounded_rect(5, 5, 10, 0, 5, 0xFFFFFFFF);
    assert_eq!(canvas.get_pixel(5, 5), Some(0));

    canvas.stroke_rounded_rect(5, 5, 0, 10, 5, 0xFFFFFFFF, 1);
    canvas.stroke_rounded_rect(5, 5, 10, 10, 5, 0xFFFFFFFF, 0);
    assert_eq!(canvas.get_pixel(5, 5), Some(0));

    // Zero-length line and arrow
    canvas.draw_line(10, 10, 10, 10, 0xFFFFFFFF, 1);
    assert_eq!(canvas.get_pixel(10, 10), Some(0xFFFFFFFF));

    canvas.draw_arrow(20, 20, 20, 20, 0xFFFFFFFF, 1, 4); // len < 1.0 safe return

    // Shadow with blur 0
    canvas.draw_shadow(10, 10, 20, 20, 0, 0xFF000000);
}

// =========================================================================
// 2. Scissor Clipping Stack Nested, Disjoint, and Empty Intersections
// =========================================================================

#[test]
fn test_adv_canvas_clipping_stack_invariants() {
    let mut canvas = Canvas::new(100, 100);

    // Default clip is entire canvas (0, 0, 100, 100)
    assert_eq!(canvas.current_clip(), Rect::new(0, 0, 100, 100));

    // Push clip 1: (10, 10, 80, 80)
    canvas.push_clip(Rect::new(10, 10, 80, 80));
    assert_eq!(canvas.current_clip(), Rect::new(10, 10, 80, 80));

    // Push clip 2: (50, 50, 40, 40) -> intersection is (50, 50, 40, 40)
    canvas.push_clip(Rect::new(50, 50, 40, 40));
    assert_eq!(canvas.current_clip(), Rect::new(50, 50, 40, 40));

    // Push clip 3: Disjoint from active clip (0, 0, 20, 20) -> intersection is (0, 0, 0, 0)
    canvas.push_clip(Rect::new(0, 0, 20, 20));
    assert_eq!(canvas.current_clip(), Rect::new(0, 0, 0, 0));

    // Under disjoint clip, drawing anywhere must be a no-op
    canvas.fill_rect(0, 0, 100, 100, 0xFFFFFFFF);
    assert!(canvas.pixels().iter().all(|&p| p == 0));

    // Pop clip 3 -> restores (50, 50, 40, 40)
    canvas.pop_clip();
    assert_eq!(canvas.current_clip(), Rect::new(50, 50, 40, 40));

    // Draw full canvas -> only (50..90, 50..90) should be filled
    canvas.fill_rect(0, 0, 100, 100, 0xFFFFFFFF);
    assert_eq!(canvas.get_pixel(49, 49), Some(0));
    assert_eq!(canvas.get_pixel(50, 50), Some(0xFFFFFFFF));
    assert_eq!(canvas.get_pixel(89, 89), Some(0xFFFFFFFF));
    assert_eq!(canvas.get_pixel(90, 90), Some(0));

    // Pop clip 2 -> restores (10, 10, 80, 80)
    canvas.pop_clip();
    assert_eq!(canvas.current_clip(), Rect::new(10, 10, 80, 80));

    // Pop clip 1 -> restores default (0, 0, 100, 100)
    canvas.pop_clip();
    assert_eq!(canvas.current_clip(), Rect::new(0, 0, 100, 100));

    // Pop when stack is empty -> safe no-op
    canvas.pop_clip();
    assert_eq!(canvas.current_clip(), Rect::new(0, 0, 100, 100));
}

// =========================================================================
// 3. HeadlessCanvas FNV-1a Hash Determinism & Single-Bit Sensitivity
// =========================================================================

#[test]
fn test_adv_headless_canvas_fnv1a_determinism_and_sensitivity() {
    let mut h1 = HeadlessCanvas::new(200, 100);
    let mut h2 = HeadlessCanvas::new(200, 100);

    // Initial transparent framebuffers must have identical hashes
    let hash1 = h1.pixel_hash();
    let hash2 = h2.pixel_hash();
    assert_eq!(hash1, hash2, "Identical framebuffers must produce identical FNV-1a hashes");

    // Perform identical draw commands on both
    h1.canvas_mut().fill_rect(10, 10, 50, 50, 0xFF123456);
    h1.canvas_mut().draw_text(20, 20, "Hello", 0xFFFFFFFF, 1);

    h2.canvas_mut().fill_rect(10, 10, 50, 50, 0xFF123456);
    h2.canvas_mut().draw_text(20, 20, "Hello", 0xFFFFFFFF, 1);

    assert_eq!(h1.pixel_hash(), h2.pixel_hash(), "Deterministic rendering must yield identical hashes");

    // Single-bit modification on h2: flip LSB of pixel at (199, 99)
    let last_idx = 200 * 100 - 1;
    h2.canvas_mut().pixels_mut()[last_idx] ^= 1;

    let hash_modified = h2.pixel_hash();
    assert_ne!(h1.pixel_hash(), hash_modified, "Single-bit flip MUST invalidate FNV-1a checksum");

    // Netpbm PPM binary export verification
    let ppm = h1.render_to_ppm();
    let header_str = "P6\n200 100\n255\n";
    assert_eq!(&ppm[..header_str.len()], header_str.as_bytes());
    assert_eq!(ppm.len(), header_str.len() + 200 * 100 * 3);

    // BMP 32-bit uncompressed export verification
    let bmp = h1.render_to_bmp();
    assert_eq!(&bmp[0..2], b"BM");
    assert_eq!(bmp.len(), 54 + 200 * 100 * 4);
}

// =========================================================================
// 4. Font ASCII Boundaries & Multi-line Text Measurement
// =========================================================================

#[test]
fn test_adv_font_boundaries_and_unicode_safety() {
    // Printable ASCII boundary chars
    let glyph_space = get_glyph(' ');
    let glyph_tilde = get_glyph('~');
    let glyph_question = get_glyph('?');

    assert_eq!(glyph_space, &FONT_8X8[0]);
    assert_eq!(glyph_tilde, &FONT_8X8[126 - 32]);

    // Non-printable, control, and Unicode chars must safely return '?' glyph without panicking
    let non_ascii_chars = ['\0', '\n', '\r', '\t', 31 as char, 127 as char, '🦀', '€', 'Ω', '中'];
    for ch in non_ascii_chars {
        let g = get_glyph(ch);
        assert_eq!(g, glyph_question, "Character {:?} (code {}) must return '?' glyph", ch, ch as u32);
    }

    // Measure text
    assert_eq!(measure_text("", 1), (0, 8));
    assert_eq!(measure_text("A", 1), (8, 8));
    assert_eq!(measure_text("A", 2), (16, 16));
    assert_eq!(measure_text("ABC\nDE", 1), (24, 16));
    assert_eq!(measure_text("Line1\r\nLine2\r\n", 1), (40, 24));
}

// =========================================================================
// 5. ExecutionGraph Cyclic Layout Termination & Trace Stepping
// =========================================================================

#[test]
fn test_adv_graph_cyclic_layout_and_trace_stepping() {
    let mut graph = ExecutionGraph::new("Cyclic CFG Test");

    // Construct a cyclic CFG: bb0 -> bb1 -> bb2 -> bb1 (loop)
    let n0 = GraphNode::new("bb0", "Entry", PipelineStage::BasicBlock);
    let n1 = GraphNode::new("bb1", "Loop Header", PipelineStage::BasicBlock);
    let n2 = GraphNode::new("bb2", "Loop Body", PipelineStage::BasicBlock);
    let n3 = GraphNode::new("bb3", "Exit", PipelineStage::BasicBlock);

    graph.add_node(n0);
    graph.add_node(n1);
    graph.add_node(n2);
    graph.add_node(n3);

    graph.add_edge(GraphEdge::new("bb0", "bb1", EdgeType::ControlFlow));
    graph.add_edge(GraphEdge::new("bb1", "bb2", EdgeType::BranchTrue));
    graph.add_edge(GraphEdge::new("bb2", "bb1", EdgeType::ControlFlow)); // Back-edge cycle
    graph.add_edge(GraphEdge::new("bb1", "bb3", EdgeType::BranchFalse));

    // Automated layout must terminate immediately even with back-edges (Kahn's algorithm robustness)
    graph.compute_layout(900, 600);

    for node in &graph.nodes {
        assert!(node.x >= 20.0, "Node x must be within canvas margins");
        assert!(node.y >= 70.0, "Node y must be within canvas margins");
        assert_eq!(node.width, 200.0);
        assert_eq!(node.height, 80.0);
    }

    // Dynamic trace stepping: step through loop 3 times
    let trace = vec![
        TraceRecord::new(1, 1, 0, "const_i64", Some("%0".to_string()), Some(serde_json::json!(0))),
        TraceRecord::new(2, 2, 0, "br", None, None),
        TraceRecord::new(3, 3, 1, "cmp_lt", Some("%1".to_string()), Some(serde_json::json!(true))),
        TraceRecord::new(4, 4, 1, "br_if", None, Some(serde_json::json!(true))),
        TraceRecord::new(5, 5, 2, "add", Some("%2".to_string()), Some(serde_json::json!(1))),
        TraceRecord::new(6, 6, 2, "br", None, None),
        TraceRecord::new(7, 7, 1, "cmp_lt", Some("%3".to_string()), Some(serde_json::json!(false))),
        TraceRecord::new(8, 8, 1, "br_if", None, Some(serde_json::json!(false))),
        TraceRecord::new(9, 9, 3, "ret", None, Some(serde_json::json!(1))),
    ];

    // Step 0: bb0 executing
    graph.update_from_trace(&trace, 0);
    assert_eq!(graph.get_node("bb0").unwrap().state, NodeExecutionState::Executing);
    assert_eq!(graph.get_node("bb1").unwrap().state, NodeExecutionState::Pending);

    // Step 5: bb2 executing, bb0 and bb1 completed
    graph.update_from_trace(&trace, 4);
    assert_eq!(graph.get_node("bb0").unwrap().state, NodeExecutionState::Completed);
    assert_eq!(graph.get_node("bb1").unwrap().state, NodeExecutionState::Completed);
    assert_eq!(graph.get_node("bb2").unwrap().state, NodeExecutionState::Executing);
    assert_eq!(graph.get_node("bb3").unwrap().state, NodeExecutionState::Pending);

    // Final step: bb3 executing
    graph.update_from_trace(&trace, 8);
    assert_eq!(graph.get_node("bb3").unwrap().state, NodeExecutionState::Executing);

    // Test dual-target SVG export
    let svg = graph.render_to_svg();
    assert!(svg.starts_with("<svg"), "Must emit valid SVG root tag");
    assert!(svg.ends_with("</svg>\n"), "Must terminate with </svg>");
    assert!(svg.contains("Cyclic CFG Test"), "SVG must include graph title");
    assert!(svg.contains("bb0"), "SVG must include bb0 node");
    assert!(svg.contains("bb1"), "SVG must include bb1 node");
}

// =========================================================================
// 6. Desktop Window Configuration and Headless Presentation Fallback
// =========================================================================

#[test]
fn test_adv_window_config_and_headless_fallback() {
    let config = WindowConfig::new("Headless Test", 640, 480)
        .with_headless(true)
        .with_max_frames(3);

    let mut frames_seen = 0;
    let res = launch_desktop_window(config, |canvas, frame_idx| {
        assert_eq!(canvas.dimensions(), (640, 480));
        frames_seen += 1;
        assert_eq!(frame_idx, frames_seen - 1);
        canvas.clear(0xFF000000 | (frame_idx as u32 * 0x10));
        true
    }).expect("launch desktop window in headless mode");

    assert_eq!(res.backend, WindowBackend::Headless);
    assert_eq!(res.frames_rendered, 3);
    assert_eq!(frames_seen, 3);
    assert_ne!(res.final_pixel_hash, 0);

    // Test early termination when render closure returns false
    let config2 = WindowConfig::new("Early Exit Test", 320, 240)
        .with_headless(true)
        .with_max_frames(10);

    let mut count2 = 0;
    let res2 = launch_desktop_window(config2, |_canvas, _frame_idx| {
        count2 += 1;
        false // Exit immediately after 1 frame
    }).expect("launch desktop window");

    assert_eq!(res2.frames_rendered, 1);
    assert_eq!(count2, 1);
}
