--- Project root detection, solution discovery, and the discovery cache.
---
--- Scanning is cached rather than repeated on every command. A manual rescan refreshes it.
--- If no cache exists when a solution is first needed, discovery runs inline so selection
--- stays a single user action.

local config = require("msvclsp.config")

local M = {}

--- bufnr -> resolved root.
local root_cache = {}

--- root -> sorted string[] of discovered .sln absolute paths
local solutions_cache = {}

--- root -> memoized active solution path (or false, meaning we scanned and found nothing).
local active_cache = {}

--- Recursively collects `.sln` paths under `dir`, skipping `config.options.scan_ignore_dirs`
--- by name and never descending past `config.options.scan_max_depth` levels below `root`.
--- @param dir string
--- @param root string
--- @param depth integer
--- @param ignore table<string, true>
--- @param acc string[]
local function scan(dir, root, depth, ignore, acc)
  if depth > config.options.scan_max_depth then
    return
  end
  local ok, entries = pcall(vim.fs.dir, dir)

  if not ok then
    return
  end

  for name, kind in entries do
    if kind == "directory" then
      if not ignore[name] then
        scan(dir .. "/" .. name, root, depth + 1, ignore, acc)
      end
    elseif kind == "file" and name:match("%.sln$") then
      table.insert(acc, dir .. "/" .. name)
    end
  end
end

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

--- Discovered `.sln` paths under `root` (default: `M.project_root()`), sorted ascending.
--- Scans and populates the cache on first use; subsequent calls for the same root return the
--- cached result without rescanning.
--- @param root string|nil
--- @return string[]
function M.solutions(root)
  root = root or M.project_root()
  local cached = solutions_cache[root]
  if cached then
    return cached
  end

  local ignore = {}
  for _, name in ipairs(config.options.scan_ignore_dirs) do
    ignore[name] = true
  end

  local found = {}

  scan(root, root, 0, ignore, found)
  table.sort(found)

  solutions_cache[root] = found
  return found
end

--- Discards the solutions and active-solution cache for `root` (default: `M.project_root()`)
--- and scans again, returning the refreshed list. Leaves `M.project_root()`'s per-buffer cache
--- untouched.
--- @param root string|nil
--- @return string[]
function M.rescan(root)
  root = root or M.project_root()
  solutions_cache[root] = nil
  active_cache[root] = nil
  return M.solutions(root)
end

--- The active solution for `root` (default: `M.project_root()`): the first (sorted) `.sln`
--- discovered by `M.solutions()`, memoized. MVP assumes a single solution; choosing a
--- different one than "the first found" is V1's picker's job. `nil` if none was found.
--- @param root string|nil
--- @return string|nil
function M.active_solution(root)
  root = root or M.project_root()
  local cached = active_cache[root]
  if cached ~= nil then
    return cached or nil
  end

  local solution = M.solutions(root)[1]
  active_cache[root] = solution or false
  return solution
end

return M
