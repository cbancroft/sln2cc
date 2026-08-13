# ADR 35 — Neovim Configuration/Platform Picker

- **Status:** Accepted
- **Date:** 2026-08-13
- **Issue:** [MVP-29 — Configuration/platform picker](../todos.md#neovim-plugin)
  (`feat/nvim-config-picker`)
- **Affects:** `nvim/lua/msvclsp/picker.lua`, `nvim/tests/test_picker.lua`,
  [engineering_spec.md](../engineering_spec.md) ("Neovim Plugin — Responsibilities"),
  [ADR 34](34-nvim-cli-invocation.md)

## Problem

`picker.lua` had `M.pick_config(solution, on_choice)` stubbed with `error("not yet
implemented")`. The issue's description is concrete about the shape — call `list-configs`,
present the results via `vim.ui.select` with the active pair marked, and trigger regeneration on
selection — but two things weren't settled by that description or by anything upstream:

1. `cli.lua`'s `M.list_configs`/`M.generate` (ADR 34) both take an explicit `root` parameter
   rather than resolving it themselves, specifically so `cli.lua` doesn't have to depend on
   `discovery.lua`. The stubbed `M.pick_config(solution, on_choice)` signature had nowhere to put
   `root`, and nothing had decided whether `picker.lua` should inherit that same constraint or
   take the simpler route of calling `discovery.project_root()` itself.
2. `on_choice` carries a single value, `config: table|nil`. Three things can go wrong between "the
   user asked to pick" and "the database is regenerated" — a failed `list-configs`, an empty
   configuration list, and a failed `generate` after selection — plus the ordinary case of the
   user cancelling. Nothing had decided whether these four cases should be distinguishable to a
   caller, or whether they collapse to the same outcome.

## Decision

1. **`M.pick_config(solution, root, on_choice)`** — an explicit `root` parameter, following ADR
   34's precedent exactly. `picker.lua` does not `require("msvclsp.discovery")`; its only
   dependency is `cli.lua`.

2. **Every failure path converges on `on_choice(nil)`, preceded by a `vim.notify`:**
   - `cli.list_configs` returns an error → notify, `on_choice(nil)`, `vim.ui.select` never opens.
   - `available` is empty → notify (distinct message: no configurations declared), `on_choice(nil)`,
     `vim.ui.select` never opens.
   - The user cancels (`vim.ui.select` invokes its callback with `nil`) → `on_choice(nil)`, no
     notification (cancelling isn't a failure), `cli.generate` is never called.
   - `cli.generate` returns an error after a selection → notify, `on_choice(nil)`.

   A caller's contract is a single check: `if not config then return end`. It cannot distinguish
   "the user backed out" from "something failed and was already reported," but it also never needs
   to — in both cases there's nothing left for the caller to do, and the failure was already surfaced
   via `vim.notify` per CLAUDE.md's "fail loudly and legibly."

3. **`vim.ui.select`'s `format_item` renders `"Configuration|Platform  (active)"`**, matching
   `SolutionConfigs::render_human` (`cli/src/cli.rs`) — the same two-space marker, the same
   `|`-joined pair — so the human CLI output and the picker read identically.

4. **A `vim.notify` (info level) fires immediately before `cli.generate`** is called, since
   CLAUDE.md requires feedback while a CLI call is in flight.

## Discussion

**Why `root` is threaded rather than resolved inside `picker.lua`.** The alternative —
`picker.lua` calling `discovery.project_root()` itself — would shrink every call site by one
argument, but only by making `picker.lua` depend on a module it otherwise has no reason to know
about, and by duplicating a resolution `cli.lua`'s own callers already had to do. This issue
reuses ADR 34's reasoning nearly verbatim: whichever module invokes a picker (`commands.lua`,
MVP-31) has already resolved a root to call `discovery.active_solution()` in the first place, so
passing it through costs nothing there and keeps `picker.lua`'s dependency graph identical to
`cli.lua`'s. Confirmed with the user rather than assumed, since it's a real fork with a working
alternative.

**Why failure collapses to one shape instead of a second callback value.** The tempting
alternative is `on_choice(config, err)`, mirroring `cli.lua`'s own `(result, err)` convention. It
was rejected here because nothing downstream has a use for the distinction yet — no planned caller
(MVP-30's clangd restart, MVP-31's command) does anything different for "cancelled" versus "list-
configs failed" versus "generate failed." All three mean "there is no newly-picked, up-to-date
config to act on." Adding the second value now would be threading a distinction through every
future caller on the strength of a hypothetical need, which CLAUDE.md's "don't design for
hypothetical future requirements" argues against directly. If a caller someday needs to tell them
apart — to retry a failed `generate` without re-prompting, say — that caller can be given a second
return value then, informed by what it actually needs, rather than guessed at here.

**Why the empty-list case is guarded explicitly instead of trusting `vim.ui.select`.** Neovim's
built-in `vim.ui.select({}, ...)` calls its `on_choice` with `nil` immediately, which would already
produce the right *outcome* through the cancellation path. It's guarded explicitly anyway for two
reasons: third-party `vim.ui.select` backends (Telescope, fzf-lua, dressing.nvim) are not
guaranteed to handle an empty list the same way — some open an empty floating window instead of
short-circuiting — and "no configurations declared" is a genuinely different situation from "the
user declined to pick," worth its own `vim.notify` message rather than silently doing nothing.

**Why `format_item` mirrors the CLI's own `Display`/`render_human` output exactly.** The
alternative — some Neovim-idiomatic format like `"Debug (x64)"` — was rejected because
`ConfigPlatform`'s `Display` impl (`cli/src/lib.rs`) already exists specifically so "error messages
and pickers name a pair the way the user's own files do," per that impl's own doc comment. Matching
it means a user who runs `msvclsp list-configs` from a terminal and then uses the picker sees the
same vocabulary in both places.

## Consequences

- `commands.lua` (MVP-31) can call `M.pick_config(solution, root, on_choice)` and trust it never
  throws, never blocks, and only ever hands `on_choice` a config once `compile_commands.json` is
  actually up to date — simplifying whatever comes next (MVP-30's clangd restart) to a single nil
  check.
- `picker.lua`'s dependency graph is `cli.lua` only, matching `cli.lua`'s own `--root`-threading
  precedent from ADR 34 a second time — a pattern now established for any future module that needs
  both a root and a CLI call.
- A caller that later needs to distinguish cancellation from a CLI failure (to retry without
  re-prompting, for instance) will need `pick_config`'s contract to change; that is deferred, not
  designed for speculatively here.
- No changelog entry: no user command invokes `pick_config` yet (`commands.lua` is MVP-31's scope),
  mirroring MVP-28's own Outcome.
