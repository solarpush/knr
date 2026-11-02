use knr_runtime::*;
use std::ffi::c_void;

#[test]
fn test_c_abi_map_operations() {
    let map = rt_map_new();
    assert!(!map.is_null());

    let key1 = b"user_id";
    let val1 = 12345usize as *mut c_void;
    let ret_map = rt_map_set(map, key1.as_ptr(), key1.len(), val1);
    assert_eq!(ret_map, map);

    let key2 = b"session";
    let val2 = 67890usize as *mut c_void;
    rt_map_set(map, key2.as_ptr(), key2.len(), val2);

    let got1 = rt_map_get(map, key1.as_ptr(), key1.len());
    assert_eq!(got1, val1);

    let got2 = rt_map_get(map, key2.as_ptr(), key2.len());
    assert_eq!(got2, val2);

    let missing = b"not_present";
    let got_missing = rt_map_get(map, missing.as_ptr(), missing.len());
    assert!(got_missing.is_null());

    rt_map_free(map);
}

#[test]
fn test_c_abi_map_null_pointer_handling() {
    // Null map
    let key = b"key";
    let val = std::ptr::dangling_mut::<c_void>();
    assert_eq!(rt_map_set(std::ptr::null_mut(), key.as_ptr(), key.len(), val), std::ptr::null_mut());
    assert!(rt_map_get(std::ptr::null_mut(), key.as_ptr(), key.len()).is_null());
    rt_map_free(std::ptr::null_mut()); // No panic

    // Null key
    let map = rt_map_new();
    assert_eq!(rt_map_set(map, std::ptr::null(), 0, val), map);
    assert!(rt_map_get(map, std::ptr::null(), 0).is_null());
    rt_map_free(map);
}

#[test]
fn test_c_abi_vec_operations() {
    let vec = rt_vec_new();
    assert!(!vec.is_null());
    assert_eq!(rt_vec_len(vec), 0);

    let val1 = 100usize as *mut c_void;
    let val2 = 200usize as *mut c_void;
    let val3 = 300usize as *mut c_void;

    rt_vec_push(vec, val1);
    rt_vec_push(vec, val2);
    rt_vec_push(vec, val3);

    assert_eq!(rt_vec_len(vec), 3);
    assert_eq!(rt_vec_get(vec, 0), val1);
    assert_eq!(rt_vec_get(vec, 1), val2);
    assert_eq!(rt_vec_get(vec, 2), val3);

    // Out of bounds
    assert!(rt_vec_get(vec, 3).is_null());
    assert!(rt_vec_get(vec, 9999).is_null());

    rt_vec_free(vec);
}

#[test]
fn test_c_abi_vec_null_pointer_handling() {
    let val = 42usize as *mut c_void;
    assert!(rt_vec_push(std::ptr::null_mut(), val).is_null());
    assert_eq!(rt_vec_len(std::ptr::null_mut()), 0);
    assert!(rt_vec_get(std::ptr::null_mut(), 0).is_null());
    rt_vec_free(std::ptr::null_mut()); // No panic
}

#[test]
fn test_c_abi_aligned_buffer_alloc_and_free() {
    let sizes = [0, 1, 63, 64, 128, 4096];
    for &size in &sizes {
        let ptr = rt_buf_alloc_aligned(size, 64);
        assert!(!ptr.is_null());
        assert_eq!(ptr as usize % 64, 0, "Address must be 64-byte aligned");

        // Safely write to buffer if non-zero
        if size > 1 {
            unsafe {
                *ptr = 0xAA;
                *ptr.add(size - 1) = 0xBB;
                assert_eq!(*ptr, 0xAA);
                assert_eq!(*ptr.add(size - 1), 0xBB);
            }
        } else if size == 1 {
            unsafe {
                *ptr = 0xAA;
                assert_eq!(*ptr, 0xAA);
            }
        }

        rt_buf_free(ptr, size, 64);
    }

    // Free NULL should not panic
    rt_buf_free(std::ptr::null_mut(), 64, 64);
}
