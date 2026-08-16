-- picker.lua stubs cli.lua wholesale (package.loaded swap), the same seam every non-cli.lua
-- module's tests use per CLAUDE.md §4 -- these specs never invoke the real binary.

--- Builds a fake `msvclsp.cli` module. `list_configs_result`/`generate_result` are each
--- `{ result, err }` pairs handed straight to their respective `on_done` callbacks
--- synchronously, since these tests don't need to exercise cli.lua's own async behavior --
--- that's test_cli.lua's job.
--- @param opts { list_configs: table, generate: table|nil }
--- @return table fake_cli
--- @return table[] generate_calls each { solution, cfg, root, force }
local function fake_cli(opts)
  local generate_calls = {}
  local fake = {
    list_configs = function(solution, root, on_done)
      on_done(opts.list_configs.result, opts.list_configs.err)
    end,
    generate = function(solution, cfg, root, force, on_done)
      table.insert(generate_calls, { solution = solution, cfg = cfg, root = root, force = force })
      local gen = opts.generate or { result = { database = "compile_commands.json" } }
      on_done(gen.result, gen.err)
    end,
  }
  return fake, generate_calls
end

--- Builds a fake `vim.ui.select` that records its call and, unless `no_call` is set, invokes
--- its callback with `choice` (or `nil` to simulate cancellation) synchronously.
--- @param choice table|nil
--- @return fun(items: table[], opts: table, on_choice: fun(item: table|nil))
--- @return table[] calls each { items, opts }
local function fake_select(choice)
  local calls = {}
  return function(items, select_opts, on_choice)
    table.insert(calls, { items = items, opts = select_opts })
    on_choice(choice)
  end, calls
end

local function config(conf, plat)
  return { configuration = conf, platform = plat }
end

describe("msvclsp.picker.pick_config", function()
  local picker
  local original_ui_select

  before_each(function()
    package.loaded["msvclsp.picker"] = nil
    package.loaded["msvclsp.cli"] = nil
    original_ui_select = vim.ui.select
  end)

  after_each(function()
    vim.ui.select = original_ui_select
  end)

  it("passes list_configs' available options to vim.ui.select", function()
    local available = { config("Debug", "x64"), config("Release", "x64") }
    package.loaded["msvclsp.cli"] = fake_cli({
      list_configs = { result = { solution = "app.sln", available = available, active = nil } },
    })
    picker = require("msvclsp.picker")

    local select_fn, calls = fake_select(available[1])
    vim.ui.select = select_fn

    picker.pick_config("app.sln", "/root", function() end)

    MiniTest.expect.equality(calls[1].items, available)
  end)

  it("marks the active pair in the formatted item text", function()
    local active = config("Release", "x64")
    local available = { config("Debug", "x64"), active }
    package.loaded["msvclsp.cli"] = fake_cli({
      list_configs = { result = { solution = "app.sln", available = available, active = active } },
    })
    picker = require("msvclsp.picker")

    local select_fn, calls = fake_select(available[1])
    vim.ui.select = select_fn

    picker.pick_config("app.sln", "/root", function() end)

    local format_item = calls[1].opts.format_item
    MiniTest.expect.equality(format_item(active):find("(active)", 1, true) ~= nil, true)
    MiniTest.expect.equality(format_item(available[1]):find("(active)", 1, true) ~= nil, false)
  end)

  it("selecting an option calls generate with that configuration/platform and the given root", function()
    local chosen = config("Release", "x64")
    local available = { config("Debug", "x64"), chosen }
    local fake, generate_calls = fake_cli({
      list_configs = { result = { solution = "app.sln", available = available, active = nil } },
    })
    package.loaded["msvclsp.cli"] = fake
    picker = require("msvclsp.picker")

    vim.ui.select = select(1, fake_select(chosen))

    picker.pick_config("app.sln", "/root", function() end)

    MiniTest.expect.equality(#generate_calls, 1)
    MiniTest.expect.equality(generate_calls[1].solution, "app.sln")
    MiniTest.expect.equality(generate_calls[1].cfg, chosen)
    MiniTest.expect.equality(generate_calls[1].root, "/root")
    -- Forced: the CLI's staleness check is file-mtime-only and knows nothing about which
    -- configuration/platform was requested, so an unforced generate could silently reuse the
    -- previous configuration's database when switching to a new one touches no files.
    MiniTest.expect.equality(generate_calls[1].force, true)
  end)

  it("a successful generate calls on_choice with the picked config and the database", function()
    local chosen = config("Release", "x64")
    package.loaded["msvclsp.cli"] = fake_cli({
      list_configs = {
        result = { solution = "app.sln", available = { chosen }, active = nil },
      },
      generate = { result = { database = "/root/.msvclsp/App-abcd1234/compile_commands.json" } },
    })
    picker = require("msvclsp.picker")
    vim.ui.select = select(1, fake_select(chosen))

    local got, got_database
    picker.pick_config("app.sln", "/root", function(c, database)
      got, got_database = c, database
    end)

    MiniTest.expect.equality(got, chosen)
    MiniTest.expect.equality(got_database, "/root/.msvclsp/App-abcd1234/compile_commands.json")
  end)

  it("cancelling calls on_choice(nil, nil) and never calls generate", function()
    local available = { config("Debug", "x64") }
    local fake, generate_calls = fake_cli({
      list_configs = { result = { solution = "app.sln", available = available, active = nil } },
    })
    package.loaded["msvclsp.cli"] = fake
    picker = require("msvclsp.picker")
    -- vim.ui.select invokes its callback with nil, mirroring a user backing out (<Esc>).
    vim.ui.select = select(1, fake_select(nil))

    local got, got_database, called = "sentinel", "sentinel", false
    picker.pick_config("app.sln", "/root", function(c, database)
      got, got_database, called = c, database, true
    end)

    MiniTest.expect.equality(called, true)
    MiniTest.expect.equality(got, nil)
    MiniTest.expect.equality(got_database, nil)
    MiniTest.expect.equality(#generate_calls, 0)
  end)

  it("a failed list_configs notifies, calls on_choice(nil), never opens the selector", function()
    package.loaded["msvclsp.cli"] = fake_cli({
      list_configs = { err = { message = "boom", kind = nil, code = 1 } },
    })
    picker = require("msvclsp.picker")

    local select_called = false
    vim.ui.select = function()
      select_called = true
    end

    local got, got_database, called = "sentinel", "sentinel", false
    picker.pick_config("app.sln", "/root", function(c, database)
      got, got_database, called = c, database, true
    end)

    MiniTest.expect.equality(called, true)
    MiniTest.expect.equality(got, nil)
    MiniTest.expect.equality(got_database, nil)
    MiniTest.expect.equality(select_called, false)
  end)

  it("a failed generate after selection notifies and calls on_choice(nil, nil)", function()
    local chosen = config("Release", "x64")
    package.loaded["msvclsp.cli"] = fake_cli({
      list_configs = {
        result = { solution = "app.sln", available = { chosen }, active = nil },
      },
      generate = { err = { message = "generate failed", kind = nil, code = 1 } },
    })
    picker = require("msvclsp.picker")
    vim.ui.select = select(1, fake_select(chosen))

    local got, got_database, called = "sentinel", "sentinel", false
    picker.pick_config("app.sln", "/root", function(c, database)
      got, got_database, called = c, database, true
    end)

    MiniTest.expect.equality(called, true)
    MiniTest.expect.equality(got, nil)
    MiniTest.expect.equality(got_database, nil)
  end)

  it("an empty available list notifies, calls on_choice(nil), never opens the selector", function()
    package.loaded["msvclsp.cli"] = fake_cli({
      list_configs = { result = { solution = "app.sln", available = {}, active = nil } },
    })
    picker = require("msvclsp.picker")

    local select_called = false
    vim.ui.select = function()
      select_called = true
    end

    local got, got_database, called = "sentinel", "sentinel", false
    picker.pick_config("app.sln", "/root", function(c, database)
      got, got_database, called = c, database, true
    end)

    MiniTest.expect.equality(called, true)
    MiniTest.expect.equality(got, nil)
    MiniTest.expect.equality(got_database, nil)
    MiniTest.expect.equality(select_called, false)
  end)
end)

-- pick_solution -- V1-03. Unlike pick_config, this depends on msvclsp.discovery rather than
-- msvclsp.cli: choosing a solution is inherently a discovery.lua concern (ADR 33, ADR 45),
-- breaking ADR 35's "picker depends only on cli.lua" precedent deliberately.

--- Builds a fake `msvclsp.discovery`. `solutions_result` is the plain list `M.solutions`
--- returns; `active` is what `M.active_solution` returns before any pick is made.
--- @param opts { solutions: string[], active: string|nil }
--- @return table fake_discovery
--- @return table[] set_calls each { root, solution }
local function fake_discovery(opts)
  local set_calls = {}
  local fake = {
    solutions = function()
      return opts.solutions
    end,
    active_solution = function()
      return opts.active
    end,
    set_active_solution = function(root, solution)
      table.insert(set_calls, { root = root, solution = solution })
    end,
  }
  return fake, set_calls
end

describe("msvclsp.picker.pick_solution", function()
  local picker
  local original_ui_select

  before_each(function()
    package.loaded["msvclsp.picker"] = nil
    package.loaded["msvclsp.discovery"] = nil
    original_ui_select = vim.ui.select
  end)

  after_each(function()
    vim.ui.select = original_ui_select
  end)

  it("passes discovery.solutions' result to vim.ui.select", function()
    local solutions = { "/root/App.sln", "/root/Other.sln" }
    package.loaded["msvclsp.discovery"] = fake_discovery({ solutions = solutions })
    picker = require("msvclsp.picker")

    local select_fn, calls = fake_select(solutions[1])
    vim.ui.select = select_fn

    picker.pick_solution("/root", function() end)

    MiniTest.expect.equality(calls[1].items, solutions)
  end)

  it("marks the active solution in the formatted item text", function()
    local solutions = { "/root/App.sln", "/root/Other.sln" }
    package.loaded["msvclsp.discovery"] =
      fake_discovery({ solutions = solutions, active = "/root/Other.sln" })
    picker = require("msvclsp.picker")

    local select_fn, calls = fake_select(solutions[1])
    vim.ui.select = select_fn

    picker.pick_solution("/root", function() end)

    local format_item = calls[1].opts.format_item
    MiniTest.expect.equality(format_item("/root/Other.sln"):find("(active)", 1, true) ~= nil, true)
    MiniTest.expect.equality(format_item("/root/App.sln"):find("(active)", 1, true) ~= nil, false)
  end)

  it("selecting an option persists it and calls on_choice with it", function()
    local solutions = { "/root/App.sln", "/root/Other.sln" }
    local fake, set_calls = fake_discovery({ solutions = solutions })
    package.loaded["msvclsp.discovery"] = fake
    picker = require("msvclsp.picker")
    vim.ui.select = select(1, fake_select(solutions[2]))

    local got, called = "sentinel", false
    picker.pick_solution("/root", function(s)
      got, called = s, true
    end)

    MiniTest.expect.equality(called, true)
    MiniTest.expect.equality(got, solutions[2])
    MiniTest.expect.equality(set_calls, { { root = "/root", solution = solutions[2] } })
  end)

  it("cancelling calls on_choice(nil) and never persists a choice", function()
    local solutions = { "/root/App.sln" }
    local fake, set_calls = fake_discovery({ solutions = solutions })
    package.loaded["msvclsp.discovery"] = fake
    picker = require("msvclsp.picker")
    -- vim.ui.select invokes its callback with nil, mirroring a user backing out (<Esc>).
    vim.ui.select = select(1, fake_select(nil))

    local got, called = "sentinel", false
    picker.pick_solution("/root", function(s)
      got, called = s, true
    end)

    MiniTest.expect.equality(called, true)
    MiniTest.expect.equality(got, nil)
    MiniTest.expect.equality(#set_calls, 0)
  end)

  it("an empty solution list notifies, calls on_choice(nil), never opens the selector", function()
    package.loaded["msvclsp.discovery"] = fake_discovery({ solutions = {} })
    picker = require("msvclsp.picker")

    local select_called = false
    vim.ui.select = function()
      select_called = true
    end

    local got, called = "sentinel", false
    picker.pick_solution("/root", function(s)
      got, called = s, true
    end)

    MiniTest.expect.equality(called, true)
    MiniTest.expect.equality(got, nil)
    MiniTest.expect.equality(select_called, false)
  end)
end)

-- V1-06: inline first-run discovery. Every other pick_solution spec above stubs
-- msvclsp.discovery wholesale, so none of them can prove a fresh solution scan actually
-- happens as part of the pick rather than requiring a caller to discover/rescan first. This
-- block uses the real discovery.lua against a real temp tree to close that specific gap;
-- discovery.lua itself has been inline/lazy since MVP-27 (its M.solutions caches on first
-- call, not before), this just pins the picker's own reliance on that behavior down with a
-- test.

--- Builds a fresh temp directory tree and returns its root plus a cleanup function, mirroring
--- test_discovery.lua's own helper -- kept local rather than shared, per this repo's existing
--- per-file helper convention.
--- @return string root
--- @return fun() cleanup
local function make_tree()
  local root = vim.fs.normalize(vim.fn.tempname())
  vim.fn.mkdir(root, "p")
  return root, function()
    vim.fn.delete(root, "rf")
  end
end

--- Creates an empty `.sln` file at `tree .. "/" .. name`. Content is irrelevant --
--- discovery.lua matches by filename only.
--- @param tree string
--- @param name string
local function make_sln(tree, name)
  local fd = assert(io.open(tree .. "/" .. name, "w"))
  fd:close()
end

describe("msvclsp.picker.pick_solution — inline discovery", function()
  local picker
  local original_ui_select

  before_each(function()
    package.loaded["msvclsp.picker"] = nil
    -- Deliberately NOT stubbed: this block's whole point is exercising the real module.
    package.loaded["msvclsp.discovery"] = nil
    package.loaded["msvclsp.config"] = nil
    original_ui_select = vim.ui.select
  end)

  after_each(function()
    vim.ui.select = original_ui_select
  end)

  it("discovery runs inline on first pick, with no prior scan", function()
    local tree, cleanup = make_tree()
    local ok, err = pcall(function()
      make_sln(tree, "App.sln")
      make_sln(tree, "Other.sln")

      picker = require("msvclsp.picker")

      local select_fn, calls = fake_select(tree .. "/App.sln")
      vim.ui.select = select_fn

      local got, called = "sentinel", false
      -- No discovery.solutions()/rescan() call precedes this -- if the scan weren't inline,
      -- vim.ui.select would see an empty list instead of both .sln files.
      picker.pick_solution(tree, function(s)
        got, called = s, true
      end)

      MiniTest.expect.equality(calls[1].items, { tree .. "/App.sln", tree .. "/Other.sln" })
      MiniTest.expect.equality(called, true)
      MiniTest.expect.equality(got, tree .. "/App.sln")
    end)
    cleanup()
    if not ok then
      error(err, 0)
    end
  end)
end)
