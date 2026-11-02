use knr_core::Effect;
use knr_runtime::{
    is_aligned_64, simd_xor, simd_xor_buffers, AlignedBuffer, HostPlatform, HwPlatform,
    RuntimeError, Value,
};

#[test]
fn test_64_byte_aligned_allocations() {
    let sizes = [0, 1, 63, 64, 127, 128, 4096];
    for &size in &sizes {
        let buf = AlignedBuffer::new_64(size).expect("allocation should succeed");
        assert_eq!(buf.len(), size);
        assert!(
            is_aligned_64(buf.as_ptr()),
            "buffer of size {size} must be 64-byte aligned (ptr={:p})",
            buf.as_ptr()
        );
        assert_eq!(buf.as_ptr() as usize % 64, 0);
    }
}

#[test]
fn test_consecutive_aligned_allocations_and_free() {
    let mut buffers = Vec::new();
    for i in 0..16 {
        let buf = AlignedBuffer::new_64(64 * (i + 1)).expect("allocation should succeed");
        assert_eq!(buf.as_ptr() as usize % 64, 0);
        buffers.push(buf);
    }

    // Raw alloc & free
    for &size in &[1, 64, 256, 4096] {
        let ptr = HwPlatform::alloc_aligned_buf(size).expect("raw alloc should succeed");
        assert!(!ptr.is_null());
        assert_eq!(ptr as usize % 64, 0);
        HwPlatform::free_aligned_buf(ptr, size).expect("raw free should succeed");
    }
}

#[test]
fn test_aligned_buffer_from_slice() {
    let source_data = (0..128).map(|x| (x * 3) as u8).collect::<Vec<u8>>();
    let aligned = AlignedBuffer::from_slice(&source_data, 64).expect("from_slice should succeed");
    assert_eq!(aligned.len(), 128);
    assert_eq!(aligned.as_slice(), source_data.as_slice());
    assert_eq!(aligned.as_ptr() as usize % 64, 0);
}

#[test]
fn test_simd_xor_accuracy() {
    let size = 256;
    let data_a: Vec<u8> = (0..size).map(|i| (i * 7 + 1) as u8).collect();
    let data_b: Vec<u8> = (0..size).map(|i| (i * 13 + 5) as u8).collect();

    let mut buf_a = AlignedBuffer::from_slice(&data_a, 64).unwrap();
    let buf_b = AlignedBuffer::from_slice(&data_b, 64).unwrap();

    simd_xor(&mut buf_a, &buf_b).expect("simd_xor should succeed");

    // Reference scalar XOR comparison
    for i in 0..size {
        let expected = data_a[i] ^ data_b[i];
        assert_eq!(
            buf_a[i], expected,
            "Byte at index {i} mismatch: got {}, expected {}",
            buf_a[i], expected
        );
    }
}

#[test]
fn test_simd_xor_with_itself_produces_all_zeros() {
    let size = 128;
    let data: Vec<u8> = (0..size).map(|i| (i * 11 + 3) as u8).collect();
    let mut buf_a = AlignedBuffer::from_slice(&data, 64).unwrap();
    let buf_b = AlignedBuffer::from_slice(&data, 64).unwrap();

    simd_xor(&mut buf_a, &buf_b).expect("simd_xor should succeed");

    for (i, &byte) in buf_a.iter().enumerate() {
        assert_eq!(byte, 0, "Byte at index {i} should be 0, got {byte}");
    }
}

#[test]
fn test_simd_xor_unaligned_buffer_rejection() {
    let buf1 = AlignedBuffer::new_64(128).unwrap();
    let buf2 = AlignedBuffer::new_64(128).unwrap();

    // Create intentionally unaligned slices (offset by 1 byte)
    let unaligned_1 = &buf1.as_slice()[1..65];
    let unaligned_2 = &buf2.as_slice()[1..65];

    let mut target = [0u8; 64];
    let err = simd_xor(&mut target, unaligned_2);
    assert!(
        matches!(err, Err(RuntimeError::UnalignedBuffer)),
        "Expected UnalignedBuffer error, got {:?}",
        err
    );

    let err2 = simd_xor_buffers(unaligned_1, unaligned_2);
    assert!(
        matches!(err2, Err(RuntimeError::UnalignedBuffer)),
        "Expected UnalignedBuffer error, got {:?}",
        err2
    );
}

#[test]
fn test_simd_xor_length_mismatch() {
    let mut buf_a = AlignedBuffer::new_64(64).unwrap();
    let buf_b = AlignedBuffer::new_64(128).unwrap();

    let err = simd_xor(&mut buf_a, &buf_b);
    assert!(matches!(err, Err(RuntimeError::InvalidArgument(_))));
}

#[test]
fn test_hw_platform_mmio_and_host_calls() {
    let mut hw = HwPlatform::new();

    // MMIO register write and read
    hw.host_call(
        Effect::Hw,
        "hw.write_reg",
        Value::Vec(vec![Value::I64(0x2000), Value::I64(0xBEEF)]),
    )
    .expect("write_reg should succeed");

    let val = hw
        .host_call(Effect::Hw, "hw.read_reg", Value::I64(0x2000))
        .expect("read_reg should succeed");
    assert_eq!(val, Value::I64(0xBEEF));

    // hw.alloc and hw.free
    let ptr_val = hw
        .host_call(Effect::Hw, "hw.alloc", Value::I64(128))
        .expect("hw.alloc should succeed");
    let ptr = ptr_val.as_i64().expect("should return i64 ptr");
    assert_eq!(ptr as usize % 64, 0);

    hw.host_call(Effect::Hw, "hw.free", Value::I64(ptr))
        .expect("hw.free should succeed");

    // hw.mmio_map
    let mmio = hw
        .host_call(
            Effect::Hw,
            "hw.mmio_map",
            Value::Vec(vec![Value::I64(0x8000), Value::I64(64)]),
        )
        .expect("mmio_map should succeed");
    assert_eq!(mmio.as_buf().unwrap().len(), 64);

    // hw.simd_xor with pointers
    let p1 = HwPlatform::alloc_aligned_buf(64).unwrap();
    let p2 = HwPlatform::alloc_aligned_buf(64).unwrap();
    unsafe {
        std::slice::from_raw_parts_mut(p1, 64).fill(0xAA);
        std::slice::from_raw_parts_mut(p2, 64).fill(0x55);
    }

    let xor_res = hw
        .host_call(
            Effect::Hw,
            "hw.simd_xor",
            Value::Vec(vec![Value::I64(p1 as i64), Value::I64(p2 as i64), Value::I64(64)]),
        )
        .expect("simd_xor with ptrs should succeed");

    let xor_buf = xor_res.as_buf().unwrap();
    assert_eq!(xor_buf.len(), 64);
    assert_eq!(xor_buf[0], 0xAA ^ 0x55); // 0xFF

    HwPlatform::free_aligned_buf(p1, 64).unwrap();
    HwPlatform::free_aligned_buf(p2, 64).unwrap();
}

#[test]
fn test_simd_xor_zero_length_buffers_handling() {
    let mut b1 = AlignedBuffer::new_64(0).unwrap();
    let b2 = AlignedBuffer::new_64(0).unwrap();

    // Verify 64-byte alignment of raw pointers and slices
    assert_eq!(b1.as_ptr() as usize % 64, 0);
    assert_eq!(b2.as_ptr() as usize % 64, 0);
    assert_eq!(b1.as_slice().as_ptr() as usize % 64, 0);
    assert_eq!(b2.as_slice().as_ptr() as usize % 64, 0);

    // In-place simd_xor
    let res = simd_xor(&mut b1, &b2);
    assert!(res.is_ok(), "simd_xor on 0-length buffers must succeed");

    // Returning new buffer
    let res2 = simd_xor_buffers(&b1, &b2);
    assert!(res2.is_ok(), "simd_xor_buffers on 0-length buffers must succeed");
    let out = res2.unwrap();
    assert_eq!(out.len(), 0);
    assert_eq!(out.as_ptr() as usize % 64, 0);
}

