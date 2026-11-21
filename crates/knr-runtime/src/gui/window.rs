//! Native cross-platform desktop window presentation (Wayland / X11 / macOS / Windows)
//! via `winit 0.30` + `softbuffer 0.4` + `tiny-skia` + `ab_glyph`,
//! with automatic deterministic headless fallback for CI and displayless environments.

use crate::gui::canvas::{Canvas, HeadlessCanvas};
use crate::gui::renderer::{pixmap_fnv_hash, pixmap_to_softbuffer_u32, UiRenderer};
use crate::gui::ui_tree::{hit_test, HitBox, UiNode};
use std::env;
use std::io::{Read, Write};
use std::num::NonZeroU32;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::rc::Rc;
use std::time::Duration;
use thiserror::Error;
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, KeyEvent, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::{CursorIcon, Window, WindowId};

/// Errors that can occur during window initialization or event presentation.
#[derive(Debug, Error)]
pub enum WindowError {
    #[error("I/O error during window presentation: {0}")]
    Io(#[from] std::io::Error),

    #[error("X11 connection rejected: {0}")]
    X11Rejected(String),

    #[error("Rendering error: {0}")]
    Render(String),
}

/// The active display backend used to present the application window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowBackend {
    /// Cross-platform native desktop window via `winit` + `softbuffer` (Wayland/X11/macOS/Windows).
    Winit,
    /// Legacy X11 backend variant retained for backwards compatibility.
    X11,
    /// Deterministic headless software framebuffer (CI/container fallback).
    Headless,
}

/// Configuration options for launching a desktop window.
#[derive(Debug, Clone)]
pub struct WindowConfig {
    pub title: String,
    pub width: u32,
    pub height: u32,
    pub headless: bool,
    pub max_frames: Option<usize>,
}

/// Alias for WindowConfig for native desktop window configuration.
pub type DesktopWindowConfig = WindowConfig;

impl Default for WindowConfig {
    fn default() -> Self {
        Self {
            title: "knr Visual Runtime".to_string(),
            width: 900,
            height: 600,
            headless: false,
            max_frames: Some(1),
        }
    }
}

impl WindowConfig {
    /// Creates a new default configuration.
    pub fn new(title: impl Into<String>, width: u32, height: u32) -> Self {
        Self {
            title: title.into(),
            width,
            height,
            headless: false,
            max_frames: Some(1),
        }
    }

    /// Creates an interactive desktop configuration (stays open until user closes it).
    pub fn interactive(title: impl Into<String>, width: u32, height: u32) -> Self {
        Self {
            title: title.into(),
            width,
            height,
            headless: false,
            max_frames: None,
        }
    }

    /// Explicitly forces headless presentation (ideal for tests and CI).
    pub fn with_headless(mut self, headless: bool) -> Self {
        self.headless = headless;
        self
    }

    /// Sets the maximum number of frames to present before exiting.
    pub fn with_max_frames(mut self, frames: usize) -> Self {
        self.max_frames = Some(frames);
        self
    }
}

/// Result metrics returned after running the desktop window launcher.
#[derive(Debug, Clone)]
pub struct WindowLaunchResult {
    pub backend: WindowBackend,
    pub frames_rendered: usize,
    pub final_pixel_hash: u64,
}

/// Returns true if a desktop display server (Wayland/X11/macOS/Windows) is reachable
/// and headless mode was not explicitly requested via `KNR_HEADLESS=1`.
pub fn has_display_server() -> bool {
    if env::var("KNR_HEADLESS").map(|v| v == "1" || v.eq_ignore_ascii_case("true")).unwrap_or(false) {
        return false;
    }
    #[cfg(target_os = "linux")]
    {
        let has_wayland = env::var("WAYLAND_DISPLAY").map(|d| !d.is_empty()).unwrap_or(false);
        let has_x11 = env::var("DISPLAY").map(|d| !d.is_empty()).unwrap_or(false);
        has_wayland || has_x11
    }
    #[cfg(not(target_os = "linux"))]
    {
        true
    }
}

// ============================================================================
// Interactive MVU Desktop Application Runner (`winit` + `softbuffer` + `tiny-skia`)
// ============================================================================

struct InteractiveUiApp<FView, FAction>
where
    FView: FnMut() -> UiNode,
    FAction: FnMut(&str),
{
    config: WindowConfig,
    renderer: UiRenderer,
    view_fn: FView,
    on_action: FAction,
    current_tree: UiNode,
    hit_boxes: Vec<HitBox>,
    cursor_pos: (f32, f32),
    hovered_action: Option<String>,
    window: Option<Rc<Window>>,
    surface: Option<softbuffer::Surface<Rc<Window>, Rc<Window>>>,
    frames_rendered: usize,
    final_pixel_hash: u64,
}

impl<FView, FAction> InteractiveUiApp<FView, FAction>
where
    FView: FnMut() -> UiNode,
    FAction: FnMut(&str),
{
    fn redraw(&mut self) {
        let Some(window) = self.window.as_ref() else {
            return;
        };
        let Some(surface) = self.surface.as_mut() else {
            return;
        };
        let size = window.inner_size();
        let w = size.width.max(1);
        let h = size.height.max(1);

        if let (Some(nw), Some(nh)) = (NonZeroU32::new(w), NonZeroU32::new(h)) {
            if surface.resize(nw, nh).is_err() {
                return;
            }
        }

        let (pixmap, hit_boxes) = self.renderer.render_ui(
            &self.current_tree,
            w,
            h,
            self.hovered_action.as_deref(),
        );
        self.hit_boxes = hit_boxes;
        self.final_pixel_hash = pixmap_fnv_hash(&pixmap);
        self.frames_rendered += 1;

        if let Ok(mut buffer) = surface.buffer_mut() {
            pixmap_to_softbuffer_u32(&pixmap, &mut buffer);
            let _ = buffer.present();
        }
    }
}

impl<FView, FAction> ApplicationHandler for InteractiveUiApp<FView, FAction>
where
    FView: FnMut() -> UiNode,
    FAction: FnMut(&str),
{
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_none() {
            let attrs = Window::default_attributes()
                .with_title(self.config.title.clone())
                .with_inner_size(LogicalSize::new(
                    self.config.width as f64,
                    self.config.height as f64,
                ));
            match event_loop.create_window(attrs) {
                Ok(win) => {
                    let win_rc = Rc::new(win);
                    if let Ok(context) = softbuffer::Context::new(win_rc.clone()) {
                        if let Ok(surface) = softbuffer::Surface::new(&context, win_rc.clone()) {
                            self.surface = Some(surface);
                            self.window = Some(win_rc.clone());
                            win_rc.request_redraw();
                            return;
                        }
                    }
                    event_loop.exit();
                }
                Err(_) => event_loop.exit(),
            }
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        logical_key: Key::Named(NamedKey::Escape),
                        state: ElementState::Pressed,
                        ..
                    },
                ..
            } => event_loop.exit(),
            WindowEvent::Resized(_) => {
                if let Some(w) = &self.window {
                    w.request_redraw();
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor_pos = (position.x as f32, position.y as f32);
                let next_hover = hit_test(&self.hit_boxes, self.cursor_pos.0, self.cursor_pos.1)
                    .map(|s| s.to_string());
                if next_hover != self.hovered_action {
                    if let Some(w) = &self.window {
                        w.set_cursor(if next_hover.is_some() {
                            CursorIcon::Pointer
                        } else {
                            CursorIcon::Default
                        });
                        w.request_redraw();
                    }
                    self.hovered_action = next_hover;
                }
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } => {
                if let Some(action) =
                    hit_test(&self.hit_boxes, self.cursor_pos.0, self.cursor_pos.1)
                        .map(|s| s.to_string())
                {
                    (self.on_action)(&action);
                    self.current_tree = (self.view_fn)();
                    if let Some(w) = &self.window {
                        w.request_redraw();
                    }
                }
            }
            WindowEvent::RedrawRequested => {
                self.redraw();
            }
            _ => {}
        }
    }
}

/// Launches an interactive MVU declarative UI app on the native desktop (`winit` + `softbuffer` + `tiny-skia`),
/// or renders a deterministic off-screen `tiny-skia` frame if headless / no display server is present.
pub fn launch_interactive_ui<FView, FAction>(
    config: WindowConfig,
    mut view_fn: FView,
    on_action: FAction,
) -> Result<WindowLaunchResult, WindowError>
where
    FView: FnMut() -> UiNode,
    FAction: FnMut(&str),
{
    let renderer = UiRenderer::new();
    let initial_tree = view_fn();

    let can_open_window = !config.headless && config.max_frames.is_none() && has_display_server();
    if can_open_window {
        if let Ok(event_loop) = EventLoop::new() {
            event_loop.set_control_flow(ControlFlow::Wait);
            let mut app = InteractiveUiApp {
                config,
                renderer,
                view_fn,
                on_action,
                current_tree: initial_tree,
                hit_boxes: Vec::new(),
                cursor_pos: (0.0, 0.0),
                hovered_action: None,
                window: None,
                surface: None,
                frames_rendered: 0,
                final_pixel_hash: 0,
            };
            if event_loop.run_app(&mut app).is_ok() && app.frames_rendered > 0 {
                return Ok(WindowLaunchResult {
                    backend: WindowBackend::Winit,
                    frames_rendered: app.frames_rendered,
                    final_pixel_hash: app.final_pixel_hash,
                });
            }
            // If window creation failed at runtime, fall through cleanly to headless snapshot
            let (pixmap, _) = app.renderer.render_ui(
                &app.current_tree,
                app.config.width,
                app.config.height,
                None,
            );
            return Ok(WindowLaunchResult {
                backend: WindowBackend::Headless,
                frames_rendered: 1,
                final_pixel_hash: pixmap_fnv_hash(&pixmap),
            });
        }
    }

    // Deterministic Headless Fallback using the exact same tiny-skia + ab_glyph renderer!
    let (pixmap, _) = renderer.render_ui(&initial_tree, config.width, config.height, None);
    Ok(WindowLaunchResult {
        backend: WindowBackend::Headless,
        frames_rendered: 1,
        final_pixel_hash: pixmap_fnv_hash(&pixmap),
    })
}

// ============================================================================
// Canvas Desktop Window Runner (for ExecutionGraph & low-level 2D Canvas)
// ============================================================================

struct CanvasWindowApp {
    title: String,
    canvas: HeadlessCanvas,
    window: Option<Rc<Window>>,
    surface: Option<softbuffer::Surface<Rc<Window>, Rc<Window>>>,
    frames_rendered: usize,
}

impl ApplicationHandler for CanvasWindowApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_none() {
            let (w, h) = self.canvas.canvas().dimensions();
            let attrs = Window::default_attributes()
                .with_title(self.title.clone())
                .with_inner_size(LogicalSize::new(w as f64, h as f64));
            match event_loop.create_window(attrs) {
                Ok(win) => {
                    let win_rc = Rc::new(win);
                    if let Ok(context) = softbuffer::Context::new(win_rc.clone()) {
                        if let Ok(surface) = softbuffer::Surface::new(&context, win_rc.clone()) {
                            self.surface = Some(surface);
                            self.window = Some(win_rc.clone());
                            win_rc.request_redraw();
                            return;
                        }
                    }
                    event_loop.exit();
                }
                Err(_) => event_loop.exit(),
            }
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        logical_key: Key::Named(NamedKey::Escape),
                        state: ElementState::Pressed,
                        ..
                    },
                ..
            } => event_loop.exit(),
            WindowEvent::RedrawRequested => {
                if let (Some(win), Some(surface)) = (&self.window, &mut self.surface) {
                    let size = win.inner_size();
                    let sw = size.width.max(1);
                    let sh = size.height.max(1);
                    if let (Some(nw), Some(nh)) = (NonZeroU32::new(sw), NonZeroU32::new(sh)) {
                        let _ = surface.resize(nw, nh);
                    }
                    if let Ok(mut buf) = surface.buffer_mut() {
                        let src = self.canvas.canvas();
                        let (cw, ch) = src.dimensions();
                        for y in 0..sh {
                            for x in 0..sw {
                                let px = if x < cw && y < ch {
                                    src.pixels()[(y * cw + x) as usize] & 0x00FF_FFFF
                                } else {
                                    0x000B_1120
                                };
                                buf[(y * sw + x) as usize] = px;
                            }
                        }
                        let _ = buf.present();
                        self.frames_rendered += 1;
                    }
                }
            }
            _ => {}
        }
    }
}

/// Launches a native desktop window for a `Canvas` callback.
///
/// Opens a real interactive `winit` + `softbuffer` window when `config.max_frames.is_none()`,
/// `!config.headless`, and a display server is available; otherwise renders deterministically
/// off-screen into a `HeadlessCanvas` (fast & deterministic for CI and tests).
pub fn launch_desktop_window<F>(
    config: WindowConfig,
    mut render_fn: F,
) -> Result<WindowLaunchResult, WindowError>
where
    F: FnMut(&mut Canvas, usize) -> bool,
{
    let mut headless = HeadlessCanvas::new(config.width, config.height);

    let can_open_interactive =
        !config.headless && config.max_frames.is_none() && has_display_server();

    if can_open_interactive {
        render_fn(headless.canvas_mut(), 0);
        let hash = headless.pixel_hash();
        if let Ok(event_loop) = EventLoop::new() {
            event_loop.set_control_flow(ControlFlow::Wait);
            let mut app = CanvasWindowApp {
                title: config.title,
                canvas: headless,
                window: None,
                surface: None,
                frames_rendered: 0,
            };
            if event_loop.run_app(&mut app).is_ok() && app.frames_rendered > 0 {
                return Ok(WindowLaunchResult {
                    backend: WindowBackend::Winit,
                    frames_rendered: app.frames_rendered,
                    final_pixel_hash: hash,
                });
            }
            return Ok(WindowLaunchResult {
                backend: WindowBackend::Headless,
                frames_rendered: 1,
                final_pixel_hash: hash,
            });
        }
    }

    // Deterministic Headless Presentation
    let mut frame_count = 0;
    let max_frames = config.max_frames.unwrap_or(1);

    while frame_count < max_frames {
        let should_continue = render_fn(headless.canvas_mut(), frame_count);
        frame_count += 1;
        if !should_continue {
            break;
        }
    }

    Ok(WindowLaunchResult {
        backend: WindowBackend::Headless,
        frames_rendered: frame_count,
        final_pixel_hash: headless.pixel_hash(),
    })
}

/// Convenience launcher for presenting an `ExecutionGraph` directly in a native desktop window.
pub fn launch_desktop_window_for_graph(
    graph: &mut crate::gui::graph::ExecutionGraph,
    config: &WindowConfig,
) -> Result<WindowLaunchResult, WindowError> {
    graph.compute_layout(config.width, config.height);
    launch_desktop_window(config.clone(), |canvas, _frame| {
        graph.render_to_canvas(canvas);
        false
    })
}

/// Legacy pure-Rust X11 connection handle retained for API compatibility.
pub struct X11Connection {
    stream: UnixStream,
    pub root_window_id: u32,
    pub visual_id: u32,
    pub resource_base: u32,
    pub resource_mask: u32,
    next_resource_id: u32,
}

impl X11Connection {
    pub fn connect() -> Result<Self, WindowError> {
        let display_str = env::var("DISPLAY").unwrap_or_else(|_| ":0".to_string());
        let display_num = display_str
            .strip_prefix(':')
            .and_then(|s| s.split('.').next())
            .unwrap_or("0");
        let socket_path = format!("/tmp/.X11-unix/X{display_num}");
        if !Path::new(&socket_path).exists() {
            return Err(WindowError::X11Rejected(format!(
                "X11 socket {socket_path} does not exist"
            )));
        }
        let mut stream = UnixStream::connect(&socket_path)?;
        stream.set_read_timeout(Some(Duration::from_millis(250)))?;
        stream.set_write_timeout(Some(Duration::from_millis(250)))?;
        let setup_req: [u8; 12] = [0x6c, 0x00, 11, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        stream.write_all(&setup_req)?;
        let mut reply_hdr = [0u8; 8];
        stream.read_exact(&mut reply_hdr)?;
        if reply_hdr[0] != 1 {
            return Err(WindowError::X11Rejected("X11 refused connection".into()));
        }
        Ok(Self {
            stream,
            root_window_id: 0,
            visual_id: 0,
            resource_base: 0,
            resource_mask: 0,
            next_resource_id: 1,
        })
    }

    pub fn alloc_id(&mut self) -> u32 {
        let id = self.resource_base | (self.next_resource_id & self.resource_mask);
        self.next_resource_id += 1;
        id
    }

    pub fn flush(&mut self) -> Result<(), WindowError> {
        self.stream.flush()?;
        Ok(())
    }
}
