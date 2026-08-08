# ADR 15 — Migrate the Neovim Plugin Test Harness to `mini.test`

- **Status:** Accepted
- **Date:** 2026-08-08
- **Issue:** [MVP-39 — Migrate the plugin test harness from `plenary.nvim` to `mini.test`](../todos.md#neovim-plugin) (`chore/nvim-mini-test`)
- **Affects:** `nvim/tests/`, [CLAUDE.md](../../CLAUDE.md) (§2, §5, §7, §8),
  [README.md](../../README.md), [engineering_spec.md](../engineering_spec.md),
  `.github/workflows/ci.yml`, `docs/todos.md` (MVP-25 through MVP-34, V1-03 through V1-06, V2-01,
  V2-04)

## Problem

`nvim/tests/` has used `plenary.nvim`'s busted-style harness since the plugin's test scaffolding was
first written, and every plugin issue from MVP-25 onward is specified in `docs/todos.md` as
"plenary specs." `plenary.nvim`'s own README now carries a notice that it is no longer actively
maintained and will be officially archived, with only critical bug fixes through 2026-06-30 and no
new features — and it names no successor for its test harness specifically.

The timing matters more than the deprecation itself: `nvim/tests/discovery_spec.lua` is the only
spec file that exists, and every case in it is still a `pending()` placeholder, because none of
MVP-25 through MVP-34 (the issues that write real plugin logic and its tests) has started. This is
the one point in the project's life where migrating costs one placeholder file and some docs/CI
wiring, rather than a rewrite of a dozen real specs once those issues land.

## Decision

Replace `plenary.nvim` with [`mini.test`](https://github.com/nvim-mini/mini.test), part of the
actively-maintained `mini.nvim` library, adopting its defaults rather than configuring around
plenary's shape:

- `nvim/tests/minimal_init.lua` locates `mini.test` the same way it located plenary — an explicit
  `MINI_TEST_DIR` environment variable (CI sets this), falling back to a `lazy.nvim`-managed
  checkout at `stdpath("data")/lazy/mini.test`. No `require('mini.test').setup()` call lives here;
  that happens via `-c` at invocation time, matching the pattern `mini.nvim` uses on itself.
- `discovery_spec.lua` is renamed to `test_discovery.lua`, and its `pending("...")` bodies become
  `MiniTest.skip("...")`, mini.test's own documented replacement for `pending()`. The `describe`/
  `it` structure is unchanged, since `mini.test`'s `emulate_busted` option (on by default) supplies
  those globals.
- `.github/workflows/ci.yml`'s `nvim` job clones `nvim-mini/mini.test` instead of
  `nvim-lua/plenary.nvim`, and the invocation becomes two `-c` calls —
  `-c "lua require('mini.test').setup()" -c "lua MiniTest.run()"` — replacing plenary's single
  `PlenaryBustedDirectory tests/ { minimal_init = '...' }` command.
- Every `docs/todos.md` entry that specified "plenary specs" as its test plan is reworded to name
  `mini.test` instead, since those are still unstarted specifications, not a historical record of
  what was actually built.

No `nvim/lua/**` code changes: this is a test-tooling swap, not a plugin behavior change, so
`docs/changelog.md` gets no entry (CLAUDE.md §9 excludes test scaffolding unless behavior changes).

## Discussion

**Confirmed by reading `mini.test`'s source, not just its docs.** A clone of `nvim-mini/mini.nvim`
(which bundles `mini.test`) showed three things worth recording because they shaped the decision:

1. `collect.emulate_busted` defaults to `true` and supplies `describe`/`it`/`setup`/`before_each`/
   `after_each`/`teardown` as globals during collection — the existing spec file's shape needed no
   rewrite beyond the `pending()` → `MiniTest.skip()` swap.
2. `pending()` is deliberately *not* emulated. `mini.test`'s own doc comment for `collect()` says
   "Use `MiniTest.skip()` instead of `pending()`" — this is a documented, intentional difference
   from busted, not a gap to work around.
3. The stdout reporter used automatically under `--headless` already exits nonzero on any failing
   case (`silent! <n>cquit`), with no extra flag needed — plenary's invocation needed the entire
   `PlenaryBustedDirectory` command spelled out to get this right.

**`mini.test` over a custom `nvim -l` harness.** A zero-dependency harness built on Neovim's
built-in script mode (`nvim -l`) plus bare `assert`/`vim.deep_equal` was considered, and fits ADR
01's dependency-minimization stance most literally. Rejected because it trades a one-time migration
cost for an ongoing one: this project would then own the nested-describe structure, hook semantics,
and directory-wide runner that `plenary.nvim` and `mini.test` both already provide and maintain.
That is a worse trade than accepting one more test-only dependency, the same category `plenary.nvim`
already occupied.

**`mini.test` over `busted` + `nlua`.** Running real Busted under `nlua` (a Lua interpreter with
Neovim's API shimmed in) is closer to "the real thing" than either plenary's or `mini.test`'s
busted-style emulation. Rejected: `nlua`'s setup path goes through LuaRocks, which is a heavier
dependency chain than this project accepts anywhere else — the CLI side rejected comparable
weight when choosing `roxmltree` over alternatives, per ADR 01.

**File rename (`*_spec.lua` → `test_*.lua`) over a `find_files` override.** `mini.test`'s default
`collect.find_files` globs `tests/**/test_*.lua`, not `*_spec.lua`. Keeping the old naming would
need a permanent `collect.find_files` override passed to every `setup()`/`run()` call. Renaming the
one file that exists today costs nothing and matches what other `mini.test` consumers' tooling
(editor integrations, doc generators keyed on the convention) expect to find, so there is no reason
to carry a config override fighting the library's own default indefinitely.

**No polyfill for `pending()`.** Defining a global `_G.pending` shim in `minimal_init.lua` that
forwards to `MiniTest.skip()` would have preserved the exact wording CLAUDE.md §8 used to document
("`pending()` placeholders... in Lua"). Rejected as one more piece of bespoke glue standing between
spec files and the library's documented interface, for a cost of touching one call per placeholder
case. CLAUDE.md §8 now says `MiniTest.skip()` directly.

## Consequences

- **MVP-25 through MVP-34** write their real specs as `test_*.lua` files using `MiniTest.skip()` for
  any case they can't complete yet, per the reworded `docs/todos.md` entries.
- **V1-03 through V1-06, V2-01, V2-04** inherit the same reworded test plans; no behavioral change
  to what those issues must cover, only which harness proves it.
- `.github/workflows/ci.yml`'s `nvim` job now depends on `nvim-mini/mini.test` instead of
  `nvim-lua/plenary.nvim` for its one external clone.
- If `mini.test` itself is ever deprecated, the same "migrate while there's nothing real to
  rewrite" argument no longer applies once MVP-25 onward lands — a future migration would carry
  real cost, unlike this one.
