# ADR 41 — Forcing Regeneration From Neovim

- **Status:** Accepted
- **Date:** 2026-08-14
- **Issue:** [MVP-46 — Force regeneration from Neovim](../todos.md#neovim-plugin)
  (`feat/nvim-force-regenerate`)
- **Affects:** `nvim/lua/sln2cc/cli.lua`, `nvim/lua/sln2cc/commands.lua`,
  `nvim/lua/sln2cc/picker.lua`, `nvim/tests/test_cli.lua`, `nvim/tests/test_commands.lua`,
  `nvim/doc/sln2cc.txt`, [engineering_spec.md](../engineering_spec.md) ("Neovim Plugin —
  Responsibilities"), [ADR 37](37-nvim-user-commands.md)

## Problem

`generate` has accepted `--force` since MVP-21/22 — bypass the CLI's own mtime-based staleness
cache and regenerate `compile_commands.json` unconditionally. Nothing in the Neovim plugin can
reach it: `cli.lua`'s `M.generate` doesn't accept the flag, and `:Sln2ccGenerate` (ADR 37) always
lets the cache decide. A user who edits a `.vcxproj` in a way the cache's mtime check can't see
coming, or simply doesn't trust it, has no way to ask for a forced rebuild without shelling out to
the CLI binary directly — which defeats the point of the plugin driving the whole workflow.

## Decision

1. **`:Sln2ccGenerate!`** — a `!` bang on the existing command, not a new
   `:Sln2ccRegenerate`. Confirmed with the user over the alternative. `nvim_create_user_command`
   gets `bang = true` for `Sln2ccGenerate` only; the other three commands (`Sln2ccSelectConfig`,
   `Sln2ccRescan`, `Sln2ccRestart`) don't declare it, so Neovim itself refuses a bang on them
   rather than the plugin needing to check.
2. **`cli.lua`'s `M.generate` gains a `force` parameter, positional, before `on_done`.** New
   signature `M.generate(solution, cfg, root, force, on_done)`, appending `--force` to the argv
   only when `force` is truthy.
3. **`picker.lua`'s internal `cli.generate` call passes `force = false` explicitly.**

## Discussion

**Bang, not a new command.** A new `:Sln2ccRegenerate` was the other option on the table. It would
sit in `M.commands` and `nvim/doc/sln2cc.txt`'s command table as a fifth entry that is, in every
respect but one flag, `:Sln2ccGenerate` again — same active-configuration lookup, same
`auto_restart_clangd` gating, same failure handling. A bang is the idiomatic Neovim spelling for
"do the thing, but don't let normal caution stop you" (`:w!`, `:q!`, `:bd!`), so a user who already
knows Neovim can guess `:Sln2ccGenerate!` does something stronger than `:Sln2ccGenerate` without
reading the docs first — which a distinct command name can't offer for free. The cost is that
`M._generate` now takes a parameter (`opts`, to read `.bang`) the other three handlers don't need
and don't get, which is an asymmetry in the handler table rather than a uniform signature; accepted
because giving the other three an unused parameter for symmetry alone is exactly the kind of
speculative surface CLAUDE.md's "don't design for hypothetical future requirements" argues against.

**Why `picker.lua` doesn't get a bang-equivalent of its own.** `pick_config`'s internal `generate`
call evaluates whatever configuration/platform the user just picked. For the MVP's per-solution,
single-namespaced-database assumption (V1-02 hasn't landed multi-database namespacing yet), a fresh
pick and a forced regenerate aren't the same question — nothing about *choosing* a
configuration/platform implies distrust of the cache for it. `force = false`, explicit rather than
relying on a default, so the call site says what it means rather than a reader having to check
`cli.lua`'s signature to learn what an omitted argument would have done.

**Why `force` is positional rather than an options table.** `cli.lua` has no options-table
parameter anywhere today — `list_configs(solution, root, on_done)`, the pre-change
`generate(solution, cfg, root, on_done)` — so a `force` boolean slotted in before `on_done` matches
the file's own established style rather than introducing a second calling convention for one
parameter.

## Consequences

- `cli.lua`'s `M.generate` signature changed for both its call sites
  (`commands.lua`, `picker.lua`) and its one direct test (`nvim/tests/test_cli.lua`) — paid once,
  now, rather than needing a second migration if `force` had been bolted on differently later.
- `nvim/doc/sln2cc.txt`'s `:Sln2ccGenerate` entry documents the bang form directly rather than
  gaining a new command section.
- No CLI (`cli/src/**`) change — `--force` already existed; this issue is entirely plugin-side
  wiring.
