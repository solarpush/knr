//! Native GPU & CPU GUI Engine, Unconstrained Declarative UI Layout/Renderer,
//! 60/120 FPS Animation Engine, Custom `.knr` WGSL Shader Pipeline, and Cross-Platform Windowing.

pub mod anim;
pub mod canvas;
pub mod font;
#[cfg(feature = "gpu")]
pub mod gpu;
pub mod graph;
pub mod renderer;
pub mod style;
pub mod ui_tree;
pub mod window;

pub use anim::UiAnimator;
pub use canvas::{Canvas, HeadlessCanvas};
pub use font::{get_glyph, measure_text, FONT_8X8};
#[cfg(feature = "gpu")]
pub use gpu::{GpuUiEngine, KnrShaderUniforms};
pub use graph::{
    EdgeType, ExecutionGraph, GraphEdge, GraphNode, NodeExecutionState, PipelineStage,
    TraceRecord,
};
pub use renderer::{
    parse_hex_color, pixmap_fnv_hash, pixmap_to_softbuffer_u32, RenderInteractionState, UiRenderer,
};
pub use style::{Color, Rect, Style};
pub use ui_tree::{
    compute_ui_layout, fill_to_css, fit_input_text_tail, hit_test, hit_test_box, parse_fill_spec,
    parse_hex_rgba_f32, wrap_text_lines, HitBox, LayoutNode, LayoutRect, ParsedFill, TextMeasurer,
    UiKind, UiNode, UiStyle,
};
pub use window::{
    has_display_server, launch_desktop_window, launch_desktop_window_for_graph,
    launch_interactive_ui, launch_ui_app, launch_ui_app_with_reload, DesktopWindowConfig,
    HotReloadUpdate, UiEvent, WindowBackend, WindowConfig, WindowError, WindowLaunchResult,
    X11Connection,
};
