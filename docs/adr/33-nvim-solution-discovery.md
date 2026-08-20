# ADR 33 — Neovim Solution Discovery

- **Status:** Accepted
- **Date:** 2026-08-12
- **Issue:** [MVP-27 — Solution discovery](../todos.md#neovim-plugin)
  (`feat/nvim-solution-discovery`)
- **Affects:** `nvim/lua/sln2cc/discovery.lua`, `nvim/lua/sln2cc/config.lua`,
  `nvim/tests/test_discovery.lua`

## Problem

`discovery.lua` had three functions that all `error("not yet implemented")`: `M.solutions()`,
`M.rescan()`, `M.active_solution()`. The todos entry specifies the shape: walk downward from the
project root collecting `.sln` paths, skip directories that make the walk slow and useless
(`.git`, `node_modules`, build output), bound the walk with a configurable depth limit, cache per
project root, and expose a single invalidation entry point both the MVP's manual rescan command
and V1's future workflows can call. Per `docs/todos.md`, MVP assumes a single solution — the
first result when several are found — and defers the interactive picker to V1
(`docs/milestones.md`).

ADR 28 (project root detection) also left an explicit open question for this issue: whether
`M.rescan()` should also clear `M.project_root()`'s per-buffer root cache, or whether the two
caches should stay independent.

## Decision

1. **The scan is synchronous.** `M.solutions()`/`M.rescan()` walk the filesystem inline via
   `vim.fs.dir()`, the same style `M.project_root()` already uses for `vim.fs.root()`. CLAUDE.md's
   "never block the UI" is stated specifically about CLI invocations — real subprocess latency.
   A directory walk bounded by an ignore list and a depth limit is not that: it's the same kind
   of bounded, synchronous stdlib call `project_root()` already makes.
2. **The two caches stay independent.** `M.rescan()` clears only the solutions cache (keyed by
   project root), not `project_root()`'s per-buffer cache. A cached root only goes stale if the
   user opens a buffer belonging to a different project — an event `discovery.lua` still has no
   way to observe, exactly as ADR 28 described. Tying the two together now would be speculative
   coupling between caches keyed by different things (`bufnr` vs. root) to guard against a case
   nothing in the codebase exercises yet.
3. **Two new config keys**, since CLAUDE.md §4 requires every configurable value to carry a
   default in `config.lua` rather than a magic value inline:
   - `scan_ignore_dirs` — directory names skipped during the walk (matched by name, not path, so
     e.g. `bin` is skipped everywhere it appears, not only at the root). Default covers VCS
     metadata, the plugin's own output directory, dependency trees (`node_modules`, `packages`),
     and common MSBuild output directory names (`bin`, `obj`, `Debug`, `Release`, `x64`, `x86`,
     `Win32`).
   - `scan_max_depth` — directory levels descended below the project root, default `6`. Bounds
     worst-case scan cost on an unexpectedly deep or large tree.
4. **`M.solutions(root?)`** defaults `root` to `M.project_root()` and returns a plain,
   ascending-sorted `string[]` of absolute `.sln` paths. Sorting makes "first result" — what
   `M.active_solution()` returns — deterministic regardless of the underlying directory listing
   order, which `vim.fs.dir()` does not itself guarantee.
5. **`M.active_solution(root?)`** is a thin memoized wrapper around `M.solutions(root)[1]`,
   cleared by `M.rescan()` alongside the solutions cache since it derives from the same scan. It
   has no setter: choosing a solution other than "the first one found" is V1's interactive
   picker's job (`picker.lua`'s already-stubbed `M.pick_solution`), not this issue's. It returns
   `nil` when no `.sln` is found, mirroring `M.solutions()` returning `{}`.

## Discussion

**An async, callback-based scan (matching `cli.lua`'s `fun(result, err)` convention) was
considered**, for API consistency with the CLI-invocation half of the plugin. Rejected: the scan
is bounded by design (ignore list + depth limit), so it's closer in cost and shape to
`project_root()`'s existing synchronous `vim.fs.root()` call than to a real subprocess
invocation. A callback-based API would also make the test suite meaningfully heavier — driving
the event loop or faking `vim.loop` — for a scan that's fast in practice. If a real project ever
makes this walk slow enough to matter, that is a reason to revisit, not a reason to design for it
speculatively now.

**Sharing invalidation between the root cache and the solutions cache was the more conservative
option** on paper — clearing more on rescan can't hide a stale value the other way can. Rejected
because it conflates two caches keyed by different things for a scenario that has never been
observed: nothing today changes which buffer is active *and* expects a rescan to fix a stale
root in the same stroke. ADR 28 named this explicitly as a decision for whoever implemented
MVP-27 to make rather than inherit by omission; this is that decision, made explicitly rather
than left open a second time.

**A setter on `M.active_solution()` was considered** so a future picker could just call
`discovery.set_active_solution(root, path)` instead of `picker.lua` reaching into a cache table
directly. Deferred rather than built now: the picker doesn't exist yet, MVP-27's own scope is
single-solution "take the first," and guessing the setter's shape ahead of the V1 picker's actual
needs risks getting it wrong in a way that has to be undone. `picker.lua`'s `M.pick_solution` is
already stubbed as the seam where that decision belongs.

**`cli.lua`'s existing `M.discover(root, on_done)` stub was not built out or removed here.** It
predates ADR 03's decision that the CLI's `discover` subcommand is deliberately absent for MVP —
"the MVP plugin scans in Lua (MVP-27)." Nothing in this change calls it, and it is now dead code
disagreeing with the settled design; whoever implements MVP-28 (`cli.lua`'s real
`M.run`/`M.list_configs`/`M.generate`) should delete it rather than fill it in.

## Consequences

- ADR 28's open question is closed: the per-buffer root cache and the solutions cache are, and
  stay, independently invalidated.
- `docs/engineering_spec.md`'s "Neovim Plugin — Responsibilities" section, which still described
  solution discovery as "invoke the CLI to find `.sln` files under the project root," is corrected
  to match ADR 03 and this decision: the plugin scans in Lua.
- `cli.lua`'s `M.discover` stub is flagged as dead code for MVP-28 to remove; nothing depends on
  it.
- `M.solutions()`/`M.active_solution()`/`M.rescan()` are safe for later modules
  (`picker.lua`, `commands.lua`) to call trusting they never throw and never block, once the
  functions are implemented against this design.
