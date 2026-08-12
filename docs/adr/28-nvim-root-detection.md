# ADR 28 — Neovim Project Root Detection

- **Status:** Accepted
- **Date:** 2026-08-11
- **Issue:** [MVP-26 — Project root detection](../todos.md#neovim-plugin)
  (`feat/nvim-root-detection`)
- **Affects:** `nvim/lua/msvclsp/discovery.lua`, `nvim/tests/test_discovery.lua`, `README.md`

## Problem

`discovery.lua` was four functions that all `error("not yet implemented")`. Every later
command needs a project root before it can locate `.msvclsp/` or scan for `.sln` files, and
the todos entry specifies the shape: walk upward from the current buffer for a configured root
marker — "the same marker convention Neovim's LSP clients already use" — falling back to the
current working directory when nothing matches, caching per buffer since this runs on every
command, and never throwing on unnamed or non-file buffers.

## Decision

`M.project_root(bufnr?)` resolves and caches a project root per buffer:

1. It calls `vim.fs.root(bufnr, config.options.root_markers)` directly rather than
   reimplementing the upward walk with `vim.fs.find()`. `vim.fs.root()` (Neovim 0.10+) *is* the
   stdlib entry point `nvim-lspconfig` and Neovim's native LSP client use for this — calling it
   is a closer match to "the same convention Neovim's LSP clients use" than re-deriving the same
   walk, and avoids the kind of internal-duplication liability CLAUDE.md §5 already warns
   against for external dependencies.
2. This raises the plugin's Neovim floor from 0.9+ to **0.12+**, not just the 0.10+
   `vim.fs.root()` needs. `README.md` already documents `vim.pack` — Neovim's built-in plugin
   manager, 0.12+ — as an installation path, so a user following that path is on 0.12+ already;
   aligning the stated floor to 0.12 doesn't newly exclude anyone the README already invites,
   and avoids carrying a floor (0.10) the README's own install instructions don't actually
   support.
3. `vim.fs.root()`'s `nil` return (no marker found anywhere up to the filesystem root) becomes
   `vim.fn.getcwd()`. Unnamed/scratch buffers need no special case in `project_root()` itself —
   `vim.fs.root()` already handles `buftype ~= ""` by starting the walk from `uv.cwd()` rather
   than throwing on an empty buffer name, so that path already resolves to something sane before
   `project_root()` ever sees it; the two `M.project_root()`-level fallbacks (no marker found;
   unnamed buffer) collapse into one `or vim.fn.getcwd()` on `vim.fs.root()`'s result.
4. The cache is a plain module-local table keyed by (normalized) `bufnr`, populated lazily. This
   issue adds no eviction policy.

## Discussion

**Hand-rolling the walk with `vim.fs.find()` was the original plan**, staying inside the
existing 0.9+ floor. Rejected on reflection: `vim.fs.root()` is not just "a `vim.fs.find()` call
with `upward = true`" — it also owns the unnamed-buffer special case, so skipping it means
re-deriving behavior the stdlib already got right, for a floor the README's own `vim.pack`
mention doesn't actually promise anyone.

**Raising the floor only to 0.10 (the `vim.fs.root()` minimum) rather than 0.12 was also
considered.** Rejected because `README.md` already tells `vim.pack` users the plugin works
with it, and `vim.pack` itself requires 0.12 — a stated 0.10 floor would be true of the function
call but false of the install path the README already documents. 0.12 is the honest floor for
what the plugin actually supports today.

**A hand-eviction policy for the per-buffer cache (`BufDelete`/`BufFilePost` autocmds) was
considered and deferred, not rejected.** The todos entry asks for caching because this runs on
every command but names no invalidation trigger, and `M.rescan()` (MVP-27) is scoped to the
*solutions* cache, not this one. A stale root cached against a reused `bufnr` self-corrects the
moment that buffer is used for real discovery against a solution, so an eviction policy now
would be speculative — nothing in the codebase yet exercises the case where it would matter.
Left as an open question below rather than silently decided either way.

## Consequences

- Every later discovery/command function can call `M.project_root()` trusting it never throws,
  regardless of buffer state.
- The plugin no longer runs on Neovim 0.9–0.11. Nothing else in the codebase currently depends
  on that range, so the cost is contained to the README's stated floor.
- **Open question:** whether the per-buffer root cache and MVP-27's solutions cache should share
  an invalidation mechanism (e.g. both cleared by the same `:MsvclspRescan`) or stay independent,
  since a root only changes if the user opens a buffer belonging to a different project — an
  event `discovery.lua` has no way to observe today. Whoever implements MVP-27's `M.rescan()`
  should decide this explicitly rather than inherit it by omission.
