# ADR 45 — Neovim Solution Picker and Its Persistence

- **Status:** Accepted
- **Date:** 2026-08-15
- **Issue:** [V1-03 — Solution picker with persistence](../todos.md#v1)
  (`feat/nvim-solution-picker`)
- **Affects:** `nvim/lua/sln2cc/discovery.lua`, `nvim/lua/sln2cc/picker.lua`,
  `nvim/lua/sln2cc/commands.lua`, `nvim/tests/test_discovery.lua`,
  `nvim/tests/test_picker.lua`, `nvim/tests/test_commands.lua`,
  [engineering_spec.md](../engineering_spec.md) ("Neovim Plugin — Responsibilities", "Storage
  Layout"), [ADR 33](33-nvim-solution-discovery.md), [ADR 35](35-nvim-config-picker.md),
  [ADR 37](37-nvim-user-commands.md), [ADR 44](44-drop-cli-side-discovery.md)

## Problem

Three stubs, deferred across three prior issues, all converge on this one:

1. ADR 33 (MVP-27) left `discovery.lua`'s `M.active_solution()` with no setter: "choosing a
   different one than 'the first one found' is V1's picker's job... `picker.lua`'s
   already-stubbed `M.pick_solution` is already stubbed as the seam where that decision belongs."
2. `picker.lua`'s `M.pick_solution(on_choice)` itself has always been `error("not yet
   implemented")`.
3. ADR 37 (MVP-31) deliberately did not register `Sln2ccSelectSolution` — "V1-03 adds it back
   alongside `picker.pick_solution`" — and named a gap only this issue could close: "a new active
   solution under the same root invalidates the [per-root] cached database" that
   `Sln2ccRestart` reads.

`docs/todos.md`'s one-line description ("remember the choice per project root so later sessions
don't re-prompt") doesn't by itself say *where* that memory lives. Every other piece of sln2cc
state so far is CLI-owned (`db.rs`'s `state.json`), but ADR 44 already ruled that out here:
"finding `.sln` files, choosing among them, remembering a choice" is `discovery.lua`'s and this
issue's territory, never the CLI's — the CLI is only ever handed a solution path, never asked to
find or remember one.

## Decision

1. **A second, plugin-owned file, colocated with the CLI's own state.**
   `<root>/<config.options.data_dir>/nvim-solution.json` (default
   `<root>/.sln2cc/nvim-solution.json`), holding `{"solution": "<absolute path>"}`. It sits
   beside `db.rs`'s `state.json` inside the same per-root `.sln2cc` directory CLAUDE.md's Project
   Goals section already designates for "generated artifacts," under a name that can't collide
   with the CLI's own private `STATE_FILE` constant. No root-to-file keying scheme is needed
   (unlike `state.json`'s root-relative solution keys) because `.sln2cc` is already one directory
   per project root.

2. **`discovery.lua` gains `M.set_active_solution(root, solution)`.** It updates the in-memory
   `active_cache` and writes the persisted file. It is the only setter; `picker.lua`'s
   `M.pick_solution` is its only caller. `M.active_solution()`'s cold path (nothing in
   `active_cache`) now reads the persisted file first, falling back to "first found" only when
   nothing was ever recorded.

3. **A corrupt or missing persisted file degrades silently to "nothing remembered."** Unlike
   `db.rs`'s `Store::open`, which hard-fails on a malformed `state.json` because that file feeds a
   correctness-affecting staleness check, `nvim-solution.json` is pure convenience: a bad read just
   means one extra prompt, not a wrong `compile_commands.json`. A *write* failure still gets a
   `vim.notify` (WARN) — a user who explicitly picked a solution should know if that choice won't
   survive a restart — but the pick itself still succeeds for the rest of the session.

4. **`M.rescan()` is unchanged.** It already clears `active_cache`. Because `active_solution()`'s
   cold path now checks the persisted file before falling back to "first found," a rescan
   re-derives the same persisted answer rather than reverting to "first found" — no extra
   invalidation logic needed.

5. **`picker.pick_solution(root, on_choice)` depends on `discovery.lua`.** ADR 35 deliberately kept
   `pick_config`'s dependency graph to `cli.lua` alone; that precedent doesn't extend here, because
   choosing a *solution* — as opposed to a configuration/platform — is inherently a scan-and-select
   operation only `discovery.lua` can do locally. `pick_solution` calls `discovery.solutions(root)`
   for the candidate list, marks the currently active one in `format_item` (mirroring
   `pick_config`'s `"  (active)"` marker for UI consistency between the two pickers), and on
   selection calls `discovery.set_active_solution(root, chosen)` before invoking
   `on_choice(chosen)`. Cancellation and an empty solution list both converge on `on_choice(nil)`,
   mirroring `pick_config`'s single-branch contract (ADR 35 point 2) — no `pick_config`, no
   `generate`, and no clangd restart happen from this picker at all.

6. **`commands.lua`'s `Sln2ccSelectSolution` only picks and invalidates, it never regenerates.**
   The handler is `picker.pick_solution(root, function(solution) if solution then
   last_database[root] = nil end end)`. Restoring the newly chosen solution's own remembered
   configuration/platform, regenerating `compile_commands.json`, and restarting clangd is V1-05's
   ("Switch-solution command") explicit, separately-scoped job — this issue only changes *which*
   solution is active and makes sure a stale cached database path from the old solution can't leak
   into a later `Sln2ccRestart`.

## Discussion

**Why not `stdpath("data")` instead of `.sln2cc`.** A single global file keyed by project root
(the way some Neovim plugins persist cross-session state) was considered. Rejected: sln2cc
already has a per-project-root artifacts directory doing exactly this job for every other piece of
state, and a second storage location would mean two different answers to "where does sln2cc keep
its state" for no benefit — `.sln2cc` is already gitignored, already created on demand, and
already scoped correctly per root with zero extra keying logic.

**Why JSON instead of a bare path string.** A one-line text file would work and is simpler to
write. JSON was chosen to match `db.rs`'s own "always JSON" persistence convention throughout the
codebase, and leaves a natural, non-breaking extension point (e.g. a timestamp, if V2-01's
"vanished solution" handling ever wants one) without forcing a file-format migration. No such field
is added speculatively now — CLAUDE.md's "don't design for hypothetical future requirements"
still applies; the shape just doesn't foreclose the option the way a bare string would.

**Why a corrupt persisted file isn't a loud error, unlike `state.json`.** `db.rs` hard-fails on a
malformed `state.json` because that file's correctness gates whether `compile_commands.json`
itself is regenerated correctly — a silently-wrong read there could produce a subtly wrong
database, which CLAUDE.md's "never silently guess" specifically warns against. `nvim-solution.json`
gates nothing but which solution is *offered first* — the worst outcome of a bad read is one extra
`vim.ui.select` prompt, which is a strictly better failure mode than refusing to load the plugin
over a one-line convenience cache.

**Why `pick_solution` doesn't regenerate or restart clangd, unlike `pick_config`.** `pick_config`
(ADR 35) triggers `cli.generate` on selection because a configuration/platform pick is meaningless
without an up-to-date database for it. A *solution* pick is different: the newly active solution
has its own, independently-remembered configuration/platform (`db.rs`'s per-solution
`active_config`), which this issue has no way to know without an extra `list-configs` round trip —
and even if it did, silently regenerating against whatever configuration happens to be on file
conflates "I switched solutions" with "I want this specific configuration built now." V1-05 owns
deciding how that restoration should actually work; folding it in here would be guessing at that
issue's design instead of leaving the seam ADR 37 already pointed at.

## Consequences

- `discovery.lua`'s `M.active_solution()` and `M.set_active_solution()` are now the single source
  of truth for which solution is active, backed by disk — `commands.lua` and any future caller can
  trust a value read after a Neovim restart without re-scanning or re-prompting.
- `Sln2ccSelectSolution` exists and is registered, closing the five-command surface ADR 37's
  `M.commands` table originally described minus one.
- `commands.lua`'s `last_database` cache is now correctly invalidated on a solution switch; a
  stale database path from a previous solution can no longer leak into `Sln2ccRestart`.
- V1-05 inherits a concrete starting point: `picker.pick_solution`'s `on_choice` already hands back
  the newly active solution path, so V1-05's own handler can call `discovery.active_solution` or
  read that value directly, then look up *that* solution's own remembered configuration/platform
  via `cli.list_configs`, regenerate, and restart — none of which this issue attempts.
- Changelog entry added: `:Sln2ccSelectSolution` is the first user-facing way to change which
  solution sln2cc targets, and the choice now survives a restart.
