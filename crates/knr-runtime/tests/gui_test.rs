//! Comprehensive unit and integration test suite for knr native GUI engine,
//! 2D software canvas rasterizer, bitmap font, and structured execution graph.

use knr_core::Effect;
use knr_runtime::gui::canvas::{Canvas, HeadlessCanvas};
use knr_runtime::gui::font::{get_glyph, measure_text, FONT_8X8};
use knr_runtime::gui::graph::{
    EdgeType, ExecutionGraph, GraphEdge, GraphNode, NodeExecutionState, PipelineStage,
    TraceRecord,
};
use knr_runtime::gui::style::{Color, Rect};
use knr_runtime::gui::window::{launch_desktop_window, WindowBackend, WindowConfig};

use knr_runtime::platform::{HostPlatform, Value};
use knr_runtime::ui::UiPlatform;

#[test]
fn test_canvas_buffer_and_clear() {
    let mut canvas = Canvas::new(100, 80);
    assert_eq!(canvas.dimensions(), (100, 80));
    assert_eq!(canvas.pixels().len(), 8000);

    // Initial state is transparent 0
    assert_eq!(canvas.pixels()[0], 0);

    // Clear with opaque test color
    let test_col = Color::rgb(30, 41, 59).to_u32(); // 0xFF1E293B
    canvas.clear(test_col);

    assert_eq!(canvas.get_pixel(0, 0), Some(test_col));
    assert_eq!(canvas.get_pixel(99, 79), Some(test_col));
    assert_eq!(canvas.get_pixel(50, 40), Some(test_col));
    assert_eq!(canvas.get_pixel(-1, 0), None);
    assert_eq!(canvas.get_pixel(100, 80), None);
}

#[test]
fn test_canvas_alpha_blending() {
    let mut canvas = Canvas::new(10, 10);
    // Background: solid red (0xFFFF0000)
    canvas.clear(Color::rgb(255, 0, 0).to_u32());

    // Paint semi-transparent blue (alpha = 128, blue = 255)
    let semi_blue = Color::rgba(0, 0, 255, 128).to_u32();
    canvas.set_pixel(5, 5, semi_blue);

    let blended = canvas.get_pixel(5, 5).unwrap();
    let r = (blended >> 16) & 0xFF;
    let b = blended & 0xFF;
    let a = (blended >> 24) & 0xFF;

    // Both red and blue should be mixed (~127..128)
    assert_eq!(a, 255);
    assert!((120..=135).contains(&r), "Red channel blended: {r}");
    assert!((120..=135).contains(&b), "Blue channel blended: {b}");

    // Fully transparent pixel does not mutate destination
    canvas.set_pixel(0, 0, Color::rgba(0, 255, 0, 0).to_u32());
    assert_eq!(canvas.get_pixel(0, 0), Some(Color::rgb(255, 0, 0).to_u32()));
}

#[test]
fn test_canvas_fill_and_stroke_rect() {
    let mut canvas = Canvas::new(50, 50);
    canvas.clear(Color::BLACK.to_u32());

    let cyan = Color::ACCENT_CYAN.to_u32();
    // Fill 20x20 rectangle at (10, 10)
    canvas.fill_rect(10, 10, 20, 20, cyan);

    // Inside rect
    assert_eq!(canvas.get_pixel(15, 15), Some(cyan));
    assert_eq!(canvas.get_pixel(10, 10), Some(cyan));
    assert_eq!(canvas.get_pixel(29, 29), Some(cyan));

    // Outside rect
    assert_eq!(canvas.get_pixel(9, 9), Some(Color::BLACK.to_u32()));
    assert_eq!(canvas.get_pixel(30, 30), Some(Color::BLACK.to_u32()));

    // Stroke rect border
    let mut stroke_canvas = Canvas::new(50, 50);
    stroke_canvas.clear(Color::BLACK.to_u32());
    stroke_canvas.stroke_rect(10, 10, 20, 20, cyan, 2);

    // Border pixels are cyan
    assert_eq!(stroke_canvas.get_pixel(10, 10), Some(cyan));
    assert_eq!(stroke_canvas.get_pixel(11, 11), Some(cyan));
    // Interior pixel is untouched black
    assert_eq!(
        stroke_canvas.get_pixel(20, 20),
        Some(Color::BLACK.to_u32())
    );
}

#[test]
fn test_canvas_rounded_rect() {
    let mut canvas = Canvas::new(60, 60);
    canvas.clear(Color::BLACK.to_u32());

    let emerald = Color::ACCENT_EMERALD.to_u32();
    canvas.fill_rounded_rect(10, 10, 40, 40, 8, emerald);

    // Center is filled
    assert_eq!(canvas.get_pixel(30, 30), Some(emerald));

    // Corner (10, 10) is outside the rounded radius of 8 and should remain black
    assert_eq!(canvas.get_pixel(10, 10), Some(Color::BLACK.to_u32()));
    assert_eq!(canvas.get_pixel(49, 10), Some(Color::BLACK.to_u32()));
    assert_eq!(canvas.get_pixel(10, 49), Some(Color::BLACK.to_u32()));
    assert_eq!(canvas.get_pixel(49, 49), Some(Color::BLACK.to_u32()));
}

#[test]
fn test_canvas_shadow_rendering() {
    let mut canvas = Canvas::new(60, 60);
    canvas.clear(Color::BLACK.to_u32());

    let shadow_col = Color::rgba(255, 255, 255, 200).to_u32();
    canvas.draw_shadow(20, 20, 20, 20, 8, shadow_col);

    // Pixels near the edge have higher intensity than pixels farther away
    let near = (canvas.get_pixel(19, 25).unwrap() >> 16) & 0xFF;
    let mid = (canvas.get_pixel(16, 25).unwrap() >> 16) & 0xFF;
    let far = (canvas.get_pixel(13, 25).unwrap() >> 16) & 0xFF;
    let outside = (canvas.get_pixel(5, 25).unwrap() >> 16) & 0xFF;

    assert!(near > mid, "Near ({near}) should be > mid ({mid})");
    assert!(mid > far, "Mid ({mid}) should be > far ({far})");
    assert_eq!(outside, 0, "Outside blur radius should be 0");
}

#[test]
fn test_canvas_line_and_arrow() {
    let mut canvas = Canvas::new(50, 50);
    canvas.clear(Color::BLACK.to_u32());

    let col = Color::WHITE.to_u32();
    canvas.draw_line(5, 5, 45, 5, col, 1);

    // Horizontal line
    for x in 5..=45 {
        assert_eq!(canvas.get_pixel(x, 5), Some(col));
    }

    // Directed arrow
    let mut arrow_canvas = Canvas::new(50, 50);
    arrow_canvas.clear(Color::BLACK.to_u32());
    arrow_canvas.draw_arrow(5, 25, 40, 25, col, 2, 8);

    // Shaft pixel is drawn
    assert_eq!(arrow_canvas.get_pixel(15, 25), Some(col));
    // Arrow tip pixel is drawn
    assert_eq!(arrow_canvas.get_pixel(40, 25), Some(col));
}

#[test]
fn test_canvas_clipping_stack() {
    let mut canvas = Canvas::new(100, 100);
    canvas.clear(Color::BLACK.to_u32());

    // Push scissor clip [20..50, 20..50]
    canvas.push_clip(Rect::new(20, 20, 30, 30));
    assert_eq!(canvas.current_clip(), Rect::new(20, 20, 30, 30));

    // Attempt to fill entire 0..100 canvas with cyan
    let cyan = Color::ACCENT_CYAN.to_u32();
    canvas.fill_rect(0, 0, 100, 100, cyan);

    // Pixels inside clip are cyan
    assert_eq!(canvas.get_pixel(25, 25), Some(cyan));
    assert_eq!(canvas.get_pixel(49, 49), Some(cyan));

    // Pixels outside clip are untouched black
    assert_eq!(canvas.get_pixel(10, 10), Some(Color::BLACK.to_u32()));
    assert_eq!(canvas.get_pixel(60, 60), Some(Color::BLACK.to_u32()));

    canvas.pop_clip();
    assert_eq!(canvas.current_clip(), Rect::new(0, 0, 100, 100));
}

#[test]
fn test_font_glyph_and_measurement() {
    // Check ASCII 32 to 126 in font table
    assert_eq!(FONT_8X8.len(), 95);

    // Character 'A' is ASCII 65 -> index 33
    let glyph_a = get_glyph('A');
    assert_eq!(*glyph_a, FONT_8X8[65 - 32]);

    // Test text measurement
    let (w1, h1) = measure_text("Hello", 1);
    assert_eq!(w1, 40); // 5 * 8
    assert_eq!(h1, 8);

    let (w2, h2) = measure_text("Line1\nLineTwo", 2);
    assert_eq!(w2, 7 * 16); // max chars = 7 ("LineTwo")
    assert_eq!(h2, 2 * 16); // 2 lines

    // Draw text on canvas and ensure pixels are set
    let mut canvas = Canvas::new(50, 20);
    canvas.clear(Color::BLACK.to_u32());
    canvas.draw_text(0, 0, "A", Color::WHITE.to_u32(), 1);

    let mut non_black_count = 0;
    for y in 0..8 {
        for x in 0..8 {
            if canvas.get_pixel(x, y) == Some(Color::WHITE.to_u32()) {
                non_black_count += 1;
            }
        }
    }
    assert!(non_black_count > 10, "Letter 'A' rendered bitmap glyph");
}

#[test]
fn test_headless_canvas_snapshot_and_deterministic_hash() {
    let mut h1 = HeadlessCanvas::new(80, 60);
    let mut h2 = HeadlessCanvas::new(80, 60);

    // Draw identical contents
    h1.canvas_mut().clear(Color::CARD_SURFACE.to_u32());
    h1.canvas_mut()
        .fill_rounded_rect(10, 10, 40, 30, 4, Color::ACCENT_CYAN.to_u32());
    h1.canvas_mut()
        .draw_text(12, 16, "KNR", Color::TEXT_PRIMARY.to_u32(), 1);

    h2.canvas_mut().clear(Color::CARD_SURFACE.to_u32());
    h2.canvas_mut()
        .fill_rounded_rect(10, 10, 40, 30, 4, Color::ACCENT_CYAN.to_u32());
    h2.canvas_mut()
        .draw_text(12, 16, "KNR", Color::TEXT_PRIMARY.to_u32(), 1);

    // Deterministic FNV-1a hash must match exactly
    assert_eq!(h1.pixel_hash(), h2.pixel_hash());
    assert_ne!(h1.pixel_hash(), 0);

    // Mutating h2 alters hash
    h2.canvas_mut().set_pixel(0, 0, Color::WHITE.to_u32());
    assert_ne!(h1.pixel_hash(), h2.pixel_hash());

    // Test Netpbm PPM export
    let ppm = h1.render_to_ppm();
    let header_bytes = b"P6\n80 60\n255\n";
    assert!(ppm.starts_with(header_bytes));
    assert_eq!(ppm.len(), header_bytes.len() + (80 * 60 * 3));


    // Test BMP export
    let bmp = h1.render_to_bmp();
    assert_eq!(&bmp[0..2], b"BM");
    assert_eq!(bmp.len(), 54 + (80 * 60 * 4));
}

#[test]
fn test_execution_graph_dag_layout_and_overlap_avoidance() {
    let mut graph = ExecutionGraph::new("Compiler Pipeline");

    graph.add_node(GraphNode::new("ast", "AST Resolution", PipelineStage::FrontendAst));
    graph.add_node(GraphNode::new("verify", "Static Verify", PipelineStage::VerifyPass));
    graph.add_node(GraphNode::new("bb0", "Entry CFG bb0", PipelineStage::BasicBlock));
    graph.add_node(GraphNode::new("bb1", "Reset bb1", PipelineStage::BasicBlock));
    graph.add_node(GraphNode::new("bb2", "Step bb2", PipelineStage::BasicBlock));
    graph.add_node(GraphNode::new("call", "apply_step", PipelineStage::InterpOracle));

    graph.add_edge(GraphEdge::new("ast", "verify", EdgeType::ControlFlow));
    graph.add_edge(GraphEdge::new("verify", "bb0", EdgeType::ControlFlow));
    graph.add_edge(GraphEdge::new("bb0", "bb1", EdgeType::BranchTrue).with_label("op == 0"));
    graph.add_edge(GraphEdge::new("bb0", "bb2", EdgeType::BranchFalse).with_label("op != 0"));
    graph.add_edge(GraphEdge::new("bb2", "call", EdgeType::ControlFlow));

    // Compute topological DAG layout
    graph.compute_layout(900, 600);

    let ast = graph.get_node("ast").unwrap();
    let verify = graph.get_node("verify").unwrap();
    let bb0 = graph.get_node("bb0").unwrap();
    let bb1 = graph.get_node("bb1").unwrap();
    let bb2 = graph.get_node("bb2").unwrap();
    let call = graph.get_node("call").unwrap();

    // Verify top-down topological ordering
    assert!(ast.y < verify.y, "AST layer must be above verify");
    assert!(verify.y < bb0.y, "Verify layer must be above bb0");
    assert!(bb0.y < bb1.y, "bb0 layer must be above branch targets");
    assert!(bb0.y < bb2.y, "bb0 layer must be above branch targets");
    assert!(bb2.y < call.y, "bb2 layer must be above call apply_step");

    // Branch nodes bb1 and bb2 are on the same vertical rank but horizontally separated
    assert_eq!(bb1.y, bb2.y, "bb1 and bb2 must share the same horizontal row");
    assert!(
        (bb1.x - bb2.x).abs() >= bb1.width,
        "bb1 and bb2 must not overlap horizontally"
    );
}

#[test]
fn test_execution_graph_trace_stepping() {
    let mut graph = ExecutionGraph::new("Trace Execution");

    graph.add_node(GraphNode::new("ast", "AST", PipelineStage::FrontendAst));
    graph.add_node(GraphNode::new("verify", "Verify", PipelineStage::VerifyPass));
    graph.add_node(GraphNode::new("bb0", "bb0", PipelineStage::BasicBlock));
    graph.add_node(GraphNode::new("bb1", "bb1", PipelineStage::BasicBlock));
    graph.add_node(GraphNode::new("bb2", "bb2", PipelineStage::BasicBlock));
    graph.add_node(GraphNode::new("call", "call", PipelineStage::InterpOracle));

    graph.add_edge(GraphEdge::new("bb0", "bb2", EdgeType::BranchFalse));
    graph.add_edge(GraphEdge::new("bb2", "call", EdgeType::ControlFlow));

    let trace = vec![
        TraceRecord::new(0, 1, 0, "const_i64", Some("%2".into()), None),
        TraceRecord::new(1, 2, 0, "br_if", None, None),
        TraceRecord::new(2, 3, 2, "mul", Some("%6".into()), None),
        TraceRecord::new(3, 4, 2, "call apply_step", Some("%8".into()), None),
    ];

    // Step 0: In bb0
    graph.update_from_trace(&trace, 0);
    assert_eq!(graph.get_node("bb0").unwrap().state, NodeExecutionState::Executing);
    assert_eq!(graph.get_node("bb2").unwrap().state, NodeExecutionState::Pending);

    // Step 2: In bb2
    graph.update_from_trace(&trace, 2);
    assert_eq!(graph.get_node("bb0").unwrap().state, NodeExecutionState::Completed);
    assert_eq!(graph.get_node("bb2").unwrap().state, NodeExecutionState::Executing);
    assert_eq!(graph.get_node("call").unwrap().state, NodeExecutionState::Pending);

    // Step 3: At call apply_step
    graph.update_from_trace(&trace, 3);
    assert_eq!(graph.get_node("bb2").unwrap().state, NodeExecutionState::Executing);
    assert_eq!(graph.get_node("call").unwrap().state, NodeExecutionState::Executing);


    // Edge bb0 -> bb2 was traversed
    let edge_bb0_bb2 = graph
        .edges
        .iter()
        .find(|e| e.from == "bb0" && e.to == "bb2")
        .unwrap();
    assert!(edge_bb0_bb2.is_active);
}

#[test]
fn test_execution_graph_dual_rendering_canvas_and_svg() {
    let mut graph = ExecutionGraph::new("Pipeline Demo");
    graph.add_node(
        GraphNode::new("ast", "AST Parser", PipelineStage::FrontendAst)
            .with_code("[vdom.ssa] -> Leaf"),
    );
    graph.add_node(GraphNode::new("verify", "knr-verify", PipelineStage::VerifyPass));
    graph.add_edge(GraphEdge::new("ast", "verify", EdgeType::ControlFlow));
    graph.compute_layout(900, 600);

    // 1. Render to software Canvas
    let mut canvas = Canvas::new(900, 600);
    graph.render_to_canvas(&mut canvas);
    let pixel = canvas.get_pixel(24, 24).unwrap();
    assert_ne!(pixel, 0, "Title text pixel rendered on canvas");

    // 2. Render to semantic SVG
    let svg = graph.render_to_svg();

    // Verify valid XML/SVG structure
    assert!(svg.starts_with("<svg xmlns=\"http://www.w3.org/2000/svg\""));
    assert!(svg.contains("<defs>"));
    assert!(svg.contains("<marker id=\"arrow-default\""));
    assert!(svg.contains("<g class=\"graph-edges\">"));
    assert!(svg.contains("<g class=\"graph-nodes\">"));
    assert!(svg.contains("class=\"graph-node node-ast state-pending\""));
    assert!(svg.contains("AST Parser"));
    assert!(svg.contains("[vdom.ssa] -&gt; Leaf"));
    assert!(svg.ends_with("</svg>\n"));

    // Ensure NO CSS-string injection hack is present
    assert!(!svg.contains("<style class=\"trace-router\">"));
    assert!(!svg.contains("!important"));
}

#[test]
fn test_ui_platform_graph_integration() {
    let mut platform = UiPlatform::new();
    let mut graph = ExecutionGraph::new("Interactive CFG");
    graph.add_node(GraphNode::new("bb0", "Basic Block 0", PipelineStage::BasicBlock));

    let chan_id = platform.mount_graph(graph);
    assert_eq!(chan_id, 1);
    assert!(platform.get_mounted_graph().is_some());

    // Host call: ui.render_svg
    let svg_val = platform
        .host_call(Effect::Ui, "ui.render_svg", Value::Void)
        .unwrap();
    match svg_val {
        Value::Str(svg) => {
            assert!(svg.contains("<svg"));
            assert!(svg.contains("Basic Block 0"));
        }
        other => panic!("Expected Value::Str, got {other:?}"),
    }

    // Host call: ui.step_graph
    let step_res = platform
        .host_call(Effect::Ui, "ui.step_graph", Value::I64(3))
        .unwrap();
    assert_eq!(step_res, Value::Bool(true));
    assert_eq!(platform.get_mounted_graph().unwrap().active_step, 3);
}

#[test]
fn test_window_launcher_graceful_headless_presentation() {
    let config = WindowConfig::new("Test App", 120, 80).with_headless(true);
    let mut frames_called = 0;

    let res = launch_desktop_window(config, |canvas, frame| {
        frames_called += 1;
        canvas.clear(Color::CARD_SURFACE.to_u32());
        canvas.draw_text(10, 10, "Frame", Color::WHITE.to_u32(), 1);
        frame < 2
    })
    .expect("Headless window launch must succeed gracefully");

    assert_eq!(res.backend, WindowBackend::Headless);
    assert_eq!(res.frames_rendered, 1);
    assert_ne!(res.final_pixel_hash, 0);
    assert_eq!(frames_called, 1);
}
