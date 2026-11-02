//! Tier 5 Adversarial Hardening Test Suite for knr-runtime
//!
//! Focus areas:
//! 1. SIMD alignment enforcement and rejection of unaligned addresses
//! 2. Empty buffer operations and zero-length boundaries in SIMD XOR
//! 3. HwPlatform negative parameter rejection and boundary handling
//! 4. C-ABI FFI null-pointer safety across all external runtime bindings

use knr_core::Effect;
use knr_runtime::hw::{is_aligned_64, simd_xor, simd_xor_buffers, AlignedBuffer, HwPlatform};
use knr_runtime::platform::{HostPlatform, RuntimeError, Value};
use knr_runtime::{
    rt_buf_alloc_aligned, rt_buf_free, rt_map_free, rt_map_get, rt_map_new, rt_map_set,
    rt_vec_free, rt_vec_get, rt_vec_len, rt_vec_new, rt_vec_push,
};
use std::collections::BTreeMap;
use std::ffi::c_void;

// ============================================================================
// 1. SIMD Alignment Enforcement
// ============================================================================

#[test]
fn test_adv_simd_alignment_strict_rejection() {
    let mut aligned_a = AlignedBuffer::new_64(128).unwrap();
    let aligned_b = AlignedBuffer::new_64(128).unwrap();

    assert!(is_aligned_64(aligned_a.as_ptr()));
    assert!(is_aligned_64(aligned_b.as_ptr()));

    // Valid aligned XOR succeeds
    assert!(simd_xor(&mut aligned_a, &aligned_b).is_ok());

    // Slice with 1-byte offset breaks 64-byte alignment
    let unaligned_dst = &mut aligned_a[1..65];
    let aligned_src = &aligned_b[0..64];
    assert!(!is_aligned_64(unaligned_dst.as_ptr()));

    let res = simd_xor(unaligned_dst, aligned_src);
    assert!(
        matches!(res, Err(RuntimeError::UnalignedBuffer)),
        "Must reject unaligned dst buffer"
    );

    // Unaligned src buffer
    let aligned_dst = &mut aligned_a[0..64];
    let unaligned_src = &aligned_b[1..65];
    let res = simd_xor(aligned_dst, unaligned_src);
    assert!(
        matches!(res, Err(RuntimeError::UnalignedBuffer)),
        "Must reject unaligned src buffer"
    );
}

// ============================================================================
// 2. Empty and Boundary Buffer Handling
// ============================================================================

#[test]
fn test_adv_simd_empty_and_zero_size_handling() {
    // 1. Both empty
    let mut empty_a: [u8; 0] = [];
    let empty_b: [u8; 0] = [];
    assert!(simd_xor(&mut empty_a, &empty_b).is_ok());

    let out = simd_xor_buffers(&empty_a, &empty_b).expect("empty XOR buffers");
    assert_eq!(out.len(), 0);
    assert!(out.is_empty());

    // 2. Length mismatch with one empty and one non-empty:
    // 2a. Unaligned empty slice triggers UnalignedBuffer due to pointer alignment check before length
    let mut non_empty_a = AlignedBuffer::new_64(64).unwrap();
    let unaligned_empty_slice: &[u8] = &[];
    let res_unaligned = simd_xor(&mut non_empty_a, unaligned_empty_slice);
    assert!(
        matches!(res_unaligned, Err(RuntimeError::UnalignedBuffer)),
        "Dangling slice &[] fails 64-byte alignment check"
    );

    // 2b. Aligned 0-byte buffer triggers InvalidArgument (length mismatch 64 vs 0)
    let aligned_empty = AlignedBuffer::new_64(0).unwrap();
    let res_aligned = simd_xor(&mut non_empty_a, &aligned_empty);
    assert!(
        matches!(res_aligned, Err(RuntimeError::InvalidArgument(_))),
        "Aligned 0-byte buffer fails length check (64 != 0)"
    );

    // 3. Self-XOR on 64 bytes yields all zeros
    for byte in non_empty_a.as_mut_slice() {
        *byte = 0xAA;
    }
    let copy_src = AlignedBuffer::from_slice(&non_empty_a, 64).unwrap();
    simd_xor(&mut non_empty_a, &copy_src).expect("self XOR");
    assert!(non_empty_a.iter().all(|&b| b == 0));
}

// ============================================================================
// 3. HwPlatform Host Call Boundary & Negative Parameter Rejection
// ============================================================================

#[test]
fn test_adv_hw_platform_boundary_rejection() {
    let mut hw = HwPlatform::new();

    // 1. hw.alloc with negative size must be rejected
    let res = hw.host_call(Effect::Hw, "hw.alloc", Value::I64(-1));
    assert!(
        matches!(res, Err(RuntimeError::InvalidArgument(_))),
        "Must reject negative allocation size"
    );

    let mut map_neg = BTreeMap::new();
    map_neg.insert("size".to_string(), Value::I64(-100));
    let res = hw.host_call(Effect::Hw, "hw.alloc", Value::Map(map_neg));
    assert!(
        matches!(res, Err(RuntimeError::InvalidArgument(_))),
        "Must reject negative allocation size in map"
    );

    // 2. hw.mmio_map with negative or zero length must be rejected
    let res = hw.host_call(
        Effect::Hw,
        "hw.mmio_map",
        Value::Vec(vec![Value::I64(0x1000), Value::I64(0)]),
    );
    assert!(
        matches!(res, Err(RuntimeError::InvalidArgument(_))),
        "Must reject mmio_map with len = 0"
    );

    let res = hw.host_call(
        Effect::Hw,
        "hw.mmio_map",
        Value::Vec(vec![Value::I64(0x1000), Value::I64(-50)]),
    );
    assert!(
        matches!(res, Err(RuntimeError::InvalidArgument(_))),
        "Must reject mmio_map with negative len"
    );

    // 3. Unsupported effect on HwPlatform
    let res = hw.host_call(Effect::Net, "hw.alloc", Value::I64(64));
    assert!(
        matches!(res, Err(RuntimeError::UnsupportedEffect(Effect::Net))),
        "Must reject wrong effect"
    );

    // 4. Unknown operation
    let res = hw.host_call(Effect::Hw, "hw.unknown_op", Value::I64(0));
    assert!(
        matches!(res, Err(RuntimeError::InvalidOperation(_))),
        "Must reject unknown hw operation"
    );
}

// ============================================================================
// 4. C-ABI FFI Null Safety & Boundary Checks
// ============================================================================

#[test]
fn test_adv_ffi_null_safety_and_boundaries() {
    // 1. Map null safety
    let null_map: *mut c_void = std::ptr::null_mut();
    let dummy_key = b"key";

    // Set on null map returns null
    let res = rt_map_set(null_map, dummy_key.as_ptr(), dummy_key.len(), 42 as *mut c_void);
    assert!(res.is_null());

    // Valid map with null key
    let map = rt_map_new();
    assert!(!map.is_null());
    let res = rt_map_set(map, std::ptr::null(), 0, 42 as *mut c_void);
    assert_eq!(res, map);

    // Get on null map returns null
    let val = rt_map_get(null_map, dummy_key.as_ptr(), dummy_key.len());
    assert!(val.is_null());

    // Get with null key returns null
    let val = rt_map_get(map, std::ptr::null(), 0);
    assert!(val.is_null());

    // Free null map does not panic
    rt_map_free(null_map);
    // Free valid map
    rt_map_free(map);

    // 2. Vec null safety
    let null_vec: *mut c_void = std::ptr::null_mut();

    let res = rt_vec_push(null_vec, 123 as *mut c_void);
    assert!(res.is_null());

    let val = rt_vec_get(null_vec, 0);
    assert!(val.is_null());

    let len = rt_vec_len(null_vec);
    assert_eq!(len, 0);

    rt_vec_free(null_vec);

    // Valid vec bounds safety
    let vec = rt_vec_new();
    assert!(!vec.is_null());
    assert_eq!(rt_vec_len(vec), 0);

    // Out of bounds get returns null
    assert!(rt_vec_get(vec, 0).is_null());
    assert!(rt_vec_get(vec, 9999).is_null());
    assert!(rt_vec_get(vec, usize::MAX).is_null());

    // Push elements and check len
    rt_vec_push(vec, 100 as *mut c_void);
    rt_vec_push(vec, 200 as *mut c_void);
    assert_eq!(rt_vec_len(vec), 2);
    assert_eq!(rt_vec_get(vec, 0) as usize, 100);
    assert_eq!(rt_vec_get(vec, 1) as usize, 200);
    assert!(rt_vec_get(vec, 2).is_null());

    rt_vec_free(vec);

    // 3. Aligned buffer free null safety
    rt_buf_free(std::ptr::null_mut(), 0, 64);
    rt_buf_free(std::ptr::null_mut(), 1024, 64);

    let buf_ptr = rt_buf_alloc_aligned(128, 64);
    assert!(!buf_ptr.is_null());
    assert!(is_aligned_64(buf_ptr as *const u8));
    rt_buf_free(buf_ptr, 128, 64);
}
