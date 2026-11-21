//! Native GUI engine, typed declarative UI layout/renderer, and cross-platform window subsystem for knr.
//!
//! Provides:
//! - Typed declarative UI trees (`UiNode`, `UiKind`) + pure-Rust Flexbox layout (`compute_ui_layout`)
//! - Anti-aliased 2D vector & TrueType/OpenType text rendering (`tiny-skia` + `ab_glyph`)
//! - Cross-platform native desktop windowing (`winit` + `softbuffer`) with deterministic headless fallback
//! - Low-level software framebuffer `Canvas` & DAG `ExecutionGraph` visualizer

pub mod canvas;
pub mod font;
pub mod graph;
pub mod renderer;
pub mod style;
pub mod ui_tree;
pub mod window;

pub use canvas::{Canvas, HeadlessCanvas};
pub use font::{get_glyph, measure_text, FONT_8X8};
pub use graph::{
    EdgeType, ExecutionGraph, GraphEdge, GraphNode, NodeExecutionState, PipelineStage,
    TraceRecord,
};
pub use renderer::{parse_hex_color, pixmap_fnv_hash, pixmap_to_softbuffer_u32, UiRenderer};
pub use style::{Color, Rect, Style};
pub use ui_tree::{compute_ui_layout, hit_test, HitBox, LayoutNode, LayoutRect, TextMeasurer, UiKind, UiNode};
pub use window::{
    has_display_server, launch_desktop_window, launch_desktop_window_for_graph,
    launch_interactive_ui, DesktopWindowConfig, WindowBackend, WindowConfig, WindowError,
    WindowLaunchResult, X11Connection,
};
