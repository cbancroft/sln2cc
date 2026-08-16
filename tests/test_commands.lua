-- commands.lua stubs discovery.lua, cli.lua, picker.lua, and lsp.lua wholesale
-- (package.loaded swap), the same seam every non-cli.lua module's tests use per CLAUDE.md §4 --
-- these specs never invoke the real binary or touch real LSP state.

--- Builds a fake `msvclsp.discovery`.
--- @param opts { root: string|nil, solution: string|nil, rescan_result: string[]|nil }
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
      return opts.solution
    end,
    rescan = function(r)
      table.insert(rescan_calls, r)
      return opts.rescan_result or {}
    end,
  }
  return fake, rescan_calls
end

--- Builds a fake `msvclsp.cli`. `list_configs_result`/`generate_result` are each
--- `{ result, err }` pairs handed straight to their respective `on_done` callbacks
--- synchronously.
--- @param opts { list_configs: table|nil, generate: table|nil }
--- @return table fake_cli
--- @return table[] list_configs_calls
--- @return table[] generate_calls each also records `force`, for MVP-46
local function fake_cli(opts)
  opts = opts or {}
  local list_configs_calls, generate_calls = {}, {}
  local fake = {
    list_configs = function(solution, root, on_done)
      table.insert(list_configs_calls, { solution = solution, root = root })
      local lc = opts.list_configs or { result = { active = nil } }
      on_done(lc.result, lc.err)
    end,
    generate = function(solution, cfg, root, force, on_done)
      table.insert(generate_calls, { solution = solution, cfg = cfg, root = root, force = force })
      local gen = opts.generate or { result = { database = "compile_commands.json" } }
      on_done(gen.result, gen.err)
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

    it("a successful pick invalidates the root's cached database", function()
      config.setup({ auto_restart_clangd = false })
      local discovery = fake_discovery({ root = "/root", solution = "app.sln" })
      local database = "/root/.msvclsp/App-abcd1234/compile_commands.json"
      local cli = fake_cli({
        list_configs = { result = { active = { configuration = "Debug", platform = "x64" } } },
        generate = { result = { database = database } },
      })
      local picker = fake_picker({ solution = "/root/Other.sln" })
      local lsp, restart_calls = fake_lsp()
      ready_commands({ discovery = discovery, cli = cli, picker = picker, lsp = lsp })

      -- Populate the per-root database cache the way a prior MsvclspGenerate would.
      -- auto_restart_clangd is off so this populates the cache without itself restarting,
      -- keeping the restart below attributable only to MsvclspRestart's own resolution logic.
      vim.cmd("MsvclspGenerate")
      MiniTest.expect.equality(#restart_calls, 0)

      vim.cmd("MsvclspSelectSolution")
      vim.cmd("MsvclspRestart")

      -- The cached database belonged to the old solution; invalidation means MsvclspRestart
      -- falls through to lsp.restart(nil, root) rather than reusing the stale path.
      MiniTest.expect.equality(restart_calls[#restart_calls], { database = nil, root = "/root" })
    end)

    it("cancelling (on_choice(nil)) leaves a cached database untouched", function()
      local discovery = fake_discovery({ root = "/root", solution = "app.sln" })
      local database = "/root/.msvclsp/App-abcd1234/compile_commands.json"
      local cli = fake_cli({
        list_configs = { result = { active = { configuration = "Debug", platform = "x64" } } },
        generate = { result = { database = database } },
      })
      local picker = fake_picker({ solution = nil })
      local lsp, restart_calls = fake_lsp()
      ready_commands({ discovery = discovery, cli = cli, picker = picker, lsp = lsp })

      vim.cmd("MsvclspGenerate")
      vim.cmd("MsvclspSelectSolution")
      vim.cmd("MsvclspRestart")

      MiniTest.expect.equality(restart_calls[#restart_calls], { database = database, root = "/root" })
    end)
  end)
end)
