--- User command registration.

local M = {}

--- Commands exposed to the user. Keymaps are left to the user's config rather than
--- claiming bindings by default.
M.commands = {
  MsvclspGenerate = "Regenerate compile_commands.json for the active solution",
  MsvclspSelectSolution = "Choose the active solution (discovers first if needed)",
  MsvclspSelectConfig = "Choose the configuration/platform for the active solution",
  MsvclspRescan = "Re-scan the project tree for .sln files",
  MsvclspRestart = "Restart clangd against the current compilation database",
}

function M.register()
  error("not yet implemented")
end

return M
