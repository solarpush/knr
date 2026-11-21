//! Real `wgpu 23.0` Hardware-Accelerated GPU UI & Custom `.knr` Shader Engine.
//!
//! Renders the entire knr declarative UI tree directly on the GPU (Vulkan / Metal / DX12 / GL) via:
//! - Pass 1: Instanced analytical SDF Rounded Quads (4 corner radii, linear/radial gradients,
//!   crisp borders, soft analytical SDF shadows, 2D scale/rotate/translate transforms, opacity)
//! - Pass 2: Dual Kawase offscreen `backdrop_blur` glassmorphism pass
//! - Pass 3: Dynamic `2048x2048` `R8Unorm` GPU `GlyphAtlas` (`ab_glyph` + 8x8 fallback)
//!   and `Rgba8UnormSrgb` `ImageCache`
//! - Pass 4: Dynamic `CustomShaderCache` compiling `.knr`-generated WGSL shaders directly into
//!   live `wgpu::RenderPipeline`s driven by 128-byte `KnrUniforms` at 60 FPS VSync!
//!
//! Safety & resource discipline:
//! - Uses strict `PresentMode::Fifo` + `desired_maximum_frame_latency: 1` (never queues frames ahead)
//! - Reuses persistent GPU vertex & dynamic-offset uniform buffers (zero GPU buffer allocations/frame)
//! - Clamps texture sizes & scissor/viewport rectangles strictly within attachment bounds
//! - Calls `device.poll(Maintain::Poll)` each frame to promptly reclaim GPU resources

use std::collections::HashMap;
use std::num::NonZeroU64;
use std::ops::Range;
use std::sync::Arc;

use ab_glyph::{Font, FontVec, PxScale, ScaleFont};
use bytemuck::{Pod, Zeroable};
use winit::window::Window;

use crate::gui::font::get_glyph;
use crate::gui::renderer::{parse_hex_color, RenderInteractionState, UiRenderer};
use crate::gui::ui_tree::{
    parse_fill_spec, LayoutNode, LayoutRect, ParsedFill, TextMeasurer, UiKind,
};

// ============================================================================
// GPU Vertex / Uniform Structures (16-byte aligned, Pod + Zeroable)
// ============================================================================

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct ScreenUniform {
    viewport: [f32; 2],
    _pad: [f32; 2],
}

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct QuadInstance {
    rect: [f32; 4],         // x, y, w, h
    radii: [f32; 4],        // tl, tr, br, bl
    color_start: [f32; 4],  // rgba
    color_end: [f32; 4],    // rgba
    border_color: [f32; 4], // rgba
    shadow_color: [f32; 4], // rgba
    params0: [f32; 4],      // border_w, fill_mode (0=solid,1=linear,2=radial,3=none), grad_rad, opacity
    params1: [f32; 4],      // shadow_x, shadow_y, shadow_blur, rotate_rad
    params2: [f32; 4],      // scale, translate_x, translate_y, 0.0
}

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct GlyphInstance {
    rect: [f32; 4],    // x, y, w, h
    uv_rect: [f32; 4], // u0, v0, u1, v1
    color: [f32; 4],   // rgba
}

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct BlurUniform {
    texel_size: [f32; 2],
    offset: f32,
    _pad: f32,
}

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct GlassCompositeUniform {
    viewport: [f32; 2],
    opacity: f32,
    use_local_uv: f32,
    rect: [f32; 4],
    radii: [f32; 4],
}

/// 128-byte uniform layout matching `knr-codegen::wgsl` `KnrUniforms` byte-for-byte!
#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
pub struct KnrShaderUniforms {
    pub resolution: [f32; 2],
    pub time: f32,
    pub dt: f32,
    pub mouse: [f32; 4],
    pub rect_radius: [f32; 4],
    pub params: [[f32; 4]; 4],
}

const MAX_BLUR_SLOTS: usize = 64;
const MAX_GLASS_SLOTS: usize = 64;
const MAX_SHADER_SLOTS: usize = 32;

// ============================================================================
// Built-in WGSL Shaders (SDF Quad, Dual Kawase Blur, Glass Composite, Text, Blit)
// ============================================================================

const SDF_QUAD_WGSL: &str = r#"
struct ScreenUniform {
    viewport: vec2<f32>,
    _pad: vec2<f32>,
};
@group(0) @binding(0) var<uniform> screen: ScreenUniform;

struct QuadIn {
    @location(0) rect: vec4<f32>,
    @location(1) radii: vec4<f32>,
    @location(2) color_start: vec4<f32>,
    @location(3) color_end: vec4<f32>,
    @location(4) border_color: vec4<f32>,
    @location(5) shadow_color: vec4<f32>,
    @location(6) params0: vec4<f32>,
    @location(7) params1: vec4<f32>,
    @location(8) params2: vec4<f32>,
};

struct VsOut {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) local_and_half: vec4<f32>,
    @location(1) radii: vec4<f32>,
    @location(2) color_start: vec4<f32>,
    @location(3) color_end: vec4<f32>,
    @location(4) @interpolate(flat) packed_bs: vec2<u32>,
    @location(5) params0: vec4<f32>,
    @location(6) shadow_params: vec3<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vid: u32, q: QuadIn) -> VsOut {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>( 1.0, -1.0),
        vec2<f32>(-1.0,  1.0),
        vec2<f32>(-1.0,  1.0),
        vec2<f32>( 1.0, -1.0),
        vec2<f32>( 1.0,  1.0),
    );
    let half = q.rect.zw * 0.5;
    let expand = q.params1.z + max(abs(q.params1.x), abs(q.params1.y)) + 4.0;
    let local = corners[vid] * (half + vec2<f32>(expand, expand));

    let sc = q.params2.x;
    let ang = q.params1.w;
    let ca = cos(ang);
    let sa = sin(ang);
    let scaled = local * sc;
    let rotated = vec2<f32>(scaled.x * ca - scaled.y * sa, scaled.x * sa + scaled.y * ca);
    let center = q.rect.xy + half + q.params2.yz;
    let px = center + rotated;

    let ndc = vec2<f32>(
        (px.x / screen.viewport.x) * 2.0 - 1.0,
        1.0 - (px.y / screen.viewport.y) * 2.0,
    );

    var out: VsOut;
    out.clip_pos = vec4<f32>(ndc, 0.0, 1.0);
    out.local_and_half = vec4<f32>(local, half);
    out.radii = q.radii;
    out.color_start = q.color_start;
    out.color_end = q.color_end;
    out.packed_bs = vec2<u32>(pack4x8unorm(q.border_color), pack4x8unorm(q.shadow_color));
    out.params0 = q.params0;
    out.shadow_params = q.params1.xyz;
    return out;
}

fn sd_rounded_box4(p: vec2<f32>, half: vec2<f32>, radii: vec4<f32>) -> f32 {
    var r = radii.x;
    if (p.x > 0.0) {
        r = select(radii.y, radii.z, p.y > 0.0);
    } else {
        r = select(radii.x, radii.w, p.y > 0.0);
    }
    let rc = clamp(r, 0.0, min(half.x, half.y));
    let q = abs(p) - half + vec2<f32>(rc, rc);
    return length(max(q, vec2<f32>(0.0, 0.0))) + min(max(q.x, q.y), 0.0) - rc;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let local_pos = in.local_and_half.xy;
    let half_size = in.local_and_half.zw;
    let border_color = unpack4x8unorm(in.packed_bs.x);
    let shadow_color = unpack4x8unorm(in.packed_bs.y);
    let opacity = in.params0.w;
    let d = sd_rounded_box4(local_pos, half_size, in.radii);

    var out_col = vec4<f32>(0.0, 0.0, 0.0, 0.0);

    // 1. Soft analytical SDF shadow
    let s_blur = in.shadow_params.z;
    if (shadow_color.a > 0.001 && s_blur > 0.0) {
        let sp = local_pos - in.shadow_params.xy;
        let sd = sd_rounded_box4(sp, half_size, in.radii);
        let sa = (1.0 - smoothstep(-s_blur * 0.3, s_blur, sd)) * shadow_color.a * opacity;
        out_col = vec4<f32>(shadow_color.rgb * sa, sa);
    }

    // 2. Interior fill (solid / linear / radial)
    let fill_mode = in.params0.y;
    let shape_alpha = clamp(0.5 - d, 0.0, 1.0);
    if (shape_alpha > 0.001 && fill_mode < 2.5) {
        var fill_col = in.color_start;
        let uv = (local_pos / max(half_size * 2.0, vec2<f32>(1.0, 1.0))) + vec2<f32>(0.5, 0.5);
        if (fill_mode > 0.5 && fill_mode < 1.5) {
            let dir = vec2<f32>(cos(in.params0.z), sin(in.params0.z));
            let t = clamp(dot(uv - vec2<f32>(0.5, 0.5), dir) + 0.5, 0.0, 1.0);
            fill_col = mix(in.color_start, in.color_end, t);
        } else if (fill_mode >= 1.5) {
            let t = clamp(length((uv - vec2<f32>(0.5, 0.5)) * 2.0), 0.0, 1.0);
            fill_col = mix(in.color_start, in.color_end, t);
        }
        let fa = fill_col.a * shape_alpha * opacity;
        out_col = vec4<f32>(fill_col.rgb * fa, fa) + out_col * (1.0 - fa);
    }

    // 3. Crisp anti-aliased border
    let bw = in.params0.x;
    if (bw > 0.0 && border_color.a > 0.001) {
        let border_alpha = clamp(0.5 - d, 0.0, 1.0) * clamp(d + bw + 0.5, 0.0, 1.0);
        let ba = border_color.a * border_alpha * opacity;
        out_col = vec4<f32>(border_color.rgb * ba, ba) + out_col * (1.0 - ba);
    }

    if (out_col.a <= 0.001) {
        discard;
    }
    return out_col;
}
"#;

const TEXT_WGSL: &str = r#"
struct ScreenUniform {
    viewport: vec2<f32>,
    _pad: vec2<f32>,
};
@group(0) @binding(0) var<uniform> screen: ScreenUniform;
@group(1) @binding(0) var atlas_tex: texture_2d<f32>;
@group(1) @binding(1) var atlas_samp: sampler;

struct GlyphIn {
    @location(0) rect: vec4<f32>,
    @location(1) uv_rect: vec4<f32>,
    @location(2) color: vec4<f32>,
};

struct VsOut {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vid: u32, g: GlyphIn) -> VsOut {
    var quad = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0),
        vec2<f32>(1.0, 0.0),
        vec2<f32>(0.0, 1.0),
        vec2<f32>(0.0, 1.0),
        vec2<f32>(1.0, 0.0),
        vec2<f32>(1.0, 1.0),
    );
    let q = quad[vid];
    let px = g.rect.xy + q * g.rect.zw;
    let ndc = vec2<f32>(
        (px.x / screen.viewport.x) * 2.0 - 1.0,
        1.0 - (px.y / screen.viewport.y) * 2.0,
    );
    var out: VsOut;
    out.clip_pos = vec4<f32>(ndc, 0.0, 1.0);
    out.uv = mix(g.uv_rect.xy, g.uv_rect.zw, q);
    out.color = g.color;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let cov = textureSample(atlas_tex, atlas_samp, in.uv).r;
    let a = cov * in.color.a;
    if (a <= 0.004) {
        discard;
    }
    return vec4<f32>(in.color.rgb * a, a);
}
"#;

const BLIT_WGSL: &str = r#"
@group(0) @binding(0) var src_tex: texture_2d<f32>;
@group(0) @binding(1) var src_samp: sampler;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vid: u32) -> VsOut {
    var positions = array<vec2<f32>, 6>(
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, -1.0), vec2<f32>(-1.0, 1.0),
        vec2<f32>(-1.0,  1.0), vec2<f32>(1.0, -1.0), vec2<f32>( 1.0, 1.0),
    );
    var uvs = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 1.0), vec2<f32>(1.0, 1.0), vec2<f32>(0.0, 0.0),
        vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 1.0), vec2<f32>(1.0, 0.0),
    );
    var out: VsOut;
    out.pos = vec4<f32>(positions[vid], 0.0, 1.0);
    out.uv = uvs[vid];
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    return textureSample(src_tex, src_samp, in.uv);
}
"#;

const KAWASE_BLUR_WGSL: &str = r#"
struct BlurUniform {
    texel_size: vec2<f32>,
    offset: f32,
    _pad: f32,
};
@group(0) @binding(0) var src_tex: texture_2d<f32>;
@group(0) @binding(1) var src_samp: sampler;
@group(0) @binding(2) var<uniform> bu: BlurUniform;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vid: u32) -> VsOut {
    var positions = array<vec2<f32>, 6>(
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, -1.0), vec2<f32>(-1.0, 1.0),
        vec2<f32>(-1.0,  1.0), vec2<f32>(1.0, -1.0), vec2<f32>( 1.0, 1.0),
    );
    var uvs = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 1.0), vec2<f32>(1.0, 1.0), vec2<f32>(0.0, 0.0),
        vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 1.0), vec2<f32>(1.0, 0.0),
    );
    var out: VsOut;
    out.pos = vec4<f32>(positions[vid], 0.0, 1.0);
    out.uv = uvs[vid];
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let o = bu.texel_size * bu.offset;
    var sum = textureSample(src_tex, src_samp, in.uv) * 4.0;
    sum += textureSample(src_tex, src_samp, in.uv + vec2<f32>(-o.x, -o.y));
    sum += textureSample(src_tex, src_samp, in.uv + vec2<f32>( o.x, -o.y));
    sum += textureSample(src_tex, src_samp, in.uv + vec2<f32>(-o.x,  o.y));
    sum += textureSample(src_tex, src_samp, in.uv + vec2<f32>( o.x,  o.y));
    return sum / 8.0;
}
"#;

const GLASS_COMPOSITE_WGSL: &str = r#"
struct GlassUniform {
    viewport: vec2<f32>,
    opacity: f32,
    use_local_uv: f32,
    rect: vec4<f32>,
    radii: vec4<f32>,
};
@group(0) @binding(0) var blur_tex: texture_2d<f32>;
@group(0) @binding(1) var blur_samp: sampler;
@group(0) @binding(2) var<uniform> gu: GlassUniform;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) local_pos: vec2<f32>,
    @location(1) sample_uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vid: u32) -> VsOut {
    var quad = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0), vec2<f32>(0.0, 1.0),
        vec2<f32>(0.0, 1.0), vec2<f32>(1.0, 0.0), vec2<f32>(1.0, 1.0),
    );
    let q = quad[vid];
    let px = gu.rect.xy + q * gu.rect.zw;
    let ndc = vec2<f32>(
        (px.x / gu.viewport.x) * 2.0 - 1.0,
        1.0 - (px.y / gu.viewport.y) * 2.0,
    );
    var out: VsOut;
    out.pos = vec4<f32>(ndc, 0.0, 1.0);
    out.local_pos = (q - vec2<f32>(0.5, 0.5)) * gu.rect.zw;
    out.sample_uv = select(px / gu.viewport, q, gu.use_local_uv > 0.5);
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let half = gu.rect.zw * 0.5;
    let p = in.local_pos;
    var r = gu.radii.x;
    if (p.x > 0.0) {
        r = select(gu.radii.y, gu.radii.z, p.y > 0.0);
    } else {
        r = select(gu.radii.x, gu.radii.w, p.y > 0.0);
    }
    let rc = clamp(r, 0.0, min(half.x, half.y));
    let q = abs(p) - half + vec2<f32>(rc, rc);
    let d = length(max(q, vec2<f32>(0.0, 0.0))) + min(max(q.x, q.y), 0.0) - rc;
    let mask = clamp(0.5 - d, 0.0, 1.0) * gu.opacity;
    if (mask <= 0.002) {
        discard;
    }
    let col = textureSample(blur_tex, blur_samp, in.sample_uv);
    let a = col.a * mask;
    return vec4<f32>(col.rgb * a, a);
}
"#;

// ============================================================================
// GPU Glyph Atlas (2048x2048 R8Unorm shelf-packed)
// ============================================================================

#[derive(Clone, Copy)]
struct AtlasEntry {
    u0: f32,
    v0: f32,
    u1: f32,
    v1: f32,
    w: f32,
    h: f32,
    off_x: f32,
    off_y: f32,
}

struct GlyphAtlas {
    size: u32,
    texture: wgpu::Texture,
    bind_group: wgpu::BindGroup,
    cursor_x: u32,
    cursor_y: u32,
    row_h: u32,
    cache: HashMap<(char, u32), AtlasEntry>,
}

impl GlyphAtlas {
    fn new(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        sampler: &wgpu::Sampler,
    ) -> Self {
        let size = 2048u32;
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("knr_glyph_atlas"),
            size: wgpu::Extent3d {
                width: size,
                height: size,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("knr_glyph_atlas_bg"),
            layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(sampler),
                },
            ],
        });
        Self {
            size,
            texture,
            bind_group,
            cursor_x: 1,
            cursor_y: 1,
            row_h: 0,
            cache: HashMap::new(),
        }
    }

    fn get_or_upload(
        &mut self,
        queue: &wgpu::Queue,
        font: Option<&FontVec>,
        ch: char,
        px_size: u32,
    ) -> Option<AtlasEntry> {
        let key = (ch, px_size);
        if let Some(&entry) = self.cache.get(&key) {
            return Some(entry);
        }

        let (gw, gh, off_x, off_y, pixels) = if let Some(f) = font {
            let scale = PxScale::from(px_size as f32);
            let scaled = f.as_scaled(scale);
            let gid = scaled.glyph_id(ch);
            let glyph = gid.with_scale_and_position(scale, ab_glyph::point(0.0, scaled.ascent()));
            let outlined = f.outline_glyph(glyph)?;
            let bounds = outlined.px_bounds();
            let w = (bounds.width().ceil() as u32).max(1);
            let h = (bounds.height().ceil() as u32).max(1);
            let mut buf = vec![0u8; (w * h) as usize];
            outlined.draw(|gx, gy, cov| {
                if gx < w && gy < h {
                    buf[(gy * w + gx) as usize] = (cov.clamp(0.0, 1.0) * 255.0).round() as u8;
                }
            });
            (w, h, bounds.min.x, bounds.min.y, buf)
        } else {
            let s = ((px_size as f32 / 8.0).round() as u32).max(1);
            let w = 8 * s;
            let h = 8 * s;
            let mut buf = vec![0u8; (w * h) as usize];
            let g = get_glyph(ch);
            for (row, &bits) in g.iter().enumerate() {
                for col in 0..8u32 {
                    if (bits & (1 << (7 - col))) != 0 {
                        for sy in 0..s {
                            for sx in 0..s {
                                let px = col * s + sx;
                                let py = (row as u32) * s + sy;
                                buf[(py * w + px) as usize] = 255;
                            }
                        }
                    }
                }
            }
            (w, h, 0.0, 0.0, buf)
        };

        if self.cursor_x + gw + 2 >= self.size {
            self.cursor_x = 1;
            self.cursor_y += self.row_h + 2;
            self.row_h = 0;
        }
        if self.cursor_y + gh + 2 >= self.size {
            self.cache.clear();
            self.cursor_x = 1;
            self.cursor_y = 1;
            self.row_h = 0;
        }

        let ax = self.cursor_x;
        let ay = self.cursor_y;
        self.cursor_x += gw + 2;
        self.row_h = self.row_h.max(gh);

        queue.write_texture(
            wgpu::ImageCopyTexture {
                texture: &self.texture,
                mip_level: 0,
                origin: wgpu::Origin3d { x: ax, y: ay, z: 0 },
                aspect: wgpu::TextureAspect::All,
            },
            &pixels,
            wgpu::ImageDataLayout {
                offset: 0,
                bytes_per_row: Some(gw),
                rows_per_image: Some(gh),
            },
            wgpu::Extent3d {
                width: gw,
                height: gh,
                depth_or_array_layers: 1,
            },
        );

        let inv = 1.0 / self.size as f32;
        let entry = AtlasEntry {
            u0: ax as f32 * inv,
            v0: ay as f32 * inv,
            u1: (ax + gw) as f32 * inv,
            v1: (ay + gh) as f32 * inv,
            w: gw as f32,
            h: gh as f32,
            off_x,
            off_y,
        };
        self.cache.insert(key, entry);
        Some(entry)
    }
}

// ============================================================================
// Ordered Frame Draw Batching (single upload per frame, zero transient allocs)
// ============================================================================

enum DrawCmd {
    Quads(Range<usize>),
    Glyphs(Range<usize>),
    BackdropBlur {
        blur_slot_1: u32,
        blur_slot_2: u32,
        glass_slot: u32,
    },
    CustomShader {
        name: String,
        rect: [f32; 4],
        shader_slot: u32,
    },
    Image {
        path: String,
        glass_slot: u32,
    },
}

struct FrameStaging {
    quads: Vec<QuadInstance>,
    glyphs: Vec<GlyphInstance>,
    blurs: Vec<BlurUniform>,
    glasses: Vec<GlassCompositeUniform>,
    shaders: Vec<KnrShaderUniforms>,
    cmds: Vec<DrawCmd>,
}

impl FrameStaging {
    fn new() -> Self {
        Self {
            quads: Vec::with_capacity(256),
            glyphs: Vec::with_capacity(512),
            blurs: Vec::with_capacity(16),
            glasses: Vec::with_capacity(16),
            shaders: Vec::with_capacity(8),
            cmds: Vec::with_capacity(64),
        }
    }

    fn push_quad(&mut self, q: QuadInstance) {
        let idx = self.quads.len();
        self.quads.push(q);
        if let Some(DrawCmd::Quads(r)) = self.cmds.last_mut() {
            if r.end == idx {
                r.end = idx + 1;
                return;
            }
        }
        self.cmds.push(DrawCmd::Quads(idx..idx + 1));
    }

    fn push_glyphs(&mut self, list: Vec<GlyphInstance>) {
        if list.is_empty() {
            return;
        }
        let start = self.glyphs.len();
        self.glyphs.extend(list);
        let end = self.glyphs.len();
        if let Some(DrawCmd::Glyphs(r)) = self.cmds.last_mut() {
            if r.end == start {
                r.end = end;
                return;
            }
        }
        self.cmds.push(DrawCmd::Glyphs(start..end));
    }
}

// ============================================================================
// GpuUiEngine (Persistent Buffers + Strict VSync + Zero Per-Frame GPU Allocs)
// ============================================================================

struct OffscreenTargets {
    width: u32,
    height: u32,
    _scene_tex: wgpu::Texture,
    scene_view: wgpu::TextureView,
    scene_blit_bg: wgpu::BindGroup,
    _blur_a_tex: wgpu::Texture,
    blur_a_view: wgpu::TextureView,
    _blur_b_tex: wgpu::Texture,
    blur_b_view: wgpu::TextureView,
    blur_scene_to_a_bg: wgpu::BindGroup,
    blur_a_to_b_bg: wgpu::BindGroup,
    glass_blur_bg: wgpu::BindGroup,
}

pub struct GpuUiEngine {
    // Drop order (top -> bottom): resources & offscreen targets -> surface -> queue -> device
    targets: OffscreenTargets,
    glyph_atlas: GlyphAtlas,
    image_cache: HashMap<String, Option<wgpu::BindGroup>>,
    custom_shaders: HashMap<String, wgpu::RenderPipeline>,
    quad_pipeline: wgpu::RenderPipeline,
    text_pipeline: wgpu::RenderPipeline,
    blit_bgl: wgpu::BindGroupLayout,
    blit_pipeline: wgpu::RenderPipeline,
    blur_bgl: wgpu::BindGroupLayout,
    blur_pipeline: wgpu::RenderPipeline,
    glass_bgl: wgpu::BindGroupLayout,
    glass_pipeline: wgpu::RenderPipeline,
    shader_bgl: wgpu::BindGroupLayout,
    screen_bg: wgpu::BindGroup,
    screen_buf: wgpu::Buffer,
    shader_bg: wgpu::BindGroup,
    quad_vbuf: wgpu::Buffer,
    quad_vbuf_cap: usize,
    glyph_vbuf: wgpu::Buffer,
    glyph_vbuf_cap: usize,
    blur_ubuf: wgpu::Buffer,
    glass_ubuf: wgpu::Buffer,
    shader_ubuf: wgpu::Buffer,
    linear_sampler: wgpu::Sampler,
    config: wgpu::SurfaceConfiguration,
    scene_format: wgpu::TextureFormat,
    max_tex_dim: u32,
    uniform_align: u32,
    surface: wgpu::Surface<'static>,
    queue: wgpu::Queue,
    device: wgpu::Device,
}

impl GpuUiEngine {
    /// Attempts to initialize a hardware `wgpu 23.0` surface & pipelines for `window`.
    /// Returns `None` cleanly if no GPU adapter/device is available (triggering CPU fallback).
    pub fn try_new(
        window: Arc<Window>,
        width: u32,
        height: u32,
        shaders_wgsl: &HashMap<String, String>,
    ) -> Option<Self> {
        let instance = wgpu::Instance::default();
        let surface = instance.create_surface(window).ok()?;
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
        }))?;

        let (device, queue) = pollster::block_on(adapter.request_device(
            &wgpu::DeviceDescriptor {
                label: Some("knr_gpu_device"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::downlevel_defaults()
                    .using_resolution(adapter.limits()),
                memory_hints: wgpu::MemoryHints::MemoryUsage,
            },
            None,
        ))
        .ok()?;

        // Catch any driver/validation errors cleanly so try_new falls back to CPU instead of panicking!
        device.push_error_scope(wgpu::ErrorFilter::Validation);

        let limits = device.limits();
        let max_tex_dim = limits.max_texture_dimension_2d.clamp(256, 8192);
        let uniform_align = limits.min_uniform_buffer_offset_alignment.max(256);

        let caps = surface.get_capabilities(&adapter);
        let surface_format = caps
            .formats
            .iter()
            .copied()
            .find(|f| !f.is_srgb())
            .or_else(|| caps.formats.first().copied())?;

        let w = width.clamp(1, max_tex_dim);
        let h = height.clamp(1, max_tex_dim);
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format: surface_format,
            width: w,
            height: h,
            // Strict VSync FIFO + latency 1 prevents Vulkan/Wayland frame queue flooding
            present_mode: wgpu::PresentMode::Fifo,
            alpha_mode: caps
                .alpha_modes
                .first()
                .copied()
                .unwrap_or(wgpu::CompositeAlphaMode::Opaque),
            view_formats: vec![],
            desired_maximum_frame_latency: 1,
        };
        surface.configure(&device, &config);

        let scene_format = wgpu::TextureFormat::Rgba8Unorm;

        let linear_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("knr_linear_sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Nearest,
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            ..Default::default()
        });

        let screen_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("knr_screen_bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        let screen_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("knr_screen_uniform"),
            size: std::mem::size_of::<ScreenUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let screen_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("knr_screen_bg"),
            layout: &screen_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: screen_buf.as_entire_binding(),
            }],
        });

        // Persistent vertex buffers
        let quad_vbuf_cap = 512usize;
        let quad_vbuf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("knr_quad_vbuf_persistent"),
            size: (quad_vbuf_cap * std::mem::size_of::<QuadInstance>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let glyph_vbuf_cap = 2048usize;
        let glyph_vbuf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("knr_glyph_vbuf_persistent"),
            size: (glyph_vbuf_cap * std::mem::size_of::<GlyphInstance>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // Persistent dynamic-offset uniform buffers
        let blur_ubuf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("knr_blur_ubuf_persistent"),
            size: (MAX_BLUR_SLOTS as u64) * (uniform_align as u64),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let glass_ubuf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("knr_glass_ubuf_persistent"),
            size: (MAX_GLASS_SLOTS as u64) * (uniform_align as u64),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let shader_ubuf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("knr_shader_ubuf_persistent"),
            size: (MAX_SHADER_SLOTS as u64) * (uniform_align as u64),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let premul_blend = wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING;

        // 1. SDF Quad Pipeline
        let quad_sm = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("knr_sdf_quad_shader"),
            source: wgpu::ShaderSource::Wgsl(SDF_QUAD_WGSL.into()),
        });
        let quad_pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("knr_quad_pl"),
            bind_group_layouts: &[&screen_bgl],
            push_constant_ranges: &[],
        });
        let quad_attrs = [
            wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x4, offset: 0, shader_location: 0 },
            wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x4, offset: 16, shader_location: 1 },
            wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x4, offset: 32, shader_location: 2 },
            wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x4, offset: 48, shader_location: 3 },
            wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x4, offset: 64, shader_location: 4 },
            wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x4, offset: 80, shader_location: 5 },
            wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x4, offset: 96, shader_location: 6 },
            wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x4, offset: 112, shader_location: 7 },
            wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x4, offset: 128, shader_location: 8 },
        ];
        let quad_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("knr_quad_pipeline"),
            layout: Some(&quad_pl),
            vertex: wgpu::VertexState {
                module: &quad_sm,
                entry_point: Some("vs_main"),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<QuadInstance>() as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &quad_attrs,
                }],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &quad_sm,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: scene_format,
                    blend: Some(premul_blend),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        // 2. Text / Glyph Atlas Pipeline
        let tex_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("knr_tex_bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let text_sm = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("knr_text_shader"),
            source: wgpu::ShaderSource::Wgsl(TEXT_WGSL.into()),
        });
        let text_pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("knr_text_pl"),
            bind_group_layouts: &[&screen_bgl, &tex_bgl],
            push_constant_ranges: &[],
        });
        let glyph_attrs = [
            wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x4, offset: 0, shader_location: 0 },
            wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x4, offset: 16, shader_location: 1 },
            wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x4, offset: 32, shader_location: 2 },
        ];
        let text_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("knr_text_pipeline"),
            layout: Some(&text_pl),
            vertex: wgpu::VertexState {
                module: &text_sm,
                entry_point: Some("vs_main"),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<GlyphInstance>() as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &glyph_attrs,
                }],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &text_sm,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: scene_format,
                    blend: Some(premul_blend),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        // 3. Blit Pipeline (scene_tex -> swapchain)
        let blit_sm = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("knr_blit_shader"),
            source: wgpu::ShaderSource::Wgsl(BLIT_WGSL.into()),
        });
        let blit_pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("knr_blit_pl"),
            bind_group_layouts: &[&tex_bgl],
            push_constant_ranges: &[],
        });
        let blit_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("knr_blit_pipeline"),
            layout: Some(&blit_pl),
            vertex: wgpu::VertexState {
                module: &blit_sm,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &blit_sm,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: surface_format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        // 4. Dual Kawase Blur + Glass Composite Pipelines (dynamic-offset uniforms!)
        let blur_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("knr_blur_bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: true,
                        min_binding_size: NonZeroU64::new(std::mem::size_of::<BlurUniform>() as u64),
                    },
                    count: None,
                },
            ],
        });
        let blur_sm = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("knr_kawase_blur_shader"),
            source: wgpu::ShaderSource::Wgsl(KAWASE_BLUR_WGSL.into()),
        });
        let blur_pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("knr_blur_pl"),
            bind_group_layouts: &[&blur_bgl],
            push_constant_ranges: &[],
        });
        let blur_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("knr_blur_pipeline"),
            layout: Some(&blur_pl),
            vertex: wgpu::VertexState {
                module: &blur_sm,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &blur_sm,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: scene_format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        let glass_sm = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("knr_glass_shader"),
            source: wgpu::ShaderSource::Wgsl(GLASS_COMPOSITE_WGSL.into()),
        });
        let glass_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("knr_glass_bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: true,
                        min_binding_size: NonZeroU64::new(
                            std::mem::size_of::<GlassCompositeUniform>() as u64,
                        ),
                    },
                    count: None,
                },
            ],
        });
        let glass_pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("knr_glass_pl"),
            bind_group_layouts: &[&glass_bgl],
            push_constant_ranges: &[],
        });
        let glass_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("knr_glass_pipeline"),
            layout: Some(&glass_pl),
            vertex: wgpu::VertexState {
                module: &glass_sm,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &glass_sm,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: scene_format,
                    blend: Some(premul_blend),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        // 5. Custom `.knr` Shader Bind Group Layout (`KnrUniforms` with dynamic offset)
        let shader_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("knr_custom_shader_bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: true,
                    min_binding_size: NonZeroU64::new(
                        std::mem::size_of::<KnrShaderUniforms>() as u64,
                    ),
                },
                count: None,
            }],
        });
        let shader_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("knr_custom_shader_bg_persistent"),
            layout: &shader_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &shader_ubuf,
                    offset: 0,
                    size: NonZeroU64::new(std::mem::size_of::<KnrShaderUniforms>() as u64),
                }),
            }],
        });

        let glyph_atlas = GlyphAtlas::new(&device, &tex_bgl, &linear_sampler);
        let targets = create_offscreen_targets(
            &device,
            w,
            h,
            scene_format,
            &tex_bgl,
            &blur_bgl,
            &glass_bgl,
            &blur_ubuf,
            &glass_ubuf,
            &linear_sampler,
        );

        let mut engine = Self {
            surface,
            device,
            queue,
            config,
            scene_format,
            max_tex_dim,
            uniform_align,
            linear_sampler,
            screen_buf,
            screen_bg,
            quad_vbuf,
            quad_vbuf_cap,
            glyph_vbuf,
            glyph_vbuf_cap,
            blur_ubuf,
            glass_ubuf,
            shader_ubuf,
            shader_bg,
            quad_pipeline,
            text_pipeline,
            blit_bgl: tex_bgl,
            blit_pipeline,
            blur_bgl,
            blur_pipeline,
            glass_bgl,
            glass_pipeline,
            shader_bgl,
            custom_shaders: HashMap::new(),
            glyph_atlas,
            image_cache: HashMap::new(),
            targets,
        };

        for (name, wgsl) in shaders_wgsl {
            engine.register_shader(name, wgsl);
        }

        if let Some(err) = pollster::block_on(engine.device.pop_error_scope()) {
            eprintln!("[knr-gpu] Falling back to CPU renderer due to GPU validation error: {err}");
            return None;
        }

        Some(engine)
    }

    /// Drains all in-flight GPU work cleanly before tearing down the swapchain/surface on window close.
    pub fn shutdown(&self) {
        self.device.poll(wgpu::Maintain::Wait);
    }

    /// Registers & compiles a `.knr`-generated WGSL shader into a live GPU `RenderPipeline`.
    pub fn register_shader(&mut self, name: &str, wgsl_src: &str) {
        let sm = self
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some(name),
                source: wgpu::ShaderSource::Wgsl(wgsl_src.into()),
            });
        let pl = self
            .device
            .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("knr_custom_shader_pl"),
                bind_group_layouts: &[&self.shader_bgl],
                push_constant_ranges: &[],
            });
        let pipe = self
            .device
            .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(name),
                layout: Some(&pl),
                vertex: wgpu::VertexState {
                    module: &sm,
                    entry_point: Some("vs_main"),
                    buffers: &[],
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &sm,
                    entry_point: Some("fs_main"),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: self.scene_format,
                        blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: Default::default(),
                }),
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview: None,
                cache: None,
            });
        self.custom_shaders.insert(name.to_string(), pipe);
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        let w = width.clamp(1, self.max_tex_dim);
        let h = height.clamp(1, self.max_tex_dim);
        if self.config.width == w && self.config.height == h {
            return;
        }
        self.config.width = w;
        self.config.height = h;
        self.surface.configure(&self.device, &self.config);
        self.targets = create_offscreen_targets(
            &self.device,
            w,
            h,
            self.scene_format,
            &self.blit_bgl,
            &self.blur_bgl,
            &self.glass_bgl,
            &self.blur_ubuf,
            &self.glass_ubuf,
            &self.linear_sampler,
        );
    }

    /// Renders a laid-out UI tree directly on the GPU and presents it to the window swapchain.
    /// Layout coordinates are in logical pixels (`logical_size`), projected crisply onto the
    /// physical HiDPI framebuffer (`self.config.width x self.config.height`).
    #[allow(clippy::too_many_arguments)]
    pub fn render_frame(
        &mut self,
        layout_root: &LayoutNode,
        measurer: &UiRenderer,
        state: &RenderInteractionState<'_>,
        cursor_pos: (f32, f32),
        dt_sec: f32,
        logical_size: (f32, f32),
        scale_factor: f32,
    ) -> Result<(), String> {
        let w = self.config.width.max(1);
        let h = self.config.height.max(1);
        let lw = logical_size.0.max(1.0);
        let lh = logical_size.1.max(1.0);
        let scale = scale_factor.clamp(0.5, 4.0);

        let screen = ScreenUniform {
            viewport: [lw, lh],
            _pad: [0.0, 0.0],
        };
        self.queue
            .write_buffer(&self.screen_buf, 0, bytemuck::bytes_of(&screen));

        // 1. Collect all draw calls & upload vertex/uniform buffers ONCE before recording passes
        let mut staging = FrameStaging::new();
        self.collect_draw_cmds(
            layout_root,
            measurer,
            state,
            cursor_pos,
            dt_sec,
            w,
            h,
            lw,
            lh,
            scale,
            1.0,
            &mut staging,
        );

        // Grow persistent vertex buffers only when capacity is exceeded (zero alloc in steady state)
        if staging.quads.len() > self.quad_vbuf_cap {
            self.quad_vbuf_cap = staging.quads.len().next_power_of_two();
            self.quad_vbuf = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("knr_quad_vbuf_persistent"),
                size: (self.quad_vbuf_cap * std::mem::size_of::<QuadInstance>()) as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
        if !staging.quads.is_empty() {
            self.queue
                .write_buffer(&self.quad_vbuf, 0, bytemuck::cast_slice(&staging.quads));
        }

        if staging.glyphs.len() > self.glyph_vbuf_cap {
            self.glyph_vbuf_cap = staging.glyphs.len().next_power_of_two();
            self.glyph_vbuf = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("knr_glyph_vbuf_persistent"),
                size: (self.glyph_vbuf_cap * std::mem::size_of::<GlyphInstance>()) as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
        if !staging.glyphs.is_empty() {
            self.queue
                .write_buffer(&self.glyph_vbuf, 0, bytemuck::cast_slice(&staging.glyphs));
        }

        let align = self.uniform_align as u64;
        for (i, bu) in staging.blurs.iter().take(MAX_BLUR_SLOTS).enumerate() {
            self.queue
                .write_buffer(&self.blur_ubuf, (i as u64) * align, bytemuck::bytes_of(bu));
        }
        for (i, gu) in staging.glasses.iter().take(MAX_GLASS_SLOTS).enumerate() {
            self.queue
                .write_buffer(&self.glass_ubuf, (i as u64) * align, bytemuck::bytes_of(gu));
        }
        for (i, ku) in staging.shaders.iter().take(MAX_SHADER_SLOTS).enumerate() {
            self.queue
                .write_buffer(&self.shader_ubuf, (i as u64) * align, bytemuck::bytes_of(ku));
        }

        let frame = match self.surface.get_current_texture() {
            Ok(f) => f,
            Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
                self.surface.configure(&self.device, &self.config);
                return Ok(());
            }
            Err(wgpu::SurfaceError::Timeout) => return Ok(()),
            Err(e) => return Err(format!("wgpu surface error: {e}")),
        };
        let frame_view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("knr_gpu_frame"),
            });

        // Clear offscreen scene texture to root background color or #0b1120
        let (br, bg, bb, ba) = parse_hex_color(&layout_root.node.bg).unwrap_or((11, 17, 32, 255));
        {
            let _clear_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("knr_clear_scene"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.targets.scene_view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: br as f64 / 255.0,
                            g: bg as f64 / 255.0,
                            b: bb as f64 / 255.0,
                            a: ba as f64 / 255.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                occlusion_query_set: None,
                timestamp_writes: None,
            });
        }

        // Execute ordered GPU draw commands using ONLY persistent buffers & pre-built bind groups
        let quad_stride = std::mem::size_of::<QuadInstance>() as u64;
        let glyph_stride = std::mem::size_of::<GlyphInstance>() as u64;

        for cmd in staging.cmds {
            match cmd {
                DrawCmd::Quads(range) if !range.is_empty() => {
                    let count = (range.end - range.start) as u32;
                    let byte_start = (range.start as u64) * quad_stride;
                    let byte_end = (range.end as u64) * quad_stride;
                    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("knr_quad_pass"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: &self.targets.scene_view,
                            resolve_target: None,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Load,
                                store: wgpu::StoreOp::Store,
                            },
                        })],
                        depth_stencil_attachment: None,
                        occlusion_query_set: None,
                        timestamp_writes: None,
                    });
                    pass.set_pipeline(&self.quad_pipeline);
                    pass.set_bind_group(0, &self.screen_bg, &[]);
                    pass.set_vertex_buffer(0, self.quad_vbuf.slice(byte_start..byte_end));
                    pass.draw(0..6, 0..count);
                }
                DrawCmd::Glyphs(range) if !range.is_empty() => {
                    let count = (range.end - range.start) as u32;
                    let byte_start = (range.start as u64) * glyph_stride;
                    let byte_end = (range.end as u64) * glyph_stride;
                    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("knr_text_pass"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: &self.targets.scene_view,
                            resolve_target: None,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Load,
                                store: wgpu::StoreOp::Store,
                            },
                        })],
                        depth_stencil_attachment: None,
                        occlusion_query_set: None,
                        timestamp_writes: None,
                    });
                    pass.set_pipeline(&self.text_pipeline);
                    pass.set_bind_group(0, &self.screen_bg, &[]);
                    pass.set_bind_group(1, &self.glyph_atlas.bind_group, &[]);
                    pass.set_vertex_buffer(0, self.glyph_vbuf.slice(byte_start..byte_end));
                    pass.draw(0..6, 0..count);
                }
                DrawCmd::BackdropBlur {
                    blur_slot_1,
                    blur_slot_2,
                    glass_slot,
                } => {
                    let off1 = blur_slot_1 * self.uniform_align;
                    let off2 = blur_slot_2 * self.uniform_align;
                    let goff = glass_slot * self.uniform_align;

                    // Downsample scene -> blur_a
                    {
                        let mut p = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                            label: Some("knr_kawase_1"),
                            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                                view: &self.targets.blur_a_view,
                                resolve_target: None,
                                ops: wgpu::Operations {
                                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                                    store: wgpu::StoreOp::Store,
                                },
                            })],
                            depth_stencil_attachment: None,
                            occlusion_query_set: None,
                            timestamp_writes: None,
                        });
                        p.set_pipeline(&self.blur_pipeline);
                        p.set_bind_group(0, &self.targets.blur_scene_to_a_bg, &[off1]);
                        p.draw(0..6, 0..1);
                    }

                    // Second Kawase pass: blur_a -> blur_b
                    {
                        let mut p = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                            label: Some("knr_kawase_2"),
                            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                                view: &self.targets.blur_b_view,
                                resolve_target: None,
                                ops: wgpu::Operations {
                                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                                    store: wgpu::StoreOp::Store,
                                },
                            })],
                            depth_stencil_attachment: None,
                            occlusion_query_set: None,
                            timestamp_writes: None,
                        });
                        p.set_pipeline(&self.blur_pipeline);
                        p.set_bind_group(0, &self.targets.blur_a_to_b_bg, &[off2]);
                        p.draw(0..6, 0..1);
                    }

                    // Composite blurred backdrop clipped to widget rounded rect into scene_view
                    {
                        let mut p = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                            label: Some("knr_glass_composite"),
                            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                                view: &self.targets.scene_view,
                                resolve_target: None,
                                ops: wgpu::Operations {
                                    load: wgpu::LoadOp::Load,
                                    store: wgpu::StoreOp::Store,
                                },
                            })],
                            depth_stencil_attachment: None,
                            occlusion_query_set: None,
                            timestamp_writes: None,
                        });
                        p.set_pipeline(&self.glass_pipeline);
                        p.set_bind_group(0, &self.targets.glass_blur_bg, &[goff]);
                        p.draw(0..6, 0..1);
                    }
                }
                DrawCmd::CustomShader {
                    name,
                    rect,
                    shader_slot,
                } => {
                    // Convert logical rect -> physical framebuffer coords with strict bounds guard
                    let x0 = (rect[0] * scale).floor().max(0.0).min(w as f32) as u32;
                    let y0 = (rect[1] * scale).floor().max(0.0).min(h as f32) as u32;
                    let x1 = ((rect[0] + rect[2]) * scale).ceil().max(0.0).min(w as f32) as u32;
                    let y1 = ((rect[1] + rect[3]) * scale).ceil().max(0.0).min(h as f32) as u32;
                    if x1 <= x0 || y1 <= y0 || x0 >= w || y0 >= h {
                        continue;
                    }
                    let sw = x1 - x0;
                    let sh = y1 - y0;

                    if let Some(pipe) = self.custom_shaders.get(&name) {
                        let dyn_off = shader_slot * self.uniform_align;
                        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                            label: Some("knr_custom_shader_pass"),
                            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                                view: &self.targets.scene_view,
                                resolve_target: None,
                                ops: wgpu::Operations {
                                    load: wgpu::LoadOp::Load,
                                    store: wgpu::StoreOp::Store,
                                },
                            })],
                            depth_stencil_attachment: None,
                            occlusion_query_set: None,
                            timestamp_writes: None,
                        });
                        pass.set_viewport(x0 as f32, y0 as f32, sw as f32, sh as f32, 0.0, 1.0);
                        pass.set_scissor_rect(x0, y0, sw, sh);
                        pass.set_pipeline(pipe);
                        pass.set_bind_group(0, &self.shader_bg, &[dyn_off]);
                        pass.draw(0..6, 0..1);
                    }
                }
                DrawCmd::Image { path, glass_slot } => {
                    self.ensure_image_loaded(&path);
                    if let Some(Some(img_bg)) = self.image_cache.get(&path) {
                        let goff = glass_slot * self.uniform_align;
                        let mut p = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                            label: Some("knr_img_pass"),
                            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                                view: &self.targets.scene_view,
                                resolve_target: None,
                                ops: wgpu::Operations {
                                    load: wgpu::LoadOp::Load,
                                    store: wgpu::StoreOp::Store,
                                },
                            })],
                            depth_stencil_attachment: None,
                            occlusion_query_set: None,
                            timestamp_writes: None,
                        });
                        p.set_pipeline(&self.glass_pipeline);
                        p.set_bind_group(0, img_bg, &[goff]);
                        p.draw(0..6, 0..1);
                    }
                }
                _ => {}
            }
        }

        // Final blit: scene_view -> swapchain surface
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("knr_present_blit"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &frame_view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                occlusion_query_set: None,
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.blit_pipeline);
            pass.set_bind_group(0, &self.targets.scene_blit_bg, &[]);
            pass.draw(0..6, 0..1);
        }

        self.queue.submit(Some(encoder.finish()));
        frame.present();
        self.device.poll(wgpu::Maintain::Poll);
        Ok(())
    }

    fn ensure_image_loaded(&mut self, path: &str) {
        if self.image_cache.contains_key(path) {
            return;
        }
        let loaded = image::open(path).ok().map(|img| {
            let rgba = img.to_rgba8();
            let (iw, ih) = rgba.dimensions();
            let tex = self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some(path),
                size: wgpu::Extent3d {
                    width: iw.max(1),
                    height: ih.max(1),
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8UnormSrgb,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            self.queue.write_texture(
                wgpu::ImageCopyTexture {
                    texture: &tex,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                &rgba,
                wgpu::ImageDataLayout {
                    offset: 0,
                    bytes_per_row: Some(iw * 4),
                    rows_per_image: Some(ih),
                },
                wgpu::Extent3d {
                    width: iw,
                    height: ih,
                    depth_or_array_layers: 1,
                },
            );
            let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
            self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("knr_img_draw_bg_cached"),
                layout: &self.glass_bgl,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&self.linear_sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                            buffer: &self.glass_ubuf,
                            offset: 0,
                            size: NonZeroU64::new(
                                std::mem::size_of::<GlassCompositeUniform>() as u64,
                            ),
                        }),
                    },
                ],
            })
        });
        self.image_cache.insert(path.to_string(), loaded);
    }

    #[allow(clippy::too_many_arguments)]
    fn collect_draw_cmds(
        &mut self,
        laid: &LayoutNode,
        measurer: &UiRenderer,
        state: &RenderInteractionState<'_>,
        cursor_pos: (f32, f32),
        dt_sec: f32,
        w: u32,
        h: u32,
        lw: f32,
        lh: f32,
        scale: f32,
        parent_opacity: f32,
        staging: &mut FrameStaging,
    ) {
        let node = &laid.node;
        let rect = laid.rect;
        let opacity = (parent_opacity * node.style.opacity).clamp(0.0, 1.0);
        if opacity <= 0.004 || rect.w <= 0.0 || rect.h <= 0.0 {
            return;
        }
        // Cull completely off-screen subtrees (huge speedup when scrolled)
        if rect.y + rect.h < -64.0 || rect.y > lh + 64.0 {
            return;
        }

        let is_hovered =
            !node.action.is_empty() && state.hovered_action == Some(node.action.as_str());
        let is_focused =
            !node.action.is_empty() && state.focused_action == Some(node.action.as_str());
        let radii = node.effective_radii();

        // 1. Optional Dual Kawase backdrop_blur behind glass cards
        if node.style.backdrop_blur > 0.0
            && staging.blurs.len() + 2 <= MAX_BLUR_SLOTS
            && staging.glasses.len() < MAX_GLASS_SLOTS
        {
            let bw = (self.targets.width / 2).max(1) as f32;
            let bh = (self.targets.height / 2).max(1) as f32;
            let offset = (node.style.backdrop_blur * 0.2).clamp(1.0, 6.0);

            let blur_slot_1 = staging.blurs.len() as u32;
            staging.blurs.push(BlurUniform {
                texel_size: [1.0 / w as f32, 1.0 / h as f32],
                offset,
                _pad: 0.0,
            });
            let blur_slot_2 = staging.blurs.len() as u32;
            staging.blurs.push(BlurUniform {
                texel_size: [1.0 / bw, 1.0 / bh],
                offset: offset * 1.5,
                _pad: 0.0,
            });
            let glass_slot = staging.glasses.len() as u32;
            staging.glasses.push(GlassCompositeUniform {
                viewport: [lw, lh],
                opacity,
                use_local_uv: 0.0,
                rect: [rect.x, rect.y, rect.w, rect.h],
                radii,
            });
            staging.cmds.push(DrawCmd::BackdropBlur {
                blur_slot_1,
                blur_slot_2,
                glass_slot,
            });
        }

        // 2. Quad background + shadow + border
        let active_bg = if is_hovered && !node.style.hover_bg.is_empty() {
            node.style.hover_bg.as_str()
        } else {
            node.bg.as_str()
        };
        let (fill_mode, mut color_start, color_end, grad_rad) = match parse_fill_spec(active_bg) {
            ParsedFill::Solid(c) => (0.0f32, c, c, 0.0f32),
            ParsedFill::LinearGradient {
                angle_deg,
                start,
                end,
            } => (1.0f32, start, end, (angle_deg - 90.0).to_radians()),
            ParsedFill::RadialGradient { center, edge } => (2.0f32, center, edge, 0.0f32),
            ParsedFill::None => {
                if node.kind == UiKind::Input {
                    (0.0, [0.06, 0.09, 0.16, 0.92], [0.06, 0.09, 0.16, 0.92], 0.0)
                } else {
                    (3.0, [0.0; 4], [0.0; 4], 0.0)
                }
            }
        };
        if fill_mode < 0.5 && is_hovered && node.style.hover_bg.is_empty() {
            color_start[0] = (color_start[0] + 0.08).min(1.0);
            color_start[1] = (color_start[1] + 0.08).min(1.0);
            color_start[2] = (color_start[2] + 0.08).min(1.0);
        }

        let active_border = if is_focused {
            "#38bdf8"
        } else if is_hovered && !node.style.hover_border.is_empty() {
            node.style.hover_border.as_str()
        } else {
            node.border.as_str()
        };
        let (border_color, border_w) = if let Some((r, g, b, a)) = parse_hex_color(active_border) {
            let bw = if is_focused {
                2.0
            } else if is_hovered {
                node.style.border_w.max(1.5)
            } else {
                node.style.border_w.max(1.0)
            };
            (
                [
                    r as f32 / 255.0,
                    g as f32 / 255.0,
                    b as f32 / 255.0,
                    a as f32 / 255.0,
                ],
                bw,
            )
        } else if is_hovered && node.kind == UiKind::Button {
            ([1.0, 1.0, 1.0, 0.38], 1.5)
        } else {
            ([0.0; 4], 0.0)
        };

        let (shadow_color, sx, sy, sblur) = if node.style.shadow_blur > 0.0
            && !node.style.shadow_color.is_empty()
        {
            let sc = parse_hex_color(&node.style.shadow_color)
                .map(|(r, g, b, a)| {
                    [
                        r as f32 / 255.0,
                        g as f32 / 255.0,
                        b as f32 / 255.0,
                        a as f32 / 255.0,
                    ]
                })
                .unwrap_or([0.0, 0.0, 0.0, 0.45]);
            (
                sc,
                node.style.shadow_x,
                node.style.shadow_y,
                node.style.shadow_blur,
            )
        } else if matches!(node.kind, UiKind::Card | UiKind::Button) && fill_mode < 2.5 {
            ([0.0, 0.0, 0.0, 0.28], 0.0, 4.0, 10.0)
        } else {
            ([0.0; 4], 0.0, 0.0, 0.0)
        };

        if fill_mode < 2.5 || border_w > 0.0 || sblur > 0.0 {
            staging.push_quad(QuadInstance {
                rect: [rect.x, rect.y, rect.w, rect.h],
                radii,
                color_start,
                color_end,
                border_color,
                shadow_color,
                params0: [border_w, fill_mode, grad_rad, opacity],
                params1: [sx, sy, sblur, node.style.rotate_deg.to_radians()],
                params2: [
                    node.style.scale,
                    node.style.translate_x,
                    node.style.translate_y,
                    0.0,
                ],
            });
        }

        // 3. Specialized interactive widgets & custom GPU shaders
        match node.kind {
            UiKind::Shader if staging.shaders.len() < MAX_SHADER_SLOTS => {
                let mut u16 = [0.0f32; 16];
                for (i, &v) in node.uniforms.iter().take(16).enumerate() {
                    u16[i] = v as f32;
                }
                let local_mx = ((cursor_pos.0 - rect.x) / rect.w.max(1.0)).clamp(0.0, 1.0);
                let local_my = ((cursor_pos.1 - rect.y) / rect.h.max(1.0)).clamp(0.0, 1.0);
                let shader_slot = staging.shaders.len() as u32;
                staging.shaders.push(KnrShaderUniforms {
                    resolution: [rect.w, rect.h],
                    time: state.elapsed_sec,
                    dt: dt_sec,
                    mouse: [local_mx, local_my, 0.0, 0.0],
                    rect_radius: radii,
                    params: [
                        [u16[0], u16[1], u16[2], u16[3]],
                        [u16[4], u16[5], u16[6], u16[7]],
                        [u16[8], u16[9], u16[10], u16[11]],
                        [u16[12], u16[13], u16[14], u16[15]],
                    ],
                });
                staging.cmds.push(DrawCmd::CustomShader {
                    name: node.text.clone(),
                    rect: [rect.x, rect.y, rect.w, rect.h],
                    shader_slot,
                });
            }
            UiKind::Image if staging.glasses.len() < MAX_GLASS_SLOTS => {
                let glass_slot = staging.glasses.len() as u32;
                staging.glasses.push(GlassCompositeUniform {
                    viewport: [lw, lh],
                    opacity,
                    use_local_uv: 1.0,
                    rect: [rect.x, rect.y, rect.w, rect.h],
                    radii,
                });
                staging.cmds.push(DrawCmd::Image {
                    path: node.text.clone(),
                    glass_slot,
                });
            }
            UiKind::Slider => {
                let t = node.value.clamp(0.0, 1.0) as f32;
                let track_h = 8.0f32;
                let track_y = rect.y + (rect.h - track_h) * 0.5;
                staging.push_quad(simple_quad(
                    [rect.x, track_y, rect.w, track_h],
                    [4.0; 4],
                    [0.12, 0.16, 0.23, 0.95],
                    [0.0; 4],
                    0.0,
                    opacity,
                ));
                let fill_w = (rect.w * t).max(8.0).min(rect.w);
                staging.push_quad(QuadInstance {
                    rect: [rect.x, track_y, fill_w, track_h],
                    radii: [4.0; 4],
                    color_start: [0.22, 0.74, 0.97, 1.0],
                    color_end: [0.51, 0.55, 0.97, 1.0],
                    border_color: [0.0; 4],
                    shadow_color: [0.0; 4],
                    params0: [0.0, 1.0, 0.0, opacity],
                    params1: [0.0; 4],
                    params2: [1.0, 0.0, 0.0, 0.0],
                });
                let tr = 9.0f32;
                let tx = rect.x + (rect.w - tr * 2.0) * t;
                let ty = rect.y + (rect.h - tr * 2.0) * 0.5;
                staging.push_quad(simple_quad(
                    [tx, ty, tr * 2.0, tr * 2.0],
                    [tr; 4],
                    [0.97, 0.98, 0.99, 1.0],
                    [0.22, 0.74, 0.97, 1.0],
                    2.0,
                    opacity,
                ));
            }
            UiKind::Toggle => {
                let checked = node.value > 0.5;
                let pill_w = 42.0f32;
                let pill_h = 24.0f32;
                let px = rect.x + node.effective_pad()[3];
                let py = rect.y + (rect.h - pill_h) * 0.5;
                let col = if checked {
                    [0.22, 0.74, 0.97, 1.0]
                } else {
                    [0.20, 0.25, 0.33, 1.0]
                };
                staging.push_quad(simple_quad(
                    [px, py, pill_w, pill_h],
                    [12.0; 4],
                    col,
                    [0.0; 4],
                    0.0,
                    opacity,
                ));
                let kx = if checked { px + pill_w - 20.0 } else { px + 4.0 };
                staging.push_quad(simple_quad(
                    [kx, py + 4.0, 16.0, 16.0],
                    [8.0; 4],
                    [0.97, 0.98, 0.99, 1.0],
                    [0.0; 4],
                    0.0,
                    opacity,
                ));
            }
            _ => {}
        }

        // 4. Text via GPU GlyphAtlas (uses pre-wrapped multi-line layout so text never overflows!)
        if !laid.wrapped_lines.is_empty()
            && !matches!(node.kind, UiKind::Shader | UiKind::Image | UiKind::Slider)
        {
            let (tr, tg, tb, ta) = parse_hex_color(&node.color).unwrap_or((248, 250, 252, 255));
            let color = [
                tr as f32 / 255.0,
                tg as f32 / 255.0,
                tb as f32 / 255.0,
                (ta as f32 / 255.0) * opacity,
            ];
            let [pt, _pr, _pb, pl] = node.effective_pad();
            let first_line = &laid.wrapped_lines[0];
            let (tw, th) = measurer.measure_text_size(first_line, node.font_size);
            let (tx, ty) = match node.kind {
                UiKind::Button | UiKind::Badge => (
                    rect.x + ((rect.w - tw) * 0.5).max(0.0),
                    rect.y + ((rect.h - th) * 0.5).max(0.0),
                ),
                UiKind::Input => (
                    rect.x + pl.max(12.0),
                    rect.y + ((rect.h - th) * 0.5).max(0.0),
                ),
                UiKind::Toggle => (
                    rect.x + pl + 52.0,
                    rect.y + ((rect.h - th) * 0.5).max(0.0),
                ),
                _ => (rect.x + pl, rect.y + pt),
            };
            let tx_f = tx + node.style.translate_x;
            let ty_f = ty + node.style.translate_y;
            for (line_idx, line) in laid.wrapped_lines.iter().enumerate() {
                if !line.is_empty() {
                    let ly = ty_f + (line_idx as f32) * laid.line_height;
                    self.push_text_glyphs(
                        staging,
                        measurer.font(),
                        line,
                        tx_f,
                        ly,
                        node.font_size,
                        scale,
                        color,
                    );
                }
            }
            if node.kind == UiKind::Input && is_focused {
                staging.push_quad(simple_quad(
                    [tx_f + tw + 2.0, ty_f, 2.0, th.max(14.0)],
                    [1.0; 4],
                    [0.22, 0.74, 0.97, 1.0],
                    [0.0; 4],
                    0.0,
                    opacity,
                ));
            }
        }

        // 5. Children
        for child in &laid.children {
            self.collect_draw_cmds(
                child,
                measurer,
                state,
                cursor_pos,
                dt_sec,
                w,
                h,
                lw,
                lh,
                scale,
                opacity,
                staging,
            );
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn push_text_glyphs(
        &mut self,
        staging: &mut FrameStaging,
        font: Option<&FontVec>,
        text: &str,
        x: f32,
        y: f32,
        font_size: f32,
        scale: f32,
        color: [f32; 4],
    ) {
        let logical_px = font_size.clamp(8.0, 96.0);
        let phys_px = ((logical_px * scale).round() as u32).clamp(8, 128);
        let inv_scale = 1.0 / scale.max(0.5);
        let mut caret_x = x;
        let mut prev_gid = None;
        let scaled = font.map(|f| f.as_scaled(PxScale::from(logical_px)));

        let mut glyphs = Vec::new();
        for ch in text.chars() {
            if let Some(ref sc) = scaled {
                let gid = sc.glyph_id(ch);
                if let Some(prev) = prev_gid {
                    caret_x += sc.kern(prev, gid);
                }
                if let Some(entry) = self.glyph_atlas.get_or_upload(&self.queue, font, ch, phys_px)
                {
                    glyphs.push(GlyphInstance {
                        rect: [
                            caret_x + entry.off_x * inv_scale,
                            y + entry.off_y * inv_scale,
                            entry.w * inv_scale,
                            entry.h * inv_scale,
                        ],
                        uv_rect: [entry.u0, entry.v0, entry.u1, entry.v1],
                        color,
                    });
                }
                caret_x += sc.h_advance(gid);
                prev_gid = Some(gid);
            } else {
                let s = (logical_px / 8.0).round().max(1.0);
                if let Some(entry) = self
                    .glyph_atlas
                    .get_or_upload(&self.queue, None, ch, logical_px.round() as u32)
                {
                    glyphs.push(GlyphInstance {
                        rect: [caret_x, y, entry.w, entry.h],
                        uv_rect: [entry.u0, entry.v0, entry.u1, entry.v1],
                        color,
                    });
                }
                caret_x += 8.0 * s;
            }
        }
        staging.push_glyphs(glyphs);
    }
}

fn simple_quad(
    rect: [f32; 4],
    radii: [f32; 4],
    color: [f32; 4],
    border_color: [f32; 4],
    border_w: f32,
    opacity: f32,
) -> QuadInstance {
    QuadInstance {
        rect,
        radii,
        color_start: color,
        color_end: color,
        border_color,
        shadow_color: [0.0; 4],
        params0: [border_w, 0.0, 0.0, opacity],
        params1: [0.0; 4],
        params2: [1.0, 0.0, 0.0, 0.0],
    }
}

#[allow(clippy::too_many_arguments)]
fn create_offscreen_targets(
    device: &wgpu::Device,
    width: u32,
    height: u32,
    format: wgpu::TextureFormat,
    blit_bgl: &wgpu::BindGroupLayout,
    blur_bgl: &wgpu::BindGroupLayout,
    glass_bgl: &wgpu::BindGroupLayout,
    blur_ubuf: &wgpu::Buffer,
    glass_ubuf: &wgpu::Buffer,
    sampler: &wgpu::Sampler,
) -> OffscreenTargets {
    let make_tex = |label: &str, w: u32, h: u32| {
        let tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width: w.max(1),
                height: h.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
        (tex, view)
    };

    let (scene_tex, scene_view) = make_tex("knr_scene_tex", width, height);
    let (blur_a_tex, blur_a_view) = make_tex("knr_blur_a", (width / 2).max(1), (height / 2).max(1));
    let (blur_b_tex, blur_b_view) = make_tex("knr_blur_b", (width / 2).max(1), (height / 2).max(1));

    let scene_blit_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("knr_scene_blit_bg"),
        layout: blit_bgl,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&scene_view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
        ],
    });

    let blur_binding_size = NonZeroU64::new(std::mem::size_of::<BlurUniform>() as u64);
    let blur_scene_to_a_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("knr_blur_scene_to_a_bg"),
        layout: blur_bgl,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&scene_view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: blur_ubuf,
                    offset: 0,
                    size: blur_binding_size,
                }),
            },
        ],
    });
    let blur_a_to_b_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("knr_blur_a_to_b_bg"),
        layout: blur_bgl,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&blur_a_view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: blur_ubuf,
                    offset: 0,
                    size: blur_binding_size,
                }),
            },
        ],
    });
    let glass_blur_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("knr_glass_blur_bg"),
        layout: glass_bgl,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&blur_b_view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: glass_ubuf,
                    offset: 0,
                    size: NonZeroU64::new(std::mem::size_of::<GlassCompositeUniform>() as u64),
                }),
            },
        ],
    });

    OffscreenTargets {
        width,
        height,
        _scene_tex: scene_tex,
        scene_view,
        scene_blit_bg,
        _blur_a_tex: blur_a_tex,
        blur_a_view,
        _blur_b_tex: blur_b_tex,
        blur_b_view,
        blur_scene_to_a_bg,
        blur_a_to_b_bg,
        glass_blur_bg,
    }
}

/// All built-in GPU UI WGSL shaders exposed for compile-time / CI `naga` validation.
pub const BUILTIN_WGSL_SHADERS: &[(&str, &str)] = &[
    ("sdf_quad", SDF_QUAD_WGSL),
    ("text", TEXT_WGSL),
    ("blit", BLIT_WGSL),
    ("kawase_blur", KAWASE_BLUR_WGSL),
    ("glass_composite", GLASS_COMPOSITE_WGSL),
];

#[allow(dead_code)]
fn _keep_layout_rect_import(_: LayoutRect) {}
