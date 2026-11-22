# `knr` — Strict TypeScript/TSX (`.tsk`), Zero-GC Linear SSA & Unified GPU Engine (v0.4.0)

`knr` is an **Agentic Flutter** cross-platform programming language with a **Strict TypeScript / TSX** surface syntax (`.tsk` — zero `tsserver` conflicts) compiling to a canonical monotonic SSA IR (`.ssa` / `.knrbin`). It delivers zero-GC memory safety without lifetime annotations (`'a`), a **Unified GPU UI Engine** across Desktop, Web, and Headless PNG, a built-in **MCP Server (`knr mcp`)** with 8 native tools, and a complete **Agentic Verification, Flow & Visual Audit Loop** (`knr explore` · `knr check` · `knr fix` · `knr fmt` · `knr test` · `knr bench` · `knr doctor` · `knr render --flow --viewports --diff-out --assert-audit`).

Spec: [`SPEC.md`](./SPEC.md) | AI Agent Skill: [`.agents/skills/knr-ssa/SKILL.md`](./.agents/skills/knr-ssa/SKILL.md) | Flagship v0.4.0 App: [`examples/neural_studio/`](./examples/neural_studio/)

---

## Installation & Updates

Prebuilt binaries for **Linux (`x86_64`, `aarch64`)** and **macOS Apple Silicon (`aarch64`)** bundle `knr` + `libknr_runtime.a` so you can build standalone native binaries without installing Rust:

```bash
# Install the latest release into ~/.local/bin
curl -fsSL https://raw.githubusercontent.com/solarpush/knr/main/install.sh | sh

# Pin a specific version or install directory
curl -fsSL https://raw.githubusercontent.com/solarpush/knr/main/install.sh | KNR_VERSION=v0.4.0 KNR_INSTALL_DIR="$HOME/.local/bin" sh

# Verify environment health & self-update
knr doctor
knr update --check
knr update
```

---

## What's New in v0.4.0

- **Expressive Strict TSX Language & Module Ergonomics**:
  - Full functional collection suite lowered to verified monotonic SSA loops: `.map((item, idx) => ...)`, `.filter((item, idx) => ...)` (Copy arrays), `.reduce((acc, item, idx) => ..., init)`, `.some`, `.every`, `.count`, `.findIndex`, `.includes`, `.indexOf`, `.flatMap`, `.slice`, `.concat`, and `.reverse`.
  - Non-consuming **object & array destructuring** (`const { theme_idx, viewport_w } = s;`, `const [a, b] = arr;`), **nullish coalescing `??`** on `Option<T>` / `Result<T, E>`, and compiler-enforced **exhaustive enum matching** (`NonExhaustiveMatch`).
  - Ergonomic modular imports: extension-less imports (`"./state"`), `as` aliases, `* as ns` namespace imports, barrel re-exports (`export { ... } from ...` / `export * from ...` / `index.tsk`), and `knr.toml` package imports.
- **10 Built-in Standard Library Modules (`std/*`)**:
  - Added **`std/theme`** (`Theme`, `theme_obsidian`, `theme_light`, `theme_nord`, `theme_solar`), **`std/router`** (`Route`, `router_init`, `router_push`, `router_back`, `router_replace`, `router_is`, `router_can_back`), **`std/layout`** (`is_compact`, `is_medium`, `is_wide`, `responsive_cols`, `responsive_pad`), and **`std/testing`** (`assert_eq_i64`, `assert_near`, `assert_str_eq`, `assert_true`, `assert_false`, `assert_gt`, `assert_lt`) alongside `std/ui`, `std/core`, `std/cmd`, `std/math`, `std/anim`, and `std/sys`.
- **Unified GPU Engine, Spring Physics, 67 Icons & Visual Auditor Pro v2**:
  - `z_index` (`z={...}`) layer sorting, horizontal & vertical `<Scroll>` clipping (`overflow="scroll"` / `"scroll-x"`), critically-damped **spring physics** (`UiAnimator` / `spring_step`), **67 crisp analytical vector icons**, full keyboard accessibility (`Tab`/`Shift+Tab` focus ring + `on_key(s, key)` shortcuts + `on_resize(s, w, h)` responsive callback), and **16-uniform GPU `@shader` buffers** with automatic `res`/`resolution: Vec2` and `dt: f64` binding.
  - **Visual Auditor Pro v2** adds `DuplicateKey`, `TextTruncation`, `EmptyInteractiveAction`, `OffscreenInteractive`, and a structured `AccessibilitySummary` (`focusable_count`, `interactive_keys`, `min_contrast_ratio`, `duplicate_keys`).
- **Built-in MCP Server, E2E Flows, Multi-Viewport Audit, Benchmarks & IDE-Ultra LSP**:
  - **`knr mcp`**: Native Model Context Protocol (2024-11-05) JSON-RPC 2.0 server over stdio exposing **8 native AI tools** (`knr_check`, `knr_explore`, `knr_test`, `knr_render_audit`, `knr_fmt`, `knr_fix`, `knr_bench`, `knr_get_skill`).
  - **`knr render --flow` / `--viewports` / `--diff-out`**: Declarative E2E user-journey JSON scripts, single-command responsive audits (`mobile,tablet,desktop` or `WxH`), and 3-panel magenta visual regression diff PNGs.
  - **`knr doctor`**, **`knr bench`**, and **`knr new --template app|dashboard|shader|ui|minimal`**.
  - **IDE-Ultra `knr lsp`**: Cross-file hover, Go-to-Definition, completions & auto-imports, Find References, Safe Rename, QuickFix CodeActions, Format Document, **SignatureHelp**, **InlayHints**, **WorkspaceSymbol**, and **FoldingRange**.

---

## Quick Start & Agentic Workflow

```bash
# 0. Verify environment health or scaffold a new modular project
knr doctor
knr new my_studio --template dashboard

# 1. Map project architecture, symbols, callers, and effects in one shot
knr explore examples/neural_studio/ --format ai

# 2. Verify SSA, linear ownership, readonly borrows, exhaustive enums, effects & @shader purity (< 100 us)
knr check examples/neural_studio/src/app.tsk --json
knr fix broken.tsk --write --json
knr fmt --check examples/neural_studio/

# 3. Run deterministic unit tests & VM/Cranelift JIT/WASM benchmarks
knr test examples/neural_studio/src/app.tsk
knr bench examples/neural_studio/src/app.tsk

# 4. Headless E2E user-flow journey, multi-viewport audit & visual diff regression check
knr render examples/neural_studio/src/app.tsk --flow examples/neural_studio/flow.json --assert-audit
knr render examples/neural_studio/src/app.tsk --viewports mobile,desktop --assert-audit
knr render site/app.tsk --baseline /tmp/ref.png --diff-out /tmp/diff.png --assert-audit

# 5. Launch native GPU desktop studio with live hot-reload
knr dev examples/neural_studio/src/app.tsk --gui
knr dev site/app.tsk --gui

# 6. Compile standalone native binary, zero-import WASM, or Unified Web GPU bundle
knr build examples/http_server.tsk --target native -o ./http_server
knr build examples/raymarch_3d.tsk --target native --gui -o ./raymarch_3d
knr build examples/counter_app.tsk --target wasm -o counter.wasm
knr build site/app.tsk --target web -o dist_site/
```

---

## Built-in AI Agent MCP Server (`knr mcp`)

Register `knr mcp` directly in Claude Code, Cursor, Windsurf, or Antigravity (run `make mcp-info` to print this snippet):

```json
{
  "mcpServers": {
    "knr": {
      "command": "knr",
      "args": ["mcp"]
    }
  }
}
```

| MCP Tool | Capability |
| :--- | :--- |
| `knr_check` | 4-pass static verifier + `naga` WGSL/GLSL shader validation with JSON diagnostics |
| `knr_explore` | Project graph, schemas, callers/callees, `@effects` boundaries, and orphans |
| `knr_test` | Deterministic VM test discovery and execution with mocked effects |
| `knr_render_audit` | Headless HiDPI render + interaction simulation + Visual Auditor Pro v2 (+ optional base64 PNG) |
| `knr_fmt` | Canonical `.tsk`/`.knr` formatter and check mode |
| `knr_fix` | Deterministic compiler-verified QuickFix auto-healer |
| `knr_bench` | VM gas/steps, Cranelift JIT native latency & speedup, and `.knrbin`/`.wasm` binary sizes |
| `knr_get_skill` | Returns the complete embedded `SKILL.md` language & toolchain reference |

---

## Included Examples

| Example | Description | Commands |
| :--- | :--- | :--- |
| [`examples/neural_studio/`](./examples/neural_studio/) | **v0.4.0 Multi-File Flagship**: `knr.toml`, barrel `index.tsk`, `std/theme` (4 live themes), `std/router`, `std/layout`, `std/cmd`, `std/testing`, `on_key`/`on_resize`, functional array ops, 2 GPU `@shader` kernels (`res: Vec2` + 40-step 3D SDF orb), E2E `flow.json` | `knr dev examples/neural_studio/src/app.tsk --gui` · `knr render examples/neural_studio/src/app.tsk --flow examples/neural_studio/flow.json --assert-audit` |
| [`examples/task_studio.tsk`](./examples/task_studio.tsk) | **Single-File Studio**: `.map()` dynamic lists, typed `enum Action`, `Option<i64>`, async `std/cmd`, clipped `<Scroll>`, decoded `<Image>`, `@shader` | `knr dev examples/task_studio.tsk --gui` · `knr render examples/task_studio.tsk --click btn_complete_next -o /tmp/tasks.png --assert-audit` |
| [`examples/counter_app.tsk`](./examples/counter_app.tsk) | Interactive Strict TSX MVU counter (`readonly State`, `<Icon>`, deterministic tests) | `knr test examples/counter_app.tsk` · `knr render examples/counter_app.tsk -o /tmp/counter.png --assert-audit` |
| [`examples/raymarch_3d.tsk`](./examples/raymarch_3d.tsk) | 48-step 3D SDF raymarching flagship (`std/ui` + `std/math` + `@shader`) | `knr run examples/raymarch_3d.tsk --gui` · `knr build examples/raymarch_3d.tsk --target native --gui -o ./raymarch_3d` |
| [`examples/shader_studio.tsk`](./examples/shader_studio.tsk) | Dual-kernel 2D GPU shader workbench (`aurora_waves`, `plasma_orbit` -> WGSL + GLSL ES 3.00) | `knr dev examples/shader_studio.tsk --gui` · `knr build examples/shader_studio.tsk --target web -o dist_shader/` |
| [`examples/http_server.tsk`](./examples/http_server.tsk) | Multi-file native HTTP server (`@effects("net")`, `enum Route`, Cranelift + Tokio) | `knr build examples/http_server.tsk --target native -o ./http_server` |
| [`examples/sys_demo.tsk`](./examples/sys_demo.tsk) | `std/sys` demonstration (`@effects("hw")`: filesystem + JSON validation/extraction) | `knr run examples/sys_demo.tsk` · `knr test examples/sys_demo.tsk` |
| [`site/app.tsk`](./site/app.tsk) | Official modular `knr` showcase in 100% pure `.tsk` with 3 live GPU shaders | `knr dev site/app.tsk --gui` · `knr build site/app.tsk --target web -o site/` |

---

## Editor Support & IDE-Ultra Built-in LSP (`knr lsp`)

`knr` ships a zero-dependency JSON-RPC 2.0 Language Server (`knr lsp`) dedicated to `.tsk`, `.knr`, `.ssa`, and `.ssa.schema` (diagnostics, cross-file hover/definition/references/rename/completions/auto-import, SignatureHelp, InlayHints, WorkspaceSymbol, FoldingRange, QuickFix CodeActions, and Format Document):

- **VS Code**: [`editors/vscode`](./editors/vscode) — Zero-npm extension with full Strict TSX TextMate grammar (`source.tsk`).
- **Neovim 0.10+**: [`editors/neovim`](./editors/neovim) — Drop-in `vim.lsp` + Tree-sitter TSX plugin with `:KnrLspRestart`.
- **Zed**: [`editors/zed`](./editors/zed) — Dev extension wired exclusively to `knr lsp`.

---

## Workspace Architecture

| Crate | Path | Role |
| :--- | :--- | :--- |
| **`knr-core`** | `crates/knr-core` | Strict TSX (`.tsk`) parser, generics monomorphizer, functional array & destructuring lowering, `??`, barrel/alias/namespace/package imports, canonical formatter, 10 built-in `std/*` modules, `explore`, lossless `.knrbin` codec |
| **`knr-verify`** | `crates/knr-verify` | Sub-100 us 4-pass static verifier (SSA, linear ownership + `readonly` borrows, exhaustive enums, types, `@effects`, GPU purity) + JSON diagnostics |
| **`knr-interp`** | `crates/knr-interp` | Deterministic gas-metered reference VM with step-by-step tracing and mocked effects |
| **`knr-runtime`** | `crates/knr-runtime` | Rust runtime (`rlib` + `staticlib` C-ABI): `Net` (Tokio), `Hw`/`Sys`, Unified GPU Engine (`wgpu` + CPU fallback + `z_index` sorting + spring physics + 67 vector icons + H/V `<Scroll>` + keyboard focus/shortcuts + `<Image>` + async `UiCmd` + Visual Auditor Pro v2) |
| **`knr-codegen`** | `crates/knr-codegen` | Cranelift native AOT/JIT, zero-import WebAssembly (`wasm-encoder` with `__knr_heap` arena reset), and `naga`-validated WGSL / GLSL ES 3.00 shader compiler (16 uniforms + `res`/`dt` auto-binding) |
| **`knr-cli`** | `crates/knr-cli` | Unified `knr` CLI (`mcp`, `doctor`, `bench`, `explore`, `check`, `fix`, `fmt`, `test`, `render`, `run`, `dev`, `build`, `lsp`, `asm`, `disasm`, `init`, `new`, `skill`, `update`) |
