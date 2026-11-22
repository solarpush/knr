//! Comprehensive v0.3.0 Unified GPU/CPU UI Engine test suite:
//! 1. `<Scroll>` & `overflow="hidden"` pixel-exact clipping, scroll offsets, nested scroll hit-testing,
//!    and clipped-ancestor awareness in `audit_layout`.
//! 2. Real `<Image src="..." />` PNG file & `data:image/png;base64,...` decoding, caching, cover-fit
//!    rendering, broken-image fallback, and `MissingImage` audit detection.
//! 3. Production `<Input>` caret positioning, selection highlight rendering (`#38bdf844`),
//!    and click-to-caret hit testing.
//! 4. Non-blocking async MVU `UiCmd` parsing & execution (`timer`, `storage_save`/`storage_load`,
//!    `http_get`, `http_post`) + `launch_ui_app_with_reload_and_effects` integration.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use knr_runtime::gui::{
    audit_layout, audit_layout_with_images, compute_ui_layout, compute_ui_layout_with_scroll,
    hit_test, hit_test_scroll_target, launch_ui_app_with_reload_and_effects, pixmap_fnv_hash,
    spawn_ui_cmd, RenderInteractionState, UiCmd, UiEvent, UiKind, UiNode, UiRenderer, UiStyle,
    WindowConfig,
};
use knr_runtime::platform::Value;
use tiny_skia::{Color as SkColor, Pixmap};

fn encode_base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = if chunk.len() > 1 { chunk[1] as u32 } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] as u32 } else { 0 };
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[((n >> 18) & 63) as usize] as char);
        out.push(TABLE[((n >> 12) & 63) as usize] as char);
        if chunk.len() > 1 {
            out.push(TABLE[((n >> 6) & 63) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(TABLE[(n & 63) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

fn make_solid_png_bytes(w: u32, h: u32, r: u8, g: u8, b: u8) -> Vec<u8> {
    let mut pm = Pixmap::new(w, h).expect("create test png pixmap");
    pm.fill(SkColor::from_rgba8(r, g, b, 255));
    pm.encode_png().expect("encode solid png")
}

#[test]
fn test_scroll_and_overflow_hidden_pixel_clipping_and_scroll_offsets() {
    let renderer = UiRenderer::new();

    // Dark #0b1120 root with a 100px tall <Scroll key="feed"> at y=40..140 containing
    // 100px bright red box + 100px bright green box + hidden button below viewport,
    // plus an overflow="hidden" container at y=200..260 containing a 180px bright yellow box.
    let root = UiNode {
        key: "root".into(),
        kind: UiKind::Col,
        bg: "#0b1120".into(),
        style: UiStyle {
            pad_edges: [40.0, 20.0, 20.0, 20.0],
            gap_y: 60.0,
            ..UiStyle::default()
        },
        children: vec![
            UiNode {
                key: "feed".into(),
                kind: UiKind::Scroll,
                bg: "#0f172a".into(),
                style: UiStyle {
                    w: 300.0,
                    h: 100.0,
                    overflow: "scroll".into(),
                    ..UiStyle::default()
                },
                children: vec![
                    UiNode {
                        key: "red-item".into(),
                        kind: UiKind::Col,
                        bg: "#ef4444".into(),
                        style: UiStyle {
                            h: 100.0,
                            ..UiStyle::default()
                        },
                        children: vec![UiNode {
                            kind: UiKind::Text,
                            text: "Top Red Item".into(),
                            color: "#ffffff".into(),
                            font_size: 16.0,
                            ..UiNode::default()
                        }],
                        ..UiNode::default()
                    },
                    UiNode {
                        key: "green-item".into(),
                        kind: UiKind::Col,
                        bg: "#22c55e".into(),
                        style: UiStyle {
                            h: 100.0,
                            ..UiStyle::default()
                        },
                        children: vec![UiNode {
                            kind: UiKind::Button,
                            text: "Hidden Button".into(),
                            action: "hidden_btn".into(),
                            bg: "#16a34a".into(),
                            color: "#ffffff".into(),
                            style: UiStyle {
                                h: 40.0,
                                ..UiStyle::default()
                            },
                            ..UiNode::default()
                        }],
                        ..UiNode::default()
                    },
                ],
                ..UiNode::default()
            },
            UiNode {
                key: "hidden-box".into(),
                kind: UiKind::Col,
                bg: "#0f172a".into(),
                style: UiStyle {
                    w: 300.0,
                    h: 60.0,
                    overflow: "hidden".into(),
                    ..UiStyle::default()
                },
                children: vec![UiNode {
                    key: "yellow-overflow".into(),
                    kind: UiKind::Col,
                    bg: "#facc15".into(),
                    style: UiStyle {
                        h: 180.0,
                        ..UiStyle::default()
                    },
                    ..UiNode::default()
                }],
                ..UiNode::default()
            },
        ],
        ..UiNode::default()
    };

    // 1. Unscrolled render: red visible in feed (y=40..140), green & yellow overflow MUST NOT bleed!
    let (pm0, layout0, hbs0) = renderer.render_ui_with_shaders(
        &root,
        400,
        420,
        1.0,
        &RenderInteractionState::default(),
        None,
    );

    let scroll_node = &layout0.children[0];
    assert!((scroll_node.rect.h - 100.0).abs() < 0.5);
    assert!(
        scroll_node.content_height >= 199.0,
        "Scroll content_height should preserve full children height, got {}",
        scroll_node.content_height
    );
    // Hidden button at y=140..180 is outside scroll viewport y=40..140 -> clipped from hit_boxes!
    assert_eq!(hit_test(&hbs0, 100.0, 160.0), None);

    // Pixel inside scroll viewport (y=80) is red (#ef4444)
    let inside_red = pm0.pixel(100, 80).expect("inside red");
    assert!(inside_red.red() > 200 && inside_red.green() < 100);

    // Pixels STRICTLY below <Scroll> (y=145..195) MUST be pure root background #0b1120 (11, 17, 32)
    for y in [145u32, 160, 175, 190] {
        for x in [40u32, 100, 200, 280] {
            let px = pm0.pixel(x, y).expect("below scroll pixel");
            assert_eq!(
                (px.red(), px.green(), px.blue()),
                (11, 17, 32),
                "Leaked pixel below <Scroll> at ({x},{y}): {px:?}"
            );
        }
    }

    // Pixels STRICTLY below overflow="hidden" box (box is y=200..260; check y=265..380)
    // MUST be pure root background #0b1120 (11, 17, 32) — zero yellow bleed!
    for y in [265u32, 300, 350, 380] {
        for x in [40u32, 100, 200, 280] {
            let px = pm0.pixel(x, y).expect("below overflow:hidden pixel");
            assert_eq!(
                (px.red(), px.green(), px.blue()),
                (11, 17, 32),
                "Leaked pixel below overflow=hidden at ({x},{y}): {px:?}"
            );
        }
    }

    // Wheel hit-testing finds "feed" inside y=40..140, and None outside
    let target = hit_test_scroll_target(&layout0, 100.0, 80.0).expect("hit feed scroll target");
    assert_eq!(target.id, "feed");
    assert!((target.max_scroll_y - 100.0).abs() < 1.0);
    assert!(hit_test_scroll_target(&layout0, 100.0, 160.0).is_none());

    // 2. Scrolled by 100px: green item moves into y=40..140, red item moves ABOVE y=40 and MUST NOT bleed above!
    let mut offsets = HashMap::new();
    offsets.insert("feed".to_string(), 100.0f32);
    let scrolled_state = RenderInteractionState {
        scroll_offsets: Some(&offsets),
        ..Default::default()
    };
    let (pm1, layout1, hbs1) =
        renderer.render_ui_with_shaders(&root, 400, 420, 1.0, &scrolled_state, None);
    assert!((layout1.children[0].scroll_offset_y - 100.0).abs() < 0.5);

    // Inside scroll viewport at y=110 is now green (#22c55e)
    let inside_green = pm1.pixel(100, 110).expect("inside green");
    assert!(
        inside_green.green() > 160 && inside_green.red() < 80,
        "Expected scrolled green content inside <Scroll>, got {inside_green:?}"
    );
    // Hidden button is now scrolled into view at y=40..80 -> clickable!
    assert_eq!(hit_test(&hbs1, 100.0, 60.0), Some("hidden_btn"));

    // Pixels ABOVE <Scroll> (y=5..35) MUST remain pure root background #0b1120 (zero upward red bleed!)
    for y in [5u32, 15, 25, 35] {
        for x in [40u32, 100, 200] {
            let px = pm1.pixel(x, y).expect("above scroll pixel");
            assert_eq!(
                (px.red(), px.green(), px.blue()),
                (11, 17, 32),
                "Leaked scrolled-up pixel above <Scroll> at ({x},{y}): {px:?}"
            );
        }
    }

    // 3. Nested <Scroll> hit-testing picks innermost container first
    let nested = UiNode {
        key: "outer".into(),
        kind: UiKind::Scroll,
        style: UiStyle {
            w: 300.0,
            h: 150.0,
            ..UiStyle::default()
        },
        children: vec![
            UiNode {
                key: "inner".into(),
                kind: UiKind::Scroll,
                style: UiStyle {
                    w: 260.0,
                    h: 80.0,
                    ..UiStyle::default()
                },
                children: vec![UiNode {
                    kind: UiKind::Col,
                    style: UiStyle {
                        h: 220.0,
                        ..UiStyle::default()
                    },
                    ..UiNode::default()
                }],
                ..UiNode::default()
            },
            UiNode {
                kind: UiKind::Col,
                style: UiStyle {
                    h: 200.0,
                    ..UiStyle::default()
                },
                ..UiNode::default()
            },
        ],
        ..UiNode::default()
    };
    let (nested_layout, _) =
        compute_ui_layout_with_scroll(&nested, 400.0, 300.0, &HashMap::new(), &renderer);
    assert_eq!(
        hit_test_scroll_target(&nested_layout, 50.0, 40.0).map(|t| t.id),
        Some("inner".to_string())
    );
    assert_eq!(
        hit_test_scroll_target(&nested_layout, 50.0, 120.0).map(|t| t.id),
        Some("outer".to_string())
    );
}

#[test]
fn test_audit_layout_respects_clipped_scroll_ancestors_and_catches_unclipped_overflow() {
    let renderer = UiRenderer::new();

    // Child wider than viewport INSIDE a properly sized <Scroll> container -> NOT ViewportOverflow!
    let clipped_ok = UiNode {
        key: "root".into(),
        kind: UiKind::Col,
        bg: "#0b1120".into(),
        style: UiStyle {
            pad_edges: [16.0; 4],
            ..UiStyle::default()
        },
        children: vec![UiNode {
            key: "scroll-viewport".into(),
            kind: UiKind::Scroll,
            bg: "#0f172a".into(),
            style: UiStyle {
                w: 400.0,
                h: 120.0,
                ..UiStyle::default()
            },
            children: vec![UiNode {
                key: "wide-scrolled-child".into(),
                kind: UiKind::Card,
                bg: "#1e293b".into(),
                style: UiStyle {
                    w: 750.0,
                    h: 360.0,
                    ..UiStyle::default()
                },
                ..UiNode::default()
            }],
            ..UiNode::default()
        }],
        ..UiNode::default()
    };
    let (laid_ok, _) = compute_ui_layout(&clipped_ok, 600.0, 400.0, &renderer);
    let rep_ok = audit_layout(&laid_ok, 600.0, 400.0, &[], &|ch, ff, fw| {
        renderer.has_glyph(ch, ff, fw)
    });
    assert_eq!(
        rep_ok.status, "pass",
        "Clipped children inside <Scroll> must not trigger ViewportOverflow: {:?}",
        rep_ok.issues
    );
    assert!(
        rep_ok.content_height <= 405.0,
        "Clipped children must not inflate page content_height: {}",
        rep_ok.content_height
    );

    // Genuine unclipped top-level overflow MUST still be flagged!
    let mut unclipped_bad = clipped_ok;
    unclipped_bad.children.push(UiNode {
        key: "unclipped-wide".into(),
        kind: UiKind::Card,
        bg: "#1e293b".into(),
        style: UiStyle {
            w: 750.0,
            h: 40.0,
            ..UiStyle::default()
        },
        ..UiNode::default()
    });
    let (laid_bad, _) = compute_ui_layout(&unclipped_bad, 600.0, 400.0, &renderer);
    let rep_bad = audit_layout(&laid_bad, 600.0, 400.0, &[], &|ch, ff, fw| {
        renderer.has_glyph(ch, ff, fw)
    });
    assert_eq!(rep_bad.status, "fail");
    assert!(
        rep_bad
            .issues
            .iter()
            .any(|i| i.kind == "ViewportOverflow" && i.key == "unclipped-wide"),
        "Expected unclipped ViewportOverflow issue: {:?}",
        rep_bad.issues
    );
}

#[test]
fn test_image_file_and_base64_data_uri_rendering_and_missing_image_audit() {
    let renderer = UiRenderer::new();

    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let png_path = std::env::temp_dir().join(format!(
        "knr_v03_img_test_{}_{}.png",
        std::process::id(),
        stamp
    ));
    let magenta_png = make_solid_png_bytes(16, 16, 255, 0, 220);
    std::fs::write(&png_path, &magenta_png).expect("write test png");
    let png_path_str = png_path.to_string_lossy().into_owned();

    let cyan_png = make_solid_png_bytes(16, 16, 0, 235, 255);
    let data_uri = format!("data:image/png;base64,{}", encode_base64(&cyan_png));

    assert!(renderer.can_load_image(&png_path_str));
    assert!(renderer.can_load_image(&data_uri));
    assert!(!renderer.can_load_image(""));
    assert!(!renderer.can_load_image("/definitely/missing/knr_v03_image.png"));
    assert!(!renderer.can_load_image("data:image/png;base64,!!!not-base64!!!"));

    let valid_tree = UiNode {
        key: "img-root".into(),
        kind: UiKind::Col,
        bg: "#0b1120".into(),
        style: UiStyle {
            pad_edges: [20.0; 4],
            gap_y: 20.0,
            ..UiStyle::default()
        },
        children: vec![
            UiNode {
                key: "file-img".into(),
                kind: UiKind::Image,
                text: png_path_str.clone(),
                style: UiStyle {
                    w: 120.0,
                    h: 60.0,
                    radius_corners: [10.0; 4],
                    ..UiStyle::default()
                },
                ..UiNode::default()
            },
            UiNode {
                key: "b64-img".into(),
                kind: UiKind::Image,
                text: data_uri.clone(),
                style: UiStyle {
                    w: 120.0,
                    h: 60.0,
                    radius_corners: [10.0; 4],
                    ..UiStyle::default()
                },
                ..UiNode::default()
            },
        ],
        ..UiNode::default()
    };

    let (pm, layout, _) = renderer.render_ui_with_shaders(
        &valid_tree,
        240,
        200,
        1.0,
        &RenderInteractionState::default(),
        None,
    );

    // Center of first image (file PNG) must be bright magenta
    let r0 = layout.children[0].rect;
    let p0 = pm
        .pixel((r0.x + r0.w * 0.5) as u32, (r0.y + r0.h * 0.5) as u32)
        .expect("file image center pixel");
    assert!(
        p0.red() > 230 && p0.green() < 20 && p0.blue() > 200,
        "Expected decoded PNG file magenta pixel, got {p0:?}"
    );

    // Center of second image (base64 data URI) must be bright cyan
    let r1 = layout.children[1].rect;
    let p1 = pm
        .pixel((r1.x + r1.w * 0.5) as u32, (r1.y + r1.h * 0.5) as u32)
        .expect("base64 image center pixel");
    assert!(
        p1.red() < 20 && p1.green() > 215 && p1.blue() > 235,
        "Expected decoded base64 data URI cyan pixel, got {p1:?}"
    );

    // Valid images pass audit with 0 errors
    let rep_ok = audit_layout_with_images(
        &layout,
        240.0,
        200.0,
        &[],
        &|ch, ff, fw| renderer.has_glyph(ch, ff, fw),
        &|src| renderer.can_load_image(src),
    );
    assert_eq!(rep_ok.status, "pass", "Unexpected issues: {:?}", rep_ok.issues);

    // Missing / broken images render clean fallback AND trigger MissingImage audit errors
    let broken_tree = UiNode {
        key: "broken-img-root".into(),
        kind: UiKind::Col,
        bg: "#0b1120".into(),
        style: UiStyle {
            pad_edges: [16.0; 4],
            gap_y: 12.0,
            ..UiStyle::default()
        },
        children: vec![
            UiNode {
                key: "empty-img".into(),
                kind: UiKind::Image,
                text: String::new(),
                style: UiStyle {
                    w: 100.0,
                    h: 50.0,
                    ..UiStyle::default()
                },
                ..UiNode::default()
            },
            UiNode {
                key: "bad-path-img".into(),
                kind: UiKind::Image,
                text: "/nonexistent/knr_missing.png".into(),
                style: UiStyle {
                    w: 100.0,
                    h: 50.0,
                    ..UiStyle::default()
                },
                ..UiNode::default()
            },
        ],
        ..UiNode::default()
    };
    let (broken_pm, broken_layout, _) = renderer.render_ui_with_shaders(
        &broken_tree,
        240,
        180,
        1.0,
        &RenderInteractionState::default(),
        None,
    );
    assert_ne!(pixmap_fnv_hash(&broken_pm), 0);

    let rep_bad = audit_layout(&broken_layout, 240.0, 180.0, &[], &|ch, ff, fw| {
        renderer.has_glyph(ch, ff, fw)
    });
    assert_eq!(rep_bad.status, "fail");
    let missing_img_keys: Vec<&str> = rep_bad
        .issues
        .iter()
        .filter(|i| i.kind == "MissingImage" && i.severity == "error")
        .map(|i| i.key.as_str())
        .collect();
    assert_eq!(missing_img_keys, vec!["empty-img", "bad-path-img"]);

    let _ = std::fs::remove_file(&png_path);
}

#[test]
fn test_input_caret_position_and_selection_highlight_rendering() {
    let renderer = UiRenderer::new();
    let root = UiNode {
        key: "input-root".into(),
        kind: UiKind::Col,
        bg: "#0b1120".into(),
        style: UiStyle {
            pad_edges: [20.0; 4],
            ..UiStyle::default()
        },
        children: vec![UiNode {
            key: "search".into(),
            kind: UiKind::Input,
            text: "Hello World".into(),
            action: "search_input".into(),
            bg: "#0f172a".into(),
            color: "#f8fafc".into(),
            border: "#334155".into(),
            font_size: 16.0,
            style: UiStyle {
                w: 280.0,
                h: 42.0,
                radius_corners: [8.0; 4],
                ..UiStyle::default()
            },
            ..UiNode::default()
        }],
        ..UiNode::default()
    };

    let (pm_start, _) = renderer.render_ui_with_state(
        &root,
        340,
        100,
        &RenderInteractionState {
            focused_action: Some("search_input"),
            input_caret: Some(0),
            input_selection: None,
            ..Default::default()
        },
    );
    let (pm_end, _) = renderer.render_ui_with_state(
        &root,
        340,
        100,
        &RenderInteractionState {
            focused_action: Some("search_input"),
            input_caret: Some(11),
            input_selection: None,
            ..Default::default()
        },
    );
    let (pm_default_end, _) = renderer.render_ui_with_state(
        &root,
        340,
        100,
        &RenderInteractionState {
            focused_action: Some("search_input"),
            input_caret: None,
            input_selection: None,
            ..Default::default()
        },
    );
    let (pm_mid_plain, _) = renderer.render_ui_with_state(
        &root,
        340,
        100,
        &RenderInteractionState {
            focused_action: Some("search_input"),
            input_caret: Some(5),
            input_selection: None,
            ..Default::default()
        },
    );
    let (pm_mid_selected, _) = renderer.render_ui_with_state(
        &root,
        340,
        100,
        &RenderInteractionState {
            focused_action: Some("search_input"),
            input_caret: Some(5),
            input_selection: Some((0, 5)),
            ..Default::default()
        },
    );

    assert_ne!(
        pixmap_fnv_hash(&pm_start),
        pixmap_fnv_hash(&pm_end),
        "Caret at index 0 must render differently from caret at end of text"
    );
    assert_eq!(
        pixmap_fnv_hash(&pm_end),
        pixmap_fnv_hash(&pm_default_end),
        "input_caret: None must default cleanly to end of text"
    );
    assert_ne!(
        pixmap_fnv_hash(&pm_mid_plain),
        pixmap_fnv_hash(&pm_mid_selected),
        "Active selection range must visibly render translucent selection highlight"
    );

    // Verify click-to-caret hit testing advances monotonically with X
    let idx0 = renderer.hit_test_text_char_idx("Hello World", 0.0, 16.0, "sans", 400, 0.0);
    let half_w = renderer.measure_text_prefix_x("Hello World", 5, 16.0, "sans", 400, 0.0);
    let idx_mid = renderer.hit_test_text_char_idx("Hello World", half_w, 16.0, "sans", 400, 0.0);
    let idx_end = renderer.hit_test_text_char_idx("Hello World", 500.0, 16.0, "sans", 400, 0.0);
    assert_eq!(idx0, 0);
    assert!((4..=6).contains(&idx_mid), "Expected mid index near 5, got {idx_mid}");
    assert_eq!(idx_end, 11);
}

#[test]
fn test_ui_cmd_from_value_and_spawn_timer_storage_and_http() {
    // 1. UiCmd::from_value parses 5-field Cmd struct and returns None for empty / "none"
    let none_cmd = Value::Struct {
        schema: "Cmd".into(),
        fields: vec![
            Value::Str("none".into()),
            Value::Str(String::new()),
            Value::Str(String::new()),
            Value::Str(String::new()),
            Value::F64(0.0),
        ],
    };
    assert_eq!(UiCmd::from_value(&none_cmd), None);

    let empty_cmd = Value::Struct {
        schema: "Cmd".into(),
        fields: vec![
            Value::Str(String::new()),
            Value::Str(String::new()),
            Value::Str(String::new()),
            Value::Str(String::new()),
            Value::F64(0.0),
        ],
    };
    assert_eq!(UiCmd::from_value(&empty_cmd), None);

    let timer_val = Value::Struct {
        schema: "Cmd".into(),
        fields: vec![
            Value::Str("timer".into()),
            Value::Str(String::new()),
            Value::Str(String::new()),
            Value::Str("timer_fired".into()),
            Value::F64(20.0),
        ],
    };
    let parsed_timer = UiCmd::from_value(&timer_val).expect("parse timer Cmd");
    assert_eq!(parsed_timer.kind, "timer");
    assert_eq!(parsed_timer.on_result, "timer_fired");
    assert!((parsed_timer.delay_ms - 20.0).abs() < 0.01);

    // 2. Non-blocking "timer" execution
    let (tx, rx) = mpsc::channel();
    spawn_ui_cmd(parsed_timer, tx.clone());
    let ev = rx
        .recv_timeout(Duration::from_secs(2))
        .expect("receive timer click event");
    assert_eq!(ev, UiEvent::Click("timer_fired".into()));

    // 3. Non-blocking "storage_save" + "storage_load" roundtrip
    let unique_key = format!(
        "v03_key_{}_{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    );
    spawn_ui_cmd(
        UiCmd {
            kind: "storage_save".into(),
            target: unique_key.clone(),
            payload: "persisted_v03_value".into(),
            on_result: "save_done".into(),
            delay_ms: 0.0,
        },
        tx.clone(),
    );
    let save_ev = rx
        .recv_timeout(Duration::from_secs(2))
        .expect("receive storage_save confirmation");
    assert_eq!(save_ev, UiEvent::Click("save_done".into()));

    spawn_ui_cmd(
        UiCmd {
            kind: "storage_load".into(),
            target: unique_key,
            payload: String::new(),
            on_result: "load_done".into(),
            delay_ms: 0.0,
        },
        tx.clone(),
    );
    let load_ev = rx
        .recv_timeout(Duration::from_secs(2))
        .expect("receive storage_load result");
    assert_eq!(
        load_ev,
        UiEvent::TextInput {
            action: "load_done".into(),
            value: "persisted_v03_value".into(),
        }
    );

    // 4. Non-blocking "http_get" and "http_post" against ephemeral local TCP listener
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral HTTP test server");
    let port = listener.local_addr().unwrap().port();
    let server = std::thread::spawn(move || {
        for _ in 0..2 {
            if let Ok((mut stream, _)) = listener.accept() {
                let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
                let mut buf = [0u8; 2048];
                let n = stream.read(&mut buf).unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]);
                let resp_body = if req.starts_with("GET /ping") {
                    "pong_v03".to_string()
                } else if req.starts_with("POST /submit") {
                    let body = req.split("\r\n\r\n").nth(1).unwrap_or("");
                    format!("posted:{body}")
                } else {
                    "unknown".to_string()
                };
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{resp_body}",
                    resp_body.len()
                );
                let _ = stream.write_all(resp.as_bytes());
                let _ = stream.flush();
            }
        }
    });

    spawn_ui_cmd(
        UiCmd {
            kind: "http_get".into(),
            target: format!("http://127.0.0.1:{port}/ping"),
            payload: String::new(),
            on_result: "http_get_ok".into(),
            delay_ms: 0.0,
        },
        tx.clone(),
    );
    let get_ev = rx
        .recv_timeout(Duration::from_secs(3))
        .expect("receive http_get event");
    assert_eq!(
        get_ev,
        UiEvent::TextInput {
            action: "http_get_ok".into(),
            value: "pong_v03".into(),
        }
    );

    spawn_ui_cmd(
        UiCmd {
            kind: "http_post".into(),
            target: format!("http://127.0.0.1:{port}/submit"),
            payload: r#"{"ok":true}"#.into(),
            on_result: "http_post_ok".into(),
            delay_ms: 0.0,
        },
        tx,
    );
    let post_ev = rx
        .recv_timeout(Duration::from_secs(3))
        .expect("receive http_post event");
    assert_eq!(
        post_ev,
        UiEvent::TextInput {
            action: "http_post_ok".into(),
            value: r#"posted:{"ok":true}"#.into(),
        }
    );
    server.join().expect("ephemeral server thread finished");

    // 5. Headless launch_ui_app_with_reload_and_effects drains cmd_rx cleanly
    let (fx_tx, fx_rx) = mpsc::channel();
    fx_tx
        .send(UiEvent::TextInput {
            action: "async_banner".into(),
            value: "Async Ready".into(),
        })
        .unwrap();
    let seen = Arc::new(Mutex::new(Vec::<UiEvent>::new()));
    let seen_clone = Arc::clone(&seen);
    let res = launch_ui_app_with_reload_and_effects(
        WindowConfig::new("v0.3 Effects Test", 320, 200)
            .with_headless(true)
            .with_max_frames(2),
        HashMap::new(),
        false,
        || UiNode {
            kind: UiKind::Text,
            text: "Effects".into(),
            ..UiNode::default()
        },
        move |ev| {
            seen_clone.lock().unwrap().push(ev);
        },
        |_dt, _elapsed| {},
        || None,
        Some(fx_rx),
    )
    .expect("launch_ui_app_with_reload_and_effects headless");
    assert_eq!(res.frames_rendered, 2);
    assert_eq!(
        *seen.lock().unwrap(),
        vec![UiEvent::TextInput {
            action: "async_banner".into(),
            value: "Async Ready".into(),
        }]
    );
}
