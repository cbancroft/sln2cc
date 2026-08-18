--- `vim.ui.select` wrappers for choosing a solution and a configuration/platform.
local cli = require("msvclsp.cli")
local discovery = require("msvclsp.discovery")
local notify = require("msvclsp.notify")

local M = {}

--- Renders a `ConfigPlatform` table.
--- @param cfg {configuration: string, platform: string}
--- @param active { configuration: string, platform: string}|nil
--- @return string
local function format_config(cfg, active)
  local marker = ""
  if active and cfg.configuration == active.configuration and cfg.platform == active.platform then
    marker = "  (active)"
  end

  return cfg.configuration .. "|" .. cfg.platform .. marker
end


--- Prompt for a configuration/platform belonging to `solution` and regenerate the compilation
--- database for whichever is chosen.
---
--- Every failure path is handled by `on_choice(nil)`.
--- @param solution string absolute path to the .sln file
--- @param root string project root passed through to CLI calls
--- @param on_choice fun(config: { configuration: string, platform: string }|nil, database: string|nil)
function M.pick_config(solution, root, on_choice)
  cli.list_configs(solution, root, function(configs, err, warnings)
    if err then
      notify.notify_error(err)
      on_choice(nil, nil)
      return
    end
    notify.notify_warnings(warnings)

    if #configs.available == 0 then
      vim.notify("msvclsp: no configurations declared in " .. solution, vim.log.levels.WARN)
      on_choice(nil, nil)
      return
    end

    vim.ui.select(configs.available, {
      prompt = "Select configuration/platform:",
      format_item = function(cfg)
        return format_config(cfg, configs.active)
      end,
    }, function(chosen)
      if not chosen then
        on_choice(nil, nil)
        return
      end

      vim.notify("msvclsp: generating compile_commands.json...", vim.log.levels.INFO)
      -- Forced: the CLI's mtime-based staleness check knows nothing about which
      -- configuration/platform was requested, so switching to a different one with no file
      -- changes would otherwise silently reuse the old database.
      cli.generate(solution, chosen, root, true, function(gen_result, gen_err, gen_warnings)
        if gen_err then
          notify.notify_error(gen_err)
          on_choice(nil, nil)
          return
        end
        notify.notify_warnings(gen_warnings)
        on_choice(chosen, gen_result.database)
      end)
    end)
  end)
end

--- Prompt for the active solution among those discovered under `root`. Selecting one persists 
--- it via discovery.set_active_solution so later sessions don't re-prompt. Does not regenerate
--- or restart clangd. That happens as part of V1-05.
--- @param root string project root
--- @param on_choice fun(solution: string|nil)
function M.pick_solution(root, on_choice)
  local solutions = discovery.solutions(root)
  if #solutions == 0 then
    vim.notify("msvclsp: no solutions found under " .. root, vim.log.levels.WARN)
    on_choice(nil)
    return
  end

  local active = discovery.active_solution(root)
  vim.ui.select(solutions, {
    prompt = "Select solution:",
    format_item = function(path)
      return vim.fs.basename(path) .. (path == active and "  (active)" or "")
    end,
  }, function(chosen)
    if not chosen then
      on_choice(nil)
      return
    end
    discovery.set_active_solution(root, chosen)
    on_choice(chosen)
  end)
end

return M
