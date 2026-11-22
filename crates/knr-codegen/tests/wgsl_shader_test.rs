//! End-to-End GPU Shader Compiler (`knr-codegen::wgsl`) & Static Purity Verifier Tests.
//!
//! Proves:
//! 1. Pure Strict TS `@shader` function (with flat structs, `#hex` Color literals, loops, branches,
//!    pure helper calls, and GPU math intrinsics) compiles to `naga`-validated WGSL AND
//!    `naga`-transpiled WebGL2 GLSL ES 3.00 (`#version 300 es`).
//! 2. `knr-verify` statically rejects effectful, heap-allocating, and recursive `@shader` functions.
//! 3. `examples/shader_studio.tsk` compiles both `aurora_waves` and `plasma_orbit` cleanly.

use std::path::PathBuf;

use knr_codegen::WgslBackend;
use knr_core::lower_program;
use knr_core::syntax::parse_knr;
use knr_verify::{verify_program, verify_program_from_file};

#[test]
fn test_shader_fn_compiles_to_naga_validated_wgsl_and_webgl2_glsl() {
    let src = r##"
        type Vec2 = {
            x: f64,
            y: f64,
        };

        type Color = {
            r: f64,
            g: f64,
            b: f64,
            a: f64,
        };

        function wave_mix(a: f64, b: f64, t: f64): f64 {
            return lerp(a, b, smoothstep(0.0, 1.0, t));
        }

        @shader
        function cosmic_grid(uv: Vec2, time: f64, speed: f64, rings: i64): Color {
            const base: Color = #0f172a;
            const accent: Color = #38bdf8;
            const cx = uv.x - 0.5;
            const cy = uv.y - 0.5;
            const dist = sqrt(cx * cx + cy * cy);
            const ang = atan2(cy, cx);

            let acc = 0.0;
            let i = 0;
            while (i < rings) {
                const fi = to_f64(i + 1);
                acc = acc + sin(dist * fi * 8.0 - time * speed + ang);
                i = i + 1;
            }
            const norm = clamp(0.5 + 0.25 * acc, 0.0, 1.0);
            const r = wave_mix(base.r, accent.r, norm);
            const g = wave_mix(base.g, accent.g, norm);
            const b = wave_mix(base.b, accent.b, norm);
            return { r: r, g: g, b: b, a: 1.0 };
        }
    "##;

    let prog = parse_knr(src).expect("parse shader .knr");
    verify_program(&prog).unwrap_or_else(|d| panic!("verify shader failed: {d:#?}"));
    let module = lower_program(&prog).expect("lower shader .knr");

    let compiled = WgslBackend::new()
        .compile_shader(&module, "cosmic_grid")
        .expect("WGSL + GLSL compilation must pass naga validation");

    assert_eq!(compiled.name, "cosmic_grid");
    assert_eq!(compiled.custom_param_count, 2);
    assert!(compiled.wgsl.contains("@vertex"));
    assert!(compiled.wgsl.contains("@fragment"));
    assert!(compiled.wgsl.contains("struct KnrUniforms"));
    assert!(compiled.wgsl.contains("fn knr_fn_cosmic_grid"));
    assert!(compiled.wgsl.contains("fn knr_fn_wave_mix"));

    // WebGL2 GLSL ES 3.00 output emitted by naga
    assert!(
        compiled.glsl_vertex.starts_with("#version 300 es"),
        "Vertex GLSL must be ES 3.00:\n{}",
        compiled.glsl_vertex
    );
    assert!(
        compiled.glsl_fragment.starts_with("#version 300 es"),
        "Fragment GLSL must be ES 3.00:\n{}",
        compiled.glsl_fragment
    );
    assert!(compiled.glsl_fragment.contains("void main()"));
}

#[test]
fn test_shader_verifier_rejects_effects_heap_and_recursion() {
    // 1. Effectful shader forbidden
    let effect_src = r#"
        @shader
        @effects("net")
        function bad_effect(x: f64, y: f64): f64 {
            return x + y;
        }
    "#;
    let err1 = verify_program(&parse_knr(effect_src).unwrap())
        .expect_err("Effectful shader must be rejected");
    assert!(
        err1.iter().any(|d| d.error_kind == "ShaderEffectForbidden"),
        "Expected ShaderEffectForbidden, got: {err1:#?}"
    );

    // 2. Heap allocation / string forbidden in GPU shader
    let heap_src = r#"
        @shader
        function bad_heap(x: f64): f64 {
            const s = "illegal_on_gpu";
            const n = str_len(s);
            return x + to_f64(n);
        }
    "#;
    let err2 = verify_program(&parse_knr(heap_src).unwrap())
        .expect_err("Heap/string shader must be rejected");
    assert!(
        err2.iter()
            .any(|d| d.error_kind == "ShaderImpureOp" || d.error_kind == "ShaderTypeForbidden"),
        "Expected ShaderImpureOp/ShaderTypeForbidden, got: {err2:#?}"
    );

    // 3. Direct recursion forbidden in GPU shader
    let rec_src = r#"
        @shader
        function bad_rec(x: f64): f64 {
            if (x <= 0.0) {
                return 0.0;
            }
            return bad_rec(x - 1.0);
        }
    "#;
    let err3 = verify_program(&parse_knr(rec_src).unwrap())
        .expect_err("Recursive shader must be rejected");
    assert!(
        err3.iter()
            .any(|d| d.error_kind == "ShaderRecursionForbidden"),
        "Expected ShaderRecursionForbidden, got: {err3:#?}"
    );
}

#[test]
fn test_examples_shader_studio_compiles_all_gpu_shaders() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    let studio_path = root.join("examples/shader_studio.tsk");
    let module = verify_program_from_file(&studio_path).expect("verify examples/shader_studio.tsk");

    let shaders = WgslBackend::new()
        .compile_all_shaders(&module)
        .expect("compile all shaders in shader_studio.tsk");
    assert_eq!(shaders.len(), 2, "Expected aurora_waves and plasma_orbit");

    let names: Vec<&str> = shaders.iter().map(|s| s.name.as_str()).collect();
    assert!(names.contains(&"aurora_waves"));
    assert!(names.contains(&"plasma_orbit"));
    for sh in &shaders {
        assert_eq!(sh.custom_param_count, 3);
        assert!(sh.wgsl.contains("@fragment"));
        assert!(sh.glsl_vertex.contains("#version 300 es"));
        assert!(sh.glsl_fragment.contains("#version 300 es"));
    }
}
