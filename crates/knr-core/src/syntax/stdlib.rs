//! Built-in standard library modules embedded directly in `knr-core` (Strict TypeScript / TSX).
//!
//! Enables zero-config imports such as:
//! - `import { Column, Row, Card, Text, Button, Shader, Icon, Code } from "std/ui";`
//! - `import { easeOutCubic, easeInOutSine, smoothDamp } from "std/anim";`
//! - `import { Vec2, Vec3, smin, vec3Norm, pi, tau } from "std/math";`
//! - `import { fsRead, fsWrite, jsonGet, httpGet } from "std/sys";`

/// Pure animation, easing, and damping helpers (`import ... from "std/anim";`).
pub const STD_ANIM_SRC: &str = r###"// Standard Pure Animation, Easing & Damping Helpers (`std/anim`).

export function ease_out_cubic(t: f64): f64 {
    const c = clamp(t, 0.0, 1.0);
    const inv = 1.0 - c;
    return 1.0 - inv * inv * inv;
}

export function easeOutCubic(t: f64): f64 {
    return ease_out_cubic(t);
}

export function ease_in_out_sine(t: f64): f64 {
    const c = clamp(t, 0.0, 1.0);
    return 0.5 - 0.5 * cos(c * 3.141592653589793);
}

export function easeInOutSine(t: f64): f64 {
    return ease_in_out_sine(t);
}

export function ease_out_expo(t: f64): f64 {
    const c = clamp(t, 0.0, 1.0);
    if (c <= 0.0) {
        return 0.0;
    }
    if (c >= 1.0) {
        return 1.0;
    }
    return 1.0 - pow(2.0, -10.0 * c);
}

export function easeOutExpo(t: f64): f64 {
    return ease_out_expo(t);
}

export function smooth_damp(current: f64, target: f64, dt: f64, rate: f64): f64 {
    const factor = clamp(dt * rate, 0.0, 1.0);
    return lerp(current, target, factor);
}

export function smoothDamp(current: f64, target: f64, dt: f64, rate: f64): f64 {
    return smooth_damp(current, target, dt, rate);
}
"###;

/// Pure GPU-compatible 2D/3D vector and SDF math helpers (`import ... from "std/math";`).
pub const STD_MATH_SRC: &str = r###"// Standard Pure GPU-Compatible 2D/3D Vector & SDF Math Library (`std/math`).

export type Vec2 = {
    x: f64;
    y: f64;
};

export type Vec3 = {
    x: f64;
    y: f64;
    z: f64;
};

export function pi(): f64 {
    return 3.141592653589793;
}

export function tau(): f64 {
    return 6.283185307179586;
}

export function vec2_len(v: readonly Vec2): f64 {
    return sqrt(v.x * v.x + v.y * v.y);
}

export function vec2_dot(a: readonly Vec2, b: readonly Vec2): f64 {
    return a.x * b.x + a.y * b.y;
}

export function vec3_add(a: readonly Vec3, b: readonly Vec3): Vec3 {
    return {
        x: a.x + b.x,
        y: a.y + b.y,
        z: a.z + b.z,
    };
}

export function vec3_sub(a: readonly Vec3, b: readonly Vec3): Vec3 {
    return {
        x: a.x - b.x,
        y: a.y - b.y,
        z: a.z - b.z,
    };
}

export function vec3_scale(v: readonly Vec3, s: f64): Vec3 {
    return {
        x: v.x * s,
        y: v.y * s,
        z: v.z * s,
    };
}

export function vec3_dot(a: readonly Vec3, b: readonly Vec3): f64 {
    return a.x * b.x + a.y * b.y + a.z * b.z;
}

export function vec3_len(v: readonly Vec3): f64 {
    return sqrt(v.x * v.x + v.y * v.y + v.z * v.z);
}

export function vec3_norm(v: readonly Vec3): Vec3 {
    const l = max(vec3_len(v), 0.000001);
    const inv = 1.0 / l;
    return {
        x: v.x * inv,
        y: v.y * inv,
        z: v.z * inv,
    };
}

export function smin(a: f64, b: f64, k: f64): f64 {
    const kk = max(k, 0.000001);
    const h = max(kk - abs(a - b), 0.0) / kk;
    return min(a, b) - h * h * kk * 0.25;
}
"###;

/// Standard unconstrained typed GPU UI, styling, widget constructors, and animation re-exports (`std/ui`).
/// 45-field `Style` schema rendered pixel-identically by the Unified GPU Engine (Desktop `wgpu`,
/// Headless PNG `knr render`, and Flutter-style Web Canvas+WebGPU).
pub const STD_UI_SRC: &str = r###"// Standard Unconstrained Typed GPU UI & Styling Primitives (`std/ui`).
// Zero HTML DOM, zero CSS, zero GLSL — 100% statically verified schemas rendered by ONE unified GPU engine.

import "std/anim";

export type Vec2 = {
    x: f64;
    y: f64;
};

export type Color = {
    r: f64;
    g: f64;
    b: f64;
    a: f64;
};

export type Style = {
    w: f64;
    h: f64;
    min_w: f64;
    max_w: f64;
    bg: string;
    color: string;
    border: string;
    border_w: f64;
    r_tl: f64;
    r_tr: f64;
    r_br: f64;
    r_bl: f64;
    pad_t: f64;
    pad_r: f64;
    pad_b: f64;
    pad_l: f64;
    gap_x: f64;
    gap_y: f64;
    font_size: f64;
    font_weight: i64;
    align: string;
    justify: string;
    grid_cols: i64;
    shadow_x: f64;
    shadow_y: f64;
    shadow_blur: f64;
    shadow_color: string;
    backdrop_blur: f64;
    opacity: f64;
    translate_x: f64;
    translate_y: f64;
    scale: f64;
    rotate_deg: f64;
    hover_bg: string;
    hover_border: string;
    hover_scale: f64;
    transition_ms: f64;
    overflow: string;
    z_index: i64;
    grow: f64;
    font_family: string;
    text_align: string;
    line_height: f64;
    col_span: i64;
    letter_spacing: f64;
};

export type Widget = {
    key: string;
    kind: string;
    text: string;
    action: string;
    value: f64;
    uniforms: f64[];
    style: Style;
    children: Widget[];
};

export function style_default(): Style {
    return {
        w: 0.0,
        h: 0.0,
        min_w: 0.0,
        max_w: 0.0,
        bg: "",
        color: "#f8fafc",
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
        font_size: 15.0,
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
        font_family: "sans",
        text_align: "left",
        line_height: 0.0,
        col_span: 1,
        letter_spacing: 0.0,
    };
}

export function style_card(): Style {
    return {
        ...style_default(),
        bg: "linear(155, #0e1424ee, #090d18f6)",
        color: "#f8fafc",
        border: "#ffffff16",
        border_w: 1.0,
        r_tl: 16.0,
        r_tr: 16.0,
        r_br: 16.0,
        r_bl: 16.0,
        pad_t: 22.0,
        pad_r: 22.0,
        pad_b: 22.0,
        pad_l: 22.0,
        gap_x: 12.0,
        gap_y: 12.0,
        shadow_x: 0.0,
        shadow_y: 14.0,
        shadow_blur: 32.0,
        shadow_color: "#00000066",
        backdrop_blur: 14.0,
        transition_ms: 160.0,
    };
}

export function style_button(bg: string, color: string): Style {
    return {
        ...style_default(),
        bg: bg,
        color: color,
        border: "#ffffff1c",
        border_w: 1.0,
        r_tl: 10.0,
        r_tr: 10.0,
        r_br: 10.0,
        r_bl: 10.0,
        pad_t: 10.0,
        pad_r: 16.0,
        pad_b: 10.0,
        pad_l: 16.0,
        font_size: 14.0,
        font_weight: 600,
        hover_scale: 1.02,
        transition_ms: 140.0,
    };
}
"###;

/// Standard system, filesystem, JSON, and HTTP/1.1 client helpers (`import ... from "std/sys";`).
pub const STD_SYS_SRC: &str = r###"// Standard System, Filesystem, JSON & HTTP Client Library (`std/sys`).
// Capability-isolated via `@effects("hw")` (filesystem & JSON) and `@effects("net")` (HTTP/1.1).

export type FileWrite = {
    path: string;
    content: string;
};

export type JsonQuery = {
    json: string;
    key: string;
};

export type HttpPost = {
    url: string;
    body: string;
};

@effects("hw")
export function fs_read(path: string): string {
    const out: string = syscall("hw", "fs.read_file", path);
    return out;
}

@effects("hw")
export function fs_write(path: string, content: string): bool {
    const req: FileWrite = { path, content };
    const ok: bool = syscall("hw", "fs.write_file", req);
    return ok;
}

@effects("hw")
export function fs_exists(path: string): bool {
    const ok: bool = syscall("hw", "fs.exists", path);
    return ok;
}

@effects("hw")
export function fs_remove(path: string): bool {
    const ok: bool = syscall("hw", "fs.remove_file", path);
    return ok;
}

@effects("hw")
export function fs_list(dir: string): string {
    const out: string = syscall("hw", "fs.list_dir", dir);
    return out;
}

@effects("hw")
export function json_is_valid(raw: string): bool {
    const ok: bool = syscall("hw", "json.is_valid", raw);
    return ok;
}

@effects("hw")
export function json_pretty(raw: string): string {
    const out: string = syscall("hw", "json.pretty", raw);
    return out;
}

@effects("hw")
export function json_get(raw: string, key: string): string {
    const q: JsonQuery = { json: raw, key };
    const out: string = syscall("hw", "json.get", q);
    return out;
}

@effects("net")
export function http_get(url: string): string {
    const body: string = syscall("net", "http.get", url);
    return body;
}

@effects("net")
export function http_post(url: string, body: string): string {
    const req: HttpPost = { url, body };
    const resp: string = syscall("net", "http.post", req);
    return resp;
}
"###;

/// Standard core algebraic generic types (`import { Option, Result } from "std/core";`).
pub const STD_CORE_SRC: &str = r###"// Standard Core Algebraic Generic Types (`std/core`).

export enum Option<T> {
    Some(T),
    None,
}

export enum Result<T, E> {
    Ok(T),
    Err(E),
}
"###;

/// Standard declarative side-effect commands (`import { Cmd, cmd_none, cmd_http_get, ... } from "std/cmd";`).
pub const STD_CMD_SRC: &str = r###"// Standard Declarative Side-Effect Commands (`std/cmd`).

export type Cmd = {
    kind: string;
    target: string;
    payload: string;
    on_result: string;
    delay_ms: f64;
};

export function cmd_none(): Cmd {
    return {
        kind: "none",
        target: "",
        payload: "",
        on_result: "",
        delay_ms: 0.0,
    };
}

export function cmd_http_get(url: string, on_result: string): Cmd {
    return {
        kind: "http_get",
        target: url,
        payload: "",
        on_result: on_result,
        delay_ms: 0.0,
    };
}

export function cmd_http_post(url: string, body: string, on_result: string): Cmd {
    return {
        kind: "http_post",
        target: url,
        payload: body,
        on_result: on_result,
        delay_ms: 0.0,
    };
}

export function cmd_timer(delay_ms: f64, on_fire: string): Cmd {
    return {
        kind: "timer",
        target: "",
        payload: "",
        on_result: on_fire,
        delay_ms: delay_ms,
    };
}

export function cmd_storage_save(key: string, value: string): Cmd {
    return {
        kind: "storage_save",
        target: key,
        payload: value,
        on_result: "",
        delay_ms: 0.0,
    };
}

export function cmd_storage_load(key: string, on_result: string): Cmd {
    return {
        kind: "storage_load",
        target: key,
        payload: "",
        on_result: on_result,
        delay_ms: 0.0,
    };
}
"###;

/// Standard design tokens & curated color themes (`import { Theme, theme_obsidian, ... } from "std/theme";`).
pub const STD_THEME_SRC: &str = r###"// Standard Design Tokens & Curated Themes (`std/theme`).

export type Theme = {
    bg: string;
    surface: string;
    surface_alt: string;
    border: string;
    text: string;
    text_muted: string;
    accent: string;
    accent_alt: string;
    success: string;
    warning: string;
    danger: string;
    radius_sm: f64;
    radius_md: f64;
    radius_lg: f64;
};

export function theme_obsidian(): Theme {
    return {
        bg: "#06080f",
        surface: "#0e1424",
        surface_alt: "#161e33",
        border: "#ffffff1a",
        text: "#f8fafc",
        text_muted: "#94a3b8",
        accent: "#38bdf8",
        accent_alt: "#818cf8",
        success: "#22c55e",
        warning: "#f59e0b",
        danger: "#ef4444",
        radius_sm: 6.0,
        radius_md: 12.0,
        radius_lg: 18.0,
    };
}

export function theme_light(): Theme {
    return {
        bg: "#f8fafc",
        surface: "#ffffff",
        surface_alt: "#f1f5f9",
        border: "#cbd5e1",
        text: "#0f172a",
        text_muted: "#475569",
        accent: "#0284c7",
        accent_alt: "#4f46e5",
        success: "#16a34a",
        warning: "#d97706",
        danger: "#dc2626",
        radius_sm: 6.0,
        radius_md: 12.0,
        radius_lg: 18.0,
    };
}

export function theme_nord(): Theme {
    return {
        bg: "#2e3440",
        surface: "#3b4252",
        surface_alt: "#434c5e",
        border: "#4c566a",
        text: "#eceff4",
        text_muted: "#d8dee9",
        accent: "#88c0d0",
        accent_alt: "#81a1c1",
        success: "#a3be8c",
        warning: "#ebcb8b",
        danger: "#bf616a",
        radius_sm: 6.0,
        radius_md: 10.0,
        radius_lg: 16.0,
    };
}

export function theme_solar(): Theme {
    return {
        bg: "#002b36",
        surface: "#073642",
        surface_alt: "#0d4452",
        border: "#586e75",
        text: "#fdf6e3",
        text_muted: "#93a1a1",
        accent: "#268bd2",
        accent_alt: "#2aa198",
        success: "#859900",
        warning: "#b58900",
        danger: "#dc322f",
        radius_sm: 6.0,
        radius_md: 12.0,
        radius_lg: 18.0,
    };
}
"###;

/// Standard zero-GC client-side router state helpers (`import { Route, router_init, ... } from "std/router";`).
pub const STD_ROUTER_SRC: &str = r###"// Standard Zero-GC Client-Side Router (`std/router`).

export type Route = {
    current: string;
    previous: string;
    depth: i64;
};

export function router_init(path: string): Route {
    return {
        current: path,
        previous: "",
        depth: 0,
    };
}

export function router_push(r: Route, path: string): Route {
    return {
        ...r,
        previous: r.current,
        current: path,
        depth: r.depth + 1,
    };
}

export function router_back(r: Route): Route {
    if (r.depth > 0) {
        return {
            ...r,
            current: r.previous,
            previous: "",
            depth: r.depth - 1,
        };
    }
    return r;
}

export function router_replace(r: Route, path: string): Route {
    return {
        ...r,
        current: path,
    };
}

export function router_is(r: readonly Route, path: string): bool {
    return r.current === path;
}

export function router_can_back(r: readonly Route): bool {
    return r.depth > 0;
}
"###;

/// Standard responsive breakpoint & layout helpers (`import { is_compact, responsive_cols, ... } from "std/layout";`).
pub const STD_LAYOUT_SRC: &str = r###"// Standard Responsive Breakpoint & Layout Helpers (`std/layout`).

export function is_compact(w: f64): bool {
    return w < 640.0;
}

export function is_medium(w: f64): bool {
    return w >= 640.0 && w < 1024.0;
}

export function is_wide(w: f64): bool {
    return w >= 1024.0;
}

export function responsive_cols(w: f64, sm: i64, md: i64, lg: i64): i64 {
    if (w < 640.0) {
        return sm;
    }
    if (w < 1024.0) {
        return md;
    }
    return lg;
}

export function responsive_pad(w: f64): f64 {
    if (w < 640.0) {
        return 12.0;
    }
    if (w < 1024.0) {
        return 20.0;
    }
    return 28.0;
}
"###;

/// Standard deterministic unit testing assertions (`import { assert_eq_i64, assert_near, ... } from "std/testing";`).
pub const STD_TESTING_SRC: &str = r###"// Standard Deterministic Unit Testing Assertions (`std/testing`).

export function assert_eq_i64(a: i64, b: i64): bool {
    return a === b;
}

export function assert_near(a: f64, b: f64, eps: f64): bool {
    return abs(a - b) <= eps;
}

export function assert_str_eq(a: string, b: string): bool {
    return a === b;
}

export function assert_true(b: bool): bool {
    return b;
}

export function assert_false(b: bool): bool {
    return !b;
}

export function assert_gt(a: i64, b: i64): bool {
    return a > b;
}

export function assert_lt(a: i64, b: i64): bool {
    return a < b;
}
"###;

/// Resolves a built-in standard library import path (`"std/ui"`, `"std/anim"`, `"std/math"`,
/// `"std/sys"`, `"std/core"`, `"std/cmd"`, `"std/theme"`, `"std/router"`, `"std/layout"`,
/// `"std/testing"`, with or without `.tsk`/`.knr` suffix) to its embedded source code.
pub fn resolve_builtin_std(import_path: &str) -> Option<&'static str> {
    let normalized = import_path
        .strip_suffix(".tsk")
        .or_else(|| import_path.strip_suffix(".knr"))
        .unwrap_or(import_path);
    match normalized {
        "std/ui" => Some(STD_UI_SRC),
        "std/anim" => Some(STD_ANIM_SRC),
        "std/math" => Some(STD_MATH_SRC),
        "std/sys" => Some(STD_SYS_SRC),
        "std/core" => Some(STD_CORE_SRC),
        "std/cmd" => Some(STD_CMD_SRC),
        "std/theme" => Some(STD_THEME_SRC),
        "std/router" => Some(STD_ROUTER_SRC),
        "std/layout" => Some(STD_LAYOUT_SRC),
        "std/testing" => Some(STD_TESTING_SRC),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lower::lower_program;
    use crate::syntax::parse_tsx;

    #[test]
    fn test_resolve_builtin_std_paths() {
        assert!(resolve_builtin_std("std/ui").is_some());
        assert!(resolve_builtin_std("std/ui.tsk").is_some());
        assert!(resolve_builtin_std("std/anim").is_some());
        assert!(resolve_builtin_std("std/math").is_some());
        assert!(resolve_builtin_std("std/sys").is_some());
        assert!(resolve_builtin_std("std/core").is_some());
        assert!(resolve_builtin_std("std/cmd").is_some());
        assert!(resolve_builtin_std("std/theme").is_some());
        assert!(resolve_builtin_std("std/router").is_some());
        assert!(resolve_builtin_std("std/layout").is_some());
        assert!(resolve_builtin_std("std/testing").is_some());
        assert!(resolve_builtin_std("std/unknown").is_none());
    }

    #[test]
    fn test_std_ui_and_jsx_lower_cleanly() {
        let src = r###"
import { Column, Row, Card, Text, Button, Shader, Icon, Code } from "std/ui";

type State = { count: i64; energy: f64; };

export function init(): State {
    return { count: 1, energy: 0.5 };
}

@shader
export function fx(uv: Vec2, time: f64, energy: f64): Color {
    const w = 0.5 + 0.5 * sin(uv.x * 6.0 + time + energy);
    return { r: w, g: 0.4, b: 0.9, a: 1.0 };
}

export function view(s: readonly State): Widget {
    return (
        <Column p={24} gap={16} bg="#06080f">
            <Card p={20} radius={16}>
                <Row justify="between" align="center">
                    <Text size={24} weight="bold">Count: {s.count}</Text>
                    <Button onClick="inc" bg="#0ea5e9" px={16} py={10}>
                        <Icon name="plus" size={16} color="#ffffff" />
                        <Text size={14} weight="semibold">Add</Text>
                    </Button>
                </Row>
                <Shader fn={fx} h={220} radius={12} uniforms={[s.energy]} />
                <Code lang="tsx">{`const x = ${s.count};`}</Code>
            </Card>
        </Column>
    );
}
"###;
        let prog = parse_tsx(src).expect("parse TSX");
        let module = lower_program(&prog).expect("lower TSX + JSX");
        assert!(module.get_schema("Widget").is_some());
        assert!(module.get_schema("Style").is_some());
        assert!(module.get_function("view").is_some());
        assert!(module.get_function("fx").is_some());
    }
}
