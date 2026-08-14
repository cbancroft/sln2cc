--- `vim.ui.select` wrappers for choosing a solution and a configuration/platform.
local cli = require("msvclsp.cli")

local M = {}

--- Prompt for the active solution. Runs discovery first if nothing is cached yet.
---@param on_choice fun(solution: table|nil)
function M.pick_solution(on_choice) -- luacheck: no unused args
  error("not yet implemented")
end

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
  cli.list_configs(solution, root, function(configs, err)
    if err then
      vim.notify("msvclsp: " .. err.message, vim.log.levels.ERROR)
      on_choice(nil, nil)
      return
    end

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
      cli.generate(solution, chosen, root, function(gen_result, gen_err)
        if gen_err then
          vim.notify("msvclsp: " .. gen_err.message, vim.log.levels.ERROR)
          on_choice(nil, nil)
          return
        end
        on_choice(chosen, gen_result.database)
      end)
    end)
  end)
end

return M
