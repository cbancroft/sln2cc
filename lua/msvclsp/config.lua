--- User configuration and defaults.

local M = {}

M.defaults = {
  --- Path to the msvclsp binary. Resolution strategy is an open question
  --- (bundled build vs. release download vs. cargo install).
  bin = "msvclsp",

  --- Root markers used to locate the project root, matching what Neovim's LSP clients
  --- already use. `.msvclsp/` is created here.
  root_markers = { ".git" },

  --- Directory name for generated artifacts, relative to the project root.
  data_dir = ".msvclsp",

  --- Restart clangd automatically after regenerating the compilation database.
  auto_restart_clangd = true,
}

M.options = {}

function M.setup(opts)
  M.options = vim.tbl_deep_extend("force", {}, M.defaults, opts or {})
  return M.options
end

return M
