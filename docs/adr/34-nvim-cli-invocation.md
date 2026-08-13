# ADR 34 — Neovim CLI Invocation

- **Status:** Accepted
- **Date:** 2026-08-12
- **Issue:** [MVP-28 — Async CLI invocation and JSON decoding](../todos.md#neovim-plugin)
  (`feat/nvim-cli-bridge`)
- **Affects:** `nvim/lua/msvclsp/cli.lua`, `nvim/tests/test_cli.lua`, `cli/src/cli.rs`
  (`matches_the_argv_cli_lua_builds`), [engineering_spec.md](../engineering_spec.md) ("Neovim
  Plugin — Responsibilities"), [ADR 03](03-cli-surface-and-json-contract.md),
  [ADR 13](13-diagnostic-severity-and-wording.md), [ADR 33](33-nvim-solution-discovery.md), V2-04

## Problem

`cli.lua` had three functions that all `error("not yet implemented")` — `M.run`, `M.list_configs`,
`M.generate` — plus a fourth, `M.discover(root, on_done)`, that ADR 33 confirmed as dead code: it
predates ADR 03's decision that the CLI has no `discover` subcommand for the MVP and nothing calls
it.

The wire contract on the other side is fully settled (ADR 03's envelope, ADR 13's `Severity`), but
nothing had decided how `cli.lua` turns "spawn a process, wait for it, read two streams" into the
`(result, err)` callback shape every other module was already written to expect, or how its own
tests intercept that spawn without a real binary — the issue's Tests line asks for "a stubbed job
runner," which is a different seam than every *other* module's tests use (they stub `cli.lua`
itself via a `package.loaded` swap, per CLAUDE.md §4).

`engineering_spec.md`'s "CLI Surface" table also documents that `--root` is the project root the
plugin resolved, and that it is expected to diverge from the solution's own directory — but neither
stubbed signature (`list_configs(solution, on_done)`, `generate(solution, config, on_done)`) had a
place to put it.

## Decision

1. **`vim.system()`, not `vim.fn.jobstart()`.** `vim.system`'s `on_exit` callback already hands
   back `obj.code`, `obj.stdout`, and `obj.stderr` as complete strings, which is exactly "collect
   stdout and stderr separately... hand `(result, err)` to a callback." No version cost:
   `vim.system` needs Neovim 0.10+, and ADR 28 already raised the plugin's floor to 0.12+ for
   `vim.fs.root()`.

2. **The job runner is an overridable module field, `M._system`, defaulting to `vim.system`.**
   `cli.lua`'s own tests replace `cli._system` with a fake before calling `M.run`. This is
   `cli.lua`'s *internal* seam, distinct from the `package.loaded["msvclsp.cli"]` swap every other
   module's tests use to avoid invoking the binary at all.

3. **`M.run(args, on_done)` hands back the whole decoded envelope as `result` on success**, not
   just `envelope.result`. `envelope.warnings` has nowhere else to go, and V2-04 ("Surface CLI
   errors in the plugin UI") owns rendering warnings, not this issue — handing back the full table
   now means V2-04 doesn't have to change `cli.lua` to reach them. `M.list_configs`/`M.generate`,
   built on `M.run`, unwrap to `envelope.result` before calling their own `on_done`, since their
   callers want `SolutionConfigs`/`GenerateResult` directly.

4. **The error object folds stderr's text into `message`, and carries `kind` when the envelope
   decoded with `has_error = true`.**

   ```lua
   { message = string, kind = string|nil, code = integer|nil }
   ```

   `message` is always legible: stderr's trimmed text, or `"msvclsp exited with code N"` in the one
   case stderr can be empty on a non-zero exit — a `clap` usage error (exit `2`), which per ADR 03
   writes its own text to stderr and leaves stdout undecodable. `kind` rides along when the
   envelope did decode and `has_error` was true, so V2-04 has something to branch on later without
   parsing `message`. `code` is `nil` only when no process ever ran at all.

5. **A missing binary is caught with `pcall` around the `_system` call**, not inside `on_exit`.
   `vim.system()` raises synchronously when the underlying spawn fails (e.g. `ENOENT`) rather than
   reaching the callback. `M.run` wraps the call and reports it through the same `on_done(nil, err)`
   shape as every other failure, with `err.kind = nil` and `err.code = nil`.

6. **`M.list_configs`/`M.generate` take an explicit `root` parameter**, appended as `--root`:
   `list_configs(solution, root, on_done)`, `generate(solution, config, root, on_done)`. `cli.lua`
   does not `require("msvclsp.discovery")` — whichever module wires commands together resolves the
   root and passes it in, keeping `cli.lua`'s only dependency the binary itself.

7. **`M.discover` is deleted**, per ADR 33's note. Nothing calls it and it disagrees with ADR 03's
   settled surface.

## Discussion

**Why the stub seam is a module field and not a constructor argument.** Threading a runner through
every call site (`M.run(args, on_done, system)`) would make every caller of `list_configs`/
`generate` aware of a seam that exists purely for `cli.lua`'s own tests. `M._system` keeps it local:
production code never sets it, and every call site outside this module is unaffected. The cost is a
mutable module-level field, which is unusual in this codebase but already how `config.lua` exposes
`M.options` for the same reason — tests need to reach in and change behavior without threading a
parameter through everything downstream.

**Why `M.run` doesn't unwrap `envelope.result` itself.** The tempting simpler shape is `on_done`
receiving just the outcome, mirroring what `list_configs`/`generate` end up handing their own
callers. It was rejected because `M.run` is the only place that ever sees the full envelope —
unwrapping here means `warnings` is gone by the time anything else looks, and V2-04 would have to
change this module's return shape to get them back, at which point `M.run`'s contract has changed
twice for the same underlying reason. Keeping the full envelope at this one layer and unwrapping
one level up, in the two functions whose callers actually want a bare result, costs one extra field
read in two places and avoids that.

**Why the error carries `kind` at all, when nothing consumes it yet.** The alternative was `message`
only, which is simpler and sufficient for MVP-28 alone. It was rejected on the same timing argument
ADR 13 made for `Severity::Note`: `envelope.error.kind` is already sitting in the decoded table by
the time `handle_exit` runs, at zero marginal cost, and V2-04 is a named, planned consumer of
exactly this field ("Present structured CLI errors... legibly"). Discarding it now means V2-04
has to thread a second return value or re-decode stdout itself.

**Why `--root` is a parameter rather than `cli.lua` resolving it.** `discovery.lua` already owns
project-root detection (ADR 28) and solution discovery (ADR 33), both cached per-buffer or
per-root. Having `cli.lua` call into `discovery.lua` directly would make the CLI-invocation module
depend on the discovery module for a value its caller almost always already has in hand (whichever
command handler is about to invoke `list_configs`/`generate` has just resolved a root and an active
solution to get there). Passing it in keeps `cli.lua`'s dependency graph to "the binary and
`config.lua`" only, and makes the argv it builds fully determined by its own arguments — which is
also what makes `matches_the_argv_cli_lua_builds` in `cli/src/cli.rs` possible to write without
guessing at plugin internals.

**Why the Rust side gained a test instead of just the Lua side.** ADR 03 already named this as
open work: "MVP-28 should extend `both_subcommands_accept_json`... to assert against the argv
`cli.lua` actually builds." A test only on the Lua side would catch a `cli.lua` regression but not
a CLI-side rename of `--root`/`--configuration`/`--platform` that silently breaks every plugin
call; pinning the literal argv on both sides means either one drifting from the other fails a test
instead of shipping a plugin that stops parsing.

## Consequences

- `cli.lua` is a real, stubbable boundary: `picker.lua`, `commands.lua`, and `lsp.lua` (none of
  which exist yet) can call `M.list_configs`/`M.generate` and stub the whole module in their own
  tests, per CLAUDE.md §4.
- **V2-04 already has warnings available** via `M.run`'s full-envelope result and a `kind` field on
  every error, without needing to touch `cli.lua` again.
- **Whichever issue builds `commands.lua` owns resolving `root`** (via `discovery.project_root()`)
  before calling `list_configs`/`generate` — this issue does not wire either function into a user
  command.
- `cli/src/cli.rs` gained `matches_the_argv_cli_lua_builds`, pinning the exact argv literal on the
  Rust side; a rename on either side of the plugin/binary boundary now fails a test rather than
  shipping silently.
- `M.discover` and the `DiscoveredSolution` type it never used are no longer reachable from the
  plugin. `DiscoveredSolution` itself stays in `cli.rs` as a V1-01 placeholder, per ADR 03.
