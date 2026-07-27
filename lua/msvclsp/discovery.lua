--- Project root detection, solution discovery, and the discovery cache.
---
--- Scanning is cached rather than repeated on every command. A manual rescan refreshes it.
--- If no cache exists when a solution is first needed, discovery runs inline so selection
--- stays a single user action.

local M = {}

--- Locate the project root using the configured root markers.
---@return string|nil
function M.project_root()
  error("not yet implemented")
end

--- Discovered solutions for the current project root, populating the cache if empty.
function M.solutions()
  error("not yet implemented")
end

--- Discard the cache and scan again.
function M.rescan()
  error("not yet implemented")
end

--- The workspace's remembered active solution, if one has been chosen.
function M.active_solution()
  error("not yet implemented")
end

return M
