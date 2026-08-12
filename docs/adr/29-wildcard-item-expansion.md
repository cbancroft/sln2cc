# ADR 29 — Wildcard `ClCompile` Item Expansion

- **Status:** Accepted
- **Date:** 2026-08-12
- **Issue:** [MVP-37 — Expand wildcard `ClCompile` includes](../todos.md#msbuild-evaluation)
  (`feat/item-globbing`)
- **Affects:** `cli/src/msbuild/glob.rs` (new), `cli/src/msbuild/items.rs`,
  `cli/src/msbuild.rs`, [engineering_spec.md](../engineering_spec.md) ("CLI Binary — Internal
  Modules", "Diagnostics"), [ADR 12](12-clcompile-flag-extraction.md),
  [ADR 13](13-diagnostic-severity-and-wording.md)

## Problem

`<ClCompile Include="src\*.cpp" />` is legal MSBuild that Visual Studio itself never writes —
it shows up in hand-maintained `.vcxproj` files and the shared-`.props` pattern a multi-project
solution uses to declare "everything under this directory." MVP-12 found it already producing
the worst available outcome: a project came out with no source files and no explanation. That
issue closed the silent half of the problem — warn and contribute nothing — deliberately
without building a globber, and ADR 12 named this issue as where the debt would get paid. ADR
13 then spent real design effort on the *interim* wording: a three-variant `Wildcard` enum
(`Loses`/`Keeps`/`Harmless`) existed only to describe what an *unexpanded* wildcard cost,
depending on whether it appeared in an `Include`, a `Remove`, or an `Exclude` paired with a
literal or wildcard `Include`.

Expanding wildcards for real means the crate's evaluation passes touch the filesystem for the
first time. Every module up to this point — `sln.rs`, `msbuild/eval.rs`, `msbuild/imports.rs`
(past reading the files it imports), `msbuild/items.rs` — is pure evaluation over an already-
parsed graph; `items.rs`'s own `settle_does_not_read_the_filesystem` test asserts exactly that
for literal specs. A `*`/`?`/`**` spec breaks that assumption on purpose: answering "what does
this name" requires `read_dir`.

## Decision

**A new module, `msbuild/glob.rs`, owns the walk.** `pub(crate) fn expand(dir: &Path, spec:
&str) -> Result<Vec<PathBuf>>` takes an already-resolved base directory and one `;`-delimited
spec element, and returns the absolute, sorted paths it names. `items.rs::specs()` calls it for
any spec element containing `*` or `?`; a literal element still goes through `paths::resolve`
exactly as before.

**Matching supports `*` and `?` within one directory segment, plus MSBuild's `**` recursive
form**, via a depth-first walk over real directory entries — segments that carry no wildcard
character are joined onto the base directly, without a `read_dir` call, so a wildcard whose
*literal* prefix directory doesn't exist costs nothing extra.

**A directory a wildcard walk passes through that doesn't exist expands to nothing; any other
I/O failure is a real `Error::Io`.** Matching MSBuild's own behavior for the first case, and
CLAUDE.md §3 ("never silently guess... a subtly wrong compilation database is worse than a loud
failure") for the second — permission errors and the like are not the case this issue was asked
to make quiet.

**Results are sorted before `expand` returns.** `read_dir` order is filesystem order, which a
golden file — and a test asserting exact output across two runs — cannot depend on.

**Matching is case-sensitive**, following whatever `read_dir` actually returns, with no
case-folding layer for Windows' case-insensitive filesystem.

**The `Wildcard` enum, `include_item`'s `exclusion` classification, and every severity-split
wildcard test are deleted, not kept as dead branches.** All three of `Wildcard`'s cases existed
to describe what an *unexpanded* wildcard cost; once wildcards expand for real, `Exclude` and
`Remove` subtract real matched paths the same way a literal spec always could, and no
classification survives to do anything. An unmatched wildcard is now diagnosed exactly like a
literal spec naming a file that doesn't exist: not at all — `settle_does_not_read_the_filesystem`
already established that a literal miss is silent, and treating the two inconsistently would be
the new bug.

## Discussion

**Inlining the walk into `items.rs::specs()` was rejected.** `items.rs` is already the densest
module in the crate — its own `Scope` struct exists, by its doc comment, "because threading
[three parameters] individually pushed `include_item` past what clippy will accept." Filesystem
globbing is also a clean, independently testable unit with no dependency on `ProjectTree`,
`Metadata`, or a `Reporter`; splitting it out mirrors how `paths.rs` already holds the lexical
helpers both `items.rs` and `flags.rs` share; `glob.rs` is that module's filesystem-touching
sibling rather than a widening of `items.rs`'s own job.

**Swallowing every I/O error from the walk, not just `NotFound`, was considered and rejected.**
It would have been the simpler rule — "a wildcard that fails to resolve just contributes
nothing" — but it also would have made a permission problem or a genuinely broken filesystem
state indistinguishable from an empty match, which is exactly the "subtly wrong compilation
database" CLAUDE.md §3 warns against. The cost is a plausible new way `generate` can fail that
didn't exist before this issue; accepted, because the alternative is a wildcard that can go
quietly wrong in a way nothing else in the crate does.

**Naive recursive matching (`match_here(n, p) = match_here(n, p[1..]) || (!n.empty() &&
match_here(n[1..], p))` for a `*`) was rejected for the single-segment matcher**, in favor of
the standard two-pointer backtracking-position algorithm. The naive version is exponential on a
pattern with many adjacent `*` characters, and a `.vcxproj` is untrusted input under CLAUDE.md
§5 — "deep recursion must ... fail gracefully rather than hang." The chosen algorithm has no
blowup case and costs nothing in readability.

**Case-insensitive matching on Windows was considered and deferred rather than built.** VS
itself is case-preserving and most real specs are written to match real casing, so the gap is
narrow; building case-folding for one platform only, with no test coverage possible from Linux
CI, was judged not worth it for the MVP. Flagged in `glob.rs`'s doc comment as a call worth
revisiting rather than a silent decision.

## Consequences

- **`Exclude` does real work for the first time.** ADR 12 flagged this as the one case where an
  unexpanded `Exclude` mattered; `Include="src\*.cpp" Exclude="src\*.generated.cpp"` now
  actually subtracts the matched `.generated.cpp` files from the database, rather than leaving
  them in for clangd to index as translation units the real build never compiles.
- **The three wildcard rows in engineering_spec.md's diagnostics table are removed outright**,
  not reworded — there is no wildcard-specific diagnostic left. An unmatched wildcard is silent,
  the same as an unmatched literal spec always was.
- **`msbuild/` gains its first filesystem-touching evaluation code.** Every module before this
  operated purely over the parsed graph; `glob.rs` is now a documented, deliberate exception,
  not an accidental one — worth remembering if a future issue is tempted to add another one
  without the same care around error handling and determinism.
- **Closes the engineering_spec.md open question** ("whether wildcard `Include` attributes are
  ever *expanded*") outright — see the spec's now-updated wildcard paragraph.
- **ADR 13's severity split stays on record even though its wildcard rows are moot.** The house
  rule is that a superseded ADR stays put with the resolved question marked in place, not
  rewritten — see ADR 13's own file for the pointer here.
