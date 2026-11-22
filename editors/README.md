# knr Editor Extensions (v0.2.0)

First-party editor integrations for **`.tsk`** (*TypeScript for Knr* — Strict TSX/TS + linear ownership + GPU shaders), legacy `.knr`, and `.ssa` / `.ssa.schema` IR, powered exclusively by the built-in `knr lsp` JSON-RPC 2.0 Language Server.

> **Why `.tsk` instead of `.tsx`/`.ts`?**
> Using a dedicated `.tsk` extension completely avoids conflicts with `tsserver`, `vtsls`, ESLint, and Prettier (which would otherwise flag `i64`/`f64`, `readonly State`, top-level `@shader`/`@effects`, algebraic `enum`, and `"std/ui"` imports as TypeScript errors). Meanwhile, `.gitattributes` maps `*.tsk` to TSX on GitHub and all editor extensions ship native TSX + knr highlighting.

## Prerequisites

Ensure the `knr` v0.2.0 CLI is installed and available on your `$PATH`:

```bash
knr --version
```

### Features Powered by `knr lsp`

- **Live Diagnostics** (`textDocument/publishDiagnostics`): real-time `.tsk` parsing, JSX lowering, static linear ownership and `readonly T` second-class borrow checks, `@effects` capability checks (`net`, `ui`, `hw`, `async`), and `@shader` WGSL/GLSL validation via `naga`.
- **Hover Documentation** (`textDocument/hover`): signatures and docs for `function`, `@shader`, `type`, `enum`, built-in JSX primitives (`<Column>`, `<Row>`, `<Grid>`, `<Card>`, `<Shader>`, `<Icon>`, `<Code>`, ...), and math/sys intrinsics.
- **Autocomplete** (`textDocument/completion`): context-aware completions triggered on `.`, `:`, `"`, `<`, and `@` including `std/ui`, `std/math`, `std/anim`, `std/sys` imports, JSX props, icon names, and local symbols.
- **Document Outline** (`textDocument/documentSymbol`): hierarchical outline of types, enums, functions, components, and `@shader` kernels.

---

## VS Code (`editors/vscode/`)

Zero external npm runtime dependencies — no `npm install` or build step required. Activates exclusively on `.tsk`, `.knr`, `.ssa`, and `.ssa.schema` with a full built-in Strict TSX + knr TextMate grammar (`source.tsk`).

### Quick Install (Symlink or Copy)

```bash
ln -s "$(pwd)/editors/vscode" ~/.vscode/extensions/knr-lang
```

Reload VS Code (`Developer: Reload Window`) and open any `.tsk` file (e.g. `site/app.tsk` or `examples/counter_app.tsk`).

### Run in Extension Development Host

```bash
code --extensionDevelopmentPath="$(pwd)/editors/vscode" examples/raymarch_3d.tsk
```

### Settings & Commands

- `knr.serverPath` (default: `"knr"`): path to the `knr` executable.
- `knr.serverArgs` (default: `["lsp"]`): arguments passed to the executable.
- `knr.trace.server` (default: `"off"`): set to `"messages"` or `"verbose"` to inspect JSON-RPC traffic in the **knr Language Server** output channel.
- Command Palette: **knr: Restart Language Server** (`knr.restartServer`).

---

## Neovim 0.10+ (`editors/neovim/knr.lua`)

Automatically registers `.tsk` -> `knr` filetype, wires Neovim's Tree-sitter `tsx` parser for highlighting, and attaches `knr lsp` exclusively without waking `ts_ls`/`vtsls`.

### Option A: Lua Module (Recommended)

```bash
mkdir -p ~/.config/nvim/lua
cp editors/neovim/knr.lua ~/.config/nvim/lua/knr.lua
```

Add to `~/.config/nvim/init.lua`:

```lua
require("knr").setup()
```

### Option B: Zero-Config Plugin Drop-In

```bash
mkdir -p ~/.config/nvim/plugin
cp editors/neovim/knr.lua ~/.config/nvim/plugin/knr.lua
```

Use `:KnrLspRestart` at any time to restart `knr-lsp`.

---

## Zed (`editors/zed/`)

1. Open Zed and run **zed: install dev extension** from the Command Palette (`Cmd/Ctrl+Shift+P`).
2. Select the `editors/zed` directory in this repository.
3. Point `knr-lsp` to your `knr` binary in `~/.config/zed/settings.json`:

```json
{
  "lsp": {
    "knr-lsp": {
      "binary": {
        "path": "knr",
        "arguments": ["lsp"]
      }
    }
  }
}
```
