-- cli.lua is the plugin's only seam onto the real binary. Every other module stubs it
-- wholesale (package.loaded swap); these specs stub one level deeper, at `cli._system`,
-- since cli.lua's own tests are the ones that have to intercept the actual spawn.

--- Builds a fake `vim.system`-shaped function that ignores the process entirely and
--- hands `result` to `on_exit` on the next scheduled tick, mimicking `vim.system`'s own
--- off-main-loop callback. Records the args/opts it was called with in `calls`.
--- @param result table shape accepted by vim.system's on_exit: { code, stdout, stderr }
--- @param calls table[] accumulator this test can inspect afterwards
--- @return fun(cmd: string[], opts: table, on_exit: fun(obj: table))
local function fake_system(result, calls)
  return function(cmd, opts, on_exit)
    table.insert(calls, { cmd = cmd, opts = opts })
    vim.schedule(function()
      on_exit(result)
    end)
  end
end

--- A `_system` replacement that raises synchronously, the way `vim.system` does on a
--- spawn failure (e.g. the binary is not on PATH).
--- @param message string
--- @return fun(cmd: string[], opts: table, on_exit: fun(obj: table))
local function throwing_system(message)
  return function()
    error(message)
  end
end

--- Runs `fn`, then spins the event loop until `predicate()` is true or `timeout_ms`
--- elapses. Needed because the fakes above complete via `vim.schedule`, never
--- synchronously.
--- @param predicate fun(): boolean
--- @param timeout_ms integer|nil defaults to 1000
local function wait_for(predicate, timeout_ms)
  vim.wait(timeout_ms or 1000, predicate, 10)
end

local function envelope(overrides)
  local base = {
    schema_version = 1,
    has_error = false,
    result = { solution = "app.sln" },
    error = nil,
    warnings = {},
  }
  return vim.tbl_extend("force", base, overrides or {})
end

describe("msvclsp.cli", function()
  local cli

  before_each(function()
    -- Fresh module per test: `_system` is reassigned by nearly every test below, and it
    -- must not leak into the next one.
    package.loaded["msvclsp.cli"] = nil
    package.loaded["msvclsp.config"] = nil
    cli = require("msvclsp.cli")
  end)

  it("decodes a successful envelope and hands it back whole", function()
    local ok_envelope = envelope({ warnings = { { severity = "warning", message = "x" } } })
    cli._system = fake_system({ code = 0, stdout = vim.json.encode(ok_envelope), stderr = "" }, {})

    local got_result, got_err
    cli.run({ "list-configs", "app.sln" }, function(result, err)
      got_result, got_err = result, err
    end)

    wait_for(function()
      return got_result ~= nil or got_err ~= nil
    end)

    MiniTest.expect.equality(got_err, nil)
    -- The whole envelope rides through, not just `.result` -- warnings must survive for
    -- V2-04 to render later.
    MiniTest.expect.equality(got_result.result, ok_envelope.result)
    MiniTest.expect.equality(got_result.warnings, ok_envelope.warnings)
  end)

  it("always appends --json to the argv it builds", function()
    local calls = {}
    cli._system = fake_system({ code = 0, stdout = vim.json.encode(envelope()), stderr = "" }, calls)

    cli.run({ "list-configs", "app.sln" }, function() end)
    wait_for(function()
      return #calls > 0
    end)

    MiniTest.expect.equality(calls[1].cmd[#calls[1].cmd], "--json")
  end)

  it("folds stderr and the envelope's error kind into a structured error on a non-zero exit", function()
    local err_envelope = envelope({
      has_error = true,
      result = nil,
      error = { kind = "solution-not-found", message = "no such file", causes = {} },
    })
    cli._system = fake_system({
      code = 3,
      stdout = vim.json.encode(err_envelope),
      stderr = "msvclsp: no such file\n",
    }, {})

    local got_result, got_err
    cli.run({ "list-configs", "missing.sln" }, function(result, err)
      got_result, got_err = result, err
    end)

    wait_for(function()
      return got_result ~= nil or got_err ~= nil
    end)

    MiniTest.expect.equality(got_result, nil)
    MiniTest.expect.equality(got_err.kind, "solution-not-found")
    MiniTest.expect.equality(got_err.message, "msvclsp: no such file")
  end)

  it("builds a structured error from stderr alone when stdout does not decode", function()
    -- Simulates a clap usage error: exit 2, empty stdout, no envelope at all.
    cli._system = fake_system({ code = 2, stdout = "", stderr = "error: missing argument\n" }, {})

    local got_result, got_err
    cli.run({ "list-configs" }, function(result, err)
      got_result, got_err = result, err
    end)

    wait_for(function()
      return got_result ~= nil or got_err ~= nil
    end)

    MiniTest.expect.equality(got_result, nil)
    MiniTest.expect.equality(got_err.kind, nil)
    MiniTest.expect.equality(got_err.message, "error: missing argument")
    MiniTest.expect.equality(got_err.code, 2)
  end)

  it("reports a missing binary as a structured error instead of raising", function()
    cli._system = throwing_system("ENOENT: msvclsp not found")

    local got_result, got_err
    local ok = pcall(cli.run, { "list-configs", "app.sln" }, function(result, err)
      got_result, got_err = result, err
    end)

    MiniTest.expect.equality(ok, true)
    MiniTest.expect.equality(got_result, nil)
    MiniTest.expect.equality(got_err.kind, nil)
    MiniTest.expect.equality(got_err.code, nil)
  end)

  it("never calls on_done synchronously", function()
    cli._system = fake_system({ code = 0, stdout = vim.json.encode(envelope()), stderr = "" }, {})

    local called = false
    cli.run({ "list-configs", "app.sln" }, function()
      called = true
    end)

    -- The fake defers via vim.schedule, same as the real vim.system; on_done must not
    -- have fired before this line runs.
    MiniTest.expect.equality(called, false)
    wait_for(function()
      return called
    end)
    MiniTest.expect.equality(called, true)
  end)

  it("does not error on an unrecognized warning severity", function()
    -- Pins ADR 13: a decoder must not branch exhaustively on `severity`. cli.lua hands the
    -- envelope through undecoded further, so this is a smoke test that nothing along the
    -- path pattern-matches the string.
    local future_envelope = envelope({
      warnings = { { severity = "something-new", message = "future construct" } },
    })
    cli._system = fake_system({ code = 0, stdout = vim.json.encode(future_envelope), stderr = "" }, {})

    local got_result, got_err
    local ok = pcall(cli.run, { "list-configs", "app.sln" }, function(result, err)
      got_result, got_err = result, err
    end)
    wait_for(function()
      return got_result ~= nil or got_err ~= nil
    end)

    MiniTest.expect.equality(ok, true)
    MiniTest.expect.equality(got_err, nil)
    MiniTest.expect.equality(got_result.warnings[1].severity, "something-new")
  end)

  describe("list_configs", function()
    it("builds the list-configs argv including --root and unwraps the result", function()
      local calls = {}
      local payload = { solution = "C:/proj/app.sln", available = {}, active = nil }
      cli._system =
        fake_system({ code = 0, stdout = vim.json.encode(envelope({ result = payload })), stderr = "" }, calls)

      local got_result, got_err
      cli.list_configs("C:/proj/app.sln", "C:/proj", function(result, err)
        got_result, got_err = result, err
      end)
      wait_for(function()
        return got_result ~= nil or got_err ~= nil
      end)

      MiniTest.expect.equality(
        calls[1].cmd,
        { "msvclsp", "list-configs", "C:/proj/app.sln", "--root", "C:/proj", "--json" }
      )
      MiniTest.expect.equality(got_err, nil)
      MiniTest.expect.equality(got_result, payload)
    end)

    it("passes the structured error through unchanged on failure", function()
      local err_envelope = envelope({
        has_error = true,
        result = nil,
        error = { kind = "solution-parse", message = "bad xml", causes = {} },
      })
      cli._system =
        fake_system({ code = 4, stdout = vim.json.encode(err_envelope), stderr = "msvclsp: bad xml\n" }, {})

      local got_result, got_err
      cli.list_configs("app.sln", "C:/proj", function(result, err)
        got_result, got_err = result, err
      end)
      wait_for(function()
        return got_result ~= nil or got_err ~= nil
      end)

      MiniTest.expect.equality(got_result, nil)
      MiniTest.expect.equality(got_err.kind, "solution-parse")
    end)
  end)

  describe("generate", function()
    it("builds the generate argv including configuration, platform and --root", function()
      local calls = {}
      local payload = { database = "C:/proj/.msvclsp/App-abcd1234/compile_commands.json", regenerated = true }
      cli._system =
        fake_system({ code = 0, stdout = vim.json.encode(envelope({ result = payload })), stderr = "" }, calls)

      local got_result, got_err
      cli.generate(
        "C:/proj/app.sln",
        { configuration = "Debug", platform = "x64" },
        "C:/proj",
        false,
        function(result, err)
          got_result, got_err = result, err
        end
      )
      wait_for(function()
        return got_result ~= nil or got_err ~= nil
      end)

      MiniTest.expect.equality(calls[1].cmd, {
        "msvclsp",
        "generate",
        "C:/proj/app.sln",
        "--configuration",
        "Debug",
        "--platform",
        "x64",
        "--root",
        "C:/proj",
        "--json",
      })
      MiniTest.expect.equality(got_err, nil)
      MiniTest.expect.equality(got_result, payload)
    end)

    it("appends --force when asked, and nothing when not", function()
      -- MVP-46: `:MsvclspGenerate!` reaches this. `--force` bypasses the CLI's own staleness
      -- cache (MVP-21/22); the default argv above must stay exactly what it is today, which is
      -- what the sibling test pins.
      local calls = {}
      cli._system =
        fake_system({ code = 0, stdout = vim.json.encode(envelope()), stderr = "" }, calls)

      cli.generate("C:/proj/app.sln", { configuration = "Debug", platform = "x64" }, "C:/proj", true, function() end)
      wait_for(function()
        return #calls > 0
      end)

      MiniTest.expect.equality(vim.tbl_contains(calls[1].cmd, "--force"), true)
      -- Comes before --json, which M.run always appends last.
      MiniTest.expect.equality(calls[1].cmd[#calls[1].cmd - 1], "--force")
    end)
  end)

  it("decodes a JSON null field to real Lua nil, not the vim.NIL sentinel", function()
    -- vim.json.decode defaults to vim.NIL for null, since a Lua table can't hold a real nil
    -- as a value. Every other module relies on cli.lua decoding with `luanil` enabled instead,
    -- so optional fields like SolutionConfigs.active can be checked with plain `if not x then`
    -- rather than every caller special-casing the sentinel.
    local payload = { solution = "app.sln", available = {}, active = vim.NIL }
    cli._system =
      fake_system({ code = 0, stdout = vim.json.encode(envelope({ result = payload })), stderr = "" }, {})

    local got_result, got_err
    cli.run({ "list-configs", "app.sln" }, function(result, err)
      got_result, got_err = result, err
    end)
    wait_for(function()
      return got_result ~= nil or got_err ~= nil
    end)

    MiniTest.expect.equality(got_err, nil)
    MiniTest.expect.equality(got_result.result.active, nil)
    MiniTest.expect.equality(got_result.result.active ~= vim.NIL, true)
  end)

  it("no longer exposes the dead discover stub", function()
    -- Predates ADR 03's decision that scanning is Lua's job (MVP-27); ADR 33 flagged it as
    -- dead code for this issue to remove.
    MiniTest.expect.equality(cli.discover, nil)
  end)
end)
