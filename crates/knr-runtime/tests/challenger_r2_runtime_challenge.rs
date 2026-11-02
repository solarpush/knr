use knr_core::Effect;
use knr_runtime::{
    simd_xor, simd_xor_buffers, AlignedBuffer, HostPlatform, HwPlatform,
    RuntimeError, Value,
};

#[test]
fn test_adversarial_zero_sized_aligned_buffer_properties() {
    let mut b1 = AlignedBuffer::new_64(0).expect("Allocating 0-sized buffer failed");
    let b2 = AlignedBuffer::new_64(0).expect("Allocating 0-sized buffer failed");

    // Must be marked empty and length 0
    assert_eq!(b1.len(), 0);
    assert_eq!(b2.len(), 0);
    assert!(b1.is_empty());
    assert!(b2.is_empty());
    assert_eq!(b1.align(), 64);
    assert_eq!(b2.align(), 64);

    // Raw pointer must be strictly 64-byte aligned
    assert_eq!(b1.as_ptr() as usize % 64, 0);
    assert_eq!(b2.as_ptr() as usize % 64, 0);
    assert_eq!(b1.as_mut_ptr() as usize % 64, 0);

    // Slice pointer must retain the 64-byte alignment
    let s1 = b1.as_slice();
    let s2 = b2.as_slice();
    assert_eq!(s1.len(), 0);
    assert_eq!(s2.len(), 0);
    assert_eq!(s1.as_ptr() as usize % 64, 0, "Slice ptr must be 64-byte aligned");
    assert_eq!(s2.as_ptr() as usize % 64, 0, "Slice ptr must be 64-byte aligned");

    let ms1 = b1.as_mut_slice();
    assert_eq!(ms1.len(), 0);
    assert_eq!(ms1.as_ptr() as usize % 64, 0, "Mut slice ptr must be 64-byte aligned");

    // In-place simd_xor must succeed without UnalignedBuffer error
    let res_xor = simd_xor(&mut b1, &b2);
    assert!(res_xor.is_ok(), "simd_xor failed on zero-sized buffers: {:?}", res_xor);

    // simd_xor_buffers must return an empty 64-byte aligned buffer
    let res_buf = simd_xor_buffers(&b1, &b2);
    assert!(res_buf.is_ok(), "simd_xor_buffers failed on zero-sized buffers: {:?}", res_buf);
    let out = res_buf.unwrap();
    assert_eq!(out.len(), 0);
    assert!(out.is_empty());
    assert_eq!(out.as_ptr() as usize % 64, 0);
}

#[test]
fn test_adversarial_zero_sized_from_slice() {
    let empty_data: [u8; 0] = [];
    let mut b = AlignedBuffer::from_slice(&empty_data, 64).expect("from_slice failed");
    assert_eq!(b.len(), 0);
    assert!(b.is_empty());
    assert_eq!(b.as_ptr() as usize % 64, 0);
    assert_eq!(b.as_slice().as_ptr() as usize % 64, 0);

    let b2 = AlignedBuffer::new_64(0).unwrap();
    let res = simd_xor(&mut b, &b2);
    assert!(res.is_ok());
}

#[test]
fn test_adversarial_zero_sized_vs_non_zero_length_mismatch() {
    let mut b_zero = AlignedBuffer::new_64(0).unwrap();
    let b_64 = AlignedBuffer::new_64(64).unwrap();

    // 0 vs 64 must fail with InvalidArgument (length mismatch), NOT UnalignedBuffer
    let res1 = simd_xor(&mut b_zero, &b_64);
    assert!(
        matches!(res1, Err(RuntimeError::InvalidArgument(_))),
        "Expected InvalidArgument for len mismatch, got {:?}",
        res1
    );

    let mut b_64_mut = AlignedBuffer::new_64(64).unwrap();
    let res2 = simd_xor(&mut b_64_mut, &b_zero);
    assert!(
        matches!(res2, Err(RuntimeError::InvalidArgument(_))),
        "Expected InvalidArgument for len mismatch, got {:?}",
        res2
    );

    let res3 = simd_xor_buffers(&b_zero, &b_64);
    assert!(matches!(res3, Err(RuntimeError::InvalidArgument(_))));
}

#[test]
fn test_adversarial_hw_platform_zero_sized_host_calls() {
    let mut hw = HwPlatform::new();

    // 1. hw.alloc with 0 size
    let p1_val = hw.host_call(Effect::Hw, "hw.alloc", Value::I64(0)).expect("hw.alloc(0) failed");
    let p1 = p1_val.as_i64().expect("should return i64 ptr");
    assert_eq!(p1 as usize % 64, 0, "Allocated ptr for size 0 must be 64-byte aligned");

    let p2_val = hw.host_call(Effect::Hw, "hw.alloc", Value::I64(0)).expect("hw.alloc(0) failed");
    let p2 = p2_val.as_i64().expect("should return i64 ptr");
    assert_eq!(p2 as usize % 64, 0, "Allocated ptr for size 0 must be 64-byte aligned");

    // 2. hw.simd_xor with [p1, p2, 0]
    let xor_res = hw.host_call(
        Effect::Hw,
        "hw.simd_xor",
        Value::Vec(vec![Value::I64(p1), Value::I64(p2), Value::I64(0)]),
    ).expect("hw.simd_xor with len 0 failed");

    let xor_buf = xor_res.as_buf().expect("must return Value::Buf");
    assert_eq!(xor_buf.len(), 0);

    // 3. hw.free on 0-sized allocations
    let free1 = hw.host_call(Effect::Hw, "hw.free", Value::I64(p1)).expect("hw.free failed");
    assert_eq!(free1, Value::Void);
    let free2 = hw.host_call(Effect::Hw, "hw.free", Value::I64(p2)).expect("hw.free failed");
    assert_eq!(free2, Value::Void);
}

#[test]
fn test_adversarial_repeated_alloc_free_stress() {
    // Stress test allocator with 10,000 zero-sized and small buffers
    for _ in 0..10_000 {
        let b = AlignedBuffer::new_64(0).unwrap();
        assert_eq!(b.as_ptr() as usize % 64, 0);
        // implicit drop
    }
}
