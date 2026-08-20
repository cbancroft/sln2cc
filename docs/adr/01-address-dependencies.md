# ADR 01 — Address CLI Dependencies

- **Status:** Accepted
- **Date:** 2026-07-28
- **Issue:** [MVP-01 — Select and wire up CLI dependencies](../todos.md#mvp) (`chore/cli-dependencies`)
- **Affects:** `cli/Cargo.toml`, `cli/Cargo.lock`, `.github/workflows/ci.yml`, `cli/src/db.rs`,
  [engineering_spec.md](../engineering_spec.md) ("Dependency Decisions"), MVP-04, MVP-18

## Problem

The crate was deliberately dependency-free at the point MVP-01 was picked up, with candidates
parked as comments in `cli/Cargo.toml`. Six slots were open — XML parsing, argument parsing,
serialization, error handling, persistence, and logging — and every downstream issue in the MVP
depended on them: MVP-02 can't define an error enum without knowing whether errors carry a
machine-readable kind, MVP-07 can't parse a `.vcxproj` without an XML strategy, and MVP-18 can't
define a state-file shape if the state might be a SQLite database.

Two constraints made this more than a shopping trip. The repository's standing policy is to prefer
the standard library and treat every crate as a maintenance liability, so each slot had to justify
itself rather than be filled by default. And the crate must *build* on any OS while only *running*
on Windows, because CI lints and tests on Linux — which rules out anything whose build story
differs between the two.

## Decision

Four direct dependencies, 20 packages resolved including transitives:

| Slot | Choice |
| ---- | ------ |
| XML parsing | `roxmltree` |
| Argument parsing | `clap` (derive, `default-features = false`) |
| Serialization | `serde` + `serde_json` |
| Error handling | `thiserror` |
| Persistence | *no crate* — a JSON state file written through `serde_json` |
| Logging | *no crate* — deferred |

Two of the six slots were closed by deciding to take nothing at all. Supporting decisions: MSRV
pinned at 1.85 and recorded as `rust-version`, edition 2024, ordinary caret version requirements
with `Cargo.lock` committed, `[dev-dependencies]` intentionally empty, and a new CI job pinned to
the declared MSRV running `cargo check --all-targets --locked`.

The full reasoning for each choice lives in "Dependency Decisions" in
[engineering_spec.md](../engineering_spec.md), with a short rejection summary in `cli/Cargo.toml`
comments so it is visible at the point of temptation.

## Discussion

**`roxmltree` vs. `quick-xml`.** `quick-xml` looked like the stronger candidate on paper — it is
the more widely used crate and its serde integration promises deserializing project files straight
into structs. Both advantages evaporated on inspection. `PropertyGroup` children are arbitrary
user-defined element names (`<MyCustomInclude>`), which no derived schema can model, so generic
tree access is required regardless. And MSBuild evaluation is not a single forward pass: import
resolution interleaves with property expansion and condition evaluation, so nodes get revisited
and parents get walked — a pull parser is the wrong shape, a DOM is the right one. The throughput
argument that normally favors `quick-xml` is irrelevant at kilobyte file sizes.

The cost accepted is a lifetime one: `roxmltree` borrows from the source `&str`. Rather than
holding every file's text alive in an arena, `msbuild.rs` parses one file at a time and lowers it
into an owned IR, dropping the `Document` before moving to the next import. That constraint was
recorded in the spec because it shapes MVP-07's design.

**`thiserror` vs. `anyhow` vs. hand-written impls.** `anyhow` is the reflex choice for an
application binary, and this *is* a binary, but the plugin contract forbids it: the plugin must
distinguish "solution not found" from "unsupported MSBuild construct" from "no Visual Studio
installation" in order to map failures onto exit codes and JSON error kinds. Type erasure would
mean downcasting to recover the kind, which defeats the point. A narrower version of the argument —
"`thiserror` in the library, `anyhow` in `main.rs`" — was also rejected, since `main.rs` is
precisely where the kind-to-exit-code mapping happens. Hand-writing `Display`/`Error` stayed on the
table longest, given the no-dependencies preference and that `thiserror` earns its keep only at
scale; the expected variant count tipped it, and `thiserror` is a proc macro with no runtime
component.

**`clap` vs. hand-rolling `std::env::args`.** Three subcommands is genuinely small enough to parse
by hand, and this was the closest call of the four. The deciding factor was not the parsing but the
`--help`: the tool is invoked by hand constantly while debugging the evaluator, and discoverable
help earns its keep there. Derive also keeps the surface declarative in `cli.rs`, which supports
the standing rule that `main.rs` stays thin. `default-features = false` then drops `color`
(`anstream`, `colorchoice`) and `suggestions` (`strsim`) — styled help and fuzzy matching are close
to worthless when the primary consumer is a program.

**`rusqlite` vs. a JSON file.** This was the slot where the honest answer was "neither candidate,"
and where the platform constraint decided it rather than any aesthetic preference. On Windows there
is no system SQLite dev library to link against, so `rusqlite` effectively requires the `bundled`
feature, which compiles the SQLite C amalgamation via `cc` — making a working C toolchain a hard
requirement to build sln2cc *at all*. That is a bad trade when building from source is a live
distribution option and the plugin's users are Neovim users, not necessarily C developers. Weighed
against a build-time C toolchain requirement, the stored data is a few KB: known configuration/
platform pairs, the last-used selection per solution, and dependency mtimes.

What SQLite would have bought, and what is therefore now owed: transactional writes. Two Neovim
instances in one project root are last-writer-wins. The mitigation is that writes must be atomic
(temp file, then rename), so a *torn* file is impossible even though a *lost update* is possible.
Whether that is acceptable long term was left as a new open question rather than assumed away.
Staleness checks over a few thousand mtimes become a JSON parse rather than a query — milliseconds
at MVP scale, worth revisiting if it bites.

**`tracing` vs. `log` vs. nothing.** The requirement that appeared to demand a logging facade was
"warn on unsupported MSBuild constructs." Examining it dissolved it: those warnings are *structured
data the plugin must surface to the user*, and the plugin is forbidden from scraping human-readable
text, so they belong in the JSON contract as a `warnings` array — not on stderr where the plugin
can't reach them. What remained for a facade was developer-facing debug tracing while building the
evaluator, which no current milestone blocks on, and `eprintln!` covers the handful of human
progress messages. So: deferred, with a preference recorded for when it is wanted. `log` over
`tracing`, because spans pay off in async and long-running services, whereas the nesting context
worth having here ("evaluating `a.props`, imported by `b.vcxproj`") belongs in the error and
diagnostic payloads where the plugin can actually use it.

**MSRV and edition.** `clap` 4.6 sets the floor at 1.85 — the highest in the tree, with every other
dependency at 1.71 or below. Edition 2024 requires exactly 1.85, so taking it costs no
compatibility beyond what `clap` already imposed, and migrating while the codebase was still stubs
made the change free; the cost only grows once the evaluator exists. Worth not confusing the two
knobs: edition 2021 needs only Rust 1.56, so staying on it would have locked out nobody — but it
would have protected nobody either. The real lever for supporting older toolchains was downgrading
to `clap` 4.5, which holds MSRV 1.74 across all its patch releases. That was rejected because
sln2cc only runs on Windows, where Rust arrives via rustup rather than a distro package, so the
population stranded between 1.74 and 1.85 is small and `rustup update` fixes it.

Because every other CI job uses `stable`, the declared floor would have drifted upward the first
time someone used a newer language feature, so MSRV needed its own pinned job to mean anything.
`--locked` there also catches a stale lockfile.

## Consequences

- `cli/Cargo.toml` gains four dependencies and a rejection summary in comments. The engineering
  spec's crate-selection open question is closed and replaced by a "Dependency Decisions" section.
- **MVP-04 reshaped** from a logging-setup issue into structured diagnostic collection plus
  stdout/stderr discipline: a `Diagnostic` type accumulated during evaluation and threaded into the
  JSON output, with `-v/--verbose` human progress on stderr via `eprintln!`.
- **MVP-18 reshaped** from a SQLite schema into a JSON state-file shape, carrying a schema version
  field, create-or-load `open()` semantics, and atomic temp-file-plus-rename writes.
- SQLite references removed from `cli/src/db.rs`, both specs, and `docs/milestones.md`.
- Style edition 2024 changes `use` statement ordering, so `cargo fmt` output differs from 2021 —
  the reordering churn in this change is expected, not accidental.
- **New open question:** whether last-writer-wins state is acceptable long term, or whether two
  Neovim instances sharing a project root need real locking.
