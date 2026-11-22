//! Native `.knr` -> WGSL + WebGL2 GLSL ES 3.00 GPU Shader Compiler Backend.
//!
//! Compiles pure verified `shader fn` functions (and their flat structs & pure helpers)
//! from canonical knr SSA into:
//! - Validated **WGSL** (executed on Desktop via `wgpu` Vulkan/Metal/DX12 and on Web via WebGPU)
//! - Validated **GLSL ES 3.00** (universal WebGL2 browser fallback generated via `naga`)

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;

use knr_core::{Function, Module, Op, RegId, Schema, Terminator, Type};

use crate::CodegenError;

/// Compiled & naga-validated GPU shader bundle ready for both Desktop (`wgpu`) and Web (WebGPU + WebGL2).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CompiledShader {
    pub name: String,
    pub wgsl: String,
    pub glsl_vertex: String,
    pub glsl_fragment: String,
    pub custom_param_count: usize,
}

#[derive(Debug, Default, Clone)]
pub struct WgslBackend;

impl WgslBackend {
    pub fn new() -> Self {
        Self
    }

    /// Compiles every `shader fn` in `module` into validated WGSL + WebGL2 GLSL shaders.
    pub fn compile_all_shaders(&self, module: &Module) -> Result<Vec<CompiledShader>, CodegenError> {
        let mut out = Vec::new();
        for func in &module.functions {
            if func.is_shader {
                out.push(self.compile_shader(module, &func.name)?);
            }
        }
        Ok(out)
    }

    /// Compiles every `shader fn` in `module` using parameter names from `source_map`
    /// (enabling automatic `resolution`/`res`, `dt`, `mouse`, `uv`, `time` binding by name).
    pub fn compile_all_shaders_with_source_map(
        &self,
        module: &Module,
        source_map: &knr_core::SourceMap,
    ) -> Result<Vec<CompiledShader>, CodegenError> {
        let mut out = Vec::new();
        for func in &module.functions {
            if func.is_shader {
                out.push(self.compile_shader_with_source_map(module, &func.name, source_map)?);
            }
        }
        Ok(out)
    }

    /// Compiles a single named `shader fn` (plus its transitive pure helpers & flat schemas)
    /// into validated WGSL and WebGL2 GLSL ES 3.00.
    pub fn compile_shader(
        &self,
        module: &Module,
        shader_name: &str,
    ) -> Result<CompiledShader, CodegenError> {
        self.compile_shader_inner(module, shader_name, None)
    }

    /// Compiles a single named `shader fn` using parameter names recorded in `source_map`.
    pub fn compile_shader_with_source_map(
        &self,
        module: &Module,
        shader_name: &str,
        source_map: &knr_core::SourceMap,
    ) -> Result<CompiledShader, CodegenError> {
        let entry = module
            .get_function(shader_name)
            .ok_or_else(|| CodegenError::Wgsl(format!("Shader function '{shader_name}' not found")))?;
        let names: Vec<Option<&str>> = entry
            .params
            .iter()
            .map(|(reg, _)| source_map.get_var(&entry.name, *reg))
            .collect();
        self.compile_shader_inner(module, shader_name, Some(&names))
    }

    /// Compiles a single named `shader fn` with explicit entry parameter names
    /// (for automatic `resolution`/`res`, `dt`, `uv`, `time`, `mouse` binding).
    pub fn compile_shader_with_param_names<S: AsRef<str>>(
        &self,
        module: &Module,
        shader_name: &str,
        param_names: &[S],
    ) -> Result<CompiledShader, CodegenError> {
        let names: Vec<Option<&str>> = param_names.iter().map(|s| Some(s.as_ref())).collect();
        self.compile_shader_inner(module, shader_name, Some(&names))
    }

    fn compile_shader_inner(
        &self,
        module: &Module,
        shader_name: &str,
        param_names: Option<&[Option<&str>]>,
    ) -> Result<CompiledShader, CodegenError> {
        let entry = module
            .get_function(shader_name)
            .ok_or_else(|| CodegenError::Wgsl(format!("Shader function '{shader_name}' not found")))?;

        if !entry.is_shader {
            return Err(CodegenError::Wgsl(format!(
                "Function '{shader_name}' is not declared with `shader fn`"
            )));
        }

        // Collect all transitively called helper functions in topological order (helpers before entry)
        let mut needed_fns: Vec<&Function> = Vec::new();
        let mut visited_fns: HashSet<String> = HashSet::new();
        collect_called_functions(module, entry, &mut visited_fns, &mut needed_fns)?;

        // Collect all referenced flat schemas in dependency order
        let mut needed_schemas: Vec<&Schema> = Vec::new();
        let mut visited_schemas: HashSet<String> = HashSet::new();
        for f in &needed_fns {
            for (_, ty) in &f.params {
                collect_type_schemas(module, ty, &mut visited_schemas, &mut needed_schemas);
            }
            collect_type_schemas(module, &f.ret_ty, &mut visited_schemas, &mut needed_schemas);
            for blk in &f.blocks {
                for (_, ty) in &blk.params {
                    collect_type_schemas(module, ty, &mut visited_schemas, &mut needed_schemas);
                }
                for inst in &blk.instructions {
                    collect_type_schemas(module, &inst.ty, &mut visited_schemas, &mut needed_schemas);
                    if let Op::StructNew(ref sname) = inst.op {
                        collect_type_schemas(
                            module,
                            &Type::Struct(sname.clone()),
                            &mut visited_schemas,
                            &mut needed_schemas,
                        );
                    }
                }
            }
        }

        let mut wgsl = String::with_capacity(4096);
        writeln!(wgsl, "// Generated by knr-codegen::wgsl for `shader fn {shader_name}`").unwrap();
        writeln!(
            wgsl,
            r#"struct KnrUniforms {{
    resolution: vec2<f32>,
    time: f32,
    dt: f32,
    mouse: vec4<f32>,
    rect_radius: vec4<f32>,
    params: array<vec4<f32>, 4>,
}};

@group(0) @binding(0)
var<uniform> u: KnrUniforms;

struct VsOut {{
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
}};

@vertex
fn vs_main(@builtin(vertex_index) vid: u32) -> VsOut {{
    var positions = array<vec2<f32>, 6>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>( 1.0, -1.0),
        vec2<f32>(-1.0,  1.0),
        vec2<f32>(-1.0,  1.0),
        vec2<f32>( 1.0, -1.0),
        vec2<f32>( 1.0,  1.0),
    );
    var uvs = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 1.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(0.0, 0.0),
        vec2<f32>(0.0, 0.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(1.0, 0.0),
    );
    var out: VsOut;
    out.pos = vec4<f32>(positions[vid], 0.0, 1.0);
    out.uv = uvs[vid];
    return out;
}}
"#
        )
        .unwrap();

        // Emit user flat struct definitions (fields sorted by index to match constructors)
        let schema_map: HashMap<&str, &Schema> = module
            .schemas
            .iter()
            .map(|s| (s.name.as_str(), s))
            .collect();

        for schema in &needed_schemas {
            writeln!(wgsl, "struct knr_s_{} {{", schema.name).unwrap();
            let mut fields_sorted: Vec<&knr_core::FieldDef> = schema.fields.iter().collect();
            fields_sorted.sort_by_key(|f| f.index);
            if fields_sorted.is_empty() {
                writeln!(wgsl, "    _pad: i32,").unwrap();
            } else {
                for field in fields_sorted {
                    let fty = wgsl_type(&field.ty)?;
                    writeln!(wgsl, "    f{}: {fty},", field.index).unwrap();
                }
            }
            writeln!(wgsl, "}};\n").unwrap();
        }

        // Emit compiled helper + entry functions
        for func in &needed_fns {
            emit_wgsl_function(func, &schema_map, &mut wgsl)?;
        }

        // Emit `@fragment fn fs_main` binding KnrUniforms -> entry shader params + rounded SDF clip
        let (call_args, custom_param_count) =
            build_entry_call_args(entry, &schema_map, param_names)?;
        writeln!(
            wgsl,
            r#"fn knr_rounded_mask(uv: vec2<f32>, res: vec2<f32>, rad: vec4<f32>) -> f32 {{
    let half = res * 0.5;
    let p = (uv - vec2<f32>(0.5, 0.5)) * res;
    var r = rad.x;
    if (p.x > 0.0) {{
        r = select(rad.y, rad.z, p.y > 0.0);
    }} else {{
        r = select(rad.x, rad.w, p.y > 0.0);
    }}
    let max_r = min(half.x, half.y);
    let rc = clamp(r, 0.0, max_r);
    let q = abs(p) - half + vec2<f32>(rc, rc);
    let d = length(max(q, vec2<f32>(0.0, 0.0))) + min(max(q.x, q.y), 0.0) - rc;
    return clamp(0.5 - d, 0.0, 1.0);
}}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {{
    let mask = knr_rounded_mask(in.uv, u.resolution, u.rect_radius);
    if (mask <= 0.001) {{
        discard;
    }}"#
        )
        .unwrap();

        match &entry.ret_ty {
            Type::Struct(sname) => {
                let sc = schema_map.get(sname.as_str()).ok_or_else(|| {
                    CodegenError::Wgsl(format!("Unknown shader return schema '{sname}'"))
                })?;
                if sc.fields.len() >= 4 {
                    writeln!(
                        wgsl,
                        "    let c = knr_fn_{}({});",
                        entry.name,
                        call_args.join(", ")
                    )
                    .unwrap();
                    writeln!(
                        wgsl,
                        "    return vec4<f32>(clamp(c.f0, 0.0, 1.0), clamp(c.f1, 0.0, 1.0), clamp(c.f2, 0.0, 1.0), clamp(c.f3, 0.0, 1.0) * mask);"
                    )
                    .unwrap();
                } else if sc.fields.len() == 3 {
                    writeln!(
                        wgsl,
                        "    let c = knr_fn_{}({});",
                        entry.name,
                        call_args.join(", ")
                    )
                    .unwrap();
                    writeln!(
                        wgsl,
                        "    return vec4<f32>(clamp(c.f0, 0.0, 1.0), clamp(c.f1, 0.0, 1.0), clamp(c.f2, 0.0, 1.0), mask);"
                    )
                    .unwrap();
                } else {
                    return Err(CodegenError::Wgsl(format!(
                        "Shader '{shader_name}' return struct '{sname}' must have 3 or 4 f64 color fields (r, g, b[, a])"
                    )));
                }
            }
            Type::F64 => {
                writeln!(
                    wgsl,
                    "    let lum = clamp(knr_fn_{}({}), 0.0, 1.0);",
                    entry.name,
                    call_args.join(", ")
                )
                .unwrap();
                writeln!(wgsl, "    return vec4<f32>(lum, lum, lum, mask);").unwrap();
            }
            other => {
                return Err(CodegenError::Wgsl(format!(
                    "Shader '{shader_name}' must return Color/Vec4/Vec3 or f64, found {other}"
                )));
            }
        }
        writeln!(wgsl, "}}").unwrap();

        // Validate WGSL with naga AND transpile to GLSL ES 3.00 (WebGL2 universal fallback!)
        let (glsl_vertex_raw, glsl_fragment_raw) = validate_and_transpile_glsl(&wgsl)?;
        let wgsl = minify_shader_source(&wgsl);
        let glsl_vertex = minify_shader_source(&glsl_vertex_raw);
        let glsl_fragment = minify_shader_source(&glsl_fragment_raw);

        Ok(CompiledShader {
            name: shader_name.to_string(),
            wgsl,
            glsl_vertex,
            glsl_fragment,
            custom_param_count,
        })
    }
}

/// Minimal standalone vertex-only WGSL module used to transpile `glsl_vertex` via `naga`
/// without pulling dead fragment helper functions into the WebGL2 vertex shader.
const VERTEX_ONLY_WGSL: &str = r#"struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vid: u32) -> VsOut {
    var positions = array<vec2<f32>, 6>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>( 1.0, -1.0),
        vec2<f32>(-1.0,  1.0),
        vec2<f32>(-1.0,  1.0),
        vec2<f32>( 1.0, -1.0),
        vec2<f32>( 1.0,  1.0),
    );
    var uvs = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 1.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(0.0, 0.0),
        vec2<f32>(0.0, 0.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(1.0, 0.0),
    );
    var out: VsOut;
    out.pos = vec4<f32>(positions[vid], 0.0, 1.0);
    out.uv = uvs[vid];
    return out;
}
"#;

/// Validates raw WGSL source via `naga` (used for built-in GPU UI shaders & tests).
pub fn validate_wgsl_source(wgsl: &str) -> Result<(), CodegenError> {
    let naga_mod = naga::front::wgsl::parse_str(wgsl).map_err(|e| {
        CodegenError::Wgsl(format!("WGSL parse error:\n{}", e.emit_to_string(wgsl)))
    })?;
    let mut validator = naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    );
    validator
        .validate(&naga_mod)
        .map_err(|e| CodegenError::Wgsl(format!("WGSL validation error: {e:?}")))?;
    Ok(())
}

/// Safely compacts generated WGSL or GLSL ES 3.00 shader source:
/// - Strips `//` comments and blank/indentation whitespace
/// - Preserves preprocessor `#...` lines (such as `#version 300 es`) verbatim on their own lines
/// - Tightens redundant spaces around unambiguous delimiters without merging operators
fn minify_shader_source(src: &str) -> String {
    let mut out = String::with_capacity(src.len() / 2);
    for raw_line in src.lines() {
        let trimmed = raw_line.trim();
        if trimmed.is_empty() || trimmed.starts_with("//") {
            continue;
        }
        if trimmed.starts_with('#') {
            if !out.is_empty() && !out.ends_with('\n') {
                out.push('\n');
            }
            out.push_str(trimmed);
            out.push('\n');
            continue;
        }
        // Strip inline `//` comments (knr WGSL/GLSL has no string literals)
        let code = match trimmed.find("//") {
            Some(idx) => trimmed[..idx].trim_end(),
            None => trimmed,
        };
        if code.is_empty() {
            continue;
        }
        let compact = compact_shader_line(code);
        out.push_str(&compact);
        out.push('\n');
    }
    out
}

fn compact_shader_line(line: &str) -> String {
    let bytes = line.as_bytes();
    let mut out = String::with_capacity(line.len());
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b' ' || b == b'\t' {
            while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b'\t') {
                i += 1;
            }
            let prev = out.as_bytes().last().copied();
            let next = bytes.get(i).copied();
            if let (Some(p), Some(n)) = (prev, next) {
                let tight_prev = matches!(p, b'(' | b'[' | b'{' | b',' | b';' | b':' | b'=');
                let tight_next = matches!(n, b')' | b']' | b'}' | b',' | b';' | b'=');
                // Keep space around `-` / `+` to avoid accidentally forming `--`, `++`, or `->`
                let safe_pair = !((p == b'-' && (n == b'-' || n == b'>'))
                    || (p == b'+' && n == b'+')
                    || (p == b'/' && n == b'/'));
                if !(tight_prev || tight_next) || !safe_pair {
                    out.push(' ');
                }
            }
            continue;
        }
        out.push(b as char);
        i += 1;
    }
    out
}

fn validate_and_transpile_glsl(wgsl: &str) -> Result<(String, String), CodegenError> {
    let naga_mod = naga::front::wgsl::parse_str(wgsl)
        .map_err(|e| CodegenError::Wgsl(format!("WGSL parse error:\n{}\n--- WGSL ---\n{wgsl}", e.emit_to_string(wgsl))))?;

    let mut validator = naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    );
    let mod_info = validator
        .validate(&naga_mod)
        .map_err(|e| CodegenError::Wgsl(format!("WGSL naga validation error: {e:?}\n--- WGSL ---\n{wgsl}")))?;

    let glsl_opts = naga::back::glsl::Options {
        version: naga::back::glsl::Version::Embedded {
            version: 300,
            is_webgl: true,
        },
        writer_flags: naga::back::glsl::WriterFlags::ADJUST_COORDINATE_SPACE,
        binding_map: Default::default(),
        zero_initialize_workgroup_memory: true,
    };

    // Compile the standalone vertex quad module so dead fragment functions are never duplicated
    // inside `glsl_vertex` (cuts ~137 KB of dead GLSL on multi-shader apps!).
    let vert_mod = naga::front::wgsl::parse_str(VERTEX_ONLY_WGSL)
        .map_err(|e| CodegenError::Wgsl(format!("Vertex WGSL parse error: {e:?}")))?;
    let vert_info = validator
        .validate(&vert_mod)
        .map_err(|e| CodegenError::Wgsl(format!("Vertex WGSL validation error: {e:?}")))?;

    let mut glsl_vert = String::new();
    {
        let pipe = naga::back::glsl::PipelineOptions {
            shader_stage: naga::ShaderStage::Vertex,
            entry_point: "vs_main".to_string(),
            multiview: None,
        };
        let mut writer = naga::back::glsl::Writer::new(
            &mut glsl_vert,
            &vert_mod,
            &vert_info,
            &glsl_opts,
            &pipe,
            naga::proc::BoundsCheckPolicies::default(),
        )
        .map_err(|e| CodegenError::Wgsl(format!("GLSL vertex init error: {e:?}")))?;
        writer
            .write()
            .map_err(|e| CodegenError::Wgsl(format!("GLSL vertex emit error: {e:?}")))?;
    }

    let mut glsl_frag = String::new();
    {
        let pipe = naga::back::glsl::PipelineOptions {
            shader_stage: naga::ShaderStage::Fragment,
            entry_point: "fs_main".to_string(),
            multiview: None,
        };
        let mut writer = naga::back::glsl::Writer::new(
            &mut glsl_frag,
            &naga_mod,
            &mod_info,
            &glsl_opts,
            &pipe,
            naga::proc::BoundsCheckPolicies::default(),
        )
        .map_err(|e| CodegenError::Wgsl(format!("GLSL fragment init error: {e:?}")))?;
        writer
            .write()
            .map_err(|e| CodegenError::Wgsl(format!("GLSL fragment emit error: {e:?}")))?;
    }

    Ok((glsl_vert, glsl_frag))
}

fn is_vec2_f64_schema(sname: &str, schema_map: &HashMap<&str, &Schema>) -> bool {
    schema_map.get(sname).is_some_and(|sc| {
        sc.fields.len() == 2
            && sc.fields.iter().all(|f| {
                f.ty == Type::F64 && f.name.as_deref() != Some("__tag")
            })
    })
}

fn next_param_slot(custom_idx: &mut usize) -> Result<String, CodegenError> {
    if *custom_idx >= 16 {
        return Err(CodegenError::Wgsl(
            "Shader exceeds maximum 16 custom uniform float slots (`u.params[0..4].xyzw`)"
                .to_string(),
        ));
    }
    let slot = *custom_idx / 4;
    let comp = ["x", "y", "z", "w"][*custom_idx % 4];
    *custom_idx += 1;
    Ok(format!("u.params[{slot}].{comp}"))
}

fn unpack_custom_uniform(
    ty: &Type,
    schema_map: &HashMap<&str, &Schema>,
    custom_idx: &mut usize,
) -> Result<String, CodegenError> {
    match ty {
        Type::F64 => next_param_slot(custom_idx),
        Type::I64 => Ok(format!("i32({})", next_param_slot(custom_idx)?)),
        Type::Bool => Ok(format!("({} > 0.5)", next_param_slot(custom_idx)?)),
        Type::Struct(sname) => {
            let sc = schema_map.get(sname.as_str()).ok_or_else(|| {
                CodegenError::Wgsl(format!("Unknown shader uniform schema '{sname}'"))
            })?;
            let mut fields_sorted: Vec<&knr_core::FieldDef> = sc.fields.iter().collect();
            fields_sorted.sort_by_key(|f| f.index);
            if fields_sorted.is_empty() {
                return Ok(format!("knr_s_{sname}(0i)"));
            }
            let mut exprs = Vec::with_capacity(fields_sorted.len());
            for f in fields_sorted {
                exprs.push(unpack_custom_uniform(&f.ty, schema_map, custom_idx)?);
            }
            Ok(format!("knr_s_{sname}({})", exprs.join(", ")))
        }
        other => Err(CodegenError::Wgsl(format!(
            "Unsupported shader uniform parameter type {other}"
        ))),
    }
}

fn build_entry_call_args(
    entry: &Function,
    schema_map: &HashMap<&str, &Schema>,
    param_names: Option<&[Option<&str>]>,
) -> Result<(Vec<String>, usize), CodegenError> {
    let mut args = Vec::with_capacity(entry.params.len());
    let mut custom_idx: usize = 0;
    let mut first_was_uv_x = false;
    let mut uv_bound = false;
    let mut time_bound = false;
    let mut mouse_bound = false;
    let mut res_bound = false;

    for (idx, (_, ty)) in entry.params.iter().enumerate() {
        let pname = param_names.and_then(|ns| ns.get(idx).copied().flatten());

        // 1. Named automatic special parameters (when SourceMap or explicit param_names provided)
        if let Some(name) = pname {
            match (name, ty) {
                ("uv", Type::Struct(sname)) if is_vec2_f64_schema(sname, schema_map) => {
                    args.push(format!("knr_s_{sname}(in.uv.x, in.uv.y)"));
                    uv_bound = true;
                    continue;
                }
                ("resolution" | "res", Type::Struct(sname))
                    if is_vec2_f64_schema(sname, schema_map) =>
                {
                    args.push(format!("knr_s_{sname}(u.resolution.x, u.resolution.y)"));
                    res_bound = true;
                    continue;
                }
                ("mouse", Type::Struct(sname)) if is_vec2_f64_schema(sname, schema_map) => {
                    args.push(format!("knr_s_{sname}(u.mouse.x, u.mouse.y)"));
                    mouse_bound = true;
                    continue;
                }
                ("time", Type::F64) => {
                    args.push("u.time".to_string());
                    time_bound = true;
                    continue;
                }
                ("dt" | "delta_time", Type::F64) => {
                    args.push("u.dt".to_string());
                    continue;
                }
                _ => {}
            }
        }

        // 2. Positional & schema-name bindings (100% backwards-compatible with legacy shaders!)
        match (idx, ty) {
            (_, Type::Struct(sname))
                if is_vec2_f64_schema(sname, schema_map)
                    && (sname.eq_ignore_ascii_case("resolution")
                        || sname.eq_ignore_ascii_case("res")) =>
            {
                args.push(format!("knr_s_{sname}(u.resolution.x, u.resolution.y)"));
                res_bound = true;
            }
            (0, Type::Struct(sname)) if is_vec2_f64_schema(sname, schema_map) && !uv_bound => {
                args.push(format!("knr_s_{sname}(in.uv.x, in.uv.y)"));
                uv_bound = true;
            }
            (0, Type::F64) => {
                args.push("in.uv.x".to_string());
                first_was_uv_x = true;
            }
            (1, Type::F64) if first_was_uv_x => {
                args.push("in.uv.y".to_string());
            }
            (1, Type::F64)
                if !time_bound && (pname.is_none() || matches!(pname, Some("t" | "elapsed"))) =>
            {
                args.push("u.time".to_string());
                time_bound = true;
            }
            (2, Type::F64)
                if first_was_uv_x
                    && !time_bound
                    && (pname.is_none() || matches!(pname, Some("t" | "elapsed"))) =>
            {
                args.push("u.time".to_string());
                time_bound = true;
            }
            (_, Type::Struct(sname))
                if pname.is_none()
                    && is_vec2_f64_schema(sname, schema_map)
                    && !mouse_bound
                    && custom_idx == 0 =>
            {
                // First extra Vec2 param before custom uniforms -> mouse UV
                args.push(format!("knr_s_{sname}(u.mouse.x, u.mouse.y)"));
                mouse_bound = true;
            }
            (_, Type::Struct(sname))
                if pname.is_none()
                    && is_vec2_f64_schema(sname, schema_map)
                    && mouse_bound
                    && !res_bound
                    && custom_idx == 0 =>
            {
                // Second extra Vec2 param before custom uniforms -> resolution
                args.push(format!("knr_s_{sname}(u.resolution.x, u.resolution.y)"));
                res_bound = true;
            }
            (_, Type::F64 | Type::I64 | Type::Bool | Type::Struct(_)) => {
                args.push(unpack_custom_uniform(ty, schema_map, &mut custom_idx)?);
            }
            _ => {
                return Err(CodegenError::Wgsl(format!(
                    "Unsupported shader entry parameter #{idx} type {ty}"
                )));
            }
        }
    }

    Ok((args, custom_idx))
}

fn collect_called_functions<'a>(
    module: &'a Module,
    func: &'a Function,
    visited: &mut HashSet<String>,
    out: &mut Vec<&'a Function>,
) -> Result<(), CodegenError> {
    if !visited.insert(func.name.clone()) {
        return Ok(());
    }
    for block in &func.blocks {
        for inst in &block.instructions {
            if let Op::Call(ref callee_name, _) = inst.op {
                let callee = module.get_function(callee_name).ok_or_else(|| {
                    CodegenError::Wgsl(format!("Unknown function '{callee_name}' called in shader"))
                })?;
                collect_called_functions(module, callee, visited, out)?;
            }
        }
    }
    out.push(func);
    Ok(())
}

fn collect_type_schemas<'a>(
    module: &'a Module,
    ty: &Type,
    visited: &mut HashSet<String>,
    out: &mut Vec<&'a Schema>,
) {
    if let Type::Struct(ref name) = ty {
        if !visited.insert(name.clone()) {
            return;
        }
        if let Some(sc) = module.get_schema(name) {
            for f in &sc.fields {
                collect_type_schemas(module, &f.ty, visited, out);
            }
            out.push(sc);
        }
    }
}

fn wgsl_type(ty: &Type) -> Result<String, CodegenError> {
    match ty {
        Type::F64 => Ok("f32".to_string()),
        Type::I64 => Ok("i32".to_string()),
        Type::Bool => Ok("bool".to_string()),
        Type::Struct(name) => Ok(format!("knr_s_{name}")),
        Type::Void => Ok(String::new()),
        other => Err(CodegenError::Wgsl(format!(
            "Type '{other}' is not representable in WGSL"
        ))),
    }
}

fn wgsl_zero(ty: &Type, schema_map: &HashMap<&str, &Schema>) -> Result<String, CodegenError> {
    match ty {
        Type::F64 => Ok("0.0".to_string()),
        Type::I64 => Ok("0i".to_string()),
        Type::Bool => Ok("false".to_string()),
        Type::Struct(name) => {
            let sc = schema_map.get(name.as_str()).ok_or_else(|| {
                CodegenError::Wgsl(format!("Unknown schema '{name}' in WGSL zero init"))
            })?;
            let mut fields_sorted: Vec<&knr_core::FieldDef> = sc.fields.iter().collect();
            fields_sorted.sort_by_key(|f| f.index);
            if fields_sorted.is_empty() {
                return Ok(format!("knr_s_{name}(0i)"));
            }
            let mut fields_zero = Vec::with_capacity(fields_sorted.len());
            for f in fields_sorted {
                fields_zero.push(wgsl_zero(&f.ty, schema_map)?);
            }
            Ok(format!("knr_s_{name}({})", fields_zero.join(", ")))
        }
        other => Err(CodegenError::Wgsl(format!(
            "Cannot zero-initialize '{other}' in WGSL"
        ))),
    }
}

fn wgsl_eq_expr(
    a_expr: &str,
    b_expr: &str,
    ty: &Type,
    schema_map: &HashMap<&str, &Schema>,
    depth: usize,
) -> Result<String, CodegenError> {
    if depth > 16 {
        return Ok("true".to_string());
    }
    match ty {
        Type::Struct(name) => {
            let sc = schema_map.get(name.as_str()).ok_or_else(|| {
                CodegenError::Wgsl(format!("Unknown schema '{name}' in WGSL equality"))
            })?;
            let mut fields_sorted: Vec<&knr_core::FieldDef> = sc.fields.iter().collect();
            fields_sorted.sort_by_key(|f| f.index);
            if fields_sorted.is_empty() {
                return Ok("true".to_string());
            }
            let mut parts = Vec::with_capacity(fields_sorted.len());
            for f in fields_sorted {
                let fa = format!("{a_expr}.f{}", f.index);
                let fb = format!("{b_expr}.f{}", f.index);
                parts.push(wgsl_eq_expr(&fa, &fb, &f.ty, schema_map, depth + 1)?);
            }
            Ok(format!("({})", parts.join(" && ")))
        }
        _ => Ok(format!("({a_expr} == {b_expr})")),
    }
}

fn format_f32_lit(v: f64) -> String {
    let f = v as f32;
    if !f.is_finite() {
        return "0.0".to_string();
    }
    let s = format!("{f}");
    if !s.contains('.') && !s.contains('e') && !s.contains('E') {
        format!("{s}.0")
    } else {
        s
    }
}

fn format_i32_lit(v: i64) -> String {
    let clamped = v.clamp(i32::MIN as i64, i32::MAX as i64) as i32;
    if clamped == i32::MIN {
        "(-2147483647i - 1i)".to_string()
    } else {
        format!("{clamped}i")
    }
}

fn for_each_shader_op_reg(op: &Op, mut f: impl FnMut(RegId)) {
    match op {
        Op::ConstI64(_) | Op::ConstF64(_) | Op::ConstBool(_) | Op::StructNew(_) => {}
        Op::Sin(a)
        | Op::Cos(a)
        | Op::Tan(a)
        | Op::Sqrt(a)
        | Op::Abs(a)
        | Op::Floor(a)
        | Op::Ceil(a)
        | Op::IntToFloat(a)
        | Op::FloatToInt(a)
        | Op::StructGet(a, _)
        | Op::Free(a) => f(*a),
        Op::Add(a, b)
        | Op::Sub(a, b)
        | Op::Mul(a, b)
        | Op::Div(a, b)
        | Op::Rem(a, b)
        | Op::CmpEq(a, b)
        | Op::CmpLt(a, b)
        | Op::Pow(a, b)
        | Op::Atan2(a, b)
        | Op::Min(a, b)
        | Op::Max(a, b)
        | Op::StructSet(a, _, b) => {
            f(*a);
            f(*b);
        }
        Op::Call(_, args) => {
            for &r in args {
                f(r);
            }
        }
        _ => {}
    }
}

fn for_each_shader_term_reg(term: &Terminator, mut f: impl FnMut(RegId)) {
    match term {
        Terminator::Ret(Some(r)) => f(*r),
        Terminator::Ret(None) | Terminator::Br(_) => {}
        Terminator::BrIf { cond, .. } => f(*cond),
        Terminator::BrArgs { args, .. } => {
            for &r in args {
                f(r);
            }
        }
        Terminator::BrIfArgs {
            cond,
            then_args,
            else_args,
            ..
        } => {
            f(*cond);
            for &r in then_args {
                f(r);
            }
            for &r in else_args {
                f(r);
            }
        }
    }
}

fn wgsl_zero_fields(
    sname: &str,
    schema_map: &HashMap<&str, &Schema>,
) -> Result<Vec<String>, CodegenError> {
    let sc = schema_map.get(sname).ok_or_else(|| {
        CodegenError::Wgsl(format!("Unknown schema '{sname}' in WGSL zero init"))
    })?;
    let mut fields_sorted: Vec<&knr_core::FieldDef> = sc.fields.iter().collect();
    fields_sorted.sort_by_key(|f| f.index);
    if fields_sorted.is_empty() {
        return Ok(vec!["0i".to_string()]);
    }
    let mut out = Vec::with_capacity(fields_sorted.len());
    for f in fields_sorted {
        out.push(wgsl_zero(&f.ty, schema_map)?);
    }
    Ok(out)
}

struct WgslBlockCtx<'a> {
    schema_map: &'a HashMap<&'a str, &'a Schema>,
    reg_types: &'a HashMap<RegId, Type>,
    param_regs: &'a HashSet<RegId>,
    const_lits: &'a HashMap<RegId, String>,
    cross_block_regs: &'a HashSet<RegId>,
    use_count: &'a HashMap<RegId, usize>,
    consumed_by_struct_set: HashSet<RegId>,
    inlined_exprs: HashMap<RegId, String>,
    pending_structs: HashMap<RegId, (String, Vec<String>)>,
    temp_counter: usize,
}

impl<'a> WgslBlockCtx<'a> {
    fn resolve(&self, r: RegId) -> String {
        if let Some(lit) = self.const_lits.get(&r) {
            return lit.clone();
        }
        if let Some(expr) = self.inlined_exprs.get(&r) {
            return expr.clone();
        }
        if let Some((sname, fields)) = self.pending_structs.get(&r) {
            return format!("knr_s_{sname}({})", fields.join(","));
        }
        if self.param_regs.contains(&r) {
            return format!("p{r}");
        }
        format!("r{r}")
    }

    fn emit_val_expr(&mut self, dst: RegId, expr: String, indent: &str, out: &mut String) {
        if self.cross_block_regs.contains(&dst) {
            writeln!(out, "{indent}r{dst} = {expr};").unwrap();
            return;
        }
        let uses = self.use_count.get(&dst).copied().unwrap_or(0);
        if uses == 0 {
            return;
        }
        if uses == 1 && expr.len() <= 96 {
            self.inlined_exprs.insert(dst, expr);
        } else {
            writeln!(out, "{indent}let r{dst} = {expr};").unwrap();
        }
    }
}

fn emit_wgsl_function(
    func: &Function,
    schema_map: &HashMap<&str, &Schema>,
    out: &mut String,
) -> Result<(), CodegenError> {
    let mut param_strs = Vec::with_capacity(func.params.len());
    let mut reg_types: HashMap<RegId, Type> = HashMap::new();
    let mut param_regs: HashSet<RegId> = HashSet::new();

    for (reg, ty) in &func.params {
        let wty = wgsl_type(ty)?;
        param_strs.push(format!("p{reg}: {wty}"));
        reg_types.insert(*reg, ty.clone());
        param_regs.insert(*reg);
    }

    let ret_sig = if func.ret_ty == Type::Void {
        String::new()
    } else {
        format!(" -> {}", wgsl_type(&func.ret_ty)?)
    };

    writeln!(
        out,
        "fn knr_fn_{}({}){ret_sig} {{",
        func.name,
        param_strs.join(", ")
    )
    .unwrap();

    // Pass 1: discover register types, constant literals, block params, defining blocks, and use counts.
    let mut block_param_regs: HashSet<RegId> = HashSet::new();
    let mut def_block: HashMap<RegId, u32> = HashMap::new();
    let mut const_lits: HashMap<RegId, String> = HashMap::new();
    let mut ordered_decl_regs: Vec<RegId> = Vec::new();
    let mut seen_decl_regs: HashSet<RegId> = HashSet::new();

    for block in &func.blocks {
        for (reg, ty) in &block.params {
            reg_types.insert(*reg, ty.clone());
            block_param_regs.insert(*reg);
            if seen_decl_regs.insert(*reg) {
                ordered_decl_regs.push(*reg);
            }
        }
        for inst in &block.instructions {
            if let Some(dst) = inst.dst {
                if inst.ty != Type::Void {
                    reg_types.insert(dst, inst.ty.clone());
                    def_block.insert(dst, block.id);
                    if seen_decl_regs.insert(dst) {
                        ordered_decl_regs.push(dst);
                    }
                    match &inst.op {
                        Op::ConstI64(v) => {
                            const_lits.insert(dst, format_i32_lit(*v));
                        }
                        Op::ConstF64(v) => {
                            const_lits.insert(dst, format_f32_lit(*v));
                        }
                        Op::ConstBool(b) => {
                            const_lits.insert(dst, if *b { "true" } else { "false" }.to_string());
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    let mut use_count: HashMap<RegId, usize> = HashMap::new();
    let mut cross_block_regs: HashSet<RegId> = HashSet::new();
    for block in &func.blocks {
        for inst in &block.instructions {
            for_each_shader_op_reg(&inst.op, |r| {
                if !matches!(inst.op, Op::Free(_)) {
                    *use_count.entry(r).or_insert(0) += 1;
                    if let Some(&db) = def_block.get(&r) {
                        if db != block.id && !const_lits.contains_key(&r) {
                            cross_block_regs.insert(r);
                        }
                    }
                }
            });
        }
        for_each_shader_term_reg(&block.terminator, |r| {
            *use_count.entry(r).or_insert(0) += 1;
            if let Some(&db) = def_block.get(&r) {
                if db != block.id && !const_lits.contains_key(&r) {
                    cross_block_regs.insert(r);
                }
            }
        });
    }

    // Only declare mutable `var` at function top for block parameters and cross-block SSA values.
    // All single-block SSA values become immutable `let` bindings or inlined expressions,
    // which eliminates 80%+ of Naga's redundant `float _eXX = r_N;` GLSL load temporaries!
    for &reg in &ordered_decl_regs {
        if param_regs.contains(&reg) || const_lits.contains_key(&reg) {
            continue;
        }
        if block_param_regs.contains(&reg) || cross_block_regs.contains(&reg) {
            if let Some(ty) = reg_types.get(&reg) {
                let wty = wgsl_type(ty)?;
                let z = wgsl_zero(ty, schema_map)?;
                writeln!(out, "    var r{reg}: {wty} = {z};").unwrap();
            }
        }
    }

    let block_map: HashMap<u32, &knr_core::BasicBlock> =
        func.blocks.iter().map(|b| (b.id, b)).collect();

    let emit_block_body = |blk: &knr_core::BasicBlock,
                           indent: &str,
                           emit_term: bool,
                           out: &mut String|
     -> Result<(), CodegenError> {
        let mut consumed_by_struct_set: HashSet<RegId> = HashSet::new();
        for inst in &blk.instructions {
            if let Op::StructSet(s, _, _) = inst.op {
                if use_count.get(&s).copied().unwrap_or(0) == 1 && !cross_block_regs.contains(&s) {
                    consumed_by_struct_set.insert(s);
                }
            }
        }
        let mut ctx = WgslBlockCtx {
            schema_map,
            reg_types: &reg_types,
            param_regs: &param_regs,
            const_lits: &const_lits,
            cross_block_regs: &cross_block_regs,
            use_count: &use_count,
            consumed_by_struct_set,
            inlined_exprs: HashMap::new(),
            pending_structs: HashMap::new(),
            temp_counter: 0,
        };
        for inst in &blk.instructions {
            emit_wgsl_inst(inst, &mut ctx, indent, out)?;
        }
        if emit_term {
            emit_wgsl_term(&blk.terminator, &block_map, &ctx, indent, out)?;
        } else {
            match &blk.terminator {
                Terminator::Ret(Some(r)) => {
                    let expr = ctx.resolve(*r);
                    writeln!(out, "{indent}return {expr};").unwrap();
                }
                Terminator::Ret(None) => writeln!(out, "{indent}return;").unwrap(),
                _ => {}
            }
        }
        Ok(())
    };

    if func.blocks.len() == 1 {
        emit_block_body(&func.blocks[0], "    ", false, out)?;
    } else {
        let entry_id = func.blocks.first().map(|b| b.id).unwrap_or(0);
        writeln!(out, "    var _bb: u32 = {entry_id}u;").unwrap();
        writeln!(out, "    var _guard: i32 = 0i;").unwrap();
        writeln!(out, "    loop {{").unwrap();
        writeln!(out, "        _guard = _guard + 1i;").unwrap();
        writeln!(out, "        if (_guard > 4096i) {{ break; }}").unwrap();
        writeln!(out, "        switch (_bb) {{").unwrap();

        for blk in &func.blocks {
            writeln!(out, "            case {}u: {{", blk.id).unwrap();
            emit_block_body(blk, "                ", true, out)?;
            writeln!(out, "            }}").unwrap();
        }
        writeln!(out, "            default: {{ break; }}").unwrap();
        writeln!(out, "        }}").unwrap();
        writeln!(out, "    }}").unwrap();
        if func.ret_ty != Type::Void {
            let z = wgsl_zero(&func.ret_ty, schema_map)?;
            writeln!(out, "    return {z};").unwrap();
        }
    }

    writeln!(out, "}}\n").unwrap();
    Ok(())
}

fn emit_wgsl_inst(
    inst: &knr_core::Instruction,
    ctx: &mut WgslBlockCtx<'_>,
    indent: &str,
    out: &mut String,
) -> Result<(), CodegenError> {
    let dst = match inst.dst {
        Some(d) if inst.ty != Type::Void => Some(d),
        _ => None,
    };

    match &inst.op {
        Op::ConstI64(_) | Op::ConstF64(_) | Op::ConstBool(_) => {
            // Inlined directly via `ctx.const_lits` at all use sites.
        }
        Op::Add(a, b) => {
            if let Some(d) = dst {
                let expr = format!("({} + {})", ctx.resolve(*a), ctx.resolve(*b));
                ctx.emit_val_expr(d, expr, indent, out);
            }
        }
        Op::Sub(a, b) => {
            if let Some(d) = dst {
                let expr = format!("({} - {})", ctx.resolve(*a), ctx.resolve(*b));
                ctx.emit_val_expr(d, expr, indent, out);
            }
        }
        Op::Mul(a, b) => {
            if let Some(d) = dst {
                let expr = format!("({} * {})", ctx.resolve(*a), ctx.resolve(*b));
                ctx.emit_val_expr(d, expr, indent, out);
            }
        }
        Op::Div(a, b) => {
            if let Some(d) = dst {
                let expr = format!("({} / {})", ctx.resolve(*a), ctx.resolve(*b));
                ctx.emit_val_expr(d, expr, indent, out);
            }
        }
        Op::Rem(a, b) => {
            if let Some(d) = dst {
                let ty_a = ctx.reg_types.get(a).unwrap_or(&inst.ty);
                let mut ae = ctx.resolve(*a);
                let mut be = ctx.resolve(*b);
                if *ty_a == Type::F64 || inst.ty == Type::F64 {
                    if ae.len() > 16 {
                        let t = format!("_t{}", ctx.temp_counter);
                        ctx.temp_counter += 1;
                        writeln!(out, "{indent}let {t} = {ae};").unwrap();
                        ae = t;
                    }
                    if be.len() > 16 {
                        let t = format!("_t{}", ctx.temp_counter);
                        ctx.temp_counter += 1;
                        writeln!(out, "{indent}let {t} = {be};").unwrap();
                        be = t;
                    }
                    let expr = format!("({ae} - {be} * floor({ae} / {be}))");
                    ctx.emit_val_expr(d, expr, indent, out);
                } else {
                    let expr = format!("({ae} % {be})");
                    ctx.emit_val_expr(d, expr, indent, out);
                }
            }
        }
        Op::CmpEq(a, b) => {
            if let Some(d) = dst {
                let ty_a = ctx.reg_types.get(a).unwrap_or(&Type::I64);
                let mut ae = ctx.resolve(*a);
                let mut be = ctx.resolve(*b);
                if matches!(ty_a, Type::Struct(_)) {
                    if ae.len() > 16 {
                        let t = format!("_t{}", ctx.temp_counter);
                        ctx.temp_counter += 1;
                        writeln!(out, "{indent}let {t} = {ae};").unwrap();
                        ae = t;
                    }
                    if be.len() > 16 {
                        let t = format!("_t{}", ctx.temp_counter);
                        ctx.temp_counter += 1;
                        writeln!(out, "{indent}let {t} = {be};").unwrap();
                        be = t;
                    }
                }
                let expr = wgsl_eq_expr(&ae, &be, ty_a, ctx.schema_map, 0)?;
                ctx.emit_val_expr(d, expr, indent, out);
            }
        }
        Op::CmpLt(a, b) => {
            if let Some(d) = dst {
                let expr = format!("({} < {})", ctx.resolve(*a), ctx.resolve(*b));
                ctx.emit_val_expr(d, expr, indent, out);
            }
        }
        Op::Sin(a) => {
            if let Some(d) = dst {
                let expr = format!("sin({})", ctx.resolve(*a));
                ctx.emit_val_expr(d, expr, indent, out);
            }
        }
        Op::Cos(a) => {
            if let Some(d) = dst {
                let expr = format!("cos({})", ctx.resolve(*a));
                ctx.emit_val_expr(d, expr, indent, out);
            }
        }
        Op::Tan(a) => {
            if let Some(d) = dst {
                let expr = format!("tan({})", ctx.resolve(*a));
                ctx.emit_val_expr(d, expr, indent, out);
            }
        }
        Op::Sqrt(a) => {
            if let Some(d) = dst {
                let expr = format!("sqrt(max({}, 0.0))", ctx.resolve(*a));
                ctx.emit_val_expr(d, expr, indent, out);
            }
        }
        Op::Abs(a) => {
            if let Some(d) = dst {
                let expr = format!("abs({})", ctx.resolve(*a));
                ctx.emit_val_expr(d, expr, indent, out);
            }
        }
        Op::Floor(a) => {
            if let Some(d) = dst {
                let expr = format!("floor({})", ctx.resolve(*a));
                ctx.emit_val_expr(d, expr, indent, out);
            }
        }
        Op::Ceil(a) => {
            if let Some(d) = dst {
                let expr = format!("ceil({})", ctx.resolve(*a));
                ctx.emit_val_expr(d, expr, indent, out);
            }
        }
        Op::Pow(a, b) => {
            if let Some(d) = dst {
                let expr = format!("pow(abs({}), {})", ctx.resolve(*a), ctx.resolve(*b));
                ctx.emit_val_expr(d, expr, indent, out);
            }
        }
        Op::Atan2(y, x) => {
            if let Some(d) = dst {
                let expr = format!("atan2({}, {})", ctx.resolve(*y), ctx.resolve(*x));
                ctx.emit_val_expr(d, expr, indent, out);
            }
        }
        Op::Min(a, b) => {
            if let Some(d) = dst {
                let expr = format!("min({}, {})", ctx.resolve(*a), ctx.resolve(*b));
                ctx.emit_val_expr(d, expr, indent, out);
            }
        }
        Op::Max(a, b) => {
            if let Some(d) = dst {
                let expr = format!("max({}, {})", ctx.resolve(*a), ctx.resolve(*b));
                ctx.emit_val_expr(d, expr, indent, out);
            }
        }
        Op::IntToFloat(a) => {
            if let Some(d) = dst {
                let expr = format!("f32({})", ctx.resolve(*a));
                ctx.emit_val_expr(d, expr, indent, out);
            }
        }
        Op::FloatToInt(a) => {
            if let Some(d) = dst {
                let expr = format!("i32({})", ctx.resolve(*a));
                ctx.emit_val_expr(d, expr, indent, out);
            }
        }
        Op::StructNew(name) => {
            if let Some(d) = dst {
                let fields = wgsl_zero_fields(name, ctx.schema_map)?;
                if ctx.consumed_by_struct_set.contains(&d) {
                    ctx.pending_structs.insert(d, (name.clone(), fields));
                } else {
                    let ctor = format!("knr_s_{name}({})", fields.join(","));
                    ctx.emit_val_expr(d, ctor, indent, out);
                }
            }
        }
        Op::StructGet(s, fidx) => {
            if let Some(d) = dst {
                let expr = if let Some((_, fields)) = ctx.pending_structs.get(s) {
                    fields
                        .get(*fidx as usize)
                        .cloned()
                        .unwrap_or_else(|| format!("{}.f{fidx}", ctx.resolve(*s)))
                } else {
                    format!("{}.f{fidx}", ctx.resolve(*s))
                };
                ctx.emit_val_expr(d, expr, indent, out);
            }
        }
        Op::StructSet(s, fidx, val) => {
            if let Some(d) = dst {
                let v_expr = ctx.resolve(*val);
                if let Some((sname, mut fields)) = ctx.pending_structs.remove(s) {
                    let idx = *fidx as usize;
                    if idx < fields.len() {
                        fields[idx] = v_expr;
                    }
                    if ctx.consumed_by_struct_set.contains(&d) {
                        ctx.pending_structs.insert(d, (sname, fields));
                    } else {
                        let ctor = format!("knr_s_{sname}({})", fields.join(","));
                        ctx.emit_val_expr(d, ctor, indent, out);
                    }
                } else {
                    let s_expr = ctx.resolve(*s);
                    if ctx.cross_block_regs.contains(&d) {
                        writeln!(out, "{indent}r{d} = {s_expr};").unwrap();
                        writeln!(out, "{indent}r{d}.f{fidx} = {v_expr};").unwrap();
                    } else {
                        writeln!(out, "{indent}var r{d} = {s_expr};").unwrap();
                        writeln!(out, "{indent}r{d}.f{fidx} = {v_expr};").unwrap();
                    }
                }
            }
        }
        Op::Call(name, args) => {
            let arg_list: Vec<String> = args.iter().map(|r| ctx.resolve(*r)).collect();
            let call_str = format!("knr_fn_{name}({})", arg_list.join(", "));
            if let Some(d) = dst {
                ctx.emit_val_expr(d, call_str, indent, out);
            } else {
                writeln!(out, "{indent}{call_str};").unwrap();
            }
        }
        Op::Free(_) => {
            // Flat GPU structs live in WGSL registers/stack — Free is a zero-cost no-op!
        }
        other => {
            return Err(CodegenError::Wgsl(format!(
                "Unsupported op in WGSL shader: {other:?}"
            )));
        }
    }
    Ok(())
}

fn emit_wgsl_term(
    term: &Terminator,
    block_map: &HashMap<u32, &knr_core::BasicBlock>,
    ctx: &WgslBlockCtx<'_>,
    indent: &str,
    out: &mut String,
) -> Result<(), CodegenError> {
    let emit_edge =
        |target: u32, args: &[RegId], ind: &str, out: &mut String| -> Result<(), CodegenError> {
            if let Some(t_blk) = block_map.get(&target) {
                let count = args.len().min(t_blk.params.len());
                if count > 0 {
                    // Parallel copy via temp locals to avoid clobbering cycles
                    for (i, (&arg, (_, pty))) in
                        args[..count].iter().zip(&t_blk.params[..count]).enumerate()
                    {
                        if *pty == Type::Void {
                            continue;
                        }
                        let ty = ctx
                            .reg_types
                            .get(&arg)
                            .cloned()
                            .unwrap_or_else(|| pty.clone());
                        let wty = wgsl_type(&ty)?;
                        let arg_expr = ctx.resolve(arg);
                        writeln!(out, "{ind}let _tmp_{i}: {wty} = {arg_expr};").unwrap();
                    }
                    for (i, (preg, pty)) in t_blk.params[..count].iter().enumerate() {
                        if *pty == Type::Void {
                            continue;
                        }
                        writeln!(out, "{ind}r{preg} = _tmp_{i};").unwrap();
                    }
                }
            }
            writeln!(out, "{ind}_bb = {target}u;").unwrap();
            Ok(())
        };

    match term {
        Terminator::Ret(Some(r)) => {
            let expr = ctx.resolve(*r);
            writeln!(out, "{indent}return {expr};").unwrap();
        }
        Terminator::Ret(None) => writeln!(out, "{indent}return;").unwrap(),
        Terminator::Br(target) => emit_edge(*target, &[], indent, out)?,
        Terminator::BrArgs { target, args } => emit_edge(*target, args, indent, out)?,
        Terminator::BrIf {
            cond,
            then_blk,
            else_blk,
        } => {
            let c_expr = ctx.resolve(*cond);
            writeln!(out, "{indent}if ({c_expr}) {{").unwrap();
            emit_edge(*then_blk, &[], &format!("{indent}    "), out)?;
            writeln!(out, "{indent}}} else {{").unwrap();
            emit_edge(*else_blk, &[], &format!("{indent}    "), out)?;
            writeln!(out, "{indent}}}").unwrap();
        }
        Terminator::BrIfArgs {
            cond,
            then_blk,
            then_args,
            else_blk,
            else_args,
        } => {
            let c_expr = ctx.resolve(*cond);
            writeln!(out, "{indent}if ({c_expr}) {{").unwrap();
            emit_edge(*then_blk, then_args, &format!("{indent}    "), out)?;
            writeln!(out, "{indent}}} else {{").unwrap();
            emit_edge(*else_blk, else_args, &format!("{indent}    "), out)?;
            writeln!(out, "{indent}}}").unwrap();
        }
    }
    Ok(())
}
