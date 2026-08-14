-- health.lua stubs msvclsp.discovery and msvclsp.cli wholesale (package.loaded swap, the
-- commands.lua/test_commands.lua pattern per CLAUDE.md §4), plus its own three seams
-- (_executable, _system, _stat) the way cli.lua stubs _system for its own tests (ADR 34) --
-- health.lua's tests are the ones that have to intercept the real side effects it owns.
-- vim.fs.root and vim.health are Neovim/stdlib surfaces, stubbed directly for the duration
-- of each test that needs them, mirroring test_lsp.lua's approach to vim.lsp/vim.api.

--- Builds a fake `msvclsp.discovery`.
--- @param opts { root: string|nil, solution: string|nil }|nil
--- @return table fake_discovery
local function fake_discovery(opts)
  opts = opts or {}
  return {
    project_root = function()
      return opts.root or "/root"
    end,
    active_solution = function()
      return opts.solution
    end,
  }
end

--- Builds a fake `msvclsp.cli`. `list_configs_result` is a `{ result, err }` pair handed
--- straight to `on_done`, synchronously -- health.lua's own `vim.wait` loop tolerates a
--- synchronous callback exactly as well as an async one, since it only polls for either
--- value having been set.
--- @param opts { list_configs: { result: table|nil, err: table|nil }|nil }|nil
--- @return table fake_cli
--- @return table[] list_configs_calls
local function fake_cli(opts)
  opts = opts or {}
  local calls = {}
  local fake = {
    list_configs = function(solution, root, on_done)
      table.insert(calls, { solution = solution, root = root })
      local lc = opts.list_configs or { result = { active = nil, database = "C:/proj/.msvclsp/App-abcd1234/compile_commands.json" } }
      on_done(lc.result, lc.err)
    end,
  }
  return fake, calls
end

--- Fake `vim.fn.executable`: `1` for every name in `present`, `0` otherwise.
--- @param present string[]
--- @return fun(name: string): integer
local function fake_executable(present)
  local set = {}
  for _, name in ipairs(present) do
    set[name] = true
  end
  return function(name)
    return set[name] and 1 or 0
  end
end

--- Fake `vim.system` for `bin --version`: returns an object whose `:wait()` hands back
--- `result` synchronously, mirroring the shape `vim.system(...):wait()` returns for real.
--- @param result { code: integer, stdout: string, stderr: string }
--- @param calls table[] accumulator this test can inspect afterwards
--- @return fun(cmd: string[], opts: table): { wait: fun(self: table, timeout: integer): table }
local function fake_system(result, calls)
  return function(cmd, opts)
    table.insert(calls, { cmd = cmd, opts = opts })
    return {
      wait = function()
        return result
      end,
    }
  end
end

--- Fake `(vim.uv or vim.loop).fs_stat`: non-nil (truthy) for every path in `present`.
--- @param present string[]
--- @return fun(path: string): table|nil
local function fake_stat(present)
  local set = {}
  for _, path in ipairs(present) do
    set[path] = true
  end
  return function(path)
    if set[path] then
      return { type = "file" }
    end
    return nil
  end
end

--- Replaces `vim.health`'s reporting functions for one test, collecting every call as
--- `{ level = "start"|"ok"|"warn"|"error"|"info", msg = string, opts = table|nil }` instead of
--- writing to a real health report buffer.
--- @return table[] calls
--- @return fun() restore
local function stub_health()
  local calls = {}
  local original = {
    start = vim.health.start,
    ok = vim.health.ok,
    warn = vim.health.warn,
    error = vim.health.error,
    info = vim.health.info,
  }
  for level in pairs(original) do
    vim.health[level] = function(msg, opts)
      table.insert(calls, { level = level, msg = msg, opts = opts })
    end
  end
  return calls, function()
    for level, fn in pairs(original) do
      vim.health[level] = fn
    end
  end
end

--- Loads health.lua fresh against the given fakes.
--- @param fakes { discovery: table, cli: table }
--- @return table health
local function load_health(fakes)
  package.loaded["msvclsp.health"] = nil
  package.loaded["msvclsp.discovery"] = fakes.discovery
  package.loaded["msvclsp.cli"] = fakes.cli
  return require("msvclsp.health")
end

--- @param calls table[] as captured by stub_health
--- @param level string
--- @return integer
local function count(calls, level)
  local n = 0
  for _, call in ipairs(calls) do
    if call.level == level then
      n = n + 1
    end
  end
  return n
end

--- Whether any call at `level` carries `substring` in its advice (`vim.health.warn`/`.error`'s
--- second argument, a string list) -- the actionable remedy, distinct from the headline `msg`.
--- @param calls table[] as captured by stub_health
--- @param level string
--- @param substring string
--- @return boolean
local function has_remedy(calls, level, substring)
  for _, call in ipairs(calls) do
    if call.level == level and call.opts then
      for _, line in ipairs(call.opts) do
        if line:find(substring, 1, true) then
          return true
        end
      end
    end
  end
  return false
end

describe("msvclsp.health", function()
  local health_calls, restore_health
  local original_fs_root
  local config

  before_each(function()
    package.loaded["msvclsp.config"] = nil
    config = require("msvclsp.config")
    config.setup({})
    health_calls, restore_health = stub_health()
    original_fs_root = vim.fs.root
  end)

  after_each(function()
    restore_health()
    vim.fs.root = original_fs_root
  end)

  it("all healthy: every check reports ok, including the database on disk", function()
    local database = "/root/.msvclsp/App-abcd1234/compile_commands.json"
    local health = load_health({
      discovery = fake_discovery({ root = "/root", solution = "/root/app.sln" }),
      cli = (fake_cli({
        list_configs = {
          result = {
            active = { configuration = "Debug", platform = "x64" },
            database = database,
          },
        },
      })),
    })
    health._executable = fake_executable({ "msvclsp", "clangd" })
    health._system = fake_system({ code = 0, stdout = "msvclsp 0.1.0\n", stderr = "" }, {})
    health._stat = fake_stat({ database })
    vim.fs.root = function()
      return "/root"
    end

    health.check()

    MiniTest.expect.equality(count(health_calls, "error"), 0)
    MiniTest.expect.equality(count(health_calls, "warn"), 0)
    -- binary version, clangd, root, solution, database: five ok reports.
    MiniTest.expect.equality(count(health_calls, "ok"), 5)
  end)

  it("missing binary: one error, and version/database checks never run", function()
    local system_calls = {}
    local list_configs_calls
    local cli
    cli, list_configs_calls = fake_cli()
    local health = load_health({
      discovery = fake_discovery({ root = "/root", solution = "/root/app.sln" }),
      cli = cli,
    })
    health._executable = fake_executable({ "clangd" }) -- msvclsp itself is absent
    health._system = fake_system({ code = 0, stdout = "msvclsp 0.1.0\n", stderr = "" }, system_calls)
    vim.fs.root = function()
      return "/root"
    end

    health.check()

    MiniTest.expect.equality(count(health_calls, "error"), 1)
    MiniTest.expect.equality(#system_calls, 0)
    MiniTest.expect.equality(#list_configs_calls, 0)
  end)

  it("missing clangd: an error naming clangd, every other check still runs", function()
    local health = load_health({
      discovery = fake_discovery({ root = "/root", solution = "/root/app.sln" }),
      cli = (fake_cli({
        list_configs = {
          result = {
            active = { configuration = "Debug", platform = "x64" },
            database = "/root/.msvclsp/App-abcd1234/compile_commands.json",
          },
        },
      })),
    })
    health._executable = fake_executable({ "msvclsp" }) -- clangd itself is absent
    health._system = fake_system({ code = 0, stdout = "msvclsp 0.1.0\n", stderr = "" }, {})
    health._stat = fake_stat({ "/root/.msvclsp/App-abcd1234/compile_commands.json" })
    vim.fs.root = function()
      return "/root"
    end

    health.check()

    local clangd_errors = 0
    for _, call in ipairs(health_calls) do
      if call.level == "error" and call.msg:find("clangd", 1, true) then
        clangd_errors = clangd_errors + 1
      end
    end
    MiniTest.expect.equality(clangd_errors, 1)
    -- binary version, root, solution, database still ok despite clangd failing.
    MiniTest.expect.equality(count(health_calls, "ok"), 4)
  end)

  it("no solution found: a warning naming :MsvclspRescan, list_configs never called", function()
    local list_configs_calls
    local cli
    cli, list_configs_calls = fake_cli()
    local health = load_health({
      discovery = fake_discovery({ root = "/root", solution = nil }),
      cli = cli,
    })
    health._executable = fake_executable({ "msvclsp", "clangd" })
    health._system = fake_system({ code = 0, stdout = "msvclsp 0.1.0\n", stderr = "" }, {})
    vim.fs.root = function()
      return "/root"
    end

    health.check()

    -- health.lua must never call list_configs with no solution to hand it.
    MiniTest.expect.equality(#list_configs_calls, 0)

    MiniTest.expect.equality(has_remedy(health_calls, "warn", "MsvclspRescan"), true)
  end)

  it("no database generated: an active config but no file on disk warns with a remedy", function()
    local database = "/root/.msvclsp/App-abcd1234/compile_commands.json"
    local health = load_health({
      discovery = fake_discovery({ root = "/root", solution = "/root/app.sln" }),
      cli = (fake_cli({
        list_configs = {
          result = {
            active = { configuration = "Debug", platform = "x64" },
            database = database,
          },
        },
      })),
    })
    health._executable = fake_executable({ "msvclsp", "clangd" })
    health._system = fake_system({ code = 0, stdout = "msvclsp 0.1.0\n", stderr = "" }, {})
    health._stat = fake_stat({}) -- nothing exists on disk
    vim.fs.root = function()
      return "/root"
    end

    health.check()

    MiniTest.expect.equality(count(health_calls, "error"), 0)
    MiniTest.expect.equality(has_remedy(health_calls, "warn", "MsvclspGenerate"), true)
  end)

  it("no configuration selected: warns naming :MsvclspSelectConfig, never stats a path", function()
    local stat_calls = {}
    local health = load_health({
      discovery = fake_discovery({ root = "/root", solution = "/root/app.sln" }),
      cli = (fake_cli({
        list_configs = {
          result = { active = nil, database = "/root/.msvclsp/App-abcd1234/compile_commands.json" },
        },
      })),
    })
    health._executable = fake_executable({ "msvclsp", "clangd" })
    health._system = fake_system({ code = 0, stdout = "msvclsp 0.1.0\n", stderr = "" }, {})
    health._stat = function(path)
      table.insert(stat_calls, path)
      return nil
    end
    vim.fs.root = function()
      return "/root"
    end

    health.check()

    MiniTest.expect.equality(#stat_calls, 0)
    MiniTest.expect.equality(has_remedy(health_calls, "warn", "MsvclspSelectConfig"), true)
  end)

  it("root falls back to the current directory: warns even though project_root() still resolves", function()
    local health = load_health({
      discovery = fake_discovery({ root = "/fallback/cwd", solution = "/fallback/cwd/app.sln" }),
      cli = (fake_cli({
        list_configs = {
          result = {
            active = { configuration = "Debug", platform = "x64" },
            database = "/fallback/cwd/.msvclsp/App-abcd1234/compile_commands.json",
          },
        },
      })),
    })
    health._executable = fake_executable({ "msvclsp", "clangd" })
    health._system = fake_system({ code = 0, stdout = "msvclsp 0.1.0\n", stderr = "" }, {})
    health._stat = fake_stat({ "/fallback/cwd/.msvclsp/App-abcd1234/compile_commands.json" })
    -- No root marker matched anywhere -- discovery.project_root() would have fallen back to cwd.
    vim.fs.root = function()
      return nil
    end

    health.check()

    local root_warned = false
    for _, call in ipairs(health_calls) do
      if call.level == "warn" and call.msg:find("root", 1, true) then
        root_warned = true
      end
    end
    MiniTest.expect.equality(root_warned, true)
  end)
end)
