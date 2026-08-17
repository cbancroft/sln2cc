-- Plugin tests stub out the CLI binary rather than requiring a real Windows toolchain,
-- so they exercise plugin logic (caching, persistence, lifecycle) only.

--- Builds a fresh temp directory tree for one test and returns its root plus a cleanup
--- function. Every root-detection test needs an isolated tree — a shared fixture would let
--- an earlier test's `.git` leak into a later "no marker" case.
--- @return string root
--- @return fun() cleanup
--- Normalized (forward-slash) so it compares equal to `vim.fs.root()`'s return value, which
--- is always normalized regardless of `vim.fn.tempname()`'s native (backslash on Windows)
--- separators.
local function make_tree()
  local root = vim.fs.normalize(vim.fn.tempname())
  vim.fn.mkdir(root, "p")
  return root, function()
    vim.fn.delete(root, "rf")
  end
end

--- Creates a real, named buffer pointed at `path` inside a temp tree. `path` need not exist
--- on disk — `vim.fs.root()` only walks directories looking for markers, it never reads the
--- buffer's own file.
--- @param path string
--- @return integer bufnr
local function named_buffer(path)
  local buf = vim.api.nvim_create_buf(false, false)
  vim.api.nvim_buf_set_name(buf, path)
  return buf
end

--- Creates an empty `.sln` file at `tree .. "/" .. rel_path`, making parent directories as
--- needed. Content is irrelevant — solution discovery matches by filename only.
--- @param tree string
--- @param rel_path string
local function make_sln(tree, rel_path)
  local full = tree .. "/" .. rel_path
  local dir = vim.fs.dirname(full)
  vim.fn.mkdir(dir, "p")
  local fd = assert(io.open(full, "w"))
  fd:close()
end

describe("msvclsp.discovery", function()
  local discovery

  before_each(function()
    -- Fresh module state per test: the root cache and solutions cache both persist across
    -- calls, so a previous test's cached values must not leak into the next one's assertions.
    -- config is reset too, since a test overriding scan_max_depth must not bleed into the
    -- next test's default-depth expectations.
    package.loaded["msvclsp.discovery"] = nil
    package.loaded["msvclsp.config"] = nil
    discovery = require("msvclsp.discovery")
  end)

  it("finds the project root from configured root markers", function()
    local tree, cleanup = make_tree()
    local ok, err = pcall(function()
      -- Marker at the same level as the buffer.
      vim.fn.mkdir(tree .. "/.git", "p")
      local buf = named_buffer(tree .. "/file.cpp")
      MiniTest.expect.equality(discovery.project_root(buf), tree)
    end)
    cleanup()
    if not ok then
      error(err, 0)
    end
  end)

  it("finds the project root several levels above the buffer", function()
    local tree, cleanup = make_tree()
    local ok, err = pcall(function()
      vim.fn.mkdir(tree .. "/.git", "p")
      local nested = tree .. "/a/b/c"
      vim.fn.mkdir(nested, "p")
      local buf = named_buffer(nested .. "/file.cpp")
      MiniTest.expect.equality(discovery.project_root(buf), tree)
    end)
    cleanup()
    if not ok then
      error(err, 0)
    end
  end)

  it("falls back to the current working directory when no marker is found", function()
    -- No `.git` anywhere under `tree`, and `tree` itself lives under the system temp
    -- directory, well outside this repository's own `.git` ancestry.
    local tree, cleanup = make_tree()
    local ok, err = pcall(function()
      local buf = named_buffer(tree .. "/file.cpp")
      MiniTest.expect.equality(discovery.project_root(buf), vim.fn.getcwd())
    end)
    cleanup()
    if not ok then
      error(err, 0)
    end
  end)

  it("falls back to the current working directory for an unnamed buffer", function()
    -- `vim.fs.root()` treats an unnamed buffer by walking up from the cwd looking for a
    -- marker, so this must run from a tree with no `.git` in its ancestry — otherwise it
    -- would find this repository's own `.git` instead of exercising the fallback.
    local tree, cleanup = make_tree()
    local original_cwd = vim.fn.getcwd()
    vim.fn.chdir(tree)
    local ok, err = pcall(function()
      local buf = vim.api.nvim_create_buf(false, true)
      local root_ok, root = pcall(discovery.project_root, buf)
      MiniTest.expect.equality(root_ok, true)
      MiniTest.expect.equality(root, vim.fn.getcwd())
    end)
    vim.fn.chdir(original_cwd)
    cleanup()
    if not ok then
      error(err, 0)
    end
  end)

  it("caches the result per buffer instead of rescanning", function()
    local tree, cleanup = make_tree()
    local ok, err = pcall(function()
      vim.fn.mkdir(tree .. "/.git", "p")
      local buf = named_buffer(tree .. "/file.cpp")

      local first = discovery.project_root(buf)
      MiniTest.expect.equality(first, tree)

      -- Remove the marker. A rescan would now resolve differently (fall back to cwd); a
      -- cached lookup must still return the original answer.
      vim.fn.delete(tree .. "/.git", "rf")
      local second = discovery.project_root(buf)
      MiniTest.expect.equality(second, first)
    end)
    cleanup()
    if not ok then
      error(err, 0)
    end
  end)

  it("caches discovered solutions instead of rescanning", function()
    local tree, cleanup = make_tree()
    local ok, err = pcall(function()
      make_sln(tree, "App.sln")
      local first = discovery.solutions(tree)
      MiniTest.expect.equality(first, { tree .. "/App.sln" })

      -- A solution added after the first scan must not appear without an explicit rescan.
      make_sln(tree, "Other.sln")
      local second = discovery.solutions(tree)
      MiniTest.expect.equality(second, first)
    end)
    cleanup()
    if not ok then
      error(err, 0)
    end
  end)

  it("rescan refreshes the cache", function()
    local tree, cleanup = make_tree()
    local ok, err = pcall(function()
      make_sln(tree, "App.sln")
      discovery.solutions(tree)

      make_sln(tree, "Other.sln")
      local refreshed = discovery.rescan(tree)
      MiniTest.expect.equality(refreshed, { tree .. "/App.sln", tree .. "/Other.sln" })
      MiniTest.expect.equality(discovery.solutions(tree), refreshed)
    end)
    cleanup()
    if not ok then
      error(err, 0)
    end
  end)

  it("remembers the active solution per project root", function()
    local tree_a, cleanup_a = make_tree()
    local tree_b, cleanup_b = make_tree()
    local ok, err = pcall(function()
      make_sln(tree_a, "App.sln")
      make_sln(tree_a, "Other.sln")
      make_sln(tree_b, "Different.sln")

      -- Sorted ascending: "App.sln" precedes "Other.sln" lexically.
      MiniTest.expect.equality(discovery.active_solution(tree_a), tree_a .. "/App.sln")
      MiniTest.expect.equality(discovery.active_solution(tree_a), tree_a .. "/App.sln")
      MiniTest.expect.equality(discovery.active_solution(tree_b), tree_b .. "/Different.sln")
    end)
    cleanup_a()
    cleanup_b()
    if not ok then
      error(err, 0)
    end
  end)

  -- set_active_solution / persistence -- V1-03. discovery.lua's own file, not the CLI's
  -- state.json: ADR 44 assigns "remembering a choice" to the plugin, never the CLI.

  it("set_active_solution overrides the discovered default", function()
    local tree, cleanup = make_tree()
    local ok, err = pcall(function()
      make_sln(tree, "App.sln")
      make_sln(tree, "Other.sln")

      -- "App.sln" would otherwise win as the sorted-first default.
      discovery.set_active_solution(tree, tree .. "/Other.sln")

      MiniTest.expect.equality(discovery.active_solution(tree), tree .. "/Other.sln")
    end)
    cleanup()
    if not ok then
      error(err, 0)
    end
  end)

  it("the choice persists across a fresh require (simulated restart)", function()
    local tree, cleanup = make_tree()
    local ok, err = pcall(function()
      make_sln(tree, "App.sln")
      make_sln(tree, "Other.sln")

      discovery.set_active_solution(tree, tree .. "/Other.sln")

      -- Drop every module-level cache by reloading, mirroring a fresh Neovim session.
      package.loaded["msvclsp.discovery"] = nil
      local reloaded = require("msvclsp.discovery")

      MiniTest.expect.equality(reloaded.active_solution(tree), tree .. "/Other.sln")
    end)
    cleanup()
    if not ok then
      error(err, 0)
    end
  end)

  it("a rescan does not forget a persisted choice", function()
    local tree, cleanup = make_tree()
    local ok, err = pcall(function()
      make_sln(tree, "App.sln")
      make_sln(tree, "Other.sln")
      discovery.set_active_solution(tree, tree .. "/Other.sln")

      -- rescan() clears the in-memory active-solution cache; the persisted choice must still
      -- win over "first found" once it's re-derived.
      discovery.rescan(tree)

      MiniTest.expect.equality(discovery.active_solution(tree), tree .. "/Other.sln")
    end)
    cleanup()
    if not ok then
      error(err, 0)
    end
  end)

  it("persisted choices are isolated per project root", function()
    local tree_a, cleanup_a = make_tree()
    local tree_b, cleanup_b = make_tree()
    local ok, err = pcall(function()
      make_sln(tree_a, "App.sln")
      make_sln(tree_a, "Other.sln")
      make_sln(tree_b, "App.sln")
      make_sln(tree_b, "Other.sln")

      discovery.set_active_solution(tree_a, tree_a .. "/Other.sln")

      MiniTest.expect.equality(discovery.active_solution(tree_a), tree_a .. "/Other.sln")
      -- tree_b never had a choice persisted, so it still falls back to "first found."
      MiniTest.expect.equality(discovery.active_solution(tree_b), tree_b .. "/App.sln")
    end)
    cleanup_a()
    cleanup_b()
    if not ok then
      error(err, 0)
    end
  end)

  -- vanished-solution detection -- V2-01. Resolves project_spec.md's "what happens when the
  -- remembered/active solution's .sln file no longer exists" Open Question: rather than
  -- returning a dead path (or silently substituting a different solution the user never
  -- chose, the way a corrupt persisted file does above), active_solution reports the vanished
  -- path via a second return value and clears its own stale state. See ADR 48.

  it("a deleted persisted solution is reported as vanished and not returned", function()
    local tree, cleanup = make_tree()
    local ok, err = pcall(function()
      make_sln(tree, "App.sln")
      local target = tree .. "/App.sln"
      discovery.set_active_solution(tree, target)

      vim.fn.delete(target)

      local solution, vanished = discovery.active_solution(tree)
      MiniTest.expect.equality(solution, nil)
      MiniTest.expect.equality(vanished, target)
    end)
    cleanup()
    if not ok then
      error(err, 0)
    end
  end)

  it("a vanished solution's persisted choice is cleared, not reported forever", function()
    local tree, cleanup = make_tree()
    local ok, err = pcall(function()
      make_sln(tree, "App.sln")
      make_sln(tree, "Other.sln")
      local target = tree .. "/App.sln"
      discovery.set_active_solution(tree, target)
      vim.fn.delete(target)

      -- First call discovers and reports the vanished path.
      local _, first_vanished = discovery.active_solution(tree)
      MiniTest.expect.equality(first_vanished, target)

      -- A second call must not keep reporting the same dead path -- the persisted choice was
      -- cleared, so this falls through to whatever's next: the other discovered solution.
      local solution, second_vanished = discovery.active_solution(tree)
      MiniTest.expect.equality(second_vanished, nil)
      MiniTest.expect.equality(solution, tree .. "/Other.sln")

      -- The clear reached disk too, not just the in-memory cache -- a fresh require (simulated
      -- restart) must not resurrect the dead persisted choice either.
      package.loaded["msvclsp.discovery"] = nil
      local reloaded = require("msvclsp.discovery")
      local reloaded_solution, reloaded_vanished = reloaded.active_solution(tree)
      MiniTest.expect.equality(reloaded_vanished, nil)
      MiniTest.expect.equality(reloaded_solution, tree .. "/Other.sln")
    end)
    cleanup()
    if not ok then
      error(err, 0)
    end
  end)

  it("a moved solution is found again after a rescan", function()
    local tree, cleanup = make_tree()
    local ok, err = pcall(function()
      make_sln(tree, "App.sln")
      local old_path = tree .. "/App.sln"
      discovery.set_active_solution(tree, old_path)

      -- Simulate a move: the old path is gone, the same solution now lives elsewhere under
      -- the same tree. There's no file-identity tracking here (see ADR 48) -- this is
      -- indistinguishable from "deleted, then an unrelated file appeared" and recovers the
      -- same way: rescan surfaces the new location once someone asks.
      vim.fn.delete(old_path)
      make_sln(tree, "Moved/App.sln")

      local refreshed = discovery.rescan(tree)
      MiniTest.expect.equality(refreshed, { tree .. "/Moved/App.sln" })
    end)
    cleanup()
    if not ok then
      error(err, 0)
    end
  end)

  it("a corrupt persisted file falls back to first-found, not an error", function()
    local tree, cleanup = make_tree()
    local ok, err = pcall(function()
      make_sln(tree, "App.sln")
      vim.fn.mkdir(tree .. "/.msvclsp", "p")
      local fd = assert(io.open(tree .. "/.msvclsp/nvim-solution.json", "w"))
      fd:write("{ not valid json")
      fd:close()

      MiniTest.expect.equality(discovery.active_solution(tree), tree .. "/App.sln")
    end)
    cleanup()
    if not ok then
      error(err, 0)
    end
  end)

  it("finds a nested solution several levels below the root", function()
    local tree, cleanup = make_tree()
    local ok, err = pcall(function()
      make_sln(tree, "a/b/c/App.sln")
      MiniTest.expect.equality(discovery.solutions(tree), { tree .. "/a/b/c/App.sln" })
    end)
    cleanup()
    if not ok then
      error(err, 0)
    end
  end)

  it("skips configured ignore directories", function()
    local tree, cleanup = make_tree()
    local ok, err = pcall(function()
      -- Both a default-ignored VCS directory and one holding an actual, findable solution.
      make_sln(tree, "node_modules/Vendored.sln")
      make_sln(tree, "src/App.sln")
      MiniTest.expect.equality(discovery.solutions(tree), { tree .. "/src/App.sln" })
    end)
    cleanup()
    if not ok then
      error(err, 0)
    end
  end)

  it("returns an empty list when no solution is found", function()
    local tree, cleanup = make_tree()
    local ok, err = pcall(function()
      MiniTest.expect.equality(discovery.solutions(tree), {})
      MiniTest.expect.equality(discovery.active_solution(tree), nil)
    end)
    cleanup()
    if not ok then
      error(err, 0)
    end
  end)

  it("does not descend past the configured max depth", function()
    local tree, cleanup = make_tree()
    local ok, err = pcall(function()
      require("msvclsp.config").setup({ scan_max_depth = 1 })
      -- Two levels below tree: deeper than the configured max_depth of 1.
      make_sln(tree, "a/b/TooDeep.sln")
      MiniTest.expect.equality(discovery.solutions(tree), {})
    end)
    cleanup()
    if not ok then
      error(err, 0)
    end
  end)

  -- config-based solution pinning -- V2-02. Resolves project_spec.md's "should there be a way
  -- to explicitly pin/override the active solution via config... bypassing the interactive
  -- picker?" Open Question. A pin is checked before either the persisted choice or the
  -- first-discovered fallback, and is reported through a *third* return value (`pin_error`)
  -- distinct from `vanished` (V2-01/ADR 48): a missing pin must not trigger that flow's
  -- rescan-and-reprompt, since a pin exists precisely for unattended CI/scripting callers with
  -- no one to answer a `vim.ui.select` prompt. See ADR 49.

  it("a pinned solution wins over a persisted choice", function()
    local tree, cleanup = make_tree()
    local ok, err = pcall(function()
      make_sln(tree, "App.sln")
      make_sln(tree, "Other.sln")
      discovery.set_active_solution(tree, tree .. "/App.sln")

      require("msvclsp.config").setup({ pinned_solution = tree .. "/Other.sln" })

      local solution, vanished, pin_error = discovery.active_solution(tree)
      MiniTest.expect.equality(solution, tree .. "/Other.sln")
      MiniTest.expect.equality(vanished, nil)
      MiniTest.expect.equality(pin_error, nil)
    end)
    cleanup()
    if not ok then
      error(err, 0)
    end
  end)

  it("a pinned solution wins over the first-discovered default", function()
    local tree, cleanup = make_tree()
    local ok, err = pcall(function()
      -- No persisted choice. "App.sln" would otherwise win as the sorted-first default.
      make_sln(tree, "App.sln")
      make_sln(tree, "Other.sln")

      require("msvclsp.config").setup({ pinned_solution = tree .. "/Other.sln" })

      MiniTest.expect.equality(discovery.active_solution(tree), tree .. "/Other.sln")
    end)
    cleanup()
    if not ok then
      error(err, 0)
    end
  end)

  it("a pinned solution that does not exist is reported via pin_error, not returned", function()
    local tree, cleanup = make_tree()
    local ok, err = pcall(function()
      make_sln(tree, "App.sln")
      local missing = tree .. "/Ghost.sln"

      require("msvclsp.config").setup({ pinned_solution = missing })

      local solution, vanished, pin_error = discovery.active_solution(tree)
      MiniTest.expect.equality(solution, nil)
      -- Not the vanished-recovery case: nothing was ever persisted, and reporting this as
      -- `vanished` would make commands.lua rescan and open the picker -- exactly what a pin
      -- exists to avoid.
      MiniTest.expect.equality(vanished, nil)
      MiniTest.expect.equality(pin_error, missing)
    end)
    cleanup()
    if not ok then
      error(err, 0)
    end
  end)

  it("an unset pin falls through to existing behavior", function()
    local tree, cleanup = make_tree()
    local ok, err = pcall(function()
      make_sln(tree, "App.sln")
      make_sln(tree, "Other.sln")
      discovery.set_active_solution(tree, tree .. "/Other.sln")

      -- Default config: pinned_solution is "", not a path -- must not be treated as a pin.
      local solution, vanished, pin_error = discovery.active_solution(tree)
      MiniTest.expect.equality(solution, tree .. "/Other.sln")
      MiniTest.expect.equality(vanished, nil)
      MiniTest.expect.equality(pin_error, nil)
    end)
    cleanup()
    if not ok then
      error(err, 0)
    end
  end)
end)
