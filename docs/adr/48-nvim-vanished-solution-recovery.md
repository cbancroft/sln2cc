# ADR 48 — Recovering From a Vanished Active Solution

- **Status:** Accepted
- **Date:** 2026-08-16
- **Issue:** [V2-01 — Handle a vanished active solution](../todos.md#v2) (`fix/missing-solution`)
- **Affects:** `nvim/lua/msvclsp/discovery.lua`, `nvim/lua/msvclsp/commands.lua`,
  `nvim/tests/test_discovery.lua`, `nvim/tests/test_commands.lua`,
  [project_spec.md](../project_spec.md) ("Multiple Solution Handling", "Open Questions"),
  [engineering_spec.md](../engineering_spec.md) ("Neovim Plugin — Responsibilities"),
  [ADR 45](45-nvim-solution-picker.md), [ADR 47](47-nvim-inline-discovery.md)

## Problem

`project_spec.md`'s Open Questions list has carried this since the document's early drafts:
"What happens when the remembered/active solution's `.sln` file no longer exists on disk
(moved or deleted)?" Nothing answered it. `discovery.lua`'s `M.active_solution(root)` returns
whatever path is cached in memory or persisted in `nvim-solution.json` without checking it
still exists. `commands.lua`'s `resolve_solution()` hands that path straight to
`cli.list_configs`/`cli.generate`, so a vanished `.sln` surfaces however the CLI's own
filesystem error happens to read — technically "loud" per CLAUDE.md (it does reach
`vim.notify` at `ERROR`), but not "legible": the user gets a raw file-not-found message, not a
path back to a working state. `docs/todos.md`'s own framing for this issue — "prompt for a new
one instead of failing obscurely" — names exactly that gap.

Two sub-questions were forked to the user via `AskUserQuestion` before this ADR was written,
since they produce materially different scope and CLAUDE.md requires raising rather than
silently picking an answer to a named spec Open Question:

1. Should the recovery **notify and point at a command to run manually**, or **auto-invoke the
   picker inline** as part of the failed command? Decided: auto-invoke — matches "prompt for a
   new one" literally, and continuing the original command (generate, select-config) with
   whatever gets picked is strictly more useful than making the user retry by hand.
2. Should vanished-detection be **checked once, when the in-memory cache is cold**, or **fresh
   on every call**? Decided: fresh every call — catches a solution deleted or moved away
   mid-session, not only one already gone before Neovim opened the project. The cost is one
   `fs_stat` per command invocation, in the same category as the `vim.fs.root()` call
   `M.project_root()` already makes on every command.

## Decision

1. **Vanished detection lives in `discovery.lua`, not `commands.lua`.** `M.active_solution`
   already owns "what is the active solution" for both the persisted-choice and
   first-found-fallback cases; teaching it to verify existence keeps that single source of
   truth intact rather than duplicating the check at every call site. A vanished path is the
   same category of problem ADR 45 already named for a corrupt persisted file — "nothing
   downstream depends on it for correctness, only for which solution is offered first" — so the
   fix belongs in the same module, using the same private-helper style (`exists`, alongside the
   existing `read_persisted`/`write_persisted`).

2. **`M.active_solution(root)` gains a second return value, `vanished`** — the previously-active
   path when this call just discovered it's gone, `nil` otherwise. This is deliberately
   *not* a silent fallback to "first found," even though that's exactly what a corrupt
   persisted file already does (ADR 45). The difference: a corrupt file never carried a real
   user choice, so substituting "first found" costs nothing a user would notice as wrong. A
   vanished *persisted* solution represents an explicit pick that's now invalid — silently
   substituting a different, alphabetically-first solution would be exactly the guess
   CLAUDE.md's "never silently guess" rules out. `discovery.lua` reports the fact and cleans up
   its own stale state (the in-memory cache entry, and the persisted `nvim-solution.json` via a
   new private `clear_persisted`, so a second call doesn't keep reporting the same dead path
   forever); the caller decides what "prompt for a new one" means.

3. **Existence is checked with `(vim.uv or vim.loop).fs_stat`, called directly — no `M._stat`
   seam.** `health.lua` exposes `M._stat` because its own tests can't (and shouldn't) touch a
   real filesystem — it's checking a CLI-generated database path that would require running the
   binary to produce for real. `discovery.lua`'s test convention is the opposite: every
   existing spec in `test_discovery.lua` already uses real temp trees with real `.sln` files,
   so there's nothing to stub. A plain private `local function exists(path)` matches the file's
   existing style rather than importing a seam it doesn't need.

4. **The recovery flow — rescan, then reprompt — lives in `commands.lua`'s `resolve_solution`,
   not in `discovery.lua` or `picker.lua`.** Keeps `discovery.lua` a pure fact-reporter (per
   Decision 1) and needs **no `picker.lua` change at all**: on a vanished solution,
   `resolve_solution` calls the already-public `discovery.rescan(root)` — necessary because a
   *moved* solution's new location isn't in the stale cached list a plain re-read would return
   (per ADR 47/V1-06, `M.solutions` only rescans on an explicit `rescan` call, not automatically)
   — and then the already-public `picker.pick_solution(root, on_choice)`, feeding whatever gets
   picked back into the original command's own continuation. `resolve_solution`'s signature
   changes from a synchronous `(solution, root)` return to `resolve_solution(on_resolved)`,
   threading a callback through its two current call sites, `M._generate` and
   `M._select_config`.

   **`M._restart` is unaffected.** It never called `resolve_solution` — it replays
   `last_database[root]` directly, without re-resolving the active solution at all. A vanished
   `.sln` therefore doesn't surface through `:MsvclspRestart`; that command isn't touching
   solution state, just restarting clangd against whatever database path a prior generate
   produced. This surfaced (and fixed, in the same change as this ADR) a factual error in
   **ADR 47**'s own prose (Problem section), which had listed `MsvclspRestart` among
   `resolve_solution`'s callers — it never was. Correcting a factual claim isn't the kind of
   decision reversal CLAUDE.md's "never rewrite" convention protects; that convention is about
   not relitigating a settled decision, not about leaving a landed ADR's prose wrong once it's
   known to be wrong.

5. **"Moved" and "deleted" are mechanically identical, deliberately.** Neither `discovery.lua`
   nor the filesystem APIs in play track file identity (no inode comparison), so there is no
   way to distinguish "this exact file moved to a new path" from "this file was deleted and an
   unrelated file happened to appear elsewhere." Both produce the same detectable symptom (the
   old path no longer exists) and recover the same way (clear the stale state, rescan,
   reprompt) — a genuine move is just the case where the rescan's fresh list happens to contain
   the file's new location. No special-casing was attempted or is possible without adding
   identity tracking this system has no other use for.

## Discussion

**Why not fold detection into `commands.lua` directly, reading `vim.uv.fs_stat` there.** Every
other piece of "what does this solution mean right now" logic — caching, persistence, the
first-found fallback — already lives in `discovery.lua`. Splitting the existence check into
`commands.lua` would mean two modules independently deciding what "active" means, and any
future caller of `M.active_solution` (there's already `picker.lua` and `health.lua`) would need
its own redundant check to get the same guarantee `commands.lua` gets for free here.

**Why a second return value instead of an error or a distinct `M.solution_vanished(root)`
query.** An error is wrong: a vanished solution isn't a failure of `active_solution` doing its
job, it's a legitimate answer ("there isn't a usable one anymore, and here's why"), so raising
would force every caller into `pcall` for an expected condition. A separate query function was
considered and rejected: it would need to run the exact same existence check `active_solution`
already has to run internally to decide what to return, duplicating the `fs_stat` call and
risking the two functions disagreeing about the state of the same file between one call and the
next.

**Why `picker.lua` needed no change at all.** Its existing `pick_solution(root, on_choice)`
contract — present a fresh `discovery.solutions(root)`, persist the choice, hand it back — is
already exactly what a vanished-solution reprompt needs, provided the caller rescans first. This
is the same reuse `ADR 47` found for V1-06: the pieces this issue needed already existed, and
the only genuine gap was orchestrating them from `resolve_solution`.

**Cost.** One `fs_stat` per `resolve_solution` call (every `:MsvclspGenerate` and
`:MsvclspSelectConfig` invocation), which is the same order of cost as the `vim.fs.root()` call
`M.project_root()` already makes on every command — not a new category of expense.
`resolve_solution`'s signature change from synchronous to callback-based is the one real
structural cost, touching both of its call sites; no alternative avoided this once auto-invoking
the picker (Decision from the forked question) was chosen over a notify-only recovery.

## Consequences

- Resolves `project_spec.md`'s Open Question: a vanished active solution now degrades to a
  reprompt through the existing picker, not a raw CLI error or a silent substitution.
- `M._restart` is explicitly out of scope — see Decision 4. A solution vanishing has no effect
  on `:MsvclspRestart`, which never looks at the active solution to begin with.
- `docs/adr/47-nvim-inline-discovery.md`'s prose has been corrected in this same change: it no
  longer claims `MsvclspRestart` called `resolve_solution`.
- Changelog entry added: a vanished active solution (deleted or moved) is now detected and
  prompts for a replacement instead of a raw CLI error.
