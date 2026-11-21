//! Comprehensive end-to-end parity test suite proving:
//!   VM (knr-interp) == Cranelift JIT (knr-codegen) == WebAssembly (executed in Node.js!)
//! plus static verification of second-class borrows (&T), exhaustive enum match,
//! block-parameter loops (while/break/continue), strings, structs, vectors,
//! multi-file .knr imports, and 1:1 .knrbin roundtrip bijection.

use knr_codegen::{CraneliftBackend, WasmBackend};
use knr_core::syntax::parse_knr;
use knr_core::{decode_knrbin, encode_knrbin, format_ssa, lower_program, parse_ssa};
use knr_interp::{Value, Vm};
use knr_runtime::mock::MockPlatform;
use knr_verify::{verify_module, verify_program, verify_program_from_file};
use std::process::Command;

/// Compile and verify `.knr` source, assert `.ssa` text + `.knrbin` binary 1:1 roundtrip,
/// execute `entry` in VM, execute `entry` in Cranelift JIT, AND execute `entry` in real
/// WebAssembly via Node.js (with ZERO external JS imports!).
fn assert_full_triple_parity_i64(src: &str, entry: &str, expected: i64) {
    let prog = parse_knr(src).expect("parse .knr");
    verify_program(&prog).unwrap_or_else(|diags| {
        panic!("verify_program failed:\n{diags:#?}");
    });
    let module = lower_program(&prog).expect("lower .knr");

    // 1. Canonical SSA text roundtrip
    let ssa_text = format_ssa(&module);
    let reparsed = parse_ssa(&ssa_text).unwrap_or_else(|e| {
        panic!("SSA roundtrip parse failed: {e}\n--- SSA ---\n{ssa_text}");
    });
    verify_module(&reparsed).expect("reparsed SSA must verify cleanly");

    // 2. Canonical .knrbin binary roundtrip bijection
    let bin = encode_knrbin(&module).expect("encode .knrbin");
    let decoded = decode_knrbin(&bin).expect("decode .knrbin");
    assert_eq!(
        format_ssa(&module),
        format_ssa(&decoded),
        ".knrbin roundtrip must be a 1:1 canonical bijection"
    );

    // 3. Interpreter VM execution
    let mut host = MockPlatform::new();
    let entry_fn = module.get_function(entry).expect("entry fn exists");
    let mut vm = Vm::new(&mut host, Some(&module), 1_000_000, false);
    let (vm_val, _) = vm.run(entry_fn, vec![]).expect("VM execution");
    assert_eq!(
        vm_val,
        Value::I64(expected),
        "VM result mismatch for {entry} (got {vm_val:?}, expected {expected})\n--- SSA ---\n{ssa_text}"
    );

    // 4. Cranelift native JIT execution
    let mut cl = CraneliftBackend::new().expect("init CraneliftBackend");
    let fn_ptr = cl
        .compile_jit_module(&module, entry)
        .expect("compile Cranelift JIT module");
    let jit_fn: extern "C" fn() -> i64 = unsafe { std::mem::transmute(fn_ptr) };
    let jit_val = jit_fn();
    assert_eq!(
        jit_val, expected,
        "Cranelift JIT result mismatch for {entry} (got {jit_val}, expected {expected})"
    );

    // 5. Real WebAssembly execution (ZERO host imports!) via Node.js
    let wasm_bytes = WasmBackend::new().compile(&module).expect("compile WASM");
    assert_eq!(&wasm_bytes[0..4], b"\0asm", "valid WASM magic");
    if let Some(wasm_val) = run_wasm_in_node_i64(&wasm_bytes, entry) {
        assert_eq!(
            wasm_val, expected,
            "WASM runtime result mismatch for {entry} (got {wasm_val}, expected {expected})"
        );
    }
}

fn run_wasm_in_node_i64(wasm_bytes: &[u8], entry: &str) -> Option<i64> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    let tmp = std::env::temp_dir().join(format!(
        "knr_wasm_parity_{}_{}_{}.wasm",
        std::process::id(),
        seq,
        entry
    ));
    std::fs::write(&tmp, wasm_bytes).ok()?;
    let js = format!(
        "const fs = require('fs'); \
         const bytes = fs.readFileSync({}); \
         WebAssembly.instantiate(bytes, {{}}).then(({{instance}}) => {{ \
           const res = instance.exports[{}](); \
           console.log(res.toString()); \
         }}).catch(e => {{ console.error(e); process.exit(1); }});",
        serde_json::to_string(tmp.to_str().unwrap()).unwrap(),
        serde_json::to_string(entry).unwrap(),
    );
    let out = Command::new("node").arg("-e").arg(&js).output();
    let _ = std::fs::remove_file(&tmp);
    let out = match out {
        Ok(o) => o,
        Err(_) => return None, // node not installed
    };
    assert!(
        out.status.success(),
        "Node.js WASM execution failed for {entry}:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    Some(text.parse::<i64>().expect("parse WASM i64 stdout"))
}

#[test]
fn test_parity_while_loop_break_continue_modulo_short_circuit() {
    let src = r#"
        fn main() -> i64 {
            let i = 0;
            let sum = 0;
            while i < 30 {
                i = i + 1;
                if i % 2 == 0 {
                    continue;
                }
                if i > 20 && sum > 50 {
                    break;
                }
                if i == 3 || i == 7 {
                    sum = sum + i * 10;
                } else {
                    sum = sum + i;
                }
            }
            // Odd i from 1..=21:
            // i=1 (+1=1), i=3 (+30=31), i=5 (+5=36), i=7 (+70=106), i=9 (+9=115),
            // i=11 (+11=126), i=13 (+13=139), i=15 (+15=154), i=17 (+17=171),
            // i=19 (+19=190), i=21 -> i>20 && sum>50 -> break!
            return sum;
        }
    "#;
    assert_full_triple_parity_i64(src, "main", 190);
}

#[test]
fn test_parity_enum_and_exhaustive_match_expr_and_stmt() {
    let src = r#"
        enum Action {
            Inc(i64),
            Dec(i64),
            Double,
            Reset,
        }

        fn apply(state: i64, act: Action) -> i64 {
            let next = match act {
                Action::Inc(n) => state + n,
                Action::Dec(n) => state - n,
                Action::Double => state * 2,
                Action::Reset => 0,
            };
            return next;
        }

        fn main() -> i64 {
            let s = 10;
            s = apply(s, Action::Inc(5));
            s = apply(s, Action::Double);
            s = apply(s, Action::Dec(8));
            let bonus = if s > 20 { 100 } else { 0 };
            return s + bonus;
        }
    "#;
    assert_full_triple_parity_i64(src, "main", 122);
}

#[test]
fn test_parity_second_class_borrows_structs_vectors_strings_and_recursion() {
    let src = r#"
        struct Point {
            0: x i64,
            1: y i64,
            2: label str,
        }

        fn manhattan(p: &Point) -> i64 {
            return p.x + p.y + str_len(p.label);
        }

        fn fact(n: i64) -> i64 {
            if n <= 1 {
                return 1;
            }
            return n * fact(n - 1);
        }

        fn main() -> i64 {
            let tag = "pt-" + to_str(42);
            let p = Point { x: 10, y: 25, label: tag };
            // Borrow p twice without moving ownership!
            let d1 = manhattan(&p);
            let d2 = manhattan(&p);
            let eq_bonus = if p.label == "pt-42" { 1000 } else { 0 };

            // Build a vector dynamically inside a while loop
            let v: vec[i64] = vec_new();
            let k = 1;
            while k <= 5 {
                v = vec_push(v, k * k);
                k = k + 1;
            }
            let total = 0;
            let idx = 0;
            let n = vec_len(v);
            while idx < n {
                total = total + v[idx];
                idx = idx + 1;
            }

            // d1 = 10 + 25 + 5 = 40; d2 = 40; eq_bonus = 1000;
            // total = 1+4+9+16+25 = 55; fact(5) = 120
            // 40 + 40 + 1000 + 55 + 120 = 1255
            return d1 + d2 + eq_bonus + total + fact(5);
        }
    "#;
    assert_full_triple_parity_i64(src, "main", 1255);
}

#[test]
fn test_static_rejects_non_exhaustive_enum_match() {
    let src = r#"
        enum Color {
            Red,
            Green,
            Blue,
        }

        fn score(c: Color) -> i64 {
            return match c {
                Color::Red => 1,
                Color::Green => 2,
            };
        }
    "#;
    let prog = parse_knr(src).expect("parse");
    let err = verify_program(&prog).expect_err("Non-exhaustive match must be rejected");
    let combined: String = err.iter().map(|d| d.to_string()).collect::<Vec<_>>().join(" ");
    assert!(
        combined.contains("Blue") || combined.to_lowercase().contains("exhaust"),
        "Diagnostic must mention missing variant Blue: {err:#?}"
    );
}

#[test]
fn test_static_rejects_second_class_borrow_escape() {
    let src = r#"
        struct Boxed {
            0: v i64,
        }

        fn leak_borrow(b: &Boxed) -> Boxed {
            return b;
        }
    "#;
    let prog = parse_knr(src).expect("parse");
    let err = verify_program(&prog).expect_err("Returning a borrowed &T parameter must be rejected");
    let combined: String = err
        .iter()
        .map(|d| format!("{}: {}", d.error_kind, d))
        .collect::<Vec<_>>()
        .join(" | ");
    assert!(
        combined.contains("BorrowEscape") || combined.to_lowercase().contains("borrow"),
        "Diagnostic must report BorrowEscape: {combined}"
    );
}

#[test]
fn test_multifile_knr_import_resolution_and_parity() {
    let dir = std::env::temp_dir().join(format!("knr_multifile_parity_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    let helper_path = dir.join("math_lib.knr");
    let main_path = dir.join("app.knr");

    std::fs::write(
        &helper_path,
        r#"
        struct Acc {
            0: val i64,
        }

        fn peek(a: &Acc) -> i64 {
            return a.val * 3;
        }
        "#,
    )
    .unwrap();

    std::fs::write(
        &main_path,
        r#"
        import "./math_lib.knr";

        fn main() -> i64 {
            let a = Acc { val: 14 };
            return peek(&a);
        }
        "#,
    )
    .unwrap();

    let module = verify_program_from_file(&main_path).expect("verify multi-file .knr");
    let mut host = MockPlatform::new();
    let entry_fn = module.get_function("main").unwrap();
    let mut vm = Vm::new(&mut host, Some(&module), 1_000_000, false);
    let (vm_val, _) = vm.run(entry_fn, vec![]).unwrap();
    assert_eq!(vm_val, Value::I64(42));

    let mut cl = CraneliftBackend::new().unwrap();
    let fn_ptr = cl.compile_jit_module(&module, "main").unwrap();
    let f: extern "C" fn() -> i64 = unsafe { std::mem::transmute(fn_ptr) };
    assert_eq!(f(), 42);

    let wasm = WasmBackend::new().compile(&module).unwrap();
    if let Some(w) = run_wasm_in_node_i64(&wasm, "main") {
        assert_eq!(w, 42);
    }

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_parity_math_intrinsics_hex_colors_and_struct_spread() {
    let src = r##"
        schema Color {
            0: r f64,
            1: g f64,
            2: b f64,
            3: a f64,
        }

        schema StyleBox {
            0: w f64,
            1: h f64,
            2: bg str,
            3: radius f64,
        }

        fn base_box() -> StyleBox {
            return StyleBox {
                w: 100.0,
                h: 50.0,
                bg: #0f172a,
                radius: 8.0,
            };
        }

        fn main() -> i64 {
            // Math intrinsics (pure zero-import WASM + Cranelift + VM parity!)
            let s = sin(1.0);
            let c = cos(1.0);
            let pyth = s * s + c * c; // ~1.0
            let t = tan(0.5); // ~0.5463
            let sq = sqrt(16.0); // 4.0
            let ab = abs(-7.5); // 7.5
            let fl = floor(3.9); // 3.0
            let cl = ceil(3.1); // 4.0
            let fr = fract(3.25); // 0.25
            let pw = pow(2.0, 5.0); // 32.0
            let at = atan2(1.0, 1.0); // pi/4 ~0.7854
            let mn = min(9.0, 4.0); // 4.0
            let mx = max(9.0, 4.0); // 9.0
            let cm = clamp(15.0, 0.0, 10.0); // 10.0
            let lr = lerp(10.0, 20.0, 0.25); // 12.5
            let sm = smoothstep(0.0, 1.0, 0.5); // 0.5

            // Hex color literal -> typed Color struct & str literal
            let sky: Color = #38bdf8;
            let box1 = StyleBox { w: 240.0, bg: #38bdf8, ..base_box() };
            let bg_ok = if box1.bg == "#38bdf8" { 100.0 } else { 0.0 };

            // Combine into deterministic scaled integer to check exact cross-backend parity
            let sum = pyth * 100.0
                + t * 100.0
                + sq
                + ab
                + fl
                + cl
                + fr * 100.0
                + pw
                + at * 100.0
                + mn
                + mx
                + cm
                + lr
                + sm * 100.0
                + sky.b * 100.0
                + box1.w
                + box1.h
                + box1.radius
                + bg_ok;
            return to_i64(floor(sum));
        }
    "##;
    // Expected breakdown:
    // pyth*100 = 100.0
    // tan(0.5)*100 = 54.6302
    // sq(4) + ab(7.5) + fl(3) + cl(4) = 18.5
    // fr*100 = 25.0
    // pw = 32.0
    // atan2(1,1)*100 = 78.5398
    // mn(4) + mx(9) + cm(10) + lr(12.5) = 35.5
    // sm*100 = 50.0
    // sky.b*100 = (248/255)*100 = 97.2549
    // box1.w(240) + box1.h(50) + box1.radius(8) + bg_ok(100) = 398.0
    // Total = 100 + 54.6302 + 18.5 + 25 + 32 + 78.5398 + 35.5 + 50 + 97.2549 + 398 = 889.4249 -> floor = 889
    assert_full_triple_parity_i64(src, "main", 889);
}
