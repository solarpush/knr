use knr_core::{lower_program, parse_tsx};
use knr_verify::{verify_module, verify_program};

#[test]
fn test_verify_v04_array_pipeline_passes_all_static_checks() {
    let src = r###"
export type Item = {
    id: i64;
    score: i64;
};

export type Acc = {
    sum: i64;
    count: i64;
};

export function run_pipeline(nums: readonly i64[], items: readonly Item[]): i64 {
    const mapped = nums.map((x, i) => x * 2 + i);
    const filtered = mapped.filter((x, i) => i < 10 && x > 0);
    const flat = filtered.flatMap((x, i) => [x, x + i]);
    const sliced = flat.slice(0, 6);
    const combined = sliced.concat([7, 8]);
    const rev = combined.reverse();

    const s1 = rev.reduce((acc, x, i) => acc + x + i, 0);
    const s2 = items.reduce(
        (acc, it) => ({ ...acc, sum: acc.sum + it.score, count: acc.count + 1 }),
        { sum: 0, count: 0 }
    );
    const s3 = nums.reduce((acc, x) => {
        if (x > 5) {
            return { sum: acc.sum + x, count: acc.count + 1 };
        }
        return acc;
    }, { sum: 0, count: 0 });

    const any_hi = items.some((it, i) => it.score > 50 || i > 10);
    const all_ok = items.every((it) => it.id >= 0);
    const cnt = items.count((it) => it.score > 10);
    const fi = items.findIndex((it) => it.id === 3);
    const inc = rev.includes(7);
    const iof = rev.indexOf(8);
    const str_inc = ["a", "b"].includes("b");
    const str_iof = ["a", "b"].indexOf("a");

    let out = s1 + s2.sum + s2.count + s3.sum + cnt + fi + iof + str_iof;
    if (any_hi && all_ok && inc && str_inc) {
        out = out + 1;
    }
    return out;
}
"###;

    let prog = parse_tsx(src).expect("parse");
    let module = lower_program(&prog).expect("lower");
    verify_module(&module).expect("full array pipeline must pass knr_verify::verify_module");
}

#[test]
fn test_verify_v04_destructuring_and_nullish_coalescing() {
    let src = r###"
import { Option, Result } from "std/core";

export type State = {
    active_tab: string;
    energy: f64;
    items: i64[];
};

export function make_state(): State {
    return { active_tab: "dash", energy: 0.5, items: [10, 20, 30] };
}

export function run_destructure_and_coalesce(
    s: State,
    opt_i: Option<i64>,
    opt_s: Option<State>,
    res: Result<string, i64>
): State {
    const { active_tab, energy: e, items } = s;
    const [first, second] = items;
    const { active_tab: temp_tab } = make_state();
    const [a, b] = [1, 2];

    const v1 = opt_i ?? (first + second + a + b);
    const v2 = Option<i64>.None ?? 99;
    const v3 = Result<i64, string>.Ok(5) ?? 0;
    const label = res ?? temp_tab;
    const picked = opt_s ?? make_state();

    return {
        ...s,
        active_tab: active_tab + label + picked.active_tab,
        energy: e + toFloat(v1 + v2 + v3),
    };
}
"###;

    let prog = parse_tsx(src).expect("parse");
    verify_program(&prog).expect("destructuring + ?? must pass full verification");
}

#[test]
fn test_verify_all_four_new_stdlib_modules() {
    for (name, src) in [
        (
            "std/theme",
            r###"
import { Theme, theme_obsidian, theme_light, theme_nord, theme_solar } from "std/theme";
export function check_theme(): bool {
    const a: Theme = theme_obsidian();
    const b: Theme = theme_light();
    const c: Theme = theme_nord();
    const d: Theme = theme_solar();
    return a.radius_sm > 0.0 && b.radius_md > 0.0 && c.radius_lg > 0.0 && d.bg !== "";
}
"###,
        ),
        (
            "std/router",
            r###"
import { Route, router_init, router_push, router_back, router_replace, router_is, router_can_back } from "std/router";
export function check_router(): bool {
    let r: Route = router_init("/");
    r = router_push(r, "/about");
    r = router_replace(r, "/docs");
    const ok1 = router_is(r, "/docs") && router_can_back(r);
    r = router_back(r);
    return ok1 && router_is(r, "/");
}
"###,
        ),
        (
            "std/layout",
            r###"
import { is_compact, is_medium, is_wide, responsive_cols, responsive_pad } from "std/layout";
export function check_layout(w: f64): bool {
    const c = responsive_cols(w, 1, 2, 4);
    const p = responsive_pad(w);
    return (is_compact(w) || is_medium(w) || is_wide(w)) && c > 0 && p > 0.0;
}
"###,
        ),
        (
            "std/testing",
            r###"
import { assert_eq_i64, assert_near, assert_str_eq, assert_true, assert_false, assert_gt, assert_lt } from "std/testing";
export function check_testing(): bool {
    return assert_eq_i64(2, 2)
        && assert_near(1.0, 1.001, 0.01)
        && assert_str_eq("knr", "knr")
        && assert_true(true)
        && assert_false(false)
        && assert_gt(5, 2)
        && assert_lt(1, 9);
}
"###,
        ),
    ] {
        let prog = parse_tsx(src).unwrap_or_else(|e| panic!("parse {name}: {e}"));
        let module = lower_program(&prog).unwrap_or_else(|e| panic!("lower {name}: {e}"));
        verify_module(&module).unwrap_or_else(|diags| panic!("verify {name} failed: {diags:#?}"));
    }
}

#[test]
fn test_verify_exhaustive_enum_switch_and_non_exhaustive_rejection() {
    let valid_src = r###"
export enum Event {
    Click(i64, i64),
    Input(string),
    Close,
}

export function handle(ev: Event): i64 {
    switch (ev) {
        case Event.Click(x, y):
            return x + y;
        case Event.Input(s):
            return s.length;
        case Event.Close:
            return 0;
    }
}
"###;
    let prog = parse_tsx(valid_src).expect("parse valid switch");
    verify_program(&prog).expect("exhaustive payload switch must verify");

    let invalid_src = r###"
export enum Event {
    Click(i64),
    Close,
    Resize(i64, i64),
}

export function handle_bad(ev: Event): i64 {
    switch (ev) {
        case Event.Click(x):
            return x;
    }
}
"###;
    let bad_prog = parse_tsx(invalid_src).expect("parse bad switch");
    let diags = verify_program(&bad_prog).expect_err("non-exhaustive switch must fail verification");
    let combined: String = diags
        .iter()
        .map(|d| d.to_string())
        .collect::<Vec<_>>()
        .join(" ");
    assert!(
        combined.contains("NonExhaustiveMatch:"),
        "Expected NonExhaustiveMatch: in diagnostic, got: {combined}"
    );
    assert!(combined.contains("Close") && combined.contains("Resize"));
}
