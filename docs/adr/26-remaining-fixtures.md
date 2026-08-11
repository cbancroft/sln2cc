# ADR 26 — The Remaining Fixtures: `multi-project`, `props-inheritance`, `multi-config`

- **Status:** Accepted
- **Date:** 2026-08-11
- **Issue:** [MVP-24 — Remaining fixtures](../todos.md#fixtures-and-test-harness)
  (`feat/fixtures-complex`)
- **Affects:** `fixtures/multi-project/`, `fixtures/props-inheritance/`, `fixtures/multi-config/`,
  `cli/tests/generate.rs`, `cli/tests/golden/`, [ADR 05](05-sln-configuration-mapping.md),
  [ADR 25](25-fixture-simple-and-golden-harness.md)

## Problem

`fixtures/multi-project`, `fixtures/props-inheritance`, and `fixtures/multi-config` were
README-only stubs, and `cli/tests/generate.rs` carried one `#[ignore]`d placeholder
(`props_inheritance_matches_golden`) naming the gap. ADR 25 built `fixtures/simple` and the
golden-comparison harness (`support::fake_toolchain`, `support::assert_matches_golden`) and
explicitly deferred the rest here, leaving its own Consequences section reading "MVP-24 has a
harness to call, not build." The remaining question was purely what each fixture should contain to
actually exercise the hazard its name promises, per the todo entry: settings leaking across project
boundaries, property-sheet override precedence, and non-default configuration names.

## Decision

All three fixtures extend `fixtures/simple`'s precedent unchanged: no `<Import>` of the real Visual
Studio VC targets (so they evaluate under `FakeToolchain` on CI, which has no VS install), and
schema-realistic but minimal XML — real project-type GUID, `ItemDefinitionGroup`/`%()` metadata,
condition-gated `PropertyGroup`s.

**`multi-project`** — two projects, `App` (an `Application`) and `Lib` (a `StaticLibrary`), each
declaring only `Debug|x64`, each with a different `WarningLevel`, preprocessor definition, and
include directory. Deliberately no shared state between them beyond the solution file itself. The
todo's "settings don't bleed" assertion needs no separate assertion code — `assert_matches_golden`
already does an exact-content comparison, so a property leaking from one project's evaluation into
the other's would show up directly as an unexpected flag in the wrong entry.

**`props-inheritance`** — a solution-level `common.props` (sets `WarningLevel=Level3` and a base
`AdditionalIncludeDirectories`) imported by two projects, `App` and `Widget`, matching the todo
entry's own description ("a sheet imported by two projects"). `App` imports `common.props` then
`overrides.props` (redefines `WarningLevel=Level4`, imported second), and its own
`ItemDefinitionGroup` contributes a third property (`ExceptionHandling=Sync`) neither sheet
touches. `Widget` imports only `common.props`. The golden output has to show `App` at `/W4` (the
later import's value winning) while still carrying the include path `common.props` set and the
`/EHsc` `App` itself added — proving properties merge across files by key rather than a later
`ItemDefinitionGroup` replacing the whole accumulated bag — and `Widget` staying at `/W3` with no
`/EHsc`, proving a shared sheet's downstream override in one importer doesn't leak into another
importer of the same sheet.

**`multi-config`** — one solution declaring `Debug|x64` and `Release|x64`. `App` maps both
solution pairs straight onto identical project pairs. `Tool` maps solution `Debug|x64` onto
project `Debug|Win32` — a differing pair, which ADR 05's Consequences flagged as needed and
otherwise covered only by inline unit snippets — and declares no `Build.0` row for `Release|x64`
at all, so it is excluded from that configuration. The golden test
(`multi_config_matches_golden`) generates only for `Debug|x64`, matching every other fixture-driven
test's shape; `Tool`'s `Release`-exclusion is real fixture content proving the fixture *can*
exercise that state, even though this one test run doesn't touch it — ADR 05 asked for the fixture
to include it, not for a second generate invocation.

Each fixture gets exactly one `#[test]` in `generate.rs`, the same one-line shape as
`simple_solution_matches_golden`: a call into `support::assert_matches_golden`.
`props_inheritance_matches_golden`'s `#[ignore]` and `todo!()` are removed. Golden files were
produced with `MSVCLSP_UPDATE_GOLDEN=1` and reviewed by hand against `msbuild/flags.rs`'s flag
tables before committing, per ADR 25's established workflow.

## Discussion

**No harness changes.** ADR 25 built `assert_matches_golden` generically over any
`(fixture_sln, golden_path)` pair specifically so this issue wouldn't need to touch it, and it
didn't — every new test is a two-line body. Confirms that prediction was correct rather than
optimistic.

**`multi-config`'s golden test only covers one configuration.** A second test generating for
`Release|x64` was considered, to exercise `Tool`'s `Build.0`-absent exclusion directly. Rejected:
`db.rs`'s own unit tests already cover an unbuilt project being skipped at the `Store` layer in
isolation (see `generate.rs`'s `an_unbuilt_project_is_still_indexed`, which proves the *opposite*
case — a mapped-but-unchecked project is still indexed — using a hand-built temp tree rather than
a named fixture), so a second full golden file would duplicate coverage that already exists at a
cheaper layer. The fixture still declares the exclusion, since ADR 05's Consequences asked the
*fixture corpus* to have a case a reader can point at, independent of what any single test run
happens to assert on.

**Why `props-inheritance` puts the override in a second `.props` file rather than a second
`<ItemDefinitionGroup>` directly in `App.vcxproj`.** A project overriding its own earlier group
would exercise property-key merge order but not import order — the hazard this fixture is named
for and the one ADR 25 called out as mattering most ("import ordering is where subtly wrong flags
are most likely to originate"). Two sheets imported in sequence is the actual shape a real
solution's shared-config setup takes, so it's also the shape most likely to catch a real
regression.

**Project GUIDs are hand-picked, not derived from any tool.** Each fixture uses its own
`{...-0000000000NN}`-suffixed block, matching `simple`'s and `generate.rs`'s `TempProject`'s own
precedent (`PROJECT_GUID`) — readable, sortable by fixture, and collision is a non-concern since
`sln.rs` scopes GUID uniqueness per solution, not across fixtures.

## Consequences

- **The fixture corpus (`fixtures/README.md`'s table) is now fully real** — no README-only stubs
  remain, and every row's "Exercises" column names a hazard an actual `#[test]` checks.
- **`cli/tests/generate.rs` has 10 tests, 0 ignored**, up from 7 passing / 1 ignored.
- **No changelog entry.** This is test/fixture scaffolding with no user-visible behavior change;
  the tool's actual output-generation code did not change.
- **Nothing new deferred.** MVP-23 and this issue between them close every gap ADR 23, ADR 24, and
  ADR 25 each pointed at MVP-24 for.
