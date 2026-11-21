-- knr Language & Built-in LSP integration for Neovim 0.10+
--
-- Usage:
--   1. Copy to ~/.config/nvim/lua/knr.lua and call in init.lua:
--        require("knr").setup()
--   2. Or customize options:
--        require("knr").setup({
--          cmd = { "knr", "lsp" },
--          root_markers = { "knr.toml", "Cargo.toml", ".git" },
--          on_attach = function(client, bufnr) end,
--        })
--   3. Alternatively, drop directly into ~/.config/nvim/plugin/knr.lua for zero-config auto-setup.

local M = {}

local DEFAULT_CONFIG = {
  cmd = { "knr", "lsp" },
  root_markers = { "knr.toml", "Cargo.toml", ".git" },
  capabilities = nil,
  on_attach = nil,
}

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

  -- Register .knr, .ssa, and .ssa.schema filetypes
  vim.filetype.add({
    extension = {
      knr = "knr",
      ssa = "knr_ssa",
    },
    pattern = {
      [".*%.ssa%.schema"] = "knr_ssa",
    },
  })

  local group = vim.api.nvim_create_augroup("knr_lsp", { clear = true })

  -- Buffer editing defaults for both .knr and .ssa/.ssa.schema
  vim.api.nvim_create_autocmd("FileType", {
    group = group,
    pattern = { "knr", "knr_ssa" },
    callback = function(args)
      configure_buffer_options(args.buf)
    end,
  })

  -- Attach built-in `knr lsp` server to .knr buffers
  vim.api.nvim_create_autocmd("FileType", {
    group = group,
    pattern = "knr",
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
        capabilities = opts.capabilities,
        on_attach = opts.on_attach,
      }, { bufnr = args.buf })
    end,
  })

  vim.api.nvim_create_user_command("KnrLspRestart", function()
    for _, client in ipairs(vim.lsp.get_clients({ name = "knr-lsp" })) do
      client:stop()
    end
    vim.defer_fn(function()
      if vim.bo.filetype == "knr" then
        vim.cmd("doautocmd FileType knr")
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
