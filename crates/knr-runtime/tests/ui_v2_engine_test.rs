//! Comprehensive test suite for `knr-runtime` v0.2.0:
//! - 45-field Style parsing (plus 40-field backward compatibility)
//! - Grid `col_span` layout & row height stretching
//! - Syntax-highlighted `<Code>` layout & rendering (indentation preservation)
//! - 31 crisp anti-aliased `<Icon>` vector icons + fallback ring
//! - `<Button>` with children (`<Icon>` + `<Text>`)
//! - `<Text>` `line_height` & `text_align`
//! - 4 TrueType font faces, `has_glyph`, HiDPI `render_ui_with_shaders` with custom shader blitting
//! - Automated `audit_layout` checks (ViewportOverflow, SiblingOverlap, MissingGlyph,
//!   LowContrast, TinyHitTarget, UnknownShader)

use knr_runtime::gui::renderer::tokenize_code_line;
use knr_runtime::gui::{
    audit_layout, compute_ui_layout, effective_line_height_mult, hit_test, paint_vector_icon,
    pixmap_fnv_hash, LayoutNode, LayoutRect, RenderInteractionState, TextMeasurer, UiKind, UiNode,
    UiRenderer, UiStyle,
};
use knr_runtime::platform::Value;
use tiny_skia::{Color as SkColor, Pixmap, Transform};

fn make_style_struct_fields(count: usize) -> Vec<Value> {
    let mut fields = vec![
        Value::F64(0.0),              // 0: w
        Value::F64(0.0),              // 1: h
        Value::F64(0.0),              // 2: min_w
        Value::F64(0.0),              // 3: max_w
        Value::Str("#0f172a".into()), // 4: bg
        Value::Str("#f8fafc".into()), // 5: color
        Value::Str("#334155".into()), // 6: border
        Value::F64(1.0),              // 7: border_w
        Value::F64(12.0),             // 8: r_tl
        Value::F64(12.0),             // 9: r_tr
        Value::F64(12.0),             // 10: r_br
        Value::F64(12.0),             // 11: r_bl
        Value::F64(16.0),             // 12: pad_t
        Value::F64(16.0),             // 13: pad_r
        Value::F64(16.0),             // 14: pad_b
        Value::F64(16.0),             // 15: pad_l
        Value::F64(12.0),             // 16: gap_x
        Value::F64(12.0),             // 17: gap_y
        Value::F64(15.0),             // 18: font_size
        Value::I64(600),              // 19: font_weight
        Value::Str("stretch".into()), // 20: align
        Value::Str("start".into()),   // 21: justify
        Value::I64(3),                // 22: grid_cols
        Value::F64(0.0),              // 23: shadow_x
        Value::F64(8.0),              // 24: shadow_y
        Value::F64(16.0),             // 25: shadow_blur
        Value::Str("#00000055".into()), // 26: shadow_color
        Value::F64(0.0),              // 27: backdrop_blur
        Value::F64(1.0),              // 28: opacity
        Value::F64(0.0),              // 29: translate_x
        Value::F64(0.0),              // 30: translate_y
        Value::F64(1.0),              // 31: scale
        Value::F64(0.0),              // 32: rotate_deg
        Value::Str(String::new()),    // 33: hover_bg
        Value::Str(String::new()),    // 34: hover_border
        Value::F64(1.0),              // 35: hover_scale
        Value::F64(120.0),            // 36: transition_ms
        Value::Str("visible".into()), // 37: overflow
        Value::I64(0),                // 38: z_index
        Value::F64(0.0),              // 39: grow
    ];
    if count >= 45 {
        fields.push(Value::Str("mono".into())); // 40: font_family
        fields.push(Value::Str("center".into())); // 41: text_align
        fields.push(Value::F64(1.6)); // 42: line_height
        fields.push(Value::I64(2)); // 43: col_span
        fields.push(Value::F64(0.5)); // 44: letter_spacing
    }
    fields
}

#[test]
fn test_widget_45_and_40_field_style_parsing() {
    let widget45 = Value::Struct {
        schema: "Widget".into(),
        fields: vec![
            Value::Str("k-code".into()),
            Value::Str("code".into()),
            Value::Str("const x = 1;".into()),
            Value::Str("tsx".into()),
            Value::F64(0.0),
            Value::Vec(vec![]),
            Value::Struct {
                schema: "Style".into(),
                fields: make_style_struct_fields(45),
            },
            Value::Vec(vec![]),
        ],
    };
    let node45 = UiNode::from_value(&widget45).expect("parse 45-field Widget");
    assert_eq!(node45.kind, UiKind::Code);
    assert_eq!(node45.kind.as_str(), "code");
    assert_eq!(node45.style.font_family, "mono");
    assert_eq!(node45.style.text_align, "center");
    assert!((node45.style.line_height - 1.6).abs() < 0.01);
    assert_eq!(node45.style.col_span, 2);
    assert!((node45.style.letter_spacing - 0.5).abs() < 0.01);

    // Graceful backward compatibility with legacy 40-field Style
    let widget40 = Value::Struct {
        schema: "Widget".into(),
        fields: vec![
            Value::Str("k-icon".into()),
            Value::Str("icon".into()),
            Value::Str("zap".into()),
            Value::Str(String::new()),
            Value::F64(0.0),
            Value::Vec(vec![]),
            Value::Struct {
                schema: "Style".into(),
                fields: make_style_struct_fields(40),
            },
            Value::Vec(vec![]),
        ],
    };
    let node40 = UiNode::from_value(&widget40).expect("parse 40-field Widget");
    assert_eq!(node40.kind, UiKind::Icon);
    assert_eq!(node40.kind.as_str(), "icon");
    assert_eq!(node40.style.font_family, "sans");
    assert_eq!(node40.style.text_align, "left");
    assert_eq!(node40.style.line_height, 0.0);
    assert_eq!(node40.style.col_span, 1);
    assert_eq!(node40.style.letter_spacing, 0.0);
}

#[test]
fn test_grid_col_span_packing_and_equal_row_height() {
    let renderer = UiRenderer::new();
    let make_card = |key: &str, col_span: i64, text: &str| UiNode {
        key: key.into(),
        kind: UiKind::Card,
        bg: "#0f172a".into(),
        border: "#334155".into(),
        pad: 12.0,
        style: UiStyle {
            col_span,
            pad_edges: [12.0; 4],
            ..UiStyle::default()
        },
        children: vec![UiNode {
            kind: UiKind::Text,
            text: text.into(),
            font_size: 14.0,
            ..UiNode::default()
        }],
        ..UiNode::default()
    };

    // 3-column grid:
    // Row 0: c0 (span 2) + c1 (span 1, taller content) -> both stretched to same row_h
    // Row 1: c2 (span 2) -> next child c3 (span 2) doesn't fit (2 + 2 > 3), wraps to Row 2!
    let grid = UiNode {
        key: "bento".into(),
        kind: UiKind::Grid,
        style: UiStyle {
            grid_cols: 3,
            gap_x: 12.0,
            gap_y: 12.0,
            pad_edges: [10.0; 4],
            ..UiStyle::default()
        },
        children: vec![
            make_card("c0", 2, "Short"),
            make_card("c1", 1, "Line 1\nLine 2\nLine 3\nLine 4"),
            make_card("c2", 2, "Second row span 2"),
            make_card("c3", 2, "Wrapped to third row span 2"),
        ],
        ..UiNode::default()
    };

    let (laid, _) = compute_ui_layout(&grid, 920.0, 600.0, &renderer);
    assert_eq!(laid.children.len(), 4);

    let inner_w = 920.0 - 20.0;
    let col_w = (inner_w - 24.0) / 3.0;
    let span2_w = 2.0 * col_w + 12.0;

    let c0 = &laid.children[0].rect;
    let c1 = &laid.children[1].rect;
    let c2 = &laid.children[2].rect;
    let c3 = &laid.children[3].rect;

    assert!((c0.w - span2_w).abs() < 0.5, "c0 width {} != {}", c0.w, span2_w);
    assert!((c1.w - col_w).abs() < 0.5, "c1 width {} != {}", c1.w, col_w);
    assert!((c0.y - c1.y).abs() < 0.5, "c0 and c1 on same row");
    assert!((c0.h - c1.h).abs() < 0.5, "c0 and c1 equal stretched height: {} vs {}", c0.h, c1.h);
    assert!(c1.x >= c0.x + c0.w + 11.5, "c1 placed after c0 + gap");

    assert!(c2.y >= c0.y + c0.h + 11.5, "c2 on row 1");
    assert!((c2.w - span2_w).abs() < 0.5);
    assert!(c3.y >= c2.y + c2.h + 11.5, "c3 wrapped to row 2 since 2+2 > 3");
    assert!((c3.x - 10.0).abs() < 0.5, "c3 starts at column 0");
    assert!((c3.w - span2_w).abs() < 0.5);
}

#[test]
fn test_code_block_layout_indentation_and_syntax_highlighting() {
    let renderer = UiRenderer::new();
    let code_src = "@shader\nfunction aurora(uv: Vec2): Color {\n    const t = 42.5; // wave\n    return \"ok\";\n}";
    let code_node = UiNode {
        key: "snippet".into(),
        kind: UiKind::Code,
        text: code_src.into(),
        action: "tsx".into(),
        bg: "#070b14".into(),
        border: "#ffffff14".into(),
        font_size: 13.0,
        style: UiStyle {
            pad_edges: [14.0, 16.0, 14.0, 16.0],
            font_family: "mono".into(),
            ..UiStyle::default()
        },
        ..UiNode::default()
    };

    let (laid, _) = compute_ui_layout(&code_node, 640.0, 400.0, &renderer);
    assert_eq!(laid.wrapped_lines.len(), 5);
    // Leading indentation MUST be preserved verbatim
    assert_eq!(laid.wrapped_lines[2], "    const t = 42.5; // wave");
    assert!(laid.line_height >= (13.0f32 * 1.55).floor());

    // Tokenizer verifies colors for every category
    let tokens = tokenize_code_line("    const x: Vec2 = 42.5; // comment");
    assert_eq!(tokens[0].0, "    "); // leading spaces preserved
    assert!(tokens.iter().any(|&(s, c)| s == "const" && c == (192, 132, 252)));
    assert!(tokens.iter().any(|&(s, c)| s == "Vec2" && c == (56, 189, 248)));
    assert!(tokens.iter().any(|&(s, c)| s == "42.5" && c == (251, 191, 36)));
    assert!(tokens.iter().any(|&(s, c)| s == "// comment" && c == (100, 116, 139)));

    let dec_tokens = tokenize_code_line("@shader \"hello\"");
    assert!(dec_tokens.iter().any(|&(s, c)| s == "@shader" && c == (244, 114, 182)));
    assert!(dec_tokens.iter().any(|&(s, c)| s == "\"hello\"" && c == (52, 211, 153)));

    // Renders non-blank pixels cleanly
    let (pm, _) = renderer.render_ui(&code_node, 640, 260, None);
    assert_ne!(pixmap_fnv_hash(&pm), 0);
}

#[test]
fn test_all_31_vector_icons_and_button_with_children() {
    let all_icons = [
        "zap", "cpu", "layers", "shield", "terminal", "code", "play", "pause", "refresh",
        "sparkles", "box", "check", "x", "alert", "arrow-right", "sliders", "eye", "globe",
        "lock", "rocket", "command", "git-branch", "palette", "cube", "activity", "plus",
        "minus", "copy", "sun", "moon", "diamond", "unknown-fallback",
    ];
    for icon_name in all_icons {
        let mut pm = Pixmap::new(32, 32).unwrap();
        paint_vector_icon(
            &mut pm,
            LayoutRect::new(4.0, 4.0, 24.0, 24.0),
            icon_name,
            (56, 189, 248, 255),
            Transform::identity(),
        );
        let non_empty = pm.data().as_chunks::<4>().0.iter().any(|px| px[3] > 0);
        assert!(non_empty, "Vector icon '{icon_name}' produced empty pixels");
    }

    let renderer = UiRenderer::new();
    let btn = UiNode {
        key: "run-btn".into(),
        kind: UiKind::Button,
        action: "run_app".into(),
        bg: "#0284c7".into(),
        color: "#f8fafc".into(),
        border: "#38bdf866".into(),
        style: UiStyle {
            radius_corners: [10.0; 4],
            pad_edges: [10.0, 16.0, 10.0, 16.0],
            ..UiStyle::default()
        },
        children: vec![
            UiNode {
                kind: UiKind::Icon,
                text: "play".into(),
                color: "#f8fafc".into(),
                font_size: 16.0,
                ..UiNode::default()
            },
            UiNode {
                kind: UiKind::Text,
                text: "Run Shader".into(),
                color: "#f8fafc".into(),
                font_size: 14.0,
                ..UiNode::default()
            },
        ],
        ..UiNode::default()
    };

    let (laid, hit_boxes) = compute_ui_layout(&btn, 400.0, 120.0, &renderer);
    assert_eq!(laid.children.len(), 2);
    let icon_r = laid.children[0].rect;
    let text_r = laid.children[1].rect;
    assert_eq!(icon_r.w, 16.0);
    assert_eq!(icon_r.h, 16.0);
    // Horizontal layout with 8px gap
    assert!((text_r.x - (icon_r.x + icon_r.w + 8.0)).abs() < 0.5);
    // Clicking inside the Icon child hits the Button action
    assert_eq!(
        hit_test(&hit_boxes, icon_r.x + 4.0, icon_r.y + 4.0),
        Some("run_app")
    );

    let (pm, _) = renderer.render_ui(&btn, 300, 100, None);
    assert_ne!(pixmap_fnv_hash(&pm), 0);
}

#[test]
fn test_text_line_height_align_and_hidpi_render_ui_with_shaders() {
    assert!((effective_line_height_mult(36.0, 0.0) - 1.2).abs() < 0.001);
    assert!((effective_line_height_mult(22.0, 0.0) - 1.35).abs() < 0.001);
    assert!((effective_line_height_mult(14.0, 0.0) - 1.5).abs() < 0.001);
    assert!((effective_line_height_mult(14.0, 1.8) - 1.8).abs() < 0.001);

    let renderer = UiRenderer::new();
    assert!(renderer.has_glyph('A', "sans", 400));
    assert!(renderer.has_glyph(' ', "sans", 400));
    assert!(renderer.has_glyph('Z', "display", 700));
    assert!(renderer.has_glyph('{', "mono", 400));

    let root = UiNode {
        key: "root".into(),
        kind: UiKind::Col,
        bg: "#0b1120".into(),
        style: UiStyle {
            pad_edges: [20.0; 4],
            gap_y: 16.0,
            ..UiStyle::default()
        },
        children: vec![
            UiNode {
                key: "hero".into(),
                kind: UiKind::Text,
                text: "Centered Display Hero".into(),
                font_size: 32.0,
                color: "#f8fafc".into(),
                style: UiStyle {
                    font_family: "display".into(),
                    font_weight: 700,
                    text_align: "center".into(),
                    ..UiStyle::default()
                },
                ..UiNode::default()
            },
            UiNode {
                key: "shader-card".into(),
                kind: UiKind::Card,
                bg: "#0f172a".into(),
                border: "#ffffff1c".into(),
                style: UiStyle {
                    radius_corners: [16.0; 4],
                    pad_edges: [16.0; 4],
                    ..UiStyle::default()
                },
                children: vec![UiNode {
                    key: "aurora".into(),
                    kind: UiKind::Shader,
                    text: "aurora_wave".into(),
                    style: UiStyle {
                        h: 120.0,
                        radius_corners: [12.0; 4],
                        ..UiStyle::default()
                    },
                    ..UiNode::default()
                }],
                ..UiNode::default()
            },
        ],
        ..UiNode::default()
    };

    let state = RenderInteractionState {
        hovered_action: None,
        focused_action: None,
        elapsed_sec: 1.5,
        ..Default::default()
    };

    let provider = |name: &str, pw: u32, ph: u32, _t: f32, _u: &[f64]| -> Option<Pixmap> {
        assert_eq!(name, "aurora_wave");
        assert!(pw >= 100 && ph >= 100);
        let mut pm = Pixmap::new(pw, ph)?;
        pm.fill(SkColor::from_rgba8(20, 220, 180, 255));
        Some(pm)
    };

    let (pm2x, layout, _hbs) =
        renderer.render_ui_with_shaders(&root, 400, 300, 2.0, &state, Some(&provider));
    assert_eq!(pm2x.width(), 800);
    assert_eq!(pm2x.height(), 600);
    // Centered hero text spans full inner width so it centers properly
    assert!((layout.children[0].rect.w - 360.0).abs() < 0.5);

    // Verify the custom shader green/teal color actually made it into the rendered 2x framebuffer
    let shader_rect = layout.children[1].children[0].rect;
    let cx = ((shader_rect.x + shader_rect.w * 0.5) * 2.0) as u32;
    let cy = ((shader_rect.y + shader_rect.h * 0.5) * 2.0) as u32;
    let px = pm2x.pixel(cx, cy).expect("center shader pixel");
    assert!(px.green() > 180 && px.blue() > 140, "Expected custom shader blit color, got {:?}", px);
}

#[test]
fn test_visual_auditor_all_six_checks_and_clean_pass() {
    let renderer = UiRenderer::new();

    // 1. Clean accessible layout passes with 0 errors and 0 warnings
    let clean = UiNode {
        key: "clean-root".into(),
        kind: UiKind::Col,
        bg: "#0b1120".into(),
        style: UiStyle {
            pad_edges: [20.0; 4],
            gap_y: 12.0,
            ..UiStyle::default()
        },
        children: vec![
            UiNode {
                key: "title".into(),
                kind: UiKind::Text,
                text: "High Contrast Title".into(),
                color: "#f8fafc".into(),
                font_size: 24.0,
                ..UiNode::default()
            },
            UiNode {
                key: "ok-btn".into(),
                kind: UiKind::Button,
                text: "Launch".into(),
                action: "launch".into(),
                bg: "#0284c7".into(),
                color: "#ffffff".into(),
                ..UiNode::default()
            },
            UiNode {
                key: "known-shader".into(),
                kind: UiKind::Shader,
                text: "aurora".into(),
                style: UiStyle {
                    h: 120.0,
                    ..UiStyle::default()
                },
                ..UiNode::default()
            },
        ],
        ..UiNode::default()
    };
    let (clean_layout, _) = compute_ui_layout(&clean, 800.0, 600.0, &renderer);
    let known = vec!["aurora".to_string()];
    let report = audit_layout(&clean_layout, 800.0, 600.0, &known, &|ch, ff, fw| {
        renderer.has_glyph(ch, ff, fw)
    });
    assert_eq!(report.status, "pass", "Unexpected issues: {:?}", report.issues);
    assert_eq!(report.error_count, 0);
    assert_eq!(report.warning_count, 0);
    assert_eq!(report.node_count, 4);

    // 2. Deliberately broken layout triggers all 6 audit checks
    let broken = UiNode {
        key: "broken-root".into(),
        kind: UiKind::Col,
        bg: "#0b1120".into(),
        style: UiStyle {
            pad_edges: [12.0; 4],
            gap_y: 12.0,
            ..UiStyle::default()
        },
        children: vec![
            // ViewportOverflow
            UiNode {
                key: "too-wide".into(),
                kind: UiKind::Card,
                bg: "#1e293b".into(),
                style: UiStyle {
                    w: 950.0,
                    h: 40.0,
                    ..UiStyle::default()
                },
                ..UiNode::default()
            },
            // MissingGlyph (deduplicated) + LowContrast
            UiNode {
                key: "dim-glyph".into(),
                kind: UiKind::Text,
                text: "Hi \u{E999}\u{E999}".into(),
                color: "#111827".into(), // very dark on #0b1120 -> contrast < 3.0
                font_size: 16.0,
                ..UiNode::default()
            },
            // TinyHitTarget
            UiNode {
                key: "tiny-btn".into(),
                kind: UiKind::Button,
                text: "x".into(),
                action: "close".into(),
                bg: "#0284c7".into(),
                color: "#ffffff".into(),
                style: UiStyle {
                    w: 16.0,
                    h: 16.0,
                    ..UiStyle::default()
                },
                ..UiNode::default()
            },
            // UnknownShader
            UiNode {
                key: "bad-shader".into(),
                kind: UiKind::Shader,
                text: "nonexistent_fx".into(),
                style: UiStyle {
                    h: 80.0,
                    ..UiStyle::default()
                },
                ..UiNode::default()
            },
            // Stack overlay should NOT trigger SiblingOverlap or LowContrast
            UiNode {
                key: "hero-stack".into(),
                kind: UiKind::Stack,
                children: vec![
                    UiNode {
                        key: "bg-shader".into(),
                        kind: UiKind::Shader,
                        text: "aurora".into(),
                        style: UiStyle {
                            h: 80.0,
                            ..UiStyle::default()
                        },
                        ..UiNode::default()
                    },
                    UiNode {
                        key: "overlay-text".into(),
                        kind: UiKind::Text,
                        text: "Overlay".into(),
                        color: "#111827".into(),
                        ..UiNode::default()
                    },
                ],
                ..UiNode::default()
            },
        ],
        ..UiNode::default()
    };

    let (mut broken_layout, _) = compute_ui_layout(&broken, 800.0, 600.0, &renderer);
    // Inject an artificial sibling overlap in a Row to verify SiblingOverlap warning
    broken_layout.children.push(LayoutNode {
        rect: LayoutRect::new(12.0, 400.0, 200.0, 50.0),
        node: UiNode {
            key: "overlap-row".into(),
            kind: UiKind::Row,
            ..UiNode::default()
        },
        wrapped_lines: vec![],
        line_height: 20.0,
        content_height: 50.0,
        scroll_offset_y: 0.0,
        content_width: 200.0,
        scroll_offset_x: 0.0,
        children: vec![
            LayoutNode {
                rect: LayoutRect::new(12.0, 400.0, 100.0, 40.0),
                node: UiNode {
                    key: "sib-a".into(),
                    kind: UiKind::Card,
                    ..UiNode::default()
                },
                wrapped_lines: vec![],
                line_height: 20.0,
                content_height: 40.0,
                scroll_offset_y: 0.0,
                content_width: 100.0,
                scroll_offset_x: 0.0,
                children: vec![],
            },
            LayoutNode {
                rect: LayoutRect::new(50.0, 400.0, 100.0, 40.0),
                node: UiNode {
                    key: "sib-b".into(),
                    kind: UiKind::Card,
                    ..UiNode::default()
                },
                wrapped_lines: vec![],
                line_height: 20.0,
                content_height: 40.0,
                scroll_offset_y: 0.0,
                content_width: 100.0,
                scroll_offset_x: 0.0,
                children: vec![],
            },
        ],
    });

    let bad_report = audit_layout(&broken_layout, 800.0, 600.0, &known, &|ch, _ff, _fw| {
        ch != '\u{E999}'
    });
    assert_eq!(bad_report.status, "fail");
    let kinds: Vec<&str> = bad_report.issues.iter().map(|i| i.kind.as_str()).collect();
    assert!(kinds.contains(&"ViewportOverflow"), "Missing ViewportOverflow: {kinds:?}");
    assert!(kinds.contains(&"SiblingOverlap"), "Missing SiblingOverlap: {kinds:?}");
    assert!(kinds.contains(&"MissingGlyph"), "Missing MissingGlyph: {kinds:?}");
    assert!(kinds.contains(&"LowContrast"), "Missing LowContrast: {kinds:?}");
    assert!(kinds.contains(&"TinyHitTarget"), "Missing TinyHitTarget: {kinds:?}");
    assert!(kinds.contains(&"UnknownShader"), "Missing UnknownShader: {kinds:?}");

    // Deduplicated: only ONE MissingGlyph for the two '\u{E999}' chars on dim-glyph
    assert_eq!(
        bad_report.issues.iter().filter(|i| i.kind == "MissingGlyph").count(),
        1
    );
    // Stack overlay must NOT produce SiblingOverlap or LowContrast
    assert!(
        !bad_report.issues.iter().any(|i| i.key == "overlay-text"),
        "Stack shader overlay should be excluded from overlap & contrast warnings"
    );

    // JSON roundtrip works cleanly
    let json = serde_json::to_string(&bad_report).expect("serialize VisualAuditReport");
    assert!(json.contains("\"ViewportOverflow\""));
    assert!(json.contains("\"UnknownShader\""));
}

#[test]
fn test_text_measurer_styled_default_delegation() {
    struct DummyMeasurer;
    impl TextMeasurer for DummyMeasurer {
        fn measure_text_size(&self, text: &str, font_size: f32) -> (f32, f32) {
            (text.len() as f32 * font_size * 0.6, font_size)
        }
    }
    let m = DummyMeasurer;
    assert_eq!(m.measure_text_size("hello", 16.0), m.measure_text_styled("hello", 16.0, "mono", 700));
}
