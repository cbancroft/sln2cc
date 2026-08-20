--- User command registration.
local config = require("sln2cc.config")
local discovery = require("sln2cc.discovery")
local cli = require("sln2cc.cli")
local picker = require("sln2cc.picker")
local lsp = require("sln2cc.lsp")
local notify = require("sln2cc.notify")

local M = {}

--- Commands exposed to the user. Keymaps are left to the user's config rather than
--- claiming bindings by default.
M.commands = {
  Sln2ccGenerate = "Regenerate compile_commands.json for the active solution",
  Sln2ccSelectConfig = "Choose the configuration/platform for the active solution",
  Sln2ccRescan = "Re-scan the project tree for .sln files",
  Sln2ccRestart = "Restart clangd against the current compilation database",
  Sln2ccSelectSolution = "Choose the active solution among discovered .sln files"
}

--- Flipped by init.lua once config.setup() succeeds. Commands registerd before this is 
--- set notify instead of touching other modules.
M._ready = false

--- root -> last database path a generate produced for it. Convenience for 
--- Sln2ccRestart. No other module needs it.
local last_database = {}

-- Marks commands live. Called by init.lua after config.setup().
function M.mark_ready()
  M._ready = true
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

--- Guards every command body. Returns false (and notifies) if setup() hasn't completed.
--- @return boolean
local function ensure_ready()
  if M._ready then
    return true
  end
  vim.notify("sln2cc: call require('sln2cc').setup() first", vim.log.levels.ERROR)
  return false
end

--- Resolves root + the active solution, calling `on_resolved(solution, root)`. `solution` is
--- nil (with a WARN notify) if nothing was found. A vanished active solution -- one whose .sln
--- no longer exists -- is not the same as "nothing found": it's cleared, the tree is rescanned,
--- and the user is reprompted via picker.pick_solution instead of silently falling back to a
--- different solution they never chose. See ADR 48. A configured `pinned_solution` that does not
--- exist on disk is reported as a hard ERROR instead, with no rescan or reprompt: a pin exists
--- for CI/scripting callers with no one to answer a picker. See ADR 49.
--- @param on_resolved fun(solution: string|nil, root: string)
local function resolve_solution(on_resolved)
  local root = discovery.project_root()
  local solution, vanished, pin_error = discovery.active_solution(root)

  if pin_error then
    vim.notify(
      "sln2cc: pinned solution does not exist: " .. pin_error,
      vim.log.levels.ERROR
    )
    on_resolved(nil, root)
    return
  end

  if vanished then
    vim.notify(
      "sln2cc: active solution no longer exists: " .. vanished .. ". Pick a replacement.",
      vim.log.levels.WARN
    )
    discovery.rescan(root)
    picker.pick_solution(root, function(chosen)
      on_resolved(chosen, root)
    end)
    return
  end

  if not solution then
    vim.notify("sln2cc: no solution found under " .. root, vim.log.levels.WARN)
    on_resolved(nil, root)
    return
  end
  on_resolved(solution, root)
end

--- Runs list_configs -> generate -> maybe_restart for `solution`. Switching solutions changes 
--- which files are in play so the mtime staleness check is useful here.
--- @param solution string
--- @param root string
--- @param force boolean
local function generate_and_restart(solution, root, force)
  cli.list_configs(solution, root, function(result, err, warnings)
    if err then
      notify.notify_error(err)
      return
    end
    notify.notify_warnings(warnings)

    if not result.active then
      vim.notify(
        "sln2cc: no configuration selected yet for "
        .. solution
        .. ". Run :Sln2ccSelectConfig",
        vim.log.levels.WARN
      )
      return
    end

    cli.generate(solution, result.active, root, force, function(gen_result, gen_err, gen_warnings)
      if gen_err then
        notify.notify_error(gen_err)
        return
      end
      notify.notify_warnings(gen_warnings)
      maybe_restart(gen_result.database, root)
    end)
  end)
end


function M._generate(opts)
  if not ensure_ready() then
    return
  end
  local force = opts and opts.bang or false
  resolve_solution(function(solution, root)
    if not solution then
      return
    end
    generate_and_restart(solution, root, force)
  end)
end

function M._select_config()
  if not ensure_ready() then
    return
  end

  resolve_solution(function(solution, root) 
    if not solution then
      return
    end
    picker.pick_config(solution, root, function(chosen, database) 
      if not chosen then
        return
      end
      maybe_restart(database, root)
    end)
  end)
end

function M._select_solution()
  if not ensure_ready() then
    return
  end

  local root = discovery.project_root()
  picker.pick_solution(root, function(solution)
    if not solution then
      return
    end
    -- The previous solution's generated db no longer applies to this root
    last_database[root] = nil
    generate_and_restart(solution, root, false)
  end)
end

function M._rescan()
  if not ensure_ready() then
    return
  end

  local root = discovery.project_root()
  local found = discovery.rescan(root)
  vim.notify(("sln2cc: found %d solutions"):format(#found), vim.log.levels.INFO)
end

function M._restart()
  if not ensure_ready() then
    return
  end
  local root = discovery.project_root()

  lsp.restart(last_database[root], root)
end

local handlers = {
  Sln2ccGenerate = M._generate,
  Sln2ccSelectConfig = M._select_config,
  Sln2ccRescan = M._rescan,
  Sln2ccRestart = M._restart,
  Sln2ccSelectSolution = M._select_solution,
}

--- Registers every command in M.commands
function M.register()
  for name, desc in pairs(M.commands) do
    local attrs = { desc = desc }
    if name == "Sln2ccGenerate" then
      attrs.bang = true
    end
    vim.api.nvim_create_user_command(name, handlers[name], attrs)
  end
end

return M
