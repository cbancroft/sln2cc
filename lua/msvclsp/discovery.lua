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

local SOLUTION_STATE_FILE = "nvim-solution.json"

--- Path to the persisted active solution file for `root`, under its own `.msvclsp` equivalent data directory.
local function persisted_path(root)
  return root .. '/' .. config.options.data_dir .. '/' .. SOLUTION_STATE_FILE
end

--- True if `path` exists on disk.
--- @param path string
--- @return boolean
local function exists(path)
  return (vim.uv or vim.loop).fs_stat(path) ~= nil
end

--- Removes the persisted active solution file for `root`, if any. Called when the persisted choice's `.sln`
--- no longer exists.
--- @param root string
local function clear_persisted(root)
  os.remove(persisted_path(root))
end


--- Reads the persisted solution for `root`, or nil if nothing is recorded, the file is absent,
--- or it fails to parse. 
--- @param root string
--- @return string|nil
local function read_persisted(root)
  local fd = io.open(persisted_path(root), "r")
  if not fd then
    return nil
  end
  local contents = fd:read("*a")
  fd:close()

  local ok, decoded = pcall(vim.json.decode, contents)
  if not ok or type(decoded) ~= "table" then
    return nil
  end
  return decoded.solution
end


--- The active solution for `root` (default: `M.project_root()`)
--- Resolution order:
---
--- 1. `config.options.pinned_solution`, if set is unconditionally taken as the active solution.
--- 2. Choice persisted via `M.set_active_solution()`, if the file exists
--- 3. The first (sorted) `.sln` discovered by `M.solutions()`.
---
--- Existence checked every call.
--- @param root string|nil
--- @return string|nil solution
--- @return string|nil vanished the path that was active until this call discovered it gone. Never set 
---         while a pin is configured.
--- @return string|nil pin_error the configured `pinned_solution` path is set but the file it points
---         to does not exist.
function M.active_solution(root)
  root = root or M.project_root()

  local pinned = config.options.pinned_solution
  if pinned ~= "" then
    if exists(pinned) then
      return pinned, nil, nil
    end
    return nil, nil, pinned
  end

  local candidate = active_cache[root]
  if candidate == nil then
    local persisted = read_persisted(root)
    candidate = persisted or M.solutions(root)[1]
    active_cache[root] = candidate or false
  end
  candidate = candidate or nil

  if candidate and not exists(candidate) then
    active_cache[root] = nil
    clear_persisted(root)
    return nil, candidate, nil
  end
  return candidate, nil, nil
end

--- Writes `solution` as the persisted choice for `root`. Write failures are printed to the user
--- but don't block execution.
--- @param root string
--- @param solution string
local function write_persisted(root, solution)
  local dir = root .. "/" .. config.options.data_dir
  vim.fn.mkdir(dir, "p")
  local fd = io.open(persisted_path(root), "w")
  if not fd then
    vim.notify("msvclsp: could not persist the selected solution for " .. root, vim.log.levels.WARN)
    return
  end
  fd:write(vim.json.encode( { solution = solution }))
  fd:close()
end

--- Explicitly sets the active solution for `root`, persisting it so later sessions don't re-prompt.
--- @param root string
--- @param solution string
function M.set_active_solution(root, solution)
  active_cache[root] = solution
  write_persisted(root, solution)
end

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


return M
