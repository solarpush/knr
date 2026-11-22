-- knr Language (.tsk) & Built-in IDE-Ultra LSP integration for Neovim 0.10+ (v0.4.0)
-- Dedicated `.tsk` (Strict TypeScript/TSX for knr), `.knr`, `.ssa`, and `.ssa.schema`
-- support with ZERO conflicts with ts_ls / vtsls.
--
-- IDE-Ultra LSP 3.17 capabilities provided out-of-the-box by `knr lsp`:
--   - Live diagnostics (parse, lower, linear ownership, effects, WGSL/GLSL @shader verification)
--   - Hover docs (local, cross-file, package, std/*, 45 Style fields, 15 array methods)
--   - Rich completions (JSX tags/props, 67 <Icon name="..."> icons, std/theme, std/router,
--     std/layout, std/testing, knr.toml [dependencies] packages, array methods)
--   - Signature help (`textDocument/signatureHelp`, triggers: `(`, `,`)
--   - Inlay hints (`textDocument/inlayHint`: inferred `const`/`let` types & call param names)
--   - Document & Workspace symbols (`textDocument/documentSymbol`, `workspace/symbol`)
--   - Semantic code folding (`textDocument/foldingRange`: comments, imports, multi-line decls)
--   - Go-to-Definition (locals, destructured bindings, cross-file, knr.toml packages)
--   - Find All References, Cross-File Rename, QuickFix Code Actions & Document Formatting
--
-- Usage:
--   1. Copy to ~/.config/nvim/lua/knr.lua and call in init.lua:
--        require("knr").setup()
--   2. Or customize options:
--        require("knr").setup({
--          cmd = { "knr", "lsp" },
--          filetypes = { "knr", "knr_ssa" },
--          root_markers = { "knr.toml", "Cargo.toml", ".git" },
--          inlay_hints = true,
--          lsp_folding = false, -- set true to use `vim.lsp.foldexpr()` for knr buffers
--          on_attach = function(client, bufnr) end,
--        })
--   3. Alternatively, drop directly into ~/.config/nvim/plugin/knr.lua for zero-config auto-setup.

local M = {}

local DEFAULT_CONFIG = {
  cmd = { "knr", "lsp" },
  filetypes = { "knr", "knr_ssa" },
  root_markers = { "knr.toml", "Cargo.toml", ".git" },
  inlay_hints = true,
  lsp_folding = false,
  capabilities = nil,
  on_attach = nil,
}

local function build_default_capabilities()
  local caps = vim.lsp.protocol.make_client_capabilities()
  caps.textDocument = caps.textDocument or {}
  caps.textDocument.signatureHelp = caps.textDocument.signatureHelp or {
    signatureInformation = {
      documentationFormat = { "markdown", "plaintext" },
      parameterInformation = { labelOffsetSupport = false },
    },
  }
  caps.textDocument.inlayHint = caps.textDocument.inlayHint or {
    dynamicRegistration = false,
  }
  caps.textDocument.foldingRange = caps.textDocument.foldingRange or {
    dynamicRegistration = false,
    lineFoldingOnly = true,
  }
  caps.workspace = caps.workspace or {}
  caps.workspace.symbol = caps.workspace.symbol or {
    dynamicRegistration = false,
  }
  return caps
end

local missing_binary_notified = false

local function resolve_root_dir(bufnr, root_markers)
  if vim.fs and vim.fs.root then
    local root = vim.fs.root(bufnr, root_markers)
    if root then
      return root
    end
  end

  local fname = vim.api.nvim_buf_get_name(bufnr)
  if fname ~= "" then
    return vim.fs.dirname(fname)
  end

  return vim.fn.getcwd()
end

local function configure_buffer_options(bufnr)
  vim.bo[bufnr].commentstring = "// %s"
  vim.bo[bufnr].tabstop = 4
  vim.bo[bufnr].shiftwidth = 4
  vim.bo[bufnr].expandtab = true
end

function M.setup(user_opts)
  local opts = vim.tbl_deep_extend("force", {}, DEFAULT_CONFIG, user_opts or {})

  -- Register .tsk (primary), .knr, .ssa, and .ssa.schema filetypes (never collides with .ts/.tsx)
  vim.filetype.add({
    extension = {
      tsk = "knr",
      knr = "knr",
      ssa = "knr_ssa",
    },
    pattern = {
      [".*%.ssa%.schema"] = "knr_ssa",
    },
  })

  -- Reuse Tree-sitter's native TSX parser for rich `.tsk` syntax highlighting if installed
  if vim.treesitter and vim.treesitter.language and vim.treesitter.language.register then
    pcall(vim.treesitter.language.register, "tsx", "knr")
  end

  local group = vim.api.nvim_create_augroup("knr_lsp", { clear = true })

  vim.api.nvim_create_autocmd("FileType", {
    group = group,
    pattern = { "knr", "knr_ssa" },
    callback = function(args)
      configure_buffer_options(args.buf)
    end,
  })

  -- Attach built-in `knr lsp` server exclusively to knr filetypes
  vim.api.nvim_create_autocmd("FileType", {
    group = group,
    pattern = opts.filetypes,
    callback = function(args)
      local bin = opts.cmd[1]
      if vim.fn.executable(bin) ~= 1 then
        if not missing_binary_notified then
          missing_binary_notified = true
          vim.notify(
            ("knr-lsp: executable '%s' not found on PATH. Install knr or pass cmd = { '/path/to/knr', 'lsp' }."):format(
              bin
            ),
            vim.log.levels.WARN
          )
        end
        return
      end

      vim.lsp.start({
        name = "knr-lsp",
        cmd = opts.cmd,
        root_dir = resolve_root_dir(args.buf, opts.root_markers),
        capabilities = opts.capabilities or build_default_capabilities(),
        on_attach = function(client, bufnr)
          if
            opts.inlay_hints
            and vim.lsp.inlay_hint
            and client:supports_method("textDocument/inlayHint")
          then
            pcall(vim.lsp.inlay_hint.enable, true, { bufnr = bufnr })
          end
          if
            opts.lsp_folding
            and vim.lsp.foldexpr
            and client:supports_method("textDocument/foldingRange")
          then
            vim.wo.foldmethod = "expr"
            vim.wo.foldexpr = "v:lua.vim.lsp.foldexpr()"
          end
          if type(opts.on_attach) == "function" then
            opts.on_attach(client, bufnr)
          end
        end,
      }, { bufnr = args.buf })
    end,
  })

  vim.api.nvim_create_user_command("KnrLspRestart", function()
    for _, client in ipairs(vim.lsp.get_clients({ name = "knr-lsp" })) do
      client:stop()
    end
    vim.defer_fn(function()
      local ft = vim.bo.filetype
      if ft ~= "" then
        vim.cmd("doautocmd FileType " .. ft)
      end
    end, 200)
  end, { desc = "Restart the knr-lsp Language Server" })
end

-- Auto-setup when placed directly in ~/.config/nvim/plugin/
local source_path = debug.getinfo(1, "S").source or ""
if source_path:match("[/\\]plugin[/\\]knr%.lua$") then
  M.setup()
end

return M
