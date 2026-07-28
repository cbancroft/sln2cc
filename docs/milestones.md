# Milestones

This document groups MSVC LSP's planned features into release milestones. Groupings only — no
timelines. It reflects the current two-artifact architecture (Rust CLI binary + Neovim plugin)
described in [project_spec.md](project_spec.md) and [engineering_spec.md](engineering_spec.md), and
supersedes the original milestone/MVP/v1 lists in [brainstorm.md](brainstorm.md), which predate the
decision to drop the LSP-proxy approach in favor of generating `compile_commands.json` directly.

# MVP — Single Solution, Manual Workflow

Prove the core value proposition end-to-end for the simplest case: one solution, one
configuration/platform, no multi-solution complexity yet.

## Rust CLI Binary

- Basic binary scaffolding with a subcommand-based CLI.
- Parse a `.sln` file and enumerate its member `.vcxproj` projects.
- MSBuild evaluation for VS2019/2022 project schemas: follow `.props`/`.targets` imports, evaluate
  conditions and macros for a given configuration/platform.
- Discover the configuration/platform combinations available in a solution.
- Toolchain/Windows SDK discovery (e.g. via `vswhere`) to resolve system include paths.
- Generate a `compile_commands.json` for a specified configuration/platform.
- Persist parsed state in a JSON state file, keyed on file modification timestamps, to skip
  reparsing when nothing has changed.
- Structured (JSON) output for anything the Neovim plugin needs to consume programmatically.

## Neovim Plugin

- Basic plugin scaffolding and installation instructions.
- Locate a `.sln` file within the project directory/subdirectories (single solution assumed).
- Invoke the CLI binary to list configuration/platform options and present a picker.
- Invoke the CLI binary to generate `compile_commands.json` into `.msvclsp/`.
- Launch `clangd` pointed explicitly at the generated `compile_commands.json`.
- Basic keymaps/commands for generating the database and selecting a configuration/platform.
- Basic user documentation.

# V1 — Multi-Solution & Workflow Polish

Extend the MVP to handle real projects with more than one solution, and make the day-to-day
workflow smoother.

## Rust CLI Binary

- Subcommand to discover `.sln` files under a project root.
- Per-solution state on disk: each discovered solution independently tracks its own last-used
  configuration/platform.
- `compile_commands.json` output namespaced by solution (`.msvclsp/<solution-name>/`).

## Neovim Plugin

- Interactive solution picker when multiple solutions are discovered, with the choice persisted per
  workspace (project root, detected via standard LSP root markers like `.git`).
- Manual rescan command to refresh the cached solution list.
- Explicit "switch solution" command that restores the newly-active solution's own last-used
  configuration/platform.
- Auto-discovery runs inline the first time a solution/config is selected if no cache exists yet.
- `clangd` restart wired into both configuration/platform switches and solution switches.

# V2 — Hardening & Edge Cases

Turn the remaining open questions from project_spec.md/engineering_spec.md into concrete,
implemented behavior once the core workflow is proven out.

## Rust CLI Binary

- Graceful handling of a remembered/active solution whose `.sln` file no longer exists on disk
  (moved or deleted).
- Config-based override to explicitly pin the active solution (e.g. for CI or scripting use),
  bypassing the interactive picker.
- Defined resolution for naming collisions between discovered solutions that share a filename in
  different directories.
- Fixture-based integration test corpus (sample solutions of varying complexity) wired into CI as
  the primary correctness signal for the parsing pipeline.

## Neovim Plugin

- Surface CLI-detected error states (missing/moved solution, naming collisions) in the picker/UI
  rather than failing silently.
- Expanded user documentation covering multi-solution workflows and troubleshooting.

# Future / Unscoped

Ideas raised but not committed to any milestone above:

- Broader Visual Studio version support (VS2015/2017), if needed beyond the current VS2019/2022
  target.
- Automated end-to-end testing that drives a real `clangd` instance against generated output
  (currently manual verification only).
- Support for multiple simultaneously-active solutions in one workspace (the data model is being
  kept solution-scoped from the start to allow this later, but it is not a committed feature).
