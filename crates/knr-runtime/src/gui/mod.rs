//! Native GPU & CPU GUI Engine, Unconstrained Declarative UI Layout/Renderer,
//! 60/120 FPS Animation Engine, Custom `.knr` WGSL Shader Pipeline, and Cross-Platform Windowing.

pub mod anim;
pub mod audit;
pub mod canvas;
pub mod font;
#[cfg(feature = "gpu")]
pub mod gpu;
pub mod graph;
pub mod renderer;
pub mod style;
pub mod ui_tree;
pub mod window;

pub use anim::{spring_step, UiAnimator};
pub use audit::{
    audit_layout, audit_layout_with_images, AccessibilitySummary, AuditIssue, AuditNode,
    VisualAuditReport,
};
pub use canvas::{Canvas, HeadlessCanvas};
pub use font::{get_glyph, measure_text, FONT_8X8};
#[cfg(feature = "gpu")]
pub use gpu::{GpuUiEngine, KnrShaderUniforms};
pub use graph::{
    EdgeType, ExecutionGraph, GraphEdge, GraphNode, NodeExecutionState, PipelineStage,
    TraceRecord,
};
pub use renderer::{
    paint_vector_icon, parse_hex_color, pixmap_fnv_hash, pixmap_to_softbuffer_u32,
    supported_icon_names, RenderInteractionState, UiRenderer, SUPPORTED_ICON_NAMES,
};
pub use style::{Color, Rect, Style};
pub use ui_tree::{
    apply_scroll_offsets, compute_ui_layout, compute_ui_layout_with_scroll,
    effective_line_height_mult, fill_to_css, fit_input_text_tail, hit_test, hit_test_box,
    hit_test_scroll_target, hit_test_scroll_targets, parse_fill_spec, parse_hex_rgba_f32,
    scroll_container_id, wrap_text_lines, wrap_text_lines_styled, HitBox, LayoutNode, LayoutRect,
    ParsedFill, ScrollTarget, TextMeasurer, UiKind, UiNode, UiStyle,
};
pub use window::{
    format_key_combo, has_display_server, launch_desktop_window, launch_desktop_window_for_graph,
    launch_interactive_ui, launch_ui_app, launch_ui_app_with_reload,
    launch_ui_app_with_reload_and_effects, spawn_ui_cmd, DesktopWindowConfig, HotReloadUpdate,
    UiCmd, UiEvent, WindowBackend, WindowConfig, WindowError, WindowLaunchResult, X11Connection,
};
