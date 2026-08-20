--- Renders structured CLI errors and evaluation warnings as vim.notify calls.
---
--- Centralizes the ErrorKind -> remedy table so "distinct, actionable notification" is one
--- decision made once, rather than reinvented at every call site that receives a
--- Sln2cc.CliError. Mirrors how diagnostics.rs centralizes wording on the CLI side (ADR 13).
--- See ADR 50.

local M = {}

--- ErrorKind wire string (cli/src/error.rs::ErrorKind::as_str) -> one-line remedy, appended
--- after the CLI's own message. Kept in sync by hand; there is no cross-language type check,
--- so a kind added to error.rs without a row here falls back to the message-only form below
--- rather than erroring -- silently under-informative, never wrong.
M.REMEDIES = {
  ["io"] = "Check that the path above exists and sln2cc can read/write it.",
  ["solution-not-found"] = "Verify the .sln path, or run :Sln2ccRescan / :Sln2ccSelectSolution if it moved.",
  ["solution-parse"] = "Open the .sln at the line above -- it may not be a valid Visual Studio solution file.",
  ["project-parse"] = "The referenced project file has malformed XML; check it for a stray tag or bad encoding.",
  ["unsupported-construct"] = "sln2cc does not model this MSBuild construct yet; generated flags may be wrong until it's simplified.",
  ["missing-import"] = "An imported .props/.targets file is missing on disk; restore it or fix the Import path.",
  ["recursion-limit"] = "An import chain or macro expansion went deeper than expected; check for a cyclical Import.",
  ["toolchain-not-found"] = "No usable Visual Studio installation with the C++ workload was found.",
  ["toolchain-incomplete"] = "Visual Studio is installed but a toolset or Windows SDK component is missing.",
  ["invalid-configuration"] = "Run :Sln2ccSelectConfig to choose one of the configurations listed above.",
  ["state-parse"] = "sln2cc's .sln2cc/state.json is corrupt; delete it and run :Sln2ccGenerate!.",
  ["state-version"] = "sln2cc's state file is from an incompatible version; delete .sln2cc/state.json and regenerate.",
  ["project-schema"] = "The project file has a value sln2cc doesn't recognize at the location above.",
  ["storage-collision"] = "Two solution directories hash to the same .sln2cc entry; see both paths above.",
}

--- @param err Sln2cc.CliError see cli.lua's own class annotation
function M.notify_error(err)
  local remedy = err.kind and M.REMEDIES[err.kind]
  local text = remedy and (err.message .. "\n" .. remedy) or err.message
  vim.notify("sln2cc: " .. text, vim.log.levels.ERROR)
end

--- @param warnings table[]|nil Diagnostic-shaped: { severity, message, file, line }
function M.notify_warnings(warnings)
  if not warnings or #warnings == 0 then
    return
  end

  local has_warning = false
  local lines = {}
  for _, w in ipairs(warnings) do
    if w.severity ~= "note" then
      -- ADR 13: don't branch exhaustively on severity. Anything that isn't exactly "note"
      -- (today just "warning", tomorrow maybe something new) reads as a warning.
      has_warning = true
    end
    local site = w.file
    if site and w.line then
      site = site .. ":" .. w.line
    end
    table.insert(lines, site and (site .. ": " .. w.message) or w.message)
  end

  local level = has_warning and vim.log.levels.WARN or vim.log.levels.INFO
  vim.notify("sln2cc:\n" .. table.concat(lines, "\n"), level)
end

return M
