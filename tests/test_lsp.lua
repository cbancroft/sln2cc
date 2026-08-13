-- lsp.lua builds on Neovim's own vim.lsp.config/vim.lsp.enable (see ADR 36) rather than a
-- private client, so its own tests stub the four calls that reach into that global LSP state
-- (M._get_config, M._set_config, M._enable, M._get_clients) instead of stubbing a require'd
-- module the way every other test file does per CLAUDE.md §4 -- there is nothing to
-- package.loaded-swap here, since vim.lsp itself is the seam.

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

--- A fake `vim.lsp.Client` with a spied `stop()`.
--- @param opts { name: string|nil, root_dir: string|nil }|nil
--- @return table client
--- @return fun(): boolean stopped
local function fake_client(opts)
  opts = opts or {}
  local stopped = false
  local client = { name = opts.name or "clangd", root_dir = opts.root_dir }
  client.stop = function(_self)
    stopped = true
  end
  return client, function()
    return stopped
  end
end

describe("msvclsp.lsp", function()
  local lsp
  local notify_calls, restore_notify
  local original_get_bufname

  before_each(function()
    package.loaded["msvclsp.lsp"] = nil
    package.loaded["msvclsp.config"] = nil
    lsp = require("msvclsp.lsp")
    notify_calls, restore_notify = stub_notify()
    original_get_bufname = vim.api.nvim_buf_get_name
  end)

  after_each(function()
    restore_notify()
    vim.api.nvim_buf_get_name = original_get_bufname
  end)

  describe("compile_commands_dir", function()
    it("returns the dirname of the given database path", function()
      MiniTest.expect.equality(
        lsp.compile_commands_dir("/root/.msvclsp/App-abcd1234/compile_commands.json"),
        "/root/.msvclsp/App-abcd1234"
      )
    end)
  end)

  describe("start", function()
    it("configures clangd with an explicit --compile-commands-dir, a root_dir function, and enables it", function()
      local set_calls, enable_calls = {}, {}
      lsp._get_config = function()
        return nil
      end
      lsp._set_config = function(name, cfg)
        table.insert(set_calls, { name = name, cfg = cfg })
      end
      lsp._enable = function(name)
        table.insert(enable_calls, name)
      end

      lsp.start("/root/.msvclsp/App-abcd1234/compile_commands.json", "/root")

      MiniTest.expect.equality(#set_calls, 1)
      MiniTest.expect.equality(set_calls[1].name, "clangd")
      MiniTest.expect.equality(
        set_calls[1].cfg.cmd[#set_calls[1].cfg.cmd],
        "--compile-commands-dir=/root/.msvclsp/App-abcd1234"
      )
      MiniTest.expect.equality(type(set_calls[1].cfg.root_dir), "function")
      MiniTest.expect.equality(enable_calls, { "clangd" })
    end)

    it("preserves fields already set on the clangd config", function()
      local sentinel_on_attach = function() end
      lsp._get_config = function()
        return { on_attach = sentinel_on_attach, cmd = { "clangd", "--old-flag" } }
      end
      local set_calls = {}
      lsp._set_config = function(name, cfg)
        table.insert(set_calls, { name = name, cfg = cfg })
      end
      lsp._enable = function() end

      lsp.start("/root/.msvclsp/App-abcd1234/compile_commands.json", "/root")

      MiniTest.expect.equality(set_calls[1].cfg.on_attach, sentinel_on_attach)
      -- cmd itself is fully replaced (it has to be, to control the trailing flag), not merged.
      MiniTest.expect.equality(
        set_calls[1].cfg.cmd[#set_calls[1].cfg.cmd],
        "--compile-commands-dir=/root/.msvclsp/App-abcd1234"
      )
    end)

    it("the root_dir function calls on_dir for a buffer under root", function()
      lsp._get_config = function()
        return nil
      end
      local set_calls = {}
      lsp._set_config = function(name, cfg)
        table.insert(set_calls, { name = name, cfg = cfg })
      end
      lsp._enable = function() end

      lsp.start("/root/.msvclsp/App-abcd1234/compile_commands.json", "/root")

      vim.api.nvim_buf_get_name = function()
        return "/root/src/main.cpp"
      end
      local called_with
      set_calls[1].cfg.root_dir(1, function(dir)
        called_with = dir
      end)

      MiniTest.expect.equality(called_with, "/root")
    end)

    it("the root_dir function never calls on_dir for a buffer outside root", function()
      lsp._get_config = function()
        return nil
      end
      local set_calls = {}
      lsp._set_config = function(name, cfg)
        table.insert(set_calls, { name = name, cfg = cfg })
      end
      lsp._enable = function() end

      lsp.start("/root/.msvclsp/App-abcd1234/compile_commands.json", "/root")

      vim.api.nvim_buf_get_name = function()
        return "/elsewhere/other.cpp"
      end
      local called = false
      set_calls[1].cfg.root_dir(1, function()
        called = true
      end)

      MiniTest.expect.equality(called, false)
    end)

    it("with no database notifies and never touches vim.lsp.config", function()
      local set_called, enable_called = false, false
      lsp._set_config = function()
        set_called = true
      end
      lsp._enable = function()
        enable_called = true
      end

      lsp.start(nil, "/root")

      MiniTest.expect.equality(set_called, false)
      MiniTest.expect.equality(enable_called, false)
      MiniTest.expect.equality(#notify_calls, 1)
      MiniTest.expect.equality(notify_calls[1].level, vim.log.levels.WARN)
    end)
  end)

  describe("restart", function()
    it("stops running clangd clients rooted at the given root, leaving other roots alone", function()
      local ours, stopped_ours = fake_client({ root_dir = "/root" })
      local theirs, stopped_theirs = fake_client({ root_dir = "/somewhere/else" })
      lsp._get_clients = function(filter)
        MiniTest.expect.equality(filter, { name = "clangd" })
        return { ours, theirs }
      end
      lsp._get_config = function()
        return nil
      end
      lsp._set_config = function() end
      lsp._enable = function() end

      lsp.restart("/root/.msvclsp/App-abcd1234/compile_commands.json", "/root")

      MiniTest.expect.equality(stopped_ours(), true)
      MiniTest.expect.equality(stopped_theirs(), false)
    end)

    it("reconfigures and re-enables after stopping", function()
      lsp._get_clients = function()
        return { (fake_client({ root_dir = "/root" })) }
      end
      lsp._get_config = function()
        return nil
      end
      local set_calls, enable_calls = {}, {}
      lsp._set_config = function(name, cfg)
        table.insert(set_calls, { name = name, cfg = cfg })
      end
      lsp._enable = function(name)
        table.insert(enable_calls, name)
      end

      lsp.restart("/root/.msvclsp/App-abcd1234/compile_commands.json", "/root")

      MiniTest.expect.equality(#set_calls, 1)
      MiniTest.expect.equality(enable_calls, { "clangd" })
    end)

    it("with no matching running client still configures and enables (degenerates to start)", function()
      lsp._get_clients = function()
        return { (fake_client({ root_dir = "/somewhere/else" })) }
      end
      lsp._get_config = function()
        return nil
      end
      local set_calls, enable_calls = {}, {}
      lsp._set_config = function(name, cfg)
        table.insert(set_calls, { name = name, cfg = cfg })
      end
      lsp._enable = function(name)
        table.insert(enable_calls, name)
      end

      lsp.restart("/root/.msvclsp/App-abcd1234/compile_commands.json", "/root")

      MiniTest.expect.equality(#set_calls, 1)
      MiniTest.expect.equality(enable_calls, { "clangd" })
    end)

    it("with no database notifies and never lists, configures, or enables anything", function()
      local get_clients_called, set_called, enable_called = false, false, false
      lsp._get_clients = function()
        get_clients_called = true
        return {}
      end
      lsp._set_config = function()
        set_called = true
      end
      lsp._enable = function()
        enable_called = true
      end

      lsp.restart(nil, "/root")

      MiniTest.expect.equality(get_clients_called, false)
      MiniTest.expect.equality(set_called, false)
      MiniTest.expect.equality(enable_called, false)
      MiniTest.expect.equality(#notify_calls, 1)
      MiniTest.expect.equality(notify_calls[1].level, vim.log.levels.WARN)
    end)
  end)
end)
