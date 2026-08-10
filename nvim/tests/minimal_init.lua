-- Minimal init for headless mini.test runs. See the README for the invocation.

-- Directory containing this file's parent (the plugin root).
local this = debug.getinfo(1, "S").source:sub(2)
local plugin_root = vim.fn.fnamemodify(this, ":p:h:h")
vim.opt.runtimepath:prepend(plugin_root)

-- mini.test must be on the runtimepath. Prefer an explicit MINI_TEST_DIR (CI clones it),
-- otherwise fall back to a vim.pack-managed checkout if the developer already has one —
-- vim.pack (built into Neovim 0.12+) installs plugins under `site/pack/core/opt/<name>`
-- of stdpath("data"); see :help vim.pack.
local mini_test = os.getenv("MINI_TEST_DIR")
if not mini_test or mini_test == "" then
  mini_test = vim.fn.stdpath("data") .. "/site/pack/core/opt/mini.test"
end

if vim.fn.isdirectory(mini_test) == 0 then
  error("mini.test not found at " .. mini_test .. " — set MINI_TEST_DIR")
end

vim.opt.runtimepath:append(mini_test)
