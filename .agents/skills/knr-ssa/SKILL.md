---
name: knr-ssa
description: >-
  Official guide, Strict TypeScript/TSX (.tsk) language reference, Unified GPU Engine UI guide, MCP server & AI workflow
  for the knr v0.4.0 linear programming language and toolchain (knr mcp, knr doctor, knr bench, knr explore, knr check,
  knr fix, knr fmt, knr test, knr render --flow/--viewports/--diff-out/--assert-audit/--baseline, knr run, knr dev,
  knr build, knr new --template, knr lsp, knr asm, knr disasm).
  Activate this skill whenever reading, writing, refactoring, exploring, verifying, visually auditing, testing,
  benchmarking, or compiling .tsk, .knr, .ssa, or .ssa.schema files, building native binaries, WebAssembly (.wasm),
  Web apps (--target web), or desktop GPU UI applications (--gui).
---

# knr Language & Toolchain Skill (v0.4.0)

Use the `knr` binary directly from `$PATH` (never invoke `cargo run` or `/tmp/.../knr` when `knr` is installed).
AI agents with MCP support can connect directly to **`knr mcp`** (Model Context Protocol 2024-11-05 over stdio) for 8 native zero-shell tools: `knr_check`, `knr_explore`, `knr_test`, `knr_render_audit`, `knr_fmt`, `knr_fix`, `knr_bench`, and `knr_get_skill`.

---

## 1. Mandatory AI Workflow (`explore` -> `check` / `fix` -> `fmt` -> `test` -> `render --flow --viewports --assert-audit` -> `build`)

1. **Inspect environment or scaffold a project**:
   ```bash
   knr doctor --json                                 # Verify CLI, linker, libknr_runtime.a, display, Node.js, VS Code ext, knr.toml
   knr new my_app --template app                     # Templates: app | dashboard | shader | ui | minimal
   ```
2. **Explore the project in one shot before reading files**:
   ```bash
   knr explore <dir_or_file>
   knr explore <dir> --symbol <TypeOrFn>
   knr explore <dir> --callers <fn_name>
   knr explore <dir> --callees <fn_name>
   knr explore <dir> --effects <ui|net|hw|async>
   knr explore <dir> --orphans
   knr explore <dir> --format <ai|json|mermaid>
   ```
3. **Statically verify in < 100 us + deterministic auto-fix + canonical formatting**:
   ```bash
   knr check <entry.tsk> --json
   knr fix <entry.tsk> --write --json                # Auto-heals UseAfterMove, UndeclaredEffect, missing std/ui, BorrowEscape
   knr fmt --check <dir_or_file>                     # Verify canonical formatting
   knr fmt <dir_or_file>                             # Canonically format .tsk/.knr files
   ```
   - Inspect `fix_hint`, `source_variable`, and `alive_registers` in `knr check --json` output if a `UseAfterMove`, `LinearLeak`, `BorrowEscape`, `NonExhaustiveMatch`, or `ShaderImpureOp` occurs.
4. **Run deterministic unit tests & performance benchmarks (zero browser/network needed)**:
   ```bash
   knr test <file_or_dir>
   knr test <file_or_dir> --json
   knr bench <entry.tsk> --json                      # VM gas/steps, Cranelift JIT compile & native latency, .knrbin/.wasm sizes
   ```
   - Discovers and runs every `export function test_*(): bool` (or `i64`) in `.tsk`/`.knr`/`.ssa` files via the deterministic VM with mocked effects. Use `std/testing` (`assert_eq_i64`, `assert_near`, `assert_str_eq`, `assert_true`, `assert_false`, `assert_gt`, `assert_lt`).
5. **Headless GPU/CPU Render, Declarative E2E Flows, Multi-Viewport & Visual Regression (`knr render`)**:
   ```bash
   knr render <app.tsk> -o /tmp/frame.png --assert-audit
   knr render <app.tsk> --viewports mobile,tablet,desktop --assert-audit --json
   knr render <app.tsk> --flow flow.json --assert-audit --json
   knr render <app.tsk> --input draft_input="Ship v0.4" --click btn_add -o /tmp/flow.png --assert-audit --json
   knr render <app.tsk> --baseline /tmp/ref.png --diff-out /tmp/diff.png --max-diff 0.0 --assert-audit
   ```
   - `--flow <flow.json>` runs declarative E2E journeys (`action`, `click`, `input`, `slider`, `tick`, `assert_text`, `assert_key`, `assert_audit` with `max_errors`/`max_warnings`, `snapshot`).
   - `--viewports` audits preset (`mobile`=390x844, `tablet`=768x1024, `desktop`=1280x900) or custom `WxH` sizes in one pass.
   - `--diff-out <diff.png>` emits a 3-panel `[Baseline | Actual | Magenta Diff]` visual regression PNG.
   - **Visual Auditor Pro v2** checks: `ViewportOverflow`, `SiblingOverlap`, `MissingGlyph`, `MissingImage`, `LowContrast`, `TinyHitTarget`, `UnknownShader`, `DuplicateKey`, `TextTruncation`, `EmptyInteractiveAction`, `OffscreenInteractive`, and outputs `accessibility` (`focusable_count`, `interactive_keys`, `min_contrast_ratio`, `duplicate_keys`).
   - **Zero-defect rule**: fix ALL audit errors and warnings before shipping (give explicit unique `key` props inside `.map()` loops, pass explicit `bg`/`color` on `<Input>`/`<Toggle>` for >= 4.5 WCAG contrast, and always use `<Icon name="..." />` instead of raw Unicode symbols!).
6. **Execute, Serve MCP/LSP, or Compile (VM == Native Cranelift == Zero-Import WASM == Unified GPU Web)**:
   ```bash
   knr run <entry.tsk>                               # Reference VM (default gas: 1,000,000)
   knr run <app.tsk> --gui                           # Native GPU desktop window (wgpu + CPU fallback + async Cmd + on_key + on_resize)
   knr dev <app.tsk> --gui                           # Live hot-reloading .tsk UI + WGSL shaders + async Cmd
   knr mcp                                           # Built-in MCP 2024-11-05 JSON-RPC 2.0 server over stdio (8 tools)
   knr lsp                                           # IDE-Ultra JSON-RPC 2.0 LSP (cross-file hover/def/refs/rename/completion/auto-import, SignatureHelp, InlayHints, WorkspaceSymbol, FoldingRange, QuickFix, Format)
   knr build <server.tsk> --target native -o server  # Standalone native binary linked with libknr_runtime.a
   knr build <app.tsk> --target native --gui -o app  # Standalone native desktop GPU executable
   knr build <app.tsk> --target wasm -o app.wasm     # Standalone zero-import WebAssembly binary
   knr build <app.tsk> --target web -o dist/         # Unified Flutter-style Canvas/WebGPU/WebGL2 + app.wasm
   knr asm <file.ssa> -o out.knrbin                  # Lossless 1:1 binary IR codec
   knr disasm out.knrbin -o roundtrip.ssa
   ```

---

## 2. Strict TypeScript / TSX Surface Syntax (`.tsk`, v0.4.0)

Primary file extension: **`.tsk`** (*TypeScript for Knr* — zero conflicts with `tsserver` / `vtsls`). Legacy `.knr` is also accepted. All source lowers to canonical statically verified linear SSA (`.ssa` / `.knrbin`).

### Imports, Barrels, Aliases & Built-in Standard Libraries (10 `std/*` Modules)
```ts
import { Column, Row, Grid, Stack, Card, Scroll, Text, Code, Badge, Button, Input, Slider, Toggle, Shader, Icon, Image, Divider, Progress, Spacer, Vec2, Color } from "std/ui";
import { Option, Result } from "std/core";
import { Cmd, cmd_none, cmd_http_get, cmd_http_post, cmd_timer, cmd_storage_save, cmd_storage_load } from "std/cmd";
import { Theme, theme_obsidian, theme_light, theme_nord, theme_solar } from "std/theme";
import { Route, router_init, router_push, router_back, router_replace, router_is, router_can_back } from "std/router";
import { is_compact, is_medium, is_wide, responsive_cols, responsive_pad } from "std/layout";
import { assert_eq_i64, assert_near, assert_str_eq, assert_true, assert_false, assert_gt, assert_lt } from "std/testing";
import { Vec3, smin, vec3_norm, vec3_dot, vec3_add, pi, tau } from "std/math";
import { ease_out_cubic, ease_in_out_sine, smooth_damp } from "std/anim";
import { fs_read, fs_write, fs_exists, fs_remove, json_is_valid, json_pretty, json_get, http_get, http_post } from "std/sys";

// v0.4.0 ergonomic module imports: extension-less, `as` aliases, `* as ns`, barrel `index.tsk`, and `knr.toml` packages
import { State, Action as AppAction, init, update } from "./state";
import * as Shaders from "./shaders";
import { TopBar, StudioView } from "./components"; // resolves ./components/index.tsk (`export { TopBar } from "./topbar.tsk";`)
```

### Functional Collection Combinators, Destructuring, Nullish Coalescing & Linear Safety
- **Copyable (non-linear) types**: `i64`, `f64`, `bool`, `string`, `void`.
- **Linear (move-only, deterministic auto-drop) types**: `type`/`interface` records, `enum` tagged unions, `T[]`, `Map<K, V>`, `Buffer`, `Channel<T>`.
- **Zero-copy second-class borrows**: annotate read-only parameters as `s: readonly State` (lowers to `&State`). Borrowed values cannot escape (`BorrowEscape`), eliminating GC and lifetime annotations.
- **Functional array methods** (lowered to verified monotonic SSA loops):
  - `.map((item, idx) => ...)`, `.filter((item, idx) => ...)` (Copy elements), `.reduce((acc, item, idx) => ..., init)` (Copy & Linear accumulators), `.some((item, idx) => cond)`, `.every((item, idx) => cond)`, `.count((item, idx) => cond)`, `.findIndex((item, idx) => cond)`, `.includes(val)`, `.indexOf(val)`, `.flatMap((item, idx) => ...)`, `.slice(start, end)`, `.concat(other)`, `.reverse()`.
- **Object & array destructuring**:
  - `const { theme_idx, viewport_w } = s;` (non-consumingly borrows linear fields without moving `s`!)
  - `const [first, second] = pair_arr;`
- **Nullish coalescing `??` on `Option<T>` / `Result<T, E>`**:
  - `const id = find_next_active_id(s) ?? 0;` unwraps `Some(v)`/`Ok(v)` or evaluates the fallback on `None`/`Err(_)`.
- **Exhaustive enum matching & typed actions (`enum Action`)**:
  - Pass `onClick={Action.AddJob}` — typos fail `knr check` immediately, and missing `switch (action)` cases fail with `NonExhaustiveMatch`.
- **Interactive MVU Hooks**:
  - `init(): State`, `view(s: readonly State): Widget`, `update(s: State, act: Action | string): State`
  - Optional: `tick(s: State, dt: f64): State`, `on_input(s: State, act: string, val: string): State`, `on_slider(s: State, act: string, val: f64): State`, `on_key(s: State, key: string): State` (e.g. `"Ctrl+1"`, `"Ctrl+T"`, `"Space"`), `on_resize(s: State, w: f64, h: f64): State`, `effect(s: readonly State, act: string): Cmd`.
- **Numeric discipline**: `f64` math (`sin`, `cos`, `tan`, `sqrt`, `abs`, `floor`, `ceil`, `fract`, `pow`, `atan2`, `min`, `max`, `clamp`, `lerp`, `mix`, `smoothstep`) requires `f64` operands (`0.0`, `1.0`). Convert with `toInt(x)`/`to_i64(x)`, `toFloat(x)`/`to_f64(x)`, `String(x)`/`to_str(x)`.

---

## 3. Unified Flutter-Style GPU UI & Pure `@shader` Kernels

### Built-in JSX Elements & Props
- **Containers**: `<Column>`, `<Row>`, `<Grid cols={responsive_cols(s.viewport_w, 1, 2, 3)}>`, `<Stack>`, `<Card>`, `<Scroll h={320} overflow="scroll">` / `overflow="scroll-x"` (real GPU `scissor` & CPU mask clipping!), `z={10}` (`z_index` layer sorting), `transitionMs={180}` (critically-damped spring physics).
- **Leaves**:
  - `<Text size={16} weight="bold" font="display" textAlign="left" color={t.text}>...</Text>`
  - `<Code lang="tsx">{snippet}</Code>` (built-in GPU syntax highlighting)
  - `<Badge bg={t.surface_alt} color={t.accent} border={t.border}>...</Badge>`
  - `<Button key="btn_id" onClick={Action.Go} bg="..." color="..." border="..." hoverBg="..." hoverBorder="..." px={14} py={9} radius={10}>...</Button>`
  - `<Input key="inp_id" value={s.draft} onInput="draft_input" bg={t.surface_alt} color={t.text} border={t.border} />` (full caret, Shift+arrow selection, Ctrl+A/C/X/V clipboard, Tab/Shift+Tab focus ring, Enter submit)
  - `<Slider key="sl_id" value={s.energy} onChange="slider_energy" />`
  - `<Toggle key="tgl_id" checked={s.paused} onToggle="TogglePause" label="Pause" bg={t.surface_alt} color={t.text} />`
  - `<Shader key="sh_id" fn={neural_aurora_field} h={240} radius={12} uniforms={[s.energy, s.hue, s.density, s.glow]} />` (up to 16 float uniforms!)
  - `<Image key="logo" src="assets/logo.png" w={36} h={36} radius={8} />` (PNG/JPEG/WebP & Base64 data URIs; audited via `MissingImage`)
  - `<Icon name="sparkles" size={16} color={t.accent} />`
  - `<Divider />`, `<Progress value={0.92} color="linear(90, #38bdf8, #818cf8)" h={6} />`, `<Spacer size={12} />`
- **Supported 67 `<Icon name="..." />` set** (NEVER use raw Unicode symbols in text — always use `<Icon>` + safe ASCII text):
  `activity`, `alert`, `arrow-left`, `arrow-right`, `bell`, `bookmark`, `box`, `calendar`, `chart`, `check`, `chevron-down`, `chevron-left`, `chevron-right`, `chevron-up`, `clock`, `code`, `command`, `copy`, `cpu`, `cube`, `database`, `diamond`, `download`, `edit`, `external`, `eye`, `file`, `filter`, `folder`, `git`, `git-branch`, `globe`, `heart`, `home`, `inbox`, `info`, `layers`, `lock`, `mail`, `menu`, `minus`, `moon`, `more`, `palette`, `pause`, `play`, `plus`, `refresh`, `rocket`, `search`, `send`, `settings`, `shield`, `sliders`, `sparkles`, `star`, `sun`, `tag`, `terminal`, `trash`, `unlock`, `upload`, `user`, `users`, `wifi`, `x`, `zap`.

### Pure GPU `@shader` Kernels (16 Uniforms + Auto `res`/`dt` Binding)
```ts
@shader
export function neural_aurora_field(
    uv: Vec2, time: f64, mouse: Vec2, res: Vec2,
    energy: f64, hue: f64, density: f64, glow: f64,
): Color {
    const aspect = clamp(res.x / max(res.y, 0.001), 0.85, 2.4);
    const wave = 0.5 + 0.5 * sin((uv.x - 0.5) * aspect * (2.4 + density * 2.4) + time * (0.4 + energy));
    return { r: clamp(wave * 0.25, 0.0, 1.0), g: clamp(wave * 0.65, 0.0, 1.0), b: clamp(0.15 + wave * glow, 0.0, 1.0), a: 1.0 };
}
```
- Verified pure (no effects, no heap allocations, no recursion) and compiled via `naga` to both WGSL and WebGL2 GLSL ES 3.00.
- Automatically binds `uv: Vec2`, `time: f64`, `mouse: Vec2`, `res`/`resolution: Vec2`, `dt: f64`, plus up to **16 custom float uniforms** via `params: array<vec4<f32>, 4>`.
- Avoid `atan2(py, px)` multiplied by non-integers (branch-cut seam at `+/-pi`) and avoid high-frequency `sin(uv * 90)` moire patterns.
