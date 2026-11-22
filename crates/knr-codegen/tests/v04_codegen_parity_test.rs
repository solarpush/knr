//! Comprehensive v0.4.0 Codegen & Shader Parity Test Suite.
//!
//! Verifies:
//! 1. 3-way Differential Parity (`VM == Cranelift JIT == WebAssembly via Node.js` with ZERO JS imports):
//!    - Loops with multiple mutable block params (swaps, break/continue, nested loops)
//!    - Deep recursive calls (direct & mutual recursion, multi-arg functions)
//!    - Nested `Vec<Struct>` and `Struct` containing `Vec<T>` (including growth past initial cap)
//!    - Tagged `enum` matching & structural `===` equality
//!    - Pure in-WASM `Map` (`map_new`, `map_set`, `map_get`, `map_del`) with `string` and `i64` keys,
//!      `f64`/`bool` values, overwrite & dynamic growth
//!    - Pure in-WASM `Buf` (`buf_alloc`, `buf_write_u8`, `buf_read_u8`, `buf_slice`, zero-init)
//!    - `f64` `Op::Rem`, `IntToFloat`, `FloatToInt`, math intrinsics, `StrLen`, `IntToStr` (`i64`/`bool`/`f64`)
//! 2. WASM Zero-Cost Arena Telemetry (`__knr_heap_used`, `__knr_heap_capacity`, `__knr_heap_mark`, `__knr_heap_reset`)
//!    on both pure-arithmetic and heap-allocating modules, including arena reset & zero-init safety.
//! 3. Multi-Uniform & Rich-Type WGSL + WebGL2 GLSL ES 3.00 GPU Shader Compiler (`naga` validated):
//!    - Up to 16 scalar uniforms (`f64`/`i64`/`bool`) packed into `u.params[0..4].xyzw` + overflow rejection
//!    - `Vec2`, `Vec3`, and `Color` struct uniform parameters unpacked from consecutive `u.params` slots
//!    - Automatic `resolution` / `res: Vec2` and `dt: f64` special bindings alongside `uv`, `time`, `mouse`
//!    - `Op::Rem` for BOTH `Type::I64` and `Type::F64`, plus all GPU math/conversion intrinsics
//!    - Tagged `enum` (`__tag` switches, payloads, and `===` comparisons) inside `@shader` functions

use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use knr_codegen::{CraneliftBackend, WasmBackend, WgslBackend};
use knr_core::syntax::parse_knr;
use knr_core::{
    lower_program, lower_program_with_source_map, BasicBlock, Function, Instruction, Module, Op,
    Schema, Terminator, Type,
};
use knr_interp::{execute_module, Value};
use knr_runtime::MockPlatform;
use knr_verify::{verify_module, verify_program};

static WASM_TMP_SEQ: AtomicU64 = AtomicU64::new(0);

fn run_node_wasm_script(wasm_bytes: &[u8], js_body: &str) -> Option<String> {
    let seq = WASM_TMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let tmp = std::env::temp_dir().join(format!(
        "knr_v04_parity_{}_{}.wasm",
        std::process::id(),
        seq
    ));
    std::fs::write(&tmp, wasm_bytes).ok()?;
    let js = format!(
        "const fs = require('fs'); \
         const bytes = fs.readFileSync({}); \
         WebAssembly.instantiate(bytes, {{}}).then(({{instance}}) => {{ \
           const e = instance.exports; \
           {js_body} \
         }}).catch(err => {{ console.error(err); process.exit(1); }});",
        serde_json::to_string(tmp.to_str().unwrap()).unwrap(),
    );
    let out = Command::new("node").arg("-e").arg(&js).output();
    let _ = std::fs::remove_file(&tmp);
    let out = match out {
        Ok(o) => o,
        Err(_) => return None,
    };
    assert!(
        out.status.success(),
        "Node.js WASM execution failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn assert_module_triple_parity_i64(module: &Module, entry: &str, expected: i64) {
    // 1. VM
    let mut host = MockPlatform::new();
    let (vm_val, _) =
        execute_module(module, entry, vec![], &mut host, 2_000_000, false).expect("VM exec");
    assert_eq!(
        vm_val,
        Value::I64(expected),
        "VM mismatch for {entry}: got {vm_val:?}, expected {expected}"
    );

    // 2. Cranelift JIT
    let mut cl = CraneliftBackend::new().expect("Cranelift init");
    let fn_ptr = cl
        .compile_jit_module(module, entry)
        .expect("Cranelift JIT compile");
    let jit_fn: extern "C" fn() -> i64 = unsafe { std::mem::transmute(fn_ptr) };
    let jit_val = jit_fn();
    assert_eq!(
        jit_val, expected,
        "Cranelift JIT mismatch for {entry}: got {jit_val}, expected {expected}"
    );

    // 3. Zero-Import WebAssembly via Node.js
    let wasm_bytes = WasmBackend::new().compile(module).expect("WASM compile");
    assert_eq!(&wasm_bytes[0..4], b"\0asm");
    let entry_json = serde_json::to_string(entry).unwrap();
    if let Some(stdout) = run_node_wasm_script(
        &wasm_bytes,
        &format!("console.log(e[{entry_json}]().toString());"),
    ) {
        let wasm_val: i64 = stdout.parse().expect("parse WASM i64 output");
        assert_eq!(
            wasm_val, expected,
            "WASM mismatch for {entry}: got {wasm_val}, expected {expected}"
        );
    }
}

fn assert_source_triple_parity_i64(src: &str, entry: &str, expected: i64) {
    let prog = parse_knr(src).expect("parse .knr");
    verify_program(&prog).unwrap_or_else(|d| panic!("verify_program failed:\n{d:#?}"));
    let module = lower_program(&prog).expect("lower_program");
    assert_module_triple_parity_i64(&module, entry, expected);
}

// ============================================================================
// 1. 3-WAY DIFFERENTIAL PARITY TESTS (VM == Cranelift JIT == WASM)
// ============================================================================

#[test]
fn test_v04_parity_multi_param_loops_and_swaps() {
    let src = r#"
        function main(): i64 {
            let a = 1;
            let b = 2;
            let c = 3;
            let d = 4;
            let acc = 0;
            let i = 0;
            while (i < 12) {
                // Rotate 4 loop-carried mutable variables simultaneously
                const na = b;
                const nb = c;
                const nc = d;
                const nd = a + 1;
                a = na;
                b = nb;
                c = nc;
                d = nd;
                acc = acc + a * 3 - b + c * 2 + d;
                i = i + 1;
            }
            return acc + a + b + c + d;
        }
    "#;
    // Compute expected independently in Rust
    let (mut a, mut b, mut c, mut d, mut acc) = (1i64, 2i64, 3i64, 4i64, 0i64);
    for _ in 0..12 {
        (a, b, c, d) = (b, c, d, a + 1);
        acc += a * 3 - b + c * 2 + d;
    }
    assert_source_triple_parity_i64(src, "main", acc + a + b + c + d);
}

#[test]
fn test_v04_parity_deep_recursion_and_multi_arg_functions() {
    let src = r#"
        function mix5(a: i64, b: f64, flag: bool, c: i64, d: f64): i64 {
            const bonus = flag ? 10 : 0;
            return a + c + bonus + to_i64(floor(b * d));
        }

        function sum_rec(n: i64, acc: i64): i64 {
            if (n <= 0) {
                return acc;
            }
            return sum_rec(n - 1, acc + n);
        }

        function is_even(n: i64): bool {
            if (n === 0) {
                return true;
            }
            return is_odd(n - 1);
        }

        function is_odd(n: i64): bool {
            if (n === 0) {
                return false;
            }
            return is_even(n - 1);
        }

        function main(): i64 {
            const s = sum_rec(40, 0); // 40 * 41 / 2 = 820
            const m = mix5(7, 2.5, true, 3, 4.0); // 7 + 3 + 10 + 10 = 30
            const e = is_even(24) ? 100 : 0;
            const o = is_odd(25) ? 50 : 0;
            return s + m + e + o;
        }
    "#;
    assert_source_triple_parity_i64(src, "main", 820 + 30 + 100 + 50);

    // Also test deep recursion (depth 400) directly between Cranelift JIT and WASM!
    let deep_src = r#"
        function sum_deep(n: i64): i64 {
            if (n <= 0) {
                return 0;
            }
            return n + sum_deep(n - 1);
        }

        function main(): i64 {
            return sum_deep(400);
        }
    "#;
    let prog = parse_knr(deep_src).expect("parse deep recursion");
    verify_program(&prog).expect("verify deep recursion");
    let module = lower_program(&prog).expect("lower deep recursion");
    let expected = 400 * 401 / 2;

    let mut cl = CraneliftBackend::new().expect("cl init");
    let ptr = cl.compile_jit_module(&module, "main").expect("cl jit");
    let f: extern "C" fn() -> i64 = unsafe { std::mem::transmute(ptr) };
    assert_eq!(f(), expected);

    let wasm = WasmBackend::new().compile(&module).expect("wasm compile");
    if let Some(out) = run_node_wasm_script(&wasm, "console.log(e.main().toString());") {
        assert_eq!(out.parse::<i64>().unwrap(), expected);
    }
}

#[test]
fn test_v04_parity_nested_vec_of_struct_and_struct_with_vec() {
    let src = r#"
        type Item = {
            id: i64,
            weight: f64,
            active: bool,
            name: string,
        };

        type Bag = {
            title: string,
            items: Item[],
            scores: i64[],
        };

        function sum_bag(bag: readonly Bag): i64 {
            let total = str_len(bag.title);
            let i = 0;
            const n = vec_len(bag.items);
            while (i < n) {
                const it = bag.items[i];
                const act = it.active ? 10 : 1;
                total = total + it.id + to_i64(floor(it.weight)) + act + str_len(it.name);
                i = i + 1;
            }
            let j = 0;
            const m = vec_len(bag.scores);
            while (j < m) {
                total = total + bag.scores[j];
                j = j + 1;
            }
            return total;
        }

        function main(): i64 {
            let items: Item[] = vec_new();
            let scores: i64[] = vec_new();
            let k = 0;
            // Push 16 items to force dynamic vector reallocation (> initial cap 8)
            while (k < 16) {
                const nm = "item-" + to_str(k);
                const it: Item = {
                    id: k * 3,
                    weight: to_f64(k) + 0.75,
                    active: k % 2 === 0,
                    name: nm,
                };
                items = vec_push(items, it);
                scores = vec_push(scores, k * 5);
                k = k + 1;
            }
            const bag: Bag = {
                title: "v04-bag",
                items: items,
                scores: scores,
            };
            return sum_bag(bag);
        }
    "#;
    let mut expected = "v04-bag".len() as i64;
    for k in 0..16i64 {
        let nm = format!("item-{k}");
        let act = if k % 2 == 0 { 10 } else { 1 };
        expected += k * 3 + k + act + nm.len() as i64 + k * 5;
    }
    assert_source_triple_parity_i64(src, "main", expected);
}

#[test]
fn test_v04_parity_enums_structural_equality_and_strings() {
    let src = r#"
        enum Mode {
            Idle,
            Boost(i64),
            Scale(f64),
        }

        enum Tag {
            Alpha,
            Beta,
            Gamma,
        }

        function eval_mode(m: Mode): i64 {
            switch (m) {
                case Mode.Idle:
                    return 5;
                case Mode.Boost(v):
                    return v * 4;
                case Mode.Scale(s):
                    return to_i64(floor(s * 10.0));
            }
        }

        function main(): i64 {
            const t1 = Tag.Beta;
            const t2 = Tag.Beta;
            const t3 = Tag.Gamma;
            const eq_same = t1 === t2 ? 100 : 0;
            const eq_diff = t1 === t3 ? 1000 : 10;

            const s_true = to_str(true);
            const s_false = to_str(false);
            const s_neg = to_str(-12345);
            const s_flt = to_str(3.5);
            const s_nflt = to_str(-0.25);

            const bool_ok = (s_true === "true" && s_false === "false") ? 50 : 0;
            const flt_ok = (s_flt === "3.5" && s_nflt === "-0.25") ? 75 : 0;
            const lens = str_len(s_true) + str_len(s_false) + str_len(s_neg) + str_len(s_flt);

            const m1 = eval_mode(Mode.Idle);
            const m2 = eval_mode(Mode.Boost(9));
            const m3 = eval_mode(Mode.Scale(2.7));
            return eq_same + eq_diff + bool_ok + flt_ok + lens + m1 + m2 + m3;
        }
    "#;
    // eq_same=100, eq_diff=10, bool_ok=50, flt_ok=75, lens=4+5+6+3=18, m1=5, m2=36, m3=27 => 321
    assert_source_triple_parity_i64(src, "main", 321);
}

#[test]
fn test_v04_parity_map_str_and_i64_keys_set_get_del_growth() {
    // Build canonical SSA exercising map_new, map_set (overwrite + >8 growth), map_get, map_del
    // with BOTH string keys (including dynamic concatenated keys) AND i64 keys!
    let mut module = Module::new();
    let mut f = Function::new("main", Type::I64);
    let mut bb0 = BasicBlock::new(0, Terminator::Ret(Some(100)));

    // String-keyed map with 12 entries (triggers dynamic resize past cap 8)
    let mut m_reg = 0u32;
    let map_ty = Type::Map(Box::new(Type::Str), Box::new(Type::I64));
    bb0.push(Instruction::new(
        Some(m_reg),
        map_ty.clone(),
        Op::MapNew(Type::Str, Type::I64),
        1,
    ));
    let mut next_r = 1u32;
    for i in 0..12i64 {
        let rk = next_r;
        next_r += 1;
        bb0.push(Instruction::new(
            Some(rk),
            Type::Str,
            Op::ConstStr(format!("k{i}")),
            2,
        ));
        let rv = next_r;
        next_r += 1;
        bb0.push(Instruction::new(
            Some(rv),
            Type::I64,
            Op::ConstI64((i + 1) * 10),
            3,
        ));
        let rm = next_r;
        next_r += 1;
        bb0.push(Instruction::new(
            Some(rm),
            map_ty.clone(),
            Op::MapSet(m_reg, rk, rv),
            4,
        ));
        m_reg = rm;
    }

    // Overwrite "k3" using a dynamically concatenated key ("k" + to_str(3)) -> 999
    let rk_pre = next_r;
    next_r += 1;
    bb0.push(Instruction::new(Some(rk_pre), Type::Str, Op::ConstStr("k".into()), 5));
    let r3 = next_r;
    next_r += 1;
    bb0.push(Instruction::new(Some(r3), Type::I64, Op::ConstI64(3), 6));
    let r3_s = next_r;
    next_r += 1;
    bb0.push(Instruction::new(Some(r3_s), Type::Str, Op::IntToStr(r3), 7));
    let rk3_dyn = next_r;
    next_r += 1;
    bb0.push(Instruction::new(Some(rk3_dyn), Type::Str, Op::Add(rk_pre, r3_s), 8));
    let r999 = next_r;
    next_r += 1;
    bb0.push(Instruction::new(Some(r999), Type::I64, Op::ConstI64(999), 9));
    let rm_ow = next_r;
    next_r += 1;
    bb0.push(Instruction::new(Some(rm_ow), map_ty.clone(), Op::MapSet(m_reg, rk3_dyn, r999), 10));

    // Delete "k5" and verify "k3" (999) and "k11" (120) remain intact
    let rk5 = next_r;
    next_r += 1;
    bb0.push(Instruction::new(Some(rk5), Type::Str, Op::ConstStr("k5".into()), 11));
    let rm_del = next_r;
    next_r += 1;
    bb0.push(Instruction::new(Some(rm_del), map_ty.clone(), Op::MapDel(rm_ow, rk5), 12));

    let rk11 = next_r;
    next_r += 1;
    bb0.push(Instruction::new(Some(rk11), Type::Str, Op::ConstStr("k11".into()), 13));
    let rv3 = next_r;
    next_r += 1;
    bb0.push(Instruction::new(Some(rv3), Type::I64, Op::MapGet(rm_del, rk3_dyn), 14));
    let rv11 = next_r;
    next_r += 1;
    bb0.push(Instruction::new(Some(rv11), Type::I64, Op::MapGet(rm_del, rk11), 15));
    bb0.push(Instruction::new(None, Type::Void, Op::Free(rm_del), 16));

    // Integer-keyed map: Map<i64, f64> (set 42 -> 7.5, get 42, del 42, set 42 -> 12.5, get 42)
    let imap_ty = Type::Map(Box::new(Type::I64), Box::new(Type::F64));
    let im0 = next_r;
    next_r += 1;
    bb0.push(Instruction::new(Some(im0), imap_ty.clone(), Op::MapNew(Type::I64, Type::F64), 17));
    let ik = next_r;
    next_r += 1;
    bb0.push(Instruction::new(Some(ik), Type::I64, Op::ConstI64(42), 18));
    let iv1 = next_r;
    next_r += 1;
    bb0.push(Instruction::new(Some(iv1), Type::F64, Op::ConstF64(7.5), 19));
    let im1 = next_r;
    next_r += 1;
    bb0.push(Instruction::new(Some(im1), imap_ty.clone(), Op::MapSet(im0, ik, iv1), 20));
    let ig1 = next_r;
    next_r += 1;
    bb0.push(Instruction::new(Some(ig1), Type::F64, Op::MapGet(im1, ik), 21));
    let im2 = next_r;
    next_r += 1;
    bb0.push(Instruction::new(Some(im2), imap_ty.clone(), Op::MapDel(im1, ik), 22));
    let iv2 = next_r;
    next_r += 1;
    bb0.push(Instruction::new(Some(iv2), Type::F64, Op::ConstF64(12.5), 23));
    let im3 = next_r;
    next_r += 1;
    bb0.push(Instruction::new(Some(im3), imap_ty.clone(), Op::MapSet(im2, ik, iv2), 24));
    let ig2 = next_r;
    next_r += 1;
    bb0.push(Instruction::new(Some(ig2), Type::F64, Op::MapGet(im3, ik), 25));
    bb0.push(Instruction::new(None, Type::Void, Op::Free(im3), 26));

    let fsum = next_r;
    next_r += 1;
    bb0.push(Instruction::new(Some(fsum), Type::F64, Op::Add(ig1, ig2), 27)); // 20.0
    let isum = next_r;
    next_r += 1;
    bb0.push(Instruction::new(Some(isum), Type::I64, Op::FloatToInt(fsum), 28)); // 20

    let s1 = next_r;
    next_r += 1;
    bb0.push(Instruction::new(Some(s1), Type::I64, Op::Add(rv3, rv11), 29)); // 999 + 120 = 1119
    let s2 = next_r;
    bb0.push(Instruction::new(Some(s2), Type::I64, Op::Add(s1, isum), 30)); // 1139
    bb0.terminator = Terminator::Ret(Some(s2));

    f.add_block(bb0);
    module.add_function(f);

    verify_module(&module).expect("map SSA module verifies");
    assert_module_triple_parity_i64(&module, "main", 1139);

    // Additionally verify in Node.js that deleting "k5" actually returns 0 on MapGet("k5")!
    let mut del_mod = Module::new();
    let mut df = Function::new("check_deleted", Type::I64);
    let mut dbb = BasicBlock::new(0, Terminator::Ret(Some(5)));
    dbb.push(Instruction::new(Some(0), map_ty.clone(), Op::MapNew(Type::Str, Type::I64), 1));
    dbb.push(Instruction::new(Some(1), Type::Str, Op::ConstStr("temp".into()), 2));
    dbb.push(Instruction::new(Some(2), Type::I64, Op::ConstI64(777), 3));
    dbb.push(Instruction::new(Some(3), map_ty.clone(), Op::MapSet(0, 1, 2), 4));
    dbb.push(Instruction::new(Some(4), map_ty, Op::MapDel(3, 1), 5));
    dbb.push(Instruction::new(Some(5), Type::I64, Op::MapGet(4, 1), 6));
    df.add_block(dbb);
    del_mod.add_function(df);

    let wasm = WasmBackend::new().compile(&del_mod).expect("compile del_mod");
    if let Some(out) = run_node_wasm_script(&wasm, "console.log(e.check_deleted().toString());") {
        assert_eq!(out.parse::<i64>().unwrap(), 0, "Deleted key must return 0 in WASM");
    }
}

#[test]
fn test_v04_parity_buf_ops_and_f64_rem() {
    // 1. BufAlloc (zero-init) + BufWriteU8 + BufReadU8 + BufSlice parity (VM == Cranelift == WASM)
    let mut buf_mod = Module::new();
    let mut bf = Function::new("main", Type::I64);
    let mut bb = BasicBlock::new(0, Terminator::Ret(Some(16)));
    bb.push(Instruction::new(Some(0), Type::I64, Op::ConstI64(64), 1));
    bb.push(Instruction::new(Some(1), Type::Buf, Op::BufAlloc(0), 2));
    bb.push(Instruction::new(Some(2), Type::I64, Op::ConstI64(10), 3));
    bb.push(Instruction::new(Some(3), Type::I64, Op::ConstI64(200), 4));
    bb.push(Instruction::new(Some(4), Type::Buf, Op::BufWriteU8(1, 2, 3), 5));
    bb.push(Instruction::new(Some(5), Type::I64, Op::ConstI64(15), 6));
    bb.push(Instruction::new(Some(6), Type::I64, Op::ConstI64(55), 7));
    bb.push(Instruction::new(Some(7), Type::Buf, Op::BufWriteU8(4, 5, 6), 8));
    // Verify zero-initialized untouched byte at index 12 == 0
    bb.push(Instruction::new(Some(8), Type::I64, Op::ConstI64(12), 9));
    bb.push(Instruction::new(Some(9), Type::I64, Op::BufReadU8(7, 8), 10));
    // Slice starting at index 10, len 16 -> slice[0] == 200, slice[5] == 55
    bb.push(Instruction::new(Some(10), Type::I64, Op::ConstI64(16), 11));
    bb.push(Instruction::new(Some(11), Type::Buf, Op::BufSlice(7, 2, 10), 12));
    bb.push(Instruction::new(Some(12), Type::I64, Op::ConstI64(0), 13));
    bb.push(Instruction::new(Some(13), Type::I64, Op::ConstI64(5), 14));
    bb.push(Instruction::new(Some(14), Type::I64, Op::BufReadU8(11, 12), 15));
    bb.push(Instruction::new(Some(15), Type::I64, Op::BufReadU8(11, 13), 16));
    bb.push(Instruction::new(Some(16), Type::I64, Op::Add(14, 15), 17));
    bb.push(Instruction::new(Some(17), Type::I64, Op::Add(16, 9), 18));
    bb.terminator = Terminator::Ret(Some(17));
    bf.add_block(bb);
    buf_mod.add_function(bf);
    assert_module_triple_parity_i64(&buf_mod, "main", 255);

    // 2. f64 Op::Rem + IntToFloat + FloatToInt 3-way parity (VM == Cranelift == WASM)
    // Computes: (7.5 % 2.0) * 100 + (-7.5 % 2.0) * 100 + (10.75 % 3.0) * 100
    //         = 150 + (-150) + 175 = 175
    let mut rem_mod = Module::new();
    let mut rf = Function::new("main", Type::I64);
    let mut rbb = BasicBlock::new(0, Terminator::Ret(Some(14)));
    rbb.push(Instruction::new(Some(0), Type::F64, Op::ConstF64(7.5), 1));
    rbb.push(Instruction::new(Some(1), Type::F64, Op::ConstF64(2.0), 2));
    rbb.push(Instruction::new(Some(2), Type::F64, Op::Rem(0, 1), 3)); // 1.5
    rbb.push(Instruction::new(Some(3), Type::F64, Op::ConstF64(-7.5), 4));
    rbb.push(Instruction::new(Some(4), Type::F64, Op::Rem(3, 1), 5)); // -1.5
    rbb.push(Instruction::new(Some(5), Type::F64, Op::ConstF64(10.75), 6));
    rbb.push(Instruction::new(Some(6), Type::F64, Op::ConstF64(3.0), 7));
    rbb.push(Instruction::new(Some(7), Type::F64, Op::Rem(5, 6), 8)); // 1.75
    rbb.push(Instruction::new(Some(8), Type::F64, Op::Add(2, 4), 9)); // 0.0
    rbb.push(Instruction::new(Some(9), Type::F64, Op::Add(8, 7), 10)); // 1.75
    rbb.push(Instruction::new(Some(10), Type::I64, Op::ConstI64(100), 11));
    rbb.push(Instruction::new(Some(11), Type::F64, Op::IntToFloat(10), 12)); // 100.0
    rbb.push(Instruction::new(Some(12), Type::F64, Op::Mul(9, 11), 13)); // 175.0
    rbb.push(Instruction::new(Some(13), Type::F64, Op::Floor(12), 14));
    rbb.push(Instruction::new(Some(14), Type::I64, Op::FloatToInt(13), 15)); // 175
    rf.add_block(rbb);
    rem_mod.add_function(rf);
    assert_module_triple_parity_i64(&rem_mod, "main", 175);
}

// ============================================================================
// 2. WASM ZERO-COST ARENA TELEMETRY TESTS
// ============================================================================

#[test]
fn test_v04_wasm_arena_telemetry_pure_and_heap_modules() {
    // A. Pure scalar module (no heap builtins needed) MUST still export telemetry!
    let scalar_src = r#"
        function add_two(a: i64, b: i64): i64 {
            return a + b;
        }
    "#;
    let prog_s = parse_knr(scalar_src).unwrap();
    let mod_s = lower_program(&prog_s).unwrap();
    let wasm_s = WasmBackend::new().compile(&mod_s).expect("compile scalar wasm");

    if let Some(out) = run_node_wasm_script(
        &wasm_s,
        r#"
        const used0 = e.__knr_heap_used();
        const cap0 = e.__knr_heap_capacity();
        const mark0 = e.__knr_heap_mark();
        const sum = e.add_two(20n, 22n);
        e.__knr_heap_reset(mark0);
        const used1 = e.__knr_heap_used();
        console.log(JSON.stringify({ used0, cap0, mark0, sum: Number(sum), used1 }));
        "#,
    ) {
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["used0"], 0);
        assert_eq!(v["cap0"], 65536);
        assert_eq!(v["mark0"], 65536);
        assert_eq!(v["sum"], 42);
        assert_eq!(v["used1"], 0);
    }

    // B. Heap-allocating module: verify exact byte accounting, arena reset, zero-init, and capacity growth!
    let heap_src = r#"
        type Pair = {
            a: i64,
            b: i64,
            c: i64,
        };

        function alloc_many(n: i64): i64 {
            let v: Pair[] = vec_new();
            let i = 0;
            while (i < n) {
                const p: Pair = { a: i, b: i * 2, c: i * 3 };
                v = vec_push(v, p);
                i = i + 1;
            }
            return vec_len(v);
        }
    "#;
    let prog_h = parse_knr(heap_src).unwrap();
    let mod_h = lower_program(&prog_h).unwrap();
    let wasm_h = WasmBackend::new().compile(&mod_h).expect("compile heap wasm");

    if let Some(out) = run_node_wasm_script(
        &wasm_h,
        r#"
        const used0 = e.__knr_heap_used();
        const cap0 = e.__knr_heap_capacity();
        const mark = e.__knr_heap_mark();
        const n1 = Number(e.alloc_many(10n));
        const used1 = e.__knr_heap_used();
        e.__knr_heap_reset(mark);
        const used_after_reset = e.__knr_heap_used();
        // Force growth beyond initial 4 pages (256 KiB): 12000 structs * 24B > 288 KiB
        const n2 = Number(e.alloc_many(12000n));
        const used_big = e.__knr_heap_used();
        const cap_big = e.__knr_heap_capacity();
        e.__knr_heap_reset(mark);
        const used_final = e.__knr_heap_used();
        console.log(JSON.stringify({
            used0, cap0, n1, used1, used_after_reset, n2, used_big, cap_big, used_final
        }));
        "#,
    ) {
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["used0"], 0);
        assert_eq!(v["cap0"], 262144); // 4 pages * 65536
        assert_eq!(v["n1"], 10);
        assert!(
            v["used1"].as_i64().unwrap() > 240,
            "Expected > 240 bytes allocated for 10 24-byte structs + vec, got {v}"
        );
        assert_eq!(v["used_after_reset"], 0);
        assert_eq!(v["n2"], 12000);
        assert!(v["used_big"].as_i64().unwrap() > 262144);
        assert!(
            v["cap_big"].as_i64().unwrap() > 262144,
            "Capacity must grow when allocations exceed 256 KiB: {v}"
        );
        assert_eq!(v["cap_big"].as_i64().unwrap() % 65536, 0);
        assert_eq!(v["used_final"], 0);
    }
}

// ============================================================================
// 3. MULTI-UNIFORM & RICH-TYPE WGSL + GLSL ES 3.00 SHADER COMPILER TESTS
// ============================================================================

#[test]
fn test_v04_wgsl_16_scalar_uniforms_and_overflow_rejection() {
    let src = r#"
        type Vec2 = { x: f64, y: f64 };
        type Color = { r: f64, g: f64, b: f64, a: f64 };

        @shader
        function sixteen_uniforms(
            uv: Vec2,
            time: f64,
            u0: f64, u1: f64, u2: f64, u3: f64,
            u4: f64, u5: f64, u6: f64, u7: f64,
            u8: i64, u9: i64, u10: f64, u11: f64,
            u12: bool, u13: f64, u14: f64, u15: f64,
        ): Color {
            const sum_a = u0 + u1 + u2 + u3 + u4 + u5 + u6 + u7;
            const sum_b = to_f64(u8 + u9) + u10 + u11 + u13 + u14 + u15;
            const flag = u12 ? 1.0 : 0.25;
            const r = clamp(uv.x * flag + sin(time + sum_a) * 0.5, 0.0, 1.0);
            const g = clamp(uv.y + cos(sum_b) * 0.5, 0.0, 1.0);
            return { r: r, g: g, b: 0.5, a: 1.0 };
        }
    "#;
    let prog = parse_knr(src).expect("parse 16-uniform shader");
    verify_program(&prog).unwrap_or_else(|d| panic!("verify failed: {d:#?}"));
    let (module, smap) = lower_program_with_source_map(&prog).expect("lower");

    let compiled = WgslBackend::new()
        .compile_shader_with_source_map(&module, "sixteen_uniforms", &smap)
        .expect("16 uniforms must compile & pass naga WGSL + GLSL ES 3.00");

    assert_eq!(compiled.custom_param_count, 16);
    assert!(compiled.wgsl.contains("u.params[0].x"));
    assert!(compiled.wgsl.contains("u.params[3].w"));
    assert!(compiled.wgsl.contains("i32(u.params[2].x)"));
    assert!(compiled.wgsl.contains("(u.params[3].x > 0.5)"));
    assert!(compiled.glsl_vertex.starts_with("#version 300 es"));
    assert!(compiled.glsl_fragment.starts_with("#version 300 es"));

    // 17th uniform must fail cleanly
    let overflow_src = r#"
        type Vec2 = { x: f64, y: f64 };
        @shader
        function seventeen_uniforms(
            uv: Vec2, time: f64,
            u0: f64, u1: f64, u2: f64, u3: f64,
            u4: f64, u5: f64, u6: f64, u7: f64,
            u8: f64, u9: f64, u10: f64, u11: f64,
            u12: f64, u13: f64, u14: f64, u15: f64,
            u16: bool,
        ): f64 {
            return uv.x + time + u0 + u1 + u2 + u3 + u4 + u5 + u6 + u7 + u8 + u9 + u10 + u11 + u12 + u13 + u14 + u15 + (u16 ? 1.0 : 0.0);
        }
    "#;
    let p2 = parse_knr(overflow_src).unwrap();
    let (m2, sm2) = lower_program_with_source_map(&p2).unwrap();
    let err = WgslBackend::new()
        .compile_shader_with_source_map(&m2, "seventeen_uniforms", &sm2)
        .expect_err("17 uniform slots must fail");
    assert!(err.to_string().contains("16"));
}

#[test]
fn test_v04_wgsl_vec2_vec3_color_uniforms_and_resolution_dt_auto_binding() {
    let src = r#"
        type Vec2 = { x: f64, y: f64 };
        type Vec3 = { x: f64, y: f64, z: f64 };
        type Color = { r: f64, g: f64, b: f64, a: f64 };

        @shader
        function rich_scene(
            uv: Vec2,
            time: f64,
            dt: f64,
            resolution: Vec2,
            mouse: Vec2,
            offset: Vec2,
            light_dir: Vec3,
            tint: Color,
            gain: f64,
        ): Color {
            const aspect = resolution.x / max(resolution.y, 1.0);
            const px = (uv.x - mouse.x - offset.x) * aspect;
            const py = uv.y - mouse.y - offset.y;
            const ndl = clamp(px * light_dir.x + py * light_dir.y + light_dir.z, 0.0, 1.0);
            const pulse = 0.5 + 0.5 * sin(time + dt * 60.0);
            return {
                r: clamp(tint.r * ndl * gain + pulse * 0.1, 0.0, 1.0),
                g: clamp(tint.g * ndl * gain, 0.0, 1.0),
                b: clamp(tint.b * ndl * gain, 0.0, 1.0),
                a: tint.a,
            };
        }
    "#;
    let prog = parse_knr(src).expect("parse rich_scene");
    verify_program(&prog).unwrap_or_else(|d| panic!("verify rich_scene failed: {d:#?}"));
    let (module, smap) = lower_program_with_source_map(&prog).expect("lower rich_scene");

    let compiled = WgslBackend::new()
        .compile_shader_with_source_map(&module, "rich_scene", &smap)
        .expect("rich_scene must pass naga WGSL + GLSL ES 3.00");

    // Custom slots: offset (2) + light_dir (3) + tint (4) + gain (1) = 10 slots
    assert_eq!(compiled.custom_param_count, 10);
    assert!(compiled.wgsl.contains("knr_s_Vec2(in.uv.x, in.uv.y)"));
    assert!(compiled.wgsl.contains("u.time"));
    assert!(compiled.wgsl.contains("u.dt"));
    assert!(compiled.wgsl.contains("knr_s_Vec2(u.resolution.x, u.resolution.y)"));
    assert!(compiled.wgsl.contains("knr_s_Vec2(u.mouse.x, u.mouse.y)"));
    assert!(compiled.wgsl.contains("knr_s_Vec2(u.params[0].x, u.params[0].y)"));
    assert!(compiled.wgsl.contains("knr_s_Vec3(u.params[0].z, u.params[0].w, u.params[1].x)"));
    assert!(
        compiled
            .wgsl
            .contains("knr_s_Color(u.params[1].y, u.params[1].z, u.params[1].w, u.params[2].x)")
    );
    assert!(compiled.wgsl.contains("u.params[2].y"));
    assert!(compiled.glsl_vertex.starts_with("#version 300 es"));
    assert!(compiled.glsl_fragment.starts_with("#version 300 es"));

    // Also test short alias `res: Vec2` via `compile_shader_with_param_names`
    let compiled_alias = WgslBackend::new()
        .compile_shader_with_param_names(
            &module,
            "rich_scene",
            &[
                "uv", "time", "dt", "res", "mouse", "offset", "light_dir", "tint", "gain",
            ],
        )
        .expect("`res` alias must bind u.resolution cleanly");
    assert!(compiled_alias
        .wgsl
        .contains("knr_s_Vec2(u.resolution.x, u.resolution.y)"));
    assert_eq!(compiled_alias.custom_param_count, 10);
}

#[test]
fn test_v04_wgsl_f64_and_i64_rem_and_all_math_intrinsics() {
    // Construct canonical SSA shader using BOTH i64 Op::Rem and f64 Op::Rem plus every math intrinsic
    let mut module = Module::new();
    let mut vec2 = Schema::new("Vec2");
    vec2.add_field(0, Some("x".into()), Type::F64);
    vec2.add_field(1, Some("y".into()), Type::F64);
    module.add_schema(vec2);

    let mut sh = Function::new("math_and_rem_shader", Type::F64);
    sh.is_shader = true;
    sh.add_param(0, Type::Struct("Vec2".into())); // uv
    sh.add_param(1, Type::F64); // time
    sh.add_param(2, Type::I64); // bands

    let mut bb = BasicBlock::new(0, Terminator::Ret(Some(25)));
    bb.push(Instruction::new(Some(3), Type::F64, Op::StructGet(0, 0), 1)); // x
    bb.push(Instruction::new(Some(4), Type::F64, Op::StructGet(0, 1), 2)); // y
    bb.push(Instruction::new(Some(5), Type::F64, Op::ConstF64(0.5), 3));
    bb.push(Instruction::new(Some(6), Type::F64, Op::Rem(3, 5), 4)); // f64 Rem!
    bb.push(Instruction::new(Some(7), Type::I64, Op::ConstI64(3), 5));
    bb.push(Instruction::new(Some(8), Type::I64, Op::Rem(2, 7), 6)); // i64 Rem!
    bb.push(Instruction::new(Some(9), Type::F64, Op::IntToFloat(8), 7));
    bb.push(Instruction::new(Some(10), Type::F64, Op::Sin(6), 8));
    bb.push(Instruction::new(Some(11), Type::F64, Op::Cos(4), 9));
    bb.push(Instruction::new(Some(12), Type::F64, Op::Tan(6), 10));
    bb.push(Instruction::new(Some(13), Type::F64, Op::Sqrt(3), 11));
    bb.push(Instruction::new(Some(14), Type::F64, Op::Abs(12), 12));
    bb.push(Instruction::new(Some(15), Type::F64, Op::Floor(14), 13));
    bb.push(Instruction::new(Some(16), Type::F64, Op::Ceil(13), 14));
    bb.push(Instruction::new(Some(17), Type::F64, Op::Pow(13, 5), 15));
    bb.push(Instruction::new(Some(18), Type::F64, Op::Atan2(4, 3), 16));
    bb.push(Instruction::new(Some(19), Type::F64, Op::Min(10, 11), 17));
    bb.push(Instruction::new(Some(20), Type::F64, Op::Max(15, 16), 18));
    bb.push(Instruction::new(Some(21), Type::I64, Op::FloatToInt(20), 19));
    bb.push(Instruction::new(Some(22), Type::F64, Op::IntToFloat(21), 20));
    bb.push(Instruction::new(Some(23), Type::F64, Op::Add(17, 18), 21));
    bb.push(Instruction::new(Some(24), Type::F64, Op::Add(19, 22), 22));
    bb.push(Instruction::new(Some(25), Type::F64, Op::Add(23, 24), 23));
    let _ = 9; // silence mental check: reg 9 used below if needed
    bb.push(Instruction::new(Some(26), Type::F64, Op::Add(25, 9), 24));
    bb.terminator = Terminator::Ret(Some(26));
    sh.add_block(bb);
    module.add_function(sh);

    let compiled = WgslBackend::new()
        .compile_shader(&module, "math_and_rem_shader")
        .expect("f64 + i64 Op::Rem and all math intrinsics must pass naga WGSL + GLSL ES 3.00");

    assert!(
        compiled.wgsl.contains("floor((r_3 / r_5))")
            || compiled.wgsl.contains("r_3 - r_5 * floor(r_3 / r_5)"),
        "Expected f64 Rem lowered to a - b * floor(a / b):\n{}",
        compiled.wgsl
    );
    assert!(compiled.wgsl.contains("(r_2 % r_7)"));
    assert!(compiled.glsl_vertex.starts_with("#version 300 es"));
    assert!(compiled.glsl_fragment.starts_with("#version 300 es"));
}

#[test]
fn test_v04_wgsl_tagged_enum_branching_and_equality() {
    let src = r#"
        type Vec2 = { x: f64, y: f64 };
        type Color = { r: f64, g: f64, b: f64, a: f64 };

        enum ShaderMode {
            Gradient,
            Rings(f64),
            Radial(f64, f64),
        }

        enum Palette {
            Ocean,
            Sunset,
        }

        function shade_mode(uv: Vec2, time: f64, mode: ShaderMode, pal: Palette): Color {
            let v = 0.0;
            switch (mode) {
                case ShaderMode.Gradient:
                    v = uv.x * 0.5 + uv.y * 0.5;
                    break;
                case ShaderMode.Rings(freq):
                    v = 0.5 + 0.5 * sin(sqrt(uv.x * uv.x + uv.y * uv.y) * freq - time);
                    break;
                case ShaderMode.Radial(cx, cy):
                    const dx = uv.x - cx;
                    const dy = uv.y - cy;
                    v = clamp(1.0 - sqrt(dx * dx + dy * dy) * 2.0, 0.0, 1.0);
                    break;
            }
            if (pal === Palette.Ocean) {
                return { r: 0.1 * v, g: 0.6 * v, b: 0.95 * v, a: 1.0 };
            }
            return { r: 0.95 * v, g: 0.45 * v, b: 0.2 * v, a: 1.0 };
        }

        @shader
        function enum_kernel(uv: Vec2, time: f64, mode_id: i64, freq: f64, pal: Palette): Color {
            const m = mode_id === 0
                ? ShaderMode.Gradient
                : mode_id === 1
                    ? ShaderMode.Rings(freq)
                    : ShaderMode.Radial(0.5, 0.5);
            return shade_mode(uv, time, m, pal);
        }
    "#;
    let prog = parse_knr(src).expect("parse enum shader");
    verify_program(&prog).unwrap_or_else(|d| panic!("verify enum shader failed: {d:#?}"));
    let (module, smap) = lower_program_with_source_map(&prog).expect("lower enum shader");

    let compiled = WgslBackend::new()
        .compile_shader_with_source_map(&module, "enum_kernel", &smap)
        .expect("Tagged enum branching & equality in @shader must pass naga WGSL + GLSL ES 3.00");

    // Custom params: mode_id (1) + freq (1) + pal (unit enum with 1 __tag field = 1) => 3 slots
    assert_eq!(compiled.custom_param_count, 3);
    assert!(compiled.wgsl.contains("struct knr_s_ShaderMode"));
    assert!(compiled.wgsl.contains("struct knr_s_Palette"));
    assert!(compiled.wgsl.contains(".f0 =="));
    assert!(compiled.glsl_vertex.starts_with("#version 300 es"));
    assert!(compiled.glsl_fragment.starts_with("#version 300 es"));
}
