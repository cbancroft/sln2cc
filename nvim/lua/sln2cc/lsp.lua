--- clangd lifecycle management.
---
--- clangd is always launched pointed at an explicit compile_commands.json under
--- `.sln2cc/<solution-name>-<hash>/`, never relying on its own directory-walk discovery, because
--- a solution's location in the tree may not line up with where sources live. The path is always handed
--- in by the caller.
---
--- Integrated with Neovim's `vim.lsp.config` and `vim.lsp.enable`. This overrides the clangd config and
--- (re-)enables it, letting `FileType` autocmds start and attach clangd as normal. We just point the
--- clangd instance at the proper `compile_commands.json`.

local config = require("sln2cc.config")

local M = {}

--- Overrideable so that tests can intercept without actual LSP state.
--- @type fun(name: string): table|nil
M._get_config = function(name)
  return vim.lsp.config[name]
end

--- @type fun(name: string, cfg: table)
M._set_config = function(name, cfg)
  vim.lsp.config[name] = cfg
end

--- @type fun(name: string)
M._enable = vim.lsp.enable

--- @type fun(filter: table): table[]
M._get_clients = vim.lsp.get_clients


--- Directory clangd should index from, given the absolute path to a generated `compile_commands.json`.
--- @param database string
--- @return string
function M.compile_commands_dir(database)
  return vim.fs.dirname(database)
end

--- Whether `bufnr`'s file lies at or under `root`. Case-sensitive, normalized-prefix check. Should
--- be good enough to gate autostart.
--- @param bufnr integer
--- @param root string
--- @return boolean
local function under_root(bufnr, root)
  local bufname = vim.api.nvim_buf_get_name(bufnr)
  if bufname == "" then
    return false
  end
 
  local buf_path = vim.fs.normalize(bufname)
  local root_path = vim.fs.normalize(root)
  return buf_path == root_path or vim.startswith(buf_path, root_path .. "/")
end

--- Overrides the "clangd" LSP config with `database`/`root`, preserving other fields already
--- set for it and (re-)enables it so the FileType autocmd picks up the changes for every matching
--- buffer under `root`. A buffer outside `root` is left alone.
--- @param database string
--- @param root string
local function activate(database, root)
  local cmd = vim.deepcopy(config.options.clangd_cmd)
  table.insert(cmd, "--compile-commands-dir=" .. M.compile_commands_dir(database))

  local merged = vim.tbl_extend("force", M._get_config("clangd") or {}, {
    cmd = cmd,
    filetypes = config.options.filetypes,
    root_dir = function(bufnr, on_dir)
      if under_root(bufnr, root) then
        on_dir(root)
      end
    end,
  })

  M._set_config("clangd", merged)
  M._enable("clangd")
end

--- Start clangd pointed at `database` for buffers under `root`. A no-op if nothing has been generated
--- yet.
--- @param database string|nil absolute path to compile_commands.json. Nil means not generated yet.
--- @param root string project root. Only buffers under it are pointed at `database`
function M.start(database, root)
  if not database then
    vim.notify(
      "sln2cc: no compile_commands.json generated yet. Pick a configuration first.",
      vim.log.levels.WARN
    )
    return
  end

  activate(database, root)
end

--- Restart clangd — used after regenerating the database, switching configuration/platform,
--- or switching solutions. Stops running clangd clients rooted at `root` so `M.start`'s FileType
--- re-trigger starts fresh ones.
--- @param database string|nil
--- @param root string
function M.restart(database, root)
  if not database then
    vim.notify(
      "sln2cc: no compile_commands.json generated yet. Pick a configuration first.",
      vim.log.levels.WARN
    )
    return
  end

  for _, client in ipairs(M._get_clients({ name = "clangd" })) do
    if client.root_dir == root then
      client:stop()
    end
  end

  activate(database, root)
end

return M
