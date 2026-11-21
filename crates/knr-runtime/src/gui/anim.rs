//! Real-Time 60/120 FPS Implicit Transition & Spring Animation Engine keyed by `UiNode.key`.
//!
//! Smoothly interpolates `scale`, `opacity`, `rotate_deg`, `translate_x/y`, and hover scale
//! between frames whenever `transition_ms > 0.0`, while reporting `has_active_transitions()`
//! so idle apps consume 0% CPU.

use std::collections::HashMap;

use crate::gui::ui_tree::LayoutNode;

#[derive(Debug, Clone)]
struct KeyAnimState {
    scale: f32,
    opacity: f32,
    rotate_deg: f32,
    translate_x: f32,
    translate_y: f32,
}

#[derive(Debug, Default)]
pub struct UiAnimator {
    states: HashMap<String, KeyAnimState>,
    active: bool,
}

impl UiAnimator {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn has_active_transitions(&self) -> bool {
        self.active
    }

    /// Steps all keyed implicit transitions in `layout_root` by `dt` seconds (clamped to 0.05s)
    /// and writes the interpolated transforms directly onto each `LayoutNode`'s style.
    pub fn step_and_apply(
        &mut self,
        layout_root: &mut LayoutNode,
        hovered_action: Option<&str>,
        dt: f32,
    ) {
        let clamped_dt = dt.clamp(0.0001, 0.05);
        self.active = false;
        let mut active_flag = false;
        step_node(&mut self.states, layout_root, hovered_action, clamped_dt, &mut active_flag);
        self.active = active_flag;
    }
}

fn step_node(
    states: &mut HashMap<String, KeyAnimState>,
    laid: &mut LayoutNode,
    hovered_action: Option<&str>,
    dt: f32,
    any_active: &mut bool,
) {
    let is_hovered =
        !laid.node.action.is_empty() && hovered_action == Some(laid.node.action.as_str());
    let target_scale = laid.node.style.scale
        * if is_hovered && (laid.node.style.hover_scale - 1.0).abs() > 0.001 {
            laid.node.style.hover_scale
        } else {
            1.0
        };
    let target_opacity = laid.node.style.opacity;
    let target_rot = laid.node.style.rotate_deg;
    let target_tx = laid.node.style.translate_x;
    let target_ty = laid.node.style.translate_y;

    let key = if !laid.node.key.is_empty() {
        laid.node.key.clone()
    } else if !laid.node.action.is_empty() {
        format!("act:{}", laid.node.action)
    } else {
        String::new()
    };

    let dur_ms = laid.node.style.transition_ms;
    if !key.is_empty() && dur_ms > 0.0 {
        // Exponential critically-damped smoothing with time constant matched to transition_ms
        let tau = (dur_ms / 1000.0).max(0.016) * 0.35;
        let alpha = 1.0 - (-dt / tau).exp();

        let entry = states.entry(key).or_insert(KeyAnimState {
            scale: target_scale,
            opacity: target_opacity,
            rotate_deg: target_rot,
            translate_x: target_tx,
            translate_y: target_ty,
        });

        entry.scale += (target_scale - entry.scale) * alpha;
        entry.opacity += (target_opacity - entry.opacity) * alpha;
        entry.rotate_deg += (target_rot - entry.rotate_deg) * alpha;
        entry.translate_x += (target_tx - entry.translate_x) * alpha;
        entry.translate_y += (target_ty - entry.translate_y) * alpha;

        let delta = (target_scale - entry.scale).abs()
            + (target_opacity - entry.opacity).abs()
            + (target_rot - entry.rotate_deg).abs() * 0.02
            + (target_tx - entry.translate_x).abs() * 0.02
            + (target_ty - entry.translate_y).abs() * 0.02;

        if delta > 0.002 {
            *any_active = true;
        } else {
            entry.scale = target_scale;
            entry.opacity = target_opacity;
            entry.rotate_deg = target_rot;
            entry.translate_x = target_tx;
            entry.translate_y = target_ty;
        }

        laid.node.style.scale = entry.scale;
        laid.node.style.opacity = entry.opacity;
        laid.node.style.rotate_deg = entry.rotate_deg;
        laid.node.style.translate_x = entry.translate_x;
        laid.node.style.translate_y = entry.translate_y;
    } else if is_hovered && (laid.node.style.hover_scale - 1.0).abs() > 0.001 {
        laid.node.style.scale = target_scale;
    }

    for child in &mut laid.children {
        step_node(states, child, hovered_action, dt, any_active);
    }
}
