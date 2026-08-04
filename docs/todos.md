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
  `Error::PathEscape` remains unused until then. See
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

- [ ] **MVP-07 — Read `.vcxproj` structure**
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

- [ ] **MVP-08 — Property bag and `$(Macro)` expansion**
  `feat/msbuild-macros` · ~1d

  **Summary:** Implement the property store and recursive expansion of `$(Name)` references
  within property values.

  **Description:** Properties are assigned in order with later definitions shadowing earlier
  ones, and their values may themselves reference other properties, including ones defined
  later in some cases. Implement expansion with a recursion depth limit so a self-referential
  or cyclic property fails cleanly instead of hanging. Unknown properties expand to the empty
  string, matching MSBuild, but should be recorded so MVP-13 can warn about them.

  **Tests:** Unit tests for simple expansion, nested expansion, redefinition ordering, an
  undefined property expanding to empty, and a cyclic reference erroring at the depth limit.

- [ ] **MVP-09 — `Condition` expression evaluation**
  `feat/msbuild-conditions` · ~1d

  **Summary:** Evaluate the subset of MSBuild condition syntax that real `.vcxproj` and
  `.props` files actually use.

  **Description:** In practice this means quoted string equality and inequality
  (`'$(Configuration)|$(Platform)'=='Debug|x64'`), boolean `And`/`Or`/`!`, parentheses, and
  the `Exists()` function; the full MSBuild grammar is far larger and out of scope. Macros
  are expanded before comparison. Anything outside the supported subset must be reported as
  unsupported rather than silently evaluating false, because a silently-skipped
  `PropertyGroup` produces a plausible but wrong compilation database.

  **Tests:** Unit tests per operator, a compound `And`/`Or` expression, `Exists()` against a
  real and missing path, case-insensitive comparison behavior, and an unsupported construct
  producing an error.

- [ ] **MVP-10 — Resolve `.props`/`.targets` imports**
  `feat/msbuild-imports` · ~1d

  **Summary:** Recursively follow `<Import>` elements and return the full file list in
  evaluation order.

  **Description:** Import paths are themselves macro-expanded and condition-gated, so
  resolution has to interleave with MVP-08 and MVP-09 rather than run as a separate pre-pass.
  Track visited files to detect cycles, cap recursion depth, and treat a missing import as an
  error unless the `Import` is marked conditional. Imports reaching outside the project tree
  (into the Visual Studio installation) are expected and legitimate, but paths that escape via
  `..` traversal from untrusted input should be validated.

  **Tests:** Unit tests for a linear import chain, a diamond import, a cycle producing an
  error, a conditional import that evaluates false being skipped, and a missing required
  import erroring with the referencing file named.

- [ ] **MVP-11 — Reserved and well-known MSBuild properties**
  `feat/msbuild-reserved-properties` · ~5h

  **Summary:** Seed the property bag with the built-in properties that project files assume
  exist.

  **Description:** Files reference `$(SolutionDir)`, `$(ProjectDir)`, `$(ProjectName)`,
  `$(Configuration)`, `$(Platform)`, `$(MSBuildThisFileDirectory)`, and similar without ever
  defining them, so evaluation produces empty include paths without these. Populate them from
  the solution and project being evaluated, with `MSBuildThisFile*` varying per file during
  import traversal. Directory-valued properties must carry MSBuild's trailing separator, since
  project files concatenate them directly.

  **Tests:** Unit tests asserting each property resolves to the expected absolute path,
  trailing separators are present, and `MSBuildThisFileDirectory` differs correctly between a
  project and an imported property sheet.

- [ ] **MVP-12 — Extract compiler settings into flags**
  `feat/clcompile-flags` · ~1d

  **Summary:** Turn evaluated `ClCompile` settings into per-file include paths, defines, and
  MSVC-form compiler flags.

  **Description:** Read `AdditionalIncludeDirectories`, `PreprocessorDefinitions`,
  `LanguageStandard`, `RuntimeLibrary`, `ExceptionHandling`, and the other settings that
  affect parsing, mapping each to its `cl` flag rather than a clang equivalent. Handle
  `%(Inherited)` semantics so project-level settings compose with item-level overrides, and
  resolve include paths to absolute form. Settings that only affect codegen or linking can be
  skipped — clangd only needs what changes how the file parses.

  **Tests:** Unit tests per setting mapping, a file-level override beating the project-level
  default, `%(Inherited)` composition, and semicolon-delimited lists splitting correctly.

- [ ] **MVP-13 — Warn on unsupported MSBuild constructs**
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

  **Tests:** Unit tests asserting a warning is recorded for an unsupported condition function
  and an undefined property; integration test asserting warnings appear in both stderr and
  the JSON output.

- [ ] **MVP-35 — Bound XML nesting depth before parsing**
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

  **Tests:** Un-`#[ignore]` `a_deeply_nested_document_is_rejected_rather_than_crashing` in
  `msbuild/project.rs`, which already asserts `ErrorKind::RecursionLimit`. Add a unit test that a
  file at the limit still parses, and one that `<` inside a comment or CDATA does not count
  toward depth.

## Toolchain discovery

- [ ] **MVP-14 — Locate Visual Studio via `vswhere`**
  `feat/vswhere-discovery` · ~5h

  **Summary:** Implement `VsWhereToolchain`'s discovery of installed Visual Studio instances.

  **Description:** Invoke `vswhere` from its well-known Program Files location using an
  argument vector — never a shell string — requesting JSON output filtered to instances with
  the C++ workload, and pick the newest when several are installed. Absence of `vswhere` or
  of any suitable instance is a normal, actionable failure that must produce a clear error,
  not a panic, since it is what a user without Visual Studio will hit first. Keep this
  Windows-only behavior behind the existing trait so the crate still builds elsewhere.

  **Tests:** Unit tests parsing recorded `vswhere` JSON output (multiple instances, empty
  result, malformed output). Live discovery is verified manually — no test may depend on a
  real installation.

- [ ] **MVP-15 — Resolve toolset and Windows SDK include paths**
  `feat/toolchain-includes` · ~1d

  **Summary:** Derive the MSVC toolset and Windows SDK system include directories from a
  discovered Visual Studio installation.

  **Description:** Given an install root, read the toolset version file, locate the matching
  `VC/Tools/MSVC/<version>/include` directory, and find the Windows SDK's `ucrt`, `um`, and
  `shared` include directories along with their versioned subdirectory. These paths are what
  make `#include <windows.h>` resolve, so getting them wrong shows up immediately as clangd
  reporting missing includes. Return them ordered as `cl` would search them.

  **Tests:** Unit tests against a fake directory tree mimicking a VS layout, asserting
  correct version selection when several toolsets are installed and a clear error when the
  SDK is absent.

## Compilation database

- [ ] **MVP-16 — Build compilation database entries**
  `feat/compdb-build` · ~6h

  **Summary:** Combine evaluated project settings and toolchain includes into
  `CompileCommand` entries.

  **Description:** Each entry needs an absolute `file`, an absolute `directory`, and an
  argument vector beginning with a clang-cl-compatible driver invocation so clangd interprets
  the MSVC-form flags correctly. Toolchain system includes are appended after project
  includes, matching search order. Use the `arguments` array rather than a `command` string to
  sidestep quoting problems entirely, and ensure the same file appearing in two projects
  produces deterministic output.

  **Tests:** Unit tests asserting absolute paths, argument ordering (project includes before
  system includes), driver-mode presence, and deterministic ordering across runs.

- [ ] **MVP-17 — Serialize and write `compile_commands.json`**
  `feat/compdb-write` · ~3h

  **Summary:** Write the database to `.msvclsp/<solution-name>/compile_commands.json`,
  creating directories as needed.

  **Description:** Serialize with stable key and entry ordering so the file is diffable and
  golden tests are reproducible, and write atomically via a temporary file and rename so an
  interrupted run can't leave clangd reading a truncated database. Create the `.msvclsp`
  directory tree on demand. Windows path separators must be escaped correctly in JSON —
  verify against a real clangd, since this is an easy place to emit something that parses but
  doesn't resolve.

  **Tests:** Unit test round-tripping entries through serialization; test asserting stable
  ordering across runs and that an existing file is replaced atomically.

## Persistence

- [ ] **MVP-18 — State file shape and store initialization**
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

- [ ] **MVP-19 — Persist active configuration/platform per solution**
  `feat/db-active-config` · ~3h

  **Summary:** Implement `set_active_config`/`active_config` keyed on the solution.

  **Description:** Each solution independently remembers its own last-used
  configuration/platform, because solutions may define entirely different names and carrying
  a selection across them would produce an invalid combination. Reading a solution with no
  recorded selection returns `None` rather than a default, letting the caller prompt. Solution
  identity should survive the working directory changing.

  **Tests:** Unit tests for set-then-get round trip, `None` for an unknown solution,
  overwriting an existing selection, and two solutions holding independent selections.

- [ ] **MVP-20 — Modification-time staleness tracking**
  `feat/db-staleness` · ~6h

  **Summary:** Record the mtimes of every file consumed during evaluation and use them to
  skip unnecessary reparsing.

  **Description:** The dependency set is everything the evaluation touched — the `.sln`, every
  `.vcxproj`, and every imported `.props`/`.targets`, including ones from the Visual Studio
  installation — so it must be captured during evaluation rather than guessed afterward. A
  solution is up to date only if every recorded file still exists with an unchanged mtime;
  anything else triggers a full reparse. A deleted dependency counts as stale, not an error.

  **Tests:** Unit tests asserting an untouched solution reports up-to-date, touching any
  dependency reports stale, a deleted dependency reports stale, and adding a new import
  invalidates the cache.

## CLI subcommands

- [ ] **MVP-21 — `list-configs` subcommand**
  `feat/cmd-list-configs` · ~4h

  **Summary:** Emit a solution's available configuration/platform pairs and its remembered
  selection as JSON on stdout.

  **Description:** Wire solution parsing to the persistence layer and serialize the result to
  the schema the plugin's picker consumes, including which pair is currently active so the
  picker can preselect it. This is the first end-to-end path through the codebase, so it also
  validates the error and exit-code plumbing. Keep the JSON shape versioned or at least
  documented, since it is a contract with the plugin.

  **Tests:** Integration test against the `simple` and `multi-config` fixtures asserting exact
  JSON output; test asserting a nonexistent solution exits non-zero with a structured error.

- [ ] **MVP-22 — `generate` subcommand**
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

  **Tests:** Integration test generating from the `simple` fixture and comparing against its
  golden file; tests covering an invalid configuration, `--force` overriding the cache, and a
  second run being a no-op.

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

- [ ] **MVP-25 — Configuration defaults and `setup()`**
  `feat/nvim-config` · ~3h

  **Summary:** Implement option merging with validation over the defaults in `config.lua`.

  **Description:** Merge user options over defaults, validate types and reject unknown keys
  with a clear `vim.notify` error rather than failing mysteriously later, and expose the
  resolved table to other modules. Every configurable value lives here with a default — no
  magic values inline elsewhere. Calling `setup()` twice should be safe.

  **Tests:** plenary specs for default values, user overrides, deep-merge of nested tables,
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

  **Tests:** plenary specs over a temporary directory tree covering a marker at the root, a
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

  **Tests:** plenary specs over a temporary tree covering one solution, a nested solution,
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

  **Tests:** plenary specs with a stubbed job runner covering successful decode, non-zero
  exit, malformed JSON, and a missing binary — no spec invokes the real binary.

- [ ] **MVP-29 — Configuration/platform picker**
  `feat/nvim-config-picker` · ~4h

  **Summary:** Present the solution's configuration/platform options via `vim.ui.select` and
  persist the choice through the CLI.

  **Description:** Call `list-configs`, present the results with the active pair marked, and
  on selection trigger regeneration. Using `vim.ui.select` rather than a bespoke picker means
  the user's existing picker UI applies automatically. Cancelling must be a clean no-op, and
  the in-flight CLI call should give feedback so the editor doesn't appear frozen.

  **Tests:** plenary specs with stubbed CLI covering options passed to `vim.ui.select`, the
  active entry being marked, selection triggering generation, and cancellation doing nothing.

- [ ] **MVP-30 — clangd lifecycle**
  `feat/nvim-clangd-lifecycle` · ~5h

  **Summary:** Start and restart clangd pointed explicitly at the generated database.

  **Description:** Launch clangd with an explicit `--compile-commands-dir` rather than relying
  on its directory-walking discovery, because a solution's location often doesn't line up with
  where sources live. Restart cleanly after regeneration or a configuration switch, stopping
  the existing client and reattaching open buffers. If the database doesn't exist yet, say so
  via `vim.notify` instead of starting a client that will silently fail.

  **Tests:** plenary specs asserting the correct `--compile-commands-dir` argument, restart
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

  **Tests:** plenary specs asserting each command exists after `setup()`, dispatches to the
  right module, and that invocation before setup produces a clear error.

- [ ] **MVP-32 — `:checkhealth msvclsp`**
  `feat/nvim-health` · ~4h

  **Summary:** Report on the environment conditions users most often get wrong.

  **Description:** Check that the binary is present and runnable and report its version, that
  clangd is on `PATH`, that a project root is detectable, that a solution has been found, and
  that a configuration/platform is selected with a database present on disk. Each failure
  should carry an actionable remedy rather than just a red mark, since this is the first place
  a user looks when nothing works.

  **Tests:** plenary specs with stubbed CLI and filesystem covering all-healthy, missing
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
  later sessions don't re-prompt. Tests: plenary specs for selection, persistence across
  simulated restarts, and cancellation.

- [ ] **V1-04 — Rescan command** · `feat/nvim-rescan` · ~2h
  Invalidate the discovery cache and re-scan on demand for when solutions are added, moved, or
  removed. Tests: plenary spec asserting the cache is dropped and repopulated.

- [ ] **V1-05 — Switch-solution command** · `feat/nvim-switch-solution` · ~5h
  Change the active solution, restore *that* solution's own remembered configuration/platform,
  regenerate if needed, and restart clangd. Tests: plenary specs asserting per-solution
  configuration restoration rather than a carried-over global value.

- [ ] **V1-06 — Inline first-run discovery** · `feat/nvim-inline-discovery` · ~3h
  Run discovery automatically when selection is requested and no cache exists, keeping it one
  user action instead of two. Tests: plenary spec asserting discovery runs then the picker
  appears.

---

# V2 — Hardening and edge cases

- [ ] **V2-01 — Handle a vanished active solution** · `fix/missing-solution` · ~4h
  Detect that a remembered solution's `.sln` no longer exists and prompt for a new one instead
  of failing obscurely. Tests: unit and plenary specs covering a deleted and a moved solution.

- [ ] **V2-02 — Config-based solution pinning** · `feat/solution-pin` · ~4h
  Allow an explicit solution path via configuration for CI and scripting, bypassing the
  picker entirely. Tests: assert the pinned solution wins over any remembered selection.

- [ ] **V2-03 — Resolve solution name collisions** · `fix/name-collisions` · ~4h
  Disambiguate two discovered solutions sharing a filename so their `.msvclsp/<name>/`
  directories don't collide. Tests: two same-named solutions in different directories
  generating independently.

- [ ] **V2-04 — Surface CLI errors in the plugin UI** · `feat/nvim-error-surfacing` · ~4h
  Present structured CLI errors and evaluation warnings legibly rather than letting them
  vanish into a silent no-op. Tests: plenary specs asserting each error kind produces a
  distinct, actionable notification.

- [ ] **V2-05 — Expanded documentation** · `docs/multi-solution-docs` · ~4h
  Document multi-solution workflows and troubleshooting once V1 behavior has settled. Tests:
  manual read-through against actual behavior.
