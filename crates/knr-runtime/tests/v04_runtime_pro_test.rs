//! Comprehensive v0.4.0 GPU Runtime, Layout, Accessibility & Visual Audit Pro v2 test suite:
//! 1. True `z_index` layer sorting across CPU rendering, pointer hit-testing (`hit_test`,
//!    `hit_test_box`), and scroll target hit-testing (`hit_test_scroll_targets`).
//! 2. Accessibility keyboard focus ring (`focused_widget_key`) rendering crisp `#38bdf8` outline
//!    pixels + `format_key_combo` & `UiEvent::KeyPress` / `UiEvent::Resize` event coverage.
//! 3. Vertical & horizontal (`overflow="scroll-x"`) scroll layout, clipping, scroll offsets,
//!    and rounded scrollbar thumb rendering (`#94a3b855` idle, `#38bdf888` active).
//! 4. All 67 analytical crisp vector icons in `supported_icon_names()` render non-empty visible
//!    pixels with zero panics and distinct geometry.
//! 5. Critically-damped spring physics (`spring_step`, `UiAnimator::animate_spring`,
//!    `UiAnimator::step_spring`) convergence & guaranteed zero overshoot.
//! 6. Visual Auditor Pro v2 checks (`DuplicateKey`, `TextTruncation`, `EmptyInteractiveAction`,
//!    `OffscreenInteractive`) and structured `AccessibilitySummary`.

use std::collections::HashMap;

use knr_runtime::gui::{
    audit_layout, compute_ui_layout, compute_ui_layout_with_scroll, format_key_combo, hit_test,
    hit_test_box, hit_test_scroll_target, hit_test_scroll_targets, paint_vector_icon,
    pixmap_fnv_hash, spring_step, supported_icon_names, LayoutRect, RenderInteractionState,
    UiAnimator, UiEvent, UiKind, UiNode, UiRenderer, UiStyle,
};
use tiny_skia::{Pixmap, Transform};

#[test]
fn test_z_index_render_and_hit_test_precedence() {
    let renderer = UiRenderer::new();

    // Child 0 has HIGHER z_index (100, bright cyan modal) than Child 1 (0, bright red background),
    // and Child 2 has equal z_index (100, green badge) later in document order -> stable sort puts
    // Child 1 (z=0) at bottom, Child 0 (z=100) above it, Child 2 (z=100, later doc order) on top!
    let tree = UiNode {
        key: "z-stack".into(),
        kind: UiKind::Stack,
        bg: "#0b1120".into(),
        style: UiStyle {
            w: 320.0,
            h: 220.0,
            ..UiStyle::default()
        },
        children: vec![
            // First in DOM, but z_index = 100 -> must paint OVER Child 1 and capture clicks!
            UiNode {
                key: "modal-layer".into(),
                kind: UiKind::Card,
                bg: "#38bdf8".into(),
                action: "modal_backdrop".into(),
                style: UiStyle {
                    w: 200.0,
                    h: 120.0,
                    z_index: 100,
                    ..UiStyle::default()
                },
                children: vec![UiNode {
                    key: "modal-ok".into(),
                    kind: UiKind::Button,
                    text: "Confirm".into(),
                    action: "confirm_modal".into(),
                    bg: "#0284c7".into(),
                    color: "#ffffff".into(),
                    style: UiStyle {
                        w: 100.0,
                        h: 40.0,
                        ..UiStyle::default()
                    },
                    ..UiNode::default()
                }],
                ..UiNode::default()
            },
            // Second in DOM, z_index = 0 -> must paint UNDER modal-layer and lose overlapping hits!
            UiNode {
                key: "under-btn".into(),
                kind: UiKind::Button,
                text: "Underneath".into(),
                action: "under_click".into(),
                bg: "#ef4444".into(),
                color: "#ffffff".into(),
                style: UiStyle {
                    w: 280.0,
                    h: 180.0,
                    z_index: 0,
                    ..UiStyle::default()
                },
                ..UiNode::default()
            },
            // Third in DOM, z_index = 100 (tie with modal-layer -> stable sort paints this on top!)
            UiNode {
                key: "tooltip-top".into(),
                kind: UiKind::Button,
                text: "Tip".into(),
                action: "tooltip_click".into(),
                bg: "#22c55e".into(),
                color: "#ffffff".into(),
                style: UiStyle {
                    w: 60.0,
                    h: 32.0,
                    z_index: 100,
                    ..UiStyle::default()
                },
                ..UiNode::default()
            },
        ],
        ..UiNode::default()
    };

    let (pm, _layout, hbs) = renderer.render_ui_with_shaders(
        &tree,
        320,
        220,
        1.0,
        &RenderInteractionState::default(),
        None,
    );

    // 1. Overlapping top-left (20, 15): all three overlap; tooltip-top (z=100, doc idx 2) wins!
    assert_eq!(hit_test(&hbs, 20.0, 15.0), Some("tooltip_click"));
    assert_eq!(
        hit_test_box(&hbs, 20.0, 15.0).map(|h| h.action.as_str()),
        Some("tooltip_click")
    );
    let px_top = pm.pixel(50, 16).expect("tooltip pixel");
    assert!(
        px_top.green() > 160 && px_top.red() < 100,
        "Expected green z=100 tie-winner painted on top, got {px_top:?}"
    );

    // 2. Inside modal-ok (80, 20): modal-layer (z=100, doc idx 0) beats under-btn (z=0, doc idx 1)!
    assert_eq!(hit_test(&hbs, 80.0, 20.0), Some("confirm_modal"));

    // 3. Inside modal-layer backdrop outside modal-ok (150, 90): modal_backdrop beats under_click!
    assert_eq!(hit_test(&hbs, 150.0, 90.0), Some("modal_backdrop"));
    let px_modal = pm.pixel(150, 90).expect("modal backdrop pixel");
    assert!(
        px_modal.blue() > 200 && px_modal.green() > 160 && px_modal.red() < 100,
        "Expected cyan z=100 modal painted over red z=0 under-btn, got {px_modal:?}"
    );

    // 4. Outside modal-layer (240, 150): under-btn (z=0) is visible and clickable!
    assert_eq!(hit_test(&hbs, 240.0, 150.0), Some("under_click"));
    let px_under = pm.pixel(240, 150).expect("under pixel");
    assert!(
        px_under.red() > 200 && px_under.green() < 100,
        "Expected red z=0 under-btn visible outside modal, got {px_under:?}"
    );

    // 5. Scroll target hit-testing also honors reverse z_index order!
    let scroll_stack = UiNode {
        key: "scroll-z-stack".into(),
        kind: UiKind::Stack,
        style: UiStyle {
            w: 300.0,
            h: 120.0,
            ..UiStyle::default()
        },
        children: vec![
            UiNode {
                key: "top-drawer-scroll".into(),
                kind: UiKind::Scroll,
                style: UiStyle {
                    w: 240.0,
                    h: 100.0,
                    z_index: 50,
                    ..UiStyle::default()
                },
                children: vec![UiNode {
                    kind: UiKind::Col,
                    style: UiStyle {
                        h: 300.0,
                        ..UiStyle::default()
                    },
                    ..UiNode::default()
                }],
                ..UiNode::default()
            },
            UiNode {
                key: "bg-page-scroll".into(),
                kind: UiKind::Scroll,
                style: UiStyle {
                    w: 280.0,
                    h: 100.0,
                    z_index: 0,
                    ..UiStyle::default()
                },
                children: vec![UiNode {
                    kind: UiKind::Col,
                    style: UiStyle {
                        h: 400.0,
                        ..UiStyle::default()
                    },
                    ..UiNode::default()
                }],
                ..UiNode::default()
            },
        ],
        ..UiNode::default()
    };
    let (scroll_layout, _) = compute_ui_layout(&scroll_stack, 300.0, 120.0, &renderer);
    let targets = hit_test_scroll_targets(&scroll_layout, 80.0, 50.0);
    assert_eq!(targets.len(), 2);
    assert_eq!(
        targets[0].id, "top-drawer-scroll",
        "Higher z_index scroll container must receive wheel events first"
    );
    assert_eq!(targets[1].id, "bg-page-scroll");
}

#[test]
fn test_accessibility_focus_ring_rendering_and_key_combo_formatting() {
    let renderer = UiRenderer::new();
    let tree = UiNode {
        key: "root".into(),
        kind: UiKind::Col,
        bg: "#0b1120".into(),
        style: UiStyle {
            pad_edges: [24.0; 4],
            gap_y: 16.0,
            ..UiStyle::default()
        },
        children: vec![
            UiNode {
                key: "primary-btn".into(),
                kind: UiKind::Button,
                text: "Save Changes".into(),
                action: "save_changes".into(),
                bg: "#1e293b".into(),
                color: "#f8fafc".into(),
                style: UiStyle {
                    w: 160.0,
                    h: 40.0,
                    radius_corners: [8.0; 4],
                    ..UiStyle::default()
                },
                ..UiNode::default()
            },
            UiNode {
                key: "volume-slider".into(),
                kind: UiKind::Slider,
                action: "vol".into(),
                value: 0.5,
                style: UiStyle {
                    w: 160.0,
                    ..UiStyle::default()
                },
                ..UiNode::default()
            },
        ],
        ..UiNode::default()
    };

    let (pm_unfocused, layout, _) = renderer.render_ui_with_shaders(
        &tree,
        240,
        160,
        1.0,
        &RenderInteractionState::default(),
        None,
    );
    let (pm_focused, _, _) = renderer.render_ui_with_shaders(
        &tree,
        240,
        160,
        1.0,
        &RenderInteractionState {
            focused_widget_key: Some("primary-btn".into()),
            ..Default::default()
        },
        None,
    );

    assert_ne!(
        pixmap_fnv_hash(&pm_unfocused),
        pixmap_fnv_hash(&pm_focused),
        "Setting focused_widget_key must visibly paint the #38bdf8 accessibility focus ring"
    );

    // Sample pixels immediately outside the button edge (ring spans -2.5px..-0.5px around rect)
    let btn_rect = layout.children[0].rect;
    let ring_x = (btn_rect.x + btn_rect.w * 0.5).round() as u32;
    let ring_y = (btn_rect.y - 2.0).round() as u32;
    let px_ring = pm_focused.pixel(ring_x, ring_y).expect("focus ring top pixel");
    let px_bg = pm_unfocused.pixel(ring_x, ring_y).expect("unfocused bg pixel");
    assert_ne!(px_ring, px_bg);
    assert!(
        px_ring.blue() > 200 && px_ring.green() > 140,
        "Expected crisp #38bdf8 cyan focus ring pixel at ({ring_x},{ring_y}), got {px_ring:?}"
    );

    // Verify Slider focus ring also renders distinct visible pixels
    let (pm_slider_focused, _, _) = renderer.render_ui_with_shaders(
        &tree,
        240,
        160,
        1.0,
        &RenderInteractionState {
            focused_widget_key: Some("volume-slider".into()),
            ..Default::default()
        },
        None,
    );
    assert_ne!(
        pixmap_fnv_hash(&pm_unfocused),
        pixmap_fnv_hash(&pm_slider_focused)
    );

    // Key combo formatting & UiEvent additive variants
    assert_eq!(format_key_combo("k", true, false, false), "Ctrl+K");
    assert_eq!(format_key_combo("p", true, true, false), "Ctrl+Shift+P");
    assert_eq!(format_key_combo("Escape", false, false, false), "Escape");
    assert_eq!(format_key_combo("Tab", false, true, false), "Shift+Tab");
    assert_eq!(format_key_combo("ArrowDown", false, false, true), "Alt+ArrowDown");

    let key_ev = UiEvent::KeyPress {
        key: "Ctrl+K".into(),
        ctrl: true,
        shift: false,
        alt: false,
    };
    let resize_ev = UiEvent::Resize {
        width: 1280,
        height: 720,
    };
    assert_ne!(key_ev, resize_ev);
}

#[test]
fn test_vertical_and_horizontal_scrollbars_and_scroll_x_offsets() {
    let renderer = UiRenderer::new();

    // 1. Vertical <Scroll> with overflowing content vs non-overflowing content
    let v_overflow = UiNode {
        key: "v-scroll".into(),
        kind: UiKind::Scroll,
        bg: "#0f172a".into(),
        style: UiStyle {
            w: 220.0,
            h: 120.0,
            overflow: "scroll".into(),
            ..UiStyle::default()
        },
        children: vec![UiNode {
            key: "tall-body".into(),
            kind: UiKind::Col,
            bg: "#0f172a".into(),
            style: UiStyle {
                h: 360.0,
                ..UiStyle::default()
            },
            ..UiNode::default()
        }],
        ..UiNode::default()
    };

    let (pm_idle, layout_v, _) = renderer.render_ui_with_shaders(
        &v_overflow,
        220,
        120,
        1.0,
        &RenderInteractionState::default(),
        None,
    );
    assert!(layout_v.content_height >= 359.0);
    // Vertical scrollbar thumb sits along right edge (x = w - 7..w - 2 -> x = 215, y = 16)
    let thumb_idle = pm_idle.pixel(215, 16).expect("vertical thumb idle pixel");
    let bg_inside = pm_idle.pixel(100, 16).expect("scroll bg pixel");
    assert_ne!(
        thumb_idle, bg_inside,
        "Vertical scrollbar thumb must be visibly rendered on right edge when content overflows"
    );

    // Active scrollbar renders `#38bdf888` cyan highlight distinct from idle `#94a3b855`
    let (pm_active, _, _) = renderer.render_ui_with_shaders(
        &v_overflow,
        220,
        120,
        1.0,
        &RenderInteractionState {
            active_scroll_id: Some("v-scroll"),
            ..Default::default()
        },
        None,
    );
    let thumb_active = pm_active.pixel(215, 16).expect("vertical thumb active pixel");
    assert!(
        thumb_active.blue() > thumb_idle.blue(),
        "Active scrollbar thumb (#38bdf888) should be more cyan/blue than idle (#94a3b855): active={thumb_active:?}, idle={thumb_idle:?}"
    );

    // 2. Horizontal `overflow="scroll-x"` strip with 3 wide cards + horizontal scrollbar along bottom!
    let h_strip = UiNode {
        key: "h-carousel".into(),
        kind: UiKind::Row,
        bg: "#0f172a".into(),
        style: UiStyle {
            w: 220.0,
            h: 100.0,
            gap_x: 10.0,
            overflow: "scroll-x".into(),
            ..UiStyle::default()
        },
        children: vec![
            UiNode {
                key: "card-1".into(),
                kind: UiKind::Button,
                text: "First".into(),
                action: "act_first".into(),
                bg: "#ef4444".into(),
                style: UiStyle {
                    w: 150.0,
                    h: 72.0,
                    ..UiStyle::default()
                },
                ..UiNode::default()
            },
            UiNode {
                key: "card-2".into(),
                kind: UiKind::Button,
                text: "Second".into(),
                action: "act_second".into(),
                bg: "#22c55e".into(),
                style: UiStyle {
                    w: 150.0,
                    h: 72.0,
                    ..UiStyle::default()
                },
                ..UiNode::default()
            },
            UiNode {
                key: "card-3".into(),
                kind: UiKind::Button,
                text: "Third".into(),
                action: "act_third".into(),
                bg: "#3b82f6".into(),
                style: UiStyle {
                    w: 150.0,
                    h: 72.0,
                    ..UiStyle::default()
                },
                ..UiNode::default()
            },
        ],
        ..UiNode::default()
    };

    let (pm_h0, laid_h0, hbs_h0) = renderer.render_ui_with_shaders(
        &h_strip,
        220,
        100,
        1.0,
        &RenderInteractionState::default(),
        None,
    );
    assert!(
        laid_h0.content_width >= 460.0,
        "Horizontal scroll-x Row must preserve full non-wrapping strip width, got {}",
        laid_h0.content_width
    );
    let target = hit_test_scroll_target(&laid_h0, 100.0, 50.0).expect("hit horizontal scroll target");
    assert_eq!(target.id, "h-carousel");
    assert!(target.horizontal);
    assert!(target.max_scroll_x >= 240.0);

    // Bottom edge horizontal scrollbar thumb is visible at (x=24, y=95) vs dark bg at (x=200, y=95)
    let h_thumb = pm_h0.pixel(24, 95).expect("horizontal thumb pixel");
    let h_empty = pm_h0.pixel(200, 95).expect("horizontal empty track pixel");
    assert_ne!(
        h_thumb, h_empty,
        "Horizontal scrollbar thumb must be visibly rendered along bottom edge"
    );

    // Initially card-3 is clipped off to the right; scrolling horizontally by 250px brings card-3 into view!
    assert_eq!(hit_test(&hbs_h0, 120.0, 36.0), Some("act_first"));
    let mut h_offsets = HashMap::new();
    h_offsets.insert("h-carousel".to_string(), 250.0f32);
    let (laid_h1, hbs_h1) =
        compute_ui_layout_with_scroll(&h_strip, 220.0, 100.0, &h_offsets, &renderer);
    assert!((laid_h1.scroll_offset_x - 250.0).abs() < 0.5);
    assert_eq!(
        hit_test(&hbs_h1, 150.0, 36.0),
        Some("act_third"),
        "Scrolling scroll-x container horizontally must bring offscreen right button into hit-test viewport"
    );
}

#[test]
fn test_all_40_plus_analytical_vector_icons_render_non_empty_without_panics() {
    let names = supported_icon_names();
    assert!(
        names.len() >= 60,
        "Expected 60+ built-in analytical icons (40+ required), got {}",
        names.len()
    );

    // Verify all newly requested production icons are listed
    let required = [
        "search", "trash", "edit", "copy", "download", "upload", "folder", "file", "home",
        "bell", "mail", "user", "users", "lock", "unlock", "sun", "moon", "wifi", "heart",
        "star", "bookmark", "filter", "refresh", "external", "chevron-left", "chevron-right",
        "chevron-up", "chevron-down", "arrow-left", "arrow-right", "menu", "more", "info",
        "alert", "calendar", "clock", "chart", "database", "git", "settings", "send", "inbox",
        "tag", "sparkles", "cube", "layers", "cpu", "globe", "eye", "shield", "diamond",
        "palette", "sliders", "plus", "check", "x", "play", "pause", "zap", "terminal", "code",
    ];
    for req in required {
        assert!(
            names.contains(&req),
            "Required icon '{req}' missing from supported_icon_names()"
        );
    }

    // Render unknown fallback hash so we verify every supported icon has real bespoke geometry
    let mut fallback_pm = Pixmap::new(32, 32).unwrap();
    paint_vector_icon(
        &mut fallback_pm,
        LayoutRect::new(4.0, 4.0, 24.0, 24.0),
        "__nonexistent_icon_sentinel__",
        (56, 189, 248, 255),
        Transform::identity(),
    );
    let fallback_hash = pixmap_fnv_hash(&fallback_pm);

    for &icon_name in names {
        let mut pm = Pixmap::new(32, 32).unwrap();
        paint_vector_icon(
            &mut pm,
            LayoutRect::new(4.0, 4.0, 24.0, 24.0),
            icon_name,
            (56, 189, 248, 255),
            Transform::identity(),
        );
        let non_empty_px = pm.data().as_chunks::<4>().0.iter().filter(|px| px[3] > 0).count();
        assert!(
            non_empty_px >= 8,
            "Vector icon '{icon_name}' rendered too few visible pixels ({non_empty_px})"
        );
        assert_ne!(
            pixmap_fnv_hash(&pm),
            fallback_hash,
            "Vector icon '{icon_name}' unexpectedly fell through to the generic fallback!"
        );
    }
}

#[test]
fn test_critically_damped_spring_physics_convergence_and_zero_overshoot() {
    let stiffness = 220.0f32;
    let critical_damping = 2.0 * stiffness.sqrt();
    let target = 100.0f32;
    let dt = 1.0 / 60.0;

    let mut val = 0.0f32;
    let mut vel = 0.0f32;
    let mut max_val = 0.0f32;

    for _ in 0..180 {
        let (next_val, next_vel) = spring_step(val, vel, target, stiffness, critical_damping, dt);
        assert!(
            next_val >= val - 1e-4,
            "Critically-damped spring must approach monotonically: prev={val}, next={next_val}"
        );
        assert!(
            next_val <= target + 1e-4,
            "Critically-damped spring must NOT overshoot target {target}: got {next_val}"
        );
        val = next_val;
        vel = next_vel;
        max_val = max_val.max(val);
    }

    assert!(
        (val - target).abs() < 0.01,
        "Spring must converge to target {target}, got {val}"
    );
    assert!(vel.abs() < 0.05, "Spring velocity must settle to ~0, got {vel}");
    assert!(max_val <= target + 1e-4);

    // UiAnimator keyed scalar spring + LayoutNode step_spring integration
    let mut anim = UiAnimator::new();
    anim.set_spring_value("drawer_x", 0.0);
    let mut cur = 0.0f32;
    for _ in 0..120 {
        cur = anim.animate_spring("drawer_x", 240.0, 260.0, 2.0 * 260.0_f32.sqrt(), dt);
        assert!(cur <= 240.001, "animate_spring must not overshoot: {cur}");
    }
    assert!((cur - 240.0).abs() < 0.1);

    let renderer = UiRenderer::new();
    let card = UiNode {
        key: "spring-card".into(),
        kind: UiKind::Card,
        style: UiStyle {
            w: 120.0,
            h: 60.0,
            translate_x: 0.0,
            ..UiStyle::default()
        },
        ..UiNode::default()
    };
    let (mut laid0, _) = compute_ui_layout(&card, 400.0, 200.0, &renderer);
    anim.step_spring(&mut laid0, None, 200.0, 2.0 * 200.0_f32.sqrt(), dt);
    assert!(!anim.has_active_transitions());

    // Change target translate_x -> 100.0 and verify smooth spring convergence
    let mut card_moved = card;
    card_moved.style.translate_x = 100.0;
    let (mut laid1, _) = compute_ui_layout(&card_moved, 400.0, 200.0, &renderer);
    anim.step_spring(&mut laid1, None, 200.0, 2.0 * 200.0_f32.sqrt(), dt);
    assert!(anim.has_active_transitions());
    assert!(
        laid1.node.style.translate_x > 0.0 && laid1.node.style.translate_x < 100.0,
        "First spring step should be strictly between 0 and 100, got {}",
        laid1.node.style.translate_x
    );
}

#[test]
fn test_visual_auditor_pro_v2_four_new_checks_and_accessibility_summary() {
    let renderer = UiRenderer::new();

    let tree = UiNode {
        key: "audit-v2-root".into(),
        kind: UiKind::Col,
        bg: "#0b1120".into(),
        style: UiStyle {
            pad_edges: [16.0; 4],
            gap_y: 12.0,
            ..UiStyle::default()
        },
        children: vec![
            // 1. DuplicateKey: two distinct cards sharing key "dup-card"
            UiNode {
                key: "dup-card".into(),
                kind: UiKind::Card,
                bg: "#1e293b".into(),
                style: UiStyle {
                    w: 200.0,
                    h: 40.0,
                    ..UiStyle::default()
                },
                ..UiNode::default()
            },
            UiNode {
                key: "dup-card".into(),
                kind: UiKind::Card,
                bg: "#1e293b".into(),
                style: UiStyle {
                    w: 200.0,
                    h: 40.0,
                    ..UiStyle::default()
                },
                ..UiNode::default()
            },
            // 2. TextTruncation: tall wrapped Text inside a 24px overflow="hidden" ancestor
            UiNode {
                key: "clip-box".into(),
                kind: UiKind::Col,
                bg: "#0f172a".into(),
                style: UiStyle {
                    w: 140.0,
                    h: 24.0,
                    overflow: "hidden".into(),
                    ..UiStyle::default()
                },
                children: vec![UiNode {
                    key: "truncated-para".into(),
                    kind: UiKind::Text,
                    text: "This is a long multi-line paragraph that definitely exceeds twenty-four pixels of fixed hidden height.".into(),
                    color: "#f8fafc".into(),
                    font_size: 16.0,
                    ..UiNode::default()
                }],
                ..UiNode::default()
            },
            // 3. EmptyInteractiveAction: Button with empty action string
            UiNode {
                key: "dead-btn".into(),
                kind: UiKind::Button,
                text: "No Action".into(),
                action: String::new(),
                bg: "#0284c7".into(),
                color: "#ffffff".into(),
                style: UiStyle {
                    w: 120.0,
                    h: 36.0,
                    ..UiStyle::default()
                },
                ..UiNode::default()
            },
            // 4. OffscreenInteractive: interactive Button pushed completely past viewport_w (500px)
            UiNode {
                key: "offscreen-btn".into(),
                kind: UiKind::Button,
                text: "Far Right".into(),
                action: "far_right".into(),
                bg: "#0284c7".into(),
                color: "#ffffff".into(),
                style: UiStyle {
                    w: 120.0,
                    h: 36.0,
                    translate_x: 620.0,
                    ..UiStyle::default()
                },
                ..UiNode::default()
            },
            // Valid accessible toggle for AccessibilitySummary verification
            UiNode {
                key: "valid-toggle".into(),
                kind: UiKind::Toggle,
                text: "Dark Mode".into(),
                action: "toggle_dark".into(),
                color: "#f8fafc".into(),
                ..UiNode::default()
            },
        ],
        ..UiNode::default()
    };

    let (laid, _) = compute_ui_layout(&tree, 500.0, 420.0, &renderer);
    let report = audit_layout(&laid, 500.0, 420.0, &[], &|ch, ff, fw| {
        renderer.has_glyph(ch, ff, fw)
    });

    let kinds: Vec<&str> = report.issues.iter().map(|i| i.kind.as_str()).collect();
    assert!(
        report
            .issues
            .iter()
            .any(|i| i.kind == "DuplicateKey" && i.severity == "warning" && i.key == "dup-card"),
        "Missing DuplicateKey warning: {:?}",
        report.issues
    );
    assert!(
        report
            .issues
            .iter()
            .any(|i| i.kind == "TextTruncation" && i.severity == "warning" && i.key == "truncated-para"),
        "Missing TextTruncation warning: {:?}",
        report.issues
    );
    assert!(
        report.issues.iter().any(|i| {
            i.kind == "EmptyInteractiveAction" && i.severity == "warning" && i.key == "dead-btn"
        }),
        "Missing EmptyInteractiveAction warning: {:?}",
        report.issues
    );
    assert!(
        report.issues.iter().any(|i| {
            i.kind == "OffscreenInteractive" && i.severity == "warning" && i.key == "offscreen-btn"
        }),
        "Missing OffscreenInteractive warning: {kinds:?} -> {:?}",
        report.issues
    );

    // Structured AccessibilitySummary checks
    assert_eq!(report.accessibility.focusable_count, 3);
    assert_eq!(
        report.accessibility.interactive_keys,
        vec![
            "dead-btn".to_string(),
            "offscreen-btn".to_string(),
            "valid-toggle".to_string()
        ]
    );
    assert_eq!(
        report.accessibility.duplicate_keys,
        vec!["dup-card".to_string()]
    );
    assert!(
        report.accessibility.min_contrast_ratio.is_some_and(|r| r > 3.0),
        "Expected valid min_contrast_ratio > 3.0, got {:?}",
        report.accessibility.min_contrast_ratio
    );

    // JSON serialization includes `accessibility` field cleanly
    let json = serde_json::to_string(&report).expect("serialize v2 VisualAuditReport");
    assert!(json.contains("\"accessibility\""));
    assert!(json.contains("\"focusable_count\":3"));
    assert!(json.contains("\"DuplicateKey\""));
    assert!(json.contains("\"TextTruncation\""));
    assert!(json.contains("\"EmptyInteractiveAction\""));
    assert!(json.contains("\"OffscreenInteractive\""));
}
