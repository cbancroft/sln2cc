# ADR 37 — Neovim User Commands

- **Status:** Accepted
- **Date:** 2026-08-13
- **Issue:** [MVP-31 — User commands](../todos.md#neovim-plugin) (`feat/nvim-commands`)
- **Affects:** `nvim/lua/sln2cc/commands.lua`, `nvim/lua/sln2cc/picker.lua`,
  `nvim/lua/sln2cc/init.lua`, `nvim/plugin/sln2cc.lua`, `nvim/tests/test_commands.lua`,
  `nvim/tests/test_picker.lua`, [engineering_spec.md](../engineering_spec.md) ("Neovim Plugin —
  Responsibilities"), [ADR 33](33-nvim-solution-discovery.md), [ADR 35](35-nvim-config-picker.md),
  [ADR 36](36-nvim-clangd-lifecycle.md)

## Problem

`commands.lua` had `M.register()` stubbed with `error("not yet implemented")`, and `M.commands`
pre-declared five command names — including `Sln2ccSelectSolution`, which depends on
`picker.lua`'s `M.pick_solution`, itself explicitly deferred to V1-03 (ADR 33). This issue is the
last piece wiring the four already-implemented modules (`discovery.lua`, `cli.lua`, `picker.lua`,
`lsp.lua`) to something a user actually invokes, and both ADR 35 and ADR 36 explicitly left gaps
only this issue could close:

1. `picker.pick_config`'s `on_choice` hands back only the picked configuration, not the
   `GenerateResult.database` its own internal `generate` call already produced — needed to point
   `lsp.restart` at the right database after a configuration switch.
2. Neither `picker.lua` nor `lsp.lua` reads `config.options.auto_restart_clangd`; something has to
   decide when a regenerate should restart clangd.
3. `Sln2ccRestart` has no picker or generate call of its own — it restarts clangd against
   whatever was last generated, but nothing tracked what that was.
4. `commands.lua`'s own doc comment ("register lazily so the load guard in `plugin/sln2cc.lua`
   keeps startup cost at zero") and CLAUDE.md §4 ("commands invoked before `setup()` should say so
   clearly") together imply commands exist *before* `setup()` runs, with each handler saying so
   itself — but `init.lua`'s `M.setup()` was the only call site for `commands.register()`, so
   before `setup()` the commands didn't exist at all, producing Neovim's own generic "not an
   editor command" rather than a clear, sln2cc-specific message.

## Decision

1. **`Sln2ccSelectSolution` is not registered by this issue.** `M.commands` drops it; V1-03 adds
   it back alongside `picker.pick_solution`. MVP-31's own todo description lists only
   generate/select-config/rescan/restart, and registering a command whose only possible behavior
   is an immediate stub error contributes nothing a user couldn't get from the command simply not
   existing yet.

2. **`Sln2ccGenerate` reads `list-configs`' `.active` pair** (the CLI's own last-persisted
   configuration/platform from `db.rs`) rather than opening a fresh picker. No active
   configuration yet → `vim.notify` (WARN) pointing at `:Sln2ccSelectConfig`, and `generate` is
   never called. Confirmed with the user over the alternative (always routing `Sln2ccGenerate`
   through `pick_config`), which would make the two commands nearly redundant — "regenerate with
   what's already selected" is a materially different action from "pick something new."

3. **`picker.pick_config`'s `on_choice` widens to `(config, database)`.** `database` is the
   `GenerateResult.database` from the `generate` call `pick_config` already makes internally on a
   successful selection; it's `nil` on every existing failure path (failed `list-configs`, empty
   list, cancellation, failed `generate`) — the same paths that already produced `config == nil`.
   This is an added success payload, not a new failure-disambiguating value, so ADR 35's point 2
   (why failure collapses to one shape) is unaffected: a caller still branches on a single `if not
   config then return end`, just with one more piece of data on the other side of that check.
   Confirmed with the user over having `commands.lua` re-invoke `generate` itself, which would
   double the CLI process spawns per selection for no benefit (the CLI's own up-to-date check would
   make the second call a no-op, but a no-op subprocess is not free).

4. **`Sln2ccRestart` reads a database cached by `commands.lua` itself**, populated by a successful
   `Sln2ccGenerate` or `Sln2ccSelectConfig`, keyed by root (matching every other cache in the
   plugin under the MVP's single-active-solution assumption). With nothing cached yet,
   `Sln2ccRestart` calls `lsp.restart(nil, root)` directly — that call already warns clearly on
   its own (established by MVP-30), so `commands.lua` doesn't duplicate the message.

5. **`auto_restart_clangd` gates on the option alone, not on `GenerateResult.regenerated`.** Even a
   no-op generate (the CLI's cache was already current) still means clangd should end up attached —
   a fresh Neovim session has no running client regardless of whether the database file itself
   changed. `lsp.restart` already degenerates cleanly to a start when nothing was running
   (MVP-30's own tests establish this), so `Sln2ccGenerate`/`Sln2ccSelectConfig` make a single
   unconditional `lsp.restart` call when the option is on, not a start/restart branch keyed on
   `regenerated`.

6. **Commands are registered before `setup()`.** `plugin/sln2cc.lua` calls
   `require("sln2cc.commands").register()` unconditionally at Neovim startup, immediately after
   the load guard — five `vim.api.nvim_create_user_command` calls with no filesystem access, so it
   doesn't violate "never scan the filesystem at startup." `commands.lua` gains an internal
   `M._ready` flag (default `false`); every command handler checks it first and, if unset,
   `vim.notify`s `"sln2cc: call require('sln2cc').setup() first"` (ERROR) without touching any
   other module. `init.lua`'s `M.setup(opts)` no longer calls `commands.register()`; once
   `config.setup(opts)` succeeds it calls `commands.mark_ready()` instead. A failed `setup()` (bad
   `opts`) leaves commands registered but inert, matching `config.setup`'s existing "notify and
   return `nil`" failure shape rather than throwing.

## Discussion

**Why `Sln2ccSelectSolution` isn't stubbed with a friendly "coming soon" message instead of being
omitted entirely.** Both were on the table. A registered-but-inert command would show up in
command completion, which has some value — a user discovers the name exists. It was rejected
because the MVP's own framing (single, first-found solution; ADR 33) means there is nothing this
command could do differently from what already happens automatically, and a command whose only
purpose is announcing its own future existence is speculative surface area CLAUDE.md's "don't
design for hypothetical future requirements" argues against. V1-03 adds the real command when
`pick_solution` gives it something to do.

**Why `list-configs`' `.active` and not a locally-remembered "last picked" value.** `commands.lua`
could have remembered the configuration `Sln2ccSelectConfig` last picked, in the same per-root
cache holding the database path. It wasn't, because `db.rs` already persists exactly this
(`active_config`, per the CLI's own state file) and is the source of truth across CLI invocations,
Neovim restarts, and even a bare `sln2cc generate` run from a terminal outside the plugin
entirely. A second, plugin-local copy of "the active configuration" would drift from the CLI's own
the moment either was updated independently, and CLAUDE.md's storage layout already assigns this
job to `db.rs`.

**Why the database cache lives in `commands.lua` rather than being added to `discovery.lua` or
`lsp.lua`.** Both were considered. `discovery.lua` already tracks per-root state (`solutions_cache`,
`active_cache`), so a `database_cache` alongside them wasn't a stretch. It was kept in
`commands.lua` instead because no other module needs it — `lsp.lua` takes `database` as an explicit
parameter by design (ADR 36 point 6), and `discovery.lua`'s job is solution discovery, not tracking
what the CLI last generated. Adding it to either would widen that module's responsibility for a
need specific to one command's convenience.

**Why registration moved out of `init.lua`.** The alternative — leaving `commands.register()` in
`M.setup()` as it already was — is simpler and was the status quo. It was changed because it
directly contradicts two already-written requirements: `commands.lua`'s own doc comment (commands
register off the *load guard*, not `setup()`) and CLAUDE.md §4's "commands invoked before setup()
should say so clearly," which presumes the commands exist to be invoked. Splitting "register" (cheap,
happens at load) from "ready" (gated on `setup()` succeeding) is what makes both statements true
simultaneously without contradicting either.

## Consequences

- A user who installs the plugin but never calls `setup()` sees `:Sln2ccGenerate` (etc.) in
  command completion and gets a specific, actionable error on invocation, rather than Neovim's
  generic "not an editor command."
- `picker.lua`'s `on_choice` contract changed for every existing and future caller — there is
  exactly one caller today (`commands.lua`), so this cost is paid once, now, rather than needing a
  second migration later. `nvim/tests/test_picker.lua`'s existing specs were extended in place
  (asserting the new second argument) rather than duplicated.
- `Sln2ccSelectSolution` stays unregistered until V1-03, which will need to decide how a solution
  switch interacts with the per-root database cache this issue introduces (a new active solution
  under the same root invalidates the cached database) — left to that issue, not designed for here.
- Changelog entry added: this is the first issue where a user-facing command actually exists to
  invoke any of the previously-landed MVP-28 through MVP-30 machinery.
