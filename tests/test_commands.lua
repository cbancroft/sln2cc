-- commands.lua stubs discovery.lua, cli.lua, picker.lua, and lsp.lua wholesale
-- (package.loaded swap), the same seam every non-cli.lua module's tests use per CLAUDE.md §4 --
-- these specs never invoke the real binary or touch real LSP state.

--- Builds a fake `msvclsp.discovery`. `vanished` (V2-01) makes `active_solution` report a
--- vanished path as its second return value, alongside a nil solution -- mirroring what the
--- real module does when the resolved `.sln` no longer exists on disk. See ADR 48. `pin_error`
--- (V2-02) makes it report a configured-but-missing pinned solution as the third return value
--- instead -- a distinct case from `vanished`, since it must not trigger the same
--- rescan-and-reprompt flow. See ADR 49.
--- @param opts { root: string|nil, solution: string|nil, vanished: string|nil, pin_error: string|nil, rescan_result: string[]|nil }
--- @return table fake_discovery
--- @return table[] rescan_calls each is the root passed
local function fake_discovery(opts)
  opts = opts or {}
  local root = opts.root or "/root"
  local rescan_calls = {}
  local fake = {
    project_root = function()
      return root
    end,
    active_solution = function()
      return opts.solution, opts.vanished, opts.pin_error
    end,
    rescan = function(r)
      table.insert(rescan_calls, r)
      return opts.rescan_result or {}
    end,
  }
  return fake, rescan_calls
end

--- Builds a fake `msvclsp.cli`. `list_configs_result`/`generate_result` are each
--- `{ result, err, warnings }` triples handed straight to their respective `on_done`
--- callbacks synchronously -- `warnings` (V2-04) mirrors the real `on_done(result, err,
--- warnings)` 3rd argument ADR 50 adds, defaulting to nil so every pre-existing opts table
--- (which never set it) still exercises the "no warnings" path. `list_configs_by_solution`
--- (V1-05) lets a test give different solutions different answers -- needed to prove a
--- solution switch restores *that* solution's own active configuration rather than a
--- carried-over global value -- and takes priority over the flat `list_configs` when both
--- are given.
--- @param opts { list_configs: table|nil, list_configs_by_solution: table<string, table>|nil, generate: table|nil }
--- @return table fake_cli
--- @return table[] list_configs_calls
--- @return table[] generate_calls each also records `force`, for MVP-46
local function fake_cli(opts)
  opts = opts or {}
  local list_configs_calls, generate_calls = {}, {}
  local fake = {
    list_configs = function(solution, root, on_done)
      table.insert(list_configs_calls, { solution = solution, root = root })
      local lc = (opts.list_configs_by_solution and opts.list_configs_by_solution[solution])
        or opts.list_configs
        or { result = { active = nil } }
      on_done(lc.result, lc.err, lc.warnings)
    end,
    generate = function(solution, cfg, root, force, on_done)
      table.insert(generate_calls, { solution = solution, cfg = cfg, root = root, force = force })
      local gen = opts.generate or { result = { database = "compile_commands.json" } }
      on_done(gen.result, gen.err, gen.warnings)
    end,
  }
  return fake, list_configs_calls, generate_calls
end

--- Builds a fake `msvclsp.picker`. `pick_config` calls `on_choice` synchronously with
--- `opts.config`/`opts.database`; `pick_solution` (V1-03) calls its `on_choice` synchronously
--- with `opts.solution`.
--- @param opts { config: table|nil, database: string|nil, solution: string|nil }|nil
--- @return table fake_picker
--- @return table[] pick_config_calls
--- @return table[] pick_solution_calls each { root }
local function fake_picker(opts)
  opts = opts or {}
  local pick_config_calls, pick_solution_calls = {}, {}
  local fake = {
    pick_config = function(solution, root, on_choice)
      table.insert(pick_config_calls, { solution = solution, root = root })
      on_choice(opts.config, opts.database)
    end,
    pick_solution = function(root, on_choice)
      table.insert(pick_solution_calls, { root = root })
      on_choice(opts.solution)
    end,
  }
  return fake, pick_config_calls, pick_solution_calls
end

--- Builds a fake `msvclsp.lsp`.
--- @return table fake_lsp
--- @return table[] restart_calls each { database, root }
local function fake_lsp()
  local calls = {}
  local fake = {
    restart = function(database, root)
      table.insert(calls, { database = database, root = root })
    end,
  }
  return fake, calls
end

--- Mirrors test_config.lua's stub_notify: replaces vim.notify for one test, collecting every
--- call instead of printing to the test runner's own output.
local function stub_notify()
  local calls = {}
  local original = vim.notify
  vim.notify = function(msg, level)
    table.insert(calls, { msg = msg, level = level })
  end
  return calls, function()
    vim.notify = original
  end
end

--- Loads commands.lua fresh against the given fakes, without marking it ready.
--- @param fakes { discovery: table|nil, cli: table|nil, picker: table|nil, lsp: table|nil }
--- @return table commands
local function load_commands(fakes)
  package.loaded["msvclsp.commands"] = nil
  package.loaded["msvclsp.discovery"] = fakes.discovery
  package.loaded["msvclsp.cli"] = fakes.cli
  package.loaded["msvclsp.picker"] = fakes.picker
  package.loaded["msvclsp.lsp"] = fakes.lsp
  return require("msvclsp.commands")
end

--- Loads, registers, and readies commands.lua -- the state every spec but the "before setup"
--- and "register" ones themselves needs before invoking a command.
--- @param fakes { discovery: table|nil, cli: table|nil, picker: table|nil, lsp: table|nil }
--- @return table commands
local function ready_commands(fakes)
  local commands = load_commands(fakes)
  commands.register()
  commands.mark_ready()
  return commands
end

describe("msvclsp.commands", function()
  local notify_calls, restore_notify
  local config

  before_each(function()
    package.loaded["msvclsp.config"] = nil
    config = require("msvclsp.config")
    config.setup({})
    notify_calls, restore_notify = stub_notify()
  end)

  after_each(function()
    restore_notify()
  end)

  describe("register", function()
    it("creates all five commands, including MsvclspSelectSolution (V1-03)", function()
      local commands = load_commands({
        discovery = fake_discovery(),
        cli = (fake_cli()),
        picker = (fake_picker()),
        lsp = (fake_lsp()),
      })

      commands.register()

      MiniTest.expect.equality(vim.fn.exists(":MsvclspGenerate"), 2)
      MiniTest.expect.equality(vim.fn.exists(":MsvclspSelectConfig"), 2)
      MiniTest.expect.equality(vim.fn.exists(":MsvclspRescan"), 2)
      MiniTest.expect.equality(vim.fn.exists(":MsvclspRestart"), 2)
      MiniTest.expect.equality(vim.fn.exists(":MsvclspSelectSolution"), 2)
      MiniTest.expect.equality(type(commands.commands.MsvclspSelectSolution), "string")
    end)

    it("accepts a bang only on MsvclspGenerate", function()
      -- MVP-46: :MsvclspGenerate! forces regeneration. The other commands have no meaning for
      -- a bang, and Neovim itself refuses `!` on a command that didn't declare it -- that
      -- refusal is the right behavior for them, not something to route around.
      load_commands({
        discovery = fake_discovery(),
        cli = (fake_cli()),
        picker = (fake_picker()),
        lsp = (fake_lsp()),
      }).register()

      local registered = vim.api.nvim_get_commands({})
      MiniTest.expect.equality(registered.MsvclspGenerate.bang, true)
      MiniTest.expect.equality(registered.MsvclspSelectConfig.bang, false)
      MiniTest.expect.equality(registered.MsvclspRescan.bang, false)
      MiniTest.expect.equality(registered.MsvclspRestart.bang, false)
      MiniTest.expect.equality(registered.MsvclspSelectSolution.bang, false)
    end)
  end)

  describe("commands invoked before setup", function()
    it("each command notifies a clear error and dispatches to no other module", function()
      local discovery, rescan_calls = fake_discovery({ solution = "app.sln" })
      local cli, list_calls = fake_cli()
      local chosen = { configuration = "Debug", platform = "x64" }
      local picker, pick_calls, pick_solution_calls = fake_picker({ config = chosen })
      local lsp, restart_calls = fake_lsp()
      local commands =
        load_commands({ discovery = discovery, cli = cli, picker = picker, lsp = lsp })
      commands.register()

      vim.cmd("MsvclspGenerate")
      vim.cmd("MsvclspSelectConfig")
      vim.cmd("MsvclspRescan")
      vim.cmd("MsvclspRestart")
      vim.cmd("MsvclspSelectSolution")

      MiniTest.expect.equality(#list_calls, 0)
      MiniTest.expect.equality(#pick_calls, 0)
      MiniTest.expect.equality(#pick_solution_calls, 0)
      MiniTest.expect.equality(#rescan_calls, 0)
      MiniTest.expect.equality(#restart_calls, 0)
      MiniTest.expect.equality(#notify_calls, 5)
      for _, call in ipairs(notify_calls) do
        MiniTest.expect.equality(call.level, vim.log.levels.ERROR)
      end
    end)
  end)

  describe("MsvclspGenerate", function()
    it("with no active solution notifies and never calls list_configs", function()
      local discovery = fake_discovery({ solution = nil })
      local cli, list_calls = fake_cli()
      local picker, lsp = fake_picker(), fake_lsp()
      ready_commands({ discovery = discovery, cli = cli, picker = picker, lsp = lsp })

      vim.cmd("MsvclspGenerate")

      MiniTest.expect.equality(#list_calls, 0)
      MiniTest.expect.equality(#notify_calls, 1)
      MiniTest.expect.equality(notify_calls[1].level, vim.log.levels.WARN)
    end)

    it("no active configuration notifies to run :MsvclspSelectConfig, never generates", function()
      local discovery = fake_discovery({ solution = "app.sln" })
      local cli, list_calls, generate_calls =
        fake_cli({ list_configs = { result = { active = nil } } })
      local picker, lsp = fake_picker(), fake_lsp()
      ready_commands({ discovery = discovery, cli = cli, picker = picker, lsp = lsp })

      vim.cmd("MsvclspGenerate")

      MiniTest.expect.equality(#list_calls, 1)
      MiniTest.expect.equality(#generate_calls, 0)
      MiniTest.expect.equality(#notify_calls, 1)
      MiniTest.expect.equality(notify_calls[1].level, vim.log.levels.WARN)
    end)

    it("a failed list_configs notifies and never generates", function()
      local discovery = fake_discovery({ solution = "app.sln" })
      local cli, _, generate_calls =
        fake_cli({ list_configs = { err = { message = "boom", kind = nil, code = 1 } } })
      local picker, lsp = fake_picker(), fake_lsp()
      ready_commands({ discovery = discovery, cli = cli, picker = picker, lsp = lsp })

      vim.cmd("MsvclspGenerate")

      MiniTest.expect.equality(#generate_calls, 0)
      MiniTest.expect.equality(#notify_calls, 1)
      MiniTest.expect.equality(notify_calls[1].level, vim.log.levels.ERROR)
    end)

    -- V2-04: a structured error with a `kind` now routes through notify.notify_error, which
    -- appends a kind-specific remedy -- distinct from the bare `err.message` the command
    -- handlers used to notify verbatim. invalid-configuration is the spot-check because
    -- list_configs is the easiest failure to trigger a specific kind for in this fake.
    it("a failed list_configs with a kind notifies more than the bare message", function()
      local discovery = fake_discovery({ solution = "app.sln" })
      local cli = fake_cli({
        list_configs = {
          err = { message = "Debug|arm64 is not a configuration", kind = "invalid-configuration", code = 7 },
        },
      })
      local picker, lsp = fake_picker(), fake_lsp()
      ready_commands({ discovery = discovery, cli = cli, picker = picker, lsp = lsp })

      vim.cmd("MsvclspGenerate")

      MiniTest.expect.equality(#notify_calls, 1)
      local msg = notify_calls[1].msg
      MiniTest.expect.equality(msg:find("Debug|arm64 is not a configuration", 1, true) ~= nil, true)
      MiniTest.expect.no_equality(msg, "msvclsp: Debug|arm64 is not a configuration")
    end)

    it("warnings from a successful list_configs produce exactly one notification", function()
      local discovery = fake_discovery({ solution = "app.sln" })
      local cli = fake_cli({
        list_configs = {
          result = { active = { configuration = "Debug", platform = "x64" } },
          warnings = { { severity = "warning", message = "unsupported construct" } },
        },
        generate = { result = { database = "compile_commands.json" } },
      })
      local picker, lsp = fake_picker(), fake_lsp()
      ready_commands({ discovery = discovery, cli = cli, picker = picker, lsp = lsp })

      vim.cmd("MsvclspGenerate")

      -- Exactly one notify: the warning batch. No error, so nothing else fires.
      local warning_calls = vim.tbl_filter(function(c)
        return c.msg:find("unsupported construct", 1, true) ~= nil
      end, notify_calls)
      MiniTest.expect.equality(#warning_calls, 1)
      MiniTest.expect.equality(warning_calls[1].level, vim.log.levels.WARN)
    end)

    it("generates using the active configuration/platform and root", function()
      local active = { configuration = "Release", platform = "x64" }
      local discovery = fake_discovery({ root = "/root", solution = "app.sln" })
      local database = "/root/.msvclsp/App-abcd1234/compile_commands.json"
      local cli, _, generate_calls = fake_cli({
        list_configs = { result = { active = active } },
        generate = { result = { database = database } },
      })
      local picker, lsp = fake_picker(), fake_lsp()
      ready_commands({ discovery = discovery, cli = cli, picker = picker, lsp = lsp })

      vim.cmd("MsvclspGenerate")

      MiniTest.expect.equality(#generate_calls, 1)
      MiniTest.expect.equality(generate_calls[1].solution, "app.sln")
      MiniTest.expect.equality(generate_calls[1].cfg, active)
      MiniTest.expect.equality(generate_calls[1].root, "/root")
    end)

    it("with auto_restart_clangd enabled restarts clangd with the returned database", function()
      config.setup({ auto_restart_clangd = true })
      local discovery = fake_discovery({ root = "/root", solution = "app.sln" })
      local cli = (fake_cli({
        list_configs = { result = { active = { configuration = "Debug", platform = "x64" } } },
        generate = { result = { database = "/root/.msvclsp/App-abcd1234/compile_commands.json" } },
      }))
      local lsp, restart_calls = fake_lsp()
      ready_commands({ discovery = discovery, cli = cli, picker = (fake_picker()), lsp = lsp })

      vim.cmd("MsvclspGenerate")

      local database = "/root/.msvclsp/App-abcd1234/compile_commands.json"
      MiniTest.expect.equality(#restart_calls, 1)
      MiniTest.expect.equality(restart_calls[1].database, database)
      MiniTest.expect.equality(restart_calls[1].root, "/root")
    end)

    it("with auto_restart_clangd disabled never restarts clangd", function()
      config.setup({ auto_restart_clangd = false })
      local discovery = fake_discovery({ root = "/root", solution = "app.sln" })
      local cli = (fake_cli({
        list_configs = { result = { active = { configuration = "Debug", platform = "x64" } } },
      }))
      local lsp, restart_calls = fake_lsp()
      ready_commands({ discovery = discovery, cli = cli, picker = (fake_picker()), lsp = lsp })

      vim.cmd("MsvclspGenerate")

      MiniTest.expect.equality(#restart_calls, 0)
    end)

    it("a bang forces regeneration; no bang does not", function()
      local discovery = fake_discovery({ root = "/root", solution = "app.sln" })
      local cli, _, generate_calls = fake_cli({
        list_configs = { result = { active = { configuration = "Debug", platform = "x64" } } },
      })
      local picker, lsp = fake_picker(), fake_lsp()
      ready_commands({ discovery = discovery, cli = cli, picker = picker, lsp = lsp })

      vim.cmd("MsvclspGenerate")
      vim.cmd("MsvclspGenerate!")

      MiniTest.expect.equality(#generate_calls, 2)
      MiniTest.expect.equality(generate_calls[1].force, false)
      MiniTest.expect.equality(generate_calls[2].force, true)
    end)

    it("a failed generate notifies and never restarts clangd", function()
      local discovery = fake_discovery({ root = "/root", solution = "app.sln" })
      local cli = (fake_cli({
        list_configs = { result = { active = { configuration = "Debug", platform = "x64" } } },
        generate = { err = { message = "generate failed", kind = nil, code = 1 } },
      }))
      local lsp, restart_calls = fake_lsp()
      ready_commands({ discovery = discovery, cli = cli, picker = (fake_picker()), lsp = lsp })

      vim.cmd("MsvclspGenerate")

      MiniTest.expect.equality(#restart_calls, 0)
      MiniTest.expect.equality(#notify_calls, 1)
      MiniTest.expect.equality(notify_calls[1].level, vim.log.levels.ERROR)
    end)

    -- Vanished active solution -- V2-01. Resolves project_spec.md's "what happens when the
    -- remembered/active solution's .sln no longer exists" Open Question: rescan, then
    -- reprompt via the picker, rather than silently falling back to a different solution or
    -- failing on a raw CLI file-not-found error. See ADR 48.

    it("a vanished active solution rescans, reprompts via the picker, and generates for the newly picked one", function()
      local discovery, rescan_calls =
        fake_discovery({ root = "/root", solution = nil, vanished = "/root/Old.sln" })
      local active = { configuration = "Release", platform = "x64" }
      local cli, list_calls, generate_calls = fake_cli({
        list_configs = { result = { active = active } },
      })
      local picker, _, pick_solution_calls = fake_picker({ solution = "/root/New.sln" })
      local lsp = fake_lsp()
      ready_commands({ discovery = discovery, cli = cli, picker = picker, lsp = lsp })

      vim.cmd("MsvclspGenerate")

      MiniTest.expect.equality(#rescan_calls, 1)
      MiniTest.expect.equality(#pick_solution_calls, 1)
      MiniTest.expect.equality(#generate_calls, 1)
      MiniTest.expect.equality(list_calls[1].solution, "/root/New.sln")
      MiniTest.expect.equality(generate_calls[1].solution, "/root/New.sln")
      MiniTest.expect.equality(notify_calls[1].level, vim.log.levels.WARN)
    end)

    it("cancelling the reprompt after a vanished solution never generates", function()
      local discovery, rescan_calls =
        fake_discovery({ root = "/root", solution = nil, vanished = "/root/Old.sln" })
      local cli, _, generate_calls = fake_cli()
      local picker, _, pick_solution_calls = fake_picker({ solution = nil })
      local lsp = fake_lsp()
      ready_commands({ discovery = discovery, cli = cli, picker = picker, lsp = lsp })

      vim.cmd("MsvclspGenerate")

      MiniTest.expect.equality(#rescan_calls, 1)
      MiniTest.expect.equality(#pick_solution_calls, 1)
      MiniTest.expect.equality(#generate_calls, 0)
    end)

    -- Config-based solution pinning -- V2-02. A missing pinned solution is a hard error, not
    -- the vanished-solution reprompt flow above: a pin exists so CI/scripting callers never see
    -- a picker, so silently falling into rescan-and-reprompt would defeat the point. See ADR 49.

    it("a pin_error short-circuits generate with a notify and no rescan or reprompt", function()
      local discovery, rescan_calls = fake_discovery({
        root = "/root",
        solution = nil,
        vanished = nil,
        pin_error = "/root/Ghost.sln",
      })
      local cli, list_calls, generate_calls = fake_cli()
      local picker, _, pick_solution_calls = fake_picker()
      local lsp = fake_lsp()
      ready_commands({ discovery = discovery, cli = cli, picker = picker, lsp = lsp })

      vim.cmd("MsvclspGenerate")

      MiniTest.expect.equality(#rescan_calls, 0)
      MiniTest.expect.equality(#pick_solution_calls, 0)
      MiniTest.expect.equality(#list_calls, 0)
      MiniTest.expect.equality(#generate_calls, 0)
      MiniTest.expect.equality(#notify_calls, 1)
      MiniTest.expect.equality(notify_calls[1].level, vim.log.levels.ERROR)
    end)
  end)

  describe("MsvclspSelectConfig", function()
    it("with no active solution notifies and never calls pick_config", function()
      local discovery = fake_discovery({ solution = nil })
      local picker, pick_calls = fake_picker()
      local cli, lsp = fake_cli(), fake_lsp()
      ready_commands({ discovery = discovery, cli = cli, picker = picker, lsp = lsp })

      vim.cmd("MsvclspSelectConfig")

      MiniTest.expect.equality(#pick_calls, 0)
      MiniTest.expect.equality(#notify_calls, 1)
    end)

    it("delegates to picker.pick_config with the resolved solution and root", function()
      local discovery = fake_discovery({ root = "/root", solution = "app.sln" })
      local picker, pick_calls = fake_picker({ config = nil, database = nil })
      local cli, lsp = fake_cli(), fake_lsp()
      ready_commands({ discovery = discovery, cli = cli, picker = picker, lsp = lsp })

      vim.cmd("MsvclspSelectConfig")

      MiniTest.expect.equality(#pick_calls, 1)
      MiniTest.expect.equality(pick_calls[1].solution, "app.sln")
      MiniTest.expect.equality(pick_calls[1].root, "/root")
    end)

    it("a picked config with auto_restart_clangd enabled restarts with the database", function()
      config.setup({ auto_restart_clangd = true })
      local discovery = fake_discovery({ root = "/root", solution = "app.sln" })
      local database = "/root/.msvclsp/App-abcd1234/compile_commands.json"
      local picker = (fake_picker({
        config = { configuration = "Debug", platform = "x64" },
        database = database,
      }))
      local lsp, restart_calls = fake_lsp()
      ready_commands({ discovery = discovery, cli = (fake_cli()), picker = picker, lsp = lsp })

      vim.cmd("MsvclspSelectConfig")

      MiniTest.expect.equality(#restart_calls, 1)
      MiniTest.expect.equality(restart_calls[1].database, database)
      MiniTest.expect.equality(restart_calls[1].root, "/root")
    end)

    it("cancelling (on_choice(nil, nil)) never restarts clangd", function()
      config.setup({ auto_restart_clangd = true })
      local discovery = fake_discovery({ root = "/root", solution = "app.sln" })
      local picker = (fake_picker({ config = nil, database = nil }))
      local lsp, restart_calls = fake_lsp()
      ready_commands({ discovery = discovery, cli = (fake_cli()), picker = picker, lsp = lsp })

      vim.cmd("MsvclspSelectConfig")

      MiniTest.expect.equality(#restart_calls, 0)
    end)

    -- Vanished active solution -- V2-01. Same recovery flow as MsvclspGenerate's own vanished
    -- cases above, but this command's continuation is picker.pick_config rather than
    -- generate_and_restart. See ADR 48.

    it("a vanished active solution rescans and reprompts before opening the config picker", function()
      local discovery, rescan_calls =
        fake_discovery({ root = "/root", solution = nil, vanished = "/root/Old.sln" })
      local cli = fake_cli()
      local picker, pick_config_calls, pick_solution_calls =
        fake_picker({ solution = "/root/New.sln", config = nil, database = nil })
      local lsp = fake_lsp()
      ready_commands({ discovery = discovery, cli = cli, picker = picker, lsp = lsp })

      vim.cmd("MsvclspSelectConfig")

      MiniTest.expect.equality(#rescan_calls, 1)
      MiniTest.expect.equality(#pick_solution_calls, 1)
      MiniTest.expect.equality(#pick_config_calls, 1)
      MiniTest.expect.equality(pick_config_calls[1].solution, "/root/New.sln")
    end)

    it("cancelling the reprompt after a vanished solution never opens the config picker", function()
      local discovery = fake_discovery({ root = "/root", solution = nil, vanished = "/root/Old.sln" })
      local cli = fake_cli()
      local picker, pick_config_calls = fake_picker({ solution = nil })
      local lsp = fake_lsp()
      ready_commands({ discovery = discovery, cli = cli, picker = picker, lsp = lsp })

      vim.cmd("MsvclspSelectConfig")

      MiniTest.expect.equality(#pick_config_calls, 0)
    end)
  end)

  describe("MsvclspRescan", function()
    it("rescans the resolved root and notifies the count found", function()
      local discovery, rescan_calls =
        fake_discovery({ root = "/root", rescan_result = { "a.sln", "b.sln" } })
      ready_commands({
        discovery = discovery,
        cli = (fake_cli()),
        picker = (fake_picker()),
        lsp = (fake_lsp()),
      })

      vim.cmd("MsvclspRescan")

      MiniTest.expect.equality(rescan_calls, { "/root" })
      MiniTest.expect.equality(#notify_calls, 1)
      MiniTest.expect.equality(notify_calls[1].level, vim.log.levels.INFO)
      MiniTest.expect.equality(notify_calls[1].msg:find("2", 1, true) ~= nil, true)
    end)
  end)

  describe("MsvclspRestart", function()
    it("restarts clangd using the database from a prior successful generate", function()
      config.setup({ auto_restart_clangd = false })
      local discovery = fake_discovery({ root = "/root", solution = "app.sln" })
      local database = "/root/.msvclsp/App-abcd1234/compile_commands.json"
      local cli = (fake_cli({
        list_configs = { result = { active = { configuration = "Debug", platform = "x64" } } },
        generate = { result = { database = database } },
      }))
      local lsp, restart_calls = fake_lsp()
      ready_commands({ discovery = discovery, cli = cli, picker = (fake_picker()), lsp = lsp })

      -- auto_restart_clangd is false, so this populates the cache without itself restarting.
      vim.cmd("MsvclspGenerate")
      MiniTest.expect.equality(#restart_calls, 0)

      vim.cmd("MsvclspRestart")

      MiniTest.expect.equality(#restart_calls, 1)
      MiniTest.expect.equality(restart_calls[1].database, database)
      MiniTest.expect.equality(restart_calls[1].root, "/root")
    end)

    it("with nothing cached calls lsp.restart(nil, root), relying on its own warning", function()
      local discovery = fake_discovery({ root = "/root" })
      local lsp, restart_calls = fake_lsp()
      ready_commands({
        discovery = discovery,
        cli = (fake_cli()),
        picker = (fake_picker()),
        lsp = lsp,
      })

      vim.cmd("MsvclspRestart")

      MiniTest.expect.equality(restart_calls, { { database = nil, root = "/root" } })
      -- commands.lua doesn't notify a second time -- lsp.restart is stubbed here, so the real
      -- warning (asserted in test_lsp.lua) doesn't fire, but commands.lua must not duplicate it.
      MiniTest.expect.equality(#notify_calls, 0)
    end)
  end)

  describe("MsvclspSelectSolution", function()
    it("delegates to picker.pick_solution with the resolved root", function()
      local discovery = fake_discovery({ root = "/root" })
      local picker, _, pick_solution_calls = fake_picker({ solution = nil })
      local cli, lsp = fake_cli(), fake_lsp()
      ready_commands({ discovery = discovery, cli = cli, picker = picker, lsp = lsp })

      vim.cmd("MsvclspSelectSolution")

      MiniTest.expect.equality(pick_solution_calls, { { root = "/root" } })
    end)

    it("restores the new solution's own active configuration, not the previous solution's", function()
      -- V1-05's own required case: switching from app.sln (active: Debug|x64) to
      -- other.sln (active: Release|arm64) must generate against other.sln's *own* answer,
      -- never a value carried over from whichever solution was active before the switch.
      local discovery = fake_discovery({ root = "/root", solution = "app.sln" })
      local old_active = { configuration = "Debug", platform = "x64" }
      local new_active = { configuration = "Release", platform = "arm64" }
      local cli, list_calls, generate_calls = fake_cli({
        list_configs_by_solution = {
          ["app.sln"] = { result = { active = old_active } },
          ["/root/Other.sln"] = { result = { active = new_active } },
        },
        generate = { result = { database = "/root/.msvclsp/Other-abcd1234/compile_commands.json" } },
      })
      local picker = fake_picker({ solution = "/root/Other.sln" })
      local lsp = fake_lsp()
      ready_commands({ discovery = discovery, cli = cli, picker = picker, lsp = lsp })

      vim.cmd("MsvclspSelectSolution")

      MiniTest.expect.equality(list_calls[#list_calls].solution, "/root/Other.sln")
      MiniTest.expect.equality(generate_calls[#generate_calls].solution, "/root/Other.sln")
      MiniTest.expect.equality(generate_calls[#generate_calls].cfg, new_active)
    end)

    it("a successful switch regenerates unforced and restarts when enabled", function()
      config.setup({ auto_restart_clangd = true })
      local discovery = fake_discovery({ root = "/root", solution = "app.sln" })
      local database = "/root/.msvclsp/Other-abcd1234/compile_commands.json"
      local active = { configuration = "Release", platform = "arm64" }
      local cli, list_calls, generate_calls = fake_cli({
        list_configs_by_solution = { ["/root/Other.sln"] = { result = { active = active } } },
        generate = { result = { database = database } },
      })
      local picker = fake_picker({ solution = "/root/Other.sln" })
      local lsp, restart_calls = fake_lsp()
      ready_commands({ discovery = discovery, cli = cli, picker = picker, lsp = lsp })

      vim.cmd("MsvclspSelectSolution")

      MiniTest.expect.equality(list_calls[#list_calls], { solution = "/root/Other.sln", root = "/root" })
      MiniTest.expect.equality(generate_calls[#generate_calls].solution, "/root/Other.sln")
      MiniTest.expect.equality(generate_calls[#generate_calls].cfg, active)
      MiniTest.expect.equality(generate_calls[#generate_calls].root, "/root")
      -- Unforced: unlike pick_config's configuration switch, a solution switch changes which
      -- files are even in play, so the CLI's own mtime staleness check is meaningful here.
      MiniTest.expect.equality(generate_calls[#generate_calls].force, false)
      MiniTest.expect.equality(#restart_calls, 1)
      MiniTest.expect.equality(restart_calls[1].database, database)
      MiniTest.expect.equality(restart_calls[1].root, "/root")
    end)

    it("with auto_restart_clangd disabled, regenerates but never restarts", function()
      config.setup({ auto_restart_clangd = false })
      local discovery = fake_discovery({ root = "/root", solution = "app.sln" })
      local active = { configuration = "Release", platform = "arm64" }
      local cli, _, generate_calls = fake_cli({
        list_configs_by_solution = { ["/root/Other.sln"] = { result = { active = active } } },
      })
      local picker = fake_picker({ solution = "/root/Other.sln" })
      local lsp, restart_calls = fake_lsp()
      ready_commands({ discovery = discovery, cli = cli, picker = picker, lsp = lsp })

      vim.cmd("MsvclspSelectSolution")

      MiniTest.expect.equality(#generate_calls, 1)
      MiniTest.expect.equality(#restart_calls, 0)
    end)

    it("no active configuration for the new solution notifies and never generates", function()
      local discovery = fake_discovery({ root = "/root", solution = "app.sln" })
      local cli, _, generate_calls = fake_cli({
        list_configs_by_solution = { ["/root/Other.sln"] = { result = { active = nil } } },
      })
      local picker = fake_picker({ solution = "/root/Other.sln" })
      local lsp, restart_calls = fake_lsp()
      ready_commands({ discovery = discovery, cli = cli, picker = picker, lsp = lsp })

      vim.cmd("MsvclspSelectSolution")

      MiniTest.expect.equality(#generate_calls, 0)
      MiniTest.expect.equality(#restart_calls, 0)
      MiniTest.expect.equality(#notify_calls, 1)
      MiniTest.expect.equality(notify_calls[1].level, vim.log.levels.WARN)
    end)

    it("a failed list_configs after switching notifies and never generates", function()
      local discovery = fake_discovery({ root = "/root", solution = "app.sln" })
      local cli, _, generate_calls = fake_cli({
        list_configs_by_solution = {
          ["/root/Other.sln"] = { err = { message = "boom", kind = nil, code = 1 } },
        },
      })
      local picker = fake_picker({ solution = "/root/Other.sln" })
      local lsp, restart_calls = fake_lsp()
      ready_commands({ discovery = discovery, cli = cli, picker = picker, lsp = lsp })

      vim.cmd("MsvclspSelectSolution")

      MiniTest.expect.equality(#generate_calls, 0)
      MiniTest.expect.equality(#restart_calls, 0)
      MiniTest.expect.equality(#notify_calls, 1)
      MiniTest.expect.equality(notify_calls[1].level, vim.log.levels.ERROR)
    end)

    it("a failed generate after switching notifies and never restarts", function()
      config.setup({ auto_restart_clangd = true })
      local discovery = fake_discovery({ root = "/root", solution = "app.sln" })
      local active = { configuration = "Release", platform = "arm64" }
      local cli = fake_cli({
        list_configs_by_solution = { ["/root/Other.sln"] = { result = { active = active } } },
        generate = { err = { message = "generate failed", kind = nil, code = 1 } },
      })
      local picker = fake_picker({ solution = "/root/Other.sln" })
      local lsp, restart_calls = fake_lsp()
      ready_commands({ discovery = discovery, cli = cli, picker = picker, lsp = lsp })

      vim.cmd("MsvclspSelectSolution")

      MiniTest.expect.equality(#restart_calls, 0)
      MiniTest.expect.equality(#notify_calls, 1)
      MiniTest.expect.equality(notify_calls[1].level, vim.log.levels.ERROR)
    end)

    it("cancelling (on_choice(nil)) never calls list_configs or generate, leaves a cached database untouched", function()
      config.setup({ auto_restart_clangd = false })
      local discovery = fake_discovery({ root = "/root", solution = "app.sln" })
      local database = "/root/.msvclsp/App-abcd1234/compile_commands.json"
      local cli, list_calls, generate_calls = fake_cli({
        list_configs = { result = { active = { configuration = "Debug", platform = "x64" } } },
        generate = { result = { database = database } },
      })
      local picker = fake_picker({ solution = nil })
      local lsp, restart_calls = fake_lsp()
      ready_commands({ discovery = discovery, cli = cli, picker = picker, lsp = lsp })

      -- Populate the per-root database cache the way a prior MsvclspGenerate would.
      vim.cmd("MsvclspGenerate")
      local calls_after_generate = #list_calls

      vim.cmd("MsvclspSelectSolution")
      vim.cmd("MsvclspRestart")

      -- Cancelling never touches list_configs/generate again, and the cached database from
      -- the earlier MsvclspGenerate survives to serve MsvclspRestart.
      MiniTest.expect.equality(#list_calls, calls_after_generate)
      MiniTest.expect.equality(#generate_calls, 1)
      MiniTest.expect.equality(restart_calls[#restart_calls], { database = database, root = "/root" })
    end)
  end)
end)
