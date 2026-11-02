---
name: knr-ssa
description: >-
  Official guide, ISA reference, and AI workflow for the knr linear SSA programming
  language and toolchain (knr explore, knr check, knr run, knr build, knr asm, knr disasm, knr skill).
  Activate this skill whenever reading, writing, refactoring, exploring, verifying, or compiling
  .ssa or .ssa.schema files, building WebAssembly (.wasm) or Cranelift native binaries, or constructing
  declarative VDOM UI components in knr.
---

# knr Linear SSA Language & Toolchain Skill

Use the `knr` binary directly from `$PATH` (never invoke `cargo run` or `/tmp/.../knr` when `knr` is installed).

---

## 1. Mandatory AI Workflow (`explore` -> `check` -> `run` / `build`)

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
2. **Statically verify in < 100 µs (4 passes: SSA, Linear Ownership, Types, Effects)**:
   ```bash
   knr check <entry.ssa> --graph
   knr check <entry.ssa> --json
   ```
   - Always inspect `fix_hint` and `registers_state` in `--json` output if a `UseAfterMove`, `LinearLeak`, or `SsaRedefinition` occurs.
3. **Execute or Compile**:
   ```bash
   knr run <entry.ssa>                          # Reference VM (default gas: 1,000,000)
   knr run <server.ssa> --unlimited-gas         # Long-running HTTP SSR server
   knr build <file.ssa> --target wasm -o out.wasm
   knr build <file.ssa> --target native -o out_bin
   knr asm <file.ssa> -o out.knrbin
   knr disasm out.knrbin -o roundtrip.ssa
   ```

---

## 2. Compiler-Enforced Multi-File Topology

`knr` enforces strict file separation at the compiler level:
- **`.ssa.schema` files (`FileKind::Schema`)**:
  - May **ONLY** contain `schema Name { ... }` declarations.
  - Functions (`fn`) and `import` statements are **forbidden** (`FunctionInSchemaFile`, `ImportInSchemaFile`).
- **`.ssa` files (`FileKind::Code`)**:
  - May **ONLY** contain `import "..."` declarations and `fn ...` definitions.
  - Inline `schema` declarations are **forbidden** (`SchemaInCodeFile`).
- **Diamond Imports**:
  - `import "../schemas/vdom.ssa.schema"` and `import "./component.ssa"` are deduplicated by canonical path (`Loaded`) while cyclic imports fail immediately (`CyclicImport`).

---

## 3. Grammar, Monotonic SSA & Linear Ownership Rules

### 3.1 Monotonic SSA Numbering
- Function parameters occupy `%0, %1, ..., %(k-1)`.
- Every instruction producing a value must assign to a strictly new register (`%k, %(k+1), ...`).
- Basic blocks start with `bb0:`, `bb1:`, etc., and **must** end with exactly one terminator:
  - `ret %r` (or `ret` for `void`)
  - `br bbX`
  - `br_if %cond bbTrue bbFalse` (`%cond` must be `bool`)

### 3.2 Copy vs Linear (`Move`) Types
- **`Copy` types** (`i64`, `f64`, `bool`, `str`): Can be read multiple times without moving.
- **Linear (`Move`) types** (`buf`, `vec[T]`, `map[K, V]`, `chan[T]`, `struct[S]`):
  - Created in state **`Alive`**.
  - **Mutating / consuming operations (`Move`)**:
    - `buf_write %b %idx %val` -> consumes `%b` (`Moved`), returns new `Alive` register.
    - `buf_slice %b %start %end` -> consumes `%b` (`Moved`), returns new `Alive` register.
    - `buf_free %b` / `free %r` -> consumes `%b` / `%r` (`Moved`).
    - `vec_push %v %item` -> consumes `%v` (`Moved`), returns new `Alive` register.
    - `map_set %m %k %v` -> consumes `%m` (`Moved`), returns new `Alive` register.
    - `chan_send %c %val` -> consumes `%c` (`Moved`), returns new `Alive` register.
    - `struct_set %s <field_idx> %val` -> consumes `%s` (`Moved`) and `%val` (if linear), returns new `Alive` register.
    - `call fn_name %arg0 ...` / `syscall Eff "op" %arg0 ...` -> consumes any linear arguments passed!
  - **Non-consuming read operations (`Borrow`)**:
    - `buf_read %b %idx`, `vec_get %v %idx`, `map_get %m %k`, `chan_recv %c`, `struct_get %s <field_idx>` keep the container **`Alive`** (note: `struct_get` on a linear field is forbidden; keep linear children inside `vec` or pass ownership).
  - **Zero-Leak Invariant**: Every `Alive` linear register must be either returned (`ret %r`), consumed (`struct_set`, `vec_push`, `call`, `syscall`), or explicitly freed (`free %r` / `buf_free %b`) on **all** CFG paths (`br_if` branches must have identical `Alive`/`Moved` states at join points).

---

## 4. Complete 29-Opcode ISA & 3 Terminators

| Hex | Mnemonic | Syntax | Semantics |
| :--- | :--- | :--- | :--- |
| `0x01` | `const_i64` | `%d = const_i64 42` | Load 64-bit signed integer (`Copy`) |
| `0x02` | `const_f64` | `%d = const_f64 3.14` | Load IEEE-754 float (`Copy`) |
| `0x03` | `const_bool` | `%d = const_bool true` | Load boolean (`Copy`) |
| `0x04` | `const_str` | `%d = const_str "text"` | Load UTF-8 string from pool (`Copy`) |
| `0x05..0x09` | `add`, `sub`, `mul`, `div`, `rem` | `%d = add %a %b` | Arithmetic on `i64` or `f64` |
| `0x0A..0x0E` | `bit_and`, `bit_or`, `bit_xor`, `shl`, `shr` | `%d = bit_xor %a %b` | Bitwise ops on `i64` |
| `0x0F..0x12` | `cmp_eq`, `cmp_ne`, `cmp_lt`, `cmp_gt` | `%d = cmp_eq %a %b` | Comparison returning `bool` |
| `0x13` | `buf_alloc` | `%b = buf_alloc %size` | Allocate 64-byte aligned linear buffer (`Alive`) |
| `0x14` | `buf_free` | `buf_free %b` | Consume and free linear `buf` |
| `0x15` | `buf_read` | `%v = buf_read %b %idx` | Borrow `buf` and read `i64` at `%idx` |
| `0x16` | `buf_write` | `%b2 = buf_write %b %idx %v` | Move `%b`, write `%v` at `%idx`, return `%b2` |
| `0x17` | `buf_slice` | `%b2 = buf_slice %b %s %e` | Move `%b`, slice `[%s..%e)`, return `%b2` |
| `0x18` | `vec_new` | `%v = vec_new <Type>` | Allocate linear vector (`vec[i64]`, `vec[struct[Leaf]]`) |
| `0x19` | `vec_push` | `%v2 = vec_push %v %item` | Move `%v` (and `%item` if linear), append in-place |
| `0x1A` | `vec_get` | `%x = vec_get %v %idx` | Borrow `%v`, read `Copy` element at `%idx` |
| `0x1B` | `map_new` | `%m = map_new <K> <V>` | Allocate linear map (`K` must be `i64` or `str`) |
| `0x1C` | `map_set` | `%m2 = map_set %m %k %v` | Move `%m`, insert/update key in-place |
| `0x1D` | `map_get` | `%v = map_get %m %k` | Borrow `%m`, read value for key `%k` |
| `0x1E` | `call` | `%r = call fn_name %a0 %a1` | Synchronous inter-function call (propagates effects) |
| Ext | `chan_new`, `chan_send`, `chan_recv` | `%c = chan_new i64 %cap` | Bounded FIFO linear channel (`effects(Async)`) |
| Ext | `struct_new`, `struct_get`, `struct_set` | `%s2 = struct_set %s 0 %v` | Linear schema instantiation, field borrow, in-place mutation |
| Ext | `syscall` | `%r = syscall Ui "ui.render" %p` | Host platform call (`Ui`, `Net`, `Hw`, `Async`) |
| Ext | `free` | `free %r` | Deterministically drop any linear value (`vec`, `map`, `chan`, `struct`, `buf`) |

---

## 5. Declarative Zero-Inline-JS VDOM & WebAssembly Pattern

Never embed raw `<script>` tags inside `.ssa` components. Use the standard VDOM schema (`site/schemas/vdom.ssa.schema`):
```text
schema Leaf {
    0: tag str,
    1: class str,
    2: text str,
    3: action str,
    4: target str,
}
```
- Setting field `3` (`action`, e.g. `"/api/inc"`) emits `data-knr-action="/api/inc"`.
- Setting field `4` (`target`, e.g. `".counter-val"`) emits `data-knr-target=".counter-val"`.
- `UiPlatform` (`syscall Ui "ui.render" %page`) automatically injects the `<script data-knr-runtime="v1">` hybrid bridge that dispatches clicks to:
  1. HTTP SSR (`POST /api/inc` on `site/server.ssa`), or
  2. WebAssembly client reducer (`site.wasm` -> `knr_dispatch(cur, op)` compiled via `knr build site/client_state.ssa --target wasm -o site/site.wasm`).

### Canonical Chaining Pattern for `Leaf` (Avoid `UseAfterMove`!)
Always pass the **previous `struct_set` register** (never the `const_str` register!) as the first argument to the next `struct_set`:
```text
    %10 = struct_new Leaf
    %11 = const_str "button"
    %12 = struct_set %10 0 %11
    %13 = const_str "btn act-inc"
    %14 = struct_set %12 1 %13
    %15 = const_str "+1"
    %16 = struct_set %14 2 %15
    %17 = const_str "/api/inc"
    %18 = struct_set %16 3 %17
    %19 = const_str ".counter-val"
    %20 = struct_set %18 4 %19
    %21 = vec_push %9 %20
```
For large VDOM components, use the helper script [scripts/gen_vdom_ssa.py](./scripts/gen_vdom_ssa.py) to auto-number registers without human error.
