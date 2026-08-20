# ADR 23 — Wiring `list-configs` and the Fixture-Corpus Gap

- **Status:** Accepted
- **Date:** 2026-08-10
- **Issue:** [MVP-21 — `list-configs` subcommand](../todos.md#solution-parsing) (`feat/cmd-list-configs`)
- **Affects:** `cli/src/cli.rs`, `cli/tests/cli_surface.rs`, [ADR 03](03-cli-surface-and-json-contract.md),
  [ADR 05](05-sln-configuration-mapping.md), [ADR 20](20-state-file-shape.md),
  [ADR 21](21-store-mutation-signature.md), MVP-22, MVP-23, MVP-24

## Problem

`list_configs` had been a hard-coded `Error::Unimplemented` since MVP-03 settled the CLI surface
(ADR 03). Nothing else needed building: `sln::parse_sln` (MVP-05/06) already returns a `Solution`
carrying `path` and a deduplicated, declaration-ordered `configurations: Vec<ConfigPlatform>`, and
`db::Store` (MVP-18/19/20) already has working `open`/`active_config`. This is simply the first
place either gets called from a reachable CLI path — `Store::open` has had no caller at all until
now.

A second, unrelated problem surfaced while scoping this issue's own test plan: its **Tests:** entry
in `docs/todos.md` calls for integration tests "against the `simple` and `multi-config` fixtures,"
but both are still README-only stubs. `docs/todos.md` explicitly assigns authoring the fixture
corpus to MVP-23 (`simple`, with its golden file) and MVP-24 (`multi-project`, `props-inheritance`,
`multi-config`, with theirs) — and MVP-10's own outcome note says so directly: "fixture-driven
integration is MVP-23 and MVP-24's, and `fixtures/` is still empty." Writing this issue's tests as
specified was not possible without either pulling fixture authorship forward out of its own issues
or testing against files that do not exist.

## Decision

`list_configs` becomes:

```rust
fn list_configs(args: &SolutionArgs, reporter: &mut Reporter) -> crate::Result<SolutionConfigs> {
    let solution = sln::parse_sln(&args.solution, reporter)?;
    let sln2cc_dir = args.root().join(".sln2cc");
    let store = db::Store::open(&sln2cc_dir)?;
    let active = store.active_config(&solution.path)?;

    Ok(SolutionConfigs {
        solution: solution.path,
        available: solution.configurations,
        active,
    })
}
```

`Store::open` is called with an immutable binding — `list-configs` only *reads* the last-used
selection; nothing in this subcommand records one. `args.root().join(".sln2cc")` is the state
file's directory directly, per ADR 20's single-file-for-every-solution shape; it is unrelated to
`compdb::resolve_output_dir`'s per-solution hashed subdirectory, which exists only for
`compile_commands.json` output and has no bearing on `list-configs`.

No new `Error`/`ErrorKind` variant was needed: `parse_sln` already surfaces
`SolutionNotFound`/`SolutionParse`, and `Store::open` already surfaces
`StateParse`/`StateVersion`/`Io`.

**The fixture gap is resolved by not using fixtures for this issue.** `cli/tests/cli_surface.rs`
gained a small `TempSolution` helper — a directory-scoped reimplementation of the shape
`cli/src/testing.rs::TempTree` already has, written locally because `TempTree` is `pub(crate)`
inside the lib crate and unreachable from the separate `cli/tests/` integration-test crate — that
writes a minimal, hand-authored `.sln` (and, for one test, a hand-written `.sln2cc/state.json`) at
test time. The issue's Tests: block is satisfied in substance — exact-JSON assertions against a real
parsed solution, a real `Store` round trip, a real missing-solution error path — without touching
`fixtures/` at all.

## Discussion

**Why not author minimal `fixtures/simple` and `fixtures/multi-config` files here instead.** That
was the live alternative, and it would have satisfied the issue's Tests: block literally. It was
rejected because `fixtures/` serves two purposes per its own README — automated golden-file
comparison *and* manual clangd verification — and MVP-23/24 are scoped around getting both right:
real (if trivial) source files a project actually compiles, and golden `compile_commands.json`
output `FakeToolchain` can reproduce byte-for-byte. `list-configs` never reaches `msbuild::evaluate`
at all, so a fixture built to satisfy it would be by definition too minimal for what MVP-23/24 need
it to become, and whoever does that work next would face either extending an under-scoped fixture
mid-issue or discovering it doesn't fit and starting over. A throwaway `.sln` costs nothing and
commits to nothing.

**Why `TempSolution` is reimplemented rather than exposing `TempTree` from the lib crate.** Making
`TempTree` `pub` (or `#[cfg(test)]`-conditionally `pub`) so `cli/tests/` could reuse it was
considered. `TempTree` also carries an MSBuild-shaped `project()` helper this issue has no use for,
and CLAUDE.md §5's "prefer the standard library" posture already accepts small hand-rolled
duplication elsewhere in this crate (`db.rs`'s `relative_to`, `msbuild::paths::normalize`) over
taking on a shared abstraction for one caller. `cli/tests/cli_surface.rs`'s own doc comment already
frames these tests as reaching what unit tests cannot — a second, differently-scoped helper is a
smaller cost than blurring that boundary by exporting test-only lib internals across the crate
split.

**Why `Store::open` isn't given `&mut` here.** `list-configs` never calls `set_active_config`;
nothing picks a configuration from this subcommand; ADR 21 already settled that `active_config`
itself stays `&self`. Following that ADR to its conclusion here rather than reopening it is a
non-decision, but it is the reason `list_configs` needs no `mut store` binding — worth naming so a
future reader does not wonder why `generate` (MVP-22, which presumably does set the active
selection) needs one and this does not.

## Consequences

- **MVP-22 now owns the only remaining `Unimplemented` call site.** Per ADR 03, whichever of
  MVP-21/MVP-22 lands second retires the `Error::Unimplemented` variant, `ErrorKind::Unimplemented`,
  and exit code `9` entirely. That is MVP-22's to do, not this issue's — `generate` still returns
  the placeholder today.
- **`SolutionRecord::known_configs` (ADR 20) remains unpopulated.** Reserved since MVP-18 with no
  owning accessor; ADR 20 already flagged this issue by name as the candidate that might claim it
  and it did not — `list-configs` reads `Solution::configurations` fresh from `sln::parse_sln` on
  every invocation rather than caching it, so the field stays an open gap for a future issue to
  either claim or drop.
- **This is the first `changelog.md` entry for CLI behavior since the surface was drafted.**
  MVP-16/17/18/19/20 all skipped the changelog on the grounds that nothing was reachable from a real
  invocation yet (see their own Outcome notes and ADR 20's Consequences); `list-configs` breaks that
  streak because it is now genuinely reachable and does something a user can observe.
- **The fixture-corpus gap itself is unresolved, only worked around.** MVP-23 and MVP-24 still own
  building `fixtures/simple` and `fixtures/multi-config` for real; nothing here changes their scope
  or reduces their remaining work.
