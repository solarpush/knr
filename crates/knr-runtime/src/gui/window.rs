//! Native cross-platform desktop window presentation (Wayland / X11 / macOS / Windows)
//! powered by:
//! 1. Primary: `wgpu 23.0` Hardware GPU UI & Custom `.knr` WGSL Shader Engine (`WindowBackend::Wgpu`)
//! 2. Automatic Fallback: `softbuffer 0.4` + `tiny-skia` + `ab_glyph` CPU renderer (`WindowBackend::Winit`)
//! 3. Headless CI Fallback: deterministic multi-frame off-screen `tiny-skia` engine (`WindowBackend::Headless`)

use std::collections::{BTreeMap, HashMap};
use std::env;
use std::io::{Read, Write};
use std::num::NonZeroU32;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use thiserror::Error;
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, KeyEvent, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::window::{CursorIcon, Window, WindowId};

use crate::gui::anim::UiAnimator;
use crate::gui::canvas::{Canvas, HeadlessCanvas};
#[cfg(feature = "gpu")]
use crate::gui::gpu::GpuUiEngine;
use crate::gui::renderer::{
    pixmap_fnv_hash, pixmap_to_softbuffer_u32_scaled, RenderInteractionState, UiRenderer,
};
use crate::gui::ui_tree::{
    compute_ui_layout_with_scroll, hit_test_box, hit_test_scroll_targets, HitBox, LayoutNode,
    LayoutRect, UiKind, UiNode,
};
use crate::platform::Value;

/// Safe frame pacing ceilings (60 FPS VSync — never busy-spin).
const GPU_FRAME_INTERVAL: Duration = Duration::from_micros(16_667);
const CPU_FRAME_INTERVAL: Duration = Duration::from_micros(16_667);
/// Hot-reload polling cadence when live reload is active (~0% CPU when idle).
const RELOAD_POLL_INTERVAL: Duration = Duration::from_millis(100);
const RELOAD_IDLE_WAIT: Duration = Duration::from_millis(120);
/// Async effect channel wake interval when idle (low CPU, responsive timer/HTTP delivery).
const EFFECT_IDLE_WAIT: Duration = Duration::from_millis(16);

static STORAGE_LOCK: Mutex<()> = Mutex::new(());

fn offset_layout_y(node: &mut LayoutNode, dy: f32) {
    node.rect.y += dy;
    for ch in &mut node.children {
        offset_layout_y(ch, dy);
    }
}

fn char_to_byte_idx(s: &str, char_idx: usize) -> usize {
    s.char_indices()
        .nth(char_idx)
        .map(|(b, _)| b)
        .unwrap_or(s.len())
}

fn ordered_selection(
    caret: usize,
    anchor: Option<usize>,
    char_len: usize,
) -> Option<(usize, usize)> {
    let a = anchor?.min(char_len);
    let c = caret.min(char_len);
    if a == c {
        None
    } else if a < c {
        Some((a, c))
    } else {
        Some((c, a))
    }
}

fn delete_selection_range(
    buf: &mut String,
    caret: &mut usize,
    anchor: &mut Option<usize>,
) -> bool {
    let char_len = buf.chars().count();
    let Some((s, e)) = ordered_selection(*caret, *anchor, char_len) else {
        *anchor = None;
        return false;
    };
    let sb = char_to_byte_idx(buf, s);
    let eb = char_to_byte_idx(buf, e);
    buf.replace_range(sb..eb, "");
    *caret = s;
    *anchor = None;
    true
}

/// Deletes the word immediately preceding `caret` inside `buf` (`Ctrl+Backspace`).
fn delete_word_before_caret(buf: &mut String, caret: &mut usize) {
    let chars: Vec<char> = buf.chars().collect();
    let end = (*caret).min(chars.len());
    if end == 0 {
        return;
    }
    let mut start = end;
    while start > 0 && chars[start - 1].is_whitespace() {
        start -= 1;
    }
    if start > 0 {
        let is_word_char = |c: char| c.is_alphanumeric() || c == '_';
        if is_word_char(chars[start - 1]) {
            while start > 0 && is_word_char(chars[start - 1]) {
                start -= 1;
            }
        } else {
            while start > 0 && !chars[start - 1].is_whitespace() && !is_word_char(chars[start - 1])
            {
                start -= 1;
            }
        }
    }
    let sb = char_to_byte_idx(buf, start);
    let eb = char_to_byte_idx(buf, end);
    buf.replace_range(sb..eb, "");
    *caret = start;
}

fn find_input_node<'a>(node: &'a UiNode, action: &str) -> Option<&'a UiNode> {
    if node.kind == UiKind::Input && node.action == action {
        return Some(node);
    }
    for ch in &node.children {
        if let Some(found) = find_input_node(ch, action) {
            return Some(found);
        }
    }
    None
}

fn find_widget_key_by_action(node: &UiNode, action: &str) -> Option<String> {
    if !action.is_empty() && node.action == action {
        if !node.key.is_empty() {
            return Some(node.key.clone());
        }
        return Some(node.action.clone());
    }
    for ch in &node.children {
        if let Some(found) = find_widget_key_by_action(ch, action) {
            return Some(found);
        }
    }
    None
}

/// Formats a normalized keyboard shortcut / key press name such as `"Ctrl+K"`, `"Ctrl+Shift+P"`,
/// `"Escape"`, `"Enter"`, `"ArrowDown"`, or `"Shift+Tab"`.
pub fn format_key_combo(base_key: &str, ctrl: bool, shift: bool, alt: bool) -> String {
    let trimmed = base_key.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    let normalized = if trimmed.len() == 1 && (ctrl || alt) {
        trimmed.to_ascii_uppercase()
    } else if trimmed == " " {
        "Space".to_string()
    } else {
        trimmed.to_string()
    };
    let mut parts: Vec<&str> = Vec::with_capacity(4);
    if ctrl {
        parts.push("Ctrl");
    }
    if alt {
        parts.push("Alt");
    }
    let is_named = normalized.len() > 1;
    if shift && (ctrl || alt || is_named) {
        parts.push("Shift");
    }
    parts.push(&normalized);
    parts.join("+")
}

fn winit_key_base_name(logical_key: &Key) -> Option<String> {
    match logical_key {
        Key::Named(named) => match named {
            NamedKey::Escape => Some("Escape".into()),
            NamedKey::Enter => Some("Enter".into()),
            NamedKey::Tab => Some("Tab".into()),
            NamedKey::Space => Some("Space".into()),
            NamedKey::Backspace => Some("Backspace".into()),
            NamedKey::Delete => Some("Delete".into()),
            NamedKey::ArrowUp => Some("ArrowUp".into()),
            NamedKey::ArrowDown => Some("ArrowDown".into()),
            NamedKey::ArrowLeft => Some("ArrowLeft".into()),
            NamedKey::ArrowRight => Some("ArrowRight".into()),
            NamedKey::Home => Some("Home".into()),
            NamedKey::End => Some("End".into()),
            NamedKey::PageUp => Some("PageUp".into()),
            NamedKey::PageDown => Some("PageDown".into()),
            NamedKey::F1 => Some("F1".into()),
            NamedKey::F2 => Some("F2".into()),
            NamedKey::F3 => Some("F3".into()),
            NamedKey::F4 => Some("F4".into()),
            NamedKey::F5 => Some("F5".into()),
            NamedKey::F6 => Some("F6".into()),
            NamedKey::F7 => Some("F7".into()),
            NamedKey::F8 => Some("F8".into()),
            NamedKey::F9 => Some("F9".into()),
            NamedKey::F10 => Some("F10".into()),
            NamedKey::F11 => Some("F11".into()),
            NamedKey::F12 => Some("F12".into()),
            _ => None,
        },
        Key::Character(ch) => {
            let s = ch.as_str();
            if s.is_empty() || s.chars().all(char::is_control) {
                None
            } else if s == " " {
                Some("Space".into())
            } else {
                Some(s.to_string())
            }
        }
        _ => None,
    }
}

fn ui_storage_file_path() -> PathBuf {
    if let Ok(custom) = env::var("KNR_STORAGE_PATH") {
        let trimmed = custom.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
        }
    }
    env::temp_dir().join("knr_ui_storage.json")
}

fn read_storage_map(path: &Path) -> BTreeMap<String, String> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|raw| serde_json::from_str::<BTreeMap<String, String>>(&raw).ok())
        .unwrap_or_default()
}

fn write_storage_map(path: &Path, map: &BTreeMap<String, String>) {
    if let Some(parent) = path
        .parent()
        .filter(|dir| !dir.as_os_str().is_empty() && !dir.exists())
    {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(serialized) = serde_json::to_string_pretty(map) {
        let _ = std::fs::write(path, serialized.as_bytes());
    }
}

fn cmd_val_to_string(v: &Value) -> String {
    match v {
        Value::Str(s) => s.clone(),
        Value::I64(n) => n.to_string(),
        Value::F64(f) => f.to_string(),
        Value::Bool(b) => b.to_string(),
        _ => String::new(),
    }
}

fn cmd_val_to_f64(v: &Value) -> f64 {
    match v {
        Value::F64(f) => *f,
        Value::I64(n) => *n as f64,
        _ => 0.0,
    }
}

/// Declarative asynchronous MVU side-effect command parsed from `.knr` 5-field `Cmd` structs.
#[derive(Debug, Clone, PartialEq)]
pub struct UiCmd {
    pub kind: String,
    pub target: String,
    pub payload: String,
    pub on_result: String,
    pub delay_ms: f64,
}

impl UiCmd {
    /// Parses a 5-field `Cmd` struct `[kind, target, payload, on_result, delay_ms]` (or map).
    /// Returns `None` when `kind` is empty or `"none"`.
    pub fn from_value(val: &Value) -> Option<Self> {
        let (kind, target, payload, on_result, delay_ms) = match val {
            Value::Struct { fields, .. } if fields.len() >= 5 => (
                cmd_val_to_string(&fields[0]),
                cmd_val_to_string(&fields[1]),
                cmd_val_to_string(&fields[2]),
                cmd_val_to_string(&fields[3]),
                cmd_val_to_f64(&fields[4]),
            ),
            Value::Map(m) => (
                m.get("kind").map(cmd_val_to_string).unwrap_or_default(),
                m.get("target").map(cmd_val_to_string).unwrap_or_default(),
                m.get("payload").map(cmd_val_to_string).unwrap_or_default(),
                m.get("on_result").map(cmd_val_to_string).unwrap_or_default(),
                m.get("delay_ms").map(cmd_val_to_f64).unwrap_or(0.0),
            ),
            _ => return None,
        };
        let norm = kind.trim();
        if norm.is_empty() || norm.eq_ignore_ascii_case("none") {
            return None;
        }
        Some(Self {
            kind: norm.to_string(),
            target,
            payload,
            on_result,
            delay_ms,
        })
    }
}

/// Executes a `UiCmd` asynchronously on a background worker thread and sends the completion
/// `UiEvent` back through `tx` without ever blocking UI frame rendering.
///
/// Supported command kinds:
/// - `"timer"`: sleeps `delay_ms.clamp(0.0, 60_000.0)` ms then sends `UiEvent::Click(on_result)`
/// - `"http_get"`: executes `crate::sys::http_get(&target)` then sends `UiEvent::TextInput { action: on_result, value }`
/// - `"http_post"`: executes `crate::sys::http_post(&target, &payload)` then sends `UiEvent::TextInput { action: on_result, value }`
/// - `"storage_save"`: persists `target -> payload` in local JSON key-value store (sends `UiEvent::Click(on_result)` if non-empty)
/// - `"storage_load"`: reads `target` from local JSON key-value store then sends `UiEvent::TextInput { action: on_result, value }`
pub fn spawn_ui_cmd(cmd: UiCmd, tx: mpsc::Sender<UiEvent>) {
    match cmd.kind.trim().to_ascii_lowercase().as_str() {
        "timer" => {
            let ms = cmd.delay_ms.clamp(0.0, 60_000.0);
            let on_result = cmd.on_result;
            let _ = std::thread::Builder::new()
                .name("knr-cmd-timer".into())
                .spawn(move || {
                    if ms > 0.0 {
                        std::thread::sleep(Duration::from_secs_f64(ms / 1000.0));
                    }
                    if !on_result.is_empty() {
                        let _ = tx.send(UiEvent::Click(on_result));
                    }
                });
        }
        "http_get" => {
            let target = cmd.target;
            let on_result = cmd.on_result;
            let _ = std::thread::Builder::new()
                .name("knr-cmd-http-get".into())
                .spawn(move || {
                    let resp = crate::sys::http_get(&target);
                    if !on_result.is_empty() {
                        let _ = tx.send(UiEvent::TextInput {
                            action: on_result,
                            value: resp,
                        });
                    }
                });
        }
        "http_post" => {
            let target = cmd.target;
            let payload = cmd.payload;
            let on_result = cmd.on_result;
            let _ = std::thread::Builder::new()
                .name("knr-cmd-http-post".into())
                .spawn(move || {
                    let resp = crate::sys::http_post(&target, &payload);
                    if !on_result.is_empty() {
                        let _ = tx.send(UiEvent::TextInput {
                            action: on_result,
                            value: resp,
                        });
                    }
                });
        }
        "storage_save" => {
            let key = cmd.target;
            let val = cmd.payload;
            let on_result = cmd.on_result;
            let path = ui_storage_file_path();
            let _ = std::thread::Builder::new()
                .name("knr-cmd-storage-save".into())
                .spawn(move || {
                    {
                        let _guard = STORAGE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
                        let mut map = read_storage_map(&path);
                        map.insert(key, val);
                        write_storage_map(&path, &map);
                    }
                    if !on_result.is_empty() {
                        let _ = tx.send(UiEvent::Click(on_result));
                    }
                });
        }
        "storage_load" => {
            let key = cmd.target;
            let on_result = cmd.on_result;
            let path = ui_storage_file_path();
            let _ = std::thread::Builder::new()
                .name("knr-cmd-storage-load".into())
                .spawn(move || {
                    let value = {
                        let _guard = STORAGE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
                        read_storage_map(&path).remove(&key).unwrap_or_default()
                    };
                    if !on_result.is_empty() {
                        let _ = tx.send(UiEvent::TextInput {
                            action: on_result,
                            value,
                        });
                    }
                });
        }
        _ => {}
    }
}

/// Best-effort non-blocking system clipboard write (never blocks the UI event loop).
fn try_set_system_clipboard(text: String) {
    #[cfg(target_os = "linux")]
    {
        let has_wayland = env::var("WAYLAND_DISPLAY")
            .map(|d| !d.is_empty())
            .unwrap_or(false);
        std::thread::spawn(move || {
            let mut cmd = if has_wayland {
                std::process::Command::new("wl-copy")
            } else {
                let mut c = std::process::Command::new("xclip");
                c.args(["-selection", "clipboard"]);
                c
            };
            if let Ok(mut child) = cmd
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
            {
                if let Some(mut stdin) = child.stdin.take() {
                    let _ = stdin.write_all(text.as_bytes());
                }
                let _ = child.wait();
            }
        });
    }
    #[cfg(target_os = "macos")]
    {
        std::thread::spawn(move || {
            if let Ok(mut child) = std::process::Command::new("pbcopy")
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
            {
                if let Some(mut stdin) = child.stdin.take() {
                    let _ = stdin.write_all(text.as_bytes());
                }
                let _ = child.wait();
            }
        });
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = text;
    }
}

/// Best-effort bounded system clipboard read (returns `None` quickly if unavailable).
fn try_get_system_clipboard() -> Option<String> {
    #[cfg(target_os = "linux")]
    let mut cmd = if env::var("WAYLAND_DISPLAY")
        .map(|d| !d.is_empty())
        .unwrap_or(false)
    {
        let mut c = std::process::Command::new("wl-paste");
        c.arg("--no-newline");
        c
    } else {
        let mut c = std::process::Command::new("xclip");
        c.args(["-o", "-selection", "clipboard"]);
        c
    };
    #[cfg(target_os = "macos")]
    let mut cmd = std::process::Command::new("pbpaste");
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        None
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        let mut child = cmd
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .ok()?;
        let deadline = Instant::now() + Duration::from_millis(60);
        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    if !status.success() {
                        return None;
                    }
                    let mut out = String::new();
                    if let Some(mut stdout) = child.stdout.take() {
                        let _ = stdout.read_to_string(&mut out);
                    }
                    return (!out.is_empty()).then_some(out);
                }
                Ok(None) => {
                    if Instant::now() >= deadline {
                        let _ = child.kill();
                        let _ = child.wait();
                        return None;
                    }
                    std::thread::sleep(Duration::from_millis(2));
                }
                Err(_) => return None,
            }
        }
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

/// Interactive UI events dispatched from native window input back to `.knr` `update()`,
/// `on_slider()`, `on_input()`, `on_key()`, and `on_resize()`.
#[derive(Debug, Clone, PartialEq)]
pub enum UiEvent {
    Click(String),
    TextInput { action: String, value: String },
    SliderChange { action: String, value: f64 },
    Toggle { action: String, checked: bool },
    KeyPress { key: String, ctrl: bool, shift: bool, alt: bool },
    Resize { width: u32, height: u32 },
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
    last_layout_key: (u32, u32, i32, u64), // (logical_w, logical_h, scroll_y_px, scroll_version)
    hit_boxes: Vec<HitBox>,
    cursor_pos: (f32, f32), // always in logical pixels
    scale_factor: f32,
    hovered_action: Option<String>,
    focused_input: Option<(String, String)>, // (action, current_text)
    focused_control: Option<String>,         // Tab-focused non-input control action
    focused_widget_key: Option<String>,      // Keyboard-focused widget key/action for focus ring
    caret_char_idx: usize,
    selection_anchor: Option<usize>,
    modifiers: ModifiersState,
    clipboard_buf: String,
    last_local_copy: Option<Instant>,
    dragging_slider: Option<(String, LayoutRect)>,
    scroll_y: f32,
    scroll_vel_y: f32,
    content_height: f32,
    scroll_offsets: HashMap<String, f32>,
    container_scroll_vel: HashMap<String, (f32, f32)>, // key -> (vel_px_per_sec, max_scroll)
    active_scroll_id: Option<String>,
    scroll_version: u64,
    cmd_rx: Option<mpsc::Receiver<UiEvent>>,
    // Drop order matters on Wayland/Vulkan: GPU & softbuffer surfaces MUST drop BEFORE `window`!
    #[cfg(feature = "gpu")]
    gpu: Option<GpuUiEngine>,
    cpu_surface: Option<softbuffer::Surface<Arc<Window>, Arc<Window>>>,
    cpu_pixmap: Option<tiny_skia::Pixmap>,
    window: Option<Arc<Window>>,
    cpu_ui_dirty: bool,
    closing: bool,
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
            || self.scroll_vel_y.abs() > 1.0
            || !self.container_scroll_vel.is_empty()
    }

    fn frame_interval(&self) -> Duration {
        if self.active_backend == WindowBackend::Wgpu {
            GPU_FRAME_INTERVAL
        } else {
            CPU_FRAME_INTERVAL
        }
    }

    fn cleanup_surfaces(&mut self) {
        #[cfg(feature = "gpu")]
        if let Some(gpu) = self.gpu.take() {
            gpu.shutdown();
            drop(gpu);
        }
        self.cpu_surface = None;
        self.cpu_pixmap = None;
        self.window = None;
    }

    fn shutdown_and_exit(&mut self, event_loop: &ActiveEventLoop) {
        if self.closing {
            return;
        }
        self.closing = true;
        self.cleanup_surfaces();
        event_loop.exit();
    }

    fn redraw(&mut self) {
        if self.closing {
            return;
        }
        self.drain_async_effects();
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

        // Integrate smooth scroll momentum (page-level & per-container vertical/horizontal)
        let dt_f = dt as f32;
        let max_scroll = (self.content_height - logical_h).max(0.0);
        if self.scroll_vel_y.abs() > 1.0 {
            let next_y = (self.scroll_y + self.scroll_vel_y * dt_f).clamp(0.0, max_scroll);
            if (next_y - self.scroll_y).abs() > 0.01 {
                self.scroll_y = next_y;
                self.layout_dirty = true;
            } else {
                self.scroll_vel_y = 0.0;
            }
            self.scroll_vel_y *= (-11.0 * dt_f).exp();
            if self.scroll_vel_y.abs() < 8.0 {
                self.scroll_vel_y = 0.0;
            }
        }
        if !self.container_scroll_vel.is_empty() {
            let decay = (-11.0 * dt_f).exp();
            let mut finished_keys: Vec<String> = Vec::new();
            let mut any_moved = false;
            for (k, (vel, max_s)) in self.container_scroll_vel.iter_mut() {
                let cur = self.scroll_offsets.get(k).copied().unwrap_or(0.0);
                let next = (cur + *vel * dt_f).clamp(0.0, *max_s);
                if (next - cur).abs() > 0.01 {
                    self.scroll_offsets.insert(k.clone(), next);
                    any_moved = true;
                } else {
                    *vel = 0.0;
                }
                *vel *= decay;
                if vel.abs() < 8.0 {
                    finished_keys.push(k.clone());
                }
            }
            for k in finished_keys {
                self.container_scroll_vel.remove(&k);
            }
            if any_moved {
                self.scroll_version = self.scroll_version.wrapping_add(1);
                self.layout_dirty = true;
            }
        }
        if self.container_scroll_vel.is_empty()
            && self.scroll_vel_y.abs() <= 1.0
            && self.active_scroll_id.is_some()
        {
            self.active_scroll_id = None;
            self.cpu_ui_dirty = true;
        }

        // Recompute layout ONLY when tree, window logical size, or scroll position changed!
        self.scroll_y = self.scroll_y.clamp(0.0, max_scroll);
        let layout_key = (
            logical_w.round() as u32,
            logical_h.round() as u32,
            self.scroll_y.round() as i32,
            self.scroll_version,
        );
        if self.layout_dirty || self.cached_layout.is_none() || self.last_layout_key != layout_key {
            let (mut laid, mut hit_boxes) = compute_ui_layout_with_scroll(
                &self.current_tree,
                logical_w,
                logical_h,
                &self.scroll_offsets,
                &self.renderer,
            );
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
                self.scroll_version,
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

        let focused_action_str = self
            .focused_input
            .as_ref()
            .map(|(a, _)| a.as_str())
            .or(self.focused_control.as_deref());
        let (input_caret, input_selection, input_selected_all) =
            if let Some((_, ref buf)) = self.focused_input {
                let len = buf.chars().count();
                let caret = self.caret_char_idx.min(len);
                let sel = ordered_selection(caret, self.selection_anchor, len);
                let sel_all = len > 0 && sel == Some((0, len));
                (Some(caret), sel, sel_all)
            } else {
                (None, None, false)
            };

        let state = RenderInteractionState {
            hovered_action: self.hovered_action.as_deref(),
            focused_action: focused_action_str,
            focused_widget_key: self.focused_widget_key.clone(),
            elapsed_sec: elapsed as f32,
            input_caret,
            input_selection,
            scroll_offsets: Some(&self.scroll_offsets),
            active_scroll_id: self.active_scroll_id.as_deref(),
        };

        let scroll_state = Some((self.scroll_y, self.content_height));

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
                    scroll_state,
                    input_selected_all,
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
                    self.renderer.render_layout_into_ex(
                        pixmap,
                        &frame_layout,
                        &state,
                        scroll_state,
                        input_selected_all,
                    );
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

    fn drain_async_effects(&mut self) -> bool {
        let mut received = Vec::new();
        if let Some(ref rx) = self.cmd_rx {
            for _ in 0..64 {
                match rx.try_recv() {
                    Ok(ev) => received.push(ev),
                    Err(_) => break,
                }
            }
        }
        if received.is_empty() {
            return false;
        }
        for ev in received {
            (self.on_event)(ev);
        }
        self.current_tree = (self.view_fn)();
        self.sync_focused_input_from_tree();
        self.layout_dirty = true;
        self.cpu_ui_dirty = true;
        if let Some(w) = &self.window {
            w.request_redraw();
        }
        true
    }

    fn sync_focused_input_from_tree(&mut self) {
        if let Some((ref act, ref mut buf)) = self.focused_input {
            if let Some(node) = find_input_node(&self.current_tree, act) {
                if node.text != *buf {
                    *buf = node.text.clone();
                    let len = buf.chars().count();
                    self.caret_char_idx = self.caret_char_idx.min(len);
                    self.selection_anchor = self.selection_anchor.map(|a| a.min(len));
                }
            }
        }
    }

    fn dispatch_and_refresh(&mut self, ev: UiEvent) {
        (self.on_event)(ev);
        self.current_tree = (self.view_fn)();
        self.sync_focused_input_from_tree();
        self.layout_dirty = true;
        self.cpu_ui_dirty = true;
        if let Some(w) = &self.window {
            w.request_redraw();
        }
    }

    fn cycle_tab_focus(&mut self, backwards: bool) {
        let mut focusables: Vec<HitBox> = Vec::new();
        for hb in &self.hit_boxes {
            if hb.action.is_empty() {
                continue;
            }
            if !matches!(
                hb.kind,
                UiKind::Input | UiKind::Button | UiKind::Toggle | UiKind::Slider
            ) {
                continue;
            }
            if !focusables.iter().any(|existing| existing.action == hb.action) {
                focusables.push(hb.clone());
            }
        }
        if focusables.is_empty() {
            return;
        }
        let current_action = self
            .focused_input
            .as_ref()
            .map(|(a, _)| a.as_str())
            .or(self.focused_control.as_deref());
        let next_idx = match current_action
            .and_then(|act| focusables.iter().position(|hb| hb.action == act))
        {
            Some(idx) => {
                if backwards {
                    if idx == 0 {
                        focusables.len() - 1
                    } else {
                        idx - 1
                    }
                } else {
                    (idx + 1) % focusables.len()
                }
            }
            None => {
                if backwards {
                    focusables.len() - 1
                } else {
                    0
                }
            }
        };
        let target = &focusables[next_idx];
        let focus_key = find_widget_key_by_action(&self.current_tree, &target.action)
            .or_else(|| Some(target.action.clone()));
        if target.kind == UiKind::Input {
            let text = find_input_node(&self.current_tree, &target.action)
                .map(|n| n.text.clone())
                .unwrap_or_else(|| target.text.clone());
            self.caret_char_idx = text.chars().count();
            self.selection_anchor = None;
            self.focused_input = Some((target.action.clone(), text));
            self.focused_control = None;
            self.focused_widget_key = focus_key;
        } else {
            self.focused_input = None;
            self.caret_char_idx = 0;
            self.selection_anchor = None;
            self.focused_control = Some(target.action.clone());
            self.focused_widget_key = focus_key;
        }
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
        if self.closing {
            return;
        }
        self.drain_async_effects();
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
        } else if self.cmd_rx.is_some() {
            event_loop.set_control_flow(ControlFlow::WaitUntil(Instant::now() + EFFECT_IDLE_WAIT));
        } else if self.has_reload {
            event_loop.set_control_flow(ControlFlow::WaitUntil(Instant::now() + RELOAD_IDLE_WAIT));
        } else {
            event_loop.set_control_flow(ControlFlow::Wait);
        }
    }

    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        self.closing = true;
        self.cleanup_surfaces();
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        if self.closing {
            return;
        }
        match event {
            WindowEvent::CloseRequested | WindowEvent::Destroyed => {
                self.shutdown_and_exit(event_loop);
            }
            WindowEvent::ModifiersChanged(mods) => {
                self.modifiers = mods.state();
            }
            WindowEvent::Focused(false) => {
                self.modifiers = ModifiersState::empty();
            }
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
            } => {
                let ctrl_mod = self.modifiers.control_key() || self.modifiers.super_key();
                let alt_mod = self.modifiers.alt_key();
                let shortcut_mod = ctrl_mod && !alt_mod;
                let shift_mod = self.modifiers.shift_key();
                match logical_key {
                    Key::Named(NamedKey::Escape) => {
                        let key_str = format_key_combo("Escape", ctrl_mod, shift_mod, alt_mod);
                        if self.focused_input.is_some()
                            || self.focused_control.is_some()
                            || self.focused_widget_key.is_some()
                        {
                            self.focused_input = None;
                            self.focused_control = None;
                            self.focused_widget_key = None;
                            self.selection_anchor = None;
                            self.caret_char_idx = 0;
                            self.dispatch_and_refresh(UiEvent::KeyPress {
                                key: key_str,
                                ctrl: ctrl_mod,
                                shift: shift_mod,
                                alt: alt_mod,
                            });
                        } else {
                            let prev_tree = self.current_tree.clone();
                            self.dispatch_and_refresh(UiEvent::KeyPress {
                                key: key_str,
                                ctrl: ctrl_mod,
                                shift: shift_mod,
                                alt: alt_mod,
                            });
                            if self.current_tree == prev_tree {
                                self.shutdown_and_exit(event_loop);
                            }
                        }
                    }
                    Key::Named(NamedKey::Tab) => {
                        self.cycle_tab_focus(shift_mod);
                    }
                    Key::Named(NamedKey::Enter) => {
                        if let Some((act, _buf)) = self.focused_input.clone() {
                            self.selection_anchor = None;
                            self.dispatch_and_refresh(UiEvent::Click(format!("{act}_submit")));
                        } else if let Some(act) = self.focused_control.clone() {
                            let target_hb =
                                self.hit_boxes.iter().find(|hb| hb.action == act).cloned();
                            if let Some(hb) = target_hb {
                                match hb.kind {
                                    UiKind::Toggle => {
                                        self.dispatch_and_refresh(UiEvent::Toggle {
                                            action: hb.action,
                                            checked: hb.value <= 0.5,
                                        });
                                    }
                                    _ => {
                                        self.dispatch_and_refresh(UiEvent::Click(hb.action));
                                    }
                                }
                            }
                        } else {
                            self.dispatch_and_refresh(UiEvent::KeyPress {
                                key: format_key_combo("Enter", ctrl_mod, shift_mod, alt_mod),
                                ctrl: ctrl_mod,
                                shift: shift_mod,
                                alt: alt_mod,
                            });
                        }
                    }
                    Key::Named(NamedKey::Backspace) => {
                        if let Some((act, mut buf)) = self.focused_input.clone() {
                            let char_len = buf.chars().count();
                            self.caret_char_idx = self.caret_char_idx.min(char_len);
                            if !delete_selection_range(
                                &mut buf,
                                &mut self.caret_char_idx,
                                &mut self.selection_anchor,
                            ) {
                                if shortcut_mod {
                                    delete_word_before_caret(&mut buf, &mut self.caret_char_idx);
                                } else if self.caret_char_idx > 0 {
                                    let prev_b = char_to_byte_idx(&buf, self.caret_char_idx - 1);
                                    let cur_b = char_to_byte_idx(&buf, self.caret_char_idx);
                                    buf.replace_range(prev_b..cur_b, "");
                                    self.caret_char_idx -= 1;
                                }
                            }
                            self.focused_input = Some((act.clone(), buf.clone()));
                            self.dispatch_and_refresh(UiEvent::TextInput {
                                action: act,
                                value: buf,
                            });
                        } else {
                            self.dispatch_and_refresh(UiEvent::KeyPress {
                                key: format_key_combo("Backspace", ctrl_mod, shift_mod, alt_mod),
                                ctrl: ctrl_mod,
                                shift: shift_mod,
                                alt: alt_mod,
                            });
                        }
                    }
                    Key::Named(NamedKey::Delete) => {
                        if let Some((act, mut buf)) = self.focused_input.clone() {
                            let char_len = buf.chars().count();
                            self.caret_char_idx = self.caret_char_idx.min(char_len);
                            if !delete_selection_range(
                                &mut buf,
                                &mut self.caret_char_idx,
                                &mut self.selection_anchor,
                            ) && self.caret_char_idx < char_len
                            {
                                let cur_b = char_to_byte_idx(&buf, self.caret_char_idx);
                                let next_b = char_to_byte_idx(&buf, self.caret_char_idx + 1);
                                buf.replace_range(cur_b..next_b, "");
                            }
                            self.focused_input = Some((act.clone(), buf.clone()));
                            self.dispatch_and_refresh(UiEvent::TextInput {
                                action: act,
                                value: buf,
                            });
                        } else {
                            self.dispatch_and_refresh(UiEvent::KeyPress {
                                key: format_key_combo("Delete", ctrl_mod, shift_mod, alt_mod),
                                ctrl: ctrl_mod,
                                shift: shift_mod,
                                alt: alt_mod,
                            });
                        }
                    }
                    Key::Named(NamedKey::ArrowLeft) => {
                        if let Some((_, ref buf)) = self.focused_input {
                            let char_len = buf.chars().count();
                            let cur = self.caret_char_idx.min(char_len);
                            if shift_mod {
                                if self.selection_anchor.is_none() {
                                    self.selection_anchor = Some(cur);
                                }
                                self.caret_char_idx = cur.saturating_sub(1);
                            } else if let Some((s, _)) =
                                ordered_selection(cur, self.selection_anchor, char_len)
                            {
                                self.caret_char_idx = s;
                                self.selection_anchor = None;
                            } else {
                                self.caret_char_idx = cur.saturating_sub(1);
                                self.selection_anchor = None;
                            }
                            self.cpu_ui_dirty = true;
                            if let Some(w) = &self.window {
                                w.request_redraw();
                            }
                        } else if let Some(hb) = self
                            .focused_control
                            .as_deref()
                            .and_then(|act| self.hit_boxes.iter().find(|h| h.action == act))
                            .filter(|h| h.kind == UiKind::Slider)
                            .cloned()
                        {
                            let step = if shift_mod { 0.01 } else { 0.05 };
                            let next_val = (hb.value - step).clamp(0.0, 1.0);
                            self.dispatch_and_refresh(UiEvent::SliderChange {
                                action: hb.action,
                                value: next_val,
                            });
                        } else {
                            self.dispatch_and_refresh(UiEvent::KeyPress {
                                key: format_key_combo("ArrowLeft", ctrl_mod, shift_mod, alt_mod),
                                ctrl: ctrl_mod,
                                shift: shift_mod,
                                alt: alt_mod,
                            });
                        }
                    }
                    Key::Named(NamedKey::ArrowRight) => {
                        if let Some((_, ref buf)) = self.focused_input {
                            let char_len = buf.chars().count();
                            let cur = self.caret_char_idx.min(char_len);
                            if shift_mod {
                                if self.selection_anchor.is_none() {
                                    self.selection_anchor = Some(cur);
                                }
                                self.caret_char_idx = (cur + 1).min(char_len);
                            } else if let Some((_, e)) =
                                ordered_selection(cur, self.selection_anchor, char_len)
                            {
                                self.caret_char_idx = e;
                                self.selection_anchor = None;
                            } else {
                                self.caret_char_idx = (cur + 1).min(char_len);
                                self.selection_anchor = None;
                            }
                            self.cpu_ui_dirty = true;
                            if let Some(w) = &self.window {
                                w.request_redraw();
                            }
                        } else if let Some(hb) = self
                            .focused_control
                            .as_deref()
                            .and_then(|act| self.hit_boxes.iter().find(|h| h.action == act))
                            .filter(|h| h.kind == UiKind::Slider)
                            .cloned()
                        {
                            let step = if shift_mod { 0.01 } else { 0.05 };
                            let next_val = (hb.value + step).clamp(0.0, 1.0);
                            self.dispatch_and_refresh(UiEvent::SliderChange {
                                action: hb.action,
                                value: next_val,
                            });
                        } else {
                            self.dispatch_and_refresh(UiEvent::KeyPress {
                                key: format_key_combo("ArrowRight", ctrl_mod, shift_mod, alt_mod),
                                ctrl: ctrl_mod,
                                shift: shift_mod,
                                alt: alt_mod,
                            });
                        }
                    }
                    Key::Named(NamedKey::Home) => {
                        if let Some((_, ref buf)) = self.focused_input {
                            let char_len = buf.chars().count();
                            let cur = self.caret_char_idx.min(char_len);
                            if shift_mod {
                                if self.selection_anchor.is_none() {
                                    self.selection_anchor = Some(cur);
                                }
                            } else {
                                self.selection_anchor = None;
                            }
                            self.caret_char_idx = 0;
                            self.cpu_ui_dirty = true;
                            if let Some(w) = &self.window {
                                w.request_redraw();
                            }
                        } else {
                            self.dispatch_and_refresh(UiEvent::KeyPress {
                                key: format_key_combo("Home", ctrl_mod, shift_mod, alt_mod),
                                ctrl: ctrl_mod,
                                shift: shift_mod,
                                alt: alt_mod,
                            });
                        }
                    }
                    Key::Named(NamedKey::End) => {
                        if let Some((_, ref buf)) = self.focused_input {
                            let char_len = buf.chars().count();
                            let cur = self.caret_char_idx.min(char_len);
                            if shift_mod {
                                if self.selection_anchor.is_none() {
                                    self.selection_anchor = Some(cur);
                                }
                            } else {
                                self.selection_anchor = None;
                            }
                            self.caret_char_idx = char_len;
                            self.cpu_ui_dirty = true;
                            if let Some(w) = &self.window {
                                w.request_redraw();
                            }
                        } else {
                            self.dispatch_and_refresh(UiEvent::KeyPress {
                                key: format_key_combo("End", ctrl_mod, shift_mod, alt_mod),
                                ctrl: ctrl_mod,
                                shift: shift_mod,
                                alt: alt_mod,
                            });
                        }
                    }
                    Key::Named(key @ (NamedKey::PageDown | NamedKey::PageUp)) => {
                        if self.focused_input.is_none() {
                            let logical_h = self
                                .window
                                .as_ref()
                                .map(|w| {
                                    (w.inner_size().height as f32 / self.scale_factor.max(0.5))
                                        .max(1.0)
                                })
                                .unwrap_or(self.config.height as f32);
                            let page_step = (logical_h * 0.85).max(160.0);
                            let max_scroll = (self.content_height - logical_h).max(0.0);
                            let delta = if key == NamedKey::PageDown {
                                page_step
                            } else {
                                -page_step
                            };
                            self.scroll_y = (self.scroll_y + delta).clamp(0.0, max_scroll);
                            self.layout_dirty = true;
                            if let Some(w) = &self.window {
                                w.request_redraw();
                            }
                        }
                    }
                    Key::Character(ref ch) if shortcut_mod => {
                        let c = ch.as_str();
                        let handled_input_edit = self.focused_input.is_some()
                            && (c.eq_ignore_ascii_case("a")
                                || c == "\x01"
                                || c.eq_ignore_ascii_case("c")
                                || c == "\x03"
                                || c.eq_ignore_ascii_case("x")
                                || c == "\x18"
                                || c.eq_ignore_ascii_case("v")
                                || c == "\x16");
                        if handled_input_edit {
                            if c.eq_ignore_ascii_case("a") || c == "\x01" {
                                if let Some((_, ref buf)) = self.focused_input {
                                    let len = buf.chars().count();
                                    self.selection_anchor = Some(0);
                                    self.caret_char_idx = len;
                                    self.cpu_ui_dirty = true;
                                    if let Some(w) = &self.window {
                                        w.request_redraw();
                                    }
                                }
                            } else if c.eq_ignore_ascii_case("c") || c == "\x03" {
                                if let Some((_, ref buf)) = self.focused_input {
                                    let char_len = buf.chars().count();
                                    let copied = if let Some((s, e)) = ordered_selection(
                                        self.caret_char_idx,
                                        self.selection_anchor,
                                        char_len,
                                    ) {
                                        let sb = char_to_byte_idx(buf, s);
                                        let eb = char_to_byte_idx(buf, e);
                                        buf[sb..eb].to_string()
                                    } else {
                                        buf.clone()
                                    };
                                    self.clipboard_buf = copied.clone();
                                    self.last_local_copy = Some(Instant::now());
                                    try_set_system_clipboard(copied);
                                }
                            } else if c.eq_ignore_ascii_case("x") || c == "\x18" {
                                if let Some((act, mut buf)) = self.focused_input.clone() {
                                    let char_len = buf.chars().count();
                                    self.caret_char_idx = self.caret_char_idx.min(char_len);
                                    let cut = if let Some((s, e)) = ordered_selection(
                                        self.caret_char_idx,
                                        self.selection_anchor,
                                        char_len,
                                    ) {
                                        let sb = char_to_byte_idx(&buf, s);
                                        let eb = char_to_byte_idx(&buf, e);
                                        let slice = buf[sb..eb].to_string();
                                        buf.replace_range(sb..eb, "");
                                        self.caret_char_idx = s;
                                        self.selection_anchor = None;
                                        slice
                                    } else {
                                        let full = std::mem::take(&mut buf);
                                        self.caret_char_idx = 0;
                                        self.selection_anchor = None;
                                        full
                                    };
                                    self.clipboard_buf = cut.clone();
                                    self.last_local_copy = Some(Instant::now());
                                    try_set_system_clipboard(cut);
                                    self.focused_input = Some((act.clone(), buf.clone()));
                                    self.dispatch_and_refresh(UiEvent::TextInput {
                                        action: act,
                                        value: buf,
                                    });
                                }
                            } else if c.eq_ignore_ascii_case("v") || c == "\x16" {
                                if let Some((act, mut buf)) = self.focused_input.clone() {
                                    let recent_local = self
                                        .last_local_copy
                                        .is_some_and(|t| t.elapsed() < Duration::from_millis(250));
                                    let raw_paste = if recent_local {
                                        self.clipboard_buf.clone()
                                    } else {
                                        try_get_system_clipboard()
                                            .unwrap_or_else(|| self.clipboard_buf.clone())
                                    };
                                    let clean: String = raw_paste
                                        .chars()
                                        .map(|ch_in| {
                                            if matches!(ch_in, '\n' | '\r' | '\t') {
                                                ' '
                                            } else {
                                                ch_in
                                            }
                                        })
                                        .filter(|ch_in| !ch_in.is_control())
                                        .collect();
                                    if !clean.is_empty() {
                                        let char_len = buf.chars().count();
                                        self.caret_char_idx = self.caret_char_idx.min(char_len);
                                        delete_selection_range(
                                            &mut buf,
                                            &mut self.caret_char_idx,
                                            &mut self.selection_anchor,
                                        );
                                        let insert_b = char_to_byte_idx(&buf, self.caret_char_idx);
                                        buf.insert_str(insert_b, &clean);
                                        self.caret_char_idx += clean.chars().count();
                                        self.focused_input = Some((act.clone(), buf.clone()));
                                        self.dispatch_and_refresh(UiEvent::TextInput {
                                            action: act,
                                            value: buf,
                                        });
                                    }
                                }
                            }
                        } else if let Some(base) = winit_key_base_name(&logical_key) {
                            self.dispatch_and_refresh(UiEvent::KeyPress {
                                key: format_key_combo(&base, ctrl_mod, shift_mod, alt_mod),
                                ctrl: ctrl_mod,
                                shift: shift_mod,
                                alt: alt_mod,
                            });
                        }
                    }
                    _ if !shortcut_mod && !alt_mod => {
                        if let Some((act, mut buf)) = self.focused_input.clone() {
                            if let Some(txt) = text {
                                let clean: String =
                                    txt.chars().filter(|c| !c.is_control()).collect();
                                if !clean.is_empty() {
                                    let char_len = buf.chars().count();
                                    self.caret_char_idx = self.caret_char_idx.min(char_len);
                                    delete_selection_range(
                                        &mut buf,
                                        &mut self.caret_char_idx,
                                        &mut self.selection_anchor,
                                    );
                                    let insert_b = char_to_byte_idx(&buf, self.caret_char_idx);
                                    buf.insert_str(insert_b, &clean);
                                    self.caret_char_idx += clean.chars().count();
                                    self.focused_input = Some((act.clone(), buf.clone()));
                                    self.dispatch_and_refresh(UiEvent::TextInput {
                                        action: act,
                                        value: buf,
                                    });
                                } else if let Some(base) = winit_key_base_name(&logical_key) {
                                    self.dispatch_and_refresh(UiEvent::KeyPress {
                                        key: format_key_combo(&base, ctrl_mod, shift_mod, alt_mod),
                                        ctrl: ctrl_mod,
                                        shift: shift_mod,
                                        alt: alt_mod,
                                    });
                                }
                            } else if let Some(base) = winit_key_base_name(&logical_key) {
                                self.dispatch_and_refresh(UiEvent::KeyPress {
                                    key: format_key_combo(&base, ctrl_mod, shift_mod, alt_mod),
                                    ctrl: ctrl_mod,
                                    shift: shift_mod,
                                    alt: alt_mod,
                                });
                            }
                        } else if matches!(logical_key, Key::Named(NamedKey::Space))
                            && self.focused_control.is_some()
                        {
                            if let Some(act) = self.focused_control.clone() {
                                let target_hb =
                                    self.hit_boxes.iter().find(|hb| hb.action == act).cloned();
                                if let Some(hb) = target_hb {
                                    match hb.kind {
                                        UiKind::Toggle => {
                                            self.dispatch_and_refresh(UiEvent::Toggle {
                                                action: hb.action,
                                                checked: hb.value <= 0.5,
                                            });
                                        }
                                        _ => {
                                            self.dispatch_and_refresh(UiEvent::Click(hb.action));
                                        }
                                    }
                                }
                            }
                        } else if let Some(base) = winit_key_base_name(&logical_key) {
                            self.dispatch_and_refresh(UiEvent::KeyPress {
                                key: format_key_combo(&base, ctrl_mod, shift_mod, alt_mod),
                                ctrl: ctrl_mod,
                                shift: shift_mod,
                                alt: alt_mod,
                            });
                        }
                    }
                    _ => {
                        if let Some(base) = winit_key_base_name(&logical_key) {
                            self.dispatch_and_refresh(UiEvent::KeyPress {
                                key: format_key_combo(&base, ctrl_mod, shift_mod, alt_mod),
                                ctrl: ctrl_mod,
                                shift: shift_mod,
                                alt: alt_mod,
                            });
                        }
                    }
                }
            }
            WindowEvent::Resized(new_size) => {
                if new_size.width == 0 || new_size.height == 0 {
                    return;
                }
                #[cfg(feature = "gpu")]
                if let Some(gpu) = self.gpu.as_mut() {
                    gpu.resize(new_size.width, new_size.height);
                }
                let inv = 1.0 / self.scale_factor.max(0.5);
                let lw = ((new_size.width as f32 * inv).round() as u32).max(1);
                let lh = ((new_size.height as f32 * inv).round() as u32).max(1);
                self.dispatch_and_refresh(UiEvent::Resize {
                    width: lw,
                    height: lh,
                });
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let (raw_dx, raw_dy) = match delta {
                    MouseScrollDelta::LineDelta(x, y) => (x * 36.0, y * 36.0),
                    MouseScrollDelta::PixelDelta(pos) => {
                        let inv = 1.0 / self.scale_factor.max(0.5);
                        (pos.x as f32 * inv, pos.y as f32 * inv)
                    }
                };
                // Shift+Wheel scrolls horizontal (`overflow="scroll-x"`) containers!
                let (dx, dy) = if self.modifiers.shift_key() && raw_dx.abs() < 0.01 {
                    (raw_dy, 0.0)
                } else {
                    (raw_dx, raw_dy)
                };

                let mut handled_inner = false;
                if let Some(ref laid) = self.cached_layout {
                    let targets =
                        hit_test_scroll_targets(laid, self.cursor_pos.0, self.cursor_pos.1);
                    if dx.abs() > 0.01 {
                        for target in &targets {
                            if target.max_scroll_x <= 0.5 {
                                continue;
                            }
                            let key = if target.max_scroll_y > 0.5 {
                                format!("{}:x", target.id)
                            } else {
                                target.id.clone()
                            };
                            let cur = self
                                .scroll_offsets
                                .get(&key)
                                .copied()
                                .unwrap_or(target.scroll_offset_x);
                            let next = (cur - dx * 0.45).clamp(0.0, target.max_scroll_x);
                            if (next - cur).abs() > 0.01 || (-dx).abs() > 0.01 {
                                self.scroll_offsets.insert(key.clone(), next);
                                let entry = self
                                    .container_scroll_vel
                                    .entry(key)
                                    .or_insert((0.0, target.max_scroll_x));
                                entry.0 = (entry.0 - dx * 5.5).clamp(-2400.0, 2400.0);
                                entry.1 = target.max_scroll_x;
                                self.active_scroll_id = Some(target.id.clone());
                                self.scroll_version = self.scroll_version.wrapping_add(1);
                                handled_inner = true;
                                break;
                            }
                        }
                    }
                    if dy.abs() > 0.01 {
                        for target in &targets {
                            if target.max_scroll_y <= 0.5 {
                                continue;
                            }
                            let cur = self
                                .scroll_offsets
                                .get(&target.id)
                                .copied()
                                .unwrap_or(target.scroll_offset_y);
                            let next = (cur - dy * 0.45).clamp(0.0, target.max_scroll_y);
                            if (next - cur).abs() > 0.01 {
                                self.scroll_offsets.insert(target.id.clone(), next);
                                let entry = self
                                    .container_scroll_vel
                                    .entry(target.id.clone())
                                    .or_insert((0.0, target.max_scroll_y));
                                entry.0 = (entry.0 - dy * 5.5).clamp(-2400.0, 2400.0);
                                entry.1 = target.max_scroll_y;
                                self.active_scroll_id = Some(target.id.clone());
                                self.scroll_version = self.scroll_version.wrapping_add(1);
                                handled_inner = true;
                                break;
                            }
                        }
                    }
                }
                if !handled_inner && dy.abs() > 0.01 {
                    self.scroll_y = (self.scroll_y - dy * 0.45).max(0.0);
                    self.scroll_vel_y = (self.scroll_vel_y - dy * 5.5).clamp(-2400.0, 2400.0);
                }
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
                            let (text, font_size, font_family, font_weight, letter_spacing, pad_l) =
                                if let Some(node) = find_input_node(&self.current_tree, &hb.action)
                                {
                                    (
                                        node.text.clone(),
                                        node.font_size,
                                        node.style.font_family.clone(),
                                        node.style.font_weight,
                                        node.style.letter_spacing,
                                        node.effective_pad()[3].max(12.0),
                                    )
                                } else {
                                    (hb.text.clone(), 16.0, "sans".to_string(), 400, 0.0, 12.0)
                                };
                            let local_x = self.cursor_pos.0 - (hb.rect.x + pad_l);
                            self.caret_char_idx = self.renderer.hit_test_text_char_idx(
                                &text,
                                local_x,
                                font_size,
                                &font_family,
                                font_weight,
                                letter_spacing,
                            );
                            self.selection_anchor = None;
                            self.focused_control = None;
                            self.focused_widget_key =
                                find_widget_key_by_action(&self.current_tree, &hb.action);
                            self.focused_input = Some((hb.action.clone(), text));
                            self.cpu_ui_dirty = true;
                            if let Some(w) = &self.window {
                                w.request_redraw();
                            }
                        }
                        UiKind::Slider => {
                            self.focused_input = None;
                            self.focused_control = None;
                            self.focused_widget_key = None;
                            self.selection_anchor = None;
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
                            self.focused_control = None;
                            self.focused_widget_key = None;
                            self.selection_anchor = None;
                            self.dispatch_and_refresh(UiEvent::Toggle {
                                action: hb.action,
                                checked: hb.value <= 0.5,
                            });
                        }
                        _ => {
                            self.focused_input = None;
                            self.focused_control = None;
                            self.focused_widget_key = None;
                            self.selection_anchor = None;
                            self.dispatch_and_refresh(UiEvent::Click(hb.action));
                        }
                    },
                    None => {
                        if self.focused_input.is_some()
                            || self.focused_control.is_some()
                            || self.focused_widget_key.is_some()
                        {
                            self.focused_input = None;
                            self.focused_control = None;
                            self.focused_widget_key = None;
                            self.selection_anchor = None;
                            self.caret_char_idx = 0;
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
/// with live hot-reload AND non-blocking async `UiCmd` effect channel support.
#[allow(clippy::too_many_arguments)]
pub fn launch_ui_app_with_reload_and_effects<FView, FEvent, FTick, FReload>(
    config: WindowConfig,
    shaders_wgsl: HashMap<String, String>,
    has_tick: bool,
    view_fn: FView,
    on_event: FEvent,
    on_tick: FTick,
    poll_reload: FReload,
    cmd_rx: Option<mpsc::Receiver<UiEvent>>,
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
        cmd_rx,
    )
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
        None,
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
        None,
    )
}

#[allow(clippy::too_many_arguments)]
fn launch_ui_app_inner<FView, FEvent, FTick, FReload>(
    config: WindowConfig,
    shaders_wgsl: HashMap<String, String>,
    mut has_tick: bool,
    mut view_fn: FView,
    mut on_event: FEvent,
    mut on_tick: FTick,
    has_reload: bool,
    mut poll_reload: FReload,
    cmd_rx: Option<mpsc::Receiver<UiEvent>>,
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
                last_layout_key: (0, 0, 0, 0),
                hit_boxes: Vec::new(),
                cursor_pos: (0.0, 0.0),
                scale_factor: 1.0,
                hovered_action: None,
                focused_input: None,
                focused_control: None,
                focused_widget_key: None,
                caret_char_idx: 0,
                selection_anchor: None,
                modifiers: ModifiersState::empty(),
                clipboard_buf: String::new(),
                last_local_copy: None,
                dragging_slider: None,
                scroll_y: 0.0,
                scroll_vel_y: 0.0,
                content_height: 0.0,
                scroll_offsets: HashMap::new(),
                container_scroll_vel: HashMap::new(),
                active_scroll_id: None,
                scroll_version: 0,
                cmd_rx,
                #[cfg(feature = "gpu")]
                gpu: None,
                cpu_surface: None,
                cpu_pixmap: None,
                window: None,
                cpu_ui_dirty: true,
                closing: false,
                active_backend: WindowBackend::Headless,
                start_time: now,
                last_frame: now,
                next_frame_at: now + GPU_FRAME_INTERVAL,
                last_reload_check: now,
                frames_rendered: 0,
                final_pixel_hash: 0,
            };
            if event_loop.run_app(&mut app).is_ok() && (app.frames_rendered > 0 || app.closing) {
                app.cleanup_surfaces();
                return Ok(WindowLaunchResult {
                    backend: app.active_backend,
                    frames_rendered: app.frames_rendered,
                    final_pixel_hash: app.final_pixel_hash,
                });
            }
            app.cleanup_surfaces();
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
    let empty_scroll: HashMap<String, f32> = HashMap::new();

    for f in 0..frames {
        let elapsed = f as f64 * dt;
        if let Some(ref rx) = cmd_rx {
            let mut got_effect = false;
            while let Ok(ev) = rx.try_recv() {
                on_event(ev);
                got_effect = true;
            }
            if got_effect {
                tree = view_fn();
            }
        }
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
        let (mut laid, _) = compute_ui_layout_with_scroll(
            &tree,
            config.width as f32,
            config.height as f32,
            &empty_scroll,
            &renderer,
        );
        animator.step_and_apply(&mut laid, None, dt as f32);
        let state = RenderInteractionState {
            elapsed_sec: elapsed as f32,
            ..Default::default()
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
            UiEvent::KeyPress { key, .. } => on_action(&format!("key:{key}")),
            UiEvent::Resize { width, height } => on_action(&format!("resize:{width}x{height}")),
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
    // Drop surface BEFORE window so Wayland/X11 teardown never crashes
    surface: Option<softbuffer::Surface<Arc<Window>, Arc<Window>>>,
    window: Option<Arc<Window>>,
    closing: bool,
    frames_rendered: usize,
}

impl ApplicationHandler for CanvasWindowApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_none() && !self.closing {
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

    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        self.closing = true;
        self.surface = None;
        self.window = None;
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        if self.closing {
            return;
        }
        match event {
            WindowEvent::CloseRequested | WindowEvent::Destroyed => {
                self.closing = true;
                self.surface = None;
                self.window = None;
                event_loop.exit();
            }
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        logical_key: Key::Named(NamedKey::Escape),
                        state: ElementState::Pressed,
                        ..
                    },
                ..
            } => {
                self.closing = true;
                self.surface = None;
                self.window = None;
                event_loop.exit();
            }
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
                surface: None,
                window: None,
                closing: false,
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

#[cfg(test)]
mod input_edit_tests {
    use super::*;

    #[test]
    fn test_input_selection_and_utf8_word_deletion_helpers() {
        let mut s = "hello café world".to_string();
        assert_eq!(char_to_byte_idx(&s, 0), 0);
        assert_eq!(char_to_byte_idx(&s, 6), 6);
        // 'é' is 2 bytes in UTF-8, so char 10 (' ') is at byte 11
        assert_eq!(&s[char_to_byte_idx(&s, 6)..char_to_byte_idx(&s, 10)], "café");

        let mut caret = 10usize;
        let mut anchor = Some(6usize);
        assert_eq!(ordered_selection(caret, anchor, s.chars().count()), Some((6, 10)));
        assert!(delete_selection_range(&mut s, &mut caret, &mut anchor));
        assert_eq!(s, "hello  world");
        assert_eq!(caret, 6);
        assert_eq!(anchor, None);

        // Delete word before caret (mid-string)
        let mut s2 = "alpha beta gamma".to_string();
        let mut c2 = 10usize; // right after "beta"
        delete_word_before_caret(&mut s2, &mut c2);
        assert_eq!(s2, "alpha  gamma");
        assert_eq!(c2, 6);
    }
}
