use knr_core::Effect;
use knr_runtime::{
    rt_buf_alloc_aligned, rt_buf_free, rt_map_free, rt_map_get, rt_map_new, rt_map_set,
    rt_vec_free, rt_vec_get, rt_vec_len, rt_vec_new, rt_vec_push, simd_xor, simd_xor_buffers,
    AlignedBuffer, HostPlatform, HwPlatform, RuntimeError, UiPlatform, VNode, Value,
};
use std::collections::BTreeMap;

// ============================================================================
// 1. HARDWARE ALIGNMENT & UNALIGNED BUFFER REJECTION
// ============================================================================

#[test]
fn test_unaligned_buffer_rejection_all_offsets() {
    let mut aligned_dst = AlignedBuffer::new_64(256).expect("alloc failed");
    let aligned_src = AlignedBuffer::new_64(256).expect("alloc failed");

    // Every unaligned offset from 1..64 must be rejected by simd_xor
    for offset in 1..64 {
        let unaligned_dst_slice = &mut aligned_dst.as_mut_slice()[offset..offset + 128];
        let aligned_src_slice = &aligned_src.as_slice()[0..128];
        let res = simd_xor(unaligned_dst_slice, aligned_src_slice);
        assert!(
            matches!(res, Err(RuntimeError::UnalignedBuffer)),
            "Expected UnalignedBuffer when dst offset={offset}, got {:?}",
            res
        );

        let aligned_dst_slice = &mut aligned_dst.as_mut_slice()[0..128];
        let unaligned_src_slice = &aligned_src.as_slice()[offset..offset + 128];
        let res2 = simd_xor(aligned_dst_slice, unaligned_src_slice);
        assert!(
            matches!(res2, Err(RuntimeError::UnalignedBuffer)),
            "Expected UnalignedBuffer when src offset={offset}, got {:?}",
            res2
        );

        let unaligned_dst_slice2 = &mut aligned_dst.as_mut_slice()[offset..offset + 128];
        let unaligned_src_slice2 = &aligned_src.as_slice()[offset..offset + 128];
        let res3 = simd_xor(unaligned_dst_slice2, unaligned_src_slice2);
        assert!(
            matches!(res3, Err(RuntimeError::UnalignedBuffer)),
            "Expected UnalignedBuffer when both offset={offset}, got {:?}",
            res3
        );
    }
}

#[test]
fn test_unaligned_buffer_rejection_in_simd_xor_buffers() {
    let aligned_a = AlignedBuffer::new_64(128).unwrap();
    let aligned_b = AlignedBuffer::new_64(128).unwrap();

    let unaligned_a = &aligned_a.as_slice()[1..65];
    let aligned_slice = &aligned_b.as_slice()[0..64];

    let res = simd_xor_buffers(unaligned_a, aligned_slice);
    assert!(matches!(res, Err(RuntimeError::UnalignedBuffer)));

    let res2 = simd_xor_buffers(aligned_slice, unaligned_a);
    assert!(matches!(res2, Err(RuntimeError::UnalignedBuffer)));
}

#[test]
fn test_aligned_buffer_invalid_alignment_rejection() {
    // Non-power-of-2 alignments
    for invalid_align in [0, 3, 5, 7, 9, 15, 31, 63, 65, 127] {
        let res = AlignedBuffer::new(64, invalid_align);
        assert!(
            matches!(res, Err(RuntimeError::InvalidArgument(_))),
            "Expected InvalidArgument for align={invalid_align}, got {:?}",
            res
        );
    }
}

#[test]
fn test_hw_platform_simd_xor_unaligned_raw_pointer_rejection() {
    let mut hw = HwPlatform::new();
    let p1 = HwPlatform::alloc_aligned_buf(128).unwrap();
    let p2 = HwPlatform::alloc_aligned_buf(128).unwrap();

    // Offset p1 by 1 byte to make it unaligned
    let unaligned_p1 = (p1 as usize + 1) as i64;
    let aligned_p2 = p2 as i64;

    let res = hw.host_call(
        Effect::Hw,
        "hw.simd_xor",
        Value::Vec(vec![
            Value::I64(unaligned_p1),
            Value::I64(aligned_p2),
            Value::I64(64),
        ]),
    );
    assert!(
        matches!(res, Err(RuntimeError::UnalignedBuffer)),
        "Expected UnalignedBuffer error for unaligned pointer, got {:?}",
        res
    );

    HwPlatform::free_aligned_buf(p1, 128).unwrap();
    HwPlatform::free_aligned_buf(p2, 128).unwrap();
}

// ============================================================================
// 2. SIMD XOR CORRECTNESS ACROSS SIZES & MATHEMATICAL PROPERTIES
// ============================================================================

#[test]
fn test_simd_xor_zero_size_empty_buffer_bug() {
    let mut buf_a = AlignedBuffer::new_64(0).unwrap();
    let buf_b = AlignedBuffer::new_64(0).unwrap();

    // AlignedBuffer::new_64(0) allocated 64-byte aligned memory at buf.as_ptr()
    assert_eq!(buf_a.as_ptr() as usize % 64, 0);
    assert_eq!(buf_b.as_ptr() as usize % 64, 0);

    // With the fix, as_slice() retains the 64-byte aligned base pointer even when size == 0
    let slice_ptr_a = buf_a.as_slice().as_ptr() as usize;
    let slice_ptr_b = buf_b.as_slice().as_ptr() as usize;
    assert_eq!(slice_ptr_a % 64, 0, "Empty AlignedBuffer slice must retain 64-byte alignment");
    assert_eq!(slice_ptr_b % 64, 0, "Empty AlignedBuffer slice must retain 64-byte alignment");

    let res = simd_xor(&mut buf_a, &buf_b);
    assert!(res.is_ok(), "simd_xor on 0-sized AlignedBuffers must succeed, got {res:?}");

    let res_buf = simd_xor_buffers(&buf_a, &buf_b);
    assert!(res_buf.is_ok(), "simd_xor_buffers on 0-sized AlignedBuffers must succeed, got {res_buf:?}");
}

#[test]
fn test_simd_xor_various_boundary_sizes() {
    let sizes = [1, 7, 8, 15, 16, 63, 64, 65, 127, 128, 129, 255, 256, 1024, 4096];
    for &size in &sizes {
        let data_a: Vec<u8> = (0..size).map(|i| ((i * 17 + 3) & 0xFF) as u8).collect();
        let data_b: Vec<u8> = (0..size).map(|i| ((i * 31 + 7) & 0xFF) as u8).collect();

        let mut buf_a = AlignedBuffer::from_slice(&data_a, 64).unwrap();
        let buf_b = AlignedBuffer::from_slice(&data_b, 64).unwrap();

        simd_xor(&mut buf_a, &buf_b).expect("simd_xor failed");

        for i in 0..size {
            let expected = data_a[i] ^ data_b[i];
            assert_eq!(
                buf_a[i], expected,
                "Mismatch at size {size}, byte index {i}"
            );
        }
    }
}

#[test]
fn test_simd_xor_mathematical_properties() {
    let size = 512;
    let data_a: Vec<u8> = (0..size).map(|i| (i ^ 0x5A) as u8).collect();
    let data_b: Vec<u8> = (0..size).map(|i| (i ^ 0xA5) as u8).collect();
    let zeros = vec![0u8; size];

    // Property 1: A ^ A == 0
    let mut buf_a1 = AlignedBuffer::from_slice(&data_a, 64).unwrap();
    let buf_a2 = AlignedBuffer::from_slice(&data_a, 64).unwrap();
    simd_xor(&mut buf_a1, &buf_a2).unwrap();
    assert_eq!(buf_a1.as_slice(), zeros.as_slice());

    // Property 2: A ^ 0 == A
    let mut buf_a = AlignedBuffer::from_slice(&data_a, 64).unwrap();
    let buf_0 = AlignedBuffer::from_slice(&zeros, 64).unwrap();
    simd_xor(&mut buf_a, &buf_0).unwrap();
    assert_eq!(buf_a.as_slice(), data_a.as_slice());

    // Property 3: Involutive ((A ^ B) ^ B == A)
    let mut buf_a = AlignedBuffer::from_slice(&data_a, 64).unwrap();
    let buf_b = AlignedBuffer::from_slice(&data_b, 64).unwrap();
    simd_xor(&mut buf_a, &buf_b).unwrap();
    simd_xor(&mut buf_a, &buf_b).unwrap();
    assert_eq!(buf_a.as_slice(), data_a.as_slice());

    // Property 4: Commutativity (A ^ B == B ^ A)
    let mut buf_ab = AlignedBuffer::from_slice(&data_a, 64).unwrap();
    let mut buf_ba = AlignedBuffer::from_slice(&data_b, 64).unwrap();
    simd_xor(&mut buf_ab, &AlignedBuffer::from_slice(&data_b, 64).unwrap()).unwrap();
    simd_xor(&mut buf_ba, &AlignedBuffer::from_slice(&data_a, 64).unwrap()).unwrap();
    assert_eq!(buf_ab.as_slice(), buf_ba.as_slice());
}

// ============================================================================
// 3. VDOM SSR HTML ESCAPING & STRUCTURE
// ============================================================================

#[test]
fn test_vdom_ssr_html_xss_escaping() {
    let mut props = BTreeMap::new();
    props.insert(
        "data-attr".to_string(),
        "\" onmouseover=\"alert('xss')\" <test> &".to_string(),
    );

    let node = VNode::element(
        "div",
        props,
        vec![VNode::text("<script>alert('pwned')</script> & <b>bold</b>")],
        None,
    );

    let html = node.render_html();
    // Verify no unescaped characters in text
    assert!(!html.contains("<script>"));
    assert!(html.contains("&lt;script&gt;alert(&#39;pwned&#39;)&lt;/script&gt;"));
    assert!(html.contains("&amp;"));
    assert!(html.contains("&lt;b&gt;bold&lt;/b&gt;"));

    // Verify attribute quotes and angle brackets are escaped
    assert!(!html.contains("\" onmouseover="));
    assert!(html.contains("&quot; onmouseover=&quot;alert(&#39;xss&#39;)&quot; &lt;test&gt; &amp;"));
}

#[test]
fn test_vdom_ssr_all_void_tags_case_insensitivity() {
    let void_tags = [
        "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param",
        "source", "track", "wbr", "BR", "HR", "IMG", "INPUT",
    ];

    for &tag in &void_tags {
        let mut props = BTreeMap::new();
        props.insert("id".to_string(), "v1".to_string());
        let node = VNode::element(tag, props, Vec::new(), None);
        let html = node.render_html();
        assert!(
            html.ends_with(" />"),
            "Tag '{tag}' should render as self-closing ' />', got: {html}"
        );
    }

    // Non-void tag should NOT be self-closing
    let div_node = VNode::element("div", BTreeMap::new(), Vec::new(), None);
    assert_eq!(div_node.render_html(), "<div></div>");
}

#[test]
fn test_vdom_ssr_deep_nesting() {
    let depth = 50;
    let mut current = VNode::text("deep content");
    for i in 0..depth {
        let mut props = BTreeMap::new();
        props.insert("level".to_string(), i.to_string());
        current = VNode::element("div", props, vec![current], None);
    }

    let html = current.render_html();
    assert!(html.starts_with("<div level=\"49\">"));
    assert!(html.contains("deep content"));
    assert!(html.ends_with("</div>"));
}

#[test]
fn test_ui_platform_host_call_render_and_events() {
    let mut ui = UiPlatform::new();

    let mut vdom_map = BTreeMap::new();
    vdom_map.insert("tag".to_string(), Value::Str("h1".to_string()));
    let mut props = BTreeMap::new();
    props.insert("class".to_string(), Value::Str("header".to_string()));
    vdom_map.insert("props".to_string(), Value::Map(props));
    vdom_map.insert("text".to_string(), Value::Str("Title & Subtitle".to_string()));
    vdom_map.insert("children".to_string(), Value::Vec(Vec::new()));

    let mount_res = ui
        .host_call(Effect::Ui, "ui.mount", Value::Map(vdom_map))
        .unwrap();
    assert!(matches!(mount_res, Value::Chan(_)));

    let render_res = ui
        .host_call(Effect::Ui, "ui.render", Value::Void)
        .unwrap();
    assert_eq!(
        render_res.as_str().unwrap(),
        "<h1 class=\"header\">Title &amp; Subtitle</h1>"
    );
}

// ============================================================================
// 4. C-ABI FFI MEMORY ALLOCATION & CONTAINER OPERATIONS
// ============================================================================

#[test]
fn test_ffi_aligned_buffer_strict_64_bytes() {
    for size in [0, 1, 63, 64, 65, 128, 512, 4096] {
        let ptr = rt_buf_alloc_aligned(size, 64);
        assert!(!ptr.is_null());
        assert_eq!(ptr as usize % 64, 0, "FFI buffer of size {size} not 64-byte aligned");

        // Write to allocated memory safely
        if size > 0 {
            unsafe {
                std::ptr::write_bytes(ptr, 0x42, size);
                assert_eq!(*ptr, 0x42);
            }
        }
        rt_buf_free(ptr, size, 64);
    }
}

#[test]
fn test_ffi_map_and_vec_stress() {
    let map = rt_map_new();
    assert!(!map.is_null());

    let count = 100;
    for i in 0..count {
        let k = format!("key_{i}");
        let val = (i * 100) as *mut std::ffi::c_void;
        rt_map_set(map, k.as_ptr(), k.len(), val);
    }

    for i in 0..count {
        let k = format!("key_{i}");
        let got = rt_map_get(map, k.as_ptr(), k.len());
        assert_eq!(got as usize, i * 100);
    }

    let missing = "missing_key";
    assert!(rt_map_get(map, missing.as_ptr(), missing.len()).is_null());
    rt_map_free(map);

    let vec = rt_vec_new();
    assert!(!vec.is_null());
    assert_eq!(rt_vec_len(vec), 0);

    for i in 0..count {
        rt_vec_push(vec, (i + 1) as *mut std::ffi::c_void);
    }
    assert_eq!(rt_vec_len(vec), count);

    for i in 0..count {
        let got = rt_vec_get(vec, i);
        assert_eq!(got as usize, i + 1);
    }
    // Out of bounds returns null
    assert!(rt_vec_get(vec, count + 10).is_null());
    rt_vec_free(vec);
}
