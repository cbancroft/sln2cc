--- User command registration.
local config = require("msvclsp.config")
local discovery = require("msvclsp.discovery")
local cli = require("msvclsp.cli")
local picker = require("msvclsp.picker")
local lsp = require("msvclsp.lsp")

local M = {}

--- Commands exposed to the user. Keymaps are left to the user's config rather than
--- claiming bindings by default.
M.commands = {
  MsvclspGenerate = "Regenerate compile_commands.json for the active solution",
  MsvclspSelectConfig = "Choose the configuration/platform for the active solution",
  MsvclspRescan = "Re-scan the project tree for .sln files",
  MsvclspRestart = "Restart clangd against the current compilation database",
}

--- Flipped by init.lua once config.setup() succeeds. Commands registerd before this is 
--- set notify instead of touching other modules.
M._ready = false

--- root -> last database path a generate produced for it. Convenience for 
--- MsvclspRestart. No other module needs it.
local last_database = {}

-- Marks commands live. Called by init.lua after config.setup().
function M.mark_ready()
  M._ready = true
end

--- Guards every command body. Returns false (and notifies) if setup() hasn't completed.
--- @return boolean
local function ensure_ready()
  if M._ready then
    return true
  end
  vim.notify("msvclsp: call require('msvclsp').setup() first", vim.log.levels.ERROR)
  return false
end

--- Resolves root + the active solution, notifying and returning nil if none is found.
--- Every command needs this pair, so its factored out rather than repeated 4 times.
--- @return string|nil solution
--- @return string|nil root
local function resolve_solution()
  local root = discovery.project_root()
  local solution = discovery.active_solution(root)
  if not solution then
    vim.notify("msvclsp: no solution found under " .. root, vim.log.levels.WARN)
    return nil, root
  end
  return solution, root
end

--- Restarts clangd for `root` iff config.options.auto_restart_clangd.
--- @param database string
--- @param root string
local function maybe_restart(database, root)
  last_database[root] = database
  if config.options.auto_restart_clangd then
    lsp.restart(database, root)
  end
end

function M._generate(opts)
  if not ensure_ready() then
    return
  end
  local solution, root = resolve_solution()
  if not solution then
    return
  end
  local force = opts and opts.bang or false

  cli.list_configs(solution, root, function(result, err)
    if err then
      vim.notify("msvclsp: " .. err.message, vim.log.levels.ERROR)
      return
    end
    if not result.active then
      vim.notify(
        "msvclsp: no configuration selected yet. Run :MsvclspSelectConfig",
        vim.log.levels.WARN
      )
      return
    end

    cli.generate(solution, result.active, root, force, function(gen_result, gen_err)
      if gen_err then
        vim.notify("msvclsp: " .. gen_err.message, vim.log.levels.ERROR)
        return
      end
      maybe_restart(gen_result.database, root)
    end)
  end)
end

function M._select_config()
  if not ensure_ready() then
    return
  end

  local solution, root = resolve_solution()
  if not solution then
    return
  end

  -- picker.pick_config's on_choice now hands back the database from its internal generate call, 
  -- along with the picked config.
  picker.pick_config(solution, root, function(chosen, database)
    if not chosen then
      return
    end
    maybe_restart(database, root)
  end)
end

function M._rescan()
  if not ensure_ready() then
    return
  end

  local root = discovery.project_root()
  local found = discovery.rescan(root)
  vim.notify(("msvclsp: found %d solutions"):format(#found), vim.log.levels.INFO)
end

function M._restart()
  if not ensure_ready() then
    return
  end
  local root = discovery.project_root()

  lsp.restart(last_database[root], root)
end

local handlers = {
  MsvclspGenerate = M._generate,
  MsvclspSelectConfig = M._select_config,
  MsvclspRescan = M._rescan,
  MsvclspRestart = M._restart,
}

--- Registers every command in M.commands
function M.register()
  for name, desc in pairs(M.commands) do
    local attrs = { desc = desc }
    if name == "MsvclspGenerate" then
      attrs.bang = true
    end
    vim.api.nvim_create_user_command(name, handlers[name], attrs)
  end
end

return M
