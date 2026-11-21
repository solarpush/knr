//! Structured, verified execution graph data model and dual-target renderer.
//!
//! Provides a formal DAG data structure for visualizing compiler pipeline stages,
//! basic block control flow, and runtime state machines.
//! Supports automated topological layout, dynamic trace-stepping integration
//! with `knr-interp`, and dual-target rendering (Native Software Framebuffer & Semantic SVG).

use crate::gui::canvas::Canvas;
use crate::gui::style::{Color, Rect};
use serde::{Deserialize, Serialize};

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

/// Execution state of a graph node during trace execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum NodeExecutionState {
    Pending,
    Executing,
    Completed,
    Failed,
}

impl NodeExecutionState {
    /// Returns the primary accent color associated with this state.
    pub fn accent_color(&self) -> Color {
        match self {
            Self::Pending => Color::TEXT_MUTED,
            Self::Executing => Color::ACCENT_CYAN,
            Self::Completed => Color::ACCENT_EMERALD,
            Self::Failed => Color::ACCENT_ROSE,
        }
    }

    /// Returns the CSS/SVG class name for this state.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Executing => "executing",
            Self::Completed => "completed",
            Self::Failed => "failed",
        }
    }
}

/// Pipeline stage classification for an execution graph node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PipelineStage {
    FrontendAst,
    VerifyPass,
    BasicBlock,
    InterpOracle,
    WasmRuntime,
    NativeCranelift,
}

impl PipelineStage {
    /// Suggested topological layer rank for layout if not overridden by explicit DAG edges.
    pub fn default_rank(&self) -> usize {
        match self {
            Self::FrontendAst => 0,
            Self::VerifyPass => 1,
            Self::BasicBlock => 2,
            Self::InterpOracle => 3,
            Self::WasmRuntime => 3,
            Self::NativeCranelift => 4,
        }
    }

    /// Human-readable label for the stage tag.
    pub fn label(&self) -> &'static str {
        match self {
            Self::FrontendAst => "AST PARSER",
            Self::VerifyPass => "KNR-VERIFY",
            Self::BasicBlock => "BASIC BLOCK",
            Self::InterpOracle => "INTERP ORACLE",
            Self::WasmRuntime => "WASM DISPATCH",
            Self::NativeCranelift => "NATIVE JIT",
        }
    }
}

/// Directed edge classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EdgeType {
    ControlFlow,    // Solid sequential edge
    BranchTrue,     // Emerald/Amber conditional branch taken
    BranchFalse,    // Purple conditional branch bypassed
    DataDependency, // Dashed data flow edge
}

impl EdgeType {
    /// Default line color for the edge type when inactive.
    pub fn default_color(&self) -> Color {
        match self {
            Self::ControlFlow => Color::TEXT_MUTED,
            Self::BranchTrue => Color::ACCENT_AMBER,
            Self::BranchFalse => Color::ACCENT_PURPLE,
            Self::DataDependency => Color::TEXT_SECONDARY,
        }
    }

    /// Color when active in trace execution.
    pub fn active_color(&self) -> Color {
        match self {
            Self::ControlFlow => Color::ACCENT_CYAN,
            Self::BranchTrue => Color::ACCENT_EMERALD,
            Self::BranchFalse => Color::ACCENT_PURPLE,
            Self::DataDependency => Color::ACCENT_CYAN,
        }
    }
}

/// A node within the structured execution graph.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GraphNode {
    pub id: String,
    pub label: String,
    pub subtitle: String,
    pub stage: PipelineStage,
    pub state: NodeExecutionState,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub code_snippet: Option<String>,
    pub alive_registers: Vec<(String, String)>,
    pub metadata: BTreeMap<String, String>,
}

impl GraphNode {
    /// Constructs a new graph node with default dimensions and `Pending` state.
    pub fn new(
        id: impl Into<String>,
        label: impl Into<String>,
        stage: PipelineStage,
    ) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            subtitle: String::new(),
            stage,
            state: NodeExecutionState::Pending,
            x: 0.0,
            y: 0.0,
            width: 200.0,
            height: 90.0,
            code_snippet: None,
            alive_registers: Vec::new(),
            metadata: BTreeMap::new(),
        }
    }

    /// Sets the subtitle/description of the node.
    pub fn with_subtitle(mut self, subtitle: impl Into<String>) -> Self {
        self.subtitle = subtitle.into();
        self
    }

    /// Sets an optional code or disassembly snippet.
    pub fn with_code(mut self, code: impl Into<String>) -> Self {
        self.code_snippet = Some(code.into());
        self
    }

    /// Adds an active register inspection pair `(reg, type_or_val)`.
    pub fn with_register(mut self, reg: impl Into<String>, ty: impl Into<String>) -> Self {
        self.alive_registers.push((reg.into(), ty.into()));
        self
    }

    /// Bounding rectangle of the node.
    pub fn bounds(&self) -> Rect {
        Rect::new(
            self.x.round() as i32,
            self.y.round() as i32,
            self.width.round() as u32,
            self.height.round() as u32,
        )
    }
}

/// A directed edge connecting two nodes in the execution graph.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GraphEdge {
    pub from: String,
    pub to: String,
    pub label: Option<String>,
    pub is_active: bool,
    pub edge_type: EdgeType,
}

impl GraphEdge {
    /// Constructs a new directed edge.
    pub fn new(
        from: impl Into<String>,
        to: impl Into<String>,
        edge_type: EdgeType,
    ) -> Self {
        Self {
            from: from.into(),
            to: to.into(),
            label: None,
            is_active: false,
            edge_type,
        }
    }

    /// Sets an optional label for the edge (e.g. branch condition `op == 0`).
    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }
}

/// A single step execution record for instruction-level tracing (compatible with `knr-interp`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TraceRecord {
    pub step: u64,
    pub line: usize,
    pub block: usize,
    pub opcode: String,
    pub dst_reg: Option<String>,
    pub new_value: Option<serde_json::Value>,
}

impl TraceRecord {
    /// Constructs a new trace record.
    pub fn new(
        step: u64,
        line: usize,
        block: usize,
        opcode: impl Into<String>,
        dst_reg: Option<String>,
        new_value: Option<serde_json::Value>,
    ) -> Self {
        Self {
            step,
            line,
            block,
            opcode: opcode.into(),
            dst_reg,
            new_value,
        }
    }
}

/// Structured execution graph data model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExecutionGraph {
    pub title: String,
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
    pub active_step: usize,
    pub selected_node: Option<String>,
}

impl ExecutionGraph {
    /// Creates a new empty execution graph with a given title.
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            nodes: Vec::new(),
            edges: Vec::new(),
            active_step: 0,
            selected_node: None,
        }
    }

    /// Adds a node to the execution graph.
    pub fn add_node(&mut self, node: GraphNode) {
        self.nodes.push(node);
    }

    /// Adds a directed edge to the execution graph.
    pub fn add_edge(&mut self, edge: GraphEdge) {
        self.edges.push(edge);
    }

    /// Finds a node by ID.
    pub fn get_node(&self, id: &str) -> Option<&GraphNode> {
        self.nodes.iter().find(|n| n.id == id)
    }

    /// Finds a mutable node by ID.
    pub fn get_node_mut(&mut self, id: &str) -> Option<&mut GraphNode> {
        self.nodes.iter_mut().find(|n| n.id == id)
    }

    /// Computes automated topological DAG layout for all nodes and edge anchor points.
    /// Distributes nodes across vertical layers and horizontal coordinates to prevent overlap.
    pub fn compute_layout(&mut self, canvas_width: u32, canvas_height: u32) {
        if self.nodes.is_empty() {
            return;
        }

        // Build in-degree and adjacency for topological sorting
        let id_to_idx: HashMap<String, usize> = self
            .nodes
            .iter()
            .enumerate()
            .map(|(i, n)| (n.id.clone(), i))
            .collect();

        let mut in_degrees = vec![0usize; self.nodes.len()];
        let mut adj = vec![Vec::new(); self.nodes.len()];

        for edge in &self.edges {
            if let (Some(&u), Some(&v)) = (id_to_idx.get(&edge.from), id_to_idx.get(&edge.to)) {
                adj[u].push(v);
                in_degrees[v] += 1;
            }
        }

        // Compute topological layers (ranks)
        let mut ranks = vec![0usize; self.nodes.len()];
        let mut queue = VecDeque::new();

        for i in 0..self.nodes.len() {
            // Baseline rank from pipeline stage
            ranks[i] = self.nodes[i].stage.default_rank();
            if in_degrees[i] == 0 {
                queue.push_back(i);
            }
        }

        while let Some(u) = queue.pop_front() {
            let next_rank = ranks[u] + 1;
            for &v in &adj[u] {
                if next_rank > ranks[v] {
                    ranks[v] = next_rank;
                }
                in_degrees[v] -= 1;
                if in_degrees[v] == 0 {
                    queue.push_back(v);
                }
            }
        }

        // Group node indices by rank
        let mut layers: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
        for (i, &r) in ranks.iter().enumerate() {
            layers.entry(r).or_default().push(i);
        }

        let total_layers = layers.len().max(1);
        let margin_top = 70.0f32; // Below title banner
        let margin_bottom = 40.0f32;
        let available_h = (canvas_height as f32 - margin_top - margin_bottom).max(100.0);

        let layer_h = (available_h / total_layers as f32).max(110.0);

        // Position nodes in each layer
        for (layer_idx, (_, node_indices)) in layers.into_iter().enumerate() {
            let count = node_indices.len();
            let y = margin_top + (layer_idx as f32) * layer_h;

            let card_w = 200.0f32;
            let card_h = 80.0f32;
            let gap_x = 30.0f32;

            let total_row_w = (count as f32) * card_w + ((count - 1) as f32) * gap_x;
            let start_x = ((canvas_width as f32 - total_row_w) / 2.0).max(20.0);

            for (col_idx, &node_idx) in node_indices.iter().enumerate() {
                let x = start_x + (col_idx as f32) * (card_w + gap_x);
                self.nodes[node_idx].x = x;
                self.nodes[node_idx].y = y;
                self.nodes[node_idx].width = card_w;
                self.nodes[node_idx].height = card_h;
            }
        }
    }

    /// Dynamically updates node execution states and active edges based on an execution trace.
    ///
    /// - Nodes traversed prior to `step` are marked `Completed`.
    /// - The node corresponding to the record at `step` is marked `Executing`.
    /// - Untraversed nodes remain `Pending`.
    /// - Edges connecting traversed blocks are marked `is_active = true`.
    pub fn update_from_trace(&mut self, trace: &[TraceRecord], step: usize) {
        self.active_step = step;

        if trace.is_empty() {
            for node in &mut self.nodes {
                node.state = NodeExecutionState::Pending;
            }
            for edge in &mut self.edges {
                edge.is_active = false;
            }
            return;
        }

        let max_step = step.min(trace.len().saturating_sub(1));
        let mut visited_blocks: HashSet<usize> = HashSet::new();
        let active_block = trace[max_step].block;


        for rec in &trace[0..=max_step] {
            visited_blocks.insert(rec.block);
        }

        // Frontend AST and Verify passes are always completed once execution starts
        for node in &mut self.nodes {
            match node.stage {
                PipelineStage::FrontendAst | PipelineStage::VerifyPass => {
                    node.state = NodeExecutionState::Completed;
                }
                PipelineStage::BasicBlock => {
                    let block_num = if node.id.starts_with("bb") {
                        node.id[2..].parse::<usize>().ok()
                    } else {
                        None
                    };

                    if let Some(b) = block_num {
                        if b == active_block {
                            node.state = NodeExecutionState::Executing;
                        } else if visited_blocks.contains(&b) {
                            node.state = NodeExecutionState::Completed;
                        } else {
                            node.state = NodeExecutionState::Pending;
                        }
                    } else {
                        node.state = NodeExecutionState::Pending;
                    }
                }
                PipelineStage::InterpOracle | PipelineStage::WasmRuntime => {
                    let is_active = trace[max_step].opcode.contains("call")
                        || trace[max_step].opcode.contains("apply_step");
                    let was_called = trace[0..max_step].iter().any(|r| {
                        r.opcode.contains("call") || r.opcode.contains("apply_step")
                    });
                    if is_active {
                        node.state = NodeExecutionState::Executing;
                    } else if was_called {
                        node.state = NodeExecutionState::Completed;
                    } else {
                        node.state = NodeExecutionState::Pending;
                    }
                }

                PipelineStage::NativeCranelift => {
                    node.state = NodeExecutionState::Pending;
                }
            }
        }

        // Update active edges
        let mut active_transitions: HashSet<(String, String)> = HashSet::new();
        for w in trace[0..=max_step].windows(2) {
            let from_block = format!("bb{}", w[0].block);
            let to_block = format!("bb{}", w[1].block);
            active_transitions.insert((from_block, to_block));
        }

        for edge in &mut self.edges {
            edge.is_active = active_transitions.contains(&(edge.from.clone(), edge.to.clone()))
                || (edge.from == "ast" && edge.to == "verify")
                || (edge.from == "verify" && edge.to == "bb0");
        }
    }

    /// Renders the complete interactive execution graph directly onto a software `Canvas`.
    pub fn render_to_canvas(&self, canvas: &mut Canvas) {
        // Clear background
        canvas.clear(Color::BG_VOID.to_u32());

        // Draw title header
        canvas.draw_text(
            24,
            24,
            &format!("▶ {}", self.title),
            Color::TEXT_PRIMARY.to_u32(),
            2,
        );

        let subtitle = format!(
            "Step: {} | Nodes: {} | Edges: {}",
            self.active_step,
            self.nodes.len(),
            self.edges.len()
        );
        canvas.draw_text(
            26,
            48,
            &subtitle,
            Color::TEXT_SECONDARY.to_u32(),
            1,
        );

        let node_map: HashMap<&str, &GraphNode> =
            self.nodes.iter().map(|n| (n.id.as_str(), n)).collect();

        // Render edges first (underneath nodes)
        for edge in &self.edges {
            if let (Some(from_node), Some(to_node)) = (
                node_map.get(edge.from.as_str()),
                node_map.get(edge.to.as_str()),
            ) {
                let color = if edge.is_active {
                    edge.edge_type.active_color()
                } else {
                    edge.edge_type.default_color()
                };

                let stroke_w = if edge.is_active { 3 } else { 2 };

                // Compute anchor points
                let (x0, y0, x1, y1) = if to_node.y > from_node.y + 10.0 {
                    // Downward flow
                    (
                        (from_node.x + from_node.width * 0.5) as i32,
                        (from_node.y + from_node.height) as i32,
                        (to_node.x + to_node.width * 0.5) as i32,
                        to_node.y as i32,
                    )
                } else if to_node.x > from_node.x + 10.0 {
                    // Rightward flow
                    (
                        (from_node.x + from_node.width) as i32,
                        (from_node.y + from_node.height * 0.5) as i32,
                        to_node.x as i32,
                        (to_node.y + to_node.height * 0.5) as i32,
                    )
                } else {
                    // General center-to-center
                    (
                        (from_node.x + from_node.width * 0.5) as i32,
                        (from_node.y + from_node.height * 0.5) as i32,
                        (to_node.x + to_node.width * 0.5) as i32,
                        (to_node.y + to_node.height * 0.5) as i32,
                    )
                };

                canvas.draw_arrow(x0, y0, x1, y1, color.to_u32(), stroke_w, 10);

                if let Some(lbl) = &edge.label {
                    let mid_x = (x0 + x1) / 2 + 8;
                    let mid_y = (y0 + y1) / 2 - 6;
                    canvas.draw_text(mid_x, mid_y, lbl, color.to_u32(), 1);
                }
            }
        }

        // Render nodes
        for node in &self.nodes {
            let accent = node.state.accent_color();
            let border_color = if node.state == NodeExecutionState::Executing {
                Color::ACCENT_CYAN
            } else if node.state == NodeExecutionState::Completed {
                Color::ACCENT_EMERALD
            } else {
                Color::CARD_BORDER
            };

            let shadow_blur = if node.state == NodeExecutionState::Executing {
                12
            } else {
                4
            };
            let shadow_col = if node.state == NodeExecutionState::Executing {
                Color::rgba(56, 189, 248, 120) // Cyan glow
            } else {
                Color::rgba(0, 0, 0, 100)
            };

            let nx = node.x.round() as i32;
            let ny = node.y.round() as i32;
            let nw = node.width.round() as u32;
            let nh = node.height.round() as u32;

            canvas.draw_card(
                nx,
                ny,
                nw,
                nh,
                8,
                Color::CARD_SURFACE.to_u32(),
                border_color.to_u32(),
                if node.state == NodeExecutionState::Executing { 2 } else { 1 },
                shadow_blur,
                shadow_col.to_u32(),
            );

            // Stage tag
            canvas.draw_text(
                nx + 10,
                ny + 10,
                node.stage.label(),
                accent.to_u32(),
                1,
            );

            // Node title/label
            canvas.draw_text(
                nx + 10,
                ny + 24,
                &node.label,
                Color::TEXT_PRIMARY.to_u32(),
                1,
            );

            // Subtitle or status badge
            let status_text = format!("[{}]", node.state.as_str().to_uppercase());
            canvas.draw_text(
                nx + 10,
                ny + 40,
                &status_text,
                accent.to_u32(),
                1,
            );

            // Code snippet or register preview
            if let Some(snip) = &node.code_snippet {
                let first_line = snip.lines().next().unwrap_or("");
                canvas.draw_text(
                    nx + 10,
                    ny + 56,
                    first_line,
                    Color::TEXT_SECONDARY.to_u32(),
                    1,
                );
            }
        }
    }

    /// Serializes the graph into clean semantic SVG elements.
    /// Replaces the legacy CSS-string-injected HTML mock execution graph.
    pub fn render_to_svg(&self) -> String {
        let max_x = self.nodes.iter().map(|n| n.x + n.width).fold(860.0f32, f32::max) + 40.0;
        let max_y = self.nodes.iter().map(|n| n.y + n.height).fold(560.0f32, f32::max) + 40.0;
        let svg_w = max_x.ceil() as u32;
        let svg_h = max_y.ceil() as u32;
        let mut out = String::with_capacity(8192);

        out.push_str(&format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="{svg_w}" height="{svg_h}" viewBox="0 0 {svg_w} {svg_h}" class="knr-execution-graph">"#
        ));
        out.push('\n');

        // SVG Definitions (Arrow markers, filters)
        out.push_str(
            r##"  <defs>
    <marker id="arrow-default" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="6" markerHeight="6" orient="auto-start-reverse">
      <path d="M 0 1 L 10 5 L 0 9 z" fill="#64748b" />
    </marker>
    <marker id="arrow-active" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="6" markerHeight="6" orient="auto-start-reverse">
      <path d="M 0 1 L 10 5 L 0 9 z" fill="#38bdf8" />
    </marker>
    <marker id="arrow-branch-true" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="6" markerHeight="6" orient="auto-start-reverse">
      <path d="M 0 1 L 10 5 L 0 9 z" fill="#10b981" />
    </marker>
    <marker id="arrow-branch-false" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="6" markerHeight="6" orient="auto-start-reverse">
      <path d="M 0 1 L 10 5 L 0 9 z" fill="#a855f7" />
    </marker>
    <filter id="glow-cyan" x="-20%" y="-20%" width="140%" height="140%">
      <feGaussianBlur stdDeviation="4" result="blur" />
      <feComposite in="SourceGraphic" in2="blur" operator="over" />
    </filter>
  </defs>
"##
        );

        // Background
        out.push_str(r##"  <rect width="100%" height="100%" fill="#050811" />"##);
        out.push('\n');

        // Header
        let esc_title = escape_xml(&self.title);
        out.push_str(&format!(
            r##"  <text x="24" y="36" fill="#f8fafc" font-family="monospace" font-size="16" font-weight="bold">▶ {}</text>
  <text x="24" y="56" fill="#94a3b8" font-family="monospace" font-size="12">Active Step: {} | Nodes: {} | Edges: {}</text>
"##,
            esc_title,
            self.active_step,
            self.nodes.len(),
            self.edges.len()
        ));

        let node_map: HashMap<&str, &GraphNode> =
            self.nodes.iter().map(|n| (n.id.as_str(), n)).collect();

        // Render Edges
        out.push_str("  <g class=\"graph-edges\">\n");
        for edge in &self.edges {
            if let (Some(from_node), Some(to_node)) = (
                node_map.get(edge.from.as_str()),
                node_map.get(edge.to.as_str()),
            ) {
                let stroke_color = if edge.is_active {
                    edge.edge_type.active_color().to_hex_str()
                } else {
                    edge.edge_type.default_color().to_hex_str()
                };

                let marker_id = if edge.is_active {
                    "arrow-active"
                } else {
                    match edge.edge_type {
                        EdgeType::BranchTrue => "arrow-branch-true",
                        EdgeType::BranchFalse => "arrow-branch-false",
                        _ => "arrow-default",
                    }
                };

                let (x0, y0, x1, y1) = if to_node.y > from_node.y + 10.0 {
                    (
                        from_node.x + from_node.width * 0.5,
                        from_node.y + from_node.height,
                        to_node.x + to_node.width * 0.5,
                        to_node.y,
                    )
                } else if to_node.x > from_node.x + 10.0 {
                    (
                        from_node.x + from_node.width,
                        from_node.y + from_node.height * 0.5,
                        to_node.x,
                        to_node.y + to_node.height * 0.5,
                    )
                } else {
                    (
                        from_node.x + from_node.width * 0.5,
                        from_node.y + from_node.height * 0.5,
                        to_node.x + to_node.width * 0.5,
                        to_node.y + to_node.height * 0.5,
                    )
                };

                let stroke_w = if edge.is_active { 3 } else { 2 };
                let dash = if edge.edge_type == EdgeType::DataDependency {
                    " stroke-dasharray=\"4,4\""
                } else {
                    ""
                };

                out.push_str(&format!(
                    r##"    <path d="M {x0:.1} {y0:.1} L {x1:.1} {y1:.1}" stroke="{stroke_color}" stroke-width="{stroke_w}"{dash} marker-end="url(#{marker_id})" />
"##
                ));

                if let Some(lbl) = &edge.label {
                    let mid_x = (x0 + x1) * 0.5 + 8.0;
                    let mid_y = (y0 + y1) * 0.5 - 4.0;
                    let esc_lbl = escape_xml(lbl);
                    out.push_str(&format!(
                        r##"    <text x="{mid_x:.1}" y="{mid_y:.1}" fill="{stroke_color}" font-family="monospace" font-size="11">{esc_lbl}</text>
"##
                    ));
                }
            }
        }
        out.push_str("  </g>\n");

        // Render Nodes
        out.push_str("  <g class=\"graph-nodes\">\n");
        for node in &self.nodes {
            let border_col = if node.state == NodeExecutionState::Executing {
                "#38bdf8"
            } else if node.state == NodeExecutionState::Completed {
                "#10b981"
            } else {
                "#1e293b"
            };

            let stroke_w = if node.state == NodeExecutionState::Executing { 2 } else { 1 };
            let filter = if node.state == NodeExecutionState::Executing {
                " filter=\"url(#glow-cyan)\""
            } else {
                ""
            };
            let accent_hex = node.state.accent_color().to_hex_str();
            let esc_id = escape_xml(&node.id);
            let esc_label = escape_xml(&node.label);

            out.push_str(&format!(
                r##"    <g class="graph-node node-{id} state-{state}" transform="translate({x:.1}, {y:.1})">
      <rect width="{w:.1}" height="{h:.1}" rx="8" fill="#0f172a" stroke="{border_col}" stroke-width="{stroke_w}"{filter} />
      <text x="10" y="18" fill="{accent_hex}" font-family="monospace" font-size="10" font-weight="bold">{stage}</text>
      <text x="10" y="36" fill="#f8fafc" font-family="monospace" font-size="12" font-weight="bold">{label}</text>
      <text x="10" y="52" fill="{accent_hex}" font-family="monospace" font-size="10">[{state_upper}]</text>
"##,
                id = esc_id,
                state = node.state.as_str(),
                x = node.x,
                y = node.y,
                w = node.width,
                h = node.height,
                stage = node.stage.label(),
                label = esc_label,
                state_upper = node.state.as_str().to_uppercase(),
            ));

            if let Some(snip) = &node.code_snippet {
                let first_line = snip.lines().next().unwrap_or("");
                let escaped_line = escape_xml(first_line);
                out.push_str(&format!(
                    r##"      <text x="10" y="68" fill="#94a3b8" font-family="monospace" font-size="10">{escaped_line}</text>
"##
                ));
            }

            out.push_str("    </g>\n");
        }
        out.push_str("  </g>\n");


        out.push_str("</svg>\n");
        out
    }
}

/// Escapes special XML characters (&, <, >, ", ') into XML entities.
fn escape_xml(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(c),
        }
    }
    out
}

