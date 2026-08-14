# ADR 39 — Neovim `:checkhealth`

- **Status:** Accepted
- **Date:** 2026-08-14
- **Issue:** [MVP-32 — `:checkhealth msvclsp`](../todos.md#neovim-plugin) (`feat/nvim-health`)
- **Affects:** `nvim/lua/msvclsp/health.lua`, `nvim/tests/test_health.lua`, `cli/src/cli.rs`
  (`SolutionConfigs`, `list_configs`), `cli/tests/cli_surface.rs`,
  [engineering_spec.md](../engineering_spec.md) ("CLI Surface", "Neovim Plugin —
  Responsibilities"), [ADR 03](03-cli-surface-and-json-contract.md),
  [ADR 34](34-nvim-cli-invocation.md)

## Problem

`health.lua` was a stub (`M.check() error("not yet implemented")`). CLAUDE.md §4 names
`:checkhealth` as "the first place a user looks when nothing works," and the todo item scopes five
checks: the binary is present and runnable (reporting its version), `clangd` is on `PATH`, a
project root is detectable, a solution has been found, and a configuration/platform is selected
with a database present on disk.

The first four read state `config.lua` and `discovery.lua` already expose. The fifth does not have
anywhere to read from: `list-configs`' `SolutionConfigs` (`cli.rs`) reports `active` — the chosen
configuration/platform — but never *where* that configuration's `compile_commands.json` would
live. That path is computed by `compdb::resolve_output_dir(msvclsp_dir, sln_path)`, keyed on the
solution alone (namespacing is per-solution today; V1-02 owns per-configuration namespacing), and
nothing on the Lua side duplicates that computation. Two more small gaps surfaced once the shape of
`M.check()` was worked out: `discovery.project_root()` silently folds "found via a root marker"
and "fell back to the current directory" into the same plain string, and `cli.list_configs` is
async while every `:checkhealth` provider Neovim calls is expected to have finished reporting by
the time `M.check()` returns.

## Decision

1. **`SolutionConfigs` gains a `database: PathBuf` field**, computed in `list_configs()` via
   `compdb::resolve_output_dir`, independent of `active`. Confirmed with the user over duplicating
   the naming scheme in Lua: a second implementation of "where does this solution's database live"
   is exactly the kind of drift ADR 03 already ruled out once, when solution scanning was kept out
   of the CLI specifically so it would have one home. Adding a field to an existing `--json`
   response is additive and does not bump `schema_version`, per the envelope's own documented rule.

2. **`health.lua` blocks on `cli.list_configs` with `vim.wait`.** `:checkhealth` providers are
   called synchronously and expected to have finished emitting `vim.health.*` reports by the time
   `M.check()` returns; nothing downstream re-enters a provider to let it report late. The plugin's
   own tests already wait out a `vim.schedule`-deferred fake the same way
   (`nvim/tests/test_cli.lua`'s `wait_for`), so this is the same pattern used for its own purpose
   in production code.

3. **Binary version is read by invoking `<bin> --version` directly, bypassing `cli.lua`.**
   `--version` is `clap`'s own eager flag (`#[command(version)]` on `Cli`); it never emits the
   `--json` envelope `cli.lua`'s `M.run`/`handle_exit` are built to decode (ADR 34), so routing it
   through `cli.lua` would report a perfectly healthy binary as a decode failure. `health.lua`
   instead gets its own overridable `M._system = vim.system` field — a second instance of the same
   seam `cli.lua` uses for its own tests (ADR 34 point 2), not a shared one, since the two modules'
   tests need to stub two different real side effects independently.

4. **Root-marker match and cwd-fallback are reported as different outcomes.**
   `discovery.project_root()` returns a plain string either way, so a caller can't currently tell
   which happened. `health.lua` calls `vim.fs.root(0, config.options.root_markers)` itself — the
   same primitive `discovery.lua` already calls, not a second implementation of anything stateful —
   purely to distinguish the two before still using `discovery.project_root()`'s value as the
   reported root. `discovery.lua`'s public API is unchanged.

5. **Each of the five checks degrades independently rather than short-circuiting the whole run.**
   A missing binary skips the version and database checks (both need to invoke it) but still runs
   the clangd/root/solution checks; no solution found skips the database check (nothing to call
   `list-configs` on) but still reports root and clangd. Every non-`ok` report carries a remedy —
   `:MsvclspSelectConfig`, `:MsvclspRescan`, "install clangd and put it on PATH," or, for the two
   checks with no single fix to point at (a binary that doesn't respond to `--version`, a
   `list-configs` invocation that fails outright), the equivalent CLI invocation to run by hand and
   see the underlying failure — per CLAUDE.md §4's "actionable remedy rather than just a red mark."

## Discussion

**Why the database path is a CLI field and not a Lua-side recomputation.** The alternative that
avoids any Rust change is real: `output_dir_name`'s hash is a pure function of the solution path,
and Lua could compute the same FNV-1a hash `compdb.rs` does. It was rejected on the same grounds
ADR 03 already settled for solution discovery — a second copy of naming logic that's supposed to
have exactly one home drifts the moment either side changes independently, and a hash mismatch
between the two would be a silent, hard-to-diagnose failure mode (health reporting the wrong path
as "missing" while the real database sits one directory over). An additive JSON field costs one
struct field and one `resolve_output_dir` call already available in-crate; the Lua side then owns
nothing about the naming scheme at all, just `stat`ing a path the CLI already resolved.

**Why not have `commands.lua` expose its session-local `last_database` cache instead.**
`commands.lua` already tracks the database path per root, populated after a successful `generate`
call this session (ADR 37). Reusing it would need no CLI change at all. Rejected because the todo
item asks specifically for "a database present **on disk**" — a session cache reports nothing for
a solution that was generated in a previous Neovim session (or from a terminal, or by a different
Neovim instance entirely) even though the file is sitting right there. `:checkhealth`'s whole
premise is diagnosing a fresh, possibly-broken setup; a check that only works after the user has
already successfully run `:MsvclspGenerate` once this session answers a much narrower question than
the one being asked.

**Why `resolve_output_dir`'s side effect (creating the output directory and its `.source` marker)
is acceptable to trigger from `list-configs`, a nominally read-only query.** `list_configs()`
already calls `db::Store::open`, which itself creates `.msvclsp/` and writes `state.json` if
neither exists — "read the current state" already mutates the filesystem to establish that state
exists. `resolve_output_dir` creating the per-solution subdirectory and marker is the same category
of side effect, not a new one introduced by this issue; `list-configs` was never purely read-only.

**Why `health.lua` gets its own `M._system` instead of reaching into `cli.lua`'s.** `cli.lua`'s
`M._system` is documented (ADR 34) as *that module's* internal seam for *its own* tests. Two
modules sharing one mutable field would mean a test stubbing it for one module's behavior silently
changes the other's, and — more fundamentally — `cli.lua`'s `M.run` always appends `--json` and
always decodes an envelope, which is precisely the behavior `--version` needs to skip. There is no
version of a shared seam that serves both call shapes without one module reaching around the
other's contract.

**Why checks degrade independently rather than stopping at the first failure.** A `:checkhealth`
report that stops at "binary not found" and says nothing about clangd, the project root, or
whether a solution was even found leaves a user fixing one problem at a time across repeated
`:checkhealth` runs, when several are usually independent and equally worth knowing about upfront.
The only real dependencies are "the binary must exist to ask it anything" and "a solution must
exist to ask the CLI about its configurations" — both are structural (there's genuinely nothing to
check) rather than a choice to stop early.

## Consequences

- `SolutionConfigs.database` is available to any future caller of `list-configs`, not just
  `health.lua` — e.g. a future "open the compilation database" command could reuse it without
  another wire change.
- `cli/tests/cli_surface.rs` gained two new tests pinning the `database` field's shape and the
  claim that `list-configs` resolves (and creates) the output directory without ever writing
  `compile_commands.json` itself.
- `discovery.lua`'s public API is unchanged; `health.lua` duplicates one `vim.fs.root()` call
  rather than gaining a dependency on a new `discovery.lua` export.
- `nvim/doc/msvclsp.txt`'s troubleshooting section (MVP-33, still unwritten) can now point at real,
  documented `:checkhealth` behavior instead of a stub that always errors.
