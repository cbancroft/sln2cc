# ADR 47 — Inline First-Run Discovery Was Already Delivered

- **Status:** Accepted
- **Date:** 2026-08-16
- **Issue:** [V1-06 — Inline first-run discovery](../todos.md#v1) (`feat/nvim-inline-discovery`)
- **Affects:** `nvim/tests/test_picker.lua`, [engineering_spec.md](../engineering_spec.md)
  ("Neovim Plugin — Responsibilities"), [ADR 33](33-nvim-solution-discovery.md)

## Problem

`docs/todos.md` scoped V1-06 as: "Run discovery automatically when selection is requested and
no cache exists, keeping it one user action instead of two." Read at face value, that
describes a behavior change — some caller today would have to run a `:MsvclspRescan` (or
equivalent) before a picker could show anything, and this issue would collapse that into one
step.

That caller never existed. `discovery.lua`'s `M.solutions(root)` has scanned and populated its
cache lazily, on first call, since MVP-27 (`26ab26e`, "Discover .sln files under the project
root in Neovim") — the module's own header comment has said so, unchanged, since that commit:
"If no cache exists when a solution is first needed, discovery runs inline so selection stays
a single user action." `M.active_solution(root)` calls `M.solutions(root)` on its cold path,
so it inherits the same laziness. `picker.lua`'s `M.pick_solution(root, on_choice)` (V1-03,
ADR 45) calls `discovery.solutions(root)` directly with no precondition that a scan already
ran. `commands.lua`'s `MsvclspGenerate` and `MsvclspSelectConfig` both reach discovery through
`resolve_solution()`, and `MsvclspSelectSolution` reaches it through `M._select_solution()`'s
own call to `picker.pick_solution` — so all three already get inline discovery for free.
(`MsvclspRestart` is the exception: it never calls `discovery.active_solution` at all, only
`discovery.project_root()` to key its own `last_database` cache, so it has no discovery step —
inline or otherwise — to collapse. An earlier draft of this ADR listed it among
`resolve_solution()`'s callers; it never was one. Corrected while researching V2-01, whose own
ADR 48 needed `resolve_solution()`'s real call sites to be accurate.)

This is the same shape V1-04 ("Rescan command") hit: a todo item describing work a prior issue
had already done as a matter of course, not a deliberate stopgap waiting to be finished. It
isn't quite identical, though. V1-04's own Outcome could point at tests that already existed
verbatim. Here, no test exercised the specific combination the issue's own **Tests:** line
named — "discovery runs then the picker appears." `test_picker.lua`'s existing
`pick_solution` specs stub `msvclsp.discovery` wholesale (`fake_discovery`), and
`test_discovery.lua`'s "caches discovered solutions instead of rescanning" test exercises
`discovery.solutions` in isolation, never through `picker.lua`. Nothing proved, end-to-end with
the real (non-stubbed) discovery module, that a fresh `pick_solution` call with no prior scan
actually populates the picker.

## Decision

No `cli/src/**` or `nvim/lua/**` change. Close V1-06 by adding the one test that was genuinely
missing: `nvim/tests/test_picker.lua` gains a `describe("msvclsp.picker.pick_solution — inline
discovery", ...)` block that requires the real `discovery.lua` (not stubbed) against a real
temp tree containing two `.sln` files, with only `vim.ui.select` stubbed per the file's
existing convention, and asserts that `picker.pick_solution` sees both freshly-discovered
paths and hands the chosen one to `on_choice` — with no `discovery.solutions()` or
`discovery.rescan()` call preceding it in the test. That failure mode (an empty list reaching
`vim.ui.select`) is exactly what a caller-must-discover-first design would have produced,
which is what makes the test meaningful despite pinning down behavior that already existed.

## Discussion

**Why MVP-27 already built this inline, rather than it being incidental.** MVP-27's own todo
entry (see `docs/todos.md`) scoped `M.solutions` as a synchronous, in-memory-cached scan —
closer in cost to `discovery.lua`'s own `vim.fs.root()`-based root detection than to a real
subprocess call, per that issue's Outcome. A lazy, scan-on-first-use cache was the natural
shape for that design from the start; "requires an explicit prior scan" was never a design this
codebase went through and later needed to unlearn.

**Why this counts as a distinct issue from V1-04 rather than folding into it.** V1-04 closed
with zero new tests because the exact specs it called for were already in place — a pure
"already done" close. V1-06 needed one genuine addition: an integration-style test proving the
picker relies on `discovery.lua`'s laziness rather than on some caller doing the scan first.
Recording that as this issue's own ADR, rather than silently amending V1-04's, keeps the
todo-to-ADR-to-test chain traceable to the issue that actually motivated the test.

**Why not treat "no code change needed" as reason to skip writing an ADR at all.** CLAUDE.md
requires an ADR when "an approach [is] abandoned" or a design question is settled, not only
when new code lands. The question this issue implicitly raised — is a separate discovery step
ever required before selection? — is now answered on the record ("no, and never was"), which
is exactly the kind of relitigation-prevention an ADR exists for.

**Cost.** None. There was no design trade-off to weigh; the only material change is one new
test file addition.

## Consequences

- No behavior changes for users; no `docs/changelog.md` entry, matching V1-04's own precedent
  of skipping the changelog for a no-op close.
- `nvim/tests/test_picker.lua` now has a real-discovery integration case guarding the "no
  separate discover step" property directly, rather than relying on it being an emergent
  consequence of `test_discovery.lua`'s isolated `M.solutions` test plus code reading.
- `docs/engineering_spec.md`'s discovery bullet gains one clause making the inline/lazy
  behavior explicit rather than only discoverable by reading `discovery.lua`'s header comment.
