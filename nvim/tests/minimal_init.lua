-- Minimal init for headless plenary runs. See the README for the invocation.

-- Directory containing this file's parent (the plugin root).
local this = debug.getinfo(1, "S").source:sub(2)
local plugin_root = vim.fn.fnamemodify(this, ":p:h:h")
vim.opt.runtimepath:prepend(plugin_root)

-- plenary.nvim must be on the runtimepath. Prefer an explicit PLENARY_DIR (CI clones it),
-- otherwise fall back to a lazy.nvim-managed checkout if the developer already has one.
local plenary = os.getenv("PLENARY_DIR")
if not plenary or plenary == "" then
  plenary = vim.fn.stdpath("data") .. "/lazy/plenary.nvim"
end

if vim.fn.isdirectory(plenary) == 0 then
  error("plenary.nvim not found at " .. plenary .. " — set PLENARY_DIR")
end

vim.opt.runtimepath:append(plenary)
vim.cmd("runtime plugin/plenary.vim")
