-- Plugin tests stub out the CLI binary rather than requiring a real Windows toolchain,
-- so they exercise plugin logic (caching, persistence, lifecycle) only. config.lua has no
-- CLI dependency at all, so these specs exercise it directly.

--- Replaces `vim.notify` for the duration of one test, collecting every call instead of
--- printing to the test runner's own output. Callers must call `restore()` even on failure,
--- so tests wrap usage in `pcall` where the assertion itself might error.
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

describe("msvclsp.config", function()
  local config

  before_each(function()
    -- Fresh module state per test: config.lua persists M.options across setup() calls, so a
    -- previous test's state must not leak into the next one's assertions.
    package.loaded["msvclsp.config"] = nil
    config = require("msvclsp.config")
  end)

  it("resolves to exactly the defaults when setup() is called with no options", function()
    config.setup()
    MiniTest.expect.equality(config.options, config.defaults)

    config.setup(nil)
    MiniTest.expect.equality(config.options, config.defaults)
  end)

  it("overrides a scalar default, leaving siblings untouched", function()
    local result = config.setup({ bin = "custom-msvclsp" })

    MiniTest.expect.equality(result.bin, "custom-msvclsp")
    MiniTest.expect.equality(config.options.bin, "custom-msvclsp")
    MiniTest.expect.equality(config.options.data_dir, config.defaults.data_dir)
    MiniTest.expect.equality(config.options.auto_restart_clangd, config.defaults.auto_restart_clangd)
  end)

  it("replaces a table-valued default wholesale, without aliasing M.defaults", function()
    -- Deliberately shorter than the default list — vim.tbl_deep_extend would index-merge
    -- this and leave a trailing default entry behind. See ADR 27.
    local overridden_markers = { "custom" }
    local default_markers_before = vim.deepcopy(config.defaults.root_markers)

    local result = config.setup({ root_markers = overridden_markers })

    MiniTest.expect.equality(result.root_markers, { "custom" })
    MiniTest.expect.equality(config.defaults.root_markers, default_markers_before)

    -- Mutating the resolved table must not reach into M.defaults — proves no shared
    -- reference survived the merge.
    table.insert(config.options.root_markers, "mutated")
    MiniTest.expect.equality(config.defaults.root_markers, default_markers_before)
  end)

  it("rejects an unknown key, notifies once, and leaves M.options unchanged", function()
    config.setup({ bin = "known-good" })
    local before = vim.deepcopy(config.options)

    local calls, restore = stub_notify()
    local ok, result = pcall(config.setup, { typo_key = true })
    restore()
    if not ok then
      error(result, 0)
    end

    MiniTest.expect.equality(result, nil)
    MiniTest.expect.equality(#calls, 1)
    MiniTest.expect.equality(calls[1].level, vim.log.levels.ERROR)
    MiniTest.expect.equality(calls[1].msg:find("typo_key", 1, true) ~= nil, true)
    MiniTest.expect.equality(config.options, before)
  end)

  it("rejects a wrong-typed value, notifies once, and leaves M.options unchanged", function()
    config.setup({ bin = "known-good" })
    local before = vim.deepcopy(config.options)

    local calls, restore = stub_notify()
    local ok, result = pcall(config.setup, { auto_restart_clangd = "yes" })
    restore()
    if not ok then
      error(result, 0)
    end

    MiniTest.expect.equality(result, nil)
    MiniTest.expect.equality(#calls, 1)
    MiniTest.expect.equality(calls[1].level, vim.log.levels.ERROR)
    MiniTest.expect.equality(calls[1].msg:find("auto_restart_clangd", 1, true) ~= nil, true)
    MiniTest.expect.equality(config.options, before)
  end)

  it("is idempotent and does not leak state between calls", function()
    config.setup({ bin = "x" })
    config.setup({})

    -- The second call has no `bin` override, so it must resolve back to the default rather
    -- than retaining "x" from the first call.
    MiniTest.expect.equality(config.options.bin, config.defaults.bin)

    local first = config.setup({ bin = "x" })
    local second = config.setup({ bin = "x" })
    MiniTest.expect.equality(first, second)
  end)
end)
