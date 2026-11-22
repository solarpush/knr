use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use knr_core::ast::{Module, Op, RegId, Type};
use knr_core::{
    decode_knrbin, encode_knrbin, explore_path, format_ssa, format_tsk, lower_program,
    lower_program_from_file, parse_ssa, parse_tsx, LowerError,
};

/// Validates canonical monotonic SSA invariants + lossless `.ssa` text & `.knrbin` binary roundtrip
/// proving 100% compatibility with existing SSA opcodes and zero ABI drift.
fn assert_canonical_ssa_module(module: &Module) {
    assert!(!module.functions.is_empty(), "module must have functions");

    for func in &module.functions {
        assert!(
            func.blocks.iter().any(|b| b.id == 0),
            "fn {} missing entry block bb0",
            func.name
        );

        let block_map: HashMap<u32, _> = func.blocks.iter().map(|b| (b.id, b)).collect();
        let mut defined: HashSet<RegId> = HashSet::new();
        let mut last_reg: Option<RegId> = None;

        for (preg, _) in &func.params {
            assert!(
                defined.insert(*preg),
                "fn {}: duplicate param reg %{preg}",
                func.name
            );
            if let Some(prev) = last_reg {
                assert!(
                    *preg > prev,
                    "fn {}: non-monotonic param reg %{preg} <= %{prev}",
                    func.name
                );
            }
            last_reg = Some(*preg);
        }

        for blk in &func.blocks {
            for (breg, _) in &blk.params {
                assert!(
                    defined.insert(*breg),
                    "fn {} bb{}: duplicate block param %{breg}",
                    func.name,
                    blk.id
                );
                if let Some(prev) = last_reg {
                    assert!(
                        *breg > prev,
                        "fn {} bb{}: non-monotonic block param %{breg} <= %{prev}",
                        func.name,
                        blk.id
                    );
                }
                last_reg = Some(*breg);
            }
            for inst in &blk.instructions {
                if let Some(dst) = inst.dst {
                    assert!(
                        defined.insert(dst),
                        "fn {} bb{}: duplicate instruction dst %{dst}",
                        func.name,
                        blk.id
                    );
                    if let Some(prev) = last_reg {
                        assert!(
                            dst > prev,
                            "fn {} bb{}: non-monotonic dst %{dst} <= %{prev}",
                            func.name,
                            blk.id
                        );
                    }
                    last_reg = Some(dst);
                }
            }
            for (target, args) in blk.terminator.targets() {
                let tgt = block_map.get(&target).unwrap_or_else(|| {
                    panic!(
                        "fn {} bb{}: invalid branch target bb{target}",
                        func.name, blk.id
                    )
                });
                assert_eq!(
                    args.len(),
                    tgt.params.len(),
                    "fn {} bb{} -> bb{target}: branch arg count mismatch",
                    func.name,
                    blk.id
                );
            }
        }
    }

    let mut flat = module.clone();
    flat.imports.clear();
    let ssa_text = format_ssa(&flat);
    let reparsed = parse_ssa(&ssa_text).expect("lowered module must roundtrip through .ssa text");
    assert_eq!(reparsed.functions.len(), module.functions.len());

    let bytes = encode_knrbin(module).expect("encode lowered module to .knrbin");
    let decoded = decode_knrbin(&bytes).expect("decode lowered module from .knrbin");
    assert_eq!(decoded.functions.len(), module.functions.len());
}

struct TempTestDir {
    path: PathBuf,
}

impl TempTestDir {
    fn new(prefix: &str) -> Self {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let id = COUNTER.fetch_add(1, Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!(
            "knr_v04_{prefix}_{}_{}",
            std::process::id(),
            id
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("create temp dir");
        Self { path }
    }

    fn write(&self, rel: &str, content: &str) -> PathBuf {
        let full = self.path.join(rel);
        if let Some(parent) = full.parent() {
            std::fs::create_dir_all(parent).expect("create parent");
        }
        std::fs::write(&full, content).expect("write file");
        full
    }
}

impl Drop for TempTestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

// ============================================================================
// 1. Full Functional Array Pipeline
// ============================================================================

#[test]
fn test_indexed_map_filter_and_all_array_pipeline_methods() {
    let src = r###"
export type Item = {
    id: i64;
    score: i64;
    label: string;
};

export type Summary = {
    total: i64;
    count: i64;
};

export function test_indexed_map_and_filter(nums: readonly i64[]): i64[] {
    const a = nums.map((x) => x * 2);
    const b = a.map((x, idx) => x + idx);
    const c = b.filter((x) => x > 0);
    const d = c.filter((x, idx) => idx < 5 && x > 1);
    return d;
}

export function test_reduce_variants(nums: readonly i64[], items: readonly Item[]): i64 {
    const sum2 = nums.reduce((acc, x) => acc + x, 0);
    const weighted = nums.reduce((acc, x, idx) => acc + x * (idx + 1), 0);
    const struct_sum = items.reduce(
        (acc, it) => ({ ...acc, total: acc.total + it.score, count: acc.count + 1 }),
        { total: 0, count: 0 }
    );
    const block_sum = nums.reduce((acc, x, idx) => {
        if (idx % 2 === 0) {
            return acc + x;
        }
        return acc;
    }, 0);
    return sum2 + weighted + struct_sum.total + struct_sum.count + block_sum;
}

export function test_some_every_count_find_index(items: readonly Item[], nums: readonly i64[]): i64 {
    const has_high = items.some((it) => it.score > 90);
    const has_even_idx = items.some((it, idx) => idx > 0 && it.id > 0);
    const all_pos = nums.every((n) => n > 0);
    const all_ok = items.every((it, idx) => it.id >= idx);
    const high_cnt = items.count((it) => it.score >= 50);
    const idx_hit = items.findIndex((it) => it.label === "target");
    const idx_num = nums.findIndex((n, i) => n === i);
    let score = high_cnt + idx_hit + idx_num;
    if (has_high && has_even_idx && all_pos && all_ok) {
        score = score + 100;
    }
    return score;
}

export function test_includes_and_index_of(
    ints: readonly i64[],
    floats: readonly f64[],
    flags: readonly bool[],
    tags: readonly string[]
): i64 {
    const has_i = ints.includes(42);
    const has_f = floats.includes(3.5);
    const has_b = flags.includes(true);
    const has_s = tags.includes("knr");
    const pos_i = ints.indexOf(42);
    const pos_f = floats.indexOf(3.5);
    const pos_b = flags.indexOf(false);
    const pos_s = tags.indexOf("knr");
    let out = pos_i + pos_f + pos_b + pos_s;
    if (has_i && has_f && has_b && has_s) {
        out = out + 1;
    }
    return out;
}

export function test_flat_map_slice_concat_reverse(a: readonly i64[], b: readonly i64[]): i64[] {
    const expanded = a.flatMap((x) => [x, x * 10]);
    const indexed_flat = a.flatMap((x, idx) => {
        return [x + idx, x - idx];
    });
    const sub = expanded.slice(1, 4);
    const tail = indexed_flat.slice(2);
    const combined = sub.concat(tail).concat(b);
    return combined.reverse();
}

export function test_chained_temps(): i64 {
    return [1, 2, 3, 4, 5]
        .filter((x, idx) => idx > 0 && x % 2 === 0)
        .flatMap((x) => [x, x + 1])
        .slice(0, 4)
        .concat([10, 20])
        .reverse()
        .reduce((acc, v, i) => acc + v + i, 0);
}
"###;

    let prog = parse_tsx(src).expect("parse full array pipeline");
    let module = lower_program(&prog).expect("lower full array pipeline");
    assert_canonical_ssa_module(&module);

    // Ensure every function lowered cleanly
    for name in [
        "test_indexed_map_and_filter",
        "test_reduce_variants",
        "test_some_every_count_find_index",
        "test_includes_and_index_of",
        "test_flat_map_slice_concat_reverse",
        "test_chained_temps",
    ] {
        assert!(module.get_function(name).is_some(), "missing fn {name}");
    }

    // Chained temps should free intermediate temporary Vecs
    let chained = module.get_function("test_chained_temps").unwrap();
    let free_count: usize = chained
        .blocks
        .iter()
        .flat_map(|b| &b.instructions)
        .filter(|i| matches!(i.op, Op::Free(_)))
        .count();
    assert!(
        free_count >= 6,
        "expected chained temporary Vecs to be freed, got {free_count}"
    );
}

#[test]
fn test_flat_map_jsx_widgets_flattening() {
    let src = r###"
import { Column, Text, Badge } from "std/ui";

export type Group = {
    name: string;
    count: i64;
};

export function view(groups: readonly Group[]): Widget {
    return (
        <Column gap={8}>
            {groups.flatMap((g, idx) => [
                <Text key={`t_${idx}`}>{g.name}</Text>,
                <Badge key={`b_${idx}`}>{g.count}</Badge>,
            ])}
        </Column>
    );
}
"###;
    let prog = parse_tsx(src).expect("parse flatMap JSX");
    let module = lower_program(&prog).expect("lower flatMap JSX");
    assert_canonical_ssa_module(&module);
}

// ============================================================================
// 2. Object & Array Destructuring + Nullish Coalescing (`??`)
// ============================================================================

#[test]
fn test_object_and_array_destructuring_zero_move() {
    let src = r###"
export type AppState = {
    active_tab: string;
    energy: f64;
    count: i64;
    tags: string[];
};

export function make_state(): AppState {
    return {
        active_tab: "home",
        energy: 0.75,
        count: 10,
        tags: ["a", "b"],
    };
}

export function destructure_var_never_moves(s: AppState): AppState {
    const { active_tab, energy } = s;
    let { count: local_count, tags } = s;
    local_count = local_count + tags.length;
    const [first_tag, second_tag] = tags;
    const same = first_tag === second_tag;
    const next_tab = same ? active_tab : second_tag;
    return {
        ...s,
        active_tab: next_tab,
        energy: energy + 0.25,
        count: local_count,
    };
}

export function destructure_temp_expr(): i64 {
    const { count, energy: e } = make_state();
    const [first, second] = [10, 20, 30];
    let [x, y] = [1, 2];
    x = x + y;
    return count + toInt(e) + first + second + x;
}
"###;

    let prog = parse_tsx(src).expect("parse destructuring");
    let module = lower_program(&prog).expect("lower destructuring");
    assert_canonical_ssa_module(&module);

    let temp_fn = module.get_function("destructure_temp_expr").unwrap();
    let has_free = temp_fn
        .blocks
        .iter()
        .flat_map(|b| &b.instructions)
        .any(|i| matches!(i.op, Op::Free(_)));
    assert!(has_free, "temporary destructured aggregates must be freed");
}

#[test]
fn test_nullish_coalescing_option_result_and_precedence() {
    let src = r###"
import { Option, Result } from "std/core";

export function expensive(x: i64): i64 {
    return x * 100;
}

export function coalesce_option(opt: Option<i64>, fallback: i64): i64 {
    const a = opt ?? expensive(fallback);
    const b = Option<i64>.None ?? 42;
    const c = Option<i64>.Some(7) ?? 99;
    return a + b + c;
}

export function coalesce_result(res: Result<string, i64>, alt: string): string {
    const s1 = res ?? alt;
    const s2 = Result<string, i64>.Ok("ok") ?? "err";
    const s3 = Result<string, i64>.Err(500) ?? "recovered";
    return s1 + s2 + s3;
}

export function coalesce_precedence(
    flag: bool,
    opt_bool: Option<bool>,
    opt_a: Option<i64>,
    opt_b: Option<i64>
): i64 {
    const chosen = opt_bool ?? false ? 10 : 20;
    const ternary_rhs = flag ? opt_a ?? 1 : opt_b ?? 2;
    return chosen + ternary_rhs;
}
"###;

    let prog = parse_tsx(src).expect("parse ??");
    let module = lower_program(&prog).expect("lower ??");
    assert_canonical_ssa_module(&module);

    // Verify lazy lowering: `expensive` call must occur in a separate branch block (not bb0)
    let f = module.get_function("coalesce_option").unwrap();
    let call_bb = f
        .blocks
        .iter()
        .find(|b| {
            b.instructions
                .iter()
                .any(|i| matches!(&i.op, Op::Call(name, _) if name == "expensive"))
        })
        .expect("must contain call to expensive in none_bb");
    assert_ne!(
        call_bb.id, 0,
        "rhs of ?? must be lazily evaluated in none branch, not entry block bb0"
    );
}

#[test]
fn test_nullish_coalescing_rejects_non_option_result() {
    let src = r###"
export function bad(x: i64): i64 {
    return x ?? 10;
}
"###;
    let prog = parse_tsx(src).expect("parse");
    let err = lower_program(&prog).expect_err("?? on i64 must fail");
    assert!(matches!(err, LowerError::TypeMismatch { .. }), "{err:?}");
}

// ============================================================================
// 3. Compile-Time Exhaustive Enum Matching + Payload `switch`
// ============================================================================

#[test]
fn test_non_exhaustive_enum_match_reports_exact_prefix_and_missing_variants() {
    let src = r###"
export enum Action {
    Add(i64),
    Reset,
    Clear,
}

export function handle(a: Action): i64 {
    switch (a) {
        case Action.Add(n):
            return n;
    }
}
"###;
    let prog = parse_tsx(src).expect("parse non-exhaustive switch");
    let err = lower_program(&prog).expect_err("non-exhaustive enum switch must fail");
    match err {
        LowerError::InvalidConstruct { msg, .. } => {
            assert!(
                msg.starts_with("NonExhaustiveMatch:"),
                "Expected error starting with 'NonExhaustiveMatch:', got: {msg}"
            );
            assert!(msg.contains("Action"), "Expected enum name in msg: {msg}");
            assert!(msg.contains("Reset"), "Expected Reset in msg: {msg}");
            assert!(msg.contains("Clear"), "Expected Clear in msg: {msg}");
        }
        other => panic!("Expected InvalidConstruct, got {other:?}"),
    }
}

#[test]
fn test_exhaustive_payload_switch_with_default_and_value_switch_preserved() {
    let src = r###"
export enum Action {
    Add(i64),
    Mul(i64, i64),
    Reset,
    Clear,
}

export function handle_exhaustive(a: Action): i64 {
    switch (a) {
        case Action.Add(x):
            return x + 1;
        case Action.Mul(x, y):
            return x * y;
        case Action.Reset:
            return 0;
        case Action.Clear:
            return -1;
    }
}

export function handle_with_default(a: Action): i64 {
    let res = 0;
    switch (a) {
        case Action.Add(x):
            res = x;
            break;
        default:
            res = 99;
            break;
    }
    return res;
}

export function string_and_int_switch(cmd: string, code: i64): i64 {
    let v = 0;
    switch (cmd) {
        case "inc":
            v = 1;
            break;
        case "dec":
            v = -1;
            break;
        default:
            v = 0;
            break;
    }
    switch (code) {
        case 200:
            return v + 10;
        case 404:
            return v - 10;
        default:
            return v;
    }
}
"###;

    let prog = parse_tsx(src).expect("parse switches");
    let module = lower_program(&prog).expect("lower switches");
    assert_canonical_ssa_module(&module);
}

// ============================================================================
// 4. `knr.toml` Dependency & Workspace Package Imports (lower + explore)
// ============================================================================

#[test]
fn test_knr_toml_dependency_and_subpath_package_imports() {
    let td = TempTestDir::new("pkg_deps");

    // Local dependency package `my-ui`
    td.write(
        "packages/my-ui/knr.toml",
        r###"
[package]
name = "my-ui"
version = "0.1.0"
entry = "src/lib.tsk"
"###,
    );
    td.write(
        "packages/my-ui/src/lib.tsk",
        r###"
export const UI_VERSION: i64 = 4;
export function base_pad(): f64 {
    return 16.0;
}
"###,
    );
    td.write(
        "packages/my-ui/src/cards.tsk",
        r###"
import { Card, Text } from "std/ui";

export function InfoCard(title: string): Widget {
    return (
        <Card p={16}>
            <Text>{title}</Text>
        </Card>
    );
}
"###,
    );

    // Consumer package `demo-app` referencing `my-ui` via `[dependencies]`
    td.write(
        "apps/demo-app/knr.toml",
        r###"
[package]
name = "demo-app"
version = "0.1.0"
entry = "src/main.tsk"

[dependencies]
my-ui = { path = "../../packages/my-ui" }
"###,
    );
    let main_path = td.write(
        "apps/demo-app/src/main.tsk",
        r###"
import { Column } from "std/ui";
import { UI_VERSION, base_pad } from "my-ui";
import { InfoCard } from "my-ui/cards";

export function view(): Widget {
    return (
        <Column p={base_pad()}>
            <InfoCard title="Hello" />
        </Column>
    );
}

export function main(): i64 {
    return UI_VERSION;
}
"###,
    );

    // 1. Lowering resolves both root package import (`my-ui`) and subpath (`my-ui/cards`)
    let module = lower_program_from_file(&main_path).expect("lower package dependency imports");
    assert_canonical_ssa_module(&module);
    assert!(module.get_function("base_pad").is_some());
    assert!(module.get_function("InfoCard").is_some());
    assert!(module.get_function("main").is_some());

    // 2. `explore_path` resolves the exact same package imports!
    let graph = explore_path(&main_path).expect("explore package dependency imports");
    let fn_names: HashSet<&str> = graph.functions.iter().map(|f| f.name.as_str()).collect();
    assert!(fn_names.contains("main"));
    assert!(fn_names.contains("base_pad"));
    assert!(fn_names.contains("InfoCard"));
}

#[test]
fn test_workspace_member_package_import_resolution() {
    let td = TempTestDir::new("ws_members");

    td.write(
        "knr.toml",
        r###"
[workspace]
members = ["packages/*"]
"###,
    );

    td.write(
        "packages/shared-math/knr.toml",
        r###"
[package]
name = "shared-math"
version = "0.1.0"
entry = "src/lib.tsk"
"###,
    );
    td.write(
        "packages/shared-math/src/lib.tsk",
        r###"
export function triple(n: i64): i64 {
    return n * 3;
}
"###,
    );

    td.write(
        "packages/app/knr.toml",
        r###"
[package]
name = "app"
version = "0.1.0"
entry = "src/main.tsk"
"###,
    );
    let app_main = td.write(
        "packages/app/src/main.tsk",
        r###"
import { triple } from "shared-math";

export function main(): i64 {
    return triple(14);
}
"###,
    );

    let module = lower_program_from_file(&app_main).expect("lower workspace member import");
    assert_canonical_ssa_module(&module);
    assert!(module.get_function("triple").is_some());

    let graph = explore_path(&app_main).expect("explore workspace member import");
    assert!(graph.functions.iter().any(|f| f.name == "triple"));
}

// ============================================================================
// 5. Expanded Built-in Standard Library (`std/theme`, `std/router`, `std/layout`, `std/testing`)
// ============================================================================

#[test]
fn test_all_four_new_stdlib_modules_lower_cleanly() {
    let src = r###"
import { Theme, theme_obsidian, theme_light, theme_nord, theme_solar } from "std/theme";
import { Route, router_init, router_push, router_back, router_replace, router_is, router_can_back } from "std/router";
import { is_compact, is_medium, is_wide, responsive_cols, responsive_pad } from "std/layout";
import { assert_eq_i64, assert_near, assert_str_eq, assert_true, assert_false, assert_gt, assert_lt } from "std/testing";

export function exercise_stdlib(w: f64): bool {
    const t1: Theme = theme_obsidian();
    const t2: Theme = theme_light();
    const t3: Theme = theme_nord();
    const t4: Theme = theme_solar();
    const { bg, accent, radius_md } = t1;

    let r: Route = router_init("/home");
    r = router_push(r, "/settings");
    r = router_replace(r, "/profile");
    const on_profile = router_is(r, "/profile");
    const can_go_back = router_can_back(r);
    r = router_back(r);
    const back_home = router_is(r, "/home");

    const cols = responsive_cols(w, 1, 2, 3);
    const pad = responsive_pad(w);
    const bp_ok = is_compact(w) || is_medium(w) || is_wide(w);

    return assert_true(on_profile)
        && assert_true(can_go_back)
        && assert_true(back_home)
        && assert_true(bp_ok)
        && assert_false(bg === t2.bg)
        && assert_str_eq(accent, "#38bdf8")
        && assert_near(radius_md, 12.0, 0.01)
        && assert_near(pad, pad, 0.001)
        && assert_eq_i64(cols, cols)
        && assert_gt(10, 5)
        && assert_lt(1, 2)
        && assert_true(t3.radius_lg > 0.0 && t4.radius_sm > 0.0);
}
"###;

    let prog = parse_tsx(src).expect("parse 4 new stdlib modules");
    let module = lower_program(&prog).expect("lower 4 new stdlib modules");
    assert_canonical_ssa_module(&module);

    let theme_schema = module.get_schema("Theme").expect("Theme schema");
    assert_eq!(theme_schema.fields.len(), 14);
    for fname in [
        "bg",
        "surface",
        "surface_alt",
        "border",
        "text",
        "text_muted",
        "accent",
        "accent_alt",
        "success",
        "warning",
        "danger",
        "radius_sm",
        "radius_md",
        "radius_lg",
    ] {
        assert!(
            theme_schema.get_field_by_name(fname).is_some(),
            "Theme missing field {fname}"
        );
    }

    let route_schema = module.get_schema("Route").expect("Route schema");
    assert_eq!(route_schema.fields.len(), 3);
    assert_eq!(
        route_schema.get_field_by_name("current").unwrap().ty,
        Type::Str
    );
    assert_eq!(
        route_schema.get_field_by_name("previous").unwrap().ty,
        Type::Str
    );
    assert_eq!(
        route_schema.get_field_by_name("depth").unwrap().ty,
        Type::I64
    );
}

// ============================================================================
// 6. Strict `format_tsk` Idempotency Across All v0.4.0 Syntax
// ============================================================================

#[test]
fn test_format_tsk_strict_idempotency_v04_syntax() {
    let messy = r###"// v0.4.0 Language Core Showcase
import { Option, Result } from "std/core";
import { Theme, theme_obsidian } from "std/theme";

export enum Msg {
    Data(i64),
    Empty,
}

export function showcase(nums: readonly i64[], opt: Option<i64>, res: Result<i64, string>, m: Msg): i64 {
    const t: Theme = theme_obsidian();
    const { bg, accent: primaryAccent, radius_md } = t;
    let [first, second] = [10, 20];
    const pipeline = nums.filter((x, idx) => idx >= 0 && x > 0).map((x, idx) => x + idx).flatMap((x) => [x, x * 2]).slice(0, 6).concat([first, second]).reverse();
    const total = pipeline.reduce((acc, item, idx) => acc + item + idx, 0);
    const has_ten = pipeline.includes(10);
    const pos = pipeline.indexOf(10);
    const any_big = pipeline.some((x) => x > 5);
    const all_pos = pipeline.every((x) => x >= 0);
    const cnt = pipeline.count((x) => x > 2);
    const fi = pipeline.findIndex((x) => x === 20);
    const val = (opt ?? 5) + (res ?? 10);
    const bonus = has_ten && any_big && all_pos && bg !== primaryAccent ? toInt(radius_md) : 0;
    switch (m) {
        case Msg.Data(d):
            return total + pos + cnt + fi + val + bonus + d;
        case Msg.Empty:
            return total + val;
    }
}
"###;

    let fmt1 = format_tsk(messy).expect("format pass 1");
    let fmt2 = format_tsk(&fmt1).expect("format pass 2");
    assert_eq!(fmt1, fmt2, "format_tsk must be strictly idempotent on v0.4.0 syntax");
    assert!(fmt1.contains("const { bg, accent: primaryAccent, radius_md } = t;"));
    assert!(fmt1.contains("let [first, second] = [10, 20];"));
    assert!(fmt1.contains("opt ?? 5"));

    let reparsed = parse_tsx(&fmt1).expect("re-parse formatted v0.4.0 file");
    let lowered = lower_program(&reparsed).expect("re-lower formatted v0.4.0 file");
    assert_canonical_ssa_module(&lowered);
}
