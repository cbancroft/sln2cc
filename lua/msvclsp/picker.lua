--- `vim.ui.select` wrappers for choosing a solution and a configuration/platform.

local M = {}

--- Prompt for the active solution. Runs discovery first if nothing is cached yet.
---@param on_choice fun(solution: table|nil)
function M.pick_solution(on_choice) -- luacheck: no unused args
  error("not yet implemented")
end

--- Prompt for a configuration/platform belonging to `solution`.
--- Options come from the CLI, since names are solution-specific.
---@param on_choice fun(config: table|nil)
function M.pick_config(solution, on_choice) -- luacheck: no unused args
  error("not yet implemented")
end

return M
