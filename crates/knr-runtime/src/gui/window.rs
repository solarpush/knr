//! Native cross-platform desktop window presentation (Wayland / X11 / macOS / Windows)
//! powered by:
//! 1. Primary: `wgpu 23.0` Hardware GPU UI & Custom `.knr` WGSL Shader Engine (`WindowBackend::Wgpu`)
//! 2. Automatic Fallback: `softbuffer 0.4` + `tiny-skia` + `ab_glyph` CPU renderer (`WindowBackend::Winit`)
//! 3. Headless CI Fallback: deterministic multi-frame off-screen `tiny-skia` engine (`WindowBackend::Headless`)

use std::collections::HashMap;
use std::env;
use std::io::{Read, Write};
use std::num::NonZeroU32;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use thiserror::Error;
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, KeyEvent, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::{CursorIcon, Window, WindowId};

use crate::gui::anim::UiAnimator;
use crate::gui::canvas::{Canvas, HeadlessCanvas};
#[cfg(feature = "gpu")]
use crate::gui::gpu::GpuUiEngine;
use crate::gui::renderer::{
    pixmap_fnv_hash, pixmap_to_softbuffer_u32_scaled, RenderInteractionState, UiRenderer,
};
use crate::gui::ui_tree::{
    compute_ui_layout, hit_test_box, HitBox, LayoutNode, LayoutRect, UiKind, UiNode,
};

/// Safe frame pacing ceilings (60 FPS VSync — never busy-spin).
const GPU_FRAME_INTERVAL: Duration = Duration::from_micros(16_667);
const CPU_FRAME_INTERVAL: Duration = Duration::from_micros(16_667);
/// Hot-reload polling cadence when live reload is active (~0% CPU when idle).
const RELOAD_POLL_INTERVAL: Duration = Duration::from_millis(100);
const RELOAD_IDLE_WAIT: Duration = Duration::from_millis(120);

fn offset_layout_y(node: &mut LayoutNode, dy: f32) {
    node.rect.y += dy;
    for ch in &mut node.children {
        offset_layout_y(ch, dy);
    }
}

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
    /// Hardware-accelerated GPU rendering via `wgpu 23.0` (Vulkan / Metal / DX12 / GL).
    Wgpu,
    /// Cross-platform native desktop window via `winit` + `softbuffer` + `tiny-skia` CPU fallback.
    Winit,
    /// Legacy X11 backend variant retained for backwards compatibility.
    X11,
    /// Deterministic headless software framebuffer (CI/container fallback).
    Headless,
}

/// Interactive UI events dispatched from native window input back to `.knr` `update()`.
#[derive(Debug, Clone)]
pub enum UiEvent {
    Click(String),
    TextInput { action: String, value: String },
    SliderChange { action: String, value: f64 },
    Toggle { action: String, checked: bool },
}

/// Live hot-reload payload delivered to an open desktop window without closing it.
#[derive(Debug, Clone)]
pub struct HotReloadUpdate {
    pub tree: UiNode,
    pub shaders_wgsl: HashMap<String, String>,
    pub has_tick: bool,
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
    pub fn new(title: impl Into<String>, width: u32, height: u32) -> Self {
        Self {
            title: title.into(),
            width,
            height,
            headless: false,
            max_frames: Some(1),
        }
    }

    pub fn interactive(title: impl Into<String>, width: u32, height: u32) -> Self {
        Self {
            title: title.into(),
            width,
            height,
            headless: false,
            max_frames: None,
        }
    }

    pub fn with_headless(mut self, headless: bool) -> Self {
        self.headless = headless;
        self
    }

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
    if env::var("KNR_HEADLESS")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
    {
        return false;
    }
    #[cfg(target_os = "linux")]
    {
        let has_wayland = env::var("WAYLAND_DISPLAY")
            .map(|d| !d.is_empty())
            .unwrap_or(false);
        let has_x11 = env::var("DISPLAY").map(|d| !d.is_empty()).unwrap_or(false);
        has_wayland || has_x11
    }
    #[cfg(not(target_os = "linux"))]
    {
        true
    }
}

// ============================================================================
// Interactive MVU + 60/120 FPS Animation + GPU Shader Desktop Runner
// ============================================================================

struct InteractiveUiApp<FView, FEvent, FTick, FReload>
where
    FView: FnMut() -> UiNode,
    FEvent: FnMut(UiEvent),
    FTick: FnMut(f64, f64),
    FReload: FnMut() -> Option<HotReloadUpdate>,
{
    config: WindowConfig,
    renderer: UiRenderer,
    animator: UiAnimator,
    #[cfg_attr(not(feature = "gpu"), allow(dead_code))]
    shaders_wgsl: HashMap<String, String>,
    has_tick: bool,
    has_reload: bool,
    view_fn: FView,
    on_event: FEvent,
    on_tick: FTick,
    poll_reload: FReload,
    current_tree: UiNode,
    cached_layout: Option<LayoutNode>,
    layout_dirty: bool,
    last_layout_key: (u32, u32, i32), // (logical_w, logical_h, scroll_y_px)
    hit_boxes: Vec<HitBox>,
    cursor_pos: (f32, f32), // always in logical pixels
    scale_factor: f32,
    hovered_action: Option<String>,
    focused_input: Option<(String, String)>, // (action, current_text)
    dragging_slider: Option<(String, LayoutRect)>,
    scroll_y: f32,
    content_height: f32,
    window: Option<Arc<Window>>,
    #[cfg(feature = "gpu")]
    gpu: Option<GpuUiEngine>,
    cpu_surface: Option<softbuffer::Surface<Arc<Window>, Arc<Window>>>,
    cpu_pixmap: Option<tiny_skia::Pixmap>,
    cpu_ui_dirty: bool,
    active_backend: WindowBackend,
    start_time: Instant,
    last_frame: Instant,
    next_frame_at: Instant,
    last_reload_check: Instant,
    frames_rendered: usize,
    final_pixel_hash: u64,
}

impl<FView, FEvent, FTick, FReload> InteractiveUiApp<FView, FEvent, FTick, FReload>
where
    FView: FnMut() -> UiNode,
    FEvent: FnMut(UiEvent),
    FTick: FnMut(f64, f64),
    FReload: FnMut() -> Option<HotReloadUpdate>,
{
    fn needs_continuous_redraw(&self) -> bool {
        self.has_tick
            || self.animator.has_active_transitions()
            || self.current_tree.contains_shader()
    }

    fn frame_interval(&self) -> Duration {
        if self.active_backend == WindowBackend::Wgpu {
            GPU_FRAME_INTERVAL
        } else {
            CPU_FRAME_INTERVAL
        }
    }

    fn redraw(&mut self) {
        let Some(window) = self.window.as_ref() else {
            return;
        };
        let size = window.inner_size();
        // Skip minimized / 0x0 Wayland frames cleanly
        if size.width == 0 || size.height == 0 {
            return;
        }
        let phys_w = size.width;
        let phys_h = size.height;
        let scale = (window.scale_factor() as f32).clamp(0.5, 4.0);
        self.scale_factor = scale;
        let logical_w = (phys_w as f32 / scale).max(1.0);
        let logical_h = (phys_h as f32 / scale).max(1.0);

        let now = Instant::now();
        let dt = now
            .duration_since(self.last_frame)
            .as_secs_f64()
            .clamp(0.0001, 0.05);
        self.last_frame = now;
        self.next_frame_at = now + self.frame_interval();
        let elapsed = now.duration_since(self.start_time).as_secs_f64();

        if self.has_tick {
            (self.on_tick)(dt, elapsed);
            let next_tree = (self.view_fn)();
            if next_tree != self.current_tree {
                self.current_tree = next_tree;
                self.layout_dirty = true;
                self.cpu_ui_dirty = true;
            }
        }

        // Recompute layout ONLY when tree, window logical size, or scroll position changed!
        let max_scroll = (self.content_height - logical_h).max(0.0);
        self.scroll_y = self.scroll_y.clamp(0.0, max_scroll);
        let layout_key = (
            logical_w.round() as u32,
            logical_h.round() as u32,
            self.scroll_y.round() as i32,
        );
        if self.layout_dirty || self.cached_layout.is_none() || self.last_layout_key != layout_key {
            let (mut laid, mut hit_boxes) =
                compute_ui_layout(&self.current_tree, logical_w, logical_h, &self.renderer);
            self.content_height = laid.rect.h;
            let clamped_scroll = self.scroll_y.clamp(0.0, (self.content_height - logical_h).max(0.0));
            self.scroll_y = clamped_scroll;
            if clamped_scroll > 0.5 {
                offset_layout_y(&mut laid, -clamped_scroll);
                for hb in &mut hit_boxes {
                    hb.rect.y -= clamped_scroll;
                }
            }
            self.hit_boxes = hit_boxes;
            self.cached_layout = Some(laid);
            self.last_layout_key = (
                logical_w.round() as u32,
                logical_h.round() as u32,
                clamped_scroll.round() as i32,
            );
            self.layout_dirty = false;
            self.cpu_ui_dirty = true;
        }

        let mut frame_layout = self.cached_layout.as_ref().unwrap().clone();
        self.animator
            .step_and_apply(&mut frame_layout, self.hovered_action.as_deref(), dt as f32);
        if self.animator.has_active_transitions() {
            self.cpu_ui_dirty = true;
        }

        let state = RenderInteractionState {
            hovered_action: self.hovered_action.as_deref(),
            focused_action: self.focused_input.as_ref().map(|(a, _)| a.as_str()),
            elapsed_sec: elapsed as f32,
        };

        #[cfg(feature = "gpu")]
        if let Some(gpu) = self.gpu.as_mut() {
            gpu.resize(phys_w, phys_h);
            if gpu
                .render_frame(
                    &frame_layout,
                    &self.renderer,
                    &state,
                    self.cursor_pos,
                    dt as f32,
                    (logical_w, logical_h),
                    scale,
                )
                .is_ok()
            {
                self.frames_rendered += 1;
                return;
            }
        }

        // Fast CPU fallback: render static UI only when dirty, and incrementally repaint shaders!
        if let Some(surface) = self.cpu_surface.as_mut() {
            if let (Some(nw), Some(nh)) = (NonZeroU32::new(phys_w), NonZeroU32::new(phys_h)) {
                if surface.resize(nw, nh).is_err() {
                    return;
                }
            }
            let pm_w = (logical_w.round() as u32).max(1);
            let pm_h = (logical_h.round() as u32).max(1);
            let need_new_pm = self
                .cpu_pixmap
                .as_ref()
                .is_none_or(|pm| pm.width() != pm_w || pm.height() != pm_h);
            if need_new_pm {
                self.cpu_pixmap = tiny_skia::Pixmap::new(pm_w, pm_h);
                self.cpu_ui_dirty = true;
            }
            if let Some(pixmap) = self.cpu_pixmap.as_mut() {
                if self.cpu_ui_dirty {
                    self.renderer
                        .render_layout_into(pixmap, &frame_layout, &state);
                    self.cpu_ui_dirty = false;
                } else {
                    self.renderer
                        .render_shaders_only(pixmap, &frame_layout, state.elapsed_sec);
                }
                self.frames_rendered += 1;
                if let Ok(mut buffer) = surface.buffer_mut() {
                    pixmap_to_softbuffer_u32_scaled(pixmap, &mut buffer, phys_w, phys_h);
                    let _ = buffer.present();
                }
            }
        }
    }

    fn dispatch_and_refresh(&mut self, ev: UiEvent) {
        (self.on_event)(ev);
        self.current_tree = (self.view_fn)();
        self.layout_dirty = true;
        self.cpu_ui_dirty = true;
        if let Some(w) = &self.window {
            w.request_redraw();
        }
    }

    fn apply_hot_reload(&mut self, update: HotReloadUpdate) {
        self.current_tree = update.tree;
        self.has_tick = update.has_tick;
        self.layout_dirty = true;
        self.cpu_ui_dirty = true;
        self.shaders_wgsl = update.shaders_wgsl;

        #[cfg(feature = "gpu")]
        if let Some(gpu) = self.gpu.as_mut() {
            for (name, wgsl) in &self.shaders_wgsl {
                gpu.register_shader(name, wgsl);
            }
        }

        if let Some(w) = &self.window {
            w.request_redraw();
        }
    }
}

impl<FView, FEvent, FTick, FReload> ApplicationHandler
    for InteractiveUiApp<FView, FEvent, FTick, FReload>
where
    FView: FnMut() -> UiNode,
    FEvent: FnMut(UiEvent),
    FTick: FnMut(f64, f64),
    FReload: FnMut() -> Option<HotReloadUpdate>,
{
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_none() {
            let attrs = Window::default_attributes()
                .with_title(self.config.title.clone())
                .with_resizable(true)
                .with_min_inner_size(LogicalSize::new(640.0, 480.0))
                .with_inner_size(LogicalSize::new(
                    self.config.width as f64,
                    self.config.height as f64,
                ));
            match event_loop.create_window(attrs) {
                Ok(win) => {
                    let win_arc = Arc::new(win);
                    self.scale_factor = (win_arc.scale_factor() as f32).clamp(0.5, 4.0);
                    #[cfg(feature = "gpu")]
                    {
                        let size = win_arc.inner_size();
                        let w = if size.width > 0 {
                            size.width
                        } else {
                            (self.config.width as f32 * self.scale_factor).round() as u32
                        }
                        .max(1);
                        let h = if size.height > 0 {
                            size.height
                        } else {
                            (self.config.height as f32 * self.scale_factor).round() as u32
                        }
                        .max(1);
                        let force_cpu = env::var("KNR_FORCE_CPU")
                            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
                            .unwrap_or(false);

                        // Try primary `wgpu 23` GPU engine first
                        if !force_cpu {
                            if let Some(gpu_engine) =
                                GpuUiEngine::try_new(win_arc.clone(), w, h, &self.shaders_wgsl)
                            {
                                eprintln!(
                                    "[knr-gui] Backend: GPU (wgpu) | scale={:.2}x | 60 FPS VSync",
                                    self.scale_factor
                                );
                                self.gpu = Some(gpu_engine);
                                self.active_backend = WindowBackend::Wgpu;
                                self.window = Some(win_arc.clone());
                                win_arc.request_redraw();
                                return;
                            }
                        }
                    }

                    // Seamless CPU `softbuffer` + `tiny-skia` fallback
                    if let Ok(context) = softbuffer::Context::new(win_arc.clone()) {
                        if let Ok(surface) = softbuffer::Surface::new(&context, win_arc.clone()) {
                            eprintln!(
                                "[knr-gui] Backend: CPU (softbuffer + tiny-skia) | scale={:.2}x",
                                self.scale_factor
                            );
                            self.cpu_surface = Some(surface);
                            self.active_backend = WindowBackend::Winit;
                            self.window = Some(win_arc.clone());
                            win_arc.request_redraw();
                            return;
                        }
                    }
                    event_loop.exit();
                }
                Err(_) => event_loop.exit(),
            }
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let now = Instant::now();
        if self.has_reload && now.duration_since(self.last_reload_check) >= RELOAD_POLL_INTERVAL {
            self.last_reload_check = now;
            if let Some(update) = (self.poll_reload)() {
                self.apply_hot_reload(update);
            }
        }

        if self.needs_continuous_redraw() {
            let now = Instant::now();
            if now >= self.next_frame_at {
                self.next_frame_at = now + self.frame_interval();
                if let Some(w) = &self.window {
                    w.request_redraw();
                }
            }
            // Paced WaitUntil (never busy-spin ControlFlow::Poll!)
            event_loop.set_control_flow(ControlFlow::WaitUntil(self.next_frame_at));
        } else if self.has_reload {
            event_loop.set_control_flow(ControlFlow::WaitUntil(Instant::now() + RELOAD_IDLE_WAIT));
        } else {
            event_loop.set_control_flow(ControlFlow::Wait);
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
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                self.scale_factor = (scale_factor as f32).clamp(0.5, 4.0);
                self.layout_dirty = true;
                if let Some(w) = &self.window {
                    w.request_redraw();
                }
            }
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        logical_key,
                        state: ElementState::Pressed,
                        text,
                        ..
                    },
                ..
            } => match logical_key {
                Key::Named(NamedKey::Escape) => {
                    if self.focused_input.is_some() {
                        self.focused_input = None;
                        self.cpu_ui_dirty = true;
                        if let Some(w) = &self.window {
                            w.request_redraw();
                        }
                    } else {
                        event_loop.exit();
                    }
                }
                Key::Named(NamedKey::Enter) => {
                    if self.focused_input.is_some() {
                        self.focused_input = None;
                        self.cpu_ui_dirty = true;
                        if let Some(w) = &self.window {
                            w.request_redraw();
                        }
                    }
                }
                Key::Named(NamedKey::Backspace) => {
                    if let Some((act, mut buf)) = self.focused_input.clone() {
                        buf.pop();
                        self.focused_input = Some((act.clone(), buf.clone()));
                        self.dispatch_and_refresh(UiEvent::TextInput {
                            action: act,
                            value: buf,
                        });
                    }
                }
                _ => {
                    if let Some((act, mut buf)) = self.focused_input.clone() {
                        if let Some(txt) = text {
                            let clean: String = txt.chars().filter(|c| !c.is_control()).collect();
                            if !clean.is_empty() {
                                buf.push_str(&clean);
                                self.focused_input = Some((act.clone(), buf.clone()));
                                self.dispatch_and_refresh(UiEvent::TextInput {
                                    action: act,
                                    value: buf,
                                });
                            }
                        }
                    }
                }
            },
            WindowEvent::Resized(new_size) => {
                if new_size.width == 0 || new_size.height == 0 {
                    return;
                }
                #[cfg(feature = "gpu")]
                if let Some(gpu) = self.gpu.as_mut() {
                    gpu.resize(new_size.width, new_size.height);
                }
                self.layout_dirty = true;
                if let Some(w) = &self.window {
                    w.request_redraw();
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let dy = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y * 36.0,
                    MouseScrollDelta::PixelDelta(pos) => (pos.y as f32) / self.scale_factor.max(0.5),
                };
                self.scroll_y = (self.scroll_y - dy).max(0.0);
                self.layout_dirty = true;
                if let Some(w) = &self.window {
                    w.request_redraw();
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                // Convert physical cursor coordinates -> logical UI coordinates
                let inv = 1.0 / self.scale_factor.max(0.5);
                self.cursor_pos = (position.x as f32 * inv, position.y as f32 * inv);

                // Live slider drag
                if let Some((act, rect)) = self.dragging_slider.clone() {
                    let norm = ((self.cursor_pos.0 - rect.x) / rect.w.max(1.0)).clamp(0.0, 1.0);
                    self.dispatch_and_refresh(UiEvent::SliderChange {
                        action: act,
                        value: norm as f64,
                    });
                    return;
                }

                let hit = hit_test_box(&self.hit_boxes, self.cursor_pos.0, self.cursor_pos.1);
                let next_hover = hit.map(|h| h.action.clone());
                let cursor_icon = match hit.map(|h| h.kind) {
                    Some(UiKind::Input) => CursorIcon::Text,
                    Some(_) => CursorIcon::Pointer,
                    None => CursorIcon::Default,
                };
                if next_hover != self.hovered_action {
                    self.cpu_ui_dirty = true;
                    if let Some(w) = &self.window {
                        w.set_cursor(cursor_icon);
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
                let hit =
                    hit_test_box(&self.hit_boxes, self.cursor_pos.0, self.cursor_pos.1).cloned();
                match hit {
                    Some(hb) => match hb.kind {
                        UiKind::Input => {
                            self.focused_input = Some((hb.action.clone(), hb.text.clone()));
                            self.cpu_ui_dirty = true;
                            if let Some(w) = &self.window {
                                w.request_redraw();
                            }
                        }
                        UiKind::Slider => {
                            self.focused_input = None;
                            let norm = ((self.cursor_pos.0 - hb.rect.x) / hb.rect.w.max(1.0))
                                .clamp(0.0, 1.0);
                            self.dragging_slider = Some((hb.action.clone(), hb.rect));
                            self.dispatch_and_refresh(UiEvent::SliderChange {
                                action: hb.action,
                                value: norm as f64,
                            });
                        }
                        UiKind::Toggle => {
                            self.focused_input = None;
                            self.dispatch_and_refresh(UiEvent::Toggle {
                                action: hb.action,
                                checked: hb.value <= 0.5,
                            });
                        }
                        _ => {
                            self.focused_input = None;
                            self.dispatch_and_refresh(UiEvent::Click(hb.action));
                        }
                    },
                    None => {
                        if self.focused_input.is_some() {
                            self.focused_input = None;
                            self.cpu_ui_dirty = true;
                            if let Some(w) = &self.window {
                                w.request_redraw();
                            }
                        }
                    }
                }
            }
            WindowEvent::MouseInput {
                state: ElementState::Released,
                button: MouseButton::Left,
                ..
            } => {
                self.dragging_slider = None;
            }
            WindowEvent::RedrawRequested => {
                self.redraw();
            }
            _ => {}
        }
    }
}

/// Launches a full-featured GPU-accelerated MVU + 60/120 FPS animation + `.knr` shader app
/// with live hot-reload support (reloads UI tree and recompiles WGSL shaders without closing
/// the desktop window).
pub fn launch_ui_app_with_reload<FView, FEvent, FTick, FReload>(
    config: WindowConfig,
    shaders_wgsl: HashMap<String, String>,
    has_tick: bool,
    view_fn: FView,
    on_event: FEvent,
    on_tick: FTick,
    poll_reload: FReload,
) -> Result<WindowLaunchResult, WindowError>
where
    FView: FnMut() -> UiNode,
    FEvent: FnMut(UiEvent),
    FTick: FnMut(f64, f64),
    FReload: FnMut() -> Option<HotReloadUpdate>,
{
    launch_ui_app_inner(
        config,
        shaders_wgsl,
        has_tick,
        view_fn,
        on_event,
        on_tick,
        true,
        poll_reload,
    )
}

/// Launches a full-featured GPU-accelerated MVU + 60/120 FPS animation + `.knr` shader app
/// (`wgpu 23` -> `softbuffer` + `tiny-skia` -> deterministic headless fallback).
pub fn launch_ui_app<FView, FEvent, FTick>(
    config: WindowConfig,
    shaders_wgsl: HashMap<String, String>,
    has_tick: bool,
    view_fn: FView,
    on_event: FEvent,
    on_tick: FTick,
) -> Result<WindowLaunchResult, WindowError>
where
    FView: FnMut() -> UiNode,
    FEvent: FnMut(UiEvent),
    FTick: FnMut(f64, f64),
{
    launch_ui_app_inner(
        config,
        shaders_wgsl,
        has_tick,
        view_fn,
        on_event,
        on_tick,
        false,
        || None,
    )
}

#[allow(clippy::too_many_arguments)]
fn launch_ui_app_inner<FView, FEvent, FTick, FReload>(
    config: WindowConfig,
    shaders_wgsl: HashMap<String, String>,
    mut has_tick: bool,
    mut view_fn: FView,
    on_event: FEvent,
    mut on_tick: FTick,
    has_reload: bool,
    mut poll_reload: FReload,
) -> Result<WindowLaunchResult, WindowError>
where
    FView: FnMut() -> UiNode,
    FEvent: FnMut(UiEvent),
    FTick: FnMut(f64, f64),
    FReload: FnMut() -> Option<HotReloadUpdate>,
{
    let renderer = UiRenderer::new();
    let initial_tree = view_fn();

    let can_open_window = !config.headless && config.max_frames.is_none() && has_display_server();
    if can_open_window {
        if let Ok(event_loop) = EventLoop::new() {
            event_loop.set_control_flow(ControlFlow::Wait);
            let now = Instant::now();
            let mut app = InteractiveUiApp {
                config,
                renderer,
                animator: UiAnimator::new(),
                shaders_wgsl,
                has_tick,
                has_reload,
                view_fn,
                on_event,
                on_tick,
                poll_reload,
                current_tree: initial_tree,
                cached_layout: None,
                layout_dirty: true,
                last_layout_key: (0, 0, 0),
                hit_boxes: Vec::new(),
                cursor_pos: (0.0, 0.0),
                scale_factor: 1.0,
                hovered_action: None,
                focused_input: None,
                dragging_slider: None,
                scroll_y: 0.0,
                content_height: 0.0,
                window: None,
                #[cfg(feature = "gpu")]
                gpu: None,
                cpu_surface: None,
                cpu_pixmap: None,
                cpu_ui_dirty: true,
                active_backend: WindowBackend::Headless,
                start_time: now,
                last_frame: now,
                next_frame_at: now + GPU_FRAME_INTERVAL,
                last_reload_check: now,
                frames_rendered: 0,
                final_pixel_hash: 0,
            };
            if event_loop.run_app(&mut app).is_ok() && app.frames_rendered > 0 {
                return Ok(WindowLaunchResult {
                    backend: app.active_backend,
                    frames_rendered: app.frames_rendered,
                    final_pixel_hash: app.final_pixel_hash,
                });
            }
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

    // Deterministic Multi-Frame Headless Fallback
    let frames = config.max_frames.unwrap_or(1).max(1);
    let mut animator = UiAnimator::new();
    let mut tree = initial_tree;
    let mut last_hash = 0u64;
    let dt = 1.0 / 60.0;

    for f in 0..frames {
        let elapsed = f as f64 * dt;
        if has_reload {
            if let Some(update) = poll_reload() {
                tree = update.tree;
                has_tick = update.has_tick;
            }
        }
        if f > 0 && has_tick {
            on_tick(dt, elapsed);
            tree = view_fn();
        }
        let (mut laid, _) =
            compute_ui_layout(&tree, config.width as f32, config.height as f32, &renderer);
        animator.step_and_apply(&mut laid, None, dt as f32);
        let state = RenderInteractionState {
            hovered_action: None,
            focused_action: None,
            elapsed_sec: elapsed as f32,
        };
        let pixmap = renderer.render_layout(&laid, config.width, config.height, &state);
        last_hash = pixmap_fnv_hash(&pixmap);
    }

    Ok(WindowLaunchResult {
        backend: WindowBackend::Headless,
        frames_rendered: frames,
        final_pixel_hash: last_hash,
    })
}

/// Backward-compatible launcher for simple MVU apps (`init` + `update` + `view` without `tick` or shaders).
pub fn launch_interactive_ui<FView, FAction>(
    config: WindowConfig,
    view_fn: FView,
    mut on_action: FAction,
) -> Result<WindowLaunchResult, WindowError>
where
    FView: FnMut() -> UiNode,
    FAction: FnMut(&str),
{
    launch_ui_app(
        config,
        HashMap::new(),
        false,
        view_fn,
        move |ev| match ev {
            UiEvent::Click(a) => on_action(&a),
            UiEvent::Toggle { action, .. } => on_action(&action),
            UiEvent::TextInput { action, value } => on_action(&format!("{action}:{value}")),
            UiEvent::SliderChange { action, value } => on_action(&format!("{action}:{value:.3}")),
        },
        |_dt, _elapsed| {},
    )
}

// ============================================================================
// Canvas Desktop Window Runner (for ExecutionGraph & low-level 2D Canvas)
// ============================================================================

struct CanvasWindowApp {
    title: String,
    canvas: HeadlessCanvas,
    window: Option<Arc<Window>>,
    surface: Option<softbuffer::Surface<Arc<Window>, Arc<Window>>>,
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
                    let win_arc = Arc::new(win);
                    if let Ok(context) = softbuffer::Context::new(win_arc.clone()) {
                        if let Ok(surface) = softbuffer::Surface::new(&context, win_arc.clone()) {
                            self.surface = Some(surface);
                            self.window = Some(win_arc.clone());
                            win_arc.request_redraw();
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
