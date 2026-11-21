//! Built-in standard library modules embedded directly in `knr-core`.
//!
//! Enables zero-config imports such as:
//! - `import "std/ui";`
//! - `import "std/anim";`
//! - `import "std/math";`
//! - `import "std/sys";`
//!
//! These resolve identically for both on-disk `.knr` files and in-memory
//! `parse_knr` + `lower_program` / `verify_program` pipelines.

/// Pure animation, easing, and damping helpers (`import "std/anim";`).
pub const STD_ANIM_SRC: &str = r###"// Standard Pure Animation, Easing & Damping Helpers (`std/anim`).

fn ease_out_cubic(t: f64) -> f64 {
    let c = clamp(t, 0.0, 1.0);
    let inv = 1.0 - c;
    return 1.0 - inv * inv * inv;
}

fn ease_in_out_sine(t: f64) -> f64 {
    let c = clamp(t, 0.0, 1.0);
    return 0.5 - 0.5 * cos(c * 3.141592653589793);
}

fn ease_out_expo(t: f64) -> f64 {
    let c = clamp(t, 0.0, 1.0);
    if c <= 0.0 {
        return 0.0;
    }
    if c >= 1.0 {
        return 1.0;
    }
    return 1.0 - pow(2.0, -10.0 * c);
}

fn smooth_damp(current: f64, target: f64, dt: f64, rate: f64) -> f64 {
    let factor = clamp(dt * rate, 0.0, 1.0);
    return lerp(current, target, factor);
}
"###;

/// Pure GPU-compatible 2D/3D vector and SDF math helpers (`import "std/math";`).
pub const STD_MATH_SRC: &str = r###"// Standard Pure GPU-Compatible 2D/3D Vector & SDF Math Library (`std/math`).

schema Vec2 {
    0: x f64,
    1: y f64,
}

schema Vec3 {
    0: x f64,
    1: y f64,
    2: z f64,
}

fn pi() -> f64 {
    return 3.141592653589793;
}

fn tau() -> f64 {
    return 6.283185307179586;
}

fn vec2_len(v: &Vec2) -> f64 {
    return sqrt(v.x * v.x + v.y * v.y);
}

fn vec2_dot(a: &Vec2, b: &Vec2) -> f64 {
    return a.x * b.x + a.y * b.y;
}

fn vec3_add(a: &Vec3, b: &Vec3) -> Vec3 {
    return Vec3 {
        x: a.x + b.x,
        y: a.y + b.y,
        z: a.z + b.z,
    };
}

fn vec3_sub(a: &Vec3, b: &Vec3) -> Vec3 {
    return Vec3 {
        x: a.x - b.x,
        y: a.y - b.y,
        z: a.z - b.z,
    };
}

fn vec3_scale(v: &Vec3, s: f64) -> Vec3 {
    return Vec3 {
        x: v.x * s,
        y: v.y * s,
        z: v.z * s,
    };
}

fn vec3_dot(a: &Vec3, b: &Vec3) -> f64 {
    return a.x * b.x + a.y * b.y + a.z * b.z;
}

fn vec3_len(v: &Vec3) -> f64 {
    return sqrt(v.x * v.x + v.y * v.y + v.z * v.z);
}

fn vec3_norm(v: &Vec3) -> Vec3 {
    let l = max(vec3_len(v), 0.000001);
    let inv = 1.0 / l;
    return Vec3 {
        x: v.x * inv,
        y: v.y * inv,
        z: v.z * inv,
    };
}

fn smin(a: f64, b: f64, k: f64) -> f64 {
    let kk = max(k, 0.000001);
    let h = max(kk - abs(a - b), 0.0) / kk;
    return min(a, b) - h * h * kk * 0.25;
}
"###;

/// Standard unconstrained typed UI, styling, widget constructors, and animation re-exports (`import "std/ui";`).
pub const STD_UI_SRC: &str = r###"// Standard Unconstrained Typed UI, Styling, Animation & Shader Primitives for knr (`std/ui`).
// Zero raw HTML, zero raw CSS, zero raw GLSL — 100% statically verified schemas.
// Works identically on Native GPU Desktop (`wgpu 23`) and Web (`WASM + WebGPU/WebGL2`).

import "std/anim";

schema Vec2 {
    0: x f64,
    1: y f64,
}

schema Color {
    0: r f64,
    1: g f64,
    2: b f64,
    3: a f64,
}

schema Style {
    0: w f64,
    1: h f64,
    2: min_w f64,
    3: max_w f64,
    4: bg str,
    5: color str,
    6: border str,
    7: border_w f64,
    8: r_tl f64,
    9: r_tr f64,
    10: r_br f64,
    11: r_bl f64,
    12: pad_t f64,
    13: pad_r f64,
    14: pad_b f64,
    15: pad_l f64,
    16: gap_x f64,
    17: gap_y f64,
    18: font_size f64,
    19: font_weight i64,
    20: align str,
    21: justify str,
    22: grid_cols i64,
    23: shadow_x f64,
    24: shadow_y f64,
    25: shadow_blur f64,
    26: shadow_color str,
    27: backdrop_blur f64,
    28: opacity f64,
    29: translate_x f64,
    30: translate_y f64,
    31: scale f64,
    32: rotate_deg f64,
    33: hover_bg str,
    34: hover_border str,
    35: hover_scale f64,
    36: transition_ms f64,
    37: overflow str,
    38: z_index i64,
    39: grow f64,
}

schema Widget {
    0: key str,
    1: kind str,
    2: text str,
    3: action str,
    4: value f64,
    5: uniforms vec[f64],
    6: style Style,
    7: children vec[Widget],
}

// ============================================================================
// Composable Style Presets (use with `..style_default()` / `..style_card()`)
// ============================================================================

fn style_default() -> Style {
    return Style {
        w: 0.0,
        h: 0.0,
        min_w: 0.0,
        max_w: 0.0,
        bg: "",
        color: #f8fafc,
        border: "",
        border_w: 1.0,
        r_tl: 0.0,
        r_tr: 0.0,
        r_br: 0.0,
        r_bl: 0.0,
        pad_t: 0.0,
        pad_r: 0.0,
        pad_b: 0.0,
        pad_l: 0.0,
        gap_x: 0.0,
        gap_y: 0.0,
        font_size: 16.0,
        font_weight: 400,
        align: "stretch",
        justify: "start",
        grid_cols: 0,
        shadow_x: 0.0,
        shadow_y: 0.0,
        shadow_blur: 0.0,
        shadow_color: "",
        backdrop_blur: 0.0,
        opacity: 1.0,
        translate_x: 0.0,
        translate_y: 0.0,
        scale: 1.0,
        rotate_deg: 0.0,
        hover_bg: "",
        hover_border: "",
        hover_scale: 1.0,
        transition_ms: 0.0,
        overflow: "visible",
        z_index: 0,
        grow: 0.0,
    };
}

fn style_radius(s: Style, r: f64) -> Style {
    return Style {
        r_tl: r,
        r_tr: r,
        r_br: r,
        r_bl: r,
        ..s
    };
}

fn style_pad(s: Style, p: f64) -> Style {
    return Style {
        pad_t: p,
        pad_r: p,
        pad_b: p,
        pad_l: p,
        ..s
    };
}

fn style_gap(s: Style, g: f64) -> Style {
    return Style {
        gap_x: g,
        gap_y: g,
        ..s
    };
}

fn style_card() -> Style {
    return Style {
        bg: #111827cc,
        color: #f8fafc,
        border: #334155aa,
        border_w: 1.0,
        r_tl: 18.0,
        r_tr: 18.0,
        r_br: 18.0,
        r_bl: 18.0,
        pad_t: 22.0,
        pad_r: 22.0,
        pad_b: 22.0,
        pad_l: 22.0,
        gap_x: 14.0,
        gap_y: 14.0,
        shadow_x: 0.0,
        shadow_y: 12.0,
        shadow_blur: 28.0,
        shadow_color: #00000066,
        backdrop_blur: 16.0,
        transition_ms: 160.0,
        ..style_default()
    };
}

fn style_button(bg: str, color: str) -> Style {
    return Style {
        bg: bg,
        color: color,
        border: #ffffff22,
        border_w: 1.0,
        r_tl: 12.0,
        r_tr: 12.0,
        r_br: 12.0,
        r_bl: 12.0,
        pad_t: 11.0,
        pad_r: 18.0,
        pad_b: 11.0,
        pad_l: 18.0,
        font_size: 15.0,
        font_weight: 600,
        hover_scale: 1.04,
        transition_ms: 150.0,
        ..style_default()
    };
}

// ============================================================================
// Declarative Widget Constructors
// ============================================================================

fn w_col(key: str, style: Style, children: vec[Widget]) -> Widget {
    let empty_u: vec[f64] = vec_new();
    return Widget {
        key: key,
        kind: "col",
        text: "",
        action: "",
        value: 0.0,
        uniforms: empty_u,
        style: style,
        children: children,
    };
}

fn w_row(key: str, style: Style, children: vec[Widget]) -> Widget {
    let empty_u: vec[f64] = vec_new();
    return Widget {
        key: key,
        kind: "row",
        text: "",
        action: "",
        value: 0.0,
        uniforms: empty_u,
        style: style,
        children: children,
    };
}

fn w_grid(key: str, cols: i64, style: Style, children: vec[Widget]) -> Widget {
    let empty_u: vec[f64] = vec_new();
    return Widget {
        key: key,
        kind: "grid",
        text: "",
        action: "",
        value: 0.0,
        uniforms: empty_u,
        style: Style { grid_cols: cols, ..style },
        children: children,
    };
}

fn w_stack(key: str, style: Style, children: vec[Widget]) -> Widget {
    let empty_u: vec[f64] = vec_new();
    return Widget {
        key: key,
        kind: "stack",
        text: "",
        action: "",
        value: 0.0,
        uniforms: empty_u,
        style: style,
        children: children,
    };
}

fn w_card(key: str, style: Style, children: vec[Widget]) -> Widget {
    let empty_u: vec[f64] = vec_new();
    return Widget {
        key: key,
        kind: "card",
        text: "",
        action: "",
        value: 0.0,
        uniforms: empty_u,
        style: style,
        children: children,
    };
}

fn w_text(key: str, text: str, font_size: f64, color: str) -> Widget {
    let empty_u: vec[f64] = vec_new();
    let empty_c: vec[Widget] = vec_new();
    return Widget {
        key: key,
        kind: "text",
        text: text,
        action: "",
        value: 0.0,
        uniforms: empty_u,
        style: Style {
            color: color,
            font_size: font_size,
            ..style_default()
        },
        children: empty_c,
    };
}

fn w_badge(key: str, text: str, bg: str, color: str) -> Widget {
    let empty_u: vec[f64] = vec_new();
    let empty_c: vec[Widget] = vec_new();
    return Widget {
        key: key,
        kind: "badge",
        text: text,
        action: "",
        value: 0.0,
        uniforms: empty_u,
        style: Style {
            bg: bg,
            color: color,
            border: #38bdf855,
            r_tl: 999.0,
            r_tr: 999.0,
            r_br: 999.0,
            r_bl: 999.0,
            pad_t: 6.0,
            pad_r: 12.0,
            pad_b: 6.0,
            pad_l: 12.0,
            font_size: 13.0,
            font_weight: 600,
            ..style_default()
        },
        children: empty_c,
    };
}

fn w_button(key: str, label: str, action: str, style: Style) -> Widget {
    let empty_u: vec[f64] = vec_new();
    let empty_c: vec[Widget] = vec_new();
    return Widget {
        key: key,
        kind: "button",
        text: label,
        action: action,
        value: 0.0,
        uniforms: empty_u,
        style: style,
        children: empty_c,
    };
}

fn w_input(key: str, text: str, action: str, style: Style) -> Widget {
    let empty_u: vec[f64] = vec_new();
    let empty_c: vec[Widget] = vec_new();
    return Widget {
        key: key,
        kind: "input",
        text: text,
        action: action,
        value: 0.0,
        uniforms: empty_u,
        style: style,
        children: empty_c,
    };
}

fn w_slider(key: str, action: str, value: f64, style: Style) -> Widget {
    let empty_u: vec[f64] = vec_new();
    let empty_c: vec[Widget] = vec_new();
    return Widget {
        key: key,
        kind: "slider",
        text: "",
        action: action,
        value: value,
        uniforms: empty_u,
        style: style,
        children: empty_c,
    };
}

fn w_toggle(key: str, label: str, action: str, checked: bool, style: Style) -> Widget {
    let empty_u: vec[f64] = vec_new();
    let empty_c: vec[Widget] = vec_new();
    let v = if checked { 1.0 } else { 0.0 };
    return Widget {
        key: key,
        kind: "toggle",
        text: label,
        action: action,
        value: v,
        uniforms: empty_u,
        style: style,
        children: empty_c,
    };
}

fn w_shader(key: str, shader_name: str, uniforms: vec[f64], style: Style) -> Widget {
    let empty_c: vec[Widget] = vec_new();
    return Widget {
        key: key,
        kind: "shader",
        text: shader_name,
        action: "",
        value: 0.0,
        uniforms: uniforms,
        style: style,
        children: empty_c,
    };
}

fn w_divider(key: str, style: Style) -> Widget {
    let empty_u: vec[f64] = vec_new();
    let empty_c: vec[Widget] = vec_new();
    return Widget {
        key: key,
        kind: "divider",
        text: "",
        action: "",
        value: 0.0,
        uniforms: empty_u,
        style: style,
        children: empty_c,
    };
}

fn w_progress(key: str, value: f64, style: Style) -> Widget {
    let empty_u: vec[f64] = vec_new();
    let empty_c: vec[Widget] = vec_new();
    return Widget {
        key: key,
        kind: "progress",
        text: "",
        action: "",
        value: value,
        uniforms: empty_u,
        style: style,
        children: empty_c,
    };
}
"###;

/// Standard system, filesystem, JSON, and HTTP/1.1 client helpers (`import "std/sys";`).
pub const STD_SYS_SRC: &str = r###"// Standard System, Filesystem, JSON & HTTP Client Library (`std/sys`).
// Capability-isolated via `effects(Hw)` (filesystem & JSON) and `effects(Net)` (HTTP/1.1 client).

schema FileWrite {
    0: path str,
    1: content str,
}

schema JsonQuery {
    0: json str,
    1: key str,
}

schema HttpPost {
    0: url str,
    1: body str,
}

fn fs_read(path: str) -> str effects(Hw) {
    let out: str = syscall Hw "fs.read_file" path;
    return out;
}

fn fs_write(path: str, content: str) -> bool effects(Hw) {
    let req = FileWrite { path: path, content: content };
    let ok: bool = syscall Hw "fs.write_file" req;
    return ok;
}

fn fs_exists(path: str) -> bool effects(Hw) {
    let ok: bool = syscall Hw "fs.exists" path;
    return ok;
}

fn fs_remove(path: str) -> bool effects(Hw) {
    let ok: bool = syscall Hw "fs.remove_file" path;
    return ok;
}

fn fs_list(dir: str) -> str effects(Hw) {
    let out: str = syscall Hw "fs.list_dir" dir;
    return out;
}

fn json_is_valid(raw: str) -> bool effects(Hw) {
    let ok: bool = syscall Hw "json.is_valid" raw;
    return ok;
}

fn json_pretty(raw: str) -> str effects(Hw) {
    let out: str = syscall Hw "json.pretty" raw;
    return out;
}

fn json_get(raw: str, key: str) -> str effects(Hw) {
    let q = JsonQuery { json: raw, key: key };
    let out: str = syscall Hw "json.get" q;
    return out;
}

fn http_get(url: str) -> str effects(Net) {
    let body: str = syscall Net "http.get" url;
    return body;
}

fn http_post(url: str, body: str) -> str effects(Net) {
    let req = HttpPost { url: url, body: body };
    let resp: str = syscall Net "http.post" req;
    return resp;
}
"###;

/// Resolves a built-in standard library import path (`"std/ui"`, `"std/anim"`, `"std/math"`,
/// `"std/sys"`, with or without a `.knr` suffix) to its embedded `.knr` source code.
pub fn resolve_builtin_std(import_path: &str) -> Option<&'static str> {
    let normalized = import_path.strip_suffix(".knr").unwrap_or(import_path);
    match normalized {
        "std/ui" => Some(STD_UI_SRC),
        "std/anim" => Some(STD_ANIM_SRC),
        "std/math" => Some(STD_MATH_SRC),
        "std/sys" => Some(STD_SYS_SRC),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;
    use crate::lower::lower_program;
    use crate::syntax::parse_knr;

    #[test]
    fn test_resolve_builtin_std_paths() {
        assert!(resolve_builtin_std("std/ui").is_some());
        assert!(resolve_builtin_std("std/ui.knr").is_some());
        assert!(resolve_builtin_std("std/anim").is_some());
        assert!(resolve_builtin_std("std/anim.knr").is_some());
        assert!(resolve_builtin_std("std/math").is_some());
        assert!(resolve_builtin_std("std/math.knr").is_some());
        assert!(resolve_builtin_std("std/sys").is_some());
        assert!(resolve_builtin_std("std/sys.knr").is_some());
        assert!(resolve_builtin_std("./std_ui.knr").is_none());
        assert!(resolve_builtin_std("std/unknown").is_none());
    }

    #[test]
    fn test_builtin_std_sys_lowering() {
        let src = r#"
            import "std/sys";

            fn check_fs_and_json() -> bool effects(Hw) {
                let wrote = fs_write("/tmp/test_sys.json", "{\"user\":{\"name\":\"knr\"}}");
                let exists = fs_exists("/tmp/test_sys.json");
                let valid = json_is_valid(fs_read("/tmp/test_sys.json"));
                let pretty = json_pretty("{\"a\":1}");
                let name = json_get("{\"user\":{\"name\":\"knr\"}}", "user.name");
                let listing = fs_list("/tmp");
                let removed = fs_remove("/tmp/test_sys.json");
                return wrote
                    && exists
                    && valid
                    && str_len(pretty) > 0
                    && name == "knr"
                    && str_len(listing) >= 0
                    && removed;
            }

            fn check_http() -> str effects(Net) {
                let a = http_get("http://127.0.0.1:8080/health");
                let b = http_post("http://127.0.0.1:8080/echo", "{}");
                return a + b;
            }
        "#;

        let prog = parse_knr(src).expect("parse program using std/sys");
        let module = lower_program(&prog).expect("lower program using std/sys");

        let mut schema_names = HashSet::new();
        for s in &module.schemas {
            assert!(
                schema_names.insert(s.name.clone()),
                "Duplicate schema '{}'",
                s.name
            );
        }
        assert!(schema_names.contains("FileWrite"));
        assert!(schema_names.contains("JsonQuery"));
        assert!(schema_names.contains("HttpPost"));

        let mut fn_names = HashSet::new();
        for f in &module.functions {
            assert!(
                fn_names.insert(f.name.clone()),
                "Duplicate function '{}'",
                f.name
            );
        }
        for expected in [
            "fs_read",
            "fs_write",
            "fs_exists",
            "fs_remove",
            "fs_list",
            "json_is_valid",
            "json_pretty",
            "json_get",
            "http_get",
            "http_post",
        ] {
            assert!(
                fn_names.contains(expected),
                "Missing std/sys function '{expected}'"
            );
        }
    }

    #[test]
    fn test_builtin_std_in_memory_and_diamond_dedup() {
        let src = r#"
            import "std/ui";
            import "std/anim";
            import "std/math";
            import "std/ui.knr";

            fn main() -> f64 {
                let a = Vec3 { x: 1.0, y: 2.0, z: 2.0 };
                let b = Vec3 { x: 0.0, y: 1.0, z: 0.0 };
                let c = vec3_add(&a, &b);
                let n = vec3_norm(&c);
                return smin(vec3_len(&n), ease_out_expo(0.5), 0.25) + pi();
            }
        "#;

        let prog = parse_knr(src).expect("parse program using built-in std imports");
        let module = lower_program(&prog).expect("lower program using built-in std imports");

        // Ensure zero duplicate schema names and zero duplicate function names
        let mut schema_names = HashSet::new();
        for s in &module.schemas {
            assert!(
                schema_names.insert(s.name.clone()),
                "Duplicate schema '{}'",
                s.name
            );
        }
        assert!(schema_names.contains("Vec2"));
        assert!(schema_names.contains("Vec3"));
        assert!(schema_names.contains("Color"));
        assert!(schema_names.contains("Style"));
        assert!(schema_names.contains("Widget"));

        let mut fn_names = HashSet::new();
        for f in &module.functions {
            assert!(
                fn_names.insert(f.name.clone()),
                "Duplicate function '{}'",
                f.name
            );
        }
        assert!(fn_names.contains("w_shader"));
        assert!(fn_names.contains("ease_out_expo"));
        assert!(fn_names.contains("smin"));
        assert!(fn_names.contains("vec3_norm"));
    }

    #[test]
    fn test_builtin_std_reverse_import_order() {
        let src = r#"
            import "std/math";
            import "std/anim";
            import "std/ui";

            fn main() -> f64 {
                let v = Vec2 { x: 3.0, y: 4.0 };
                return vec2_len(&v) + ease_out_cubic(0.5);
            }
        "#;

        let prog = parse_knr(src).expect("parse reverse-order std imports");
        let module = lower_program(&prog).expect("lower reverse-order std imports");
        let mut schema_names = HashSet::new();
        for s in &module.schemas {
            assert!(
                schema_names.insert(s.name.clone()),
                "Duplicate schema '{}'",
                s.name
            );
        }
        assert!(schema_names.contains("Vec2"));
        assert!(schema_names.contains("Vec3"));
        assert!(schema_names.contains("Widget"));
    }
}
