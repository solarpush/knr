//! Real-Time 60/120 FPS Implicit Transition & Critically-Damped Spring Physics Engine
//! keyed by `UiNode.key`.
//!
//! Smoothly interpolates `scale`, `opacity`, `rotate_deg`, `translate_x/y`, and hover scale
//! between frames whenever `transition_ms > 0.0`, while reporting `has_active_transitions()`
//! so idle apps consume 0% CPU.

use std::collections::HashMap;

use crate::gui::ui_tree::LayoutNode;

/// Analytically integrates a 1D damped harmonic oscillator (`x'' + damping*x' + stiffness*(x - target) = 0`)
/// forward by `dt` seconds.
///
/// Handles critically damped (`damping == 2 * sqrt(stiffness)`), underdamped, and overdamped
/// regimes in closed form — unconditionally stable for any `dt >= 0.0` with guaranteed zero
/// overshoot for critically damped parameters starting from rest.
pub fn spring_step(
    value: f32,
    velocity: f32,
    target: f32,
    stiffness: f32,
    damping: f32,
    dt: f32,
) -> (f32, f32) {
    if !value.is_finite() || !velocity.is_finite() || !target.is_finite() || dt <= 0.0 {
        return (value, velocity);
    }
    let k = stiffness.max(0.0);
    let c = damping.max(0.0);
    let x0 = value - target;
    let v0 = velocity;

    if k <= 1e-6 {
        // Pure viscous damping (no restoring force)
        if c <= 1e-6 {
            return (value + v0 * dt, v0);
        }
        let exp = (-c * dt).exp();
        let next_v = v0 * exp;
        let next_x = x0 + (v0 / c) * (1.0 - exp);
        return (target + next_x, next_v);
    }

    let omega0 = k.sqrt();
    let zeta = c / (2.0 * omega0);

    let (x_new, v_new) = if (zeta - 1.0).abs() < 1e-4 {
        // Critically damped: x(t) = (A + B*t) * exp(-omega0 * t)
        let e = (-omega0 * dt).exp();
        let a = x0;
        let b = v0 + omega0 * x0;
        let x = (a + b * dt) * e;
        let v = (b - omega0 * (a + b * dt)) * e;
        (x, v)
    } else if zeta < 1.0 {
        // Underdamped: x(t) = exp(-zeta*omega0*t) * (A*cos(wd*t) + B*sin(wd*t))
        let omega_d = omega0 * (1.0 - zeta * zeta).max(0.0).sqrt();
        let e = (-zeta * omega0 * dt).exp();
        let (sin_t, cos_t) = (omega_d * dt).sin_cos();
        let a = x0;
        let b = (v0 + zeta * omega0 * x0) / omega_d.max(1e-6);
        let x = e * (a * cos_t + b * sin_t);
        let v = e
            * ((b * omega_d - a * zeta * omega0) * cos_t
                - (a * omega_d + b * zeta * omega0) * sin_t);
        (x, v)
    } else {
        // Overdamped: x(t) = C1*exp(r1*t) + C2*exp(r2*t)
        let s = omega0 * (zeta * zeta - 1.0).max(0.0).sqrt();
        let r1 = -zeta * omega0 + s;
        let r2 = -zeta * omega0 - s;
        let denom = (r1 - r2).max(1e-6);
        let c1 = (v0 - r2 * x0) / denom;
        let c2 = x0 - c1;
        let e1 = (r1 * dt).exp();
        let e2 = (r2 * dt).exp();
        let x = c1 * e1 + c2 * e2;
        let v = c1 * r1 * e1 + c2 * r2 * e2;
        (x, v)
    };

    (target + x_new, v_new)
}

#[derive(Debug, Clone)]
struct KeyAnimState {
    scale: f32,
    vel_scale: f32,
    opacity: f32,
    vel_opacity: f32,
    rotate_deg: f32,
    vel_rotate_deg: f32,
    translate_x: f32,
    vel_translate_x: f32,
    translate_y: f32,
    vel_translate_y: f32,
}

#[derive(Debug, Default)]
pub struct UiAnimator {
    states: HashMap<String, KeyAnimState>,
    scalars: HashMap<String, (f32, f32)>,
    active: bool,
}

impl UiAnimator {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn has_active_transitions(&self) -> bool {
        self.active
    }

    /// Seeds or resets a keyed scalar spring animation value (with zero initial velocity).
    pub fn set_spring_value(&mut self, key: &str, value: f32) {
        self.scalars.insert(key.to_string(), (value, 0.0));
    }

    /// Steps a keyed 1D spring towards `target` using `(stiffness, damping)` over `dt` seconds
    /// and returns the new smoothed value.
    pub fn animate_spring(
        &mut self,
        key: &str,
        target: f32,
        stiffness: f32,
        damping: f32,
        dt: f32,
    ) -> f32 {
        let clamped_dt = dt.clamp(0.0001, 0.05);
        let entry = self
            .scalars
            .entry(key.to_string())
            .or_insert((target, 0.0));
        let (next_val, next_vel) = spring_step(entry.0, entry.1, target, stiffness, damping, clamped_dt);
        if (target - next_val).abs() > 0.001 || next_vel.abs() > 0.01 {
            entry.0 = next_val;
            entry.1 = next_vel;
            self.active = true;
        } else {
            entry.0 = target;
            entry.1 = 0.0;
        }
        entry.0
    }

    /// Steps all keyed implicit transitions in `layout_root` by `dt` seconds (clamped to 0.05s)
    /// using critically-damped spring smoothing calibrated to `transition_ms`, and writes the
    /// interpolated transforms directly onto each `LayoutNode`'s style.
    pub fn step_and_apply(
        &mut self,
        layout_root: &mut LayoutNode,
        hovered_action: Option<&str>,
        dt: f32,
    ) {
        let clamped_dt = dt.clamp(0.0001, 0.05);
        self.active = false;
        let mut active_flag = false;
        step_node(
            &mut self.states,
            layout_root,
            hovered_action,
            clamped_dt,
            None,
            &mut active_flag,
        );
        self.active = active_flag;
    }

    /// Steps all keyed widget transforms in `layout_root` using explicit spring `(stiffness, damping)`
    /// parameters over `dt` seconds (clamped to 0.05s).
    pub fn step_spring(
        &mut self,
        layout_root: &mut LayoutNode,
        hovered_action: Option<&str>,
        stiffness: f32,
        damping: f32,
        dt: f32,
    ) {
        let clamped_dt = dt.clamp(0.0001, 0.05);
        self.active = false;
        let mut active_flag = false;
        step_node(
            &mut self.states,
            layout_root,
            hovered_action,
            clamped_dt,
            Some((stiffness, damping)),
            &mut active_flag,
        );
        self.active = active_flag;
    }
}

fn step_node(
    states: &mut HashMap<String, KeyAnimState>,
    laid: &mut LayoutNode,
    hovered_action: Option<&str>,
    dt: f32,
    explicit_spring: Option<(f32, f32)>,
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
    let spring_params = explicit_spring.or_else(|| {
        if dur_ms > 0.0 {
            // Critically damped spring whose ~98% settling time matches `transition_ms`
            let omega = 5.0 / (dur_ms / 1000.0).max(0.016);
            Some((omega * omega, 2.0 * omega))
        } else {
            None
        }
    });

    if !key.is_empty() {
        if let Some((stiffness, damping)) = spring_params {
            let entry = states.entry(key).or_insert(KeyAnimState {
                scale: target_scale,
                vel_scale: 0.0,
                opacity: target_opacity,
                vel_opacity: 0.0,
                rotate_deg: target_rot,
                vel_rotate_deg: 0.0,
                translate_x: target_tx,
                vel_translate_x: 0.0,
                translate_y: target_ty,
                vel_translate_y: 0.0,
            });

            (entry.scale, entry.vel_scale) =
                spring_step(entry.scale, entry.vel_scale, target_scale, stiffness, damping, dt);
            (entry.opacity, entry.vel_opacity) = spring_step(
                entry.opacity,
                entry.vel_opacity,
                target_opacity,
                stiffness,
                damping,
                dt,
            );
            (entry.rotate_deg, entry.vel_rotate_deg) = spring_step(
                entry.rotate_deg,
                entry.vel_rotate_deg,
                target_rot,
                stiffness,
                damping,
                dt,
            );
            (entry.translate_x, entry.vel_translate_x) = spring_step(
                entry.translate_x,
                entry.vel_translate_x,
                target_tx,
                stiffness,
                damping,
                dt,
            );
            (entry.translate_y, entry.vel_translate_y) = spring_step(
                entry.translate_y,
                entry.vel_translate_y,
                target_ty,
                stiffness,
                damping,
                dt,
            );

            let delta = (target_scale - entry.scale).abs()
                + (target_opacity - entry.opacity).abs()
                + (target_rot - entry.rotate_deg).abs() * 0.02
                + (target_tx - entry.translate_x).abs() * 0.02
                + (target_ty - entry.translate_y).abs() * 0.02;
            let vel_mag = entry.vel_scale.abs()
                + entry.vel_opacity.abs()
                + entry.vel_rotate_deg.abs() * 0.02
                + entry.vel_translate_x.abs() * 0.02
                + entry.vel_translate_y.abs() * 0.02;

            if delta > 0.002 || vel_mag > 0.01 {
                *any_active = true;
            } else {
                entry.scale = target_scale;
                entry.vel_scale = 0.0;
                entry.opacity = target_opacity;
                entry.vel_opacity = 0.0;
                entry.rotate_deg = target_rot;
                entry.vel_rotate_deg = 0.0;
                entry.translate_x = target_tx;
                entry.vel_translate_x = 0.0;
                entry.translate_y = target_ty;
                entry.vel_translate_y = 0.0;
            }

            laid.node.style.scale = entry.scale;
            laid.node.style.opacity = entry.opacity.clamp(0.0, 1.0);
            laid.node.style.rotate_deg = entry.rotate_deg;
            laid.node.style.translate_x = entry.translate_x;
            laid.node.style.translate_y = entry.translate_y;
        } else if is_hovered && (laid.node.style.hover_scale - 1.0).abs() > 0.001 {
            laid.node.style.scale = target_scale;
        }
    } else if is_hovered && (laid.node.style.hover_scale - 1.0).abs() > 0.001 {
        laid.node.style.scale = target_scale;
    }

    for child in &mut laid.children {
        step_node(states, child, hovered_action, dt, explicit_spring, any_active);
    }
}
