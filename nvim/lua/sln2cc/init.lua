--- sln2cc.nvim — drives the sln2cc CLI and points clangd at the generated
--- compile_commands.json.

local M = {}

--- Entry point called by the user's plugin configuration.
---@param opts table|nil user configuration, merged over defaults
function M.setup(opts)
  local resolved = require("sln2cc.config").setup(opts)
  if resolved then
    require("sln2cc.commands").mark_ready()
  end
  return resolved
end

return M
