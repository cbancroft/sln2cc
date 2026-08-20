# ADR 24 — Wiring `generate`: Toolchain Injection and Unbuilt Projects

- **Status:** Accepted
- **Date:** 2026-08-10
- **Issue:** [MVP-22 — `generate` subcommand](../todos.md#cli-subcommands) (`feat/cmd-generate`)
- **Affects:** `cli/src/cli.rs`, `cli/src/msbuild.rs`, `cli/src/main.rs`, `cli/src/error.rs`,
  `cli/tests/generate.rs`, `cli/tests/cli_surface.rs`, [ADR 03](03-cli-surface-and-json-contract.md),
  [ADR 05](05-sln-configuration-mapping.md), [ADR 23](23-list-configs-wiring.md), MVP-23, MVP-24

## Problem

`generate` had been a hard-coded `Error::Unimplemented` since MVP-03 settled the CLI surface. Every
piece it needs already exists in isolation — `sln::parse_sln`, `msbuild::evaluate`, toolchain
discovery (`toolchain::vswhere::VsWhereToolchain`, never called from anywhere reachable),
`compdb::build`/`write`/`resolve_output_dir`, and `db::Store` — so this issue is the wiring, plus two
decisions the codebase had explicitly deferred to it:

1. ADR 05 left "whether an unbuilt project's files are still worth indexing for clangd" as
   `generate`'s call: `SolutionProject::config_for` returns a mapping either way, and only the parser
   knows the two states can be told apart.
2. A new problem surfaced while scoping this issue's own tests: `generate` needs a discovered
   `Toolchain` to evaluate anything. In production that has to be `VsWhereToolchain`, which shells
   out to `vswhere.exe` — but `engineering_spec.md`'s "Testability of toolchain discovery"
   requirement, and the fact that CI runs on a Linux runner with no Visual Studio, means something
   has to let a test substitute `FakeToolchain`. Nothing before this issue had ever called
   `ToolchainProvider::discover` from a reachable path, so the question had never been forced.

A third, narrower problem: `cli/tests/generate.rs`'s own two golden tests (`simple_solution_matches_
golden`, `props_inheritance_matches_golden`) are `#[ignore]`d placeholders naming `fixtures/simple`
and `fixtures/props-inheritance`, and per MVP-23/24 those still don't exist. ADR 23 already worked
around the identical gap for `list-configs`, but `list-configs` never reaches `msbuild::evaluate`;
`generate` is the pipeline itself, so a workaround has to reach further.

## Decision

**`ToolchainProvider` is injected, not constructed inside `generate`.** `cli::run` and the private
`generate` handler both take `toolchain: &dyn ToolchainProvider`. `main.rs` passes
`&VsWhereToolchain`; every test that needs `generate` to actually evaluate a project passes
`&FakeToolchain(..)` and calls `cli::run` directly rather than spawning the compiled binary — a
subprocess test can never substitute what runs inside the child process.

**Projects with `build: false` are still evaluated and indexed.** `generate` evaluates every project
`SolutionProject::config_for` returns `Some` for, regardless of `mapping.build`. `None` remains the
only "takes no part in this configuration" case.

**`EvaluatedProject` gains a `dependencies: Vec<PathBuf>` field**, populated from
`imports::ProjectTree::files` — computed inside `msbuild::evaluate` already and previously discarded.
`generate` unions this across every evaluated project plus the `.sln` path itself and hands the
result to `Store::record_dependencies`, so touching any file in a project's import chain — or the
solution file — invalidates the staleness cache, not just the `.vcxproj` that names it.

**`Store::set_active_config` runs on every successful `generate`, cache hit or not.** `list-configs`'s
`active` reports the last configuration a user asked to generate, not only the last one that produced
a rewrite.

**`Error::Unimplemented`, `ErrorKind::Unimplemented`, and exit code `9` are retired outright.**
Per ADR 03's own closing clause, whichever of MVP-21/MVP-22 landed second would remove them; MVP-21
only removed `list_configs`'s call site. Exit code `9` is not reserved the way `8` (`path-escape`,
ADR 10) was — nothing shipped ever depended on it as a stable contract, so it is simply unassigned
going forward rather than held open.

**The fixture-corpus gap is worked around the same way ADR 23 chose, extended for a real pipeline.**
`generate.rs`'s two golden placeholders stay untouched, still pointing at MVP-23/24. New tests for
`generate`'s actual behavior — invalid configuration, `--force`, a no-op second run, unbuilt-project
inclusion, dependency tracking including the `.sln` itself — run against a hand-authored `TempProject`
tree local to `generate.rs`: a real, minimal `.vcxproj` and `.cpp` file (no imports needed, matching
`msbuild.rs`'s own unit-test project XML), evaluated for real under `FakeToolchain` by calling
`cli::run` directly. The one case that needs no toolchain at all — an invalid configuration, which
fails during validation before `ToolchainProvider::discover` is ever called — is additionally
covered in `cli_surface.rs` against the real compiled binary, for the "real process exit code"
guarantee that file exists to provide.

## Discussion

**Why injection over constructing `VsWhereToolchain` inside `generate`.** The simpler signature was
the live alternative — `generate` would just call `VsWhereToolchain.discover()` itself, and `main.rs`
would stay untouched. It was rejected because it makes the pipeline's actual work untestable anywhere
but a Windows machine with Visual Studio installed: not on Linux CI, and not on a contributor's
machine without VS. `engineering_spec.md` states this requirement in so many words ("toolchain/SDK
discovery logic should be isolated behind an interface that can be faked/mocked in tests that run on
non-Windows CI runners"); this issue is simply the first place that requirement had a concrete caller
to apply to. The cost is a signature change to `cli::run`/`generate` and a one-line addition to
`main.rs` — small next to the alternative of `generate`'s core logic having no automated coverage at
all until someone runs it by hand on a Visual Studio box.

**Why unbuilt projects are indexed rather than skipped.** Matching what an actual MSBuild invocation
would compile — skip anything unchecked in Configuration Manager — was the other candidate, and it
is defensible: it is literally what "build" means. It was rejected because sln2cc's stated purpose
(CLAUDE.md §1) is giving `clangd` real language support for files MSBuild can compile, not mirroring
exactly what the last build produced. A project unchecked for one configuration is still a real,
evaluable project whose files a developer may open and expect completions for. The cost is that
`compile_commands.json` can now describe more than a literal `msbuild.exe` invocation would touch,
which is the trade CLAUDE.md's "database that generates cleanly and is wrong" warning already argues
for accepting in the other direction.

**Why `EvaluatedProject` carries its own dependency list rather than `generate` re-deriving it.**
`imports::resolve` is the only code that walks the import graph, and it already returns exactly the
list needed (`ProjectTree::files`) — before this issue, `msbuild::evaluate` computed it and threw it
away. The alternative, having `generate` call `imports::resolve` a second time itself to get the file
list, would duplicate an entire evaluation pass for no new information and risk drifting out of sync
with what `evaluate` actually read.

**Why the fixture-corpus workaround goes further than ADR 23's.** ADR 23's `TempSolution` was
sufficient for `list-configs` because that subcommand never reaches `msbuild::evaluate` — a bare
`.sln` text file was the whole input surface. `generate` cannot be tested at all without something
`msbuild::evaluate` can walk, so `TempProject` extends the same pattern (a small, local,
integration-test-only helper, per ADR 23's rejection of sharing `TempTree` across the lib/
integration-test boundary) to also write a project file. The project's XML deliberately imports
nothing, matching `msbuild.rs`'s own unit tests (`app()` in its test module) — MSBuild sample
projects do not need the real Visual Studio targets to parse and evaluate, only to reproduce the
exact flags a real build would emit, which fixture-driven golden tests (MVP-23/24) still own.

**Why exit code `9` is not reserved like `8`.** `path-escape` (`8`) was removed by ADR 10 after
having been a real, documented exit code; `9`/`unimplemented` never shipped as a contract anything
external could have depended on — the whole point of the variant was that it was temporary. Reserving
it anyway would have been consistent but not load-bearing, so this ADR does not add ceremony a
future reader would have to explain.

## Consequences

- **`generate` is now the CLI's only pipeline touching every module the crate has**: `sln`,
  `msbuild`, `toolchain`, `compdb`, and `db`, in one call. A regression anywhere in that chain now has
  a reachable integration test to catch it, not just the module's own unit tests.
- **`main.rs` grows a second production dependency edge**: it now names `toolchain::vswhere::
  VsWhereToolchain` directly, the first non-`cli`/non-`Reporter` import `main.rs` has needed. Still
  thin — one call site, one line.
- **The fixture-corpus gap itself remains unresolved, only worked around a second time.** MVP-23 and
  MVP-24 still own building `fixtures/simple` and friends for real, including golden-file coverage of
  the exact flags a genuine evaluation produces — `TempProject`'s hand-rolled XML is deliberately not
  a substitute and asserts nothing about flag *content*, only pipeline *behavior* (regenerated or not,
  which files appear).
- **`ErrorKind::ALL` shrinks to 14 entries** and exit code `9` is free but not reserved; a future
  issue may reuse it without ADR 10's constraint applying.
