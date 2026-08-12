--- Project root detection, solution discovery, and the discovery cache.
---
--- Scanning is cached rather than repeated on every command. A manual rescan refreshes it.
--- If no cache exists when a solution is first needed, discovery runs inline so selection
--- stays a single user action.

local config = require("msvclsp.config")

local M = {}

--- bufnr -> resolved root.
local root_cache = {}

--- Locate the project root by walking up from `bufnr` for a configured root marker.
--- Falls back to the current working directory when no marker is found.
--- @param bufnr integer|nil defaults to the current buffer
--- @return string
function M.project_root(bufnr)
  bufnr = bufnr or 0
  local real_bufnr = bufnr == 0 and vim.api.nvim_get_current_buf() or bufnr

  local cached = root_cache[real_bufnr]
  if cached then
    return cached
  end

  local root = vim.fs.root(bufnr, config.options.root_markers) or vim.fn.getcwd()
  root_cache[real_bufnr] = root
  return root
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
