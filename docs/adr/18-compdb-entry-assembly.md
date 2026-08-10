# ADR 18 — Assembling `compile_commands.json` Entries

- **Status:** Accepted
- **Date:** 2026-08-10
- **Issue:** [MVP-16 — Build compilation database entries](../todos.md#compilation-database) (`feat/compdb-build`)
- **Affects:** `cli/src/compdb.rs`, [engineering_spec.md](../engineering_spec.md) ("Compilation
  database generation"), [ADR 12](12-clcompile-flag-extraction.md), MVP-17, MVP-22

## Problem

MVP-12 (ADR 12) filled in `compdb::build`'s input: every `FileCompilation` arrives with an absolute
`file`, absolute `include_paths` ordered project-first, `defines` without their `/D`, and `flags`
already in MSVC form and deterministically ordered, ending with `AdditionalOptions`. `compdb::build`
itself was still `todo!()` — the two halves of the system (MSBuild evaluation and toolchain
discovery, MVP-15) had never been combined into an actual `CompileCommand`. Three things needed
settling: the exact shape of the `arguments` vector, where the toolchain's system includes join the
project's own, and how a file compiled by two different projects is handled.

## Decision

```rust
pub fn build(projects: &[EvaluatedProject], toolchain: &Toolchain) -> Result<Vec<CompileCommand>>;
```

- **`arguments` begins with `["clang-cl", "--driver-mode=cl"]`**, then project `/I`s, then
  toolchain `/I`s, then `/D`s, then `flags` unchanged, then the file path as the trailing argument.
- **`/I<path>` and `/D<define>`, no space**, matching the project's established `cl` flag dialect.
- **Toolchain include paths are appended after project include paths**, matching `cl`'s own search
  order — a project header wins a same-name tie against a system one.
- **`flags` is carried through unmodified**, in the order `msbuild::flags::compile` already settled.
- **`directory` is the owning project's directory** (`EvaluatedProject.path.parent()`).
- **No cross-project deduplication.** A file compiled by two different projects produces two
  `CompileCommand` entries, one per project, each with that project's own flags.
- **No sorting inside `build`.** It iterates `projects` and each project's `files` in the order it
  receives them and does nothing else to reorder them.

## Discussion

**The driver invocation repeats itself, deliberately.** `clang-cl` as the executable name already
implies clang-cl mode; `--driver-mode=cl` restates it. The redundancy was kept anyway: it costs
nothing, and it makes the entry self-describing without a reader having to know that clangd infers
mode from an executable's basename. `compdb.rs`'s own module doc and the glossary's `clang-cl` entry
already committed to spelling out `--driver-mode=cl`, so this closes that promise rather than opening
a new one.

**The file path trails the argument vector, not just sits in the separate `file` field.**
`compile_commands.json` readers — clangd included — treat `arguments` (or `command`) as a real,
parseable command line; a JSON compilation database entry that never mentions the file it compiles in
its own argument list is not one `cl` or `clang-cl` could actually run. `CLAUDE.md` §2 already commits
to the `arguments` array over a `command` string specifically to avoid re-parsing/quoting issues, so
the array has to be complete on its own.

**Toolchain includes join *after* project includes, not merged or deduped against them.** This was
the issue's own requirement, and it's also just correct: `cl` searches project-declared directories
before its own system ones, so a project vendoring, say, its own `<windows.h>` shim would silently
stop working if system paths won a tie. No attempt is made to deduplicate an `/I` that happens to
match a toolchain path — an unlikely coincidence, and `cl` tolerates a repeated `/I` harmlessly.

**No cross-project dedup, only within-project (already MVP-12's job).** A shared header compiled as
`ClCompile` from two different `.vcxproj` files is a real, if unusual, MSBuild pattern (e.g. a
`common.cpp` `#include`d by two libraries with different macro configurations). Deduplicating here
would silently pick one project's view of the file and discard the other's — exactly the kind of
plausible-but-wrong output CLAUDE.md §3 warns against. `compile_commands.json` readers are expected
to tolerate multiple entries for the same `file`; clangd itself does.

**No sorting, because none is needed.** Every input structure between `.vcxproj` parsing and this
function is `Vec`-based, and ADR 12 already made flag order stable end to end for exactly this
reason ("flag order stable across runs, since `Metadata` is a `HashMap` inside"). `build` iterating
its input in order is sufficient for the issue's "deterministic ordering across runs" requirement, so
long as the caller passes `projects` in a stable order — which it already does, since it comes from
iterating a `Vec<SolutionProject>` derived from `.sln` parse order, never a hash-keyed collection.

**Two options considered and rejected for argument order:**

- *Interleaving `/I`, `/D`, and `flags` in `ClCompile`'s own metadata order*, rather than grouping
  by kind. Rejected: nothing in MSBuild specifies a canonical order between unrelated setting kinds,
  and grouping is what a hand-written `cl` invocation looks like, which is what a human debugging a
  generated database expects to see.
- *Sorting `/D` entries alphabetically for a canonical diff.* Rejected: some preprocessor
  redefinitions are order-sensitive (later `/D` wins for a repeated macro name in `cl`), and
  `defines`'s inheritance order (sheet, then project, then file — ADR 12) already encodes that
  intentionally. Sorting would silently change which definition wins.

## Consequences

- **MVP-17 now has real input** — `compdb::build`'s output is exactly what `compdb::write` needs to
  serialize. `write` itself is untouched by this issue.
- **MVP-22** (wiring `generate` to this pipeline) can call `compdb::build` directly once it exists;
  nothing about the CLI surface or `ProjectScope`→`EvaluatedProject` mapping changes here.
- **`CompileCommand` gained `PartialEq`**, needed for the determinism test to compare whole vectors.
  `Debug`/`Clone` were already there; no other derive or field changed.
- **A `compile_commands.json` entry for a file compiled by two projects will list it twice.** Any
  consumer that assumes one entry per file (most don't — clangd doesn't) would need to pick one; this
  is unchanged by MVP-17 and worth remembering if a future issue adds explicit conflict resolution.
