# ADR 10 — Import Resolution as MSBuild's First Evaluation Pass

- **Status:** Accepted
- **Date:** 2026-08-05
- **Issue:** [MVP-10 — Resolve `.props`/`.targets` imports](../todos.md#msbuild-evaluation) (`feat/msbuild-imports`)
- **Affects:** `cli/src/msbuild/imports.rs`,
  [engineering_spec.md](../engineering_spec.md) ("CLI Binary — Internal Modules"),
  [ADR 02](02-structured-error-type.md), [ADR 04](04-sln-project-enumeration.md),
  [ADR 06](06-diagnostic-collection.md), [ADR 07](07-vcxproj-reader-ir.md),
  [ADR 08](08-eager-macro-expansion.md), [ADR 09](09-condition-evaluation-subset.md),
  MVP-11, MVP-12, MVP-13, MVP-36

## Problem

MVP-07 reads one file, MVP-08 expands its macros, MVP-09 evaluates its conditions. Nothing walks
the graph, and a `.vcxproj` is not self-contained — nearly every flag arrives through an `<Import>`.

The shape of the answer is forced by one fact: **import paths are macro-expanded and
condition-gated**. `<Import Project="$(VCTargetsPath)\Microsoft.Cpp.props" />` cannot be resolved
without the properties assigned above it, and `<Import ... Condition="Exists(...)" />` must not be
resolved at all when its condition is false. So there is no pre-pass that collects files and a
later pass that evaluates them. Resolution *is* property evaluation.

That makes this issue MSBuild's first evaluation pass, and raises three questions the issue text
answers differently than MSBuild does.

## Decision

`msbuild/imports.rs` walks the graph in document order, splicing each imported file in where its
`<Import>` sits, and returns:

```rust
pub struct ProjectTree {
    pub files: Vec<PathBuf>,       // evaluation order, root first; also the visited set
    pub deferred: Vec<Deferred>,   // item groups, spliced in true evaluation order
    pub properties: Properties,    // as the whole graph left it
}
```

- **A file already imported is skipped and warned about**, MSBuild's MSB4011 rule, covering both
  diamonds and cycles.
- **Paths are normalised lexically, not refused.** `..` is followed.
- **`ItemGroup` and `ItemDefinitionGroup` are collected with their conditions unevaluated**, for
  MVP-12 to settle in a later pass.
- **MVP-08 and MVP-09 findings become `Reporter` warnings here.**
- Depth is capped at 64 as `Error::RecursionLimit`; a missing import that a true condition asked
  for is `Error::MissingImport`.

## Discussion

**One rule for cycles and diamonds.** The issue text asks for a diamond test and a cycle *error*,
which implies two mechanisms: a global visited set for the former and a stack for the latter.
MSBuild has one. It imports a file once per project and logs MSB4011 on a repeat, which makes a
diamond evaluate its shared sheet once and makes a cycle terminate on its own.

Matching MSBuild was chosen because the diamond case is where it matters and diamonds are
everywhere: `Microsoft.Cpp.props` is reachable by several routes in an ordinary project, and
evaluating a sheet twice re-runs every `<X>$(X);more</X>` inside it. Appending twice is not a
subtle difference from what MSBuild produces — it is a different include path list.

The cost is that `Error::ImportCycle` is left with no producer, and that a genuine cycle — which
MSBuild merely warns about — is no longer distinguishable from an ordinary repeat import. The
alternative, checking the current import stack first and erroring, would keep the variant alive and
satisfy the issue's test, at the price of failing a project that builds. Fidelity won, as it did in
ADR 08 for the same reason.

The depth cap survives this, because it guards something the duplicate rule does not: a generated
chain of *distinct* files deep enough to exhaust the stack.

**Paths are normalised, not validated.** The issue asks that "paths that escape via `..` traversal
from untrusted input should be validated", while also acknowledging that imports into the Visual
Studio installation are legitimate. Those two cannot both be honoured, because there is no root
left to escape from: a legitimate import is either absolute (the toolchain) or `..`-relative (the
shared sheets directory one level up, which is how every multi-project solution is laid out).
Refusing either breaks real input, so `Error::PathEscape` gets no producer here either.

What normalisation is actually for is the duplicate rule: `sub\..\shared.props` and `shared.props`
must count as one file, or the rule silently stops working the moment a sheet in a subdirectory
reaches back up. It is done lexically rather than with `std::fs::canonicalize` for three reasons —
canonicalize requires the file to exist, which is the thing being tested a line later; it resolves
symlinks, collapsing two paths a user thinks of as different; and on Windows it returns a `\\?\`
verbatim path nobody recognises in a diagnostic.

Comparison is case-insensitive on Windows, where `Microsoft.Cpp.props` and `microsoft.cpp.props`
are one file. This is a `cfg!(windows)` in a two-line helper rather than behaviour behind the
toolchain trait, because it is a property of the filesystem rather than of the toolchain.

The residual exposure is that a hostile project file can make the tool *read* an arbitrary path.
It is read as XML and never executed, a file that is not a project fails as `project-schema`, and
ADR 06's `display_path` already keeps absolute paths out of default-verbosity stderr. That is
judged acceptable against breaking every real solution.

**Returning pass 1, not a path list.** The issue says "return the full file list in evaluation
order". A bare `Vec<PathBuf>` was rejected: the property bag falls out of the walk whether or not
it is returned, and discarding it would force MVP-12 to redo the entire walk to learn what the
paths meant.

The more interesting rejection is the middle option — return `Vec<ProjectFile>` in pre-order and
let MVP-12 walk it. That loses the *interleaving*. A `.vcxproj` writes its `ItemDefinitionGroup`s
between `Microsoft.Cpp.props` and `Microsoft.Cpp.targets`, so grouping elements by file puts the
root's item definitions ahead of the imported sheet's, where MSBuild puts them after. That is wrong
on the ordinary layout of every Visual Studio project, not on an edge case. So the deferred
elements are spliced into one stream, each carrying the file it came from.

Their conditions are deliberately **not** evaluated. MSBuild settles item definitions and items in
a later pass, against the property values evaluation *ended* with rather than the ones in scope
where the element was written. Evaluating them here would answer a different question, and it is
the question MVP-12 has to answer anyway once it has the final bag.

The cost is that `ProjectTree` is three fields where the issue asked for one, and that `Deferred`
is a type whose only job is to pair an element with its file.

**The findings bridge lands here.** ADR 08 and ADR 09 both deferred warning, on the grounds that
neither `expand` nor `condition_holds` knows what file or line its input came from. This pass is
the first code that holds both, so the drain into `Reporter::warn_at` lives here.

It will be noisy, and knowingly so. `Condition="'$(X)' != ''"` is how a property sheet legitimately
tests for an absent property, so undefined-property findings will outnumber everything else in the
channel — exactly what ADR 09 predicted and what ADR 06's open question about a second `Severity`
level was about. Reporting them anyway was chosen over dropping them, because a dropped finding is
the silent failure CLAUDE.md §3 calls the worst outcome, and because MVP-13 cannot tune a signal it
never receives. The wording is provisional and MVP-13 owns it, the same arrangement ADR 07 made for
the reader's own warnings.

**The condition is evaluated before the path is expanded.** The reverse order reads more naturally
— resolve the target, then decide whether to take it — and is wrong: a false condition is how a
sheet guards an optional import, so resolving first would report a missing file the project
explicitly allows for. Every Visual Studio project guards `Microsoft.Cpp.$(Platform).user.props`
exactly this way.

**`properties` is a parameter.** MVP-11 seeds the reserved properties, and this function should not
have to know what they are. An empty bag is a valid starting point today; `$(VCTargetsPath)` simply
expands to nothing, the import resolves to an empty path, and it is skipped with a warning rather
than resolving to the filesystem root.

## Consequences

- **MVP-11 seeds the bag it passes in**, and `MSBuildThisFileDirectory` will have to vary per file
  during the walk, which means that issue changes this module rather than only calling it.
- **MVP-12 is pass 2/3**: it evaluates each `Deferred` element's condition against
  `ProjectTree::properties` and builds flags from what survives.
- **MVP-13 inherits the warning wording** and the noise problem, and will need ADR 06's second
  `Severity` level or a filter before undefined-property findings are useful.
- **`Error::ImportCycle` and `Error::PathEscape` are removed**, along with `ErrorKind::ImportCycle`,
  `ErrorKind::PathEscape`, the wire strings `import-cycle` and `path-escape`, and the `join_paths`
  helper that only `ImportCycle`'s `Display` used. Both variants were written ahead of the code
  that would raise them, and this ADR is where it became clear that code is never going to exist:
  the duplicate rule dissolves cycles and there is no root to escape from. Leaving them would
  document a design that was not built and give the plugin two kinds it can never receive.

  `ErrorKind::ALL` drops from sixteen to fourteen. **Exit code `8` is retired rather than
  reassigned** — reusing it would change what a shell script's `exit 8` means for failures that did
  not themselves change — so the codes now run 1, 3, 4, 5, 6, 7, 9, and a test asserts nothing
  reclaims 8. This supersedes ADR 04's consequence that "MVP-10 owns path-escape validation".
- **MVP-36 is new**: ADR 09 handed this module the question of whether an unsupported condition
  stays fatal, and it cannot be answered here. The signal it needs — whether a file came from the
  user's tree or from the toolchain — does not exist until MVP-15 supplies the Visual Studio paths,
  and today `$(VCTargetsPath)` is undefined so those imports are never even reached.
- **New open question:** nothing bounds the *number* of files a graph may reach, only its depth. A
  wide generated tree is bounded by the filesystem alone.
