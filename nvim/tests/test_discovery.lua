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
end)
