//! Empirical adversarial stress tests for `ExecutionGraph` DAG layout, trace stepping,
//! and SVG/dual-target export (Milestone 2 Challenger Suite).

use knr_core::Effect;
use knr_runtime::gui::canvas::Canvas;
use knr_runtime::gui::graph::{
    EdgeType, ExecutionGraph, GraphEdge, GraphNode, NodeExecutionState, PipelineStage,
    TraceRecord,
};
use knr_runtime::platform::{HostPlatform, Value};
use knr_runtime::ui::UiPlatform;
use std::time::Instant;

/// Minimal XML 1.0 validator to check whether generated SVG markup is well-formed.
/// Checks:
/// 1. Tag balance (matching opening and closing tags).
/// 2. No unescaped `<` inside text content or attribute values.
/// 3. No unescaped `&` (unless valid entity: &amp;, &lt;, &gt;, &quot;, &apos;, &#...;).
fn validate_xml_well_formedness(xml: &str) -> Result<(), String> {
    let mut tag_stack: Vec<String> = Vec::new();
    let chars: Vec<char> = xml.chars().collect();
    let n = chars.len();
    let mut i = 0;

    while i < n {
        if chars[i] == '<' {
            // Find end of tag
            let tag_start = i;
            let mut tag_end = None;
            let mut in_quote: Option<char> = None;

            i += 1;
            while i < n {
                let c = chars[i];
                if let Some(q) = in_quote {
                    if c == q {
                        in_quote = None;
                    }
                } else if c == '"' || c == '\'' {
                    in_quote = Some(c);
                } else if c == '>' {
                    tag_end = Some(i);
                    break;
                }
                i += 1;
            }

            let end_idx = tag_end.ok_or_else(|| {
                format!("Unclosed tag starting at position {tag_start}: {:?}", &xml[tag_start..n.min(tag_start + 40)])
            })?;

            let tag_content: String = chars[tag_start + 1..end_idx].iter().collect();
            let tag_content = tag_content.trim();

            if tag_content.starts_with("?xml") || tag_content.starts_with("!--") {
                // Comment or declaration
            } else if let Some(stripped) = tag_content.strip_prefix('/') {
                // Closing tag
                let close_tag = stripped.trim();
                let expected = tag_stack.pop().ok_or_else(|| {
                    format!("Unexpected closing tag </{close_tag}> with empty tag stack")
                })?;
                if close_tag != expected {
                    return Err(format!(
                        "Tag mismatch: expected </{expected}>, found </{close_tag}>"
                    ));
                }
            } else if tag_content.ends_with('/') {
                // Self-closing tag <tag ... />
            } else {
                // Opening tag
                let tag_name = tag_content
                    .split_whitespace()
                    .next()
                    .unwrap_or("")
                    .to_string();
                if !tag_name.is_empty() {
                    tag_stack.push(tag_name);
                }
            }
            i += 1;
        } else if chars[i] == '&' {
            // Check for valid XML entity reference
            let entity_start = i;
            let mut semi_found = false;
            let mut j = i + 1;
            while j < n && j < i + 10 {
                if chars[j] == ';' {
                    semi_found = true;
                    break;
                }
                if chars[j] == '<' || chars[j] == ' ' || chars[j] == '&' {
                    break;
                }
                j += 1;
            }

            if !semi_found {
                return Err(format!(
                    "Unescaped '&' found at index {entity_start}: {:?}",
                    &xml[entity_start..n.min(entity_start + 20)]
                ));
            }

            let entity: String = chars[entity_start + 1..j].iter().collect();
            let valid = matches!(entity.as_str(), "amp" | "lt" | "gt" | "quot" | "apos")
                || entity.starts_with('#');
            if !valid {
                return Err(format!("Unknown or invalid entity '&{entity};' at index {entity_start}"));
            }
            i = j + 1;
        } else {
            i += 1;
        }
    }

    if !tag_stack.is_empty() {
        return Err(format!("Unclosed tags remaining on stack: {:?}", tag_stack));
    }

    Ok(())
}

// =========================================================================
// Category 1: Large DAGs & Complex Topologies
// =========================================================================

#[test]
fn test_large_dag_120_nodes_deep_chain() {
    let mut graph = ExecutionGraph::new("Deep 120-Node Chain");

    for i in 0..120 {
        graph.add_node(GraphNode::new(
            format!("node_{i}"),
            format!("Pipeline Step {i}"),
            PipelineStage::BasicBlock,
        ));
        if i > 0 {
            graph.add_edge(GraphEdge::new(
                format!("node_{}", i - 1),
                format!("node_{i}"),
                EdgeType::ControlFlow,
            ));
        }
    }

    // Compute layout
    let start = Instant::now();
    graph.compute_layout(1200, 800);
    let elapsed = start.elapsed();
    assert!(
        elapsed.as_millis() < 100,
        "Layout for 120-node chain took too long: {elapsed:?}"
    );

    // Verify monotonic vertical ranks
    for i in 1..120 {
        let prev = graph.get_node(&format!("node_{}", i - 1)).unwrap();
        let curr = graph.get_node(&format!("node_{i}")).unwrap();
        assert!(
            curr.y > prev.y,
            "Topological invariant violated: node_{i} y ({}) <= node_{} y ({})",
            curr.y,
            i - 1,
            prev.y
        );
    }

    // Render to Canvas: memory safety and clipping
    let mut canvas = Canvas::new(1200, 800);
    graph.render_to_canvas(&mut canvas);

    // Render to SVG: all nodes present
    let svg = graph.render_to_svg();
    for i in 0..120 {
        assert!(
            svg.contains(&format!("node-node_{i}")),
            "SVG missing node_{i}"
        );
    }
}

#[test]
fn test_large_dag_wide_layer_overlap_avoidance() {
    let mut graph = ExecutionGraph::new("Wide Layer (100 parallel nodes)");

    for i in 0..100 {
        graph.add_node(GraphNode::new(
            format!("par_{i}"),
            format!("Worker {i}"),
            PipelineStage::BasicBlock,
        ));
    }

    // All independent nodes get the same rank (default_rank = 2 for BasicBlock)
    graph.compute_layout(4000, 600);

    // Check all nodes share identical y coordinate
    let y0 = graph.get_node("par_0").unwrap().y;
    for i in 0..100 {
        let n = graph.get_node(&format!("par_{i}")).unwrap();
        assert_eq!(n.y, y0, "Node par_{i} y mismatch in wide layer");
    }

    // Check no horizontal overlap between any adjacent nodes in layer
    let mut xs: Vec<f32> = (0..100)
        .map(|i| graph.get_node(&format!("par_{i}")).unwrap().x)
        .collect();
    xs.sort_by(|a, b| a.partial_cmp(b).unwrap());

    for w in xs.windows(2) {
        let gap = w[1] - w[0];
        assert!(
            gap >= 200.0,
            "Horizontal overlap detected in wide layer: gap {gap} < card_w 200.0"
        );
    }
}

#[test]
fn test_dense_edges_complete_bipartite_dag() {
    let mut graph = ExecutionGraph::new("Dense Bipartite DAG");

    // 20 nodes in stage FrontendAst (rank 0), 20 nodes in stage BasicBlock (rank 2)
    for i in 0..20 {
        graph.add_node(GraphNode::new(
            format!("a_{i}"),
            format!("Source {i}"),
            PipelineStage::FrontendAst,
        ));
        graph.add_node(GraphNode::new(
            format!("b_{i}"),
            format!("Sink {i}"),
            PipelineStage::BasicBlock,
        ));
    }

    // Complete bipartite edges: 20 x 20 = 400 edges
    for i in 0..20 {
        for j in 0..20 {
            graph.add_edge(GraphEdge::new(
                format!("a_{i}"),
                format!("b_{j}"),
                EdgeType::DataDependency,
            ));
        }
    }

    assert_eq!(graph.nodes.len(), 40);
    assert_eq!(graph.edges.len(), 400);

    let start = Instant::now();
    graph.compute_layout(1200, 800);
    let mut canvas = Canvas::new(1200, 800);
    graph.render_to_canvas(&mut canvas);
    let svg = graph.render_to_svg();
    let elapsed = start.elapsed();

    assert!(
        elapsed.as_millis() < 250,
        "Dense graph layout + render took too long: {elapsed:?}"
    );
    assert!(svg.contains("<path d="));
}

#[test]
fn test_diamond_pattern_nested_branches() {
    let mut graph = ExecutionGraph::new("Diamond Pattern");

    // Root -> (Left, Right) -> Join -> (Left2, Right2) -> Sink
    graph.add_node(GraphNode::new("root", "Root", PipelineStage::FrontendAst));
    graph.add_node(GraphNode::new("left1", "Left 1", PipelineStage::BasicBlock));
    graph.add_node(GraphNode::new("right1", "Right 1", PipelineStage::BasicBlock));
    graph.add_node(GraphNode::new("join1", "Join 1", PipelineStage::BasicBlock));
    graph.add_node(GraphNode::new("left2", "Left 2", PipelineStage::BasicBlock));
    graph.add_node(GraphNode::new("right2", "Right 2", PipelineStage::BasicBlock));
    graph.add_node(GraphNode::new("sink", "Sink", PipelineStage::InterpOracle));

    graph.add_edge(GraphEdge::new("root", "left1", EdgeType::BranchTrue));
    graph.add_edge(GraphEdge::new("root", "right1", EdgeType::BranchFalse));
    graph.add_edge(GraphEdge::new("left1", "join1", EdgeType::ControlFlow));
    graph.add_edge(GraphEdge::new("right1", "join1", EdgeType::ControlFlow));
    graph.add_edge(GraphEdge::new("join1", "left2", EdgeType::BranchTrue));
    graph.add_edge(GraphEdge::new("join1", "right2", EdgeType::BranchFalse));
    graph.add_edge(GraphEdge::new("left2", "sink", EdgeType::ControlFlow));
    graph.add_edge(GraphEdge::new("right2", "sink", EdgeType::ControlFlow));

    graph.compute_layout(900, 600);

    let root = graph.get_node("root").unwrap();
    let left1 = graph.get_node("left1").unwrap();
    let right1 = graph.get_node("right1").unwrap();
    let join1 = graph.get_node("join1").unwrap();
    let left2 = graph.get_node("left2").unwrap();
    let right2 = graph.get_node("right2").unwrap();
    let sink = graph.get_node("sink").unwrap();

    assert!(root.y < left1.y);
    assert_eq!(left1.y, right1.y);
    assert!(left1.y < join1.y);
    assert!(join1.y < left2.y);
    assert_eq!(left2.y, right2.y);
    assert!(left2.y < sink.y);
}

#[test]
fn test_disconnected_subgraphs_multi_component() {
    let mut graph = ExecutionGraph::new("5 Disconnected Components");

    for c in 0..5 {
        let n1 = format!("comp_{c}_a");
        let n2 = format!("comp_{c}_b");
        graph.add_node(GraphNode::new(&n1, format!("Comp {c} Root"), PipelineStage::FrontendAst));
        graph.add_node(GraphNode::new(&n2, format!("Comp {c} Leaf"), PipelineStage::BasicBlock));
        graph.add_edge(GraphEdge::new(&n1, &n2, EdgeType::ControlFlow));
    }

    assert_eq!(graph.nodes.len(), 10);
    assert_eq!(graph.edges.len(), 5);

    graph.compute_layout(1000, 600);

    for c in 0..5 {
        let n1 = graph.get_node(&format!("comp_{c}_a")).unwrap();
        let n2 = graph.get_node(&format!("comp_{c}_b")).unwrap();
        assert!(n1.y < n2.y, "Component {c} root must be placed above leaf");
    }
}

// =========================================================================
// Category 2: Cyclic Graphs & Termination Verification
// =========================================================================

#[test]
fn test_cyclic_graph_self_loop_termination() {
    let mut graph = ExecutionGraph::new("Self Loop");
    graph.add_node(GraphNode::new("loop_node", "Self Loop Node", PipelineStage::BasicBlock));
    graph.add_edge(GraphEdge::new("loop_node", "loop_node", EdgeType::ControlFlow));

    let start = Instant::now();
    graph.compute_layout(800, 600);
    let mut canvas = Canvas::new(800, 600);
    graph.render_to_canvas(&mut canvas);
    let svg = graph.render_to_svg();
    let elapsed = start.elapsed();

    assert!(
        elapsed.as_millis() < 50,
        "Self loop layout must terminate promptly"
    );
    assert!(svg.contains("node-loop_node"));
}

#[test]
fn test_cyclic_graph_two_node_mutual_cycle() {
    let mut graph = ExecutionGraph::new("Mutual 2-Cycle");
    graph.add_node(GraphNode::new("node_a", "Node A", PipelineStage::BasicBlock));
    graph.add_node(GraphNode::new("node_b", "Node B", PipelineStage::BasicBlock));

    graph.add_edge(GraphEdge::new("node_a", "node_b", EdgeType::ControlFlow));
    graph.add_edge(GraphEdge::new("node_b", "node_a", EdgeType::BranchFalse));

    let start = Instant::now();
    graph.compute_layout(800, 600);
    let mut canvas = Canvas::new(800, 600);
    graph.render_to_canvas(&mut canvas);
    let elapsed = start.elapsed();

    assert!(
        elapsed.as_millis() < 50,
        "2-node cycle layout must terminate without infinite loop"
    );
}

#[test]
fn test_cyclic_graph_natural_loop_with_entry_and_exit() {
    let mut graph = ExecutionGraph::new("CFG Natural Loop");
    graph.add_node(GraphNode::new("entry", "Entry", PipelineStage::FrontendAst));
    graph.add_node(GraphNode::new("header", "Loop Header", PipelineStage::BasicBlock));
    graph.add_node(GraphNode::new("body", "Loop Body", PipelineStage::BasicBlock));
    graph.add_node(GraphNode::new("latch", "Loop Latch", PipelineStage::BasicBlock));
    graph.add_node(GraphNode::new("exit", "Loop Exit", PipelineStage::NativeCranelift));

    graph.add_edge(GraphEdge::new("entry", "header", EdgeType::ControlFlow));
    graph.add_edge(GraphEdge::new("header", "body", EdgeType::BranchTrue));
    graph.add_edge(GraphEdge::new("body", "latch", EdgeType::ControlFlow));
    graph.add_edge(GraphEdge::new("latch", "header", EdgeType::ControlFlow)); // Back edge
    graph.add_edge(GraphEdge::new("header", "exit", EdgeType::BranchFalse));

    let start = Instant::now();
    graph.compute_layout(1000, 700);
    let elapsed = start.elapsed();

    assert!(elapsed.as_millis() < 50, "Natural loop layout must terminate");

    let entry = graph.get_node("entry").unwrap();
    let header = graph.get_node("header").unwrap();
    assert!(entry.y < header.y, "Entry must be above header");
}

#[test]
fn test_cyclic_graph_15_node_dense_clique() {
    let mut graph = ExecutionGraph::new("15-Node Cyclic Clique");
    for i in 0..15 {
        graph.add_node(GraphNode::new(
            format!("clique_{i}"),
            format!("Clique Node {i}"),
            PipelineStage::BasicBlock,
        ));
    }

    // 15 * 14 = 210 cyclic directed edges
    for i in 0..15 {
        for j in 0..15 {
            if i != j {
                graph.add_edge(GraphEdge::new(
                    format!("clique_{i}"),
                    format!("clique_{j}"),
                    EdgeType::ControlFlow,
                ));
            }
        }
    }

    let start = Instant::now();
    graph.compute_layout(1000, 800);
    let elapsed = start.elapsed();
    assert!(
        elapsed.as_millis() < 100,
        "Dense cyclic clique layout must terminate gracefully"
    );
}

// =========================================================================
// Category 3: Edge Cases: Empty Graphs, Single Nodes & Missing References
// =========================================================================

#[test]
fn test_empty_graph_operations() {
    let mut graph = ExecutionGraph::new("Empty Graph");

    // Layout on empty graph
    graph.compute_layout(800, 600);

    // Canvas render on empty graph
    let mut canvas = Canvas::new(800, 600);
    graph.render_to_canvas(&mut canvas);

    // SVG render on empty graph
    let svg = graph.render_to_svg();
    assert!(svg.starts_with("<svg"));
    assert!(svg.contains("Nodes: 0 | Edges: 0"));
    assert!(svg.ends_with("</svg>\n"));

    // Trace update on empty graph
    let trace = vec![TraceRecord::new(0, 1, 0, "nop", None, None)];
    graph.update_from_trace(&trace, 0);
    assert_eq!(graph.active_step, 0);
}

#[test]
fn test_single_node_graph_centered() {
    let mut graph = ExecutionGraph::new("Single Node");
    graph.add_node(GraphNode::new("solo", "Solo Node", PipelineStage::FrontendAst));

    graph.compute_layout(800, 600);
    let solo = graph.get_node("solo").unwrap();

    // With canvas_width = 800 and card_w = 200, start_x = (800 - 200)/2 = 300
    assert_eq!(solo.x, 300.0);
    assert_eq!(solo.y, 70.0); // margin_top
}

#[test]
fn test_edges_referencing_nonexistent_node_ids() {
    let mut graph = ExecutionGraph::new("Orphan Edges");
    graph.add_node(GraphNode::new("valid_1", "Valid 1", PipelineStage::BasicBlock));

    // Edges referencing ghost nodes
    graph.add_edge(GraphEdge::new("ghost_from", "valid_1", EdgeType::ControlFlow));
    graph.add_edge(GraphEdge::new("valid_1", "ghost_to", EdgeType::ControlFlow));
    graph.add_edge(GraphEdge::new("ghost_a", "ghost_b", EdgeType::DataDependency));

    // Must not panic in layout, canvas render, or svg export
    graph.compute_layout(800, 600);
    let mut canvas = Canvas::new(800, 600);
    graph.render_to_canvas(&mut canvas);
    let svg = graph.render_to_svg();

    assert!(svg.contains("node-valid_1"));
}

#[test]
fn test_zero_and_negative_dimension_resilience() {
    let mut graph = ExecutionGraph::new("Zero Dim Layout");
    graph.add_node(GraphNode::new("n0", "N0", PipelineStage::BasicBlock));

    // Canvas size (0, 0)
    graph.compute_layout(0, 0);
    let n0 = graph.get_node("n0").unwrap();
    // Clamped coordinates
    assert!(n0.x >= 20.0);
    assert!(n0.y >= 70.0);
}

// =========================================================================
// Category 4: Trace Stepping Stress & State Transitions
// =========================================================================

#[test]
fn test_trace_stepping_out_of_order_and_rewind() {
    let mut graph = ExecutionGraph::new("Stepping Test");
    graph.add_node(GraphNode::new("bb0", "BB 0", PipelineStage::BasicBlock));
    graph.add_node(GraphNode::new("bb1", "BB 1", PipelineStage::BasicBlock));
    graph.add_node(GraphNode::new("bb2", "BB 2", PipelineStage::BasicBlock));
    graph.add_node(GraphNode::new("bb3", "BB 3", PipelineStage::BasicBlock));

    graph.add_edge(GraphEdge::new("bb0", "bb1", EdgeType::ControlFlow));
    graph.add_edge(GraphEdge::new("bb1", "bb2", EdgeType::ControlFlow));
    graph.add_edge(GraphEdge::new("bb2", "bb3", EdgeType::ControlFlow));

    let trace = vec![
        TraceRecord::new(0, 1, 0, "const_i64", None, None), // bb0
        TraceRecord::new(1, 2, 1, "add", None, None),       // bb1
        TraceRecord::new(2, 3, 2, "sub", None, None),       // bb2
        TraceRecord::new(3, 4, 3, "ret", None, None),       // bb3
    ];

    // Jump straight to step 2 (bb2)
    graph.update_from_trace(&trace, 2);
    assert_eq!(graph.get_node("bb0").unwrap().state, NodeExecutionState::Completed);
    assert_eq!(graph.get_node("bb1").unwrap().state, NodeExecutionState::Completed);
    assert_eq!(graph.get_node("bb2").unwrap().state, NodeExecutionState::Executing);
    assert_eq!(graph.get_node("bb3").unwrap().state, NodeExecutionState::Pending);

    // Rewind back to step 0 (bb0)
    graph.update_from_trace(&trace, 0);
    assert_eq!(graph.get_node("bb0").unwrap().state, NodeExecutionState::Executing);
    assert_eq!(graph.get_node("bb1").unwrap().state, NodeExecutionState::Pending);
    assert_eq!(graph.get_node("bb2").unwrap().state, NodeExecutionState::Pending);
    assert_eq!(graph.get_node("bb3").unwrap().state, NodeExecutionState::Pending);

    // Edge bb1->bb2 must NOT be active when rewound to step 0
    let edge_1_2 = graph.edges.iter().find(|e| e.from == "bb1" && e.to == "bb2").unwrap();
    assert!(!edge_1_2.is_active);

    // Advance to step 3 (bb3)
    graph.update_from_trace(&trace, 3);
    assert_eq!(graph.get_node("bb3").unwrap().state, NodeExecutionState::Executing);
    assert_eq!(graph.get_node("bb2").unwrap().state, NodeExecutionState::Completed);
    let edge_2_3 = graph.edges.iter().find(|e| e.from == "bb2" && e.to == "bb3").unwrap();
    assert!(edge_2_3.is_active);
}

#[test]
fn test_trace_stepping_nonexistent_block_ids_and_overflow() {
    let mut graph = ExecutionGraph::new("Trace Outlier Test");
    graph.add_node(GraphNode::new("bb0", "BB 0", PipelineStage::BasicBlock));

    let trace = vec![
        TraceRecord::new(0, 1, 99999, "nop", None, None), // Nonexistent block
        TraceRecord::new(1, 2, 88888, "nop", None, None),
    ];

    // Step with invalid block ID
    graph.update_from_trace(&trace, 0);
    assert_eq!(graph.get_node("bb0").unwrap().state, NodeExecutionState::Pending);

    // Step with step index far out of bounds (usize::MAX)
    graph.update_from_trace(&trace, usize::MAX);
    assert_eq!(graph.get_node("bb0").unwrap().state, NodeExecutionState::Pending);
}

#[test]
fn test_trace_stepping_non_standard_node_id_limitation() {
    // BasicBlock nodes not following strict "bb<usize>" naming
    let mut graph = ExecutionGraph::new("Custom Naming");
    graph.add_node(GraphNode::new("entry_block", "Entry", PipelineStage::BasicBlock));
    graph.add_node(GraphNode::new("loop_header", "Header", PipelineStage::BasicBlock));

    let trace = vec![
        TraceRecord::new(0, 1, 0, "nop", None, None),
        TraceRecord::new(1, 2, 1, "nop", None, None),
    ];

    graph.update_from_trace(&trace, 0);

    // Documenting empirical observation: non-"bb<usize>" node IDs remain Pending
    assert_eq!(
        graph.get_node("entry_block").unwrap().state,
        NodeExecutionState::Pending,
        "Nodes not starting with 'bb' remain Pending during trace stepping"
    );
}

// =========================================================================
// Category 5: SVG Validation & Escaping Vulnerabilities
// =========================================================================

#[test]
fn test_svg_output_well_formedness_standard() {
    let mut graph = ExecutionGraph::new("Pipeline Test");
    graph.add_node(
        GraphNode::new("ast", "AST Parser", PipelineStage::FrontendAst)
            .with_code("[vdom.ssa] -> Leaf"),
    );
    graph.add_node(GraphNode::new("verify", "knr-verify", PipelineStage::VerifyPass));
    graph.add_edge(GraphEdge::new("ast", "verify", EdgeType::ControlFlow));
    graph.compute_layout(900, 600);

    let svg = graph.render_to_svg();

    // Verify well-formed XML
    let res = validate_xml_well_formedness(&svg);
    assert!(res.is_ok(), "Generated SVG failed XML validation: {res:?}");
}

#[test]
fn test_svg_escaping_vulnerabilities_in_labels_and_titles() {
    let mut graph = ExecutionGraph::new("Title with <tag> & special chars");
    graph.add_node(
        GraphNode::new("node_special", "Node with <generics> & 'quotes'", PipelineStage::BasicBlock)
            .with_code("let x = a < 5 && b > 2;"),
    );
    graph.add_node(GraphNode::new("target", "Target", PipelineStage::BasicBlock));
    graph.add_edge(
        GraphEdge::new("node_special", "target", EdgeType::BranchTrue)
            .with_label("x < 10 && y > 5"),
    );

    let svg = graph.render_to_svg();

    // Empirical observation of whether escaping was applied
    let title_has_raw_angle = svg.contains("▶ Title with <tag>");
    let edge_has_raw_angle = svg.contains("x < 10 && y > 5");
    let node_has_raw_angle = svg.contains("Node with <generics>");

    // Code snippet IS properly escaped by graph.rs
    assert!(
        svg.contains("let x = a &lt; 5 &amp;&amp; b &gt; 2;"),
        "Code snippet must be XML-escaped"
    );

    // If title, edge, or node label contains raw '<', XML validation will fail!
    let validation_result = validate_xml_well_formedness(&svg);

    // Record the empirical finding:
    if title_has_raw_angle || edge_has_raw_angle || node_has_raw_angle {
        assert!(
            validation_result.is_err(),
            "Expected XML validator to reject unescaped XML in labels/titles"
        );
        println!(
            "VULNERABILITY CONFIRMED: SVG export contains unescaped XML characters in titles/labels: {validation_result:?}"
        );
    }
}

#[test]
fn test_svg_fixed_viewbox_vs_large_graph_dimensions() {
    let mut graph = ExecutionGraph::new("50 Layer Deep Graph");

    for i in 0..50 {
        graph.add_node(GraphNode::new(
            format!("deep_{i}"),
            format!("Node {i}"),
            PipelineStage::BasicBlock,
        ));
        if i > 0 {
            graph.add_edge(GraphEdge::new(
                format!("deep_{}", i - 1),
                format!("deep_{i}"),
                EdgeType::ControlFlow,
            ));
        }
    }

    graph.compute_layout(900, 600);

    let last_node = graph.get_node("deep_49").unwrap();
    // In compute_layout, 50 layers each take at least 110.0px -> y ~ 70 + 49 * 110 = 5460px
    assert!(
        last_node.y > 600.0,
        "50-layer graph last node y ({}) exceeds default canvas height 600.0",
        last_node.y
    );

    let svg = graph.render_to_svg();

    // Verify dynamic viewBox encompasses the full graph bounds
    let expected_min_h = (last_node.y + last_node.height + 40.0).ceil() as u32;
    assert!(
        !svg.contains("viewBox=\"0 0 900 600\""),
        "SVG viewBox must dynamically expand for deep graphs"
    );
    assert!(
        svg.contains(&format!("viewBox=\"0 0 900 {expected_min_h}\"")),
        "SVG viewBox dynamically bounds all nodes: expected height at least {expected_min_h}"
    );
}

#[test]
fn test_ui_platform_render_vnode_svg_escaping_behavior() {
    let mut platform = UiPlatform::new();
    let mut graph = ExecutionGraph::new("Mounted Graph");
    graph.add_node(GraphNode::new("n0", "N0", PipelineStage::BasicBlock));
    platform.mount_graph(graph);

    // 1. Direct SVG export via host call "ui.render_svg"
    let svg_call_res = platform.host_call(Effect::Ui, "ui.render_svg", Value::Void).unwrap();
    if let Value::Str(svg) = svg_call_res {
        assert!(svg.starts_with("<svg"), "ui.render_svg produces raw <svg tag");
    } else {
        panic!("Expected Value::Str");
    }

    // 2. Full HTML SSR rendering via host call "ui.render"
    let html_call_res = platform.host_call(Effect::Ui, "ui.render", Value::Void).unwrap();
    if let Value::Str(html) = html_call_res {
        let is_escaped = html.contains("&lt;svg");
        let is_raw = html.contains("<svg");

        println!("UiPlatform::render SSR behavior: is_escaped={is_escaped}, is_raw={is_raw}");
        assert!(
            is_raw && !is_escaped,
            "VNode::from_execution_graph renders unescaped active SVG element in HTML SSR"
        );
    } else {
        panic!("Expected Value::Str");
    }
}
