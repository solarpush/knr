---
name: knr-ssa
description: >-
  Official guide, language reference, ISA reference, and AI workflow for the knr linear programming
  language and toolchain (knr explore, knr check, knr test, knr run, knr build, knr asm, knr disasm, knr skill).
  Activate this skill whenever reading, writing, refactoring, exploring, verifying, testing, or compiling
  .knr, .ssa, or .ssa.schema files, building native binaries, WebAssembly (.wasm), Web apps (--target web),
  or native desktop MVU UI applications (--gui).
---

# knr Language & Toolchain Skill

Use the `knr` binary directly from `$PATH` (never invoke `cargo run` or `/tmp/.../knr` when `knr` is installed).

---

## 1. Mandatory AI Workflow (`explore` -> `check` -> `test` -> `run` / `build`)

1. **Explore the project in one shot before reading files**:
   ```bash
   knr explore <dir_or_file>
   knr explore <dir> --symbol <SchemaOrFn>
   knr explore <dir> --callers <fn_name>
   knr explore <dir> --callees <fn_name>
   knr explore <dir> --effects <ui|net|hw|async>
   knr explore <dir> --orphans
   knr explore <dir> --format <ai|json|mermaid>
   ```
2. **Statically verify in < 100 µs (SSA, Linear Ownership & Second-Class Borrows, Types, Effects)**:
   ```bash
   knr check <entry.knr> --graph
   knr check <entry.knr> --json
   ```
   - Always inspect `fix_hint`, `source_variable`, and `alive_registers` in `--json` output if a `UseAfterMove`, `LinearLeak`, or `BorrowEscape` occurs.
3. **Run deterministic unit tests (zero browser/network needed)**:
   ```bash
   knr test <file_or_dir>
   knr test <file_or_dir> --json
   ```
   - Discovers and runs every `fn test_*() -> bool` (or `i64`) in `.knr`/`.ssa` files via the deterministic VM with mocked effects.
4. **Execute or Compile (VM == Native Cranelift == WebAssembly == Web)**:
   ```bash
   knr run <entry.knr>                              # Reference VM (default gas: 1,000,000)
   knr run <app.knr> --gui                          # Real native GPU desktop window (wgpu + CPU fallback)
   knr dev <app.knr> --gui                          # Live hot-reloading GUI + WGSL shaders window
   knr lsp                                          # Built-in JSON-RPC 2.0 Language Server (stdio)
   knr build <server.knr> --target native -o server # Standalone native ELF linked with Rust knr-runtime
   knr build <app.knr> --target native --gui -o app # Standalone native desktop GUI executable (MVU + shaders)
   knr build <app.knr> --target wasm -o app.wasm    # Standalone zero-import WebAssembly binary
   knr build <app.knr> --target web -o dist/        # SSR index.html + zero-import app.wasm + WebGPU/WebGL2
   knr asm <file.ssa> -o out.knrbin                 # Compact binary IR codec (1:1 bijection)
   knr disasm out.knrbin -o roundtrip.ssa
   ```

---

## 2. Writing Idiomatic `.knr` (Primary Source Language)

`.knr` lowers cleanly to canonical verified linear SSA (`.ssa` / `.knrbin`):

- **Multi-file & Built-in Stdlib Imports**: `import "std/ui";` (widgets including `w_divider`/`w_progress` + responsive `w_grid`), `import "std/anim";`, `import "std/math";`, `import "std/sys";` (`fs_read`/`fs_write`/`fs_exists`/`fs_remove`/`fs_list`, `json_is_valid`/`json_pretty`/`json_get`, `http_get`/`http_post`) — built directly into `knr-core`, zero relative paths needed! Plus relative `import "./helper.knr";` (cycle-safe, diamond-deduplicated; also imports `.ssa` and `.ssa.schema`).
- **Editor Extensions**: Ready-to-use VS Code, Neovim, and Zed integrations wired to `knr lsp` live in `editors/`.
- **Typed Schemas & Tagged Unions (`enum`)**:
  ```knr
  schema State { 0: count i64, 1: status str }
  enum Action { Inc(i64), Dec(i64), Reset }
  ```
- **Exhaustive `match` (expressions & statements)**:
  ```knr
  let next = match act {
      Action::Inc(n) => state.count + n,
      Action::Dec(n) => state.count - n,
      Action::Reset => 0,
  };
  ```
- **Loops & Control Flow**: `while cond { ... }` with real SSA block-parameter mutations, `break;`, `continue;`, `if`/`else if`/`else` expressions, short-circuit `&&` / `||`, and `%`.
- **Memory Safety without GC or Lifetimes (Second-Class Borrows `&T`)**:
  - Linear values (`struct`, `enum`, `vec[T]`, `map[K, V]`, `buf`, `chan[T]`) have unique ownership and auto-drop at scope exit (zero GC, zero leaks).
  - Read a linear value without moving it using **second-class borrows (`&T`)**:
    ```knr
    fn view(state: &State) -> UiNode { ... }
    let ui = view(&state); // `state` stays Alive in caller!
    ```
  - Borrowed `&T` parameters cannot escape (returned, moved, or stored in structs/vecs) -> rejected statically as `BorrowEscape` with zero lifetime annotations!
- **Strings, Math & Builtins**: `"a" + "b"`, `s1 == s2`, `to_str(n)`, `str_len(s)`, `vec_new()`, `vec_push(v, x)`, `vec_len(v)`, `v[i]`, `map_new()`, `map_set(m, k, v)`, `map_get(m, k)`, `buf_alloc(n)`, `chan_new(cap)`, plus GPU/CPU-parity math (`sin`, `cos`, `tan`, `sqrt`, `abs`, `floor`, `ceil`, `fract`, `pow`, `atan2`, `min`, `max`, `clamp`, `lerp`, `mix`, `smoothstep`, `to_f64`, `to_i64`), typed `#hex` color literals (`#38bdf8` -> `Color` or `str`), and linear struct spread (`Style { bg: #111827, ..style_card() }`).
- **Explicit Effects & Syscalls**:
  ```knr
  fn main() -> i64 effects(Net) {
      let srv: i64 = syscall Net "net.listen" 0;
      let req: HttpRequest = syscall Net "net.wait_req" srv;
      let ok: bool = syscall Net "net.respond" HttpResponse { req_id: req.req_id, status: 200, body: "ok" };
      return 0;
  }
  ```

---

## 3. Unconstrained GPU UI, 60/120 FPS Animations & `.knr` GPU Shaders (`shader fn`)

Write ONE `.knr` app using `examples/std_ui.knr` (`Widget` + 40-field `Style` + `Vec2` + `Color`, or legacy `examples/ui.knr` `UiNode`) with optional `tick(state, dt, elapsed)`, `on_slider(state, action, val)`, `on_input(state, action, text)`, and pure `shader fn` kernels (see `examples/shader_studio.knr`):
```knr
import "./std_ui.knr";

shader fn aurora(uv: Vec2, time: f64, speed: f64) -> Color {
    let wave = 0.5 + 0.5 * sin(uv.x * 6.0 + time * speed);
    return Color { r: 0.1, g: wave, b: 0.9, a: 1.0 };
}
```
- **Desktop GPU (`wgpu 23`)**: `knr run app.knr --gui` runs an analytical SDF quad + Dual Kawase `backdrop_blur` + GPU `GlyphAtlas` + custom WGSL shader pipeline at 60/120 FPS (automatic `softbuffer` + `tiny-skia` CPU and `KNR_HEADLESS=1` fallbacks).
- **Web (`WASM + WebGPU / WebGL2`)**: `knr build app.knr --target web -o dist/` emits SSR `index.html`, zero-import `app.wasm`, key-reconciled DOM host, and `naga`-validated WGSL + `#version 300 es` WebGL2 fallback shaders.
- **Static GPU Purity**: `knr check` rejects effects (`ShaderEffectForbidden`), heap allocations (`ShaderImpureOp`), and recursion (`ShaderRecursionForbidden`) inside `shader fn`, and validates generated WGSL + GLSL via `naga`.

---

## 4. Canonical Linear SSA IR & Binary Codec (`.ssa` ↔ `.knrbin`)

For low-level IR work (`.ssa` / `.ssa.schema`):
- Monotonic SSA registers (`%0, %1, ...`), block parameters (`bb1(%3: i64):`), and branch args (`br bb1(%1)` / `br_if %c bb1(%1) bb2(%2)`).
- Borrowed params in IR: `fn view(%0: &struct[State]) -> struct[UiNode]`.
- Strict `.ssa.schema` (schemas only) vs `.ssa` (imports + functions) separation for legacy raw SSA files.
- Full 1:1 roundtrip bijection between `.ssa` text and `.knrbin` binary encoding (`knr asm` / `knr disasm`).
