--- Invocation of the sln2cc binary and decoding of its JSON output.
---
--- All plugin-facing CLI output is structured JSON; nothing here should scrape
--- human-readable text.

local config = require("sln2cc.config")

local M = {}

--- Overridable so tests can intercept the actual spawn without a binary. 
--- @type fun(cmd: string[], opts: table, on_exit: fun(obj: vim.SystemCompleted))
M._system = vim.system

--- @class Sln2cc.CliError
--- @field message string human-readable, folded in from stderr
--- @field kind string|nil ErrorKind::as_str(), when the envelope decoded and carried one
--- @field code integer|nil process exit code; nil only for a spawn failure

--- Turns a completed process into a `(result, err)`, the shape every `on_done` receives.
--- Separate from `M.run` so that decode logic is testable on a plain `obj` table without
--- going through `vim.system`.
--- @param obj vim.SystemCompleted
--- @return table|nil result
--- @return Sln2cc.CliError|nil error
local function handle_exit(obj)
  -- luanil converts JSON null to real Lua nil (absent key) rather than the vim.NIL sentinel,
  -- so downstream callers can use plain `if not x then` on optional fields like `active`.
  local decoded_ok, envelope =
    pcall(vim.json.decode, obj.stdout, { luanil = { object = true, array = true } })

  if obj.code == 0 and decoded_ok and envelope.has_error == false then
    return envelope, nil
  end

  local message = obj.stderr ~= "" and vim.trim(obj.stderr)
                  or ("sln2cc exited with code " .. obj.code)
  -- `envelope.error.kind` only exists when stdout decoded *and* the tool reported the
  -- failure (has_error== true). A clap usage error (exit 2) leaves stdout empty so
  -- `decoded_ok` is false there and kind stays nil.
  local kind = (decoded_ok and envelope.has_error) and envelope.error.kind or nil
  return nil, { message = message, kind = kind, code = obj.code }
end


--- Run a subcommand asynchronously and hand the decoded envelope to `on_done`.
---
--- `args` excludes `--json`. It is always appended so no call site can forget it
---@param args string[] subcommand and arguments
---@param on_done fun(result: table|nil, err: Sln2cc.CliError|nil)
function M.run(args, on_done)
  local cmd = { config.options.bin }
  vim.list_extend(cmd, args)
  table.insert(cmd, "--json")

  -- `vim.system` raises synchronously on a spawn failure (e.g. ENOENT) rather than reaching
  -- `on_exit`, so a missing binary has to be caught here rather than inside the callback.
  local ok, err = pcall(M._system, cmd, { text = true }, function(obj)
    -- `on_exit` runs off the main loop. `vim.json.decode` and whatever `on_done` does
    -- downstream both need it.
    vim.schedule(function() on_done(handle_exit(obj)) end)
  end)

  if not ok then
    on_done(nil, { message = tostring(err), kind = nil, code = nil})
  end

end

--- `list-configs` — configuration/platform options for a solution, plus its last-used one.
--- @param solution string absolute path to the .sln
--- @param root string project root to create/read `.sln2cc` under
--- @param on_done fun(result: table|nil, err: Sln2cc.CliError|nil, warnings: table[]|nil)
function M.list_configs(solution, root, on_done)
  M.run({ "list-configs", solution, "--root", root}, function(envelope, err)
    if err then
      on_done(nil, err, nil)
    else
      on_done(envelope.result, nil, envelope.warnings)
    end
  end)
end

--- `generate` — emit compile_commands.json for a solution + configuration/platform.
--- @param solution string absolute path to the .sln
--- @param cfg { configuration: string, platform: string }
--- @param root string project root to create/read `.sln2cc` under
--- @param force boolean regenerate unconditionally
--- @param on_done fun(result: table|nil, err: Sln2cc.CliError|nil, warnings: table[]|nil)
function M.generate(solution, cfg, root, force, on_done)
  local args = {
    "generate",
    solution,
    "--configuration",
    cfg.configuration,
    "--platform",
    cfg.platform,
    "--root",
    root,
  }
  if force then
    table.insert(args, "--force")
  end
  M.run(args, function(envelope, err)
    if err then
      on_done(nil, err, nil)
    else
      on_done(envelope.result, nil, envelope.warnings)
    end
  end)
end

return M
