use std::fs;
use std::path::{Path, PathBuf};

use knr_core::manifest::{Manifest, PackageManifest, WorkspaceManifest};
use crate::error::CliError;

pub const TEMPLATE_MINIMAL: &str = r#"// Knr Minimal Starter Application (Strict TypeScript)
export function main(): i64 {
    const code: i64 = 0;
    return code;
}
"#;

pub const TEMPLATE_APP_STATE: &str = r#"// Application MVU State & Typed Actions (`src/state.tsk`)
export enum Action {
    Increment,
    Decrement,
    Reset,
    TogglePulse,
}

export type AppState = {
    count: i64;
    label: string;
    pulse: bool;
    energy: f64;
};

export function init(): AppState {
    return {
        count: 1,
        label: "Knr v0.4 Modular GPU App",
        pulse: true,
        energy: 0.72,
    };
}

export function on_input(s: AppState, action: string, val: string): AppState {
    if (action === "label_input") {
        return { ...s, label: val };
    }
    return s;
}

export function update(s: AppState, act: Action): AppState {
    switch (act) {
        case Action.Increment:
            return { ...s, count: s.count + 1 };
        case Action.Decrement:
            return { ...s, count: s.count - 1 };
        case Action.Reset:
            return init();
        case Action.TogglePulse:
            return { ...s, pulse: !s.pulse };
    }
}
"#;

pub const TEMPLATE_APP_SHADERS: &str = r#"// Pure GPU @shader Kernels (`src/shaders.tsk`)
import { Vec2, Color } from "std/ui";

@shader
export function app_aurora(uv: Vec2, time: f64, energy: f64): Color {
    const t = time * (0.4 + energy * 0.9);
    const wave = 0.5 + 0.5 * sin(uv.x * 5.0 + t + cos(uv.y * 3.0 - t * 0.6) * 0.6);
    const r = clamp(0.04 + wave * 0.16, 0.0, 1.0);
    const g = clamp(0.10 + wave * 0.34, 0.0, 1.0);
    const b = clamp(0.24 + wave * 0.58, 0.0, 1.0);
    return { r, g, b, a: 1.0 };
}
"#;

pub const TEMPLATE_APP: &str = r##"// Modular Knr GPU MVU Application (`src/app.tsk`)
import {
    Column,
    Row,
    Stack,
    Card,
    Text,
    Button,
    Input,
    Badge,
    Shader,
    Icon,
    Divider,
} from "std/ui";
import { Action, AppState, init, update, on_input } from "./state.tsk";
import { app_aurora } from "./shaders.tsk";

export function view(s: readonly AppState): Widget {
    const energy = s.pulse ? s.energy : 0.15;
    return (
        <Column key="app_root" w="full" h="full" bg="#06080f" align="center" p={28}>
            <Column key="app_shell" w="full" maxW={760} gap={18}>
                <Stack key="app_hero" w="full" radius={16} border="#ffffff18" overflow="hidden">
                    <Shader key="app_shader" fn={app_aurora} h={110} radius={16} uniforms={[energy]} />
                    <Row key="app_header" w="full" h={110} px={22} align="center" justify="between" z={2}>
                        <Row align="center" gap={10}>
                            <Icon name="sparkles" size={20} color="#38bdf8" />
                            <Text size={22} weight="bold" font="display" color="#f8fafc">
                                {s.label}
                            </Text>
                        </Row>
                        <Badge bg="#060d1ce6" color="#38bdf8" border="#38bdf855">
                            {s.pulse ? "LIVE" : "PAUSED"}
                        </Badge>
                    </Row>
                </Stack>

                <Card key="app_card" bg="#0b0f1c" border="#ffffff16" radius={16} p={22} gap={16}>
                    <Row justify="between" align="center">
                        <Text size={14} weight="semibold" color="#94a3b8">
                            LINEAR COUNTER
                        </Text>
                        <Badge bg="#141f38" color="#e2e8f0" border="#38bdf844">
                            {`Count: ${s.count}`}
                        </Badge>
                    </Row>
                    <Text size={36} weight="bold" font="display" color="#38bdf8">
                        {`${s.count}`}
                    </Text>
                    <Row gap={10} align="center">
                        <Button key="btn_dec" onClick={Action.Decrement} bg="#1e293b" color="#f8fafc" px={14} py={9} radius={10}>
                            Decrement
                        </Button>
                        <Button key="btn_inc" onClick={Action.Increment} bg="#0ea5e9" color="#ffffff" px={16} py={9} radius={10}>
                            Increment
                        </Button>
                        <Button key="btn_pulse" onClick={Action.TogglePulse} bg="#171730" color="#cbd5e1" px={14} py={9} radius={10}>
                            Toggle Pulse
                        </Button>
                        <Button key="btn_reset" onClick={Action.Reset} bg="#0f172a" color="#cbd5e1" px={14} py={9} radius={10}>
                            Reset
                        </Button>
                    </Row>
                    <Divider />
                    <Input key="app_label_input" value={s.label} onInput="label_input" />
                </Card>
            </Column>
        </Column>
    );
}

export function main(): i64 {
    const s0 = init();
    const s1 = update(s0, Action.Increment);
    return s1.count === 2 ? 0 : 1;
}

export function test_app_mvu_transitions(): bool {
    let s = init();
    s = update(s, Action.Increment);
    s = update(s, Action.Increment);
    s = update(s, Action.TogglePulse);
    s = on_input(s, "label_input", "Verified App");
    return s.count === 3 && !s.pulse && s.label === "Verified App";
}

export function test_app_view_renders(): bool {
    const s = init();
    const tree = view(s);
    return tree.kind === "col";
}
"##;

pub const TEMPLATE_APP_MAIN: &str = r#"// CLI Entrypoint for Modular Knr App (`src/main.tsk`)
import { Action, init, update } from "./state.tsk";

export function main(): i64 {
    const s0 = init();
    const s1 = update(s0, Action.Increment);
    return s1.count === 2 ? 0 : 1;
}
"#;

pub const TEMPLATE_UI: &str = r##"// Knr Unified GPU UI Starter (Strict TSX)
import { Column, Row, Card, Text, Button, Badge, Icon } from "std/ui";

export type AppState = {
    count: i64;
};

export function init(): AppState {
    return { count: 0 };
}

export function update(s: AppState, action: string): AppState {
    if (action === "inc") {
        return { count: s.count + 1 };
    }
    if (action === "dec") {
        return { count: s.count - 1 };
    }
    if (action === "reset") {
        return { count: 0 };
    }
    return s;
}

export function view(s: readonly AppState): Widget {
    return (
        <Column p={32} gap={20} bg="#050811" align="center">
            <Card p={24} gap={16} maxW={480} radius={16}>
                <Row justify="between" align="center">
                    <Row gap={8} align="center">
                        <Icon name="sparkles" size={18} color="#38bdf8" />
                        <Text size={20} weight="bold">Knr Starter UI</Text>
                    </Row>
                    <Badge>v0.4.0</Badge>
                </Row>
                <Text size={32} weight="bold" color="#38bdf8">Count: {s.count}</Text>
                <Row gap={12}>
                    <Button onClick="dec" bg="#1e293b">Decrement</Button>
                    <Button onClick="inc" bg="#0ea5e9">Increment</Button>
                    <Button onClick="reset" bg="#0f172a">Reset</Button>
                </Row>
            </Card>
        </Column>
    );
}

export function main(): i64 {
    const s0 = init();
    const s1 = update(s0, "inc");
    return s1.count;
}
"##;

pub const TEMPLATE_DASHBOARD: &str = r##"// Knr Responsive Bento Dashboard (`src/app.tsk`)
import {
    Column,
    Row,
    Grid,
    Card,
    Scroll,
    Text,
    Badge,
    Button,
    Input,
    Slider,
    Shader,
    Icon,
    Divider,
    Progress,
    Vec2,
    Color,
} from "std/ui";

export type MetricEntry = {
    name: string;
    region: string;
    latency_ms: i64;
    healthy: bool;
};

export type DashboardState = {
    query: string;
    load_scale: f64;
    refresh_count: i64;
    entries: MetricEntry[];
};

@shader
export function dashboard_telemetry_wave(uv: Vec2, time: f64, load: f64): Color {
    const t = time * (0.45 + load * 1.4);
    const curve = 0.5 + 0.28 * sin(uv.x * 7.5 + t) * cos(uv.x * 2.5 - t * 0.4);
    const dist = abs(uv.y - curve);
    const line = smoothstep(0.16, 0.0, dist);
    const fill = smoothstep(0.55, 0.0, max(0.0, uv.y - curve));
    const r = clamp(0.04 + line * 0.20 + fill * 0.05, 0.0, 1.0);
    const g = clamp(0.08 + line * 0.72 + fill * 0.16, 0.0, 1.0);
    const b = clamp(0.16 + line * 0.96 + fill * 0.32, 0.0, 1.0);
    return { r, g, b, a: 1.0 };
}

export function init(): DashboardState {
    const entries: MetricEntry[] = [
        { name: "edge-gateway", region: "us-east", latency_ms: 14, healthy: true },
        { name: "shader-worker", region: "eu-west", latency_ms: 21, healthy: true },
        { name: "verify-cluster", region: "ap-south", latency_ms: 29, healthy: true },
        { name: "storage-node", region: "us-west", latency_ms: 42, healthy: false },
    ];
    return {
        query: "",
        load_scale: 0.64,
        refresh_count: 1,
        entries,
    };
}

export function on_input(s: DashboardState, action: string, val: string): DashboardState {
    if (action === "search_query") {
        return { ...s, query: val };
    }
    return s;
}

export function on_slider(s: DashboardState, action: string, val: f64): DashboardState {
    if (action === "slider_load") {
        return { ...s, load_scale: clamp(val, 0.0, 1.0) };
    }
    return s;
}

export function update(s: DashboardState, action: string): DashboardState {
    if (action === "refresh") {
        return { ...s, refresh_count: s.refresh_count + 1 };
    }
    if (action === "reset") {
        return init();
    }
    return s;
}

export function count_healthy(s: readonly DashboardState): i64 {
    let ok = 0;
    const n = s.entries.length;
    for (let i = 0; i < n; i += 1) {
        if (s.entries[i].healthy) {
            ok += 1;
        }
    }
    return ok;
}

export function view(s: readonly DashboardState): Widget {
    const total = s.entries.length;
    const healthy = count_healthy(s);
    const pct = total > 0 ? toFloat(healthy) / toFloat(total) : 1.0;
    const throughput = toInt(1200.0 + s.load_scale * 4800.0);

    return (
        <Column key="dash_root" w="full" h="full" bg="#06080f" align="center" p={24}>
            <Column key="dash_shell" w="full" maxW={1080} gap={16}>
                <Row key="dash_header" w="full" justify="between" align="center" gap={12}>
                    <Row align="center" gap={10}>
                        <Icon name="activity" size={20} color="#38bdf8" />
                        <Text size={22} weight="bold" font="display" color="#f8fafc">
                            Knr Telemetry Bento Dashboard
                        </Text>
                    </Row>
                    <Row align="center" gap={8}>
                        <Badge bg="#0e1424" color="#38bdf8" border="#38bdf844">
                            {`Sync #${s.refresh_count}`}
                        </Badge>
                        <Button key="btn_refresh" onClick="refresh" bg="#0ea5e9" color="#ffffff" px={14} py={8} radius={9}>
                            Refresh
                        </Button>
                    </Row>
                </Row>

                <Grid key="dash_kpi_grid" cols={3} gap={14}>
                    <Card bg="#0b0f1c" border="#ffffff14" radius={14} p={16} gap={8}>
                        <Row justify="between" align="center">
                            <Text size={12} weight="semibold" color="#94a3b8">HEALTHY NODES</Text>
                            <Icon name="shield" size={14} color="#38bdf8" />
                        </Row>
                        <Text size={26} weight="bold" font="display" color="#f8fafc">
                            {`${healthy}/${total}`}
                        </Text>
                        <Progress value={pct} h={6} color="linear(90, #38bdf8, #818cf8)" />
                    </Card>

                    <Card bg="#0b0f1c" border="#ffffff14" radius={14} p={16} gap={8}>
                        <Row justify="between" align="center">
                            <Text size={12} weight="semibold" color="#94a3b8">THROUGHPUT</Text>
                            <Icon name="zap" size={14} color="#818cf8" />
                        </Row>
                        <Text size={26} weight="bold" font="display" color="#f8fafc">
                            {`${throughput} req/s`}
                        </Text>
                        <Text size={12} font="mono" color="#94a3b8">
                            Zero-GC Cranelift native runtime
                        </Text>
                    </Card>

                    <Card bg="#0b0f1c" border="#ffffff14" radius={14} p={16} gap={8}>
                        <Row justify="between" align="center">
                            <Text size={12} weight="semibold" color="#94a3b8">LOAD TARGET</Text>
                            <Icon name="sliders" size={14} color="#38bdf8" />
                        </Row>
                        <Text size={26} weight="bold" font="display" color="#f8fafc">
                            {`${toInt(s.load_scale * 100.0)}%`}
                        </Text>
                        <Slider key="dash_load_slider" value={s.load_scale} onChange="slider_load" />
                    </Card>
                </Grid>

                <Grid key="dash_main_grid" cols={2} gap={16}>
                    <Card bg="#0b0f1c" border="#ffffff14" radius={16} p={18} gap={12}>
                        <Row justify="between" align="center">
                            <Text size={14} weight="semibold" color="#f8fafc">LIVE GPU TELEMETRY</Text>
                            <Badge bg="#141f38" color="#38bdf8" border="#38bdf833">WGSL</Badge>
                        </Row>
                        <Shader
                            key="dash_wave_shader"
                            fn={dashboard_telemetry_wave}
                            h={220}
                            radius={12}
                            uniforms={[s.load_scale]}
                        />
                        <Row gap={8} align="center">
                            <Input key="dash_search_input" value={s.query} onInput="search_query" grow={1} />
                            <Button key="btn_reset" onClick="reset" bg="#1e293b" color="#f8fafc" px={12} py={8} radius={8}>
                                Reset
                            </Button>
                        </Row>
                    </Card>

                    <Card bg="#0b0f1c" border="#ffffff14" radius={16} p={18} gap={12}>
                        <Row justify="between" align="center">
                            <Text size={14} weight="semibold" color="#f8fafc">CLUSTER SERVICES</Text>
                            <Badge bg="#171730" color="#a5b4fc" border="#818cf833">
                                {`${total} active`}
                            </Badge>
                        </Row>
                        <Divider />
                        <Scroll key="dash_service_scroll" w="full" h={240} gap={8} overflow="scroll">
                            {s.entries.map((entry, idx) => (
                                <Card
                                    key={`svc_row_${idx}`}
                                    bg="#0e1424"
                                    border="#ffffff14"
                                    radius={10}
                                    px={14}
                                    py={10}
                                >
                                    <Row justify="between" align="center" gap={10}>
                                        <Row align="center" gap={8}>
                                            <Icon
                                                name={entry.healthy ? "check" : "alert"}
                                                size={14}
                                                color={entry.healthy ? "#38bdf8" : "#f59e0b"}
                                            />
                                            <Text size={14} weight="semibold" color="#f8fafc">
                                                {entry.name}
                                            </Text>
                                        </Row>
                                        <Badge bg="#0b1222" color="#cbd5e1" border="#ffffff1a">
                                            {`${entry.region} ${entry.latency_ms}ms`}
                                        </Badge>
                                    </Row>
                                </Card>
                            ))}
                        </Scroll>
                    </Card>
                </Grid>
            </Column>
        </Column>
    );
}

export function main(): i64 {
    const s = init();
    return count_healthy(s);
}

export function test_dashboard_metrics(): bool {
    let s = init();
    s = on_slider(s, "slider_load", 0.9);
    s = on_input(s, "search_query", "edge");
    s = update(s, "refresh");
    return count_healthy(s) === 3 && s.refresh_count === 2 && s.query === "edge";
}

export function test_dashboard_view(): bool {
    const s = init();
    const tree = view(s);
    return tree.kind === "col";
}
"##;

pub const TEMPLATE_SHADER: &str = r##"// Knr 3D SDF GPU Shader Studio (`src/app.tsk`)
import {
    Column,
    Row,
    Grid,
    Card,
    Badge,
    Text,
    Button,
    Slider,
    Shader,
    Icon,
    Vec2,
    Color,
} from "std/ui";
import { Vec3, smin, vec3_norm, vec3_dot, tau } from "std/math";

export function sd_torus(px: f64, py: f64, pz: f64, major: f64, minor: f64): f64 {
    const qx = sqrt(px * px + pz * pz) - major;
    return sqrt(qx * qx + py * py) - minor;
}

export function sd_sphere(px: f64, py: f64, pz: f64, radius: f64): f64 {
    return sqrt(px * px + py * py + pz * pz) - radius;
}

export function sdf_map(px: f64, py: f64, pz: f64, time: f64, twist: f64): f64 {
    const ta = py * (twist * 2.0);
    const ct = cos(ta);
    const st = sin(ta);
    const tx = px * ct - pz * st;
    const tz = px * st + pz * ct;

    const a = time * 0.65;
    const ca = cos(a);
    const sa = sin(a);
    const ry = py * ca - tz * sa;
    const rz = py * sa + tz * ca;

    const d_torus = sd_torus(tx, ry, rz, 0.85, 0.26);
    const d_orb = sd_sphere(px - cos(time) * 0.72, py - sin(time * 1.3) * 0.35, pz - sin(time) * 0.72, 0.34);
    return smin(d_torus, d_orb, 0.38);
}

@shader
export function sdf_studio_scene(
    uv: Vec2,
    time: f64,
    mouse: Vec2,
    twist: f64,
    glow: f64,
    zoom: f64,
): Color {
    const sx = (uv.x - 0.5) * 2.1;
    const sy = (0.5 - uv.y) * 1.2;
    const yaw = (mouse.x - 0.5) * tau() * 0.5 + time * 0.25;
    const cam_dist = 3.6 - zoom * 1.2;
    const ro: Vec3 = { x: cam_dist * sin(yaw), y: 0.35, z: cam_dist * cos(yaw) };
    const neg_ro: Vec3 = { x: 0.0 - ro.x, y: 0.0 - ro.y, z: 0.0 - ro.z };
    const fwd = vec3_norm(neg_ro);
    const right_raw: Vec3 = { x: 0.0 - fwd.z, y: 0.0, z: fwd.x };
    const right = vec3_norm(right_raw);
    const up: Vec3 = {
        x: right.y * fwd.z - right.z * fwd.y,
        y: right.z * fwd.x - right.x * fwd.z,
        z: right.x * fwd.y - right.y * fwd.x,
    };
    const rd_raw: Vec3 = {
        x: fwd.x * 1.45 + right.x * sx + up.x * sy,
        y: fwd.y * 1.45 + right.y * sx + up.y * sy,
        z: fwd.z * 1.45 + right.z * sx + up.z * sy,
    };
    const rd = vec3_norm(rd_raw);

    let dist = 0.0;
    let hit = false;
    let halo = 0.0;
    for (let i = 0; i < 24; i += 1) {
        const px = ro.x + rd.x * dist;
        const py = ro.y + rd.y * dist;
        const pz = ro.z + rd.z * dist;
        const d = sdf_map(px, py, pz, time, twist);
        halo = halo + 0.025 / (abs(d) + 0.09);
        if (d < 0.008) {
            hit = true;
            break;
        }
        dist = dist + d;
        if (dist > 9.0) {
            break;
        }
    }

    let r = 0.03 + halo * glow * 0.05;
    let g = 0.05 + halo * glow * 0.14;
    let b = 0.11 + halo * glow * 0.26;

    if (hit) {
        const px = ro.x + rd.x * dist;
        const py = ro.y + rd.y * dist;
        const pz = ro.z + rd.z * dist;
        const eps = 0.006;
        const nx = sdf_map(px + eps, py, pz, time, twist) - sdf_map(px - eps, py, pz, time, twist);
        const ny = sdf_map(px, py + eps, pz, time, twist) - sdf_map(px, py - eps, pz, time, twist);
        const nz = sdf_map(px, py, pz + eps, time, twist) - sdf_map(px, py, pz - eps, time, twist);
        const n_raw: Vec3 = { x: nx, y: ny, z: nz };
        const n = vec3_norm(n_raw);
        const ldir_raw: Vec3 = { x: 0.65, y: 0.80, z: 0.55 };
        const ldir = vec3_norm(ldir_raw);
        const neg_rd: Vec3 = { x: 0.0 - rd.x, y: 0.0 - rd.y, z: 0.0 - rd.z };
        const diff = max(0.0, vec3_dot(n, ldir));
        const rim = pow(clamp(1.0 - max(0.0, vec3_dot(n, neg_rd)), 0.0, 1.0), 2.5);
        r = 0.08 + diff * 0.24 + rim * (0.30 + glow * 0.35);
        g = 0.18 + diff * 0.62 + rim * (0.55 + glow * 0.25);
        b = 0.34 + diff * 0.88 + rim * 0.92;
    }

    return {
        r: clamp(r, 0.0, 1.0),
        g: clamp(g, 0.0, 1.0),
        b: clamp(b, 0.0, 1.0),
        a: 1.0,
    };
}

export type ShaderStudioState = {
    twist: f64;
    glow: f64;
    zoom: f64;
    elapsed: f64;
};

export function init(): ShaderStudioState {
    return {
        twist: 0.45,
        glow: 0.72,
        zoom: 0.40,
        elapsed: 0.0,
    };
}

export function tick(s: ShaderStudioState, dt: f64, _wall: f64): ShaderStudioState {
    return { ...s, elapsed: s.elapsed + dt };
}

export function on_slider(s: ShaderStudioState, action: string, val: f64): ShaderStudioState {
    const v = clamp(val, 0.0, 1.0);
    if (action === "slider_twist") {
        return { ...s, twist: v };
    }
    if (action === "slider_glow") {
        return { ...s, glow: v };
    }
    if (action === "slider_zoom") {
        return { ...s, zoom: v };
    }
    return s;
}

export function update(s: ShaderStudioState, action: string): ShaderStudioState {
    if (action === "preset_nebula") {
        return { ...s, twist: 0.82, glow: 0.92, zoom: 0.65 };
    }
    if (action === "preset_minimal") {
        return { ...s, twist: 0.12, glow: 0.35, zoom: 0.25 };
    }
    if (action === "reset") {
        return init();
    }
    return s;
}

export function view(s: readonly ShaderStudioState): Widget {
    return (
        <Column key="shader_root" w="full" h="full" bg="#05070e" align="center" p={22}>
            <Column key="shader_shell" w="full" maxW={1100} gap={14}>
                <Row justify="between" align="center" gap={12}>
                    <Row align="center" gap={10}>
                        <Icon name="cube" size={20} color="#38bdf8" />
                        <Text size={20} weight="bold" font="display" color="#f8fafc">
                            Knr 3D SDF GPU Shader Studio
                        </Text>
                    </Row>
                    <Row align="center" gap={8}>
                        <Button key="btn_nebula" onClick="preset_nebula" bg="#141f38" color="#e2e8f0" border="#38bdf855" px={12} py={8} radius={8}>
                            Nebula
                        </Button>
                        <Button key="btn_minimal" onClick="preset_minimal" bg="#111827" color="#e2e8f0" border="#ffffff22" px={12} py={8} radius={8}>
                            Minimal
                        </Button>
                        <Button key="btn_reset" onClick="reset" bg="#0f172a" color="#cbd5e1" border="#ffffff18" px={12} py={8} radius={8}>
                            Reset
                        </Button>
                    </Row>
                </Row>

                <Shader
                    key="sdf_viewport"
                    fn={sdf_studio_scene}
                    h={460}
                    radius={16}
                    uniforms={[s.twist, s.glow, s.zoom]}
                />

                <Grid key="shader_controls" cols={3} gap={14}>
                    <Card bg="#0b0f1c" border="#ffffff14" radius={12} p={14} gap={8}>
                        <Row justify="between" align="center">
                            <Text size={12} weight="semibold" color="#cbd5e1">TWIST</Text>
                            <Badge bg="#141f38" color="#38bdf8">{`${toInt(s.twist * 100.0)}%`}</Badge>
                        </Row>
                        <Slider key="slider_twist" value={s.twist} onChange="slider_twist" />
                    </Card>

                    <Card bg="#0b0f1c" border="#ffffff14" radius={12} p={14} gap={8}>
                        <Row justify="between" align="center">
                            <Text size={12} weight="semibold" color="#cbd5e1">GLOW</Text>
                            <Badge bg="#141f38" color="#38bdf8">{`${toInt(s.glow * 100.0)}%`}</Badge>
                        </Row>
                        <Slider key="slider_glow" value={s.glow} onChange="slider_glow" />
                    </Card>

                    <Card bg="#0b0f1c" border="#ffffff14" radius={12} p={14} gap={8}>
                        <Row justify="between" align="center">
                            <Text size={12} weight="semibold" color="#cbd5e1">ZOOM</Text>
                            <Badge bg="#141f38" color="#38bdf8">{`${toInt(s.zoom * 100.0)}%`}</Badge>
                        </Row>
                        <Slider key="slider_zoom" value={s.zoom} onChange="slider_zoom" />
                    </Card>
                </Grid>
            </Column>
        </Column>
    );
}

export function main(): i64 {
    const s = init();
    return toInt(s.glow * 100.0);
}

export function test_shader_studio_controls(): bool {
    let s = init();
    s = on_slider(s, "slider_twist", 0.9);
    s = update(s, "preset_nebula");
    const tree = view(s);
    return toInt(s.glow * 100.0) === 92 && tree.kind === "col";
}
"##;

pub const TEMPLATE_SERVICE: &str = r#"// Knr Network Service Starter (Strict TypeScript)
export type ServiceConfig = {
    port: i64;
    workers: i64;
};

@effects("net")
export function start_service(cfg: readonly ServiceConfig): i64 {
    return cfg.port;
}

@effects("net")
export function main(): i64 {
    const config: ServiceConfig = {
        port: 8080,
        workers: 4,
    };
    const _status = start_service(config);
    return 0;
}
"#;

pub const TEMPLATE_LIB: &str = r#"// Knr Library Starter (Strict TypeScript)
export type Point = {
    x: i64;
    y: i64;
};

export function add_points(a: readonly Point, b: readonly Point): Point {
    return {
        x: a.x + b.x,
        y: a.y + b.y,
    };
}

export function main(): i64 {
    const p1: Point = { x: 10, y: 20 };
    const p2: Point = { x: 30, y: 40 };
    const p3 = add_points(p1, p2);
    return p3.x;
}
"#;

pub const DEFAULT_GITIGNORE: &str = r#"/target
/dist
*.knrbin
*.o
*.wasm
*.png
.DS_Store
"#;

fn normalize_template_name(template_name: &str) -> Result<&'static str, CliError> {
    match template_name.to_ascii_lowercase().as_str() {
        "minimal" | "basic" | "default" => Ok("minimal"),
        "app" => Ok("app"),
        "ui" => Ok("ui"),
        "dashboard" => Ok("dashboard"),
        "shader" => Ok("shader"),
        "service" | "server" => Ok("service"),
        "lib" => Ok("lib"),
        other => Err(CliError::Args(format!(
            "Unknown template '{other}'. Available templates: minimal, basic, app, ui, dashboard, shader, service, server, lib."
        ))),
    }
}

pub fn get_template_content(template_name: &str) -> Result<&'static str, CliError> {
    match normalize_template_name(template_name)? {
        "minimal" => Ok(TEMPLATE_MINIMAL),
        "app" => Ok(TEMPLATE_APP),
        "ui" => Ok(TEMPLATE_UI),
        "dashboard" => Ok(TEMPLATE_DASHBOARD),
        "shader" => Ok(TEMPLATE_SHADER),
        "service" => Ok(TEMPLATE_SERVICE),
        "lib" => Ok(TEMPLATE_LIB),
        _ => unreachable!(),
    }
}

pub fn entry_file_for_template(template_name: &str) -> &'static str {
    match template_name.to_ascii_lowercase().as_str() {
        "ui" | "app" | "dashboard" | "shader" => "src/app.tsk",
        _ => "src/main.tsk",
    }
}

fn is_ui_template(canonical: &str) -> bool {
    matches!(canonical, "ui" | "app" | "dashboard" | "shader")
}

fn write_template_files(base_dir: &Path, canonical: &str, force: bool) -> Result<(), CliError> {
    let src_dir = base_dir.join("src");
    fs::create_dir_all(&src_dir).map_err(|e| {
        CliError::Other(format!("Failed to create 'src' directory: {e}"))
    })?;

    let files: Vec<(&str, &str)> = match canonical {
        "app" => vec![
            ("src/state.tsk", TEMPLATE_APP_STATE),
            ("src/shaders.tsk", TEMPLATE_APP_SHADERS),
            ("src/app.tsk", TEMPLATE_APP),
            ("src/main.tsk", TEMPLATE_APP_MAIN),
        ],
        "ui" => vec![("src/app.tsk", TEMPLATE_UI)],
        "dashboard" => vec![("src/app.tsk", TEMPLATE_DASHBOARD)],
        "shader" => vec![("src/app.tsk", TEMPLATE_SHADER)],
        "service" => vec![("src/main.tsk", TEMPLATE_SERVICE)],
        "lib" => vec![("src/main.tsk", TEMPLATE_LIB)],
        _ => vec![("src/main.tsk", TEMPLATE_MINIMAL)],
    };

    for (rel, content) in files {
        let dest = base_dir.join(rel);
        if !dest.exists() || force {
            fs::write(&dest, content).map_err(|e| {
                CliError::Other(format!("Failed to write {rel}: {e}"))
            })?;
        }
    }

    Ok(())
}

/// Executes `knr init` in the specified directory or current working directory.
pub fn execute(
    dir: Option<&Path>,
    name: Option<&str>,
    workspace: bool,
    template: &str,
    force: bool,
) -> Result<(), CliError> {
    let target_dir = dir
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

    if !target_dir.exists() {
        fs::create_dir_all(&target_dir).map_err(|e| {
            CliError::Other(format!("Failed to create directory '{}': {e}", target_dir.display()))
        })?;
    }

    let manifest_path = target_dir.join("knr.toml");
    if manifest_path.exists() && !force {
        return Err(CliError::Other(format!(
            "Project manifest 'knr.toml' already exists in '{}'. Use --force to overwrite.",
            target_dir.display()
        )));
    }

    let canonical = normalize_template_name(template)?;
    let entry_rel = entry_file_for_template(canonical);

    if workspace {
        // Initialize multi-project workspace
        let ws_manifest = Manifest::Workspace(WorkspaceManifest {
            members: vec!["apps/*".to_string()],
            default_members: Some(vec!["apps/demo".to_string()]),
            dependencies: Default::default(),
        });
        fs::write(&manifest_path, ws_manifest.to_toml()).map_err(|e| {
            CliError::Other(format!("Failed to write workspace manifest: {e}"))
        })?;

        // Create default starter package in apps/demo
        let demo_dir = target_dir.join("apps").join("demo");
        fs::create_dir_all(&demo_dir).map_err(|e| {
            CliError::Other(format!("Failed to create demo app directory: {e}"))
        })?;

        let mut demo_pkg = PackageManifest::new("demo", "0.1.0");
        demo_pkg.entry = PathBuf::from(entry_rel);
        let demo_manifest = Manifest::Package(demo_pkg);
        fs::write(demo_dir.join("knr.toml"), demo_manifest.to_toml()).map_err(|e| {
            CliError::Other(format!("Failed to write demo manifest: {e}"))
        })?;

        write_template_files(&demo_dir, canonical, force)?;

        // Write .gitignore and README.md at workspace root
        let gitignore_path = target_dir.join(".gitignore");
        if !gitignore_path.exists() || force {
            let _ = fs::write(gitignore_path, DEFAULT_GITIGNORE);
        }

        let readme_path = target_dir.join("README.md");
        if !readme_path.exists() || force {
            let ws_name = name.unwrap_or("Knr");
            let readme_content = format!(
                "# {ws_name} Workspace\n\nA multi-project Knr workspace.\n\n## Members\n- `apps/demo`\n\n## Running\n```bash\nknr run apps/demo/{entry_rel}\n```\n"
            );
            let _ = fs::write(readme_path, readme_content);
        }

        println!("Initialized Knr workspace in '{}'", target_dir.display());
    } else {
        // Infer package name
        let inferred_name = if let Some(n) = name {
            n.to_string()
        } else {
            target_dir
                .canonicalize()
                .ok()
                .and_then(|c| c.file_name().map(|f| f.to_string_lossy().to_string()))
                .filter(|n| !n.is_empty())
                .unwrap_or_else(|| "knr_app".to_string())
        };

        // Initialize single package
        let mut pkg = PackageManifest::new(&inferred_name, "0.1.0");
        pkg.entry = PathBuf::from(entry_rel);
        let pkg_manifest = Manifest::Package(pkg);
        fs::write(&manifest_path, pkg_manifest.to_toml()).map_err(|e| {
            CliError::Other(format!("Failed to write knr.toml: {e}"))
        })?;

        write_template_files(&target_dir, canonical, force)?;

        let gitignore_path = target_dir.join(".gitignore");
        if !gitignore_path.exists() || force {
            let _ = fs::write(gitignore_path, DEFAULT_GITIGNORE);
        }

        let readme_path = target_dir.join("README.md");
        if !readme_path.exists() || force {
            let extra_ui = if is_ui_template(canonical) {
                format!(
                    "knr test {entry_rel}\nknr run {entry_rel} --gui\nknr render {entry_rel} -o preview.png --assert-audit --json\nknr build {entry_rel} --target web -o dist/\n"
                )
            } else {
                String::new()
            };
            let readme_content = format!(
                "# {inferred_name}\n\nA verifiable linear Strict TypeScript/TSX application built with Knr.\n\n## Running\n```bash\nknr check {entry_rel}\nknr run {entry_rel}\n{extra_ui}knr dev\n```\n"
            );
            let _ = fs::write(readme_path, readme_content);
        }

        println!("Initialized Knr project '{inferred_name}' in '{}'", target_dir.display());
    }

    Ok(())
}
