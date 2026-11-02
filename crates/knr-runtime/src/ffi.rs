#![allow(clippy::not_unsafe_ptr_arg_deref)]

use std::alloc::Layout;
use std::collections::BTreeMap;
use std::ffi::{c_char, c_void, CStr};

type RtMap = BTreeMap<Vec<u8>, *mut c_void>;
type RtVec = Vec<*mut c_void>;

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

/// Deallocates a runtime Map created with `rt_map_new`.
#[no_mangle]
pub extern "C" fn rt_map_free(map: *mut c_void) {
    if !map.is_null() {
        unsafe {
            drop(Box::from_raw(map as *mut RtMap));
        }
    }
}

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
