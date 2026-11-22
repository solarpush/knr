---
name: knr-ssa
description: >-
  Official guide, Strict TypeScript/TSX (.tsk) language reference, Unified GPU Engine UI guide, and AI workflow
  for the knr linear programming language and toolchain (knr explore, knr check, knr fix, knr fmt, knr test,
  knr render --click/--input/--assert-audit/--baseline, knr run, knr dev, knr build, knr lsp, knr asm, knr disasm).
  Activate this skill whenever reading, writing, refactoring, exploring, verifying, visually auditing, testing,
  or compiling .tsk, .knr, .ssa, or .ssa.schema files, building native binaries, WebAssembly (.wasm), Web apps
  (--target web), or desktop GPU UI applications (--gui).
---

# knr Language & Toolchain Skill (v0.3.0)

Use the `knr` binary directly from `$PATH` (never invoke `cargo run` or `/tmp/.../knr` when `knr` is installed).

---

## 1. Mandatory AI Workflow (`explore` -> `check` / `fix` -> `fmt` -> `test` -> `render --assert-audit` -> `build`)

1. **Explore the project in one shot before reading files**:
   ```bash
   knr explore <dir_or_file>
   knr explore <dir> --symbol <TypeOrFn>
   knr explore <dir> --callers <fn_name>
   knr explore <dir> --callees <fn_name>
   knr explore <dir> --effects <ui|net|hw|async>
   knr explore <dir> --orphans
   knr explore <dir> --format <ai|json|mermaid>
   ```
2. **Statically verify in < 100 us + deterministic auto-fix + canonical formatting**:
   ```bash
   knr check <entry.tsk> --json
   knr fix <entry.tsk> --write --json     # Auto-heals UseAfterMove, UndeclaredEffect, missing std/ui, BorrowEscape
   knr fmt --check <dir_or_file>          # Verify canonical formatting
   knr fmt <dir_or_file>                  # Canonically format .tsk/.knr files
   ```
   - Inspect `fix_hint`, `source_variable`, and `alive_registers` in `knr check --json` output if a `UseAfterMove`, `LinearLeak`, `BorrowEscape`, or `ShaderImpureOp` occurs.
3. **Run deterministic unit tests (zero browser/network needed)**:
   ```bash
   knr test <file_or_dir>
   knr test <file_or_dir> --json
   ```
   - Discovers and runs every `export function test_*(): bool` (or `i64`) in `.tsk`/`.knr`/`.ssa` files via the deterministic VM with mocked effects.
4. **Headless GPU/CPU Render, Interactive Flow Simulation & Visual Regression (`knr render`)**:
   ```bash
   knr render <app.tsk> -o /tmp/frame.png --assert-audit
   knr render <app.tsk> --input draft_input="Ship v0.3" --click btn_add --click btn_filter_open -o /tmp/flow.png --assert-audit --json
   knr render <app.tsk> --baseline /tmp/ref.png --max-diff 0.0 --assert-audit
   ```
   - Simulates real user flows (`--click <key>`, `--input <key=text>`, `--slider <key=0..1>`, `--action <act>`) headlessly without a browser.
   - Evaluates real `@shader` kernels (Cranelift JIT + parallel W^X-safe evaluator) and checks `ViewportOverflow`, `SiblingOverlap`, `MissingGlyph`, `MissingImage`, `LowContrast`, `TinyHitTarget`, and `UnknownShader`.
   - **Zero-defect rule**: fix ALL reported audit errors before shipping (always use `<Icon name="..." />` instead of raw Unicode symbols!).
5. **Execute or Compile (VM == Native Cranelift == Zero-Import WASM == Unified GPU Web)**:
   ```bash
   knr run <entry.tsk>                               # Reference VM (default gas: 1,000,000)
   knr run <app.tsk> --gui                           # Native GPU desktop window (wgpu + CPU fallback + async Cmd)
   knr dev <app.tsk> --gui                           # Live hot-reloading .tsk UI + WGSL shaders + async Cmd
   knr lsp                                           # Built-in JSON-RPC 2.0 LSP (diagnostics, hover, completion, def, refs, rename, quickfix, format)
   knr build <server.tsk> --target native -o server  # Standalone native binary linked with libknr_runtime.a
   knr build <app.tsk> --target native --gui -o app  # Standalone native desktop GPU executable
   knr build <app.tsk> --target wasm -o app.wasm     # Standalone zero-import WebAssembly binary
   knr build <app.tsk> --target web -o dist/         # Unified Flutter-style Canvas/WebGPU/WebGL2 + app.wasm
   knr asm <file.ssa> -o out.knrbin                  # Lossless 1:1 binary IR codec
   knr disasm out.knrbin -o roundtrip.ssa
   ```

---

## 2. Strict TypeScript / TSX Surface Syntax (`.tsk`, v0.3.0)

Primary file extension: **`.tsk`** (*TypeScript for Knr* — zero conflicts with `tsserver` / `vtsls`). Legacy `.knr` is also accepted. All source lowers to canonical statically verified linear SSA (`.ssa` / `.knrbin`).

### Imports & Built-in Standard Libraries (`std/*`)
```ts
import { Column, Row, Grid, Stack, Card, Scroll, Text, Code, Badge, Button, Input, Slider, Toggle, Shader, Icon, Image, Divider, Progress, Spacer } from "std/ui";
import { Option, Result } from "std/core";
import { Cmd, cmd_none, cmd_http_get, cmd_http_post, cmd_timer, cmd_storage_save, cmd_storage_load } from "std/cmd";
import { Vec3, smin, vec3_norm, vec3_dot, vec3_add, pi, tau } from "std/math";
import { ease_out_cubic, ease_in_out_sine, smooth_damp } from "std/anim";
import { fs_read, fs_write, fs_exists, fs_remove, json_is_valid, json_pretty, json_get, http_get, http_post } from "std/sys";
import { State, init, update } from "./state.tsk";
import { Hero } from "./components/hero.tsk";
```

### Generics, Typed Actions, Dynamic Lists & Ownership
- **Copyable (non-linear) types**: `i64`, `f64`, `bool`, `string`, `void`.
- **Linear (move-only, deterministic auto-drop) types**: `type`/`interface` records, `enum` tagged unions, `T[]`, `Map<K, V>`, `Buffer`, `Channel<T>`.
- **Zero-copy second-class borrows**: annotate read-only parameters as `s: readonly State` (lowers to `&State`). Borrowed values cannot escape (`BorrowEscape`), eliminating GC and lifetime annotations.
- **Generics (compile-time monomorphization)**: `export type Pair<A, B> = { first: A; second: B };`, `Option<i64>` (`Option<i64>.Some(42)`, `Option<i64>.None`), `Result<string, i64>` (`Result<string, i64>.Ok("ok")`, `Result<string, i64>.Err(1)`), and generic functions `fn<T>(...)`.
- **Dynamic JSX Lists & Conditional JSX**:
  - `{items.map((item, idx) => <Card key={`row_${idx}`}>...</Card>)}` lowers directly to a verified monotonic SSA loop and flattens `Widget[]` into parent containers (`<Column>`, `<Row>`, `<Grid>`, `<Card>`, `<Scroll>`, `<Stack>`).
  - `{s.toast_visible && <Badge>{s.toast_msg}</Badge>}` conditionally mounts children cleanly.
- **Compile-Time Typed Actions (`enum Action`)**:
  - Pass `onClick={Action.AddTask}` or `onClick={Action.Delete(idx)}` — typos fail `knr check` immediately!
  - Define `export function update(s: State, act: Action): State` with an exhaustive `switch (act)` over unit variants; the compiler automatically synthesizes the runtime dispatch bridge.
- **Async MVU Effects (`std/cmd`)**:
  - Optionally export `export function effect(s: readonly State, act: string): Cmd` returning `cmd_none()`, `cmd_timer(ms, "Action")`, `cmd_http_get(url, "on_resp")`, `cmd_http_post(url, body, "on_resp")`, `cmd_storage_save(key, val)`, or `cmd_storage_load(key, "on_load")`. Executed non-blockingly on Desktop (`wgpu`) and Web (`fetch`/`setTimeout`/`localStorage`).
- **Numeric discipline**: `f64` math (`sin`, `cos`, `tan`, `sqrt`, `abs`, `floor`, `ceil`, `fract`, `pow`, `atan2`, `min`, `max`, `clamp`, `lerp`, `mix`, `smoothstep`) requires `f64` operands (`0.0`, `1.0`). Convert with `toInt(x)`/`to_i64(x)`, `toFloat(x)`/`to_f64(x)`, `String(x)`/`to_str(x)`.

---

## 3. Unified Flutter-Style GPU UI & Pure `@shader` Kernels

### Built-in JSX Elements & Props
- **Containers**: `<Column>`, `<Row>`, `<Grid cols={3}>`, `<Stack>`, `<Card>`, `<Scroll h={300} overflow="scroll">` (real GPU `scissor` & CPU mask clipping!)
- **Leaves**:
  - `<Text size={16} weight="bold" font="display" textAlign="left" color="#f8fafc">...</Text>` (dynamic weight uses integer `400`/`600`/`700`)
  - `<Code lang="tsx">{snippet}</Code>` (built-in GPU syntax highlighting)
  - `<Badge bg="#0e1424" color="#38bdf8" border="#38bdf833">...</Badge>`
  - `<Button key="btn_id" onClick={Action.Go} bg="..." color="..." border="..." hoverBg="..." hoverBorder="..." px={16} py={10} radius={10}>...</Button>`
  - `<Input key="inp_id" value={s.draft} onInput="draft_input" />` (full cursor, Shift+arrow selection, Ctrl+A/C/X/V clipboard, Tab focus, Enter submit)
  - `<Slider key="sl_id" value={s.energy} onChange="slider_energy" />`
  - `<Toggle checked={s.paused} onToggle="toggle_pause" label="Pause" />`
  - `<Shader fn={knr_hero_nebula} h={340} radius={16} uniforms={[s.energy, s.hue, s.depth]} />`
  - `<Image key="logo" src="assets/logo.png" w={36} h={36} radius={8} />` (PNG/JPEG/WebP file paths & `data:image/png;base64,...` URIs; audited via `MissingImage`)
  - `<Icon name="zap" size={16} color="#38bdf8" />`
  - `<Divider />`, `<Progress value={0.92} color="linear(90, #38bdf8, #818cf8)" h={6} />`, `<Spacer size={12} />`
- **Supported `<Icon name="..." />` set** (NEVER use raw Unicode symbols in text — always use `<Icon>` + safe ASCII text):
  `zap`, `cpu`, `layers`, `shield`, `terminal`, `code`, `play`, `pause`, `refresh`, `sparkles`, `box`, `check`, `x`, `alert`, `arrow-right`, `sliders`, `eye`, `globe`, `lock`, `rocket`, `command`, `git-branch`, `palette`, `cube`, `activity`, `plus`, `minus`, `copy`, `sun`, `moon`, `diamond`.

### Pure GPU `@shader` Kernels
```ts
@shader
export function knr_hero_nebula(uv: Vec2, time: f64, mouse: Vec2, energy: f64, hue: f64, depth: f64): Color {
    const wave = 0.5 + 0.5 * sin(uv.x * 5.0 + time * (0.4 + energy));
    return { r: wave * 0.25, g: wave * 0.65, b: 0.95, a: 1.0 };
}
```
- Verified pure (no effects, no heap allocations, no recursion) and compiled via `naga` to both WGSL and WebGL2 GLSL ES 3.00.
- Avoid `atan2(py, px)` multiplied by non-integers (branch-cut seam at `+/-pi`) and avoid high-frequency `sin(uv * 90)` moire patterns.
