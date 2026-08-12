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

describe("msvclsp.discovery", function()
  local discovery

  before_each(function()
    -- Fresh module state per test: the root cache persists across setup() calls, so a
    -- previous test's cached root must not leak into the next one's assertions.
    package.loaded["msvclsp.discovery"] = nil
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
    MiniTest.skip("not yet implemented")
  end)

  it("rescan refreshes the cache", function()
    MiniTest.skip("not yet implemented")
  end)

  it("remembers the active solution per project root", function()
    MiniTest.skip("not yet implemented")
  end)
end)
