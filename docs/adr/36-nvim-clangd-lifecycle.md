# ADR 36 — Neovim clangd Lifecycle

- **Status:** Accepted
- **Date:** 2026-08-13
- **Issue:** [MVP-30 — clangd lifecycle](../todos.md#neovim-plugin) (`feat/nvim-clangd-lifecycle`)
- **Affects:** `nvim/lua/sln2cc/lsp.lua`, `nvim/lua/sln2cc/config.lua`,
  `nvim/tests/test_lsp.lua`, [engineering_spec.md](../engineering_spec.md) ("Neovim Plugin —
  Responsibilities"), [ADR 34](34-nvim-cli-invocation.md), [ADR 35](35-nvim-config-picker.md)

## Problem

`lsp.lua` had `M.compile_commands_dir`, `M.start`, and `M.restart` all stubbed with `error("not
yet implemented")`. The issue's description is concrete about intent — launch clangd against an
explicit `--compile-commands-dir` rather than its own directory-walk discovery, restart cleanly
after regeneration or a configuration switch (stopping the existing client and reattaching open
buffers), and warn instead of starting a client when no database exists yet — but not about
mechanism.

A first pass at this plan designed a private client: `vim.lsp.start()` under a made-up name
(`sln2cc_clangd`), with `lsp.lua` hand-tracking which buffers were attached so `M.restart` could
reattach them. That was rejected in review: Neovim 0.11+ already ships a mechanism for exactly
this — `vim.lsp.config(name, cfg)` overrides a named server's launch config, and `vim.lsp.enable
(name)` installs a `FileType` autocmd that starts and attaches that config's client to any
matching buffer, present or future (`$VIMRUNTIME/lua/vim/lsp.lua`, confirmed against the Neovim
0.12.4 shipped on the development machine — see Discussion). Hand-rolling a parallel client under
a private name means clangd stops "usually starting when opening a cpp file" the way every other
LSP-configured buffer in Neovim already works, which directly contradicts the project's own
framing in CLAUDE.md §1 ("points `clangd` at the result").

Two forks inside the redesign still needed a decision:

1. Overriding the `"clangd"` config could either fully replace whatever was there, or merge in
   only the fields this project controls (`cmd`, `root_dir`, `filetypes`), preserving anything a
   user or another plugin (nvim-lspconfig, mason-lspconfig) already set for `clangd` —
   `on_attach`, `capabilities`, `settings`.
2. Auto-activation (`vim.lsp.enable('clangd')`) could be left to the user's own config, or called
   by `lsp.lua` itself.

A third problem surfaced after the first version of this ADR was written and reviewed: editing the
single shared `"clangd"` config name applies to *every* buffer matching `filetypes`, not just
buffers inside this solution. With a static `root_dir = root`, opening a second MSVC solution, an
unrelated CMake project, or a stray `.c` file in the same Neovim session would all get pointed at
this solution's `--compile-commands-dir` — wrong, and silently so.

## Decision

1. **Built on `vim.lsp.config`/`vim.lsp.enable`, not a private `vim.lsp.start()` client.**
   `M.start(database, root)` and `M.restart(database, root)` both route through a shared private
   `activate(database, root)` that merges `{ cmd, root_dir, filetypes }` into the current
   `vim.lsp.config.clangd` and calls `vim.lsp.enable("clangd")`.

2. **Merge, not replace, when overriding the `clangd` config.** `activate` reads the current
   resolved config (`M._get_config("clangd")`), overwrites only `cmd`, `root_dir`, and
   `filetypes`, and writes the merged table back (`M._set_config`). Confirmed with the user rather
   than assumed — a full replace was the simpler option but silently drops any `on_attach`/
   `capabilities`/`settings` a user or another plugin configured for `clangd` outside this plugin.

3. **`lsp.lua` calls `vim.lsp.enable("clangd")` itself**, in both `M.start` and (after stopping
   existing clients) `M.restart`. Confirmed with the user — the alternative, requiring the user to
   call `vim.lsp.enable('clangd')` in their own config first, would mean a user could install
   sln2cc, pick a configuration, and still see nothing happen on opening a `.cpp` file, directly
   contradicting CLAUDE.md §1's framing of the project as closing the gap end-to-end.

4. **`root_dir` is a function scoped to `root`, not a static string.** The merged config's
   `root_dir` is `function(bufnr, on_dir)`, which calls `on_dir(root)` only when the buffer's file
   is at or under `root` (a normalized-path prefix check against `vim.api.nvim_buf_get_name`), and
   does nothing otherwise. Neovim only starts a client for a buffer when this function calls
   `on_dir` (`lsp_enable_callback` in `$VIMRUNTIME/lua/vim/lsp.lua`), so a buffer outside `root` is
   left entirely alone by this override — it falls through to whatever *other* `clangd`
   config/client already applies to it, if any.

5. **`M.restart` stops only running `"clangd"`-named clients whose own `root_dir` equals
   `root`** — `M._get_clients({ name = "clangd" })`, filtered by `client.root_dir == root` (the
   client's resolved root, a plain string set from the function's `on_dir` callback at start time,
   per `client.lua`), each stopped via `client:stop()`, before calling `activate`. This is the
   client-side half of point 4: without the filter, restarting this solution's clangd would stop
   every `"clangd"` client in the session regardless of which project it belonged to.
   `vim.lsp.enable`'s `FileType` re-trigger only *starts* clients for buffers that don't already
   have one whose config still applies — it does not itself detect that a *differently configured*
   client (an old `--compile-commands-dir`) is attached and needs replacing. Stopping the matching
   ones first is what makes the subsequent re-enable start fresh clients, pointed at the new
   database, for every buffer under `root` that was attached to the old one.

6. **`M.compile_commands_dir(database)` stays a small pure function**, `vim.fs.dirname(database)`
   — independently testable without stubbing anything LSP-related, and the one piece of this
   module with no side effects at all.

7. **Two new `config.lua` defaults**: `clangd_cmd = { "clangd" }` (a list, so users can prepend
   their own flags — `--compile-commands-dir=...` is always appended last) and `filetypes = { "c",
   "cpp" }` (no Objective-C/CUDA — CLAUDE.md §3 scopes the whole project to MSVC/Windows, which
   produces neither).

8. **The LSP config name is the literal `"clangd"`, not configurable.** It is the name every
   consumer in the ecosystem already uses (Neovim's own documentation examples, nvim-lspconfig,
   mason-lspconfig); a `clangd_lsp_name` option would be designing for a rename nobody has asked
   for, which CLAUDE.md's "don't design for hypothetical future requirements" argues against
   directly.

9. **The under-root check is a case-sensitive, normalized-prefix comparison** — no attempt to fold
   Windows path casing the way `db.rs`'s hashing does (ADR 19). A mismatch's failure mode is
   "clangd doesn't autostart for that buffer," never a wrong compilation database, so the added
   rigor wasn't judged worth the complexity.

10. **Four overridable module fields — `M._get_config`, `M._set_config`, `M._enable`,
    `M._get_clients`** — default to `vim.lsp.config`'s read/write and `vim.lsp.enable`/
    `vim.lsp.get_clients` respectively. Mirrors `cli.lua`'s `M._system` seam (ADR 34, point 2):
    `lsp.lua`'s own tests need to intercept real global LSP state the same way `cli.lua`'s tests
    need to intercept a real spawn — a genuine config/enable call, or a genuine `client:stop()`, is
    a side effect a test must never trigger.

## Discussion

**Why the hand-rolled-client design was rejected after being written up.** It technically
satisfied the issue's Tests: block (a distinguishable start/restart, a specific
`--compile-commands-dir` argument, reattachment, a missing-database notification), but at the
cost of clangd only ever starting through an explicit `M.start`/`M.restart` call — no `FileType`
autocmd, no "opens the file, clangd is there" behavior sln2cc is supposed to preserve. It also
duplicated bookkeeping (`attached_buffers` tracking, dedup-by-config) that
`$VIMRUNTIME/lua/vim/lsp.lua` already does correctly for every other LSP server a user runs. The
private client name (`sln2cc_clangd`) was itself a symptom: it existed only to avoid colliding
with a user's *own* `clangd` config, which is exactly the collision that disappears once this
module edits that same config instead of parking a second one beside it.

**Why merge instead of replace.** A full replace (`vim.lsp.config.clangd = { cmd, root_dir,
filetypes }`) is simpler — one assignment, no read step — and was genuinely on the table. It was
rejected because `vim.lsp.config.clangd`'s read path already resolves anything the *runtime*
supplies for that name (an `lsp/clangd.lua` on the runtimepath from nvim-lspconfig, or the user's
own prior `vim.lsp.config('clangd', ...)` call) — replacing wholesale would discard `on_attach`,
`capabilities`, or `settings` a user configured for entirely unrelated reasons (an
organization-wide `on_attach` that sets keymaps, say), the first time they picked a configuration
in an MSVC project. The cost of merging is one extra read and building the merged table by hand
instead of a literal; both are cheap next to silently breaking someone else's config.

**Why `lsp.lua` calls `vim.lsp.enable` itself instead of leaving it to the user.** The tempting
alternative treats "enabling clangd" as configuration the user owns, the same way they'd choose
`lua_ls` for a separate Lua project — sln2cc would only ever adjust `vim.lsp.config`, never
`vim.lsp.enable`. It was rejected because the project's whole premise (`CLAUDE.md` §1) is that
users on an MSVC codebase currently have *no* working clangd setup to enable in the first place;
requiring one extra manual step undercuts "closes that gap" for the exact audience this plugin is
for. The cost is that `lsp.lua` now has a real, repeatable side effect on global LSP state
(`lsp._enabled_configs`) rather than a pure config edit — accepted, since `vim.lsp.enable` is
documented as idempotent to call repeatedly and every call this module makes is gated on having a
real, just-generated `database` path.

**Why stopping existing clients happens in `lsp.lua`, not left to `vim.lsp.enable`'s own
re-trigger.** Reading `lsp_enable_callback` in `$VIMRUNTIME/lua/vim/lsp.lua`: its "stop clients
that no longer apply" loop only detaches a client when the *buffer* no longer matches
(`can_start` returns false — wrong filetype, invalid `cmd`), not when the *config itself* changed
underneath an otherwise-still-valid client. Relying on that loop alone would leave the old,
stale-database clangd process running and attached indefinitely after every regenerate. Explicitly
stopping the matching `"clangd"`-named clients before re-enabling costs one extra `M._get_clients`
call and a loop, and is the only way to guarantee the client a buffer ends up attached to is the
one just configured.

**Why `root_dir` needed to become a function at all — the problem this fixes.** The design as
first written down used a static `root_dir = root`. That was caught in review, not designed
around from the start: editing the single shared `"clangd"` config name means the override applies
to *every* buffer Neovim considers for that config, not just buffers inside this solution. With a
static string, opening a second solution, an unrelated CMake project, or a stray file elsewhere on
disk in the same session would silently point all of them at this solution's
`--compile-commands-dir` — wrong flags for code this plugin has nothing to do with, and no error
to notice it by. `vim.lsp.enable`'s own documentation shows the fix: `root_dir` accepts
`function(bufnr, on_dir)`, and a config only starts for a buffer if that function calls `on_dir`.
Gating on a normalized-path check against `root` is what makes "override clangd for this solution"
actually mean *this solution*, not "every C/C++ buffer this Neovim instance ever opens."

**Why `M.restart`'s client filter had to change alongside it.** Once `root_dir` stopped being a
single fixed string, "every client named `clangd`" stopped being a safe thing to stop — a second,
entirely unrelated project's clangd client shares the same config *name* even though its resolved
`root_dir` differs. `client.root_dir` (set from the function's `on_dir` argument at start time, per
`client.lua`) is the value to filter on instead: it identifies which solution a *running* client
belongs to the same way the `root_dir` function decides which solution a *not-yet-started* buffer
belongs to. Skipping this filter would have reintroduced, on the stop side, exactly the
cross-project bleed the `root_dir` function fixed on the start side.

**Why the module fields default to the real `vim.lsp` functions rather than tests monkey-patching
`vim.lsp` directly**, the way `test_picker.lua` reassigns the real `vim.ui.select`. `vim.ui.select`
is designed to be swapped — that is the entire point of the API, and third-party picker frontends
do it routinely. `vim.lsp.config`/`vim.lsp.enable`/`vim.lsp.get_clients` are not: they mutate
process-wide LSP state (`lsp._enabled_configs`, the resolved-config cache) that other, unrelated
plugins or the editor's own future test cases could observe. A module-level seam keeps that
mutation opt-in and local to `lsp.lua`'s own tests, matching `cli.lua`'s existing precedent for
the same underlying reason (ADR 34).

## Consequences

- `M.start`/`M.restart`'s only global side effects are `vim.lsp.config.clangd` and
  `vim.lsp.enable("clangd")` — no client bookkeeping lives in `lsp.lua` itself, and any future
  change to how Neovim resolves/starts LSP configs is inherited for free rather than needing a
  parallel update here.
- A user who already configures `clangd` themselves (directly, or via nvim-lspconfig/
  mason-lspconfig) keeps their `on_attach`/`capabilities`/`settings` after sln2cc starts
  overriding `cmd`/`root_dir`/`filetypes` for the same config name.
- A second MSVC solution, an unrelated CMake project, or any other buffer outside the active
  solution's `root` is untouched by this override — `M.start`/`M.restart` neither starts nor stops
  anything for it, even though every one of them may share the literal config name `"clangd"`.
- **MVP-31 (`commands.lua`) still owns deciding *when* to call `M.restart`.** Neither
  `GenerateResult.regenerated` nor `config.options.auto_restart_clangd` (already present in
  `config.lua`, unconsumed until now) is read by this module — consistent with ADR 35's own
  Consequences, which already flagged that `pick_config`'s `on_choice` hands back only the picked
  config, not `GenerateResult.database`, so whoever wires `lsp.lua` in next has that gap to close
  regardless of this issue.
- No changelog entry: no user command or autocmd calls `M.start`/`M.restart` yet, mirroring
  MVP-28/29's own Outcome.
