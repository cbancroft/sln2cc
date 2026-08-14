--- msvclsp.nvim — drives the msvclsp CLI and points clangd at the generated
--- compile_commands.json.

local M = {}

--- Entry point called by the user's plugin configuration.
---@param opts table|nil user configuration, merged over defaults
function M.setup(opts)
  local resolved = require("msvclsp.config").setup(opts)
  if resolved then
    require("msvclsp.commands").mark_ready()
  end
  return resolved
end

return M
