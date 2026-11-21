#![allow(clippy::not_unsafe_ptr_arg_deref)]

use crate::hw::HwPlatform;
use crate::net::NetPlatform;
use crate::platform::{HostPlatform, Value};
use crate::ui::UiPlatform;
use knr_core::Effect;
use std::alloc::Layout;
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::ffi::{c_char, c_void, CStr, CString};
use std::sync::{Mutex, OnceLock};

type RtMap = BTreeMap<Vec<u8>, *mut c_void>;
type RtVec = Vec<*mut c_void>;

/// Heap-allocated struct/enum instance for compiled native code.
/// Fields are stored as opaque 64-bit slots (i64, f64 bits, bool 0/1, or raw pointers).
pub struct RtStruct {
    pub fields: Vec<i64>,
}

struct NativeHost {
    net: Option<NetPlatform>,
    ui: UiPlatform,
    hw: HwPlatform,
    channels: HashMap<i64, VecDeque<i64>>,
    next_chan_id: i64,
}

impl NativeHost {
    fn new() -> Self {
        Self {
            net: None,
            ui: UiPlatform::new(),
            hw: HwPlatform::new(),
            channels: HashMap::new(),
            next_chan_id: 1,
        }
    }

    fn ensure_net(&mut self) -> Result<&mut NetPlatform, String> {
        if self.net.is_none() {
            self.net = Some(NetPlatform::new().map_err(|e| e.to_string())?);
        }
        Ok(self.net.as_mut().unwrap())
    }
}

fn global_host() -> &'static Mutex<NativeHost> {
    static HOST: OnceLock<Mutex<NativeHost>> = OnceLock::new();
    HOST.get_or_init(|| Mutex::new(NativeHost::new()))
}

fn leak_c_string(s: &str) -> *mut c_char {
    // Replace any interior NULs so CString::new never fails
    let clean = if s.contains('\0') {
        s.replace('\0', "")
    } else {
        s.to_string()
    };
    CString::new(clean)
        .unwrap_or_else(|_| CString::new("").unwrap())
        .into_raw()
}

unsafe fn read_c_str(ptr: *const c_char) -> String {
    if ptr.is_null() {
        return String::new();
    }
    CStr::from_ptr(ptr).to_string_lossy().into_owned()
}

// ============================================================================
// Struct / Enum C-ABI
// ============================================================================

#[no_mangle]
pub extern "C" fn rt_struct_new(field_count: u64) -> *mut c_void {
    let count = (field_count as usize).min(65536);
    let s = Box::new(RtStruct {
        fields: vec![0i64; count],
    });
    Box::into_raw(s) as *mut c_void
}

#[no_mangle]
pub extern "C" fn rt_struct_get(ptr: *mut c_void, idx: u64) -> i64 {
    if ptr.is_null() {
        return 0;
    }
    let s = unsafe { &*(ptr as *const RtStruct) };
    s.fields.get(idx as usize).copied().unwrap_or(0)
}

#[no_mangle]
pub extern "C" fn rt_struct_set(ptr: *mut c_void, idx: u64, val: i64) -> *mut c_void {
    if ptr.is_null() {
        return std::ptr::null_mut();
    }
    let s = unsafe { &mut *(ptr as *mut RtStruct) };
    let i = idx as usize;
    if i >= s.fields.len() && i < 65536 {
        s.fields.resize(i + 1, 0);
    }
    if i < s.fields.len() {
        s.fields[i] = val;
    }
    ptr
}

#[no_mangle]
pub extern "C" fn rt_struct_free(ptr: *mut c_void) {
    if !ptr.is_null() {
        unsafe {
            drop(Box::from_raw(ptr as *mut RtStruct));
        }
    }
}

// ============================================================================
// String C-ABI
// ============================================================================

#[no_mangle]
pub extern "C" fn rt_str_concat(a: *const c_char, b: *const c_char) -> *mut c_char {
    let sa = unsafe { read_c_str(a) };
    let sb = unsafe { read_c_str(b) };
    let combined = format!("{sa}{sb}");
    leak_c_string(&combined)
}

#[no_mangle]
pub extern "C" fn rt_str_eq(a: *const c_char, b: *const c_char) -> i64 {
    if a == b {
        return 1;
    }
    if a.is_null() || b.is_null() {
        return 0;
    }
    let sa = unsafe { CStr::from_ptr(a).to_bytes() };
    let sb = unsafe { CStr::from_ptr(b).to_bytes() };
    if sa == sb { 1 } else { 0 }
}

#[no_mangle]
pub extern "C" fn rt_str_len(s: *const c_char) -> i64 {
    if s.is_null() {
        return 0;
    }
    unsafe { CStr::from_ptr(s).to_bytes().len() as i64 }
}

#[no_mangle]
pub extern "C" fn rt_int_to_str(val: i64) -> *mut c_char {
    leak_c_string(&val.to_string())
}

#[no_mangle]
pub extern "C" fn rt_bool_to_str(val: i64) -> *mut c_char {
    leak_c_string(if val != 0 { "true" } else { "false" })
}

#[no_mangle]
pub extern "C" fn rt_f64_to_str(val: f64) -> *mut c_char {
    leak_c_string(&val.to_string())
}

// ============================================================================
// Math C-ABI (parité exacte avec VM, WASM et WGSL)
// ============================================================================

#[no_mangle]
pub extern "C" fn rt_sin(x: f64) -> f64 {
    x.sin()
}

#[no_mangle]
pub extern "C" fn rt_cos(x: f64) -> f64 {
    x.cos()
}

#[no_mangle]
pub extern "C" fn rt_tan(x: f64) -> f64 {
    x.tan()
}

#[no_mangle]
pub extern "C" fn rt_pow(a: f64, b: f64) -> f64 {
    a.powf(b)
}

#[no_mangle]
pub extern "C" fn rt_atan2(y: f64, x: f64) -> f64 {
    y.atan2(x)
}

// ============================================================================
// Channel C-ABI
// ============================================================================

#[no_mangle]
pub extern "C" fn rt_chan_new(cap: i64) -> i64 {
    let mut host = global_host().lock().unwrap();
    let id = host.next_chan_id;
    host.next_chan_id += 1;
    let c = cap.max(0) as usize;
    host.channels.insert(id, VecDeque::with_capacity(c));
    id
}

#[no_mangle]
pub extern "C" fn rt_chan_send(chan_id: i64, val: i64) -> i64 {
    let mut host = global_host().lock().unwrap();
    if let Some(q) = host.channels.get_mut(&chan_id) {
        q.push_back(val);
        1
    } else {
        0
    }
}

#[no_mangle]
pub extern "C" fn rt_chan_recv(chan_id: i64) -> i64 {
    let mut host = global_host().lock().unwrap();
    if let Some(q) = host.channels.get_mut(&chan_id) {
        q.pop_front().unwrap_or(0)
    } else {
        0
    }
}

// ============================================================================
// Host Syscall C-ABI (Net / Ui / Hw / Async)
// payload_kind:
//   0 = i64
//   1 = str (*const c_char)
//   2 = HttpResponse struct ([0]=req_id: i64, [1]=status: i64, [2]=body: *const c_char)
//   3 = HttpResponseInt struct ([0]=req_id: i64, [1]=status: i64, [2]=body: i64)
//   4 = bool (i64)
// ============================================================================

#[no_mangle]
pub extern "C" fn rt_syscall(
    effect_tag: i64,
    op_ptr: *const c_char,
    payload: i64,
    payload_kind: i64,
) -> i64 {
    let op_name = unsafe { read_c_str(op_ptr) };
    let effect = match effect_tag {
        0 => Effect::Net,
        1 => Effect::Ui,
        2 => Effect::Hw,
        3 => Effect::Async,
        _ => return 0,
    };

    let mut host = global_host().lock().unwrap();

    match (effect, op_name.as_str()) {
        (Effect::Net, "net.listen") => {
            let port = std::env::var("KNR_PORT")
                .ok()
                .and_then(|s| s.parse::<u16>().ok())
                .unwrap_or_else(|| payload.clamp(0, 65535) as u16);
            match host.ensure_net().and_then(|net| {
                net.listen(port).map_err(|e| e.to_string())
            }) {
                Ok((chan_id, bound_port)) => {
                    eprintln!("Listening on http://127.0.0.1:{bound_port}");
                    chan_id as i64
                }
                Err(err) => {
                    eprintln!("net.listen error: {err}");
                    0
                }
            }
        }
        (Effect::Net, "net.wait_req") => {
            let chan_id = payload as usize;
            let res = host
                .ensure_net()
                .and_then(|net| net.wait_request(chan_id).map_err(|e| e.to_string()));
            match res {
                Ok(Some(Value::Map(m))) => {
                    let req_id = m.get("req_id").and_then(Value::as_i64).unwrap_or(0);
                    let method = m.get("method").and_then(Value::as_str).unwrap_or("GET");
                    let path = m.get("path").and_then(Value::as_str).unwrap_or("/");
                    let s = Box::new(RtStruct {
                        fields: vec![
                            req_id,
                            leak_c_string(method) as i64,
                            leak_c_string(path) as i64,
                        ],
                    });
                    Box::into_raw(s) as i64
                }
                _ => 0,
            }
        }
        (Effect::Net, "net.respond") => {
            let ptr = payload as *const RtStruct;
            if ptr.is_null() {
                return 0;
            }
            let s = unsafe { &*ptr };
            let req_id = s.fields.first().copied().unwrap_or(0) as usize;
            let status = s.fields.get(1).copied().unwrap_or(200);
            let body_bytes = match payload_kind {
                3 => s.fields.get(2).copied().unwrap_or(0).to_string().into_bytes(),
                _ => {
                    let body_ptr = s.fields.get(2).copied().unwrap_or(0) as *const c_char;
                    unsafe { read_c_str(body_ptr) }.into_bytes()
                }
            };
            let ok = host
                .ensure_net()
                .and_then(|net| net.respond(req_id, status, &body_bytes).map_err(|e| e.to_string()))
                .unwrap_or(false);
            if ok { 1 } else { 0 }
        }
        (Effect::Ui, "ui.render") => {
            let rendered = match payload_kind {
                1 => unsafe { read_c_str(payload as *const c_char) },
                4 => (if payload != 0 { "true" } else { "false" }).to_string(),
                5 => {
                    let ui_node = unsafe { rt_struct_to_ui_node(payload as *const RtStruct) };
                    ui_node.render_document_html("knr Native Server")
                }
                _ => payload.to_string(),
            };
            leak_c_string(&rendered) as i64
        }
        (Effect::Hw, "hw.read_reg") => host.hw.read_reg(payload as u64) as i64,
        (Effect::Hw, "hw.write_reg") => {
            let ptr = payload as *const RtVec;
            if !ptr.is_null() {
                let v = unsafe { &*ptr };
                if v.len() >= 2 {
                    host.hw.write_reg(v[0] as u64, v[1] as u64);
                }
            }
            0
        }
        (eff, op) => {
            let val = match payload_kind {
                1 => Value::Str(unsafe { read_c_str(payload as *const c_char) }),
                4 => Value::Bool(payload != 0),
                2 | 5 => {
                    let ptr = payload as *const RtStruct;
                    if !ptr.is_null() && matches!(op, "fs.write_file" | "json.get" | "http.post") {
                        let s = unsafe { &*ptr };
                        let s0 = unsafe { read_c_str(s.fields.first().copied().unwrap_or(0) as *const c_char) };
                        let s1 = unsafe { read_c_str(s.fields.get(1).copied().unwrap_or(0) as *const c_char) };
                        let schema = match op {
                            "fs.write_file" => "FileWrite",
                            "json.get" => "JsonQuery",
                            _ => "HttpPost",
                        };
                        Value::Struct {
                            schema: schema.to_string(),
                            fields: vec![Value::Str(s0), Value::Str(s1)],
                        }
                    } else {
                        Value::I64(payload)
                    }
                }
                _ => Value::I64(payload),
            };
            let result = match eff {
                Effect::Ui => host.ui.host_call(eff, op, val),
                Effect::Hw => host.hw.host_call(eff, op, val),
                Effect::Net | Effect::Async => match host.ensure_net() {
                    Ok(net) => net.host_call(eff, op, val),
                    Err(_) => return 0,
                },
            };
            match result {
                Ok(Value::I64(n)) => n,
                Ok(Value::Bool(b)) => if b { 1 } else { 0 },
                Ok(Value::Str(s)) => leak_c_string(&s) as i64,
                Ok(Value::Chan(c)) => c as i64,
                _ => 0,
            }
        }
    }
}

unsafe fn rt_struct_to_ui_node(ptr: *const RtStruct) -> crate::gui::UiNode {
    use crate::gui::{UiKind, UiNode, UiStyle};
    if ptr.is_null() {
        return UiNode::default();
    }
    let s = &*ptr;
    let f = &s.fields;
    let get_str = |idx: usize| -> String {
        let p = f.get(idx).copied().unwrap_or(0) as *const c_char;
        read_c_str(p)
    };
    let get_i64 = |idx: usize| -> i64 { f.get(idx).copied().unwrap_or(0) };
    let get_f64 = |idx: usize| -> f64 { f64::from_bits(get_i64(idx) as u64) };

    // New canonical 8-field `Widget { key, kind, text, action, value, uniforms, style, children }`
    if f.len() == 8 {
        let key = get_str(0);
        let kind = UiKind::from_str_kind(&get_str(1));
        let text = get_str(2);
        let action = get_str(3);
        let value = get_f64(4);

        let mut uniforms = Vec::new();
        let u_ptr = get_i64(5) as *const RtVec;
        if !u_ptr.is_null() {
            for &raw in &*u_ptr {
                uniforms.push(f64::from_bits(raw as u64));
            }
        }

        let mut bg = String::new();
        let mut color = "#f8fafc".to_string();
        let mut border = String::new();
        let mut font_size = 16.0f32;
        let mut style = UiStyle::default();

        let st_ptr = get_i64(6) as *const RtStruct;
        if !st_ptr.is_null() {
            let sf = &(*st_ptr).fields;
            let sf64 = |i: usize| -> f32 { f64::from_bits(sf.get(i).copied().unwrap_or(0) as u64) as f32 };
            let sstr = |i: usize| -> String {
                let p = sf.get(i).copied().unwrap_or(0) as *const c_char;
                read_c_str(p)
            };
            let si64 = |i: usize| -> i64 { sf.get(i).copied().unwrap_or(0) };

            style.w = sf64(0);
            style.h = sf64(1);
            style.min_w = sf64(2);
            style.max_w = sf64(3);
            bg = sstr(4);
            let c = sstr(5);
            if !c.is_empty() {
                color = c;
            }
            border = sstr(6);
            let bw = sf64(7);
            style.border_w = if bw > 0.0 { bw } else { 1.0 };
            style.radius_corners = [sf64(8).max(0.0), sf64(9).max(0.0), sf64(10).max(0.0), sf64(11).max(0.0)];
            style.pad_edges = [sf64(12).max(0.0), sf64(13).max(0.0), sf64(14).max(0.0), sf64(15).max(0.0)];
            style.gap_x = sf64(16).max(0.0);
            style.gap_y = sf64(17).max(0.0);
            let fs = sf64(18);
            if fs > 0.0 {
                font_size = fs;
            }
            let fw = si64(19);
            style.font_weight = if fw > 0 { fw } else { 400 };
            let al = sstr(20);
            if !al.is_empty() {
                style.align = al;
            }
            let ju = sstr(21);
            if !ju.is_empty() {
                style.justify = ju;
            }
            style.grid_cols = si64(22);
            style.shadow_x = sf64(23);
            style.shadow_y = sf64(24);
            style.shadow_blur = sf64(25).max(0.0);
            style.shadow_color = sstr(26);
            style.backdrop_blur = sf64(27).max(0.0);
            let op = sf64(28);
            style.opacity = if sf.len() > 28 && op > 0.0 { op.clamp(0.0, 1.0) } else { 1.0 };
            style.translate_x = sf64(29);
            style.translate_y = sf64(30);
            let sc = sf64(31);
            style.scale = if sc > 0.0 { sc } else { 1.0 };
            style.rotate_deg = sf64(32);
            style.hover_bg = sstr(33);
            style.hover_border = sstr(34);
            let hs = sf64(35);
            style.hover_scale = if hs > 0.0 { hs } else { 1.0 };
            style.transition_ms = sf64(36).max(0.0);
            let ov = sstr(37);
            if !ov.is_empty() {
                style.overflow = ov;
            }
            style.z_index = si64(38);
            style.grow = sf64(39).max(0.0);
        }

        let mut children = Vec::new();
        let vec_ptr = get_i64(7) as *const RtVec;
        if !vec_ptr.is_null() {
            for &child_raw in &*vec_ptr {
                if !child_raw.is_null() {
                    children.push(rt_struct_to_ui_node(child_raw as *const RtStruct));
                }
            }
        }

        let radius = style.radius_corners[0];
        let pad = style.pad_edges[0];
        let gap = style.gap_y.max(style.gap_x);

        return UiNode {
            key,
            kind,
            text,
            action,
            value,
            uniforms,
            bg,
            color,
            border,
            radius,
            pad,
            gap,
            font_size,
            style,
            children,
        };
    }

    // Legacy 11-field `UiNode`
    let kind = UiKind::from_str_kind(&get_str(0));
    let text = get_str(1);
    let action = get_str(2);
    let bg = get_str(3);
    let color = get_str(4);
    let border = get_str(5);
    let radius = get_i64(6).max(0) as f32;
    let pad = get_i64(7).max(0) as f32;
    let gap = get_i64(8).max(0) as f32;
    let fs_raw = get_i64(9);
    let font_size = if fs_raw > 0 { fs_raw as f32 } else { 16.0 };

    let mut children = Vec::new();
    let vec_ptr = get_i64(10) as *const RtVec;
    if !vec_ptr.is_null() {
        let v = &*vec_ptr;
        for &child_raw in v {
            if !child_raw.is_null() {
                children.push(rt_struct_to_ui_node(child_raw as *const RtStruct));
            }
        }
    }

    let style = UiStyle {
        radius_corners: [radius; 4],
        pad_edges: [pad; 4],
        gap_x: gap,
        gap_y: gap,
        ..UiStyle::default()
    };

    UiNode {
        key: action.clone(),
        kind,
        text,
        action,
        value: 0.0,
        uniforms: Vec::new(),
        bg,
        color,
        border,
        radius,
        pad,
        gap,
        font_size,
        style,
        children,
    }
}

// ============================================================================
// Standalone Native Desktop GUI C-ABI
// ============================================================================

#[no_mangle]
#[allow(clippy::too_many_arguments)]
pub extern "C" fn rt_gui_launch_native(
    title_ptr: *const c_char,
    width: u32,
    height: u32,
    init_fn: Option<extern "C" fn() -> i64>,
    view_fn: Option<extern "C" fn(i64) -> i64>,
    update_fn: Option<extern "C" fn(i64, *const c_char) -> i64>,
    tick2_fn: Option<extern "C" fn(i64, f64) -> i64>,
    tick3_fn: Option<extern "C" fn(i64, f64, f64) -> i64>,
    on_slider_fn: Option<extern "C" fn(i64, *const c_char, f64) -> i64>,
    on_input_fn: Option<extern "C" fn(i64, *const c_char, *const c_char) -> i64>,
    shaders_json_ptr: *const c_char,
) -> i64 {
    use crate::gui::{launch_ui_app, UiEvent, WindowBackend, WindowConfig};
    use std::cell::Cell;

    let (Some(init), Some(view)) = (init_fn, view_fn) else {
        eprintln!("[GUI-NATIVE] Missing required init() or view() entrypoint");
        return 1;
    };

    let raw_title = unsafe { read_c_str(title_ptr) };
    let title = if raw_title.is_empty() {
        "knr native app".to_string()
    } else {
        raw_title
    };
    let w = if width == 0 { 1120 } else { width };
    let h = if height == 0 { 800 } else { height };
    let config = WindowConfig::interactive(title, w, h);

    let shaders_json = unsafe { read_c_str(shaders_json_ptr) };
    let shaders_wgsl: HashMap<String, String> = if shaders_json.is_empty() {
        HashMap::new()
    } else {
        serde_json::from_str(&shaders_json).unwrap_or_default()
    };

    let init_state = init();
    let state_cell = Cell::new(init_state);
    let has_tick = tick3_fn.is_some() || tick2_fn.is_some();

    let launch_res = launch_ui_app(
        config,
        shaders_wgsl,
        has_tick,
        || {
            let raw_ptr = view(state_cell.get());
            unsafe { rt_struct_to_ui_node(raw_ptr as *const RtStruct) }
        },
        |ev| {
            let st = state_cell.get();
            match ev {
                UiEvent::Click(action) | UiEvent::Toggle { action, .. } => {
                    if let Some(update) = update_fn {
                        let c_act = leak_c_string(&action) as *const c_char;
                        state_cell.set(update(st, c_act));
                    }
                }
                UiEvent::SliderChange { action, value } => {
                    if let Some(on_slider) = on_slider_fn {
                        let c_act = leak_c_string(&action) as *const c_char;
                        state_cell.set(on_slider(st, c_act, value));
                    } else if let Some(update) = update_fn {
                        let payload = format!("{action}:{value:.3}");
                        let c_payload = leak_c_string(&payload) as *const c_char;
                        state_cell.set(update(st, c_payload));
                    }
                }
                UiEvent::TextInput { action, value } => {
                    if let Some(on_input) = on_input_fn {
                        let c_act = leak_c_string(&action) as *const c_char;
                        let c_val = leak_c_string(&value) as *const c_char;
                        state_cell.set(on_input(st, c_act, c_val));
                    } else if let Some(update) = update_fn {
                        let payload = format!("{action}:{value}");
                        let c_payload = leak_c_string(&payload) as *const c_char;
                        state_cell.set(update(st, c_payload));
                    }
                }
            }
        },
        |dt, elapsed| {
            let st = state_cell.get();
            if let Some(tick3) = tick3_fn {
                state_cell.set(tick3(st, dt, elapsed));
            } else if let Some(tick2) = tick2_fn {
                state_cell.set(tick2(st, dt));
            }
        },
    );

    match launch_res {
        Ok(res) => {
            if res.backend == WindowBackend::Headless {
                println!(
                    "[GUI-NATIVE] Headless rendered snapshot (hash=0x{:016x})",
                    res.final_pixel_hash
                );
            }
            0
        }
        Err(err) => {
            eprintln!("[GUI-NATIVE] Desktop window error: {err}");
            1
        }
    }
}

// ============================================================================
// Map C-ABI
// ============================================================================

/// Allocates a new empty runtime Map.
#[no_mangle]
pub extern "C" fn rt_map_new() -> *mut c_void {
    let map: Box<RtMap> = Box::default();
    Box::into_raw(map) as *mut c_void
}

/// Inserts a key-value pair into the runtime Map.
/// Modifies the map in-place and returns the map pointer.
#[no_mangle]
pub extern "C" fn rt_map_set(
    map: *mut c_void,
    key: *const u8,
    mut key_len: usize,
    val: *mut c_void,
) -> *mut c_void {
    if map.is_null() || key.is_null() {
        return map;
    }
    if key_len == 0 {
        key_len = unsafe { CStr::from_ptr(key as *const c_char).to_bytes().len() };
    }
    let map_ref = unsafe { &mut *(map as *mut RtMap) };
    let key_bytes = unsafe { std::slice::from_raw_parts(key, key_len) }.to_vec();
    map_ref.insert(key_bytes, val);
    map
}

/// Retrieves a value by key from the runtime Map, or NULL if not found.
#[no_mangle]
pub extern "C" fn rt_map_get(map: *mut c_void, key: *const u8, mut key_len: usize) -> *mut c_void {
    if map.is_null() || key.is_null() {
        return std::ptr::null_mut();
    }
    if key_len == 0 {
        key_len = unsafe { CStr::from_ptr(key as *const c_char).to_bytes().len() };
    }
    let map_ref = unsafe { &*(map as *const RtMap) };
    let key_bytes = unsafe { std::slice::from_raw_parts(key, key_len) };
    map_ref.get(key_bytes).copied().unwrap_or(std::ptr::null_mut())
}

/// Deletes a key from the runtime Map in-place and returns the map pointer.
#[no_mangle]
pub extern "C" fn rt_map_del(
    map: *mut c_void,
    key: *const u8,
    mut key_len: usize,
) -> *mut c_void {
    if map.is_null() || key.is_null() {
        return map;
    }
    if key_len == 0 {
        key_len = unsafe { CStr::from_ptr(key as *const c_char).to_bytes().len() };
    }
    let map_ref = unsafe { &mut *(map as *mut RtMap) };
    let key_bytes = unsafe { std::slice::from_raw_parts(key, key_len) };
    map_ref.remove(key_bytes);
    map
}

/// Deallocates a runtime Map created with `rt_map_new`.
#[no_mangle]
pub extern "C" fn rt_map_free(map: *mut c_void) {
    if !map.is_null() {
        unsafe {
            drop(Box::from_raw(map as *mut RtMap));
        }
    }
}

// ============================================================================
// Vector C-ABI
// ============================================================================

/// Allocates a new empty runtime Vector.
#[no_mangle]
pub extern "C" fn rt_vec_new() -> *mut c_void {
    let vec: Box<RtVec> = Box::default();
    Box::into_raw(vec) as *mut c_void
}

/// Appends a value to the runtime Vector.
/// Modifies the vector in-place and returns the vector pointer.
#[no_mangle]
pub extern "C" fn rt_vec_push(vec: *mut c_void, val: *mut c_void) -> *mut c_void {
    if vec.is_null() {
        return std::ptr::null_mut();
    }
    let vec_ref = unsafe { &mut *(vec as *mut RtVec) };
    vec_ref.push(val);
    vec
}

/// Retrieves a value at the specified index from the runtime Vector, or NULL if out of bounds.
#[no_mangle]
pub extern "C" fn rt_vec_get(vec: *mut c_void, index: usize) -> *mut c_void {
    if vec.is_null() {
        return std::ptr::null_mut();
    }
    let vec_ref = unsafe { &*(vec as *const RtVec) };
    vec_ref.get(index).copied().unwrap_or(std::ptr::null_mut())
}

/// Returns the number of elements in the runtime Vector.
#[no_mangle]
pub extern "C" fn rt_vec_len(vec: *mut c_void) -> usize {
    if vec.is_null() {
        return 0;
    }
    let vec_ref = unsafe { &*(vec as *const RtVec) };
    vec_ref.len()
}

/// Deallocates a runtime Vector created with `rt_vec_new`.
#[no_mangle]
pub extern "C" fn rt_vec_free(vec: *mut c_void) {
    if !vec.is_null() {
        unsafe {
            drop(Box::from_raw(vec as *mut RtVec));
        }
    }
}

// ============================================================================
// Aligned Buffer C-ABI
// ============================================================================

/// Allocates a memory buffer aligned to the specified alignment (default 64 bytes).
#[no_mangle]
pub extern "C" fn rt_buf_alloc_aligned(size: usize, align: usize) -> *mut u8 {
    let align = if align == 0 { 64 } else { align };
    if !align.is_power_of_two() {
        return std::ptr::null_mut();
    }
    let alloc_size = if size == 0 { align } else { size };
    let layout = match Layout::from_size_align(alloc_size, align) {
        Ok(l) => l,
        Err(_) => return std::ptr::null_mut(),
    };
    let ptr = unsafe { std::alloc::alloc_zeroed(layout) };
    if ptr.is_null() || !(ptr as usize).is_multiple_of(align) {
        if !ptr.is_null() {
            unsafe { std::alloc::dealloc(ptr, layout) };
        }
        return std::ptr::null_mut();
    }
    ptr
}

/// Deallocates an aligned memory buffer previously allocated by `rt_buf_alloc_aligned`.
#[no_mangle]
pub extern "C" fn rt_buf_free(ptr: *mut u8, size: usize, align: usize) {
    if ptr.is_null() {
        return;
    }
    let align = if align == 0 { 64 } else { align };
    if !align.is_power_of_two() {
        return;
    }
    let alloc_size = if size == 0 { align } else { size };
    if let Ok(layout) = Layout::from_size_align(alloc_size, align) {
        unsafe { std::alloc::dealloc(ptr, layout) };
    }
}
