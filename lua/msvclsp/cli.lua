--- Invocation of the msvclsp binary and decoding of its JSON output.
---
--- All plugin-facing CLI output is structured JSON; nothing here should scrape
--- human-readable text.

local M = {}

--- Run a subcommand asynchronously and hand the decoded JSON to `on_done`.
---@param args string[] subcommand and arguments
---@param on_done fun(result: table|nil, err: string|nil)
function M.run(args, on_done) -- luacheck: no unused args
  error("not yet implemented")
end

--- `discover` — find .sln files under the project root.
function M.discover(root, on_done) -- luacheck: no unused args
  error("not yet implemented")
end

--- `list-configs` — configuration/platform options for a solution, plus its last-used one.
function M.list_configs(solution, on_done) -- luacheck: no unused args
  error("not yet implemented")
end

--- `generate` — emit compile_commands.json for a solution + configuration/platform.
function M.generate(solution, config, on_done) -- luacheck: no unused args
  error("not yet implemented")
end

return M
