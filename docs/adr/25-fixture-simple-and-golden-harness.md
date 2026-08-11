# ADR 25 — `fixtures/simple` and the Golden-Comparison Harness

- **Status:** Accepted
- **Date:** 2026-08-11
- **Issue:** [MVP-23 — `simple` fixture and golden test harness](../todos.md#fixtures-and-test-harness)
  (`feat/fixture-simple`)
- **Affects:** `fixtures/simple/`, `cli/tests/support/mod.rs`, `cli/tests/generate.rs`,
  `cli/tests/golden/simple.json`, [ADR 23](23-list-configs-wiring.md),
  [ADR 24](24-generate-wiring.md), MVP-24

## Problem

`cli/tests/generate.rs` carried two `#[ignore]`d placeholders, `simple_solution_matches_golden`
and `props_inheritance_matches_golden`, both naming fixtures that were still README-only stubs.
ADR 23 and ADR 24 each worked around the gap for their own issue rather than closing it — both
explicitly hand the real work to MVP-23 (`simple`) and MVP-24 (the remaining three). MVP-23's own
scope is exactly that: author `fixtures/simple` for real, and build the golden-comparison harness
MVP-24 will also need.

Two problems had to be solved together rather than separately, because the fixture's own shape
depends on how the harness will exercise it:

1. **A real `.vcxproj` a real Visual Studio writes unconditionally imports
   `$(VCTargetsPath)\Microsoft.Cpp.Default.props`, `...\Microsoft.Cpp.props`, and
   `...\Microsoft.Cpp.targets`.** Those are not optional imports MSBuild tolerates missing; per
   MVP-10's own outcome, a missing *required* import is an error. `FakeToolchain`'s
   `vc_targets_path` (ADR 24) points nowhere real, and `engineering_spec.md`'s CI Expectations
   run `cargo test` on a Linux runner with no Visual Studio installed at all — so a fixture that
   imports the real targets unconditionally cannot evaluate successfully anywhere this suite
   actually runs.
2. **`compile_commands.json` entries carry absolute paths**, and those paths depend on where the
   repository happens to be checked out and which OS produced them (`msbuild::paths::from_msbuild`
   already makes *evaluation* itself cross-platform, but a literal `C:\Users\...` vs `/home/...`
   still ends up in the output text). A byte-identical checked-in golden file needs that variance
   normalized away, on both axes, without losing the ability to review a real diff when a flag
   legitimately changes.

## Decision

**`fixtures/simple`'s `.vcxproj` does not import the real VC targets**, extending the precedent
ADR 24 already set for `generate.rs`'s own `TempProject` and for `msbuild.rs`'s unit-test project
XML: MSBuild sample projects do not need the real Visual Studio targets to parse and evaluate.
"Genuinely valid" (the todo entry's own wording) is read here as *schema-realistic* — correct
project-type GUID, `ItemDefinitionGroup`/`%()` metadata, a `Debug|x64`-conditioned property group —
not *devenv-openable*. MVP-23's actual goals, a golden `compile_commands.json` and a fixture real
enough for manual `clangd` verification (CLAUDE.md §8), don't need the latter, and nothing in this
codebase's pipeline ever shells out to a real `msbuild.exe`/`devenv.exe` to check.

The fixture (`fixtures/simple/`) is one project, one `Debug|x64` configuration, three real files —
`App/src/main.cpp` and `App/src/greet.cpp` compiling against `App/include/greet.h` — with enough
`ItemDefinitionGroup` surface (`WarningLevel`, `LanguageStandard`, `RuntimeLibrary`,
`ExceptionHandling`, a preprocessor definition, an include directory) to make the golden file worth
comparing against.

**The harness lives in `cli/tests/support/mod.rs`**, pulled into `generate.rs` (and, later,
whatever MVP-24 adds) via `mod support;`. A subdirectory carrying its own `mod.rs` is the standard
Rust idiom for code shared *within* one integration-test crate — cargo does not compile it as a
fourth top-level test binary the way a bare `tests/support.rs` would risk being confused for. This
is a different concern from ADR 23's rejection of exposing `cli/src/testing.rs::TempTree` to
`cli/tests/`: that was about not leaking `pub(crate)` lib internals across the lib/integration-test
crate boundary. Sharing a helper module within `cli/tests/` itself crosses no such boundary.

`support::assert_matches_golden(fixture_sln, golden_path)`:

- Runs `generate` for `Debug|x64` under a fixed `support::fake_toolchain()` (moved out of
  `generate.rs`'s own former private helper, byte-identical to it), pointing `--root` at a
  throwaway temp directory rather than the fixture's own directory — `generate` writes `.msvclsp/`
  under `SolutionArgs::root()`, and running the suite must never leave a generated artifact inside
  the checked-in `fixtures/` tree.
- Normalizes the written `compile_commands.json` before comparing: every JSON-escaped path
  separator (`\\`, the only place this crate's own tests ever produce a backslash) becomes `/`,
  and the fixture directory's own absolute path is replaced with the literal token `<FIXTURE>`.
  The result is byte-identical across machines, checkout locations, and Windows vs. Linux.
- Under `MSVCLSP_UPDATE_GOLDEN=1`, writes the normalized result to `golden_path` instead of
  comparing, still under `FakeToolchain` so the rewritten file stays reproducible. This is how
  `cli/tests/golden/simple.json` was produced — generated once, then read by hand before
  committing.

**This resolves `engineering_spec.md`'s open question** on the fixture corpus's source and scope:
hand-authored, minimal but schema-realistic, with no real Visual Studio install anywhere in the
automated pipeline.

## Discussion

**Why not stub `Microsoft.Cpp.*.props/.targets` files on disk instead of omitting the imports.**
Considered and rejected (raised as an open question to the user before implementation). Keeping
the real, unconditional `<Import>` lines and having the harness create empty stub target files at
`FakeToolchain`'s `vc_targets_path` would be closer to literally "genuinely valid." It was rejected
because nothing in the pipeline currently reads anything from those files beyond their existence —
`$(VCTargetsPath)` itself is already exercised by `reserved.rs`'s own unit tests (ADR 11) — so the
stubs would exist purely to satisfy an import resolution step, adding harness complexity for a
property nothing checks, and every fixture MVP-24 adds would need the same stubs kept plausible as
the corpus grows. The precedent ADR 24 already set for `TempProject` made the simpler call once;
making a different call here for a fixture with the same actual testing needs would need its own
justification, and there isn't one.

**Why textual normalization instead of comparing parsed JSON with path-aware equality.**
Deserializing both sides into `Vec<CompileCommand>`-shaped values and comparing with a normalizing
`PartialEq` was the other live alternative. It was rejected because the golden file's value is
partly as a *readable diff* when something changes — a structural comparison would still need to
render *some* text for a failing assertion to be useful, and doing that rendering only on failure
duplicates the normalization this ADR already needs on the success path. Text substitution keeps
one code path for both.

**Why per-platform golden files were not considered seriously.** Checking in `simple.windows.json`
and `simple.linux.json` would sidestep the separator problem without any normalization logic at
all. Rejected immediately: it doubles the files every future fixture needs, and the two would drift
apart silently the first time someone updated one without the other, defeating the point of a
golden file as a single source of truth for "what does this fixture produce."

**Why `--root` is a throwaway temp directory rather than `fixtures/simple` itself.** The simpler
call — run `generate` with no `--root` override, letting `.msvclsp/` land next to the fixture the
way a real invocation would — was considered. Rejected because `.msvclsp/` is gitignored but real:
running `cargo test` would litter the checked-in fixture tree with generated output on every run,
and an interrupted test run could leave it there for a developer to stumble on later, wondering
whether it was meant to be committed.

## Consequences

- **MVP-24 has a harness to call, not build.** `support::fake_toolchain()` and
  `support::assert_matches_golden` are written generically over any `(fixture_sln, golden_path)`
  pair; unignoring `props_inheritance_matches_golden` and adding tests for `multi-project` and
  `multi-config` should need only fixture authorship and one call each, not new harness code.
- **`engineering_spec.md`'s fixture-corpus open question is closed.** See the Decision section
  above; the Testing Strategy section now states the settled shape directly rather than leaving it
  open.
- **The `<FIXTURE>` substitution is a plain substring replace, not a path-aware one.** If a future
  fixture's own absolute path happened to appear as a substring of unrelated output (implausible
  given fixture paths always live under a `fixtures/<name>` directory unlikely to collide with a
  flag or define), the replacement would still fire. Not a real risk today; worth noting if a
  future fixture ever needs a name matching something else in its own output.
