-- notify.lua has no seams to stub -- it only calls vim.notify, so every spec here just swaps
-- that out and inspects what got sent, the same stub_notify pattern test_commands.lua and
-- test_config.lua already use.

--- Swaps vim.notify for the duration of a test, collecting every call instead of printing to
--- the test runner's own output.
--- @return table[] calls each { msg, level }
--- @return fun() restore
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

--- Every ErrorKind wire string cli/src/error.rs::ErrorKind::as_str emits today. Kept as a
--- literal list, mirroring how the Rust side pins its own wire strings (error.rs's
--- `ErrorKind::ALL` test) -- a kind added there without a row added here is exactly the drift
--- this list exists to catch.
local ALL_KINDS = {
  "io",
  "solution-not-found",
  "solution-parse",
  "project-parse",
  "unsupported-construct",
  "missing-import",
  "recursion-limit",
  "toolchain-not-found",
  "toolchain-incomplete",
  "invalid-configuration",
  "state-parse",
  "state-version",
  "project-schema",
  "storage-collision",
}

describe("sln2cc.notify.notify_error", function()
  local notify_calls, restore_notify
  local notify

  before_each(function()
    -- stub_notify runs before the require, so `vim.notify` is captured either way -- if
    -- `sln2cc.notify` doesn't exist yet, `after_each` can still restore it cleanly instead of
    -- compounding one failure (missing module) into two (restore_notify never assigned).
    notify_calls, restore_notify = stub_notify()
    package.loaded["sln2cc.notify"] = nil
    notify = require("sln2cc.notify")
  end)

  after_each(function()
    restore_notify()
  end)

  it("always notifies at ERROR level", function()
    notify.notify_error({ message = "boom", kind = nil, code = 1 })

    MiniTest.expect.equality(#notify_calls, 1)
    MiniTest.expect.equality(notify_calls[1].level, vim.log.levels.ERROR)
  end)

  it("a nil kind (spawn failure, clap usage error) notifies the message alone", function()
    notify.notify_error({ message = "sln2cc exited with code 2", kind = nil, code = 2 })

    MiniTest.expect.equality(#notify_calls, 1)
    local msg = notify_calls[1].msg
    MiniTest.expect.equality(msg:find("sln2cc exited with code 2", 1, true) ~= nil, true)
    -- No remedy line: a nil kind has nothing in M.REMEDIES to append.
    MiniTest.expect.equality(msg, "sln2cc: sln2cc exited with code 2")
  end)

  it("an unrecognized kind string falls back to the message-only form rather than erroring", function()
    -- Forward-compat: a kind cli/src/error.rs adds tomorrow, with no row in M.REMEDIES yet.
    local ok = pcall(
      notify.notify_error,
      { message = "something new", kind = "a-future-kind", code = 1 }
    )

    MiniTest.expect.equality(ok, true)
    MiniTest.expect.equality(notify_calls[1].msg, "sln2cc: something new")
  end)

  for _, kind in ipairs(ALL_KINDS) do
    it(("%q produces a notification carrying both the message and a remedy"):format(kind), function()
      notify.notify_error({ message = "the underlying failure", kind = kind, code = 1 })

      MiniTest.expect.equality(#notify_calls, 1)
      local msg = notify_calls[1].msg
      MiniTest.expect.equality(msg:find("the underlying failure", 1, true) ~= nil, true)
      MiniTest.expect.equality(notify.REMEDIES[kind] ~= nil, true, "M.REMEDIES is missing " .. kind)
      MiniTest.expect.equality(msg:find(notify.REMEDIES[kind], 1, true) ~= nil, true)
    end)
  end

  it("every known kind's remedy is distinct from every other kind's", function()
    -- The issue's own Tests line: "each error kind produces a distinct, actionable
    -- notification." A shared remedy string for two different kinds would fail a user
    -- exactly when they can't tell the two failures apart.
    local seen = {}
    for _, kind in ipairs(ALL_KINDS) do
      local remedy = notify.REMEDIES[kind]
      MiniTest.expect.equality(seen[remedy], nil, kind .. " shares a remedy with " .. tostring(seen[remedy]))
      seen[remedy] = kind
    end
  end)
end)

describe("sln2cc.notify.notify_warnings", function()
  local notify_calls, restore_notify
  local notify

  before_each(function()
    notify_calls, restore_notify = stub_notify()
    package.loaded["sln2cc.notify"] = nil
    notify = require("sln2cc.notify")
  end)

  after_each(function()
    restore_notify()
  end)

  it("nil sends nothing", function()
    notify.notify_warnings(nil)
    MiniTest.expect.equality(#notify_calls, 0)
  end)

  it("an empty list sends nothing", function()
    notify.notify_warnings({})
    MiniTest.expect.equality(#notify_calls, 0)
  end)

  it("any warning-severity diagnostic notifies at WARN", function()
    notify.notify_warnings({
      { severity = "note", message = "unexpected version", file = nil, line = nil },
      { severity = "warning", message = "unsupported construct", file = nil, line = nil },
    })

    MiniTest.expect.equality(#notify_calls, 1)
    MiniTest.expect.equality(notify_calls[1].level, vim.log.levels.WARN)
  end)

  it("an all-note list notifies at INFO", function()
    notify.notify_warnings({
      { severity = "note", message = "unexpected version", file = nil, line = nil },
      { severity = "note", message = "repeat import", file = nil, line = nil },
    })

    MiniTest.expect.equality(#notify_calls, 1)
    MiniTest.expect.equality(notify_calls[1].level, vim.log.levels.INFO)
  end)

  it("an unrecognized severity is treated as a warning for level purposes", function()
    -- ADR 13: a consumer must not branch exhaustively on severity, and must treat an unknown
    -- value as a warning rather than silently dropping it into the quieter INFO level.
    notify.notify_warnings({
      { severity = "something-new", message = "future construct", file = nil, line = nil },
    })

    MiniTest.expect.equality(#notify_calls, 1)
    MiniTest.expect.equality(notify_calls[1].level, vim.log.levels.WARN)
  end)

  it("batches every diagnostic into one notification, not one per diagnostic", function()
    notify.notify_warnings({
      { severity = "warning", message = "first", file = nil, line = nil },
      { severity = "warning", message = "second", file = nil, line = nil },
      { severity = "note", message = "third", file = nil, line = nil },
    })

    MiniTest.expect.equality(#notify_calls, 1)
    local msg = notify_calls[1].msg
    MiniTest.expect.equality(msg:find("first", 1, true) ~= nil, true)
    MiniTest.expect.equality(msg:find("second", 1, true) ~= nil, true)
    MiniTest.expect.equality(msg:find("third", 1, true) ~= nil, true)
  end)

  it("includes file:line when both are present", function()
    notify.notify_warnings({
      {
        severity = "warning",
        message = "unsupported construct",
        file = "C:/proj/app.vcxproj",
        line = 12,
      },
    })

    local msg = notify_calls[1].msg
    MiniTest.expect.equality(msg:find("C:/proj/app.vcxproj:12", 1, true) ~= nil, true)
    MiniTest.expect.equality(msg:find("unsupported construct", 1, true) ~= nil, true)
  end)

  it("a diagnostic with no file renders the message alone", function()
    notify.notify_warnings({
      { severity = "note", message = "no `Format Version` header", file = nil, line = nil },
    })

    MiniTest.expect.equality(
      notify_calls[1].msg:find("no `Format Version` header", 1, true) ~= nil,
      true
    )
  end)
end)
