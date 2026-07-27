--- clangd lifecycle management.
---
--- clangd is always launched pointed at an explicit compile_commands.json under
--- `.msvclsp/<solution-name>/`, never relying on its own directory-walk discovery, because
--- a solution's location in the tree may not line up with where sources live.

local M = {}

--- Path to the compilation database for a solution.
function M.compile_commands_dir(solution) -- luacheck: no unused args
  error("not yet implemented")
end

--- Start clangd for the active solution.
function M.start()
  error("not yet implemented")
end

--- Restart clangd — used after regenerating the database, switching configuration/platform,
--- or switching solutions.
function M.restart()
  error("not yet implemented")
end

return M
