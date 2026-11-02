#![allow(clippy::not_unsafe_ptr_arg_deref)]

use crate::platform::{HostPlatform, RuntimeError, Value};
use knr_core::Effect;
use std::alloc::{alloc_zeroed, dealloc, Layout};
use std::collections::HashMap;
use std::ops::{Deref, DerefMut};
use std::sync::{Arc, Mutex};

/// Check if a pointer address is aligned to the given byte boundary.
#[inline]
pub fn is_aligned(ptr: *const u8, align: usize) -> bool {
    (ptr as usize).is_multiple_of(align)
}

/// Check if a pointer address is aligned to 64 bytes.
#[inline]
pub fn is_aligned_64(ptr: *const u8) -> bool {
    is_aligned(ptr, 64)
}

/// A heap-allocated byte buffer strictly aligned to a specified boundary (default 64 bytes).
#[derive(Debug)]
pub struct AlignedBuffer {
    ptr: *mut u8,
    size: usize,
    alloc_size: usize,
    align: usize,
}

// AlignedBuffer owns its memory exclusively and is safe to send/share across threads.
unsafe impl Send for AlignedBuffer {}
unsafe impl Sync for AlignedBuffer {}

impl AlignedBuffer {
    /// Allocates a new zero-initialized buffer with the given size and alignment.
    /// Alignment must be a power of two; if size is 0, a minimum alignment block is allocated.
    pub fn new(size: usize, align: usize) -> Result<Self, RuntimeError> {
        if align == 0 || !align.is_power_of_two() {
            return Err(RuntimeError::InvalidArgument(format!(
                "Alignment must be a power of two, got {align}"
            )));
        }
        // Avoid zero-sized allocation UB in std::alloc
        let alloc_size = if size == 0 { align } else { size };
        let layout = Layout::from_size_align(alloc_size, align)
            .map_err(|_| RuntimeError::UnalignedBuffer)?;

        let ptr = unsafe { alloc_zeroed(layout) };
        if ptr.is_null() {
            return Err(RuntimeError::Io("Aligned memory allocation failed".into()));
        }

        if !(ptr as usize).is_multiple_of(align) {
            unsafe { dealloc(ptr, layout) };
            return Err(RuntimeError::UnalignedBuffer);
        }

        Ok(Self {
            ptr,
            size,
            alloc_size,
            align,
        })
    }

    /// Allocates an aligned buffer with default 64-byte alignment.
    pub fn new_64(size: usize) -> Result<Self, RuntimeError> {
        Self::new(size, 64)
    }

    /// Creates an aligned buffer copying data from an existing slice.
    pub fn from_slice(data: &[u8], align: usize) -> Result<Self, RuntimeError> {
        let mut buf = Self::new(data.len(), align)?;
        if !data.is_empty() {
            buf.copy_from_slice(data);
        }
        Ok(buf)
    }

    /// Returns the raw pointer to the buffer.
    pub fn as_ptr(&self) -> *const u8 {
        self.ptr
    }

    /// Returns the raw mutable pointer to the buffer.
    pub fn as_mut_ptr(&mut self) -> *mut u8 {
        self.ptr
    }

    /// Returns the logical size of the buffer in bytes.
    pub fn len(&self) -> usize {
        self.size
    }

    /// Returns true if the buffer is empty.
    pub fn is_empty(&self) -> bool {
        self.size == 0
    }

    /// Returns the alignment of the buffer.
    pub fn align(&self) -> usize {
        self.align
    }

    /// Converts the buffer content into an unaligned Vec<u8>.
    pub fn to_vec(&self) -> Vec<u8> {
        self.as_slice().to_vec()
    }

    /// Returns the buffer as an immutable slice.
    pub fn as_slice(&self) -> &[u8] {
        if self.ptr.is_null() {
            &[]
        } else {
            unsafe { std::slice::from_raw_parts(self.ptr, self.size) }
        }
    }

    /// Returns the buffer as a mutable slice.
    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        if self.ptr.is_null() {
            &mut []
        } else {
            unsafe { std::slice::from_raw_parts_mut(self.ptr, self.size) }
        }
    }
}

impl Deref for AlignedBuffer {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        self.as_slice()
    }
}

impl DerefMut for AlignedBuffer {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.as_mut_slice()
    }
}

impl Drop for AlignedBuffer {
    fn drop(&mut self) {
        if !self.ptr.is_null() {
            if let Ok(layout) = Layout::from_size_align(self.alloc_size, self.align) {
                unsafe { dealloc(self.ptr, layout) };
            }
        }
    }
}

/// Executes vector SIMD XOR between two 64-byte aligned buffers.
/// Bitwise XOR is computed over 64-byte chunks in-place (`dst ^= src`).
/// Both `dst` and `src` must be strictly 64-byte aligned, otherwise `RuntimeError::UnalignedBuffer` is returned.
pub fn simd_xor(dst: &mut [u8], src: &[u8]) -> Result<(), RuntimeError> {
    if dst.is_empty() && src.is_empty() {
        return Ok(());
    }
    if !is_aligned_64(dst.as_ptr()) || !is_aligned_64(src.as_ptr()) {
        return Err(RuntimeError::UnalignedBuffer);
    }
    if dst.len() != src.len() {
        return Err(RuntimeError::InvalidArgument(format!(
            "Buffer length mismatch for SIMD XOR: dst len {} != src len {}",
            dst.len(),
            src.len()
        )));
    }

    let len = dst.len();
    let chunks_64 = len / 64;

    // Process 64-byte chunks using 8 x 64-bit word operations (auto-vectorizes to AVX-512 / NEON)
    for i in 0..chunks_64 {
        let base = i * 64;
        for word in 0..8 {
            let off = base + word * 8;
            let d = u64::from_ne_bytes(dst[off..off + 8].try_into().unwrap());
            let s = u64::from_ne_bytes(src[off..off + 8].try_into().unwrap());
            dst[off..off + 8].copy_from_slice(&(d ^ s).to_ne_bytes());
        }
    }

    // Process trailing remainder bytes
    let remainder_start = chunks_64 * 64;
    for i in remainder_start..len {
        dst[i] ^= src[i];
    }

    Ok(())
}

/// Performs SIMD XOR on two slices, returning a new 64-byte aligned buffer containing the result.
pub fn simd_xor_buffers(a: &[u8], b: &[u8]) -> Result<AlignedBuffer, RuntimeError> {
    if a.is_empty() && b.is_empty() {
        return AlignedBuffer::new_64(0);
    }
    if !is_aligned_64(a.as_ptr()) || !is_aligned_64(b.as_ptr()) {
        return Err(RuntimeError::UnalignedBuffer);
    }
    if a.len() != b.len() {
        return Err(RuntimeError::InvalidArgument(format!(
            "Buffer length mismatch: {} != {}",
            a.len(),
            b.len()
        )));
    }

    let mut out = AlignedBuffer::from_slice(a, 64)?;
    simd_xor(&mut out, b)?;
    Ok(out)
}

/// Hardware platform adapter implementing simulated MMIO, 64-byte aligned allocations, and SIMD operations.
#[derive(Debug, Clone, Default)]
pub struct HwPlatform {
    registers: Arc<Mutex<HashMap<u64, u64>>>,
    allocated_buffers: Arc<Mutex<HashMap<usize, Arc<Mutex<AlignedBuffer>>>>>,
    mmio_regions: Arc<Mutex<HashMap<u64, Arc<Mutex<AlignedBuffer>>>>>,
}

impl HwPlatform {
    /// Creates a new `HwPlatform`.
    pub fn new() -> Self {
        Self::default()
    }

    /// Allocates a 64-byte aligned memory buffer of the given size.
    pub fn alloc_aligned_buf(size: usize) -> Result<*mut u8, RuntimeError> {
        let buf = AlignedBuffer::new(size, 64)?;
        let ptr = buf.ptr;
        // Forget RAII guard so caller owns the raw memory
        std::mem::forget(buf);
        Ok(ptr)
    }

    /// Deallocates a 64-byte aligned memory buffer previously allocated by `alloc_aligned_buf`.
    pub fn free_aligned_buf(ptr: *mut u8, size: usize) -> Result<(), RuntimeError> {
        if ptr.is_null() {
            return Ok(());
        }
        let alloc_size = if size == 0 { 64 } else { size };
        let layout = Layout::from_size_align(alloc_size, 64)
            .map_err(|_| RuntimeError::UnalignedBuffer)?;
        unsafe { dealloc(ptr, layout) };
        Ok(())
    }

    /// Reads a simulated 64-bit MMIO register at the given address.
    pub fn read_reg(&self, addr: u64) -> u64 {
        *self.registers.lock().unwrap().get(&addr).unwrap_or(&0)
    }

    /// Writes a simulated 64-bit MMIO register at the given address.
    pub fn write_reg(&self, addr: u64, val: u64) {
        self.registers.lock().unwrap().insert(addr, val);
    }

    /// Maps a simulated MMIO region into an aligned buffer view.
    pub fn mmio_map(&self, addr: u64, len: usize) -> Result<Vec<u8>, RuntimeError> {
        let mut regions = self.mmio_regions.lock().unwrap();
        let region = regions.entry(addr).or_insert_with(|| {
            Arc::new(Mutex::new(
                AlignedBuffer::new(len, 64).expect("MMIO buffer allocation failed"),
            ))
        });
        let guard = region.lock().unwrap();
        Ok(guard.to_vec())
    }
}

impl HostPlatform for HwPlatform {
    fn host_call(&mut self, effect: Effect, op: &str, payload: Value) -> Result<Value, RuntimeError> {
        if effect != Effect::Hw {
            return Err(RuntimeError::UnsupportedEffect(effect));
        }

        match op {
            "hw.alloc" => {
                let size = match payload {
                    Value::I64(s) if s >= 0 => s as usize,
                    Value::Map(m) => {
                        let s = m.get("size").and_then(Value::as_i64).unwrap_or(64);
                        if s < 0 {
                            return Err(RuntimeError::InvalidArgument("size must be non-negative".into()));
                        }
                        s as usize
                    }
                    _ => {
                        return Err(RuntimeError::InvalidArgument(
                            "hw.alloc expects i64 size or map with size".into(),
                        ))
                    }
                };
                let buf = AlignedBuffer::new(size, 64)?;
                let ptr = buf.ptr as usize;
                self.allocated_buffers
                    .lock()
                    .unwrap()
                    .insert(ptr, Arc::new(Mutex::new(buf)));
                Ok(Value::I64(ptr as i64))
            }

            "hw.free" => {
                let ptr = match payload {
                    Value::I64(p) => p as usize,
                    _ => {
                        return Err(RuntimeError::InvalidArgument(
                            "hw.free expects i64 pointer".into(),
                        ))
                    }
                };
                let removed = self.allocated_buffers.lock().unwrap().remove(&ptr);
                if removed.is_some() {
                    Ok(Value::Void)
                } else {
                    Err(RuntimeError::InvalidArgument("Unknown buffer pointer to free".into()))
                }
            }

            "hw.read_reg" => {
                let addr = match payload {
                    Value::I64(a) => a as u64,
                    Value::Map(m) => m
                        .get("addr")
                        .and_then(Value::as_i64)
                        .map(|a| a as u64)
                        .unwrap_or(0),
                    _ => {
                        return Err(RuntimeError::InvalidArgument(
                            "hw.read_reg expects i64 address or map with addr".into(),
                        ))
                    }
                };
                let val = self.read_reg(addr);
                Ok(Value::I64(val as i64))
            }

            "hw.write_reg" => {
                let (addr, val) = match payload {
                    Value::Vec(v) if v.len() >= 2 => {
                        let a = v[0].as_i64().ok_or_else(|| {
                            RuntimeError::InvalidArgument("addr must be i64".into())
                        })? as u64;
                        let val = v[1].as_i64().ok_or_else(|| {
                            RuntimeError::InvalidArgument("val must be i64".into())
                        })? as u64;
                        (a, val)
                    }
                    Value::Map(m) => {
                        let a = m
                            .get("addr")
                            .and_then(Value::as_i64)
                            .ok_or_else(|| {
                                RuntimeError::InvalidArgument("addr must be i64".into())
                            })? as u64;
                        let val = m
                            .get("val")
                            .and_then(Value::as_i64)
                            .ok_or_else(|| {
                                RuntimeError::InvalidArgument("val must be i64".into())
                            })? as u64;
                        (a, val)
                    }
                    _ => {
                        return Err(RuntimeError::InvalidArgument(
                            "hw.write_reg expects [addr, val] or {addr, val}".into(),
                        ))
                    }
                };
                self.write_reg(addr, val);
                Ok(Value::Void)
            }

            "hw.mmio_map" => {
                let (addr, len) = match payload {
                    Value::Vec(v) if v.len() >= 2 => {
                        let a = v[0].as_i64().ok_or_else(|| {
                            RuntimeError::InvalidArgument("addr must be i64".into())
                        })? as u64;
                        let l = v[1].as_i64().ok_or_else(|| {
                            RuntimeError::InvalidArgument("len must be i64".into())
                        })?;
                        if l <= 0 {
                            return Err(RuntimeError::InvalidArgument("len must be > 0".into()));
                        }
                        (a, l as usize)
                    }
                    Value::Map(m) => {
                        let a = m
                            .get("addr")
                            .and_then(Value::as_i64)
                            .ok_or_else(|| {
                                RuntimeError::InvalidArgument("addr must be i64".into())
                            })? as u64;
                        let l = m
                            .get("len")
                            .and_then(Value::as_i64)
                            .ok_or_else(|| {
                                RuntimeError::InvalidArgument("len must be i64".into())
                            })?;
                        if l <= 0 {
                            return Err(RuntimeError::InvalidArgument("len must be > 0".into()));
                        }
                        (a, l as usize)
                    }
                    _ => {
                        return Err(RuntimeError::InvalidArgument(
                            "hw.mmio_map expects [addr, len] or {addr, len}".into(),
                        ))
                    }
                };
                let bytes = self.mmio_map(addr, len)?;
                Ok(Value::Buf(bytes))
            }

            "hw.simd_xor" => {
                match payload {
                    Value::Vec(v) if v.len() >= 2 => {
                        match (&v[0], &v[1]) {
                            (Value::Buf(a), Value::Buf(b)) => {
                                let res = simd_xor_buffers(a, b)?;
                                Ok(Value::Buf(res.to_vec()))
                            }
                            (Value::I64(ptr_a), Value::I64(ptr_b)) => {
                                let len = if v.len() >= 3 {
                                    v[2].as_i64().unwrap_or(64) as usize
                                } else {
                                    64
                                };
                                let pa = *ptr_a as *const u8;
                                let pb = *ptr_b as *const u8;
                                if !is_aligned_64(pa) || !is_aligned_64(pb) {
                                    return Err(RuntimeError::UnalignedBuffer);
                                }
                                let sa = unsafe { std::slice::from_raw_parts(pa, len) };
                                let sb = unsafe { std::slice::from_raw_parts(pb, len) };
                                let res = simd_xor_buffers(sa, sb)?;
                                Ok(Value::Buf(res.to_vec()))
                            }
                            _ => Err(RuntimeError::InvalidArgument(
                                "hw.simd_xor expects two Buf values or two pointer addresses".into(),
                            )),
                        }
                    }
                    Value::Map(m) => {
                        let a = m.get("a").or_else(|| m.get("buf_a")).and_then(Value::as_buf);
                        let b = m.get("b").or_else(|| m.get("buf_b")).and_then(Value::as_buf);
                        match (a, b) {
                            (Some(buf_a), Some(buf_b)) => {
                                let res = simd_xor_buffers(buf_a, buf_b)?;
                                Ok(Value::Buf(res.to_vec()))
                            }
                            _ => Err(RuntimeError::InvalidArgument(
                                "hw.simd_xor map requires buf_a and buf_b keys".into(),
                            )),
                        }
                    }
                    _ => Err(RuntimeError::InvalidArgument(
                        "hw.simd_xor expects vector of 2 buffers or map with buf_a/buf_b".into(),
                    )),
                }
            }

            _ => Err(RuntimeError::InvalidOperation(format!(
                "Unknown hw operation: {op}"
            ))),
        }
    }
}
