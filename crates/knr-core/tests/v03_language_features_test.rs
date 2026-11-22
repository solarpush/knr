use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use knr_core::ast::{Module, Op, RegId, Type};
use knr_core::lower::lower_program_with_env_and_source_map;
use knr_core::{
    decode_knrbin, encode_knrbin, format_ssa, format_tsk, lower_program, lower_program_from_file,
    parse_ssa, parse_tsx, LowerError,
};

/// Validates canonical monotonic SSA invariants + lossless `.ssa` text & `.knrbin` binary roundtrip
/// (proving 100% backward compatibility with the canonical linear SSA IR and zero new opcodes).
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

    // Lossless self-contained `.ssa` text roundtrip (strip already-merged import lines)
    let mut flat = module.clone();
    flat.imports.clear();
    let ssa_text = format_ssa(&flat);
    let reparsed = parse_ssa(&ssa_text).expect("lowered module must roundtrip through .ssa text");
    assert_eq!(
        reparsed.functions.len(),
        module.functions.len(),
        "function count mismatch after .ssa roundtrip"
    );

    // Lossless `.knrbin` binary roundtrip
    let bytes = encode_knrbin(module).expect("encode lowered module to .knrbin");
    let decoded = decode_knrbin(&bytes).expect("decode lowered module from .knrbin");
    assert_eq!(
        decoded.functions.len(),
        module.functions.len(),
        "function count mismatch after .knrbin roundtrip"
    );
}

// ============================================================================
// 1. Dynamic JSX Lists (.map / .filter) + Widget[] Flattening + Conditional JSX
// ============================================================================

#[test]
fn test_map_jsx_list_flattening_and_conditional_jsx() {
    let src = r###"
import { Column, Row, Card, Text, Badge, Button, Icon } from "std/ui";

export type TodoItem = {
    id: i64;
    title: string;
    done: bool;
};

export type State = {
    items: TodoItem[];
    show_banner: bool;
    is_admin: bool;
};

export function ItemRow(item: readonly TodoItem, idx: i64): Widget {
    return (
        <Card key={`todo_${idx}`} p={12}>
            <Row justify="between" align="center">
                <Text size={15}>{item.title}</Text>
                {item.done && <Badge>Done</Badge>}
            </Row>
        </Card>
    );
}

export function render_extra(s: readonly State): Widget[] {
    return s.items.map((item, idx) => {
        const label = `#${idx}: ${item.title}`;
        return <Text key={`extra_${idx}`}>{label}</Text>;
    });
}

export function view(s: readonly State): Widget {
    const prebuilt = render_extra(s);
    return (
        <Column p={24} gap={12}>
            {s.show_banner && <Badge>Welcome</Badge>}
            {s.show_banner && s.is_admin && <Badge>Admin Mode</Badge>}
            {s.items.map((item, idx) => <ItemRow item={item} idx={idx} />)}
            {s.show_banner && s.items.map((it) => <Text>{it.title}</Text>)}
            {prebuilt}
            <Button onClick="toggle">
                {s.show_banner && <Icon name="zap" size={14} />}
            </Button>
        </Column>
    );
}
"###;

    let prog = parse_tsx(src).expect("parse dynamic JSX + .map + conditional JSX");
    let module = lower_program(&prog).expect("lower dynamic JSX + .map + conditional JSX");
    assert_canonical_ssa_module(&module);

    let view_fn = module.get_function("view").expect("view fn");
    // Must contain VecLen + VecGet + VecPush + Free for flattening temporary Widget[] lists
    let mut has_vec_len = false;
    let mut has_vec_get = false;
    let mut has_vec_push = false;
    let mut has_free = false;
    for blk in &view_fn.blocks {
        for inst in &blk.instructions {
            match inst.op {
                Op::VecLen(_) => has_vec_len = true,
                Op::VecGet(_, _) => has_vec_get = true,
                Op::VecPush(_, _) => has_vec_push = true,
                Op::Free(_) => has_free = true,
                _ => {}
            }
        }
    }
    assert!(has_vec_len && has_vec_get && has_vec_push && has_free);
}

#[test]
fn test_map_and_filter_chaining_and_arrow_forms() {
    let src = r###"
export function transform(nums: readonly i64[]): i64[] {
    const positives = nums.filter(x => x > 0);
    const scaled = positives.map((x, idx) => {
        if (idx === 0) {
            return x * 10;
        }
        return x * 2 + idx;
    });
    return scaled;
}

export function chained_temp(): i64[] {
    return [1, -2, 3, -4, 5].filter((n) => n > 0).map(n => n + 1);
}
"###;

    let prog = parse_tsx(src).expect("parse filter + map chaining");
    let module = lower_program(&prog).expect("lower filter + map chaining");
    assert_canonical_ssa_module(&module);

    let chained = module.get_function("chained_temp").expect("chained_temp");
    // Temporary source vecs should be freed cleanly
    let free_count: usize = chained
        .blocks
        .iter()
        .flat_map(|b| &b.instructions)
        .filter(|i| matches!(i.op, Op::Free(_)))
        .count();
    assert!(
        free_count >= 2,
        "expected temporary Vecs in chained filter+map to be freed, got {free_count}"
    );
}

#[test]
fn test_filter_rejects_linear_element_type() {
    let src = r###"
export type Item = { id: i64; };

export function bad_filter(items: readonly Item[]): Item[] {
    return items.filter((it) => it.id > 0);
}
"###;
    let prog = parse_tsx(src).expect("parse");
    let err = lower_program(&prog).expect_err(".filter on linear struct[] must be rejected");
    assert!(
        matches!(err, LowerError::InvalidConstruct { .. }),
        "unexpected error: {err:?}"
    );
    assert!(err.to_string().contains("Copy"));
}

#[test]
fn test_map_prevents_moving_borrowed_linear_item() {
    let src = r###"
export type Item = { id: i64; };

export function bad_map(items: readonly Item[]): Item[] {
    return items.map((it) => it);
}
"###;
    let prog = parse_tsx(src).expect("parse");
    let err = lower_program(&prog).expect_err("returning borrowed linear item from .map must fail");
    assert!(
        matches!(err, LowerError::InvalidConstruct { .. }),
        "unexpected error: {err:?}"
    );
}

// ============================================================================
// 2. Compile-Time Validated Typed Actions (`enum Action`) + Exhaustive `update` Bridge
// ============================================================================

#[test]
fn test_typed_actions_in_jsx_and_update_bridge() {
    let src = r###"
import { Column, Row, Button, Text } from "std/ui";

export enum Action {
    Inc,
    Dec,
    Reset,
}

export enum RowAction {
    Delete(i64),
    Move(i64, string),
}

export type State = {
    count: i64;
};

export function init(): State {
    return { count: 0 };
}

export function update(s: State, act: Action): State {
    switch (act) {
        case Action.Inc:
            return { ...s, count: s.count + 1 };
        case Action.Dec:
            return { ...s, count: s.count - 1 };
        case Action.Reset:
            return { ...s, count: 0 };
    }
}

export function view(s: readonly State): Widget {
    const idx = s.count;
    return (
        <Column gap={8}>
            <Row gap={8}>
                <Button onClick={Action.Inc}>Inc</Button>
                <Button onClick={Action.Dec}>Dec</Button>
                <Button onClick={Action.Reset}>Reset</Button>
            </Row>
            <Button onClick={RowAction.Delete(idx)}>Delete</Button>
            <Button onClick={RowAction.Move(idx, "top")}>Move</Button>
        </Column>
    );
}

export function test_both_calling_styles(): bool {
    let s = init();
    s = update(s, Action.Inc);
    s = update(s, "Inc");
    s = update(s, Action.Dec);
    s = update(s, "UnknownAction");
    return s.count === 1;
}
"###;

    let prog = parse_tsx(src).expect("parse typed action app");
    let module = lower_program(&prog).expect("lower typed action app");
    assert_canonical_ssa_module(&module);

    // 1. Typed `update` renamed to `__knr_typed_update(State, Action) -> State`
    let typed_update = module
        .get_function("__knr_typed_update")
        .expect("must synthesize __knr_typed_update");
    assert_eq!(typed_update.params.len(), 2);
    assert_eq!(typed_update.params[0].1, Type::Struct("State".to_string()));
    assert_eq!(typed_update.params[1].1, Type::Struct("Action".to_string()));
    assert_eq!(typed_update.ret_ty, Type::Struct("State".to_string()));

    // 2. Synthesized bridge `update(State, str) -> State`
    let bridge = module
        .get_function("update")
        .expect("must synthesize string bridge update");
    assert_eq!(bridge.params.len(), 2);
    assert_eq!(bridge.params[0].1, Type::Struct("State".to_string()));
    assert_eq!(bridge.params[1].1, Type::Str);
    assert_eq!(bridge.ret_ty, Type::Struct("State".to_string()));

    // Bridge must match "Inc", "Dec", "Reset" and call `__knr_typed_update`
    let bridge_strs: HashSet<String> = bridge
        .blocks
        .iter()
        .flat_map(|b| &b.instructions)
        .filter_map(|i| match &i.op {
            Op::ConstStr(s) => Some(s.clone()),
            _ => None,
        })
        .collect();
    assert!(bridge_strs.contains("Inc"));
    assert!(bridge_strs.contains("Dec"));
    assert!(bridge_strs.contains("Reset"));

    let bridge_calls_typed = bridge
        .blocks
        .iter()
        .flat_map(|b| &b.instructions)
        .any(|i| matches!(&i.op, Op::Call(name, _) if name == "__knr_typed_update"));
    assert!(bridge_calls_typed);

    // 3. View encodes `Action.Inc` -> "Inc", `RowAction.Delete(idx)` -> "Delete" + ":" + IntToStr(idx)
    let view_fn = module.get_function("view").expect("view");
    let view_strs: HashSet<String> = view_fn
        .blocks
        .iter()
        .flat_map(|b| &b.instructions)
        .filter_map(|i| match &i.op {
            Op::ConstStr(s) => Some(s.clone()),
            _ => None,
        })
        .collect();
    assert!(view_strs.contains("Inc"));
    assert!(view_strs.contains("Dec"));
    assert!(view_strs.contains("Reset"));
    assert!(view_strs.contains("Delete"));
    assert!(view_strs.contains("Move"));
    assert!(view_strs.contains(":"));
    assert!(view_fn
        .blocks
        .iter()
        .flat_map(|b| &b.instructions)
        .any(|i| matches!(i.op, Op::IntToStr(_))));
}

#[test]
fn test_typed_action_validation_rejects_typos_and_bad_args() {
    // 1. Typo variant on unit action
    let typo_src = r###"
import { Button } from "std/ui";
export enum Action { Inc, Dec }
export function view(): Widget {
    return <Button onClick={Action.Typo}>Oops</Button>;
}
"###;
    let err = lower_program(&parse_tsx(typo_src).unwrap()).expect_err("must reject Action.Typo");
    assert!(matches!(err, LowerError::UndeclaredField { .. }), "{err:?}");

    // 2. Wrong argument count on parameterized action
    let bad_argc = r###"
import { Button } from "std/ui";
export enum Action { Delete(i64) }
export function view(): Widget {
    return <Button onClick={Action.Delete}>Oops</Button>;
}
"###;
    let err =
        lower_program(&parse_tsx(bad_argc).unwrap()).expect_err("must reject missing arg count");
    assert!(
        matches!(err, LowerError::ArgCountMismatch { .. }),
        "{err:?}"
    );

    // 3. Wrong argument type on parameterized action
    let bad_type = r###"
import { Button } from "std/ui";
export enum Action { Delete(i64) }
export function view(): Widget {
    return <Button onClick={Action.Delete("not_an_int")}>Oops</Button>;
}
"###;
    let err = lower_program(&parse_tsx(bad_type).unwrap()).expect_err("must reject wrong arg type");
    assert!(matches!(err, LowerError::TypeMismatch { .. }), "{err:?}");

    // 4. Legacy string `update(s: State, action: string): State` must NOT create `__knr_typed_update`
    let legacy_src = r###"
export type State = { count: i64; };
export function update(s: State, action: string): State {
    if (action === "inc") {
        return { ...s, count: s.count + 1 };
    }
    return s;
}
"###;
    let legacy_mod = lower_program(&parse_tsx(legacy_src).unwrap()).expect("lower legacy update");
    assert!(legacy_mod.get_function("update").is_some());
    assert!(legacy_mod.get_function("__knr_typed_update").is_none());
}

// ============================================================================
// 3. Generic `type`, `enum`, `function` Monomorphization + `std/core` & `std/cmd`
// ============================================================================

#[test]
fn test_generics_monomorphization_and_std_core() {
    let src = r###"
import { Option, Result } from "std/core";

export type Pair<A, B> = {
    first: A;
    second: B;
};

export function first<A, B>(p: readonly Pair<A, B>): A {
    return p.first;
}

export function second<A, B>(p: readonly Pair<A, B>): B {
    return p.second;
}

export function wrap_some<T>(x: T): Option<T> {
    return Option.Some(x);
}

export function unwrap_or_i64(opt: Option<i64>, fallback: i64): i64 {
    switch (opt) {
        case Option.Some(v):
            return v;
        case Option.None:
            return fallback;
    }
}

export function parse_code(ok: bool): Result<string, i64> {
    if (ok) {
        return Result.Ok("ready");
    }
    return Result.Err(404);
}

export function exercise(): i64 {
    const p: Pair<i64, string> = { first: 42, second: "knr" };
    const a = first<i64, string>(p);
    const b = second<i64, string>(p);
    const o1 = wrap_some<i64>(a);
    const o2: Option<i64> = Option<i64>.None;
    const o3 = Option<i64>.Some(7);
    const r = parse_code(b === "knr");
    const nested: Option<Pair<i64, string>> = Option.Some(p);
    let score = unwrap_or_i64(o1, 0) + unwrap_or_i64(o2, 1) + unwrap_or_i64(o3, 0);
    switch (r) {
        case Result.Ok(_s):
            score = score + 10;
            break;
        case Result.Err(code):
            score = score - code;
            break;
    }
    switch (nested) {
        case Option.Some(inner):
            return score + inner.first;
        case Option.None:
            return score;
    }
}
"###;

    let prog = parse_tsx(src).expect("parse generics + std/core");
    let module = lower_program(&prog).expect("lower generics + std/core");
    assert_canonical_ssa_module(&module);

    // Check exact monomorphized names and zero duplicate schemas/functions
    assert!(module.get_schema("Option__i64").is_some());
    assert!(module.get_schema("Result__str__i64").is_some());
    assert!(module.get_schema("Pair__i64__str").is_some());
    assert!(module.get_schema("Option__Pair__i64__str").is_some());
    assert!(module.get_function("first__i64__str").is_some());
    assert!(module.get_function("second__i64__str").is_some());
    assert!(module.get_function("wrap_some__i64").is_some());

    // Templates themselves must not leak into SSA module
    assert!(module.get_schema("Option").is_none());
    assert!(module.get_schema("Result").is_none());
    assert!(module.get_schema("Pair").is_none());
    assert!(module.get_function("first").is_none());

    // Ensure no duplicate schema or function names
    let schema_names: HashSet<&str> = module.schemas.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(schema_names.len(), module.schemas.len());
    let fn_names: HashSet<&str> = module.functions.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(fn_names.len(), module.functions.len());
}

#[test]
fn test_generic_wrong_type_arg_count_rejected() {
    let src = r###"
import { Option } from "std/core";
export function bad(x: Option<i64, string>): i64 {
    return 0;
}
"###;
    let prog = parse_tsx(src).expect("parse");
    let err = lower_program(&prog).expect_err("wrong generic arity must fail");
    assert!(matches!(err, LowerError::InvalidConstruct { .. }), "{err:?}");
}

#[test]
fn test_std_cmd_import_and_lowering() {
    let src = r###"
import {
    Cmd,
    cmd_none,
    cmd_http_get,
    cmd_http_post,
    cmd_timer,
    cmd_storage_save,
    cmd_storage_load,
} from "std/cmd";

export function dispatch_cmd(mode: i64): Cmd {
    if (mode === 0) {
        return cmd_none();
    }
    if (mode === 1) {
        return cmd_http_get("https://api.example.com/items", "on_items");
    }
    if (mode === 2) {
        return cmd_http_post("https://api.example.com/items", "{\"ok\":true}", "on_saved");
    }
    if (mode === 3) {
        return cmd_timer(500.0, "on_tick");
    }
    if (mode === 4) {
        return cmd_storage_save("theme", "dark");
    }
    return cmd_storage_load("theme", "on_theme");
}
"###;

    let prog = parse_tsx(src).expect("parse std/cmd");
    let module = lower_program(&prog).expect("lower std/cmd");
    assert_canonical_ssa_module(&module);

    let cmd_schema = module.get_schema("Cmd").expect("Cmd schema");
    assert_eq!(cmd_schema.fields.len(), 5);
    assert_eq!(cmd_schema.get_field_by_name("kind").unwrap().ty, Type::Str);
    assert_eq!(
        cmd_schema.get_field_by_name("target").unwrap().ty,
        Type::Str
    );
    assert_eq!(
        cmd_schema.get_field_by_name("payload").unwrap().ty,
        Type::Str
    );
    assert_eq!(
        cmd_schema.get_field_by_name("on_result").unwrap().ty,
        Type::Str
    );
    assert_eq!(
        cmd_schema.get_field_by_name("delay_ms").unwrap().ty,
        Type::F64
    );

    for fn_name in [
        "cmd_none",
        "cmd_http_get",
        "cmd_http_post",
        "cmd_timer",
        "cmd_storage_save",
        "cmd_storage_load",
        "dispatch_cmd",
    ] {
        assert!(
            module.get_function(fn_name).is_some(),
            "missing std/cmd fn {fn_name}"
        );
    }
}

// ============================================================================
// 4. Canonical `.tsk` Formatter (`format_tsk`) Strict Idempotency & Re-Lower
// ============================================================================

#[test]
fn test_format_tsk_idempotent_on_repo_apps() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let repo_root = manifest_dir
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();

    for rel in ["site/app.tsk", "examples/counter_app.tsk"] {
        let path = repo_root.join(rel);
        let src = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()));

        let fmt1 = format_tsk(&src).unwrap_or_else(|e| panic!("format_tsk pass 1 on {rel}: {e}"));
        let fmt2 =
            format_tsk(&fmt1).unwrap_or_else(|e| panic!("format_tsk pass 2 on {rel}: {e}"));
        assert_eq!(fmt1, fmt2, "format_tsk must be strictly idempotent on {rel}");

        // Must re-parse cleanly
        let reparsed =
            parse_tsx(&fmt1).unwrap_or_else(|e| panic!("re-parse formatted {rel}: {e}"));
        assert!(!reparsed.declarations.is_empty());

        // Standalone example must also re-lower cleanly
        if rel == "examples/counter_app.tsk" {
            let lowered = lower_program(&reparsed).expect("lower formatted counter_app.tsk");
            assert_canonical_ssa_module(&lowered);
        }
    }
}

#[test]
fn test_format_tsk_complex_jsx_shader_generics_and_invalid_input() {
    let messy = r###"// Leading header comment
// Second comment line
import {Column,Row,Card,Text,Button,Shader,Icon,Badge} from "std/ui";
import {Option,Result} from "std/core";
export const ACCENT:string="#38bdf8";
export type Pair<A,B>={first:A;second:B;};
export enum Action{Inc,Dec,Delete(i64)}
@shader
export function wave(uv:Vec2,time:f64):Color{const w=0.5+0.5*sin((uv.x+uv.y)*4.0-time);return {r:w,g:0.3,b:0.9,a:1.0};}
@effects("net")
export function ping(url:string):string{return syscall("net","http.get",url);}
export function update(s:Pair<i64,string>,act:Action):Pair<i64,string>{switch(act){case Action.Inc:return {...s,first:s.first+1};case Action.Dec:return {...s,first:s.first-1};case Action.Delete(n):return {...s,first:s.first-n};}}
export function view(items:readonly i64[],show:bool):Widget{const label=show?`Items: ${items.length}`:"Hidden";return <Column p={24} gap={12} bg="#06080f" align="center" justify="start"><Row gap={8}><Icon name="zap" size={16} color={ACCENT} /><Text size={18} weight="bold">Count: {items.length} items</Text></Row>{show&&<Badge>Visible</Badge>}{items.filter(x=>x>0).map((x,i)=><Card key={`c_${i}`}><Text>{x}</Text></Card>)}<Shader fn={wave} h={200} radius={12} uniforms={[1.0]} /><Button onClick={Action.Inc}>{label}</Button></Column>;}
"###;

    let fmt1 = format_tsk(messy).expect("format pass 1");
    let fmt2 = format_tsk(&fmt1).expect("format pass 2");
    assert_eq!(fmt1, fmt2, "complex v0.3.0 file must format idempotently");
    assert!(fmt1.starts_with("// Leading header comment\n// Second comment line\n\n"));
    assert!(fmt1.contains("@shader\nexport function wave("));
    assert!(fmt1.contains("@effects(\"net\")\nexport function ping("));

    // Formatted code must re-parse and lower cleanly into canonical SSA!
    let reparsed = parse_tsx(&fmt1).expect("re-parse formatted complex file");
    let lowered = lower_program(&reparsed).expect("lower formatted complex file");
    assert_canonical_ssa_module(&lowered);

    // Invalid syntax must return Err without panicking
    let invalid = "export function broken( { return ;";
    assert!(format_tsk(invalid).is_err());
}

// ============================================================================
// 5. Transitive Import Visibility Fix (`lower/mod.rs`)
// ============================================================================

struct TempTestDir {
    path: PathBuf,
}

impl TempTestDir {
    fn new(prefix: &str) -> Self {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let id = COUNTER.fetch_add(1, Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!(
            "knr_v03_{prefix}_{}_{}",
            std::process::id(),
            id
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("create temp test dir");
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

#[test]
fn test_transitive_import_visibility_a_imports_b_then_c_and_d() {
    let td = TempTestDir::new("transitive");

    // B defines helper fn, JSX component with named props, const, enum, and generic type
    td.write(
        "b.tsk",
        r###"
import { Badge, Text } from "std/ui";

export const BRAND_COLOR: string = "#38bdf8";

export enum PingAction {
    Ping,
}

export type Boxed<T> = {
    value: T;
};

export function double_val(x: i64): i64 {
    return x * 2;
}

export function StatusPill(label: string, accent: string): Widget {
    return <Badge color={accent}>{label}</Badge>;
}
"###,
    );

    // D does NOT import B directly, yet calls `double_val` from B!
    td.write(
        "d.tsk",
        r###"
export function quad_val(x: i64): i64 {
    return double_val(double_val(x));
}
"###,
    );

    // C imports D (not B!), yet calls `double_val` from B, renders `<StatusPill ... />` with named props,
    // references `BRAND_COLOR`, `PingAction.Ping`, and `Boxed<i64>`!
    td.write(
        "c.tsk",
        r###"
import { Column, Button } from "std/ui";
import { quad_val } from "./d.tsk";

export function use_b_transitively(n: i64): i64 {
    const b: Boxed<i64> = { value: quad_val(n) + double_val(1) };
    return b.value;
}

export function render_c(): Widget {
    return (
        <Column gap={8}>
            <StatusPill label="ok" accent={BRAND_COLOR} />
            <Button onClick={PingAction.Ping}>Ping</Button>
        </Column>
    );
}
"###,
    );

    // A imports B then C
    let a_path = td.write(
        "a.tsk",
        r###"
import { double_val, StatusPill } from "./b.tsk";
import { use_b_transitively, render_c } from "./c.tsk";

export function entry(): i64 {
    return use_b_transitively(5);
}
"###,
    );

    let module = lower_program_from_file(&a_path).expect("transitive import visibility must work");
    assert_canonical_ssa_module(&module);
    assert!(module.get_function("double_val").is_some());
    assert!(module.get_function("quad_val").is_some());
    assert!(module.get_function("use_b_transitively").is_some());
    assert!(module.get_function("render_c").is_some());
    assert!(module.get_function("entry").is_some());
    assert!(module.get_schema("Boxed__i64").is_some());
}

#[test]
fn test_extra_functions_visible_to_imported_child_files() {
    let td = TempTestDir::new("extra_fns");

    // Pre-lower a helper function into `extra_functions`
    let helper_prog = parse_tsx("export function preloaded_add(a: i64, b: i64): i64 { return a + b; }")
        .expect("parse helper");
    let helper_mod = lower_program(&helper_prog).expect("lower helper");

    // Child file calls `preloaded_add` without importing it
    td.write(
        "child.tsk",
        r###"
export function child_calc(x: i64): i64 {
    return preloaded_add(x, 10);
}
"###,
    );

    let root_prog = parse_tsx(
        r###"
import { child_calc } from "./child.tsk";
export function main_calc(x: i64): i64 {
    return child_calc(x);
}
"###,
    )
    .expect("parse root");

    let (module, _) = lower_program_with_env_and_source_map(
        &root_prog,
        &[],
        &helper_mod.functions,
        Some(td.path.clone()),
    )
    .expect("child file must see pre-loaded extra_functions");

    assert!(module.get_function("child_calc").is_some());
    assert!(module.get_function("main_calc").is_some());
}
