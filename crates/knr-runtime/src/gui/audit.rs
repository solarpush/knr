//! Automated Visual, Layout & Accessibility Auditor Pro v2 for `knr` declarative UI trees.
//!
//! Inspects laid-out `LayoutNode` trees before or after rendering to catch:
//! - `ViewportOverflow` (error): unclipped widget overflowing viewport right edge by > 2px
//! - `SiblingOverlap` (warning): direct siblings in `Row`/`Col`/`Grid` overlapping by > 4px in X & Y
//! - `MissingGlyph` (error): non-whitespace characters missing from the selected font face
//! - `LowContrast` (warning): WCAG relative luminance contrast < 3.0 against ancestor background
//! - `TinyHitTarget` (warning): clickable `Button`/`Toggle` smaller than 24x24px
//! - `UnknownShader` (error): `UiKind::Shader` referencing a shader not in `known_shaders`
//! - `MissingImage` (error): `UiKind::Image` whose `src` is empty or fails to decode
//! - `DuplicateKey` (warning): two widgets sharing the exact same non-empty `key`
//! - `TextTruncation` (warning): `Text`/`Code` clipped inside a fixed-height `overflow="hidden"` ancestor by > 4px
//! - `EmptyInteractiveAction` (warning): `Button`/`Toggle`/`Slider`/`Input` with an empty `action`
//! - `OffscreenInteractive` (warning): interactive widget positioned completely outside `[0..viewport_w]` horizontally

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::gui::renderer::can_decode_image_src;
use crate::gui::ui_tree::{
    parse_fill_spec, parse_hex_rgba_f32, LayoutNode, LayoutRect, ParsedFill, UiKind,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditIssue {
    pub severity: String,
    pub kind: String,
    pub key: String,
    pub message: String,
    pub rect: [f32; 4],
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditNode {
    pub key: String,
    pub kind: String,
    pub rect: [f32; 4],
    pub text: String,
    pub action: String,
    pub children: Vec<AuditNode>,
}

/// Structured accessibility summary emitted alongside every `VisualAuditReport`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct AccessibilitySummary {
    pub focusable_count: usize,
    pub interactive_keys: Vec<String>,
    pub min_contrast_ratio: Option<f32>,
    pub duplicate_keys: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VisualAuditReport {
    pub status: String,
    pub width: u32,
    pub height: u32,
    pub content_height: f32,
    pub node_count: usize,
    pub error_count: usize,
    pub warning_count: usize,
    pub issues: Vec<AuditIssue>,
    pub tree: AuditNode,
    #[serde(default)]
    pub accessibility: AccessibilitySummary,
}

/// Runs automated layout, typography, accessibility, image, and shader checks over `layout`.
pub fn audit_layout(
    layout: &LayoutNode,
    viewport_w: f32,
    viewport_h: f32,
    known_shaders: &[String],
    glyph_checker: &dyn Fn(char, &str, i64) -> bool,
) -> VisualAuditReport {
    audit_layout_with_images(
        layout,
        viewport_w,
        viewport_h,
        known_shaders,
        glyph_checker,
        &can_decode_image_src,
    )
}

/// Runs automated layout, typography, accessibility, image, and shader checks over `layout`
/// using a custom `image_checker` (e.g. `&|src| renderer.can_load_image(src)`).
pub fn audit_layout_with_images(
    layout: &LayoutNode,
    viewport_w: f32,
    viewport_h: f32,
    known_shaders: &[String],
    glyph_checker: &dyn Fn(char, &str, i64) -> bool,
    image_checker: &dyn Fn(&str) -> bool,
) -> VisualAuditReport {
    let mut issues = Vec::new();
    let mut node_count = 0usize;
    let mut max_bottom = layout.rect.y + layout.rect.h;
    let mut seen_keys = HashSet::new();
    let mut accessibility = AccessibilitySummary::default();

    let default_bg = [11.0 / 255.0, 17.0 / 255.0, 32.0 / 255.0];
    let tree = audit_walk(
        layout,
        viewport_w,
        known_shaders,
        glyph_checker,
        image_checker,
        default_bg,
        1.0,
        false,
        None,
        None,
        &mut node_count,
        &mut max_bottom,
        &mut seen_keys,
        &mut accessibility,
        &mut issues,
    );

    let error_count = issues.iter().filter(|i| i.severity == "error").count();
    let warning_count = issues.iter().filter(|i| i.severity == "warning").count();
    let status = if error_count > 0 {
        "fail"
    } else if warning_count > 0 {
        "warn"
    } else {
        "pass"
    }
    .to_string();

    VisualAuditReport {
        status,
        width: viewport_w.round().max(0.0) as u32,
        height: viewport_h.round().max(0.0) as u32,
        content_height: max_bottom.max(layout.rect.h),
        node_count,
        error_count,
        warning_count,
        issues,
        tree,
        accessibility,
    }
}

#[allow(clippy::too_many_arguments)]
fn audit_walk(
    laid: &LayoutNode,
    viewport_w: f32,
    known_shaders: &[String],
    glyph_checker: &dyn Fn(char, &str, i64) -> bool,
    image_checker: &dyn Fn(&str) -> bool,
    inherited_bg: [f32; 3],
    parent_opacity: f32,
    in_shader_overlay: bool,
    clip_ancestor: Option<LayoutRect>,
    hidden_clip_ancestor: Option<LayoutRect>,
    node_count: &mut usize,
    max_bottom: &mut f32,
    seen_keys: &mut HashSet<String>,
    accessibility: &mut AccessibilitySummary,
    issues: &mut Vec<AuditIssue>,
) -> AuditNode {
    *node_count += 1;
    let node = &laid.node;
    let rect = laid.rect;
    let rect_arr = [rect.x, rect.y, rect.w, rect.h];
    let eff_opacity = (parent_opacity * node.style.opacity).clamp(0.0, 1.0);
    if let Some(clip) = clip_ancestor {
        let visible_bottom = (rect.y + rect.h).min(clip.y + clip.h);
        *max_bottom = max_bottom.max(visible_bottom);
    } else {
        *max_bottom = max_bottom.max(rect.y + rect.h);
    }

    let issue_key = if !node.key.is_empty() {
        node.key.clone()
    } else if !node.action.is_empty() {
        node.action.clone()
    } else {
        node.kind.as_str().to_string()
    };

    let is_interactive = matches!(
        node.kind,
        UiKind::Button | UiKind::Toggle | UiKind::Slider | UiKind::Input
    );
    if is_interactive {
        accessibility.focusable_count += 1;
        accessibility.interactive_keys.push(issue_key.clone());
    }

    // 8. DuplicateKey (warning): two widgets sharing the exact same explicit non-empty `key`
    //    (skip compiler-synthesized anonymous `jsx_<tag>_<line>_<col>` keys inside `.map()` loops)
    if !node.key.is_empty()
        && !node.key.starts_with("jsx_")
        && !seen_keys.insert(node.key.clone())
    {
        if !accessibility.duplicate_keys.contains(&node.key) {
            accessibility.duplicate_keys.push(node.key.clone());
        }
        issues.push(AuditIssue {
            severity: "warning".to_string(),
            kind: "DuplicateKey".to_string(),
            key: node.key.clone(),
            message: format!("Duplicate widget key '{}' found in UI tree", node.key),
            rect: rect_arr,
        });
    }

    // 1. ViewportOverflow (error): horizontal overflow past viewport right edge
    // Skip descendants inside clipped containers (`<Scroll>`, `overflow="hidden"|"scroll"|"scroll-x"`).
    if clip_ancestor.is_none() && rect.w > 0.0 && rect.x + rect.w > viewport_w + 2.0 {
        issues.push(AuditIssue {
            severity: "error".to_string(),
            kind: "ViewportOverflow".to_string(),
            key: issue_key.clone(),
            message: format!(
                "{} overflows viewport width ({:.1} > {:.1})",
                node.kind.as_str(),
                rect.x + rect.w,
                viewport_w
            ),
            rect: rect_arr,
        });
    }

    // 2. SiblingOverlap (warning): direct siblings in Row/Col/Grid overlapping > 4px in X & Y (skip Stack)
    if matches!(node.kind, UiKind::Row | UiKind::Col | UiKind::Grid) && laid.children.len() > 1 {
        for i in 0..laid.children.len() {
            let a = &laid.children[i];
            if a.rect.w <= 0.0 || a.rect.h <= 0.0 || a.node.style.opacity <= 0.004 {
                continue;
            }
            for b in laid.children.iter().skip(i + 1) {
                if b.rect.w <= 0.0 || b.rect.h <= 0.0 || b.node.style.opacity <= 0.004 {
                    continue;
                }
                let ox = (a.rect.x + a.rect.w).min(b.rect.x + b.rect.w) - a.rect.x.max(b.rect.x);
                let oy = (a.rect.y + a.rect.h).min(b.rect.y + b.rect.h) - a.rect.y.max(b.rect.y);
                if ox > 4.0 && oy > 4.0 {
                    let b_key = if !b.node.key.is_empty() {
                        b.node.key.clone()
                    } else if !b.node.action.is_empty() {
                        b.node.action.clone()
                    } else {
                        b.node.kind.as_str().to_string()
                    };
                    issues.push(AuditIssue {
                        severity: "warning".to_string(),
                        kind: "SiblingOverlap".to_string(),
                        key: b_key,
                        message: format!(
                            "Siblings in {} overlap by {:.1}x{:.1}px",
                            node.kind.as_str(),
                            ox,
                            oy
                        ),
                        rect: [b.rect.x, b.rect.y, b.rect.w, b.rect.h],
                    });
                }
            }
        }
    }

    // 3. MissingGlyph (error): deduplicated per node for Text/Code/Button/Badge/Toggle
    if matches!(
        node.kind,
        UiKind::Text | UiKind::Code | UiKind::Button | UiKind::Badge | UiKind::Toggle
    ) && !node.text.is_empty()
    {
        let font_family = if node.kind == UiKind::Code {
            "mono"
        } else {
            node.style.font_family.as_str()
        };
        let mut missing: Vec<char> = Vec::new();
        for ch in node.text.chars() {
            if ch.is_whitespace() || ch.is_ascii_control() {
                continue;
            }
            if !glyph_checker(ch, font_family, node.style.font_weight) && !missing.contains(&ch) {
                missing.push(ch);
            }
        }
        if !missing.is_empty() {
            let formatted: Vec<String> = missing
                .iter()
                .map(|c| format!("U+{:04X} '{}'", *c as u32, c))
                .collect();
            issues.push(AuditIssue {
                severity: "error".to_string(),
                kind: "MissingGlyph".to_string(),
                key: issue_key.clone(),
                message: format!(
                    "Missing glyph(s) in '{}' (weight {}): {}",
                    font_family,
                    node.style.font_weight,
                    formatted.join(", ")
                ),
                rect: rect_arr,
            });
        }
    }

    // Compute effective background color for this node and its descendants
    let own_bg_rgba = fill_representative_rgba(&node.bg);
    let effective_bg = if let Some(rgba) = own_bg_rgba {
        let a = (rgba[3] * eff_opacity).clamp(0.0, 1.0);
        composite_rgb(rgba, a, inherited_bg)
    } else if node.kind == UiKind::Input {
        composite_rgb(
            [15.0 / 255.0, 23.0 / 255.0, 42.0 / 255.0, 0.92],
            0.92 * eff_opacity,
            inherited_bg,
        )
    } else {
        inherited_bg
    };

    // 4. LowContrast (warning): WCAG relative luminance contrast < 3.0
    if !in_shader_overlay
        && eff_opacity >= 0.25
        && matches!(
            node.kind,
            UiKind::Text | UiKind::Code | UiKind::Button | UiKind::Badge | UiKind::Toggle | UiKind::Input
        )
        && !node.text.trim().is_empty()
    {
        if let Some(fg_rgba) = parse_hex_rgba_f32(&node.color) {
            let fg_alpha = (fg_rgba[3] * eff_opacity).clamp(0.0, 1.0);
            if fg_alpha >= 0.25 {
                let fg_rgb = composite_rgb(fg_rgba, fg_alpha, effective_bg);
                let ratio = wcag_contrast(fg_rgb, effective_bg);
                accessibility.min_contrast_ratio = Some(match accessibility.min_contrast_ratio {
                    Some(prev) => prev.min(ratio),
                    None => ratio,
                });
                if ratio < 3.0 {
                    issues.push(AuditIssue {
                        severity: "warning".to_string(),
                        kind: "LowContrast".to_string(),
                        key: issue_key.clone(),
                        message: format!(
                            "Low contrast ratio {:.2}:1 (< 3.0) for text color {} against background",
                            ratio, node.color
                        ),
                        rect: rect_arr,
                    });
                }
            }
        }
    }

    // 5. TinyHitTarget (warning): clickable Button/Toggle with w < 24 || h < 24
    if matches!(node.kind, UiKind::Button | UiKind::Toggle) && (rect.w < 24.0 || rect.h < 24.0) {
        issues.push(AuditIssue {
            severity: "warning".to_string(),
            kind: "TinyHitTarget".to_string(),
            key: issue_key.clone(),
            message: format!(
                "{} hit target too small ({:.1}x{:.1}px < 24x24px)",
                node.kind.as_str(),
                rect.w,
                rect.h
            ),
            rect: rect_arr,
        });
    }

    // 6. UnknownShader (error): Shader node whose name is not in known_shaders (when non-empty)
    if node.kind == UiKind::Shader
        && !known_shaders.is_empty()
        && !known_shaders.iter().any(|s| s == &node.text)
    {
        issues.push(AuditIssue {
            severity: "error".to_string(),
            kind: "UnknownShader".to_string(),
            key: issue_key.clone(),
            message: format!(
                "Unknown shader '{}' (available: {})",
                node.text,
                known_shaders.join(", ")
            ),
            rect: rect_arr,
        });
    }

    // 7. MissingImage (error): Image node with empty or undecodable src
    if node.kind == UiKind::Image && (node.text.trim().is_empty() || !image_checker(&node.text)) {
        let src_preview = if node.text.len() > 64 {
            format!("{}...", &node.text[..64])
        } else {
            node.text.clone()
        };
        issues.push(AuditIssue {
            severity: "error".to_string(),
            kind: "MissingImage".to_string(),
            key: issue_key.clone(),
            message: if node.text.trim().is_empty() {
                "Image widget has empty src".to_string()
            } else {
                format!("Image failed to decode from src '{src_preview}'")
            },
            rect: rect_arr,
        });
    }

    // 9. TextTruncation (warning): Text/Code clipped inside a fixed-height `overflow="hidden"` ancestor by > 4px
    if matches!(node.kind, UiKind::Text | UiKind::Code) && !node.text.trim().is_empty() {
        if let Some(hclip) = hidden_clip_ancestor {
            let [pt, _, pb, _] = node.effective_pad();
            let wrapped_h = if laid.wrapped_lines.is_empty() {
                0.0
            } else {
                (laid.wrapped_lines.len() as f32) * laid.line_height
            };
            let text_bottom = (rect.y + rect.h).max(rect.y + pt + pb + wrapped_h);
            let clip_bottom = hclip.y + hclip.h;
            if text_bottom > clip_bottom + 4.0 {
                issues.push(AuditIssue {
                    severity: "warning".to_string(),
                    kind: "TextTruncation".to_string(),
                    key: issue_key.clone(),
                    message: format!(
                        "{} bottom ({:.1}px) is truncated by overflow=\"hidden\" ancestor clip ({:.1}px) by {:.1}px",
                        node.kind.as_str(),
                        text_bottom,
                        clip_bottom,
                        text_bottom - clip_bottom
                    ),
                    rect: rect_arr,
                });
            }
        }
    }

    // 10. EmptyInteractiveAction (warning): Button/Toggle/Slider/Input with empty `action`
    if is_interactive && node.action.trim().is_empty() {
        issues.push(AuditIssue {
            severity: "warning".to_string(),
            kind: "EmptyInteractiveAction".to_string(),
            key: issue_key.clone(),
            message: format!(
                "Interactive {} widget has an empty action string",
                node.kind.as_str()
            ),
            rect: rect_arr,
        });
    }

    // 11. OffscreenInteractive (warning): interactive widget completely outside [0..viewport_w] horizontally
    if is_interactive && rect.w > 0.0 {
        let eff_left = rect.x + node.style.translate_x;
        let eff_right = eff_left + rect.w;
        if eff_right <= 0.0 || eff_left >= viewport_w {
            issues.push(AuditIssue {
                severity: "warning".to_string(),
                kind: "OffscreenInteractive".to_string(),
                key: issue_key.clone(),
                message: format!(
                    "Interactive {} is completely outside viewport [0..{:.1}] horizontally (x={:.1}..{:.1})",
                    node.kind.as_str(),
                    viewport_w,
                    eff_left,
                    eff_right
                ),
                rect: rect_arr,
            });
        }
    }

    let stack_has_shader = node.kind == UiKind::Stack
        && laid
            .children
            .iter()
            .any(|c| matches!(c.node.kind, UiKind::Shader | UiKind::Image));

    let next_clip = if node.clips_children() {
        Some(match clip_ancestor {
            Some(parent_clip) => parent_clip
                .intersect(rect)
                .unwrap_or_else(|| LayoutRect::new(rect.x, rect.y, 0.0, 0.0)),
            None => rect,
        })
    } else {
        clip_ancestor
    };

    let next_hidden_clip = if node.style.overflow.eq_ignore_ascii_case("hidden") {
        Some(match hidden_clip_ancestor {
            Some(parent_clip) => parent_clip
                .intersect(rect)
                .unwrap_or_else(|| LayoutRect::new(rect.x, rect.y, 0.0, 0.0)),
            None => rect,
        })
    } else {
        hidden_clip_ancestor
    };

    let mut audit_children = Vec::with_capacity(laid.children.len());
    for child in &laid.children {
        let child_is_overlay = in_shader_overlay
            || (stack_has_shader && !matches!(child.node.kind, UiKind::Shader | UiKind::Image));
        audit_children.push(audit_walk(
            child,
            viewport_w,
            known_shaders,
            glyph_checker,
            image_checker,
            effective_bg,
            eff_opacity,
            child_is_overlay,
            next_clip,
            next_hidden_clip,
            node_count,
            max_bottom,
            seen_keys,
            accessibility,
            issues,
        ));
    }

    AuditNode {
        key: node.key.clone(),
        kind: node.kind.as_str().to_string(),
        rect: rect_arr,
        text: node.text.clone(),
        action: node.action.clone(),
        children: audit_children,
    }
}

fn fill_representative_rgba(spec: &str) -> Option<[f32; 4]> {
    match parse_fill_spec(spec) {
        ParsedFill::None => None,
        ParsedFill::Solid(c) => Some(c),
        ParsedFill::LinearGradient { start, end, .. } => Some([
            (start[0] + end[0]) * 0.5,
            (start[1] + end[1]) * 0.5,
            (start[2] + end[2]) * 0.5,
            (start[3] + end[3]) * 0.5,
        ]),
        ParsedFill::RadialGradient { center, edge } => Some([
            (center[0] + edge[0]) * 0.5,
            (center[1] + edge[1]) * 0.5,
            (center[2] + edge[2]) * 0.5,
            (center[3] + edge[3]) * 0.5,
        ]),
    }
}

fn composite_rgb(fg: [f32; 4], alpha: f32, bg: [f32; 3]) -> [f32; 3] {
    let inv = 1.0 - alpha;
    [
        (fg[0] * alpha + bg[0] * inv).clamp(0.0, 1.0),
        (fg[1] * alpha + bg[1] * inv).clamp(0.0, 1.0),
        (fg[2] * alpha + bg[2] * inv).clamp(0.0, 1.0),
    ]
}

fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.03928 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn relative_luminance(rgb: [f32; 3]) -> f32 {
    0.2126 * srgb_to_linear(rgb[0])
        + 0.7152 * srgb_to_linear(rgb[1])
        + 0.0722 * srgb_to_linear(rgb[2])
}

fn wcag_contrast(a: [f32; 3], b: [f32; 3]) -> f32 {
    let l1 = relative_luminance(a);
    let l2 = relative_luminance(b);
    let (hi, lo) = if l1 >= l2 { (l1, l2) } else { (l2, l1) };
    (hi + 0.05) / (lo + 0.05)
}
