--- msvclsp.nvim — drives the msvclsp CLI and points clangd at the generated
--- compile_commands.json.

local M = {}

--- Entry point called by the user's plugin configuration.
---@param opts table|nil user configuration, merged over defaults
function M.setup(opts)
  require("msvclsp.config").setup(opts)
  require("msvclsp.commands").register()
end

return M
