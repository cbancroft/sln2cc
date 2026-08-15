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
    MiniTest.expect.equality(generate_calls[1].force, false)
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
