--- `:checkhealth sln2cc`
---
--- Verifies the pieces a user is most likely to get wrong: the binary being present and
--- runnable, clangd being on PATH, a project root being detectable, a solution having
--- been selected, and a configuration/platform having been selected with a database on disk.

local config = require("sln2cc.config")
local discovery = require("sln2cc.discovery")
local cli = require("sln2cc.cli")

local M = {}

-- Overrideable seams for testing
M._executable = vim.fn.executable
M._system = vim.system
M._stat = (vim.uv or vim.loop).fs_stat

--- Runs `bin --version` synchronously and returns the trimmed first line, or nil if 
--- it did not succeed.
--- @param bin string
--- @return string|nil
local function binary_version(bin)
  local obj = M._system({bin, "--version"}, { text = true}):wait(2000)
  if obj.code ~= 0 then
    return nil
  end
  return vim.trim(obj.stdout)
end

local function check_binary()
  local bin = config.options.bin
  if M._executable(bin) ~= 1 then
    vim.health.error(
      ("%q is not executable"):format(bin),
      { "Install sln2cc and ensure it is on PATH, or set `bin` in setup()."}
    )
    return false
  end

  local version = binary_version(bin)
  if not version then
    vim.health.error(
      bin .. " did not respond to --version",
      { ("Run `%s --version` by hand to see the underlying failure."):format(bin) }
    )
    return false
  end
  vim.health.ok(version)
  return true
end

--- @return string root
local function check_root()
  local root = discovery.project_root()
  local matched = vim.fs.root(0, config.options.root_markers)
  if matched then
    vim.health.ok("project root: " .. root)
  else
    vim.health.warn(
      "No root marker found. Falling back to the current directory: " .. root,
      { "Open a file inside your solution's tree, or add a root marker (see `root_markers`)."}
    )
  end
  return root
end

--- @param root string
--- @return string|nil solution
local function check_solution(root)
  local solution, _, pin_error = discovery.active_solution(root)
  if pin_error then
    vim.health.error(
      "pinned solution does not exist: " .. pin_error,
      { "Fix or clear the `pinned_solution` config option." }
    )
    return nil
  end
  if not solution then
    vim.health.warn(
      "no .sln found under " .. root,
      { "Run :Sln2ccRescan after adding or moving a solution into the project tree."}
    )
    return nil
  end
  vim.health.ok("active solution: " .. solution)
  return solution
end

--- Blocks on cli.list_configs.
--- @param solution string
--- @param root string
local function check_database(solution, root)
  local result, err
  cli.list_configs(solution, root, function(r,e)
    result, err = r, e
  end)
  vim.wait(2000, function()
    return result ~= nil or err ~= nil
  end)

  if err then
    vim.health.error(
      "sln2cc list-configs failed: " .. err.message,
      {
        ("Run `%s list-configs %s --root %s` by hand to see the underlying failure."):format(
          config.options.bin,
          solution,
          root
        ),
      }
    )
    return
  end

  if not result.active then
    vim.health.warn(
      "no configuration/platform selected",
      { "Run :Sln2ccSelectConfig."}
    )
    return
  end

  local stat = M._stat(result.database)
  if not stat then
    vim.health.warn(
      "configuration selected but no database generated at " .. result.database,
      { "Run :Sln2ccGenerate."}
    )
  else
    vim.health.ok("database present: " .. result.database)
  end
end

local function check_clangd()
  local clangd = config.options.clangd_cmd[1]
  if M._executable(clangd) ~= 1 then
    vim.health.error(
      ("%q is not executable"):format(clangd),
      { "Install clangd and ensure it is on PATH." }
    )
  else
    vim.health.ok(clangd .. " found on PATH")
  end
end

function M.check()
  vim.health.start("sln2cc")

  local binary_ok = check_binary()
  check_clangd()
  local root = check_root()
  local solution = check_solution(root)

  if binary_ok and solution then
    check_database(solution, root)
  end
end

return M
