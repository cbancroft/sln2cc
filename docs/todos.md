# TODOs

Issue tracker for msvclsp until the project has a real one. Each item is scoped to roughly
one developer hour to two developer days; anything larger should be split before starting.

Check an item off when its branch is merged and its tests pass. Issues are roughly in
dependency order within each section — later items generally assume earlier ones exist.

Estimates are rough (`~4h`, `~1d`) and exist only to keep scope honest.

See [milestones.md](milestones.md) for the feature groupings these come from, and
[engineering_spec.md](engineering_spec.md) for design detail. An item that settles a design question
gets an architecture decision record in [adr/](adr/) recording the reasoning; every ADR names the
item it came out of, so the two link both ways.

---

# MVP

Goal: one solution, one manually selected configuration/platform, producing a
`compile_commands.json` that clangd actually accepts.

## Foundation

- [x] **MVP-01 — Select and wire up CLI dependencies**
  `chore/cli-dependencies` · ~4h

  **Summary:** Resolve the open crate-selection question and add the chosen dependencies to
  `cli/Cargo.toml`.

  **Description:** The crate is currently dependency-free with candidates listed as comments.
  Pick an XML parser, argument parser, serialization stack, error-handling crate, and a
  persistence and logging approach; add them and record the reasoning in
  `engineering_spec.md`, removing the corresponding open question. Prefer fewer, well-
  maintained crates over convenience wrappers, and verify each one builds cleanly on both
  Linux and Windows before committing, since CI runs both.

  **Outcome:** `roxmltree`, `clap` (derive, `default-features = false`), `serde`/`serde_json`,
  `thiserror` — four direct dependencies, 20 packages resolved. Two candidate slots were
  closed by deciding *not* to take a crate: persistence is a JSON state file rather than
  `rusqlite`, whose `bundled` feature would make a C toolchain a hard build requirement, and
  no logging facade was added because unsupported-construct warnings belong in the JSON
  contract rather than on stderr. MSRV is pinned at 1.85 (set by `clap` 4.6). This reshaped
  MVP-04 and MVP-18; see [ADR 01](adr/01-address-dependencies.md) and "Dependency Decisions" in
  `engineering_spec.md`.

  **Tests:** `cargo build` and `cargo clippy -- -D warnings` clean on Linux and Windows;
  no test coverage needed beyond CI staying green.

- [x] **MVP-02 — Replace the placeholder error type**
  `refactor/error-type` · ~4h

  **Summary:** Swap `Result<T> = Result<T, Box<dyn Error>>` for a real error enum carrying
  enough structure for the CLI to emit machine-readable failures.

  **Description:** The plugin needs to distinguish "solution not found" from "unsupported
  MSBuild construct" from "no Visual Studio installation," so errors must carry a kind, not
  just a message. Define a crate-level error enum with variants per failure mode, implement
  `std::error::Error`, and include the offending path or construct where relevant. This
  underpins the exit-code and JSON-error contract, so land it before the subcommands.

  **Outcome:** A flat crate-level `Error` enum in a new `cli/src/error.rs` — fourteen leaf variants
  covering every failure mode this tracker names, plus a `Context` wrapper that **delegates its kind
  to what it wraps**, so adding context can never reclassify a failure. The wire-facing
  classification lives in a separate `ErrorKind` whose strings are written out by hand rather than
  derived from Rust variant names, so a refactor can't silently break the plugin contract, with
  `exit_code()` mapping kinds onto seven coarse classes (`2` reserved for `clap`'s usage errors).
  Serialization was deliberately left out — `ErrorKind` derives no `serde` traits, which **narrows
  MVP-03** to wiring `exit_code()` and `as_str()` into output types it already owns. Also: no
  `From<std::io::Error>`, since a bare `?` would discard the path. See
  [ADR 02](adr/02-structured-error-type.md) and "Error Handling" in `engineering_spec.md`.

  **Tests:** Unit tests asserting each variant's `Display` output and that error kinds
  survive being wrapped and propagated through a call chain.

- [x] **MVP-03 — CLI skeleton with subcommand dispatch**
  `feat/cli-skeleton` · ~4h

  **Summary:** Replace the stub `main.rs` with real argument parsing that dispatches to
  `list-configs` and `generate` handlers.

  **Description:** Define the subcommand surface, its flags (solution path, configuration,
  platform, output directory), and `--json` where relevant, keeping `main.rs` thin by
  delegating immediately into the library. Handlers can return `todo!()` initially — this
  issue is about the argument surface, exit-code plumbing (non-zero on failure, structured
  error on stderr), and making sure `--help` reads well. Record the final subcommand surface
  in `engineering_spec.md` and close that open question.

  **Outcome:** Two subcommands — `list-configs` and `generate` — taking the solution as a required
  positional, with `--configuration`/`--platform` as separate flags rather than one `Debug|x64`
  string, and global `--json`/`--verbose`. `discover` was deliberately left out; it is V1-01.
  Human-readable text is the stdout default and `--json` opts into a **static five-key envelope**
  (`schema_version`, `has_error`, `result`, `error`, `warnings`) whose shape does not vary by
  outcome. `has_error` exists because `vim.json.decode` maps `null` to `vim.NIL`, which is truthy
  in Lua — `if result.error then` would otherwise take the error branch on every success. stderr
  carries the human message in *both* modes, since MVP-28 folds it into the plugin's own error.
  Handlers return a temporary `Error::Unimplemented` (exit `9`) rather than `todo!()`, so
  CLAUDE.md §5's no-panic rule holds and the exit-code plumbing is exercised end to end;
  **MVP-21 and MVP-22 remove it**. `--force` and `-v` are defined but inert so the recorded surface
  is genuinely final. This **reshaped MVP-04** and reworded CLAUDE.md §3. See
  [ADR 03](adr/03-cli-surface-and-json-contract.md) and "CLI Surface" in `engineering_spec.md`.

  **Tests:** Integration tests invoking the built binary with `--help`, a missing required
  argument, and an unknown subcommand, asserting exit codes and that errors go to stderr.

- [x] **MVP-04 — Diagnostic collection and stdout/stderr discipline**
  `feat/diagnostics` · ~3h

  **Summary:** Collect warnings into a structured diagnostic list that rides along in the JSON
  output, and keep human-facing progress on stderr with stdout reserved for JSON.

  **Description:** Reframed by MVP-01, which deliberately added no logging facade, and narrowed
  again by MVP-03. Warnings about unsupported constructs are *data the plugin must surface to the
  user*, and the plugin is forbidden from scraping human-readable text — so they belong in the JSON
  payload as a `warnings` array, not as log lines. `cli.rs` already defines the `Diagnostic` type
  (severity, message, file, line), the envelope's `warnings` field, and
  `Envelope::with_warnings`, so this issue owns **accumulating** into them during evaluation and
  wiring the `-v/--verbose` flag, not designing the shape. Parsing a large solution takes real
  time, so `-v` reports human progress on stderr via `eprintln!`. The critical invariant is that
  under `--json`, nothing but the envelope is written to stdout, because the plugin parses it
  directly. Avoid emitting absolute user paths or full environment contents at default verbosity. If
  debug tracing during evaluation later justifies a real facade, `log` is the recorded
  preference over `tracing` — see "Dependency Decisions" in `engineering_spec.md`.

  **MVP-06 landed the first producer:** `sln::parse` parks its format-version diagnostic in
  `Solution::warnings` because this issue does not exist yet. Draining that field into the envelope
  is part of the work here, and if accumulation ends up as a collector threaded through evaluation,
  that field is what it replaces — see [ADR 05](adr/05-sln-configuration-mapping.md).

  **Outcome:** Accumulation *is* a collector. A new `cli/src/diagnostics.rs` owns `Diagnostic`,
  `Severity`, and a `Reporter` threaded as `&mut`, carrying both the warning list and the `-v`
  progress channel — one object because progress is inherently push-down (the point of `-v` is
  feedback while work is in flight) and because **`?` would discard floated-up warnings on exactly
  the failure path `Envelope::with_warnings` exists for**. `Solution::warnings` is gone, closing
  ADR 05's temporary field. The progress channel is an `Option<Box<dyn Write>>` rather than a
  `verbose: bool`, which makes suppression structural — quiet means there is no sink — and gives the
  required unit test something to assert. `Diagnostic` moved out of `cli.rs` so evaluation modules
  stop reaching into the CLI layer; `Diagnostic::render_human` lives in the library rather than
  `main.rs` because a binary crate is unreachable from integration tests. Warnings go to stderr in
  *both* modes plus the envelope under `--json`, with paths shown **relative to the project root at
  default verbosity** per CLAUDE.md §5. Found and removed a live `println!` in `sln::split_row`
  writing debug output to stdout — the exact invariant this issue owns, invisible because no CLI path
  reaches `parse_sln` yet — and added an integration test asserting stdout under `--json` is one line
  holding one object, so the next one fails a test instead of shipping. **This shrinks MVP-13** to
  deciding which constructs warn, and **MVP-07+ thread `&mut Reporter`**. The end-to-end
  "warnings in the payload" test is `#[ignore]`d naming **MVP-21**, which unblocks it. See
  [ADR 06](adr/06-diagnostic-collection.md) and "Stream discipline" in `engineering_spec.md`.

  **Tests:** Integration test asserting stdout parses as JSON and carries warnings in the
  payload while human progress goes to stderr; unit test that diagnostics accumulate in order
  and that default verbosity suppresses progress output.

## Solution parsing

- [x] **MVP-05 — Enumerate projects from a `.sln`**
  `feat/sln-projects` · ~5h

  **Summary:** Parse a solution file's `Project(...)` entries into names and absolute
  `.vcxproj` paths.

  **Description:** `.sln` is a line-oriented text format, not XML, so this needs its own
  small parser rather than reusing the XML stack. Extract each project's type GUID, name, and
  relative path, resolve paths against the solution directory, and filter to C/C++ projects —
  solution folders and non-native project types should be skipped rather than treated as
  errors. Malformed lines must produce a clear error naming the line number, not a panic.

  **Outcome:** Members are classified by project type GUID
  (`{8BC9CEB8-8B4A-11D0-8D11-00A0C91BC942}`, matched case-insensitively) rather than by file
  extension, and anything else — solution folder, C#, `.vcxitems` — is skipped **silently**, since
  it is correct solution content and warning about it would devalue MVP-13's warnings. Entry lines
  are read by scanning their four quoted fields rather than matching the `Project(...) = ...`
  grammar, because project names legitimately contain commas. `SolutionProject` gained a `guid`
  field that nothing here reads: **MVP-06 keys `ProjectConfigurationPlatforms` off it**, and MVP-06
  also adds its `GlobalSection` arms to this issue's single line-oriented scan rather than writing a
  second parser — which is why `parse()` returns `configurations: Vec::new()` for now. Project
  locations are made absolute with `std::path::absolute` and normalized lexically, never
  `fs::canonicalize` (it requires the file to exist and yields `\\?\` paths on Windows). `..` is
  resolved rather than refused, so **path-escape validation stays with MVP-10** and
  `Error::PathEscape` remains unused until then — which turned out to be never: MVP-10 found there
  was no root to escape from and removed the type. See
  [ADR 04](adr/04-sln-project-enumeration.md) and "CLI Binary — Internal Modules" in
  `engineering_spec.md`.

  **Tests:** Unit tests over inline `.sln` snippets covering a single project, multiple
  projects, solution folders, a non-C++ project type, and a truncated/malformed entry.

- [x] **MVP-06 — Enumerate solution configurations and platforms**
  `feat/sln-configurations` · ~5h

  **Summary:** Parse `SolutionConfigurationPlatforms` and the per-project mapping in
  `ProjectConfigurationPlatforms`.

  **Description:** The solution declares its own configuration/platform pairs and then maps
  each to a possibly-different pair per project, which is why a solution-level `Debug|x64`
  can mean `Debug|Win32` for one member project. Parse both sections and expose the mapping,
  treating names as opaque strings since solutions may define custom ones. Projects excluded
  from a given solution configuration must be identifiable so they can be skipped during
  generation.

  **Outcome:** Both sections are read by adding `GlobalSection` tracking to MVP-05's single scan, as
  ADR 04 anticipated. `Solution::configurations` comes from the **key** side of each declared row,
  deduplicated in declaration order, and `SolutionProject` gains a `configurations:
  Vec<ProjectConfigMapping>` holding the solution pair, the project pair it resolves to, and a
  `build` flag, reached via `SolutionProject::config_for`. Row keys are taken apart by stripping a
  known property (`.ActiveCfg`, `.Build.0`) off the **right**, never by splitting on `.`, because
  configuration names may contain a dot; `Build.0`'s presence sets `build` and its value is never
  read. Exclusion is deliberately two states — a mapping with `build: false` versus no mapping at all
  — so **MVP-22 decides** whether an unbuilt project's files are still worth indexing. Two questions
  MVP-05 handed forward are answered: a **duplicate project GUID is now an error** naming both lines,
  tracked across all entry types since a C#/C++ clash would misattach rows, which closes ADR 04's
  open question; and the **format-version header warns rather than being enforced**, which makes this
  the first issue to emit a diagnostic at all and **gives MVP-04 its first producer** in a new
  `Solution::warnings` field. See [ADR 05](adr/05-sln-configuration-mapping.md) and "CLI Binary —
  Internal Modules" in `engineering_spec.md`.

  **Tests:** Unit tests covering standard Debug/Release × Win32/x64, a custom configuration
  name, a project whose platform differs from the solution's, and a project excluded from one
  configuration.

## MSBuild evaluation

- [x] **MVP-07 — Read `.vcxproj` structure**
  `feat/vcxproj-reader` · ~1d

  **Summary:** Parse a `.vcxproj` into its `PropertyGroup`, `ItemDefinitionGroup`,
  `ItemGroup`, and `Import` elements, preserving document order.

  **Description:** Evaluation order determines which property assignment wins, so the reader
  must preserve the sequence of elements rather than collecting them into unordered maps.
  Produce a faithful intermediate representation — no evaluation, no condition filtering yet
  — including each element's `Condition` attribute so later passes can act on it. Source
  files come from `ClCompile` items; headers and other item types can be recorded but aren't
  needed for the compilation database.

  **Outcome:** A new `cli/src/msbuild/project.rs` lowers one file — `.vcxproj`, `.props` and
  `.targets` are one format with one `<Project>` root, so MVP-10 reads property sheets through the
  same function — into an owned `Vec<Element>` in document order. Duplicates are kept, inside a
  group as well as across groups, because which assignment wins depends on conditions this pass
  deliberately does not evaluate; there is no correct merge available here. A group's `Condition`
  is kept **separate** from its children's rather than combined into `(a) and (b)`, which would put
  an expression in the IR that appears in no file. One `Setting` type covers property assignments
  and item metadata, since the XML is identical and so is what MVP-08 and MVP-09 do with it.
  Elements match on their local name so the namespace is optional; values are trimmed and
  conditions are not. Unmodelled elements split two ways: `Target`/`UsingTask`/`ProjectExtensions`
  are skipped in silence as content that cannot change how a file compiles, and everything else —
  `<Choose>` above all — becomes `Element::Unsupported` *and* a warning, so **MVP-13 inherits the
  fact and owns only the wording**. Well-formed XML that is not a project fails as a new
  `project-schema` kind, distinct from `project-parse` because "this is not a C++ project" is a
  different thing to tell a user; a missing file stays `Io`, since **MVP-10 owns `MissingImport`**
  and is the only caller that knows the referrer. **MVP-12 consumes `items_of_type("ClCompile")`**
  and the `ItemDefinitionGroup` metadata beside it. Writing the hostile-input test found that
  `roxmltree::Document::parse` recurses per nesting level and aborts the process at ~200 levels of
  nesting, which **MVP-35** now owns; the test is `#[ignore]`d because running it takes the test
  binary with it. See [ADR 07](adr/07-vcxproj-reader-ir.md) and "CLI Binary — Internal Modules" in
  `engineering_spec.md`.

  **Tests:** Unit tests asserting element order is preserved, conditions are captured
  verbatim, `ClCompile` items are extracted, and malformed XML produces an error rather than
  a panic.

- [x] **MVP-08 — Property bag and `$(Macro)` expansion**
  `feat/msbuild-macros` · ~1d

  **Summary:** Implement the property store and recursive expansion of `$(Name)` references
  within property values.

  **Description:** Properties are assigned in order with later definitions shadowing earlier
  ones, and their values may themselves reference other properties, including ones defined
  later in some cases. Implement expansion with a recursion depth limit so a self-referential
  or cyclic property fails cleanly instead of hanging. Unknown properties expand to the empty
  string, matching MSBuild, but should be recorded so MVP-13 can warn about them.

  **Outcome:** `cli/src/msbuild/eval.rs` gains `Properties` — an opaque bag keyed case-insensitively,
  because `$(configuration)` reaching a different slot than `$(Configuration)` surfaces as an empty
  include path rather than as anything legible — and `expand`, a single non-recursive pass returning
  `Expansion { value, undefined, unsupported }`. The decision that shapes everything is that
  **expansion is eager**: a value is expanded when it is *assigned*, against the bag as it stood then,
  so what is stored contains no `$(` at all. That is MSBuild's own rule, and it is why
  `$(PreprocessorDefinitions);_DEBUG` appends rather than looping, why redefining a property never
  reaches back into values already built from it, and why **a cycle cannot be constructed** — which
  in turn means this item's "recursion depth limit" has nothing to count. The real hostile-input risk
  is `<A>$(A)$(A)</A>`, which doubles per assignment, so **the guard is a 1 MiB cap on the expanded
  value**, raised as the existing `Error::RecursionLimit` rather than a new wire string. `Some("")`
  and `None` stay distinct, since conflating them would have **MVP-13** warn on every idiomatic
  `Condition="'$(X)' != ''"`. Constructs outside the subset — property functions, registry reads,
  nested references, an unterminated `$(` — are copied through *verbatim* and recorded, never
  emptied: a bogus entry in the database names its own cause where a missing one does not. `expand`
  takes no `Reporter` and warns about nothing, so **MVP-13 owns every word** and gets one finding per
  distinct name rather than per occurrence. **MVP-11 seeds through `set`** (literal paths, nothing to
  expand) while everything read from a project file goes through `assign`. See
  [ADR 08](adr/08-eager-macro-expansion.md) and "CLI Binary — Internal Modules" in
  `engineering_spec.md`.

  **Tests:** Unit tests for simple expansion, nested expansion, redefinition ordering, an
  undefined property expanding to empty, and a cyclic reference erroring at the depth limit.

  The last of those **could not be written as specified** and was replaced by two: a self-reference
  is legal and terminates, and runaway doubling is refused at the size cap. Eager expansion makes a
  cyclic property unconstructible — see ADR 08. Also covered: `%(...)`/`@(...)` left untouched, a
  bare `$` staying literal, case-insensitive lookup, empty-versus-undefined, dedup order, a property
  function keeping its own parentheses, and a non-ASCII value surviving the byte-offset scan.

- [x] **MVP-09 — `Condition` expression evaluation**
  `feat/msbuild-conditions` · ~1d

  **Summary:** Evaluate the subset of MSBuild condition syntax that real `.vcxproj` and
  `.props` files actually use.

  **Description:** In practice this means quoted string equality and inequality
  (`'$(Configuration)|$(Platform)'=='Debug|x64'`), boolean `And`/`Or`/`!`, parentheses, and
  the `Exists()` function; the full MSBuild grammar is far larger and out of scope. Macros
  are expanded before comparison. Anything outside the supported subset must be reported as
  unsupported rather than silently evaluating false, because a silently-skipped
  `PropertyGroup` produces a plausible but wrong compilation database.

  **Outcome:** A tokenizer and recursive-descent evaluator in `msbuild/eval.rs`, over exactly the
  subset above plus a lone operand as a boolean literal. `condition_holds` returns
  `Evaluation { holds, undefined, unsupported }`, mirroring MVP-08's `Expansion`, and takes the
  condition's **file and line** — needed both to site an error and because `Exists()` resolves
  relative paths against the directory of the file the condition is written in, as MSBuild does. The
  decision worth internalizing is that "unsupported" splits two ways: an expression that cannot
  reduce to a boolean (`'1' < '2'`, `HasTrailingSlash(...)`) is a fatal
  `Error::UnsupportedConstruct` — **MVP-09 is that variant's first producer** — while an unsupported
  `$(...)` *inside* an operand is merely recorded, because MVP-08 left it verbatim and there are
  still two strings to compare. Conditions are **parsed before any operand is expanded**, so a
  property value containing `' Or '` cannot rewrite the expression around it; this is untrusted
  input and there is a test named for the injection. `And`/`Or` do not short-circuit, so a construct
  on the dead side is still reported. Comparison is case-insensitive; an empty condition holds;
  depth is capped at 32 as `Error::RecursionLimit`. **MVP-10 inherits the fatal-error policy** and
  owns whether to downgrade it for files imported from the Visual Studio installation, whose own
  `.targets` use constructs well outside this subset. See
  [ADR 09](adr/09-condition-evaluation-subset.md) and "CLI Binary — Internal Modules" in
  `engineering_spec.md`.

  **Tests:** Unit tests per operator, a compound `And`/`Or` expression, `Exists()` against a
  real and missing path, case-insensitive comparison behavior, and an unsupported construct
  producing an error.

  All present, plus: `And` binding tighter than `Or` (asserted with an expression that reads
  differently under flat precedence), parentheses overriding it, an unquoted `$(Platform)` operand
  surviving tokenization, an empty condition holding, a lone operand as a boolean, operand
  injection being impossible, an unsupported macro inside an operand being recorded rather than
  fatal, and nesting refused at the depth cap. `Exists()` runs against the real filesystem under
  `CARGO_MANIFEST_DIR` — no fake, because unlike a Visual Studio path, `Cargo.toml` exists on every
  machine that can run the suite.

- [x] **MVP-10 — Resolve `.props`/`.targets` imports**
  `feat/msbuild-imports` · ~1d

  **Summary:** Recursively follow `<Import>` elements and return the full file list in
  evaluation order.

  **Description:** Import paths are themselves macro-expanded and condition-gated, so
  resolution has to interleave with MVP-08 and MVP-09 rather than run as a separate pre-pass.
  Track visited files to detect cycles, cap recursion depth, and treat a missing import as an
  error unless the `Import` is marked conditional. Imports reaching outside the project tree
  (into the Visual Studio installation) are expected and legitimate, but paths that escape via
  `..` traversal from untrusted input should be validated.

  **Outcome:** `msbuild/imports.rs` walks the graph in document order, splicing each imported file
  in where its `<Import>` sits. Because import paths are macro-expanded and condition-gated, this is
  not a pre-pass over the graph — it **is MSBuild's first evaluation pass**, and `resolve` returns
  `ProjectTree { files, deferred, properties }` rather than the bare path list this item asked for:
  the bag falls out of the walk regardless, and **MVP-12 becomes pass 2/3**, settling each
  `Deferred` item group against the properties the whole graph ended with. The deferred elements are
  *spliced* rather than grouped by file, because a `.vcxproj` writes its `ItemDefinitionGroup`s
  between `Microsoft.Cpp.props` and `Microsoft.Cpp.targets` and grouping would order the root's
  ahead of the imported sheet's. A file already imported is skipped and warned about — MSBuild's
  MSB4011 rule, which covers diamonds *and* cycles with one mechanism; `..` is normalised and
  followed rather than refused, since a shared sheet one directory up is ordinary. Neither
  `Error::ImportCycle` nor `Error::PathEscape` was ever going to be raised as a result, so **both
  are removed** along with their wire strings, and exit code `8` is retired rather than reassigned.
  A condition is evaluated
  before its import path is expanded, so a guarded optional import never reports a missing file the
  project allows for. This is also where **MVP-08 and MVP-09 findings finally become warnings** —
  the first code holding both a finding and the file and line it came from — which will be noisy,
  and which **MVP-13 inherits**. `properties` is a parameter so **MVP-11 can seed it**. Depth capped
  at 64. See [ADR 10](adr/10-import-resolution.md) and "CLI Binary — Internal Modules" in
  `engineering_spec.md`.

  **Tests:** Unit tests for a linear import chain, a diamond import, a cycle producing an
  error, a conditional import that evaluates false being skipped, and a missing required
  import erroring with the referencing file named.

  The cycle test **could not be written as specified**: under MSBuild's duplicate rule a cycle is
  just a repeat import, so it asserts termination rather than an error — see ADR 10. Tests build
  throwaway trees under the system temp directory rather than using `fixtures/`, which MVP-23 and
  MVP-24 own and which is no place for an adversarial cycle case. Also covered: a relative import
  resolving against its own file's directory, `..` normalising so two spellings count as one file,
  `\` on Windows, the deferred stream's interleaving, deferred conditions surviving unevaluated,
  shadowing across the graph, an empty expanded path, the depth cap and the file at it, and an
  unsupported condition on an `<Import>` staying fatal.

- [x] **MVP-11 — Reserved and well-known MSBuild properties**
  `feat/msbuild-reserved-properties` · ~5h

  **Summary:** Seed the property bag with the built-in properties that project files assume
  exist.

  **Description:** Files reference `$(SolutionDir)`, `$(ProjectDir)`, `$(ProjectName)`,
  `$(Configuration)`, `$(Platform)`, `$(MSBuildThisFileDirectory)`, and similar without ever
  defining them, so evaluation produces empty include paths without these. Populate them from
  the solution and project being evaluated, with `MSBuildThisFile*` varying per file during
  import traversal. Directory-valued properties must carry MSBuild's trailing separator, since
  project files concatenate them directly.

  **Outcome:** A new `cli/src/msbuild/reserved.rs` seeds **twenty-four** properties in five families —
  `Solution*`, `Configuration`/`Platform`, `MSBuildProject*`, `MSBuildThisFile*`, `Project*` — all
  derived from three inputs: the `.sln` path, the `.vcxproj` path, and per ADR 05 the **project**
  pair rather than the solution-level one the user picked. The list is complete rather than the six
  names above, because a property left out fails as an include path that quietly lost a directory,
  which looks exactly like not having done the work; a test enumerates all twenty-four so a typo
  cannot hide. Toolchain-derived properties are deliberately absent — **MVP-15 extends this seed**
  with `$(VCTargetsPath)` and `$(VSInstallDir)` rather than adding a second mechanism, and
  `ProjectScope` is where they will arrive. The detail worth internalizing is that **MSBuild's two
  trailing-separator conventions are both load-bearing**: `$(ProjectDir)` and
  `$(MSBuildThisFileDirectory)` carry one and `$(MSBuildProjectDirectory)` does not, which is why
  `Microsoft.Common.props` writes `<ProjectDir>$(MSBuildProjectDirectory)\</ProjectDir>`, and
  normalising either way turns `$(ProjectDir)inc` into a path that does not exist. Separators are
  spelled with `MAIN_SEPARATOR`, since a seeded `/tmp/x\` on the Linux CI runner mixes them inside one
  path. Immutability is enforced **in `Properties`** rather than by a check at the one call site:
  `set_reserved` writes and locks, `assign` refuses and sets a new `Expansion::refused` for
  `imports.rs` to warn from, and `set` still writes through — which formalises the trust boundary
  MVP-08 had already drawn between the two functions. The lock is **two-tier**: what MSBuild reserves
  plus what a solution build passes as global, while `Project*` stays **overridable**, because
  `Microsoft.Cpp.Default.props` defines it conditionally and warning on `<ProjectName>` would be a
  false positive on ordinary files. A refused assignment is still *expanded* so its findings reach
  the reporter, on ADR 09's non-short-circuit reasoning. `MSBuildThisFile*` follows the walk, so
  **this issue changed `imports.rs`** as ADR 10 predicted: `Walk::enter` brackets each file with
  `reserved::enter_file`/`restore` beside its depth bookkeeping, and `FileScope` carries only the
  displaced *path* since all six values are a pure function of it — which is also why `Properties`
  needs no removal API and the root file needs no special case. **Nothing calls `seed` yet**:
  `msbuild::evaluate` is still `todo!()`, so **MVP-12 owns wiring it up** and needs a solution path
  added to that signature. See [ADR 11](adr/11-reserved-property-seeding.md) and "CLI Binary —
  Internal Modules" in `engineering_spec.md`.

  **Tests:** Unit tests asserting each property resolves to the expected absolute path,
  trailing separators are present, and `MSBuildThisFileDirectory` differs correctly between a
  project and an imported property sheet.

  All three were written as specified. Expectations are built from `MAIN_SEPARATOR` and an
  `absolute()` helper rather than hard-coded `D:\…`, so they assert the same thing on the Linux CI
  runner — where `NoRoot` stripping also has different work to do, which a `#[cfg(windows)]` UNC case
  covers on the other side. Also covered: the trailing-separator asymmetry as its own case, the
  `Project*` family staying fixed on the root project while `MSBuildThisFile*` moves, a two-level
  nested import unwinding one level at a time, every seeded name holding something, a non-ASCII path,
  the lock refusing each reserved family and case-insensitively, `set` still overwriting a locked key,
  nothing being reserved in an unseeded bag, a seeded `$(SolutionDir)` resolving an import path that
  MVP-10 could only assert was skipped, `Exists()` finally having a real directory to test, the
  seeded pair deciding which `PropertyGroup` applies, the idiomatic
  `<Configuration Condition="'$(Configuration)'==''">` default drawing no warning, an unconditional
  reserved assignment drawing one sited at its line, and an empty bag remaining a valid starting
  point.

- [x] **MVP-12 — Extract compiler settings into flags**
  `feat/clcompile-flags` · ~1d

  **Summary:** Turn evaluated `ClCompile` settings into per-file include paths, defines, and
  MSVC-form compiler flags.

  **Description:** Read `AdditionalIncludeDirectories`, `PreprocessorDefinitions`,
  `LanguageStandard`, `RuntimeLibrary`, `ExceptionHandling`, and the other settings that
  affect parsing, mapping each to its `cl` flag rather than a clang equivalent. Handle
  `%(Inherited)` semantics so project-level settings compose with item-level overrides, and
  resolve include paths to absolute form. Settings that only affect codegen or linking can be
  skipped — clangd only needs what changes how the file parses.

  **Narrowed by MVP-10**, which makes this pass 2/3: the `Deferred` elements it hands over carry
  their conditions **unevaluated**, to be settled against `ProjectTree::properties` — the values the
  whole graph ended with — rather than the ones in scope where each element was written.

  **Also owns wiring up MVP-11's seed.** `reserved::seed(&ProjectScope)` exists and nothing calls it,
  because `msbuild::evaluate` is still `todo!()`; its signature (`&Path, &ConfigPlatform`) has no
  solution path to build a `ProjectScope` from and needs one. Without that call the include paths this
  issue resolves to absolute form have no `$(ProjectDir)` to resolve against. See
  [ADR 11](adr/11-reserved-property-seeding.md).

  **Outcome:** Split three ways, one per pass and one per reason to change. `msbuild/eval.rs` gains
  `Metadata` and `expand_metadata` — the `%()` counterpart to `Properties`, reusing that module's
  existing `closing_paren`/`is_property_name`/size-cap machinery; `msbuild/items.rs` is pass 2, settling
  the deferred stream into `SourceItem`s; `msbuild/flags.rs` is pass 3, the MSVC switch table; and
  `msbuild/paths.rs` holds `normalize` (moved out of `imports.rs`, which gained a second caller)
  alongside `from_msbuild`, because `PathBuf::from(r"inc\sub")` is a *single component* on Linux and CI
  would otherwise compare a path Windows never produces. `evaluate` now takes `&ProjectScope` — the
  signature change this item owed MVP-11, since seeding needs the solution path and that struct already
  is the three inputs. Four decisions carry the weight: **an unset `%(Name)` expands to nothing and is
  not recorded**, because `WIN32;%(PreprocessorDefinitions)` against an empty bag is what every real
  project file writes and warning on it would train users to ignore the channel (ADR 08's `Some("")`
  argument, other sigil); **`$` expands before `%`**; **an `ItemDefinitionGroup` applies only to items
  declared after it**, so defaults are snapshotted per item rather than gathered up front, which is
  what ADR 10's ordered splice was for; and **relative item paths resolve against the `.vcxproj`'s
  directory, not the declaring file's** — MSBuild's rule, and the reason a shared sheet has to spell
  `$(MSBuildThisFileDirectory)`. `Deferred::file`'s doc comment claimed the opposite and is corrected.
  Writing the tests surfaced a defect in the deferred design: the walk *restores* `MSBuildThisFile*` on
  the way out of each import, so settling a sheet's item definitions against the final bag resolves
  `$(MSBuildThisFileDirectory)include` to the project's directory — pass 2 therefore keeps a mutable
  clone of the bag and brackets each deferred element with `reserved::enter_file`/`restore`. Wildcards
  **warn and contribute nothing** rather than expanding or failing, which closes the silent-no-sources
  hole and hands expansion to **MVP-37**. Only parse-affecting settings are translated; `/WX`, `/Yu`,
  `/Za` and `CharacterSet` are skipped with reasons in the source. An unrecognized *value* warns, an
  unrecognized *setting name* does not. `declared_configurations` warns when the requested pair is one
  the project never declares, which otherwise surfaces as files with no flags. **MVP-13 inherits six
  more wordings** and **MVP-16 finally has input**. `FileCompilation` and `EvaluatedProject` were not
  changed. See [ADR 12](adr/12-clcompile-flag-extraction.md) and "CLI Binary — Internal Modules" in
  `engineering_spec.md`.

  **Tests:** Unit tests per setting mapping, a file-level override beating the project-level
  default, `%(Inherited)` composition, and semicolon-delimited lists splitting correctly.

  All four written, plus: definition groups applying forward only; conditions settling against the
  properties the graph *ended* with; `Update`/`Remove`/`Exclude`; a duplicate declaration; a wildcard;
  path resolution against the project rather than the declaring sheet; `%()` order, dedup and the size
  cap; unrecognized and `Default` values; the skip list asserted rather than described; flag order
  stable across runs, since `Metadata` is a `HashMap` inside. `msbuild.rs` gained an end-to-end test
  over a temp tree — a `.vcxproj` importing a sheet, three layers of define composition, Debug and
  Release — which is what caught the `MSBuildThisFile*` defect above. `TempTree` moved from
  `imports.rs`'s test module to `cli/src/testing.rs` for it. No `cli/tests/` work: fixture-driven
  integration is MVP-23 and MVP-24's, and `fixtures/` is still empty.

- [x] **MVP-13 — Warn on unsupported MSBuild constructs**
  `feat/unsupported-warnings` · ~4h

  **Summary:** Collect and report constructs the evaluator doesn't understand instead of
  silently ignoring them.

  **Description:** A subtly wrong compilation database is worse than a loud failure because
  clangd will produce confident nonsense, so every skipped condition, unknown function, item
  transform, or undefined property reference should surface. Accumulate warnings during
  evaluation with enough context to act on — file, line, and the construct itself — and emit
  them on stderr plus in the JSON result so the plugin can surface them. Generation still
  succeeds; this is a warning channel, not a failure mode.

  **Narrowed by MVP-04**, which built the channel: call `Reporter::warn_at(file, line, message)` on
  the reporter already threaded through evaluation, and the stderr rendering and the envelope's
  `warnings` array both follow for free. What is left here is *which* constructs warn and what each
  message tells the user to do — see [ADR 06](adr/06-diagnostic-collection.md), which also raises
  whether `Severity` needs a second level to separate "your flags may be wrong" from "this is
  merely unusual".

  **Outcome:** It does, and that is the decision the rest follows from. `Severity` gains **`Note`**,
  split by one question — *could msvclsp's failure to model this have changed the compiler flags?* —
  with `Reporter::note_at` beside `warn_at` so only the four sites that change severity change at
  all. The argument is volume: **MVP-15 is what makes this urgent**, because importing the Visual
  Studio installation's own sheets puts hundreds of `$(X) is not defined` findings in the same
  undifferentiated list as the one that means a file lost an include directory, and a channel that
  gets skimmed is worse than none. Notes are **not** suppressed by default or held behind `-v` —
  filtering is the consumer's call and the CLI has no UI to make it with; the split is what lets
  **MVP-28** decide, and that decoder **must not branch exhaustively** on the severity string.
  `schema_version` is not bumped: a new enum *value* is additive. Four sites became notes — an
  unexpected-but-present `.sln` version, an item declaring no `Include`/`Remove`/`Update`, a repeat
  import (MSBuild's own MSB4011 behaviour), and a wildcard in a `Remove`/`Exclude` **with nothing in
  reach**, which was a false positive: an `Include` wildcard is not expanded either, so nothing was
  there to remove. That last one needed a **third** reading rather than a second — an `Exclude`
  paired with a *literal* `Include` does have files to subtract from, and not expanding it leaves
  them in the database, which is the opposite direction from the `Include` case and so cannot share
  its sentence. `include_item` picks between `Harmless` and the new `Keeps` because it is the only
  place that knows both halves. `settle`'s `Remove` arm keeps the unconditional `Harmless` and its
  latent hole, deliberately — see the ADR.
  A *duplicate source declaration* went the other way and stays a warning, because the dropped
  declaration may carry metadata — the line is "did anything get lost", not "does output still
  exist". The two wordings that existed in three copies collapse into `eval::report`, with
  consuming `Expansion::report`/`Evaluation::report` wrappers that also delete two `.clone()`s;
  `expand` and `condition_holds` stay reporter-free per ADR 08, and the other fourteen messages stay
  at their sites beside the comments that justify them, with the read-it-in-one-place requirement met
  by a **catalogue in the engineering spec** instead. `Reporter` now drops a diagnostic identical in
  all four fields to one already recorded, keeping the first — which matters for **MVP-22**, where a
  `Directory.Build.props` shared by thirty projects would otherwise repeat every finding thirty
  times. Two dangling comments are settled: `AdditionalOptions` holding a `"` now warns rather than
  corrupting silently, with the real splitter deferred to **MVP-38** because the quoting rules of
  MSBuild, `cl` and the Windows command line disagree; and `%XX` escapes stay undecoded *and*
  unwarned, recorded as a position rather than left implied. Also closes the spec's open question on
  property values with **element children**, which now warn — the last place the reader dropped
  content in silence. See [ADR 13](adr/13-diagnostic-severity-and-wording.md) and "Diagnostics" in
  `engineering_spec.md`.

  **Tests:** Unit tests asserting a warning is recorded for an unsupported condition function
  and an undefined property; integration test asserting warnings appear in both stderr and
  the JSON output.

  The undefined-property test is covered (`eval.rs`, `imports.rs`). The first **could not be written
  as specified**: ADR 09 made an unsupported condition *function* a fatal
  `Error::UnsupportedConstruct`, because an expression that cannot reduce to a boolean has no truth
  value to hand back — there is no such warning to assert. The two nearest true statements are
  covered instead: an unsupported `$(...)` *inside* an operand is recorded rather than fatal, and an
  unsupported *element* warns. Same treatment MVP-08's cyclic-property test got. The integration test
  **stays blocked**: `list-configs` and `generate` both still return `Error::Unimplemented` without
  reaching `sln::parse_sln`, so `warnings_ride_in_the_envelope` remains `#[ignore]`d naming
  **MVP-21** — its `todo!` now also owes an assertion that the severity reaches the wire, since a
  level that never leaves the library is indistinguishable from not having split it. Also covered:
  every classification in the catalogue asserted at its own site; suppression comparing all four
  fields and keeping the first occurrence, so the ordering contract survives it; the stderr label
  and the serde rename pinned to each other; `Note` rendering as `note:` rather than `warning:`; the
  two `eval::report` wordings and their siting; an ordinary Visual Studio-shaped project evaluating
  in total silence; and negative cases for the two new checks — an unquoted `AdditionalOptions` and
  the ordinary two-level XML shapes — so neither can be satisfied by always warning.

- [x] **MVP-35 — Bound XML nesting depth before parsing**
  `fix/xml-depth-guard` · ~3h

  **Summary:** Reject a project file nested deeper than a fixed limit, before its text reaches
  `roxmltree`.

  **Description:** Found while writing MVP-07's hostile-input test.
  `roxmltree::Document::parse` recurses once per nesting level: a debug build on a 2 MiB thread
  stack survives 100 levels and dies at 200, from roughly 1.5 KB of XML. The process aborts, so
  there is no error, no exit code, and nothing for the plugin to surface — exactly what CLAUDE.md
  §5 says must not happen. `roxmltree`'s own options do not help: `allow_dtd` is already `false`
  and closes the entity-expansion surface, but `nodes_limit` caps the *number* of nodes rather
  than their depth and cannot separate deep nesting from a large but ordinary project with
  thousands of items.

  So the depth has to be bounded before parsing, which is a byte scan rather than an XML parse.
  It can be conservative — real MSBuild files nest about four levels, so a limit in the tens
  leaves enormous headroom, and over-counting only costs a rejection nobody will hit. `<` cannot
  appear inside an attribute value in well-formed XML, so the scan needs no quote tracking; it
  does need to skip comments and CDATA, both of which have unambiguous terminators. Fail as
  `Error::RecursionLimit`, which already exists and already carries a limit and a context.

  Numbered 35 rather than slotted into this section's sequence because numbers are assigned in
  order and never reused. See [ADR 07](adr/07-vcxproj-reader-ir.md).

  **Outcome:** `parse_project_text` calls a private `check_nesting_depth` before `Document::parse`
  sees the text, because the failure is an abort *inside* the parser and there is consequently no
  "after" to catch it in. It is a byte scan rather than a parse: a start tag opens a level, a close
  tag closes one, a self-closing tag does neither, and comments, CDATA, processing instructions and
  declarations are skipped whole. Past `MAX_XML_DEPTH = 32` it raises the existing
  `Error::RecursionLimit`, so **no new string enters the plugin contract** — the variant already
  covers three other caps and ADR 08 already generalised it past literal depth. 32 rather than 64
  because the caps *compose*: `imports.rs` may be `MAX_IMPORT_DEPTH` frames deep at the moment a
  file is parsed, so the stack left for `roxmltree` is well short of the ~100 levels a fresh one
  survived. Two decisions carry the weight. **Self-closing detection is load-bearing** — without it
  depth grows with the number of `<ClCompile … />` items and the guard refuses every real solution
  before it refuses a hostile one, which to a user is indistinguishable from the tool being broken;
  and **finding where a tag ends needs quote tracking** while finding where one begins does not,
  because XML permits a literal `>` in an attribute value and forbids a literal `<`. The scan is a
  **guard, not a validator**: malformed markup is stepped over rather than diagnosed, so broken XML
  keeps failing as `project-parse` with `roxmltree`'s own message and does not acquire a second,
  worse one. What it approximates — a DTD internal subset, a non-ASCII name start — it approximates
  by over-counting, on files the parser rejects anyway. See
  [ADR 16](adr/16-xml-nesting-depth-guard.md).

  **Tests:** Un-`#[ignore]` `a_deeply_nested_document_is_rejected_rather_than_crashing` in
  `msbuild/project.rs`, which already asserts `ErrorKind::RecursionLimit`. Add a unit test that a
  file at the limit still parses, and one that `<` inside a comment or CDATA does not count
  toward depth.

  All three were written as specified, plus four. The at-the-limit test is paired with
  `a_document_one_level_past_the_limit_is_refused` so an off-by-one has to fail one of them rather
  than hide between them, and it is built through `project_text` so it also pins that the
  `<?xml … ?>` declaration is not counted as a level. Comment and CDATA got a test each, since the
  terminators differ. The two additions worth having are `self_closing_siblings_do_not_accumulate_depth`
  and `a_greater_than_inside_an_attribute_value_does_not_end_the_tag`: both parse an `ItemGroup` of
  `MAX_XML_DEPTH * 4` sibling items and assert every one arrives, which is the false-positive case
  the guard is most likely to fail on and the one a user would actually meet. No test asserts that
  a close tag decrements — without that, depth would grow with element *count* and the whole
  existing suite fails at once. `malformed_xml_is_an_error_not_a_panic` and `a_dtd_is_rejected`
  serve unchanged as the guard's negative cases: both still expect `project-parse`, which is what
  keeps the scan from becoming a validator.

- [ ] **MVP-40 — Bound project file size before reading**

  **Summary:** Cap the bytes `parse_project` will read from a single `.vcxproj`/`.props` file,
  the sibling gap MVP-35 found but did not close.

  **Description:** `parse_project` calls `read_to_string` and allocates however many bytes are on
  disk. A failed allocation aborts the process the same wordless way an unbounded XML-nesting
  stack overflow used to — no `Err`, no exit code, nothing for the plugin to surface, which is the
  exact failure CLAUDE.md §5 forbids and MVP-35/[ADR 16](adr/16-xml-nesting-depth-guard.md) closed
  for depth. Depth and size are separate properties: a file can be small and deeply nested, or
  large and flat, so bounding one does not bound the other. This belongs with the read itself
  rather than with the parser, which is why it is a separate item rather than folded into
  MVP-35's guard.

  Numbered 40 rather than slotted into this section's sequence because numbers are assigned in
  order and never reused.

  **Tests:** Unit test that a file over the size cap fails as a structured error (existing
  `ErrorKind`, if one fits, or a new one if not) rather than attempting the read; a file at or
  under the cap reads normally.

- [ ] **MVP-36 — Unsupported-condition policy for toolchain files**
  `fix/unsupported-condition-policy` · ~5h

  **Summary:** Decide whether an unsupported `Condition` stays fatal once the Visual Studio
  installation's own `.props`/`.targets` start being imported.

  **Description:** MVP-09 made a condition outside the supported subset an
  `Error::UnsupportedConstruct`, on the grounds that inventing a boolean either drops a
  `PropertyGroup` carrying every include path or applies flags the file never asked for. That is
  right for a *user's* project file. It is likely untenable for Microsoft's, which use
  `$([MSBuild]::VersionGreaterThanOrEquals(...))`, `HasTrailingSlash`, and numeric comparison
  freely — taken literally, the policy fails on every real solution.

  [ADR 09](adr/09-condition-evaluation-subset.md) handed the question to MVP-10, and
  [ADR 10](adr/10-import-resolution.md) could not answer it: the signal it needs — whether a file
  came from the user's tree or from the toolchain — does not exist until MVP-15 supplies the
  Visual Studio paths, and `$(VCTargetsPath)` is undefined until then so those imports are never
  reached. `Error::kind()` survives propagation (ADR 02), so catching and downgrading at the
  import boundary is mechanical once provenance exists; what needs deciding is whether downgrading
  is right, and whether a downgraded condition should be treated as true or false.

  Blocked on MVP-15. Numbered 36 rather than slotted into sequence because numbers are assigned in
  order and never reused.

  **Tests:** Unit tests that an unsupported condition in a user-tree file still fails, that one in
  a toolchain-provided file produces the chosen behaviour plus a warning naming the file, and an
  integration test that a real-shaped project importing a stub `Microsoft.Cpp.props` generates.

- [ ] **MVP-37 — Expand wildcard `ClCompile` includes**
  `feat/item-globbing` · ~6h

  **Summary:** Turn `<ClCompile Include="src\*.cpp" />` into the source files it names, instead of
  warning that it was skipped.

  **Description:** Deferred out of MVP-12, which settled the *policy* — a wildcard warns naming the
  spec and contributes nothing — precisely so the silent version could not survive. That closed the
  worst outcome (a project coming out with no source files and no explanation) without taking on
  globbing, but a project written this way still gets an incomplete database.

  Visual Studio never writes a wildcard, so this is for hand-maintained `.vcxproj` files and for the
  shared-item `.props` pattern. Scope: `*` and `?` within a single directory segment via `read_dir`,
  and MSBuild's `**` recursive form. `Exclude` becomes meaningful for the first time — it is
  evaluated against the expanded set, which is the only case where it does real work. Two things to
  be careful about: results must be **sorted**, since `read_dir` order is filesystem order and a
  golden file cannot depend on it; and this is the first item resolution that touches the
  filesystem, so a spec naming a directory that does not exist has to expand to nothing rather than
  erroring, matching MSBuild.

  Numbered 37 rather than slotted into this section's sequence because numbers are assigned in order
  and never reused. Closes the wildcard open question in `engineering_spec.md`. See
  [ADR 12](adr/12-clcompile-flag-extraction.md).

  **Tests:** Unit tests over a temp tree for a single-directory `*`, a `?`, a `**` recursive match,
  `Exclude` subtracting from an expanded set, a wildcard matching nothing expanding to nothing
  rather than erroring, a spec whose directory does not exist, and sorted output across two runs.
  Replace MVP-12's `a_wildcard_warns_and_contributes_nothing` and `a_question_mark_is_a_wildcard_too`
  in `msbuild/items.rs` rather than leaving them contradicting the new behaviour.

  MVP-13 added a severity to those cases, which is what the three-variant `Wildcard` enum in
  `msbuild/items.rs` carries: an `Include`/`Update` wildcard warns that files are missing, an
  `Exclude` with a literal `Include` warns that they were *not removed*, and an `Exclude`/`Remove`
  with nothing in reach is a note. All three collapse when expansion lands — an expanded `Exclude`
  does real work — so **delete the enum rather than keeping branches nothing takes**, and with it
  `include_item`'s `exclusion` decision. Also revisit `settle`'s `Remove` arm, which MVP-13 left
  unconditionally `Harmless` with a latent hole the ADR records. See
  [ADR 13](adr/13-diagnostic-severity-and-wording.md).

- [ ] **MVP-38 — Shell-style `AdditionalOptions` splitting**
  `fix/additional-options-splitting` · ~4h

  **Summary:** Split `AdditionalOptions` on quoting rules rather than on whitespace, so a switch
  carrying a path or a define with a space in it survives.

  **Description:** MVP-12 split on whitespace, which is right for every switch a project realistically
  carries and wrong for exactly one shape: `/DBANNER="hello world"` becomes two flags the project
  never wrote, and `/I"C:\Program Files\x\include"` points clangd at two directories that do not
  exist. MVP-13 declined to fix it and made it **visible** instead — a value containing a `"` now
  warns that it will reach clangd as separate flags — because the fix is not the twenty lines of
  splitter but the decision underneath it.

  That decision is *whose* quoting rules to implement. MSBuild's, `cl`'s response-file rules, and the
  Windows command-line parser (`CommandLineToArgvW`) disagree, most visibly about backslashes
  preceding a quote — `"a\\"` is one thing to one and another to the next — and `.vcxproj` files are
  written by people who tested against `cl` rather than against a spec. Pick one, say why in an ADR,
  and note what the other two would have done differently. clangd consumes the result in clang-cl
  mode, which is the tiebreaker worth weighing.

  Remove the warning MVP-13 added once this lands, and close the `AdditionalOptions` open question
  in `engineering_spec.md`. Numbered 38 rather than slotted into sequence because numbers are
  assigned in order and never reused. See [ADR 13](adr/13-diagnostic-severity-and-wording.md).

  **Tests:** Unit tests for a quoted argument containing a space, a quoted path in a `/I`, an
  unquoted value splitting exactly as it does today, an unterminated quote (which must not hang or
  drop the rest of the line), an embedded escaped quote, and an empty value. Replace MVP-13's
  `additional_options_with_a_quoted_argument_warns` in `msbuild/flags.rs` rather than leaving it
  asserting the corruption this removes.

## Toolchain discovery

- [x] **MVP-14 — Locate Visual Studio via `vswhere`**
  `feat/vswhere-discovery` · ~5h

  **Summary:** Implement `VsWhereToolchain`'s discovery of installed Visual Studio instances.

  **Description:** Invoke `vswhere` from its well-known Program Files location using an
  argument vector — never a shell string — requesting JSON output filtered to instances with
  the C++ workload, and pick the newest when several are installed. Absence of `vswhere` or
  of any suitable instance is a normal, actionable failure that must produce a clear error,
  not a panic, since it is what a user without Visual Studio will hit first. Keep this
  Windows-only behavior behind the existing trait so the crate still builds elsewhere.

  **Outcome:** `toolchain/vswhere.rs` gains `VsInstance` and a small pipeline split at the
  impure/pure boundary: `run_vswhere` shells out and returns raw stdout, `parse_instances` and
  `newest` are pure, and `select_instance` composes them into what `VsWhereToolchain::locate`
  calls. That split is what makes the issue's three test cases — multiple instances, an empty
  result, malformed output — unit-testable against recorded strings with no filesystem, and what
  makes "`vswhere.exe` absent" testable on any OS: `run_vswhere` takes its path as a parameter
  rather than resolving `vswhere_path()` internally, so a test can point it at a path that exists
  nowhere. Filtering settled on the **component** `Microsoft.VisualStudio.Component.VC.Tools.
  x86.x64` rather than the broader **workload** `NativeDesktop`, confirmed with the user, since a
  standalone Build Tools install carries the component without the workload and would otherwise
  read as "no usable installation". Prerelease instances are excluded, `vswhere`'s own default.
  The newest instance wins by comparing `installationVersion` as parsed per-component integers,
  not as a string, since `"9.0" > "17.0"` lexicographically. All three failure shapes — no
  `vswhere`, no matching instance, malformed output — raise the existing `Error::ToolchainNotFound`
  rather than a new kind, since its documented remediation already covers all of them and none of
  the failure modes was named ahead of time the way ADR 02's fourteen variants were. **Nothing
  calls `discover()` from a subcommand yet**, so it wires up the new `locate()` and leaves resolving
  the toolset/Windows SDK layout as `todo!()`, the same position `reserved::seed()` was in until
  MVP-12 — **MVP-15 removes it** and reads `VsInstance::install_path`. See
  [ADR 14](adr/14-vswhere-discovery.md) and "Toolchain discovery" in `engineering_spec.md`.

  **Tests:** Unit tests parsing recorded `vswhere` JSON output (multiple instances, empty
  result, malformed output). Live discovery is verified manually — no test may depend on a
  real installation.

  All three written as specified, plus: version comparison asserted directly against a
  lexicographic-vs-numeric case (`"9.0.0.0"` vs `"17.0.0.0"`), and the missing-`vswhere.exe` case
  from the issue's description (not its test list) made assertable cross-platform by parameterizing
  `run_vswhere` on the path rather than resolving it internally.

- [x] **MVP-15 — Resolve toolset and Windows SDK include paths**
  `feat/toolchain-includes` · ~1d

  **Summary:** Derive the MSVC toolset and Windows SDK system include directories from a
  discovered Visual Studio installation.

  **Description:** Given an install root, read the toolset version file, locate the matching
  `VC/Tools/MSVC/<version>/include` directory, and find the Windows SDK's `ucrt`, `um`, and
  `shared` include directories along with their versioned subdirectory. These paths are what
  make `#include <windows.h>` resolve, so getting them wrong shows up immediately as clangd
  reporting missing includes. Return them ordered as `cl` would search them.

  **Also extends MVP-11's seed.** `$(VCTargetsPath)`, `$(VSInstallDir)` and
  `$(MSBuildExtensionsPath)` are the properties `msbuild/reserved.rs` deliberately left out, because
  they need a real installation and guessing them is the silent-wrong-answer failure this project
  refuses. They belong in that module behind `ToolchainProvider`, arriving through `ProjectScope`,
  rather than in a second seeding mechanism. Every import that depends on them is skipped with an
  empty-path warning until this lands, which is also what unblocks **MVP-36**. See
  [ADR 11](adr/11-reserved-property-seeding.md).

  **Tests:** Unit tests against a fake directory tree mimicking a VS layout, asserting
  correct version selection when several toolsets are installed and a clear error when the
  SDK is absent.

  **Outcome:** `Toolchain` gains `install_path` and a precomputed `vc_targets_path`; `vswhere.rs`
  gains the pipeline that fills in the rest of `discover()`: `read_toolset_version` reads
  `VC/Auxiliary/Build/Microsoft.VCToolsVersion.default.txt` verbatim rather than picking the
  newest `VC/Tools/MSVC/*` directory on disk, since a project can legitimately pin an older
  toolset and "newest" would silently produce flags no real build uses. The Windows SDK is found
  at its own well-known `%ProgramFiles(x86)%\Windows Kits\10` location — mirroring
  `vswhere_path()`'s no-registry pattern rather than adding a dependency to read one — and
  `resolve_sdk` picks the newest version where `ucrt`, `um`, and `shared` **all** exist, skipping
  a newer-but-incomplete install in favor of an older complete one. `msbuild/reserved.rs` gains
  `ProjectScope.toolchain: Option<Toolchain>`; `seed()` writes `$(VCTargetsPath)`,
  `$(VSInstallDir)`, `$(MSBuildExtensionsPath)` when it is `Some` and leaves all three undefined
  when `None`, preserving the exact "empty path, warned about" behavior every import depending on
  them had before this issue for every call site that hasn't discovered a toolchain. Every new
  failure mode reuses the existing `Error::ToolchainIncomplete` — no new `ErrorKind`. See
  [ADR 17](adr/17-toolchain-include-resolution.md).

  All tests written as specified, plus coverage the Description required but the Tests: block
  didn't enumerate: `reserved.rs` gains three tests asserting the toolchain family is seeded when
  present, stays undefined when absent, and holds nothing empty when present — the same contract
  `every_seeded_property_holds_something` already holds the base twenty-four to.

  **Unblocks MVP-16** (compilation database assembly, which appends `Toolchain::include_paths`
  after project includes) and **MVP-36** (toolchain-import condition policy, which needed
  `$(VCTargetsPath)` seeded from a real installation before it could be worked at all).

## Compilation database

- [x] **MVP-16 — Build compilation database entries**
  `feat/compdb-build` · ~6h

  **Summary:** Combine evaluated project settings and toolchain includes into
  `CompileCommand` entries.

  **Description:** Each entry needs an absolute `file`, an absolute `directory`, and an
  argument vector beginning with a clang-cl-compatible driver invocation so clangd interprets
  the MSVC-form flags correctly. Toolchain system includes are appended after project
  includes, matching search order. Use the `arguments` array rather than a `command` string to
  sidestep quoting problems entirely, and ensure the same file appearing in two projects
  produces deterministic output.

  **Unblocked by MVP-12**, which filled in `EvaluatedProject`: each `FileCompilation` arrives with an
  absolute `file`, absolute `include_paths`, `defines` without their `/D`, and `flags` already in
  MSVC form and already deterministically ordered — so this item adds the driver invocation, formats
  `/I` and `/D`, and appends the toolchain's system includes. It does **not** need to re-resolve or
  re-sort anything. Note that a file appearing in two projects arrives twice with possibly different
  flags, which MVP-12 deduplicates only *within* one project. See
  [ADR 12](adr/12-clcompile-flag-extraction.md).

  **Tests:** Unit tests asserting absolute paths, argument ordering (project includes before
  system includes), driver-mode presence, and deterministic ordering across runs.

  **Outcome:** All four written as specified, plus: no dedup across projects (two entries, each
  keeping its own project's flags) and a no-files project contributing nothing. `arguments` is
  `["clang-cl", "--driver-mode=cl"]`, project `/I`s, toolchain `/I`s, `/D`s, `flags` unchanged, then
  the file path trailing as the final argument — full reasoning, including the two orderings
  considered and rejected, in [ADR 18](adr/18-compdb-entry-assembly.md). `EvaluatedProject` and
  `FileCompilation` needed no changes, confirming ADR 12's prediction. `CompileCommand` gained
  `PartialEq` for the determinism test. `compdb::write` (serializing to disk) remains MVP-17's, and
  wiring `generate` to this pipeline is MVP-22's — so nothing here is user-visible yet, and no
  changelog entry was added.

- [x] **MVP-17 — Serialize and write `compile_commands.json`**
  `feat/compdb-write` · ~5h

  **Summary:** Write the database to `.msvclsp/<solution-name>-<hash>/compile_commands.json`,
  creating directories as needed, where `<hash>` disambiguates two discovered solutions that
  share a filename in different directories.

  **Description:** Serialize with stable key and entry ordering so the file is diffable and
  golden tests are reproducible, and write atomically via a temporary file and rename so an
  interrupted run can't leave clangd reading a truncated database. Create the `.msvclsp`
  directory tree on demand. Windows path separators must be escaped correctly in JSON —
  verify against a real clangd, since this is an easy place to emit something that parses but
  doesn't resolve.

  The output directory name is `<solution-name>-<hash>`: `<hash>` is the low 32 bits of a 64-bit
  FNV-1a hash (hand-rolled, not `std::collections::hash_map::DefaultHasher` — its algorithm is
  documented as unspecified across releases, which would silently reshuffle every existing
  `.msvclsp` directory on a future Rust upgrade) of the lowercased, lexically-normalized absolute
  path of the directory containing the `.sln`. Lowercasing matters because Windows paths are
  case-preserving but the filesystem is case-insensitive — hashing as-written would silently split
  one solution's artifacts across two directories depending on invocation casing. On a collision
  (an existing `.msvclsp/<solution-name>-<hash>/` whose recorded source directory doesn't match the
  current one), fail loudly with a specific error rather than silently sharing or overwriting the
  directory — per CLAUDE.md §3, never silently guess. See "Storage Layout" in
  `engineering_spec.md` for the full scheme, including the accepted orphan-directory cost when a
  solution moves. This folds in what **V2-03** was tracking; V2-03 is removed rather than
  duplicated, since nothing implemented the old bare-name scheme yet.

  **Tests:** Unit test round-tripping entries through serialization; test asserting stable
  ordering across runs and that an existing file is replaced atomically; test asserting the same
  solution directory (including a differently-cased spelling of it) always produces the same
  `<solution-name>-<hash>`; test asserting two different solution directories sharing a filename
  produce two different, independently-writable directories; test asserting a genuine collision
  (same `<solution-name>-<hash>` claimed by a different source directory) errors rather than
  overwriting.

  **Outcome:** All five written as specified, plus a sixth covering the "stable ordering across
  runs" half of the atomic-write test at the serialized-bytes level, since the original split it
  from "replaces an existing file." Directory naming/collision detection (`output_dir_name`,
  `resolve_output_dir`) landed separate from serialization (`write`) so the hash tests need no
  filesystem at all. The collision marker is a plain-text `.source` file holding the same
  lowercased, normalized source directory the hash is computed from — one field, so no
  `serde_json` envelope. `Error::StorageCollision` takes exit code `10`, the next unused one
  (`8` stays reserved per ADR 10). Since a genuine FNV-1a collision can't be constructed by
  hand, its test seeds a `.source` marker directly at the path `output_dir_name` would compute,
  rather than exercising a real hash collision — the handling code is real, only the trigger is
  synthetic. Full reasoning in [ADR 19](adr/19-hashed-storage-directory-write.md). `compdb::write`
  still isn't reachable from the CLI — `generate` isn't wired to this pipeline until MVP-22 — so,
  matching MVP-16's own precedent, no changelog entry was added.

## Persistence

- [x] **MVP-18 — State file shape and store initialization**
  `feat/db-schema` · ~5h

  **Summary:** Create the JSON state file under `.msvclsp/` with a solution-scoped shape and a
  version field for future migrations.

  **Description:** Reframed by MVP-01, which chose a JSON file over SQLite. The shape covers
  solutions, their known configuration/platform pairs, the active selection per solution, and
  tracked file modification times — scoped per solution from the start even though only one is
  active in the MVP, since retrofitting that later means a migration. Include a schema version
  field so upgrades are possible, and make `open()` create-or-load idempotently, treating a
  missing file as empty state rather than an error. Writes must be atomic — serialize to a
  temporary file in the same directory, then rename over the target — so neither a crash nor a
  second Neovim instance can leave a torn file. Solution identity must be keyed so it survives
  the working directory changing. Record the final shape in `engineering_spec.md` and close
  that open question.

  **Tests:** Unit tests asserting a fresh state file is created with the expected top-level
  keys, reopening existing state round-trips unchanged, a missing file yields empty state, and
  a malformed or truncated file produces a clear error rather than a panic.

  **Outcome:** All four written as specified, plus a fifth covering the unsupported-version
  half of the version field's own purpose, since the original list didn't separately exercise
  it. Single file at `.msvclsp/state.json` (`version` + a `solutions` map), not one per
  solution. Solution identity turned out to need more than "keyed by the working directory
  surviving" implied: the working discussion surfaced that `--root` is documented to diverge
  from where the solution actually lives, so keys are the solution's path *relative to the
  workspace root* (`..`-climbing, falling back to absolute only when no relative path can be
  expressed — a different Windows drive) rather than the absolute path `engineering_spec.md`
  originally named, so relocating the whole workspace doesn't orphan every entry. `Store`
  gained private `state`/`root` fields to support this; `set_active_config`/`active_config`/
  `is_up_to_date` bodies stay `todo!()` for MVP-19/20. `known_configs` is reserved in the shape
  per the issue's own summary but has no owning accessor yet — flagged in
  [ADR 20](adr/20-state-file-shape.md)'s Consequences rather than invented here. Full reasoning
  in ADR 20. Nothing in the CLI's reachable surface calls `Store::open` yet, so no changelog
  entry, matching MVP-16/17's own precedent.

- [x] **MVP-19 — Persist active configuration/platform per solution**
  `feat/db-active-config` · ~3h

  **Summary:** Implement `set_active_config`/`active_config` keyed on the solution.

  **Description:** Each solution independently remembers its own last-used
  configuration/platform, because solutions may define entirely different names and carrying
  a selection across them would produce an invalid combination. Reading a solution with no
  recorded selection returns `None` rather than a default, letting the caller prompt. Solution
  identity should survive the working directory changing.

  **Outcome:** Both bodies turned out to be a handful of lines each, since ADR 20 already settled
  the shape and `solution_key`: `set_active_config` looks up or inserts a `SolutionRecord` via
  `self.state.solutions.entry(key).or_default()`, sets `active_config`, and calls the existing
  `write_state` to rewrite the file atomically; `active_config` reads the in-memory copy `open()`
  already loaded. The one real decision was **`set_active_config` becomes `&mut self`** — the stub
  was `&self`, which can't mutate or persist anything — write-through on every call, no batching,
  matching `write_state`'s existing per-call atomic-rewrite precedent. Nothing in the CLI calls
  `Store` yet, so this broke no published caller; **MVP-20's `is_up_to_date` may face the same
  question** if it ever updates `files` as a byproduct of checking staleness, left open rather than
  decided here. No new `Error`/`ErrorKind` variants — `write_state`'s existing `Error::io` path
  already covers a failed rewrite. See [ADR 21](adr/21-store-mutation-signature.md) and
  "Persistence" in `engineering_spec.md`. `Store::open` still isn't reachable from the CLI
  (MVP-21/22 wire it up), so, matching MVP-16/17/18's own precedent, no changelog entry.

  **Tests:** Unit tests for set-then-get round trip, `None` for an unknown solution,
  overwriting an existing selection, and two solutions holding independent selections.

  All four written as specified, plus a fifth: `set_active_config_persists_to_disk`, opening a
  second `Store` handle against the same `.msvclsp` directory to prove the write-through actually
  reaches `state.json` rather than only the in-memory copy.

- [x] **MVP-20 — Modification-time staleness tracking**
  `feat/db-staleness` · ~6h

  **Summary:** Record the mtimes of every file consumed during evaluation and use them to
  skip unnecessary reparsing.

  **Description:** The dependency set is everything the evaluation touched — the `.sln`, every
  `.vcxproj`, and every imported `.props`/`.targets`, including ones from the Visual Studio
  installation — so it must be captured during evaluation rather than guessed afterward. A
  solution is up to date only if every recorded file still exists with an unchanged mtime;
  anything else triggers a full reparse. A deleted dependency counts as stale, not an error.

  **Outcome:** Two methods, not one: `Store::record_dependencies(&mut self, solution, files)`
  captures the current mtime of every file passed in, **replacing** whatever `SolutionRecord.files`
  held before rather than merging, since each call represents one complete evaluation's dependency
  set; `Store::is_up_to_date(&self, solution)` stays a pure read, comparing only the *already-
  recorded* set against the filesystem. The issue's own fourth test case — adding a new import
  invalidates the cache — turned out not to need a dependency-set diff: adding an `<Import>` means
  editing the file that names it, and that file's own recorded mtime already catches the edit,
  without `is_up_to_date` ever re-evaluating to discover the new file. A missing recorded file, or
  any other failure reading its mtime, both count as stale in `is_up_to_date`; the same kind of
  failure inside `record_dependencies` is a real `Error::io`, since every file it's given was just
  read successfully moments earlier by the evaluation calling it. No new `Error`/`ErrorKind`
  variants. Full reasoning, including the accepted same-tick mtime-resolution gap, in
  [ADR 22](adr/22-staleness-mtime-comparison.md). `Store::open` still isn't reachable from the CLI
  (MVP-22 wires the `generate` pipeline through it), so, matching MVP-16/17/18/19's own precedent,
  no changelog entry.

  **Tests:** Unit tests asserting an untouched solution reports up-to-date, touching any
  dependency reports stale, a deleted dependency reports stale, and adding a new import
  invalidates the cache.

  All four written as specified (the fourth via touching the project file that would have gained
  the `<Import>`, per the Outcome above), plus three more: a never-recorded solution reports
  stale, `record_dependencies` replaces rather than merges an existing set, and a
  `record_dependencies_persists_to_disk` round trip through a second `Store::open`, matching
  MVP-18/19's own precedent of proving the write-through actually reaches `state.json`.

## CLI subcommands

- [x] **MVP-21 — `list-configs` subcommand**
  `feat/cmd-list-configs` · ~4h

  **Summary:** Emit a solution's available configuration/platform pairs and its remembered
  selection as JSON on stdout.

  **Description:** Wire solution parsing to the persistence layer and serialize the result to
  the schema the plugin's picker consumes, including which pair is currently active so the
  picker can preselect it. This is the first end-to-end path through the codebase, so it also
  validates the error and exit-code plumbing. Keep the JSON shape versioned or at least
  documented, since it is a contract with the plugin.

  **Outcome:** `list_configs` calls `sln::parse_sln`, opens `db::Store` at `args.root().join(
  ".msvclsp")`, and reads `Store::active_config` — the first reachable caller of `Store::open`.
  Read-only: nothing in this subcommand records a selection, so `Store` is bound immutably,
  matching `active_config`'s own `&self` signature (ADR 21). No new `Error`/`ErrorKind` variant —
  `parse_sln` and `Store::open` already surface everything this path can fail with. Full reasoning
  in [ADR 23](adr/23-list-configs-wiring.md).

  **The Tests: block below could not be written as specified.** `fixtures/simple` and
  `fixtures/multi-config` are still README-only stubs — MVP-23 and MVP-24 explicitly own authoring
  them (see MVP-10's own outcome note). Both tests are instead written against a minimal `.sln`
  built in a system temp directory at test time (`TempSolution` in `cli/tests/cli_surface.rs`), and
  the fixture-driven form remains MVP-23/24's to add once those fixtures exist. See ADR 23's
  discussion for why an under-scoped fixture wasn't authored here instead.

  **Tests:** Integration test against the `simple` and `multi-config` fixtures asserting exact
  JSON output; test asserting a nonexistent solution exits non-zero with a structured error.

  Written against a temp-tree `.sln` instead of the named fixtures, per the note above. Also
  covered: available pairs read back in declaration order, `active` reporting `null` with no
  recorded selection versus the seeded selection from a hand-written `.msvclsp/state.json`, the
  `warnings_ride_in_the_envelope` placeholder unblocked with both severities exercised end to end,
  and a unit test in `cli.rs` proving the handler reaches `sln::parse_sln` rather than
  short-circuiting.

- [x] **MVP-22 — `generate` subcommand**
  `feat/cmd-generate` · ~5h

  **Summary:** Generate `compile_commands.json` for a given solution and
  configuration/platform, honoring the staleness cache.

  **Description:** This is the full pipeline: parse the solution, evaluate each member
  project, discover the toolchain, build and write the database, record the selection and
  dependency mtimes, then report the output path and any warnings as JSON. Skip regeneration
  when nothing has changed unless `--force` is passed. An invalid configuration/platform for
  the given solution must fail with a message listing the valid ones.

  Per-project configurations come from `SolutionProject::config_for` (MVP-06): evaluate
  `mapping.project`, **not** the pair the user selected, since the two legitimately differ. A project
  with no mapping takes no part in the chosen configuration; one with `build: false` is unchecked in
  Configuration Manager, and whether its files are still worth indexing for clangd is a decision this
  issue owns — see [ADR 05](adr/05-sln-configuration-mapping.md).

  **Outcome:** `build: false` projects are indexed too — an unchecked-in-Configuration-Manager
  project still needs real completions when its files are open, which is closer to CLAUDE.md's
  stated goal than mirroring exactly what the last build touched. `cli::run`/`generate` take the
  discovered toolchain as a `&dyn ToolchainProvider` argument rather than constructing
  `VsWhereToolchain` internally — `main.rs` supplies the real one, tests supply `FakeToolchain` —
  which is what makes the pipeline testable at all on Linux CI or a machine with no Visual Studio
  install; nothing before this issue had a reachable caller forcing that question. `EvaluatedProject`
  gained a `dependencies: Vec<PathBuf>` field carrying `imports::ProjectTree::files`, previously
  computed and discarded inside `msbuild::evaluate`, which `generate` unions across every evaluated
  project plus the `.sln` itself before calling `Store::record_dependencies` — so editing an imported
  `.props`, or the solution file, invalidates the cache, not only the `.vcxproj` that names them.
  `Store::set_active_config` runs on every successful call, cache hit or not, so `list-configs`'s
  `active` always reflects the last configuration actually asked for. This issue also retires
  `Error::Unimplemented`, `ErrorKind::Unimplemented`, and exit code `9` outright, per ADR 03's
  closing clause — code `9` is left unassigned rather than reserved the way `8` was, since nothing
  shipped ever depended on it. Full reasoning in [ADR 24](adr/24-generate-wiring.md).

  **Tests:** Integration test generating from the `simple` fixture and comparing against its
  golden file; tests covering an invalid configuration, `--force` overriding the cache, and a
  second run being a no-op.

  The golden-file test **could not be written as specified**: `fixtures/simple` and its golden
  don't exist yet (MVP-23/24), so `cli/tests/generate.rs`'s two golden tests stay the `#[ignore]`d
  placeholders they already were, unchanged. The other three, plus two more this issue's own
  decisions call for, are written against a hand-authored `TempProject` tree local to
  `generate.rs` (a real, importless `.vcxproj` + `.cpp`, evaluated for real under `FakeToolchain`)
  rather than the compiled binary, since `generate` now needs a toolchain no subprocess test can
  fake: an invalid configuration (also covered against the real binary in `cli_surface.rs`, since
  that path never reaches toolchain discovery), `--force` overriding the cache, a no-op second run,
  an unbuilt project still being indexed, and both the `.vcxproj` and the `.sln` itself
  invalidating the cache when touched.

## Fixtures and test harness

- [ ] **MVP-23 — `simple` fixture and golden test harness**
  `feat/fixture-simple` · ~6h

  **Summary:** Author the single-project fixture and the shared harness that compares
  generated output against a golden file.

  **Description:** Build a minimal but genuinely valid VS2022 solution — one project, one
  configuration, a couple of real source and header files — that clangd can be pointed at
  manually. The harness injects `FakeToolchain` so golden files stay byte-identical across
  machines, normalizes the repository's absolute path prefix, and offers an opt-in environment
  variable to rewrite goldens. Never regenerate goldens against a real Visual Studio install.

  **Tests:** This issue *is* test infrastructure; success is `cargo test` comparing generated
  output to `cli/tests/golden/simple.json` and failing legibly on mismatch.

- [ ] **MVP-24 — Remaining fixtures**
  `feat/fixtures-complex` · ~1d

  **Summary:** Author the `multi-project`, `props-inheritance`, and `multi-config` fixtures
  with their golden files.

  **Description:** Each targets a specific evaluation hazard: settings leaking across project
  boundaries, property sheet override precedence, and non-default configuration names.
  `props-inheritance` matters most — import ordering is where subtly wrong flags are most
  likely to originate — so it should include a project that overrides an inherited value and a
  sheet imported by two projects. Keep them small enough to read in full when a golden diff
  needs explaining.

  **Tests:** Golden-file integration tests per fixture, plus assertions that per-project
  settings don't bleed and that overrides win over inherited values.

## Neovim plugin

- [ ] **MVP-39 — Migrate the plugin test harness from `plenary.nvim` to `mini.test`**
  `chore/nvim-mini-test` · ~6h

  **Summary:** Replace `plenary.nvim`'s busted-style test harness with `mini.test`'s, before
  MVP-25 through MVP-34 write real specs against it.

  **Description:** `plenary.nvim`'s own README now carries a deprecation notice — no new
  features, critical-bug-only support through 2026-06-30, and no successor named. Right now
  `nvim/tests/discovery_spec.lua` is still all `pending()` placeholders and every plugin issue
  from MVP-25 onward is unstarted, so this is the one point where migrating costs a rewrite of
  one placeholder file and the harness plumbing rather than a rewrite of dozens of real specs
  later. Swap `minimal_init.lua`'s plenary bootstrap for `mini.test`'s, using its
  `emulate_busted = true` mode so the existing `describe`/`it`/`pending` shape needs no rewrite
  in the spec files themselves. Update the CI workflow, README, and CLAUDE.md's command and
  etiquette references from `PlenaryBustedDirectory` to `mini.test`'s runner, and reword every
  `docs/todos.md` entry from MVP-25 onward that still says "plenary specs" — those are
  unstarted specifications, not completed work, so they get corrected in place rather than
  left to drift.

  **Tests:** This issue *is* test infrastructure — success is `discovery_spec.lua`'s existing
  `pending()` cases still discovered and reported, not silently dropped, by the new runner,
  both locally and in CI.

- [ ] **MVP-25 — Configuration defaults and `setup()`**
  `feat/nvim-config` · ~3h

  **Summary:** Implement option merging with validation over the defaults in `config.lua`.

  **Description:** Merge user options over defaults, validate types and reject unknown keys
  with a clear `vim.notify` error rather than failing mysteriously later, and expose the
  resolved table to other modules. Every configurable value lives here with a default — no
  magic values inline elsewhere. Calling `setup()` twice should be safe.

  **Tests:** mini.test specs for default values, user overrides, deep-merge of nested tables,
  rejection of an unknown key and of a wrong-typed value.

- [ ] **MVP-26 — Project root detection**
  `feat/nvim-root-detection` · ~3h

  **Summary:** Locate the project root by walking up from the current buffer for the
  configured root markers.

  **Description:** Use the same marker convention Neovim's LSP clients already use so
  `.msvclsp/` lands where users expect, walking upward from the buffer's directory and
  stopping at the first match. With no marker found, fall back to the current working
  directory rather than erroring, and cache the result per buffer since this runs on every
  command. Handle unnamed and non-file buffers without throwing.

  **Tests:** mini.test specs over a temporary directory tree covering a marker at the root, a
  marker several levels up, no marker present, and an unnamed buffer.

- [ ] **MVP-27 — Solution discovery**
  `feat/nvim-solution-discovery` · ~5h

  **Summary:** Scan the project tree for `.sln` files and cache the results in memory.

  **Description:** Walk downward from the project root collecting `.sln` paths, skipping the
  directories that make this slow and useless — `.git`, `node_modules`, build output — with a
  configurable depth limit. Results are cached per project root so repeated commands don't
  re-walk, with an explicit invalidation entry point that MVP's rescan command and V1's
  workflows both use. The MVP assumes a single solution and simply takes the first result when
  several are found; the picker arrives in V1.

  **Tests:** mini.test specs over a temporary tree covering one solution, a nested solution,
  ignored directories being skipped, no solutions found, and the second call hitting the
  cache.

- [ ] **MVP-28 — Async CLI invocation and JSON decoding**
  `feat/nvim-cli-bridge` · ~6h

  **Summary:** Implement `cli.lua` as the single, stubbable boundary between the plugin and
  the binary.

  **Description:** Invoke the binary asynchronously with an argument list, collect stdout and
  stderr separately, decode stdout as JSON, and hand `(result, err)` to a callback — never
  blocking the UI, since parsing a large solution takes real time. Non-zero exit codes and
  undecodable output both become structured errors carrying stderr's contents so failures are
  legible. Every other module goes through this boundary so tests can stub it wholesale.

  **Tests:** mini.test specs with a stubbed job runner covering successful decode, non-zero
  exit, malformed JSON, and a missing binary — no spec invokes the real binary.

- [ ] **MVP-29 — Configuration/platform picker**
  `feat/nvim-config-picker` · ~4h

  **Summary:** Present the solution's configuration/platform options via `vim.ui.select` and
  persist the choice through the CLI.

  **Description:** Call `list-configs`, present the results with the active pair marked, and
  on selection trigger regeneration. Using `vim.ui.select` rather than a bespoke picker means
  the user's existing picker UI applies automatically. Cancelling must be a clean no-op, and
  the in-flight CLI call should give feedback so the editor doesn't appear frozen.

  **Tests:** mini.test specs with stubbed CLI covering options passed to `vim.ui.select`, the
  active entry being marked, selection triggering generation, and cancellation doing nothing.

- [ ] **MVP-30 — clangd lifecycle**
  `feat/nvim-clangd-lifecycle` · ~5h

  **Summary:** Start and restart clangd pointed explicitly at the generated database.

  **Description:** Launch clangd with an explicit `--compile-commands-dir` rather than relying
  on its directory-walking discovery, because a solution's location often doesn't line up with
  where sources live. Restart cleanly after regeneration or a configuration switch, stopping
  the existing client and reattaching open buffers. If the database doesn't exist yet, say so
  via `vim.notify` instead of starting a client that will silently fail.

  **Tests:** mini.test specs asserting the correct `--compile-commands-dir` argument, restart
  stopping the previous client, buffers reattaching, and a missing database producing a
  notification rather than a start attempt.

- [ ] **MVP-31 — User commands**
  `feat/nvim-commands` · ~3h

  **Summary:** Register the `:Msvclsp*` commands and wire them to their implementations.

  **Description:** Register commands for generating the database, selecting a
  configuration/platform, rescanning, and restarting clangd, each with a description that
  reads well in command completion. Claim no keymaps — document suggested bindings instead —
  and register lazily so the load guard in `plugin/msvclsp.lua` keeps startup cost at zero.
  Commands invoked before `setup()` should say so clearly.

  **Tests:** mini.test specs asserting each command exists after `setup()`, dispatches to the
  right module, and that invocation before setup produces a clear error.

- [ ] **MVP-32 — `:checkhealth msvclsp`**
  `feat/nvim-health` · ~4h

  **Summary:** Report on the environment conditions users most often get wrong.

  **Description:** Check that the binary is present and runnable and report its version, that
  clangd is on `PATH`, that a project root is detectable, that a solution has been found, and
  that a configuration/platform is selected with a database present on disk. Each failure
  should carry an actionable remedy rather than just a red mark, since this is the first place
  a user looks when nothing works.

  **Tests:** mini.test specs with stubbed CLI and filesystem covering all-healthy, missing
  binary, missing clangd, no solution found, and no database generated.

- [ ] **MVP-33 — User documentation**
  `docs/nvim-user-docs` · ~5h

  **Summary:** Fill in `nvim/doc/msvclsp.txt` and expand the README's installation and usage
  sections.

  **Description:** Cover installation, a minimal `setup()` example, every command and
  configuration option, suggested keymaps as documentation rather than defaults, and a
  troubleshooting section pointing at `:checkhealth`. Include the first-run walkthrough — open
  a project, pick a configuration, generate, see clangd attach — since that path is where a
  new user either succeeds or gives up. Generate help tags so `:help msvclsp` works.

  **Tests:** Manual — follow the written installation steps in a clean Neovim configuration
  and confirm they work end to end.

- [ ] **MVP-34 — End-to-end verification against real clangd**
  `docs/mvp-e2e-verification` · ~5h

  **Summary:** Run the manual verification checklist against both a fixture and a real
  Visual Studio solution, and record the results.

  **Description:** This is the milestone's actual acceptance gate: generate a database, open a
  source file, and confirm completion, go-to-definition, and diagnostics work with no missing
  includes reported, then switch configuration and confirm flags visibly change. A real
  solution matters as much as the fixtures, since fixtures encode assumptions that real
  projects violate. Capture whatever fails as new issues rather than fixing it inline.

  **Tests:** Manual, following the checklist in `CLAUDE.md` section 8. Update the changelog
  and mark the MVP milestone complete once it passes.

---

# V1 — Multi-solution and workflow polish

Detail here is deliberately lighter; flesh these out when the MVP is done and the assumptions
behind them have been tested.

- [ ] **V1-01 — `discover` subcommand** · `feat/cmd-discover` · ~5h
  Move solution discovery behind the CLI so scanning rules live in one place rather than being
  duplicated in Lua. Emit JSON with each solution's path and display name. Tests: fixture-tree
  integration tests covering nested solutions and ignored directories.

- [ ] **V1-02 — Namespace output per solution** · `feat/output-namespacing` · ~4h
  Write to `.msvclsp/<solution-name>/` so a previously-used solution's artifacts survive
  switching away and back. Tests: two solutions generating into distinct directories without
  clobbering each other.

- [ ] **V1-03 — Solution picker with persistence** · `feat/nvim-solution-picker` · ~6h
  Present discovered solutions via `vim.ui.select` and remember the choice per project root so
  later sessions don't re-prompt. Tests: mini.test specs for selection, persistence across
  simulated restarts, and cancellation.

- [ ] **V1-04 — Rescan command** · `feat/nvim-rescan` · ~2h
  Invalidate the discovery cache and re-scan on demand for when solutions are added, moved, or
  removed. Tests: mini.test spec asserting the cache is dropped and repopulated.

- [ ] **V1-05 — Switch-solution command** · `feat/nvim-switch-solution` · ~5h
  Change the active solution, restore *that* solution's own remembered configuration/platform,
  regenerate if needed, and restart clangd. Tests: mini.test specs asserting per-solution
  configuration restoration rather than a carried-over global value.

- [ ] **V1-06 — Inline first-run discovery** · `feat/nvim-inline-discovery` · ~3h
  Run discovery automatically when selection is requested and no cache exists, keeping it one
  user action instead of two. Tests: mini.test spec asserting discovery runs then the picker
  appears.

---

# V2 — Hardening and edge cases

- [ ] **V2-01 — Handle a vanished active solution** · `fix/missing-solution` · ~4h
  Detect that a remembered solution's `.sln` no longer exists and prompt for a new one instead
  of failing obscurely. Tests: unit and mini.test specs covering a deleted and a moved solution.

- [ ] **V2-02 — Config-based solution pinning** · `feat/solution-pin` · ~4h
  Allow an explicit solution path via configuration for CI and scripting, bypassing the
  picker entirely. Tests: assert the pinned solution wins over any remembered selection.

- [ ] **V2-04 — Surface CLI errors in the plugin UI** · `feat/nvim-error-surfacing` · ~4h
  Present structured CLI errors and evaluation warnings legibly rather than letting them
  vanish into a silent no-op. Tests: mini.test specs asserting each error kind produces a
  distinct, actionable notification.

- [ ] **V2-05 — Expanded documentation** · `docs/multi-solution-docs` · ~4h
  Document multi-solution workflows and troubleshooting once V1 behavior has settled. Tests:
  manual read-through against actual behavior.
