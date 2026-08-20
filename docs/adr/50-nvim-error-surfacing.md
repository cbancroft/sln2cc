# ADR 50 — Surfacing CLI Errors and Warnings in the Plugin UI

- **Status:** Accepted
- **Date:** 2026-08-16
- **Issue:** [V2-04 — Surface CLI errors in the plugin UI](../todos.md#v2) (`feat/nvim-error-surfacing`)
- **Affects:** `nvim/lua/sln2cc/notify.lua` (new), `nvim/lua/sln2cc/cli.lua`,
  `nvim/lua/sln2cc/commands.lua`, `nvim/lua/sln2cc/picker.lua`, `nvim/tests/test_notify.lua`
  (new), `nvim/tests/test_cli.lua`, `nvim/tests/test_commands.lua`, `nvim/tests/test_picker.lua`,
  [engineering_spec.md](../engineering_spec.md) ("Neovim Plugin — Responsibilities"),
  [ADR 02](02-structured-error-type.md), [ADR 03](03-cli-surface-and-json-contract.md),
  [ADR 06](06-diagnostic-collection.md), [ADR 13](13-diagnostic-severity-and-wording.md),
  [ADR 34](34-nvim-cli-invocation.md)

## Problem

Both halves of "present structured CLI errors and evaluation warnings legibly" were vanishing.

Every failure path in `commands.lua`/`picker.lua` did `vim.notify("sln2cc: " .. err.message,
vim.log.levels.ERROR)`. `err.kind` — the entire reason ADR 34 threaded it through `cli.lua` at
all ("V2-04 already has warnings available... and a `kind` field on every error, without needing
to touch `cli.lua` again") — was decoded and then never read. A user saw "solution not found:
C:\App\App.sln" with no next step, when `ErrorKind` (`cli/src/error.rs`) already distinguishes
14 failure modes with 14 different fixes.

Warnings fared worse: they never reached the plugin at all on a successful run. ADR 34 has
`M.run` hand back the *whole* decoded envelope specifically so `envelope.warnings` would survive
for this issue to render ("handing back the full table now means V2-04 doesn't have to change
`cli.lua` to reach them"). But `M.list_configs`/`M.generate` — the functions `commands.lua` and
`picker.lua` actually call — unwrapped to `envelope.result` and dropped `.warnings` on the floor.
A solution that warned about an unsupported MSBuild construct (ADR 13) told the plugin user
nothing: the warning existed on stderr in human-readable mode, but the plugin only reads stderr
on a *failed* run, to build `err.message`.

This left a standing engineering-spec Open Question unresolved: "Which channel warnings should
actually reach the user through in Neovim: rendered from the envelope's `warnings` array, or
folded in with stderr the way MVP-28 handles errors... V2-04 owns error surfacing; nothing yet
says who owns warnings" — echoed verbatim in ADR 06's Consequences.

## Decision

**Warnings ride the envelope; stderr is never scraped for them.** This was not a close call once
examined: stderr's contents are captured by `cli.lua` only inside `handle_exit`'s failure branch,
to build `err.message`. A successful run's stderr text is never read by anything in the plugin,
and CLAUDE.md §3 forbids scraping human-readable text regardless. The envelope's `warnings` array
is the only channel that exists. This closes the Open Question in `engineering_spec.md` and
ADR 06.

**`M.list_configs`/`M.generate` gain a 3rd, additive `on_done` argument: `warnings`.**
`on_done(result, err, warnings)` — `nil` on any failure, `envelope.warnings` on success. Every
existing 2-parameter callback (`health.lua`'s `check_database`, and every test that predates this
issue) is unaffected: Lua silently discards a 3rd actual argument a callback's formal parameter
list doesn't declare.

**A new module, `nvim/lua/sln2cc/notify.lua`, owns both renderers.**

- `M.REMEDIES`: a hand-written table, `ErrorKind` wire string → one-line, kind-specific remedy.
  All 14 of today's kinds have a row. A kind with no row (a future addition to `error.rs` not
  yet mirrored here) falls back to the message-only form rather than erroring — silently
  under-informative is an acceptable failure mode; erroring on a notification path is not.
- `M.notify_error(err)`: `vim.notify` at `ERROR`, always. Appends the remedy when `err.kind` has
  one; otherwise the CLI's own `message` (already legible — it's stderr's text, or "sln2cc
  exited with code N") stands alone.
- `M.notify_warnings(warnings)`: no-op on `nil`/empty. Otherwise batches every diagnostic into
  **one** `vim.notify` call, not one per diagnostic — see the volume discussion below. Level is
  `WARN` if any diagnostic's `severity` is not exactly `"note"`, else `INFO`. Per ADR 13's own
  instruction ("MVP-28 must not branch exhaustively on the severity string, and should treat an
  unknown value as a warning"), an unrecognized severity is treated as a warning here too — this
  is a *new* consumer of `severity`, so the instruction needed a fresh place to apply.

**`commands.lua` and `picker.lua`'s CLI-error notify call sites are replaced with
`notify.notify_error`, and a `notify.notify_warnings` call is added after every successful
`list_configs`/`generate`.** Non-`CliError` notifications (no active solution, no active
configuration, rescan count, vanished solution) are untouched — they have no `kind` to act on and
were never part of this problem.

**`health.lua` is untouched.** `check_database`'s `cli.list_configs` call is a point-in-time
status check for `:checkhealth`, not a "run" whose new findings need surfacing; its existing
`vim.health.error`/`vim.health.warn` calls already carry a remedy in the shape `:checkhealth`
expects. Nothing here changes that.

## Discussion

**Additive 3rd argument vs. a parallel function pair vs. callers using `cli.run` directly.**
Three shapes were considered for getting warnings out of `list_configs`/`generate`:

1. The chosen one: add `warnings` as a 3rd `on_done` argument.
2. Add `list_configs_with_warnings`/`generate_with_warnings` alongside the untouched originals.
3. Have `commands.lua`/`picker.lua` call `cli.run` directly wherever they want warnings, since
   `cli.run` already hands back the full envelope and ADR 34 says this issue "doesn't have to
   change `cli.lua`" to reach them.

(2) was rejected for permanently forking every future change to the argv-building logic
(`--root`/`--configuration`/`--platform`/`--force`) across two implementations of the same two
functions, for a distinction (does the caller want warnings) that every caller actually wants —
there is no real use case for a `list_configs` call that discards them. (3) was rejected because
it reintroduces the argv-building duplication in a different place: `commands.lua`/`picker.lua`
would each have to know `list-configs`/`generate`'s exact flag names, which is precisely what
ADR 34 built `list_configs`/`generate` to encapsulate. ADR 34's "doesn't have to change
`cli.lua`" was read as being about the *envelope's shape at `M.run`* not changing — which held —
rather than as a constraint that `list_configs`/`generate` themselves could never gain a
parameter. (1) costs nothing to callers that don't care, keeps the argv-building in one place,
and is the smallest change that makes warnings reachable.

**A dedicated `notify.lua` vs. inlining into `commands.lua`/`picker.lua`.** Inlining the
`REMEDIES` table into `commands.lua` and having `picker.lua` either `require` it or duplicate a
subset was the cheaper-looking option — fewer files, less indirection. It was rejected for
exactly ADR 13's reason for centralizing `eval::report`: "wording that is explicitly a decision
cannot live in three places and stay one decision." Two call sites (`commands.lua`, `picker.lua`)
already exist and a third (`health.lua`, someday, if this issue's scope grows) is plausible; a
single module makes the fourteen-row table one honest source of truth, independently unit-tested
in `test_notify.lua` without needing to route through either caller's own stubbing machinery.

**Batched warnings, one `vim.notify` per CLI call, not one per diagnostic.** ADR 13 already
flagged the volume risk for the *stderr* rendering of warnings — "MVP-15 will... start importing
the Visual Studio installation's own `.props`/`.targets`, at which point `` `$(X)` is not defined
and expanded to nothing`` fires on the order of hundreds of times per project... a channel that is
ignored is worse than no channel, because it looks like diligence." A notification *storm* on
every `:Sln2ccGenerate` would be strictly worse than the stderr case, since each `vim.notify`
call is its own UI event (a popup, a noice.nvim entry, a statusline flash) rather than a scrolling
log line. One notification per call, with every diagnostic's message concatenated, keeps the
signal ("something needs your attention, here's everything") without turning a single `generate`
into dozens of interrupts. The cost is that a user cannot dismiss individual findings — accepted,
since `notify.lua` has no memory of what a user already acknowledged either way, and a plugin-side
"seen" tracker was out of scope for what this issue asked for.

**Level split on `severity`, not just always `WARN`.** `Severity::Note` (ADR 13) exists precisely
to distinguish "flags may be wrong" from "unusual but harmless." Collapsing both to `WARN` in the
plugin would throw that distinction away one layer up from where it was deliberately introduced.
`INFO` for an all-`Note` batch keeps the split meaningful without inventing a third level the CLI
doesn't have.

**`health.lua` out of scope.** Extending `check_database` to also render warnings from its
`cli.list_configs` call was considered, since it already calls the function that now carries
them. It was rejected because `:checkhealth`'s whole contract (ADR 39) is a synchronous snapshot
of *status*, reported through `vim.health.ok`/`warn`/`error` — not a place that fires
`vim.notify` calls, and mixing the two reporting mechanisms in one provider would be inconsistent
with every other check in the same file. If `:checkhealth` ever wants to surface warnings, that's
its own decision about its own reporting channel, not an extension of this one.

**No new config option.** Nothing in V2-04's own description asks for suppressing remedies or
warnings, and CLAUDE.md §5 (no half-finished implementations, no speculative configurability)
argues against adding one on spec. If muting turns out to matter, it's a future issue with its
own default in `config.lua`.

## Consequences

- `cli.lua`'s `list_configs`/`generate` signatures gain a 3rd `on_done` argument. Documented in
  `engineering_spec.md`'s "Neovim Plugin — Responsibilities", CLI invocation bullet.
- The engineering spec's warnings-channel Open Question is resolved and removed.
- `notify.lua`'s `M.REMEDIES` is a second, hand-maintained copy of `ErrorKind`'s wire strings
  (the first being `cli/src/error.rs::ErrorKind::ALL`). Nothing enforces the two stay in sync
  across the Rust/Lua boundary; a kind added to `error.rs` without a row added here degrades to
  the message-only notification rather than breaking, per this ADR's own decision — but it is a
  drift risk worth naming for whoever next touches `error.rs`.
- `commands.lua` and `picker.lua` each gain a `require("sln2cc.notify")` dependency, their first
  dependency on anything outside `cli.lua`/`discovery.lua`/`picker.lua`/`lsp.lua`.
- **New open question:** whether `:checkhealth`'s `check_database` should eventually surface
  warnings too, and through which mechanism (`vim.health.warn` per diagnostic vs. a summary
  line). Not needed by anything today; left for whoever next touches `health.lua`.
