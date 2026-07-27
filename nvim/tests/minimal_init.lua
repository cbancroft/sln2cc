-- Minimal init for headless plenary runs:
--   nvim --headless -u tests/minimal_init.lua \
--     -c "PlenaryBustedDirectory tests/ { minimal_init = 'tests/minimal_init.lua' }"

vim.opt.runtimepath:prepend(vim.fn.fnamemodify(vim.fn.expand("<sfile>:p"), ":h:h"))

-- plenary.nvim must be on the runtimepath; CI clones it next to this checkout.
local plenary = os.getenv("PLENARY_DIR") or "/tmp/plenary.nvim"
vim.opt.runtimepath:append(plenary)

vim.cmd("runtime plugin/plenary.vim")
