# ADR 27 — Neovim Config Validation and Merge Semantics

- **Status:** Accepted
- **Date:** 2026-08-11
- **Issue:** [MVP-25 — Configuration defaults and `setup()`](../todos.md#neovim-plugin)
  (`feat/nvim-config`)
- **Affects:** `nvim/lua/sln2cc/config.lua`, `nvim/tests/test_config.lua`

## Problem

`config.lua`'s `M.setup()` was a one-line `vim.tbl_deep_extend("force", {}, M.defaults, opts or
{})` — no validation, so a typo'd key or a wrong-typed value wasn't rejected at `setup()`; it
surfaced later as a confusing failure in whichever module first read the bad value (`discovery.lua`
indexing a `nil`, `lsp.lua` passing a string where a boolean was expected). CLAUDE.md §4 requires
exactly the opposite: "reject unknown keys with a clear `vim.notify` error rather than failing
mysteriously later." Separately, `vim.tbl_deep_extend` merges nested tables — including Lua arrays,
which are just tables keyed by integer index — key-by-key. Overriding `root_markers` with a
*shorter* list than the default silently keeps the default's trailing entries: `{ ".git", ".hg" }`
overridden with `{ "custom" }` yields `{ "custom", ".hg" }`, not `{ "custom" }`. Neither gap is
visible from the stub's one line; both needed a decision before `setup()` did anything real.

## Decision

`M.setup(opts)` validates `opts` against `M.defaults` before merging, and only mutates `M.options`
on success:

1. **Validation walks `M.defaults`, not a separately maintained schema.** For each key in `opts`, a
   key absent from the corresponding `M.defaults` subtree is "unknown"; a key present with a
   different `type()` is "wrong-typed." `M.defaults` stays the single place a configurable value is
   declared, per CLAUDE.md §4 — a hand-written schema table would just be a second copy of the same
   key list, free to drift from the first.
2. **List-valued options (`vim.islist()` true) replace wholesale; they are never index-merged.** A
   list-valued option is checked for being a list at all, with each element type-checked against the
   default list's element type, then substituted as a whole rather than merged position-by-position.
   Dict-shaped nested tables — none exist in `M.defaults` today, but the merge function stays general
   — still merge key-by-key, since index-merge's failure mode is specific to integer-keyed tables.
3. **A validation failure rejects the whole call, atomically, without mutating anything.** Every
   problem found (not just the first) is collected and reported in one `vim.notify(..., ERROR)` call;
   `M.options` is left exactly as it was before the call; `M.setup()` returns `nil`. The merge itself
   always builds a fresh table via `vim.deepcopy` rather than writing into `M.defaults` or the
   previous `M.options`, so repeated `setup()` calls — valid or not — never leak state into each
   other or into the defaults.
4. **`M.options` is a deep copy of `M.defaults` from module load**, not `{}`, so any module that reads
   it before `setup()` runs (or after a call that got rejected) sees usable defaults instead of
   indexing into an empty table.

## Discussion

**The array index-merge bug is `vim.tbl_deep_extend`'s documented behavior, not a bug in it** — Lua
has no separate array type, so a generic deep-merge over tables has no way to distinguish "shrink
this list" from "leave the rest of this dict alone." Any config surface with a list-valued option and
`vim.tbl_deep_extend("force", ...)` inherits this unless it special-cases lists, which is what the
merge function here does.

**Partial-apply-and-warn (drop only the bad keys, keep the rest) was considered and rejected.**
It's superficially more forgiving, but it means a config with one typo'd key silently loses that
one setting while the rest quietly works — exactly the "failing mysteriously later" CLAUDE.md §4
calls out, just moved one level up: the user sees a notification but the plugin still starts with an
unintended default, and nothing forces them to notice which key got dropped. Atomic rejection makes
`setup()` an all-or-nothing gate: either the whole config is trustworthy, or nothing changed and the
notification says exactly why.

**A hand-authored schema table (key → type/validator) was considered.** It would allow richer
per-key validation (e.g. non-empty strings, path existence) than "matches the default's `type()`."
Rejected for the MVP: every current option validates fully by shape-of-default alone, and a second
table listing the same keys as `M.defaults` is a maintenance liability CLAUDE.md §5 already warns
against for dependencies — the same reasoning applies to internal duplication. Revisit if a future
option needs validation `type()` can't express.

**Recursion into dict-shaped nested tables is speculative** — `M.defaults` has none today. Kept
general anyway because the alternative (a flat-only validator that would need rewriting the day a
nested table option arrives) costs nothing extra now and avoids a second migration later.

## Consequences

- Every module from MVP-26 onward (`discovery.lua`, `lsp.lua`, `health.lua`, …) can read
  `M.options` and trust it is always fully-defaulted and never the result of a partially-applied
  bad config — no defensive `nil`-checking needed at the read site for a config key that exists in
  `M.defaults`.
- "Add to the defaults" semantics (e.g. "also treat `.hg` as a root marker, in addition to `.git`")
  is not expressible through `root_markers` itself — a user who wants that must spell out the full
  list. If additive semantics are ever wanted, that's a distinct option (e.g.
  `extra_root_markers`), not a change to how `root_markers` merges — allowing accumulation there
  would reintroduce the aliasing/leakage problem atomic, defaults-relative merging rules out.
- `vim.notify` is the only error-reporting surface for bad config; there's no `:checkhealth`
  cross-check for config validity beyond what `setup()` itself already caught at call time
  (`health.lua`, MVP-25's neighbor, is unimplemented as of this ADR).
