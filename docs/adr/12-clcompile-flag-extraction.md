# ADR 12 — Settling `ClCompile` Items and Translating Them Into Flags

- **Status:** Accepted
- **Date:** 2026-08-06
- **Issue:** [MVP-12 — Extract compiler settings into flags](../todos.md#msbuild-evaluation) (`feat/clcompile-flags`)
- **Affects:** `cli/src/msbuild.rs`, `cli/src/msbuild/items.rs`, `cli/src/msbuild/flags.rs`,
  `cli/src/msbuild/paths.rs`, `cli/src/msbuild/eval.rs`, `cli/src/msbuild/imports.rs`,
  `cli/src/testing.rs`,
  [engineering_spec.md](../engineering_spec.md) ("CLI Binary — Internal Modules"),
  [ADR 05](05-sln-configuration-mapping.md), [ADR 08](08-eager-macro-expansion.md),
  [ADR 09](09-condition-evaluation-subset.md), [ADR 10](10-import-resolution.md),
  [ADR 11](11-reserved-property-seeding.md), MVP-13, MVP-15, MVP-16, MVP-37

## Problem

MVP-10 built MSBuild's first evaluation pass and deliberately stopped one pass short.
`ProjectTree::deferred` carries every `ItemGroup` and `ItemDefinitionGroup` in the graph, spliced in
evaluation order, with each `Condition` **unevaluated** — because MSBuild settles those against the
property values evaluation *ended* with rather than the ones in scope where the element was written.
Nothing consumed that stream, `msbuild::evaluate` was still `todo!()`, and so:

- `reserved::seed` had **no caller at all**. Every reserved property MVP-11 added was dead code, and
  an include directory written `$(ProjectDir)inc` would still have expanded to `inc`.
- `compdb::build` had no input, which blocks the entire second half of the MVP.

Four questions had to be answered to close it, none of them obvious:

1. What decides which item definitions apply to which items, given that both arrive interleaved.
2. What `%(Name)` means, and whether an unset one is worth reporting.
3. Which directory a relative item path resolves against.
4. Which `ClCompile` settings become flags, and which are none of clangd's business.

## Decision

Three passes, three modules, and one path helper:

```rust
// msbuild.rs — the orchestrator
pub fn evaluate(scope: &ProjectScope, reporter: &mut Reporter) -> Result<EvaluatedProject>;

// msbuild/eval.rs — the `%()` sigil mechanism, beside the `$()` one
pub struct Metadata { /* case-insensitive, Clone */ }
pub fn expand_metadata(input: &str, metadata: &Metadata) -> Result<Expansion>;

// msbuild/items.rs — pass 2, the item semantics
pub struct SourceItem { path: PathBuf, metadata: Metadata, declared_in: PathBuf, line: usize }
pub fn settle(tree: &ProjectTree, project: &Path, reporter: &mut Reporter) -> Result<Vec<SourceItem>>;
pub fn declared_configurations(tree: &ProjectTree, reporter: &mut Reporter) -> Result<Vec<ConfigPlatform>>;

// msbuild/flags.rs — pass 3, the MSVC switch table
pub fn compile(source: &SourceItem, project_dir: &Path, reporter: &mut Reporter) -> FileCompilation;

// msbuild/paths.rs — lexical helpers, `normalize` moved out of imports.rs
pub(crate) fn normalize(path: &Path) -> PathBuf;
pub(crate) fn from_msbuild(text: &str) -> PathBuf;
pub(crate) fn resolve(base: &Path, text: &str) -> PathBuf;
```

`evaluate` takes a `ProjectScope` rather than `(&Path, &ConfigPlatform)`, which is the signature
change ADR 11 said this issue owed: seeding needs the solution path, and `ProjectScope` already *is*
exactly the three inputs the seed derives from. Its `config` is the **project** pair per ADR 05.

The four answers:

1. **An `ItemDefinitionGroup` applies only to items declared after it.** Defaults accumulate during
   the walk and each item takes a *clone* of them as they stand when it is declared.
   **Superseded by [ADR 40](40-item-definitions-apply-regardless-of-item-order.md).** MVP-45 found
   a real project whose source `ItemGroup` precedes every `ItemDefinitionGroup`, confirmed against
   a real Visual Studio build that the group still applies, and split `settle()` into two passes —
   definitions fully resolved, then items evaluated against them — so an item's position relative
   to the group no longer matters. Order *among* `ItemDefinitionGroup`s is unaffected and still
   governs `%(Inherited)` composition exactly as decided here.
2. **`%(Name)` is expanded eagerly, and an unset name expands to nothing without being recorded.**
   `$()` runs before `%()`.
3. **Relative item paths resolve against the `.vcxproj`'s directory**, not the declaring file's.
4. **Only settings that change how a file parses are translated** — the table is in `flags.rs`,
   along with each skip and its reason.

## Discussion

**The three passes are three modules because they change for unrelated reasons.** Pass 2 is MSBuild
semantics — what `Update` means, which definition group wins. Pass 3 is a lookup table of MSVC
switches that grows every time a toolset adds a value. Putting them together would make one ~400-line
module where a reader looking for "why did I get `/MDd`" has to walk past `Exclude` handling. The
split also fell out of the existing policy/mechanism seam: `eval.rs` owns the sigil *mechanism* (it
already has `closing_paren`, `is_property_name` and the size cap, all reused unchanged), `items.rs`
owns the item *policy*, exactly as `eval.rs` and `reserved.rs` divide properties.

**Silence on an unset `%(Name)` is the single most consequential decision here.** Every real project
file writes `WIN32;%(PreprocessorDefinitions)`, and in the first `ItemDefinitionGroup` of a graph
that reference resolves against nothing. Recording it as a finding would put a warning on output that
is completely correct, on every project, several times per file — which is how a user learns to
ignore the warning channel altogether, and the warning channel is this tool's only defence against a
plausible-but-wrong database. This is the same argument ADR 08 used to keep `Some("")` distinct from
`None`, applied to the other sigil. Constructs genuinely outside the subset —
`%(ClCompile.Identity)`, `@(ClCompile->'%(Filename)')`, an unterminated `%(` — *are* recorded, and
copied through verbatim so a flag containing them names its own cause.

There is **no literal `%(Inherited)` token** in MSBuild, though the issue text names one. That is
Visual Studio's "inherit from parent or project defaults" checkbox, whose file representation is the
self-reference above. Worth writing down, because looking for the token is a dead end.

**`$` before `%`.** A property's value was already expanded when it was assigned (ADR 08), so it can
contain no `%(`; but `$(CommonDefines)` can perfectly well hold one, and expanding `%` first would
leave it as written. The reverse order was never viable.

**Definition groups apply forward only.** MSBuild's rule, and Visual Studio always writes the
definitions above the items, so a project that depends on this is unusual. The rule was implemented
anyway rather than gathering all defaults up front, for the reason the whole project exists: gathering
first is *invisibly* different, applying settings to files MSBuild itself compiles without them, and
producing a database that is subtly wrong rather than loudly broken. It also cost nothing — ADR 10
had already spliced the stream in order precisely so that order would be recoverable here.

**Superseded by [ADR 40](40-item-definitions-apply-regardless-of-item-order.md).** This paragraph's
premise was wrong: MSBuild does not gate an item definition's applicability on the item's position
relative to it, only on order *among* item-definition groups (which this paragraph's "gathering
first" concern was actually about, and which ADR 40 keeps). MVP-45 found a real project whose
source `ItemGroup` precedes its `ItemDefinitionGroup`s and confirmed against a real Visual Studio
build that the defaults still apply.

**Relative item paths resolve against the project directory.** This is not what the code said:
`Deferred::file`'s doc comment claimed relative item paths resolve against the declaring file's
directory, and that comment is now corrected. MSBuild evaluates with the *project* as the current
directory, which is why a shared property sheet that means its own directory has to spell
`$(MSBuildThisFileDirectory)` — an idiom that would be pointless if the sheet's directory were the
default. Resolving the other way would point clangd at source files and include directories that do
not exist, silently, in exactly the projects that share sheets.

**Which led to a defect that only writing the tests surfaced.** If a sheet's item definitions are
settled against `tree.properties` as pass 1 leaves them, `$(MSBuildThisFileDirectory)` resolves to
the *project's* directory — because the walk restores that family on the way back out of each import,
so the final bag has it pointing at the root. A shared sheet's
`$(MSBuildThisFileDirectory)include` therefore silently becomes a directory beside the `.vcxproj`
that nothing created. So pass 2 keeps a mutable clone of the final bag and brackets each deferred
element with `reserved::enter_file`/`restore`, the same two calls `Walk::enter` uses. This is the
`MSBuildThisFile*` family being a property of the *file being read* rather than of the evaluation —
which ADR 11 called load-bearing, and which turns out to be load-bearing in two passes rather than
one. Everything else in the bag is the graph's end state, as MSBuild specifies.

**Wildcards warn rather than expand or refuse.** `<ClCompile Include="src\*.cpp" />` is legal
MSBuild that Visual Studio never writes, and it had stayed out of scope by accident rather than by
decision — the spec's Open Questions flagged that a project written that way produced *no source
files at all, in silence*, which is the worst available outcome. Three options:

- *Expand it.* The honest fix, and out of proportion to this issue: single-directory `*`/`?` is
  cheap, but `**`, `Exclude` interaction, and MSBuild's own glob quirks are not, and none of it is
  reachable from a Visual Studio-authored solution. Deferred to **MVP-37**, which now exists so the
  deferral is tracked rather than forgotten.
- *Refuse it as `Error::UnsupportedConstruct`.* Loudest, and disproportionate: one legal construct
  would fail generation for an entire solution.
- *Warn and contribute nothing.* Chosen. The cost is a project that still comes out short of files;
  the gain is that it now says which spec it skipped, at which line, in the channel MVP-04 built.

**`declared_configurations` is a separate function, and deliberately ignores group conditions.** A
solution can map a project to a pair the `.vcxproj` never declares. That evaluates perfectly
cleanly, matches no configuration-gated group, and yields source files with no flags — CLAUDE.md §3's
failure mode exactly. Warning about it is ten lines. It ignores the enclosing `ItemGroup`'s condition
because `settle` already evaluates that group and reports its findings, and `Reporter` does not
deduplicate — evaluating twice would double every warning on the group that every project file has.

**Only parse-affecting settings are translated, and the skips are documented in the source.** The
issue text permitted skipping codegen and link settings; the judgement calls were at the edges:

| Setting | Decision | Why |
| --- | --- | --- |
| `AdditionalIncludeDirectories`, `PreprocessorDefinitions` | `include_paths` / `defines` | The two the database exists for. |
| `LanguageStandard`, `LanguageStandard_C`, `CompileAs` | translated | Change the grammar outright. |
| `RuntimeLibrary`, `ExceptionHandling`, `RuntimeTypeInfo`, `OpenMPSupport`, `TreatWChar_tAsBuiltInType`, `ConformanceMode` | translated | Each moves a predefined macro (`_DEBUG`, `_CPPUNWIND`, `_CPPRTTI`, `_OPENMP`, `_NATIVE_WCHAR_T_DEFINED`) or the conformance rules a header keys off. |
| `EnableEnhancedInstructionSet`, `StructMemberAlignment`, `CallingConvention`, `FloatingPointModel` | translated | Layout, calling convention and `__AVX2__`-style predefines all reach intrinsic headers. |
| `ForcedIncludeFiles`, `UndefinePreprocessorDefinitions` | translated | Preprocessor state, directly. |
| `WarningLevel`, `DisableSpecificWarnings` | translated | Decide which diagnostics the editor shows. clang-cl maps levels onto its own groups approximately, so the set will not match a real build exactly — accepted, because the alternative is ignoring a project's explicit choice to silence a warning. |
| `TreatWarningAsError` | **skipped** | clang's warning set is not MSVC's, so honouring `/WX` turns diagnostics the real build never emits into editor *errors*, and a working file reads as broken. |
| `PrecompiledHeader`, `PrecompiledHeaderFile` | **skipped** | `/Yu` injects nothing — a source using a pch still `#include`s it — so skipping costs no parse fidelity, while emitting it points clangd at a `.pch` no sln2cc run produces. |
| `DisableLanguageExtensions` | **skipped** | clang-cl rejects `/Za`, which would fail the whole command rather than one flag. |
| `CharacterSet`, `ConfigurationType` | **skipped** | Properties rather than metadata: `UNICODE`/`_UNICODE` reach `PreprocessorDefinitions` through Microsoft's own `.props`, and hard-coding a copy of that logic would go stale the moment a toolset changed it. MVP-15 makes it moot. |
| everything else | skipped | Codegen or build bookkeeping. Cannot change how a file parses. |

An **unrecognized value for a recognized setting** warns and emits nothing, rather than guessing —
`LanguageStandard=stdcpp26` from a newer toolset should say so. An unrecognized *setting name* is
silent: there are dozens of them in a real `ClCompile`, all of them correct, and warning about each
would drown the channel. An empty or `Default` value is silent too, since `Default` means "whatever
`cl` does", which is also whatever clang-cl does.

**`AdditionalOptions` is emitted last and split on whitespace.** Last because that is the order `cl`
resolves conflicting switches in, so a project that overrides `LanguageStandard` there means the
override. Splitting on whitespace loses a quoted argument containing a space; the alternative is a
shell-quoting parser, which is more machinery than the case deserves until someone hits it.

**A value that still contains `$(` or `%(` warns.** Pass 1 and pass 2 copy constructs they cannot
evaluate through verbatim, on ADR 08's reasoning that a visible bogus value beats an invisible
missing one. `flags.rs` is where that becomes a sentence, because it is the last place that knows the
value is about to reach clangd. Today's most common cause is `$(VCTargetsPath)`, undefined until
MVP-15.

**`paths.rs` exists because `normalize` gained a second caller.** Moving it also gave
`from_msbuild` a home — `PathBuf::from(r"inc\sub")` is a *single component* on Linux, so without it
CI would compare paths Windows never produces, the same trap `sln.rs` avoids by splitting by hand.
`sln.rs` keeps its own copy: it additionally has to decide whether a solution-relative path is
absolute at all, and it is settled and tested where it sits. Two lexical path helpers in the crate is
one more than ideal and two fewer than the three that consolidating nothing would have produced.

**`TempTree` moved to `cli/src/testing.rs`.** `msbuild.rs`'s end-to-end test needs real files, which
made `imports.rs`'s test-local helper the crate's second caller. Moved rather than duplicated; still
`#[cfg(test)]`, still hand-rolled rather than a dev-dependency, for the reason its own doc comment
gives.

## Consequences

- **`compdb::build` now has real input**, which unblocks MVP-16 and MVP-17 and with them the second
  half of the MVP.
- **MVP-22 must map the solution pair to the project pair** with `SolutionProject::config_for`
  before calling `evaluate`, per ADR 05. `evaluate` takes the project pair and cannot do the mapping
  itself — it never sees the `Solution`.
- **MVP-13 inherits a larger set of warning wordings**: the wildcard skip, the duplicate
  declaration, the `Update` that matches nothing, the undeclared configuration, the unrecognized
  metadata value, and the unresolved-reference-in-a-flag case. As with ADR 10 and ADR 11, the facts
  are settled here and the phrasing is MVP-13's. This is also the first issue to produce warnings
  that are *actionable but not about a mistake* — a wildcard is legal — which is more evidence for
  the open question about whether `Severity` needs a second level.
- **MVP-15 shrinks the warning noise sharply again.** Every `$(VCTargetsPath)`-derived value is
  currently either undefined or copied through verbatim, and each one now warns twice — once from
  pass 2 as an undefined reference and once from pass 3 as an unresolved reference reaching clangd.
- **MVP-37 owns wildcard expansion**, and closes the spec's open question about it once done. The
  question is downgraded rather than removed: the behaviour is now defined and warned about, but a
  project written that way still gets an incomplete database.
- **Flags are still missing everything Microsoft's own `.props` contribute** — `UNICODE`, the
  `ConfigurationType`-derived defines, the toolset's default include directories — because those
  imports resolve to an empty path until MVP-15. A database generated today is correct about what the
  project says and incomplete about what MSBuild would add.
- **`FileCompilation` and `EvaluatedProject` were not changed.** The shapes MVP-07 declared turned
  out to be right, which is worth recording because it means MVP-16 can be written against them
  without waiting for anything.
- **New open question:** whether `AdditionalOptions` needs real shell-style argument splitting.
  Whitespace splitting is correct for every switch a project is likely to carry there and wrong for
  a quoted path with a space in it.
