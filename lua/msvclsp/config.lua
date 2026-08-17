--- User configuration and defaults.

local M = {}

M.defaults = {
  --- Path to the msvclsp binary. Resolution strategy is an open question
  --- (bundled build vs. release download vs. cargo install).
  bin = "msvclsp",

  --- Root markers used to locate the project root, matching what Neovim's LSP clients
  --- already use. `.msvclsp/` is created here.
  root_markers = { ".git" },

  --- Directory name for generated artifacts, relative to the project root.
  data_dir = ".msvclsp",

  --- Restart clangd automatically after regenerating the compilation database.
  auto_restart_clangd = true,

  --- Directory names skipped while scanning for `.sln` files below the project root. Matched
  --- by name, not path — a directory called `bin` is skipped everywhere in the tree, not just
  --- at the root. Scanning never descends into these, so a huge `node_modules` costs nothing.
  scan_ignore_dirs = {
    ".git", ".msvclsp", "node_modules", "bin", "obj", "packages",
    "Debug", "Release", "x64", "x86", "Win32",
  },

  --- Maximum directory levels descended below the project root while scanning for `.sln`
  --- files. Bounds worst-case scan cost on a deep or unexpectedly large tree.
  scan_max_depth = 6,

  --- Command used to launch clangd. Can add any other arguments after this if you want to customize
  --- the execution. We always append the `--compile-commands-dir=<dir>`.
  clangd_cmd = { "clangd" },

  --- Filetypes clangd is started for.
  filetypes = { "c", "cpp" },

  --- Absolute path to a `.sln` file that is always considered the 'active' solution, no matter what 
  --- the persisted or selected solution is. Intended for CI/scripting use where nothing can 
  --- respond to a `vim.ui.select` prompt. Empty string means no pin.
  pinned_solution = "",
}

M.options = vim.deepcopy(M.defaults)

--- Recursively checks `opts` against `defaults`, collecting every problem found.
--- @param defaults table
--- @param opts table
--- @param path string dotted path for error messages, "" at the root
--- @param errors string[] error accumulator
local function validate(defaults, opts, path, errors)
  for key, value in pairs(opts) do
    local full_key = (path == "" and key) or (path .. "." .. key)
    local default_value = defaults[key]
    if default_value == nil then
      table.insert(errors, ("unknown option %q"):format(full_key))
    elseif type(value) ~= type(default_value) then
      table.insert(
        errors,
        ("option %q must be a %s, got %s"):format(full_key, type(default_value), type(value))
      )
    elseif type(value) == "table" then 
      if vim.islist(default_value) then
        -- List valued option: element-type check only.
        if not vim.islist(value) then
          table.insert(errors, ("option %q must be a list"):format(full_key))
        else
          local element_type = type(default_value[1])
          for i, element in ipairs(value) do
            if type(element) ~= element_type then
              table.insert(
                errors,
                ("option %q[%d] must be a %s"):format(full_key, i, element_type)
              )
            end
          end
        end
      else
        -- Dict shaped table. Recurse into it
        validate(default_value, value, full_key, errors)
      end
    end
  end
end

--- Merges `opts` over `defaults`. Lists are replaced wholesale. Dict-shaped nested tables merge key-by-key. 
--- Always builds a fresh table
--- @param defaults table
--- @param opts table
--- @return table
local function merge(defaults, opts)
  local result = vim.deepcopy(defaults)
  for key, value in pairs(opts) do
    if type(value) == "table" and not vim.islist(value) and type(result[key]) == "table" then
      result[key] = merge(result[key], value)
    else
      result[key] = vim.deepcopy(value)
    end
  end

  return result
end

--- Entry point called by the user. Safe to call more than once, each call resolves fresh against defaults.
--- @param opts table|nil user configuration, merged over defaults
--- @return table|nil resolved options or nil if `opts` did not validate
function M.setup(opts)
  opts = opts or {}
  local errors = {}
  validate(M.defaults, opts, "", errors)
  if #errors > 0 then
    vim.notify(
      "msvclsp: invalid configuration:\n  " .. table.concat(errors, "\n  "),
      vim.log.levels.ERROR
    )
    return nil
  end

  M.options = merge(M.defaults, opts)
  return M.options
end

return M
