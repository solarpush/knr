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
    use crate::gui::{UiKind, UiNode};
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

    UiNode {
        kind,
        text,
        action,
        bg,
        color,
        border,
        radius,
        pad,
        gap,
        font_size,
        children,
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
