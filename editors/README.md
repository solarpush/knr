# knr Editor Extensions

First-party editor integrations for `.knr`, `.ssa`, and `.ssa.schema` powered by the built-in `knr lsp` JSON-RPC 2.0 Language Server.

## Prerequisites

Ensure the `knr` CLI is installed and available on your `$PATH`:

```bash
knr --version
```

### Features Powered by `knr lsp`

- **Live Diagnostics** (`textDocument/publishDiagnostics`): real-time syntax parsing, lowering, static linear ownership / second-class borrow (`&T`) checks, effect capability checks (`Net`, `Ui`, `Hw`, `Async`), and GPU shader WGSL/GLSL validation via `naga`.
- **Hover Documentation** (`textDocument/hover`): signatures and docs for local `fn`, `shader fn`, `schema`, `enum`, fields/variants, built-in math & linear intrinsics, and all 40 `Style` / 8 `Widget` fields.
- **Autocomplete** (`textDocument/completion`): context-aware completions triggered on `.`, `:`, and `"` including keywords, snippets, `std/ui` & `std/math` imports, builtins, `Style`/`Widget` fields, and local symbols.
- **Document Outline** (`textDocument/documentSymbol`): hierarchical outline of schemas (with field slots), enums (with variants), functions, and GPU shaders.

---

## VS Code (`editors/vscode/`)

Zero external npm runtime dependencies — no `npm install` or build step required.

### Quick Install (Symlink or Copy)

```bash
ln -s "$(pwd)/editors/vscode" ~/.vscode/extensions/knr-lang
```

Reload VS Code (`Developer: Reload Window`) and open any `.knr`, `.ssa`, or `.ssa.schema` file.

### Run in Extension Development Host

```bash
code --extensionDevelopmentPath="$(pwd)/editors/vscode" examples/raymarch_3d.knr
```

### Settings & Commands

- `knr.serverPath` (default: `"knr"`): path to the `knr` executable.
- `knr.serverArgs` (default: `["lsp"]`): arguments passed to the executable.
- `knr.trace.server` (default: `"off"`): set to `"messages"` or `"verbose"` to inspect JSON-RPC traffic in the **knr Language Server** output channel.
- Command Palette: **knr: Restart Language Server** (`knr.restartServer`).

---

## Neovim 0.10+ (`editors/neovim/knr.lua`)

### Option A: Lua Module (Recommended)

Copy `editors/neovim/knr.lua` into your Neovim config:

```bash
mkdir -p ~/.config/nvim/lua
cp editors/neovim/knr.lua ~/.config/nvim/lua/knr.lua
```

Add to `~/.config/nvim/init.lua`:

```lua
require("knr").setup()
```

Optional customization:

```lua
require("knr").setup({
  cmd = { "knr", "lsp" },
  root_markers = { "knr.toml", "Cargo.toml", ".git" },
  on_attach = function(client, bufnr)
    vim.keymap.set("n", "K", vim.lsp.buf.hover, { buffer = bufnr })
  end,
})
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
