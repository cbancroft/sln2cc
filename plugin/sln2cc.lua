-- Loaded automatically by Neovim on startup.
--
-- Keep this minimal: register commands lazily and let require("sln2cc").setup() do the
-- real configuration, so merely installing the plugin costs nothing at startup.

if vim.g.loaded_sln2cc then
  return
end
vim.g.loaded_sln2cc = true
require("sln2cc.commands").register()
