# ADR 46 — Restoring Configuration on Solution Switch

- **Status:** Accepted
- **Date:** 2026-08-16
- **Issue:** [V1-05 — Switch-solution command](../todos.md#v1) (`feat/nvim-switch-solution`)
- **Affects:** `nvim/lua/msvclsp/commands.lua`, `nvim/tests/test_commands.lua`,
  [engineering_spec.md](../engineering_spec.md) ("Neovim Plugin — Responsibilities"),
  [ADR 45](45-nvim-solution-picker.md)

## Problem

ADR 45 (V1-03) built `:MsvclspSelectSolution` up to the point of picking a different `.sln`
and persisting the choice, but stopped deliberately short of the rest: "restoring the newly
active solution's own remembered configuration/platform, regenerating
`compile_commands.json`, and restarting clangd is V1-05's ... explicit, separately-scoped
job." Left as V1-03 shipped it, picking a new solution changes which `.sln` is active but
leaves clangd pointed at whatever database (or none) the *previous* solution last generated
— a silent mismatch a user has no reason to expect.

The reason V1-03 couldn't just do this itself: each solution has its own independently
remembered configuration/platform (`db.rs`'s per-solution `active_config`, surfaced through
`cli.list_configs`'s `result.active`), and that value isn't knowable until a specific solution
has actually been chosen. Guessing — reusing whichever configuration happened to be active for
the solution being switched away from — is exactly the kind of silently-wrong behavior
CLAUDE.md's "never silently guess" rules out; a subtly wrong `compile_commands.json` is worse
than an extra prompt.

## Decision

1. **`M._select_solution` calls the same helper `M._generate` calls.** `commands.lua` gained
   `generate_and_restart(solution, root, force)`: `cli.list_configs(solution, root, ...)` for
   `solution`, a check of `result.active`, and on a hit `cli.generate(solution, result.active,
   root, force, ...)` followed by the existing `maybe_restart` helper. `M._generate` calls it
   with `force` taken from `opts.bang`; `M._select_solution` calls it with `force` hardcoded to
   `false` (Decision point 2). After `picker.pick_solution` hands back a newly chosen solution
   and the stale `last_database[root]` entry is cleared (unchanged from V1-03), that's the whole
   handler. No new module and no new persistence: `cli.lua`, `discovery.lua`, `picker.lua`, and
   `lsp.lua` were already everything this needed.

2. **The regenerate is unforced.** `picker.pick_config` (ADR 35) forces its own regenerate
   because switching *configuration* for the *same* solution can touch zero files, and the
   CLI's mtime-based staleness check would otherwise silently reuse the old database. Switching
   *solution* is different: the set of files in play changes with the solution itself, so the
   CLI's own staleness check is meaningful here, and forcing unconditionally would mean an
   unnecessary regeneration on every switch back to an already-up-to-date solution.

3. **No active configuration for the new solution warns by name and stops.** Same branch shape
   as `M._generate`'s existing "no configuration selected yet" warning, but naming the specific
   solution rather than assuming it's the one the user was just looking at — after a switch, it
   usually isn't.

## Discussion

**Why not fold this into V1-03.** Considered and rejected there already (ADR 45's own
Discussion): the newly active solution's `active_config` isn't known without an extra
`list-configs` round trip specific to that solution, and doing it speculatively inside the
picker would conflate "I switched solutions" with "I want this particular configuration built
right now." Keeping the pick (V1-03) and the restore-and-restart (this issue) as separate,
sequential steps means each has one job.

**Why a shared helper, with `force` as a parameter, rather than two separate call sites.**
The plan for this issue flagged extraction-vs-duplication as a genuine judgment call: the two
call sequences are identical apart from `M._generate`'s `--bang` force (MVP-46), which has no
equivalent user gesture on `:MsvclspSelectSolution`. An early version threaded that
difference in by having `M._generate` fire a second, separate `cli.generate` call ahead of an
always-unforced `generate_and_restart` — that shape was wrong on two counts: it issued a
wasted extra CLI invocation with no configuration to generate against (crashing in
`cli.lua`'s `M.generate`, which indexes straight into `cfg.configuration`), and even when it
didn't crash, the *actual* database-producing call was still the unforced one, so `--bang`
forced nothing. The fix was to make `force` a third parameter on `generate_and_restart`
itself: `M._generate` passes `opts.bang` through, `M._select_solution` always passes `false`.
One `cli.generate` call per invocation, real config every time, and the force/no-force split
lives where it belongs — in the caller, not in a parallel call path.

**Why unforced, unlike `pick_config`.** Spelled out in Decision point 2. The alternative
(always force) was rejected because it makes every solution switch pay for a full MSBuild
re-evaluation even when nothing about that solution changed since it was last generated —
wasted work with no correctness benefit, since the CLI's staleness check already does the
right thing when the *files* haven't changed.

## Consequences

- `:MsvclspSelectSolution` closes the gap ADR 45 named: switching solutions now restores that
  solution's own remembered configuration/platform, regenerates if needed, and restarts clangd
  (subject to `config.options.auto_restart_clangd`, exactly like `:MsvclspGenerate` and
  `:MsvclspSelectConfig`).
- A solution with no configuration ever selected still requires an explicit
  `:MsvclspSelectConfig` after switching to it — this issue does not invent a "pick a
  configuration for me" fallback, matching `:MsvclspGenerate`'s existing behavior for the
  active solution.
- `nvim/tests/test_commands.lua`'s `fake_cli` gained `list_configs_by_solution`, letting a test
  assert that two different solutions get two different generated configurations — the concrete
  proof that no global value leaks across a switch.
- Changelog entry added: switching the active solution now restores and regenerates that
  solution's own configuration and restarts clangd, instead of leaving clangd pointed at the
  previous solution's database.
