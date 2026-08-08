# Engineering Spec

This document defines the technical requirements for *how* MSVC LSP is built: tech stack,
architecture and system design, engineering requirements, and testing strategy. It implements the
product intent described in [project_spec.md](project_spec.md) and [brainstorm.md](brainstorm.md).
Several details remain open and are tracked at the end of this document rather than guessed at.

# Decision Records

This document describes the design as it currently stands. *Why* it stands that way — the options
weighed, what was rejected, and what each choice costs — is recorded in the architecture decision
records under [adr/](adr/), one file per decision, each tied to the [todos.md](todos.md) item it
came out of. When a section here summarizes a decision, it links to the ADR holding the full
argument; read that before reopening the question. Conventions for writing them are in section 9 of
[../CLAUDE.md](../CLAUDE.md).

| ADR | Decision |
| --- | --- |
| [01](adr/01-address-dependencies.md) | CLI dependencies, persistence approach, MSRV and edition (MVP-01). |
| [02](adr/02-structured-error-type.md) | Structured error type with machine-readable kinds and exit codes (MVP-02). |
| [03](adr/03-cli-surface-and-json-contract.md) | Subcommand surface and the JSON envelope contract (MVP-03). |
| [04](adr/04-sln-project-enumeration.md) | `.sln` member enumeration and path resolution (MVP-05). |
| [05](adr/05-sln-configuration-mapping.md) | Solution configurations and the per-project mapping (MVP-06). |
| [06](adr/06-diagnostic-collection.md) | Diagnostic collection via a threaded `Reporter`, and stream discipline (MVP-04). |

# Tech Stack

## CLI Binary

- **Language:** Rust, edition 2024, MSRV 1.85.
- **Dependencies** — four direct, 20 packages including transitives. See "Dependency Decisions"
  below for the reasoning behind each, and for what was rejected.
  - XML parsing (`.vcxproj`/`.props`/`.targets`) — `roxmltree`.
  - CLI argument/subcommand parsing — `clap`, with derive, `default-features = false`.
  - Serialization (JSON output for the plugin contract, `compile_commands.json`, and the state
    file) — `serde` + `serde_json`.
  - Error handling — `thiserror`.
  - Persistence — no crate; a JSON state file written through `serde_json`.
  - Logging — no crate; deferred (see below).
- **Toolchain/SDK discovery** will need to shell out to or otherwise query `vswhere` (or equivalent)
  to locate installed Visual Studio instances, MSVC toolset versions, and Windows SDK versions —
  this is not derivable from the `.vcxproj` files themselves.

## Dependency Decisions

Resolved in MVP-01. Recorded here because the rejected options are the kind of thing that gets
proposed again a year later; see [ADR 01](adr/01-address-dependencies.md) for the full record,
including the discussion behind each rejection and the consequences accepted.

**`roxmltree` over `quick-xml`.** MSBuild evaluation is not a single forward pass: import
resolution interleaves with property expansion and condition evaluation, so nodes get revisited
and parents get walked. That calls for a DOM, and `roxmltree`'s is read-only and dependency-free.
`quick-xml`'s main draw is its serde integration, and that is unusable here — `PropertyGroup`
children are arbitrary user-defined element names (`<MyCustomInclude>`), which no derived schema
can model. Generic tree access is required either way, and these files are kilobytes, so
`quick-xml`'s throughput advantage buys nothing.

*Consequence to design around:* `roxmltree` borrows from the source `&str`, so lifetimes
propagate. Rather than keeping every file's text alive in an arena, `msbuild.rs` should parse one
file at a time and lower it into an owned IR, dropping the `Document` before moving to the next
import.

**A JSON state file instead of SQLite.** `db.rs` stores known configuration/platform pairs, the
last-used selection per solution, and dependency mtimes — a few KB of key-value data. Against that,
`rusqlite` on Windows effectively requires the `bundled` feature, because there is no system SQLite
dev library to link; `bundled` compiles the SQLite C amalgamation via `cc`, making a working C
toolchain a hard requirement to build msvclsp at all. That is a poor trade when building from source
is a live distribution option and the plugin's users are Neovim users, not necessarily C developers.

*Tradeoffs accepted:* SQLite would have given transactional writes for free, so two Neovim
instances in one project root are now last-writer-wins. Writes must be atomic (temp file + rename)
so a torn file is impossible even though a lost update is possible. Staleness checks over a few
thousand mtimes become a JSON parse rather than a query — milliseconds at MVP scale. Revisit if
either bites.

**`thiserror`, not `anyhow`.** The plugin must distinguish "solution not found" from "unsupported
MSBuild construct" from "no Visual Studio installation" to map failures onto exit codes and JSON
error kinds. `anyhow`'s type erasure would mean downcasting to recover the kind, which defeats the
contract; it is not used even in `main.rs`, where it would only obscure the kind-to-exit-code
mapping. `thiserror` is a proc macro with no runtime component, and the variant count will be high
enough that hand-written `Display`/`Error` impls become the larger maintenance cost.

**`clap` with a trimmed feature set.** Three subcommands is small enough to hand-roll over
`std::env::args`, but the tool is invoked by hand constantly while debugging, where discoverable
`--help` earns its keep, and derive keeps definitions declarative in `cli.rs` so `main.rs` stays
thin. `default-features = false` drops `color` and `suggestions` — styled help and fuzzy matching
are close to worthless when the primary consumer is a program.

**Logging deferred, not chosen.** The important diagnostic requirement — warn on unsupported
MSBuild constructs — is *structured data the plugin must surface*, not a log line, because the
plugin is forbidden from scraping human-readable text. Those warnings therefore belong in the JSON
contract as a `warnings` array. What remains for a facade is developer-facing debug tracing while
building the evaluator, which no current milestone blocks on. `eprintln!` covers the few human
progress messages. When tracing is genuinely wanted, prefer `log` over `tracing`: spans pay off in
async and long-running services, whereas here the nesting context worth having ("evaluating
`a.props`, imported by `b.vcxproj`") belongs in the error and diagnostic payloads where the plugin
can use it.

**Version pinning.** `Cargo.toml` carries ordinary caret requirements; `cli/Cargo.lock` is
committed, which is where exact reproducibility actually lives for a binary crate.

**MSRV 1.85** is set by `clap` 4.6, the highest floor in the tree — every other dependency sits at
1.71 or below. It is recorded as `rust-version` in `Cargo.toml` so a too-old toolchain fails with a
clear cargo message rather than a confusing compile error.

**Edition 2024**, which requires exactly 1.85 and therefore costs no compatibility beyond the floor
`clap` already imposes. Edition and MSRV are independent knobs and it's worth not confusing them:
edition 2021 needs only Rust 1.56, so staying on it would have locked out nobody — but it would
have protected nobody either, since `clap` sets the real gate. The lever for actually supporting
older toolchains is `clap` 4.5, which holds MSRV 1.74 across all its patch releases; that was
rejected because msvclsp only *runs* on Windows, where Rust arrives via rustup rather than a distro
package, so the population stranded between 1.74 and 1.85 is small and `rustup update` fixes it.
Migrating while the codebase was still stubs made the change free; the cost only grows once the
evaluator exists. Note that edition 2024 also selects style edition 2024, which changes `use`
statement ordering — `cargo fmt` output differs from 2021, so don't be surprised by the reordering.

## Neovim Plugin

- **Language:** Lua.
- Uses standard Neovim APIs (`vim.ui.select`, `vim.system`/`jobstart` for invoking the binary,
  `vim.lsp` for starting/restarting `clangd`).
- Testing via `mini.test`'s test harness (busted-style), run headless.

## Build/Tooling

- The Rust codebase should build and lint (`cargo build`, `cargo clippy`, `cargo fmt --check`) on
  any OS to keep contributor setup and CI simple.
- Actually *running* the tool against a real solution — toolchain discovery, generating a usable
  `compile_commands.json` — is Windows-only, since it depends on a local Visual Studio installation.
  This split (cross-platform-buildable, Windows-only runtime behavior) should be reflected in how
  toolchain-discovery code is isolated/mocked for tests that run on non-Windows CI runners.

# Architecture & System Design

## Two Artifacts

As defined in project_spec.md, the system is two independent artifacts communicating through the
filesystem and a subprocess boundary — no persistent daemon or IPC channel:

1. **CLI binary** — parses a solution for a given configuration/platform and emits
   `compile_commands.json` plus persisted state.
2. **Neovim plugin** — orchestrates the binary, manages `clangd`'s lifecycle, and provides the
   interactive UI (solution discovery/picker, configuration/platform switching).

## CLI Binary — Internal Modules

Rough module breakdown implied by the parsing pipeline:

- **`sln` parsing** — reads a `.sln` file to enumerate member projects, and the
  configuration/platform pairs it declares. `.sln` is line-oriented text rather than XML, so it has
  its own small parser instead of sharing the `roxmltree` stack. Members are classified by project
  type GUID — `{8BC9CEB8-8B4A-11D0-8D11-00A0C91BC942}` is C++, and solution folders, C# projects,
  and `.vcxitems` are skipped as ordinary solution content rather than reported. Project locations
  are solution-relative and backslash-spelled; they are made absolute against the solution
  directory and normalized lexically, never with `fs::canonicalize`, which requires the file to
  exist and yields `\\?\` paths on Windows. `..` is resolved rather than refused, since a project
  living outside the solution folder is ordinary — escape validation belongs to import resolution
  below. A malformed entry is an error naming the line, never a panic. A duplicate project GUID is an
  error: rows in the section below are keyed by GUID, so a duplicate would leave one member with no
  mapping and see it silently skipped. See [ADR 04](adr/04-sln-project-enumeration.md).

  The same scan reads two `GlobalSection`s. `SolutionConfigurationPlatforms` gives the pairs the
  solution declares, taken from the key side of each row, deduplicated in declaration order.
  `ProjectConfigurationPlatforms` maps each of those onto a per-project pair, which legitimately
  differs — a solution-level `Debug|x64` can mean `Debug|Win32` for one member — so both are kept and
  MSBuild evaluation is handed the *project* pair. Row keys are `{GUID}.<pair>.<Property>`, taken
  apart by stripping a known property off the right rather than splitting on `.`, since configuration
  names may contain one. `Build.0`'s presence (not its value) means the project is built in that
  configuration; a project with a mapping but no `Build.0` is unchecked in Configuration Manager,
  while a project with no mapping takes no part in the configuration at all — generation distinguishes
  the two. Names are opaque strings, normalized no further than trimming, and looked up exactly, so a
  mismatched request fails as `InvalidConfiguration` listing the valid pairs. The format-version
  header is warned about rather than enforced, since `12.00` spans VS2013–VS2022; the diagnostic goes
  to the `Reporter` the parse is handed. See [ADR 05](adr/05-sln-configuration-mapping.md).
- **`diagnostics`** — the non-fatal counterpart to `error.rs`. Owns `Diagnostic` (severity, message,
  file, line), `Severity`, and `Reporter`, which is threaded through evaluation as `&mut` and carries
  two things at once: the accumulating warning list, and the `-v` progress channel as an
  `Option<Box<dyn Write>>` that is `None` when quiet. Warnings are accumulated rather than returned
  because `?` would discard them on exactly the failure path where they matter, and progress is
  push-down by nature since the point of `-v` is feedback while work is still in flight. The type
  lives here rather than in `cli.rs` so that evaluation modules do not have to reach into the CLI
  layer to report a finding. See [ADR 06](adr/06-diagnostic-collection.md).
- **`vcxproj`/MSBuild evaluation** — reads each project's `.vcxproj`, follows `<Import>`s of
  `.props`/`.targets` files (including property sheets), evaluates `Condition` attributes and
  macros (e.g. `$(Configuration)`, `$(Platform)`, `$(SolutionDir)`) for the selected
  configuration/platform. This is the most complex piece of the system — see Open Questions.

  The reading half is settled by MVP-07 and lives in `msbuild/project.rs`, which serves `.vcxproj`,
  `.props` and `.targets` alike — one format, one `<Project>` root, one function. It lowers a file
  into an owned `Vec<Element>` in **document order**, because order decides which assignment wins
  and any map keyed by property name destroys that answer invisibly; duplicates are kept for the
  same reason. Nothing is evaluated here — no macro expanded, no condition evaluated, no import
  followed — and every `Condition` is carried forward as a string, with a group's condition kept
  separate from its children's rather than combined, since synthesizing `(a) and (b)` would put an
  expression in the IR that appears in no file. One `Setting` type covers property assignments and
  item metadata, which are the same XML and get the same treatment downstream. Elements match on
  their local name, so the MSBuild namespace is optional; values are trimmed but conditions are
  not. `Target`, `UsingTask` and `ProjectExtensions` are skipped silently as content that cannot
  change how a file compiles, while anything else unmodelled — `<Choose>` above all — is recorded
  as `Element::Unsupported` *and* warned about, so a construct that could change the flags is never
  dropped in silence. Files that are well-formed XML but not projects fail as `project-schema`,
  distinct from a `project-parse` corruption because "this is not a C++ project" is a different
  thing to tell a user. One `Document` is parsed, lowered and dropped before the next import is
  read, per ADR 01's borrowing constraint. See [ADR 07](adr/07-vcxproj-reader-ir.md).

  Nesting depth is bounded **before** the text reaches `roxmltree`, by MVP-35. `Document::parse`
  recurses once per level and aborts the process rather than returning an error once the stack is
  gone, so there is no failure to catch after the fact and no parser option that helps —
  `nodes_limit` counts nodes and cannot tell 5,000 nested elements from an ordinary solution with
  5,000 source files. A byte scan counts element nesting, skipping comments, CDATA, processing
  instructions and declarations whole, and refuses anything past `MAX_XML_DEPTH = 32` as
  `Error::RecursionLimit`. Real files reach four levels, so the headroom is eightfold and no more:
  `imports.rs` may be 64 frames deep when a file is parsed. The scan tracks quoting to find where a
  start tag ends — a literal `>` is legal in an attribute value where a `<` is not — because
  failing to see a `/>` leaks a level per source file and would refuse real projects before hostile
  ones. It is a guard rather than a validator: malformed markup is stepped over and still reported
  by the parser as `project-parse`. See [ADR 16](adr/16-xml-nesting-depth-guard.md).

  Macro expansion is settled by MVP-08 and lives in `msbuild/eval.rs`. It is **eager**: a value is
  expanded at the point of *assignment*, against the property bag as it stood at that moment, so
  what is stored contains no `$(` at all. That is MSBuild's own rule, and it is load-bearing — it is
  why `$(PreprocessorDefinitions);_DEBUG` appends rather than regressing infinitely, why redefining
  a property never reaches back into values already built from it, and why a cyclic property cannot
  be constructed at all. Expansion is therefore a single non-recursive pass, and the depth limit a
  lazy design would need is replaced by a 1 MiB cap on the expanded value, which is what actually
  bounds the one runaway case eagerness leaves open (`<A>$(A)$(A)</A>` doubles per assignment). The
  cap reuses `Error::RecursionLimit` rather than adding a kind to the plugin contract. Property
  names are matched case-insensitively, as MSBuild matches them; a property assigned an empty value
  is *defined*, distinct from one never assigned, because `Condition="'$(X)' != ''"` is how projects
  test for absence and warning about it would train users to ignore warnings. Constructs outside the
  supported subset — property functions, registry reads, nested references, an unterminated `$(` —
  are copied through verbatim and recorded rather than emptied, so a bogus entry in the database
  names its own cause; `%(Metadata)` and `@(Item)` are left untouched for MVP-12. Expansion returns
  what it could not resolve rather than warning about it, since it knows neither the file nor the
  line and MVP-13 owns the wording. See [ADR 08](adr/08-eager-macro-expansion.md).

  `Condition` evaluation is settled by MVP-09 and sits beside expansion in `msbuild/eval.rs`: a
  tokenizer and recursive-descent evaluator over quoted and unquoted operand comparison (`==`,
  `!=`), `And`/`Or`/`!`, parentheses, `Exists()`, and a lone operand as a boolean literal. Anything
  else in MSBuild's much larger grammar is out of scope. The shape of the design is that
  "unsupported" means two different things: an expression that cannot reduce to a boolean is a fatal
  `Error::UnsupportedConstruct`, because inventing one would either drop a `PropertyGroup` carrying
  every include path or apply flags the file never asked for; whereas an unsupported `$(...)` inside
  an operand is merely recorded, since expansion left it verbatim and there are still two strings to
  compare. Conditions are **parsed before any operand is expanded** — project files are untrusted
  input, and expanding first would let a property value containing `' Or '` rewrite the expression
  around it. `And`/`Or` deliberately do not short-circuit, so a construct on the dead side is still
  reported. Comparison is case-insensitive as MSBuild's is, an empty condition holds as an absent
  one does, `Exists()` resolves against the directory of the file the condition is written in, and
  nesting is capped at 32 to keep recursive descent off a stack-overflow path. MVP-10 inherits the
  fatal-error policy and owns whether to downgrade it for the Visual Studio installation's own
  `.targets`. See [ADR 09](adr/09-condition-evaluation-subset.md).

  Import resolution is settled by MVP-10 and lives in `msbuild/imports.rs`. Because an import's path
  is macro-expanded and its `Condition` gates whether the path is resolved at all, resolution cannot
  be a pre-pass over the graph — it **is** property evaluation, which makes this module MSBuild's
  first evaluation pass. It walks one file's elements in document order and splices each imported
  file in where its `<Import>` sits, returning the files entered, the property bag the whole graph
  ended with, and the `ItemGroup`/`ItemDefinitionGroup` elements flattened into one stream with
  their conditions **unevaluated** — MSBuild settles those in a later pass against the final
  property values, which is what makes MVP-12 pass 2/3. The stream is spliced rather than grouped
  by file because a `.vcxproj` writes its item definitions between `Microsoft.Cpp.props` and
  `Microsoft.Cpp.targets`, so grouping would order the root's ahead of the imported sheet's. A file
  already imported is skipped and warned about, which is MSBuild's own MSB4011 rule and covers
  diamonds and cycles with one mechanism; `..` is normalised lexically and followed rather than
  refused, since a shared sheet one directory up is how multi-project solutions are laid out, and
  there is consequently no root left to escape from. A condition is evaluated before its path is
  expanded, so a guarded optional import never reports a missing file the project allows for; one
  whose condition holds and whose file is absent is `Error::MissingImport`, naming the referrer.
  This is also where MVP-08's and MVP-09's recorded findings finally become `Reporter` warnings,
  this being the first code that holds both a finding and the file and line it came from. See
  [ADR 10](adr/10-import-resolution.md).

  The bag that pass starts from is seeded by MVP-11, in `msbuild/reserved.rs`. Project files
  reference `$(SolutionDir)`, `$(ProjectDir)`, `$(Configuration)` and `$(MSBuildThisFileDirectory)`
  without defining them, so against an empty bag every include path loses its prefix and no
  configuration-specific `PropertyGroup` applies at all — a database that generates cleanly and is
  wrong. Twenty-four properties are seeded in five families (`Solution*`, `Configuration`/`Platform`,
  `MSBuildProject*`, `MSBuildThisFile*`, `Project*`), all derived from three inputs: the `.sln` path,
  the `.vcxproj` path, and the selected **project** pair per ADR 05. Toolchain-derived properties
  (`$(VCTargetsPath)`, `$(VSInstallDir)`) are deliberately absent until MVP-15 can supply them from a
  real installation. Directory-valued properties carry MSBuild's trailing separator — spelled with
  `MAIN_SEPARATOR` so a Linux CI run never mixes separators inside one path — except
  `$(MSBuildProjectDirectory)`, which MSBuild deliberately leaves without one; both conventions are
  load-bearing, since files concatenate these values directly. This module owns the *policy* of which
  names exist; `eval.rs` owns the *mechanism* that protects them, as a locked-key set where
  `set_reserved` writes and `assign` refuses, setting `Expansion::refused` for `imports.rs` to warn
  from. That enforces the trust boundary MVP-08 had already drawn between `set` (seeding) and
  `assign` (read out of a project file), rather than leaving it to a check each caller must remember.
  The lock covers what MSBuild reserves plus what a solution build passes as global, while the
  `Project*` family is seeded **overridable**, because MSBuild's own `.props` define it conditionally
  and a project renaming itself with `<ProjectName>` is ordinary. `MSBuildThisFile*` is a property of
  the file being read rather than of the project, so it follows the import walk: `Walk::enter`
  brackets each file with `reserved::enter_file`/`restore` beside its existing depth bookkeeping,
  which is what lets a shared property sheet locate its own siblings. See
  [ADR 11](adr/11-reserved-property-seeding.md).

  The remaining two passes are settled by MVP-12 and split three ways, because they change for
  unrelated reasons. `msbuild/eval.rs` gains the `%()` sigil beside the `$()` one — a `Metadata` bag
  that is the item-scoped counterpart to `Properties`, reusing the same tokenizing helpers and the
  same 1 MiB cap. `msbuild/items.rs` is **pass 2**: it walks MVP-10's deferred stream in order,
  evaluating each condition against the property values the graph *ended* with, and produces one
  `SourceItem` per `ClCompile` source with the metadata that applies to it. `msbuild/flags.rs` is
  **pass 3**: settled metadata translated into `cl` switches. `msbuild/paths.rs` holds the lexical
  helpers both need, including `normalize` moved out of `imports.rs` and a converter for
  MSBuild-spelled paths, since `PathBuf::from(r"inc\sub")` is a single component on Linux and CI would
  otherwise compare a path Windows never produces. `msbuild::evaluate` becomes the orchestrator and
  takes a `ProjectScope`, which is the signature ADR 11 said it needed: seeding requires the solution
  path, and its `config` is the **project** pair per ADR 05.
  Four rules carry the design. An unset `%(Name)` expands to nothing and is **not** recorded, because
  `WIN32;%(PreprocessorDefinitions)` against an empty bag is what every real project file writes and
  warning on it would devalue the whole channel — ADR 08's `Some("")`-versus-`None` argument applied to
  the other sigil; genuinely unsupported forms (`%(ClCompile.Identity)`, `@(...)`, an unterminated
  `%(`) are still recorded and copied through verbatim. `$` expands before `%`, since a property's
  value cannot contain `%(` but a property reference can expand into one. An `ItemDefinitionGroup`
  applies only to items declared **after** it, so defaults are snapshotted into each item as it is
  declared rather than gathered up front — which is what ADR 10's ordered splice exists to make
  possible, and per-file metadata assigned on top is what lets an override *extend* a project default
  rather than only replace it. And relative item paths resolve against the **`.vcxproj`'s** directory
  rather than the declaring file's, which is why a shared sheet meaning its own directory has to spell
  `$(MSBuildThisFileDirectory)`; because pass 1 restores that family on the way out of each import,
  pass 2 re-enters each deferred element's file against a clone of the bag, or a sheet's own include
  directory silently becomes one beside the project. Wildcard `Include` specs warn and contribute
  nothing, with expansion deferred to MVP-37 — chosen over refusing a legal construct and over
  building a globber, and it replaces the previous behaviour of producing no source files in silence.
  Only settings that change how a file *parses* are translated, so the include directories, defines,
  language standard and the switches that move a predefined macro are all mapped, while codegen and
  link settings are skipped along with `/WX` (clang's warning set is not MSVC's, so it would turn
  diagnostics the build never emits into editor errors), `/Yu` (injects nothing, and points clangd at
  a `.pch` that does not exist), `/Za` (clang-cl rejects it) and `CharacterSet` (arrives through
  Microsoft's own `.props` once MVP-15 lands). An unrecognized *value* for a recognized setting warns
  and emits nothing rather than guessing; an unrecognized setting *name* is silent, since a real
  `ClCompile` carries dozens. Asking for a configuration/platform pair the project never declares
  warns, because it evaluates cleanly, matches no gated group, and yields source files with no flags.
  See [ADR 12](adr/12-clcompile-flag-extraction.md).
- **Toolchain discovery** — locates the MSVC toolset and Windows SDK include paths/versions for the
  resolved project, since these aren't fully specified inside the project files. Split across two
  issues along the boundary `vswhere` itself draws: MVP-14 finds *which* installation to use, MVP-15
  reads *what's in it*.

  Instance discovery is settled by MVP-14 and lives in `toolchain/vswhere.rs`. `vswhere` is shelled
  out to as an argument vector, never a shell string, from its well-known
  `%ProgramFiles(x86)%\Microsoft Visual Studio\Installer\vswhere.exe` location, requesting
  `-requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -format json` — the core MSVC compiler
  *component* rather than the broader `NativeDesktop` *workload*, so a standalone Build Tools install
  is found too, not just the full IDE. Prerelease instances are excluded, matching `vswhere`'s own
  default. The shell-out (`run_vswhere`) is kept separate from parsing (`parse_instances`) and
  selection (`select_instance`/`newest`), which is what makes "no `vswhere.exe`", "no matching
  instance", and "malformed output" unit-testable against recorded strings and a nonexistent path,
  without a filesystem or a real installation. The newest instance wins ties, compared by parsing
  `installationVersion` into per-component integers rather than as a string, since `"9.0" >
  "17.0"` lexicographically. All three failure shapes raise the existing `Error::ToolchainNotFound`
  rather than a new kind — its remediation ("something about your Visual Studio install needs
  fixing") already covers all of them. `ToolchainProvider::discover()` is not reachable from any
  subcommand yet, so it calls the new `locate()` and leaves resolving the toolset/SDK layout as
  `todo!()` for MVP-15, the same position `reserved::seed()` was in until MVP-12 wired it up. See
  [ADR 14](adr/14-vswhere-discovery.md).
- **Compilation database generation** — combines evaluated per-file compiler flags, include paths,
  and defines into `compile_commands.json` entries. Flags should be emitted in a form `clangd`
  understands directly (e.g. via `--driver-mode=cl` for MSVC-style flags) rather than translated to
  native clang flags, to minimize translation-fidelity risk.
- **Persistence (JSON state file)** — stores, per discovered solution: known configuration/platform
  values, the last-active configuration/platform, and file modification timestamps used to detect
  when a reparse is needed. Written atomically (temp file + rename) so a crash or a concurrent run
  cannot leave a torn file. The on-disk shape is not yet finalized (see Open Questions).
- **CLI/subcommand layer** — the contract the Neovim plugin drives, defined in full under
  "CLI Surface" below.

## CLI Surface

Settled by MVP-03; see [ADR 03](adr/03-cli-surface-and-json-contract.md) for the reasoning.

```
msvclsp [--json] [-v] <COMMAND>

Commands:
  list-configs  List the configuration/platform pairs a solution declares
  generate      Generate compile_commands.json for one configuration/platform

msvclsp list-configs <SOLUTION> [--root <DIR>]
msvclsp generate <SOLUTION> --configuration <NAME> --platform <NAME> [--root <DIR>] [--force]
```

The plugin always spells flags in full; `-c` and `-p` exist for invoking the tool by hand.

| Argument | Applies to | Meaning |
| -------- | ---------- | ------- |
| `<SOLUTION>` | both | Required positional; the `.sln` to read. |
| `--root <DIR>` | both | Project root to create `.msvclsp/` under. Defaults to the solution's own directory. The plugin passes the root it detected from Neovim's LSP root markers, which often is not where the solution lives. |
| `--configuration <NAME>`, `-c` | `generate` | Required. Separate from `--platform` so no shell has to quote a pipe character. |
| `--platform <NAME>`, `-p` | `generate` | Required. |
| `--force` | `generate` | Regenerate even when the staleness cache is current. Inert until MVP-22. |
| `--json` | global | Emit the envelope below on stdout instead of human-readable text. |
| `-v`, `--verbose` | global | Progress on stderr as work proceeds, and absolute rather than root-relative paths in warnings. Never affects stdout. |

`discover` is deliberately absent — it is V1-01, and the MVP plugin scans in Lua (MVP-27). Adding a
subcommand later is additive.

### Stream discipline

Settled alongside diagnostic collection by MVP-04; see [ADR 06](adr/06-diagnostic-collection.md).

- **stdout** is human-readable text by default, and exactly one JSON object under `--json`. Never a
  mix, and never partially written. Nothing in the library writes to stdout — `main.rs` is the only
  place that may, which is asserted by an integration test rather than left as a convention.
- **stderr** carries the human-readable message, warnings, and any progress output, in *both* modes.
  MVP-28 folds its contents into the plugin's own structured error whenever the exit code is
  non-zero.
- **Warnings reach stderr in both modes**, and additionally ride in the envelope's `warnings` array
  under `--json`. The duplication is deliberate: stderr is never parsed, and a captured `--json` run
  should still be legible without decoding it by hand. They are written before the result or the
  error message, so an error stays the last thing on stderr.
- **Progress is opt-in via `-v`** and goes only to stderr. A `Reporter` constructed without it has no
  sink at all, so suppression is structural rather than a check each call site has to remember.
- **Absolute paths are kept out of stderr at default verbosity.** A diagnostic's `file` is absolute in
  the envelope, because that is the plugin's contract, but on stderr it is shown relative to the
  project root when it lives underneath it. `-v` opts into the full path. Per CLAUDE.md §5, stderr
  text is what ends up pasted into a bug report.
- **`clap` usage errors are a carve-out.** `clap` writes its own text to stderr and exits `2`;
  stdout stays empty and no envelope is produced, regardless of `--json`. This is why no
  `ErrorKind` may return `2`.

### Diagnostics

Settled by MVP-13; see [ADR 13](adr/13-diagnostic-severity-and-wording.md) for the reasoning. ADR 06
above owns the *channel*; this owns what travels on it.

A `Diagnostic` carries a severity, a message, and the file and line it is attributable to. There are
two severities, and which one a finding gets is decided by a single question:

> **Could msvclsp's failure to model this have changed the compiler flags?**

| Severity | Wire string | Meaning |
| -------- | ----------- | ------- |
| `Warning` | `"warning"` | Yes. Something was skipped, dropped, or left unresolved, and clangd may now be told something the build never says. |
| `Note` | `"note"` | No. The construct is unusual, or msvclsp did exactly what MSBuild would have. Worth mentioning only because silence is how a real problem gets missed. |

Anything *fatal* is an [`Error`](#error-handling) rather than a third level. Both severities reach
stderr and both ride in the envelope: notes are not suppressed by default and not held behind `-v`,
because filtering is the consumer's decision and the CLI has no UI to make it with. `Reporter::warn_at`
and `Reporter::note_at` are the two producers; a diagnostic identical in severity, message, file
*and* line to one already recorded is dropped, keeping the first.

The catalogue below is the whole of it. A new call site is expected to add a row.

| Construct | Severity | Emitted by |
| --------- | -------- | ---------- |
| No `Format Version` header — this may not be a solution file | Warning | `sln.rs` |
| Solution format version is not the supported one; read anyway | Note | `sln.rs` |
| An element the reader models nothing for (`<Choose>` above all) | Warning | `msbuild/project.rs` |
| A non-`Import` child of an `ImportGroup`, skipped | Warning | `msbuild/project.rs` |
| A property value with element children; only its text was read | Warning | `msbuild/project.rs` |
| An item declaring no `Include`, `Remove` or `Update` | Note | `msbuild/project.rs` |
| A file already imported, not imported again (MSB4011) | Note | `msbuild/imports.rs` |
| An assignment to a reserved property, ignored | Warning | `msbuild/imports.rs` |
| An `<Import>` whose path expanded to nothing, skipped | Warning | `msbuild/imports.rs` |
| `$(X)` is not defined and expanded to nothing | Warning | `msbuild/eval.rs` |
| A construct outside the supported subset, left as written | Warning | `msbuild/eval.rs` |
| A source file declared twice; the second is dropped | Warning | `msbuild/items.rs` |
| An `Update` naming a file the project does not compile | Warning | `msbuild/items.rs` |
| A wildcard in an `Include` or `Update`, not expanded — what it names is missing | Warning | `msbuild/items.rs` |
| A wildcard in an `Exclude` with a literal `Include`, not expanded — what it names was not removed | Warning | `msbuild/items.rs` |
| A wildcard in an `Exclude`/`Remove` with nothing in reach, not expanded | Note | `msbuild/items.rs` |
| A settled value still holding `$(` or `%(` | Warning | `msbuild/flags.rs` |
| An enumerated setting whose value is not recognized | Warning | `msbuild/flags.rs` |
| `AdditionalOptions` holding a quoted argument (see MVP-38) | Warning | `msbuild/flags.rs` |
| A configuration/platform pair the project never declares | Warning | `msbuild.rs` |
| A project with no `ClCompile` sources at all | Warning | `msbuild.rs` |

Two things are deliberately **not** reported. An unset `%(Name)` expands to nothing in silence, per
[ADR 12](adr/12-clcompile-flag-extraction.md) — `WIN32;%(PreprocessorDefinitions)` is what every real
project writes. And `%XX` character escapes are neither decoded nor warned about: a stray `%3B`
reaching a flag is self-explaining in the generated database, where a `%`-scanning check would fire
on ordinary paths.

The two `msbuild/eval.rs` wordings have one definition, `eval::report`, because they are produced
mechanically at three unrelated call sites. Every other message lives at its own site, beside the
comment explaining the construct.

### JSON envelope

One shape for every response, with all five keys always present. No field is conditionally
omitted, so the plugin decodes one documented structure rather than branching on which keys exist.

```jsonc
// success
{ "schema_version": 1, "has_error": false, "result": { /* subcommand result */ },
  "error": null, "warnings": [] }

// failure
{ "schema_version": 1, "has_error": true, "result": null,
  "error": { "kind": "solution-not-found",
             "message": "solution not found: C:\\proj\\app.sln",
             "causes": [] },
  "warnings": [] }
```

| Key | Meaning |
| --- | ------- |
| `schema_version` | Bumped only when a change would break a plugin that decoded the previous shape. Adding a field does not require a bump. |
| `has_error` | The discriminator. **Not** redundant with `error == null`: `vim.json.decode` maps JSON `null` to `vim.NIL`, which is truthy in Lua, so `if result.error then` would take the error branch on every success. |
| `result` | The subcommand's own result object, serialized bare. `null` on failure. |
| `error` | `kind` is [`ErrorKind::as_str()`](#error-handling); `message` is the outermost `Display`; `causes` is the `source()` chain, outermost-inward. `null` on success. |
| `warnings` | Non-fatal diagnostics (`severity`, `message`, `file`, `line`). Always present, empty when there are none. `severity` is `"warning"` or `"note"` — see [Diagnostics](#diagnostics) — and a decoder must treat an unrecognized value as a warning rather than branching exhaustively, since a new level is additive and does not bump `schema_version`. Accumulated by a `Reporter` and attached to the envelope by `main.rs` — including on failure, so a run that warned and *then* failed still reports both. `file` is absolute. |

`result` is `SolutionConfigs` (`solution`, `available`, `active`) for `list-configs` and
`GenerateResult` (`database`, `regenerated`) for `generate`.

## Error Handling

`error.rs` defines a flat crate-level `Error` enum and `pub type Result<T> = Result<T, Error>`. Each
variant carries the offending path, line, or construct so the message names what actually failed;
`Error::io(path, source)` exists because `std::io::Error` omits the path, and there is deliberately
no `From<std::io::Error>` impl that would let a bare `?` discard it. `Error::context` and
`ResultExt::with_context` wrap an error with a description of what was being attempted — the wrapper
**delegates its kind to what it wraps**, so adding context can never reclassify a failure. See
[ADR 02](adr/02-structured-error-type.md) for why this shape and not per-module error types.

A separate `ErrorKind` carries the classification the plugin dispatches on. Its `as_str()` values
are a **contract with the plugin** and are written out explicitly rather than derived from Rust
variant names, so a refactor cannot silently change them:

| Kind | Raised when |
| ---- | ----------- |
| `io` | A filesystem operation failed. |
| `solution-not-found` | The requested `.sln` does not exist. |
| `solution-parse` | A `.sln` is malformed, naming the line. |
| `project-parse` | A `.vcxproj`/`.props` is not well-formed XML. |
| `unsupported-construct` | An MSBuild construct outside the supported subset. |
| `missing-import` | A required `<Import>` target does not exist. |
| `recursion-limit` | A macro-expansion, condition-nesting, XML-nesting or import depth cap was hit, or an expanded property value grew past its size cap. |
| `toolchain-not-found` | No `vswhere`, or no VS instance with the C++ workload. |
| `toolchain-incomplete` | VS is present but the toolset/Windows SDK layout is not. |
| `invalid-configuration` | The requested configuration/platform isn't one the solution declares. |
| `state-parse` | The state file is malformed or truncated. |
| `state-version` | The state file's schema version is unsupported. |
| `unimplemented` | A subcommand's argument surface exists but its pipeline does not. **Temporary** — removed by MVP-22. |

`ErrorKind::exit_code()` maps kinds onto coarse classes rather than one code per kind — the plugin
reads the exact kind from JSON, so exit codes only need to be useful from a shell. `2` is never
returned, because that is `clap`'s usage-error code and overloading it would make a mistyped command
indistinguishable from a real failure.

| Code | Class | Kinds |
| ---- | ----- | ----- |
| 1 | generic failure | `io` |
| 3 | input not found | `solution-not-found` |
| 4 | malformed input | `solution-parse`, `project-parse`, `project-schema`, `state-parse`, `state-version` |
| 5 | unsupported construct | `unsupported-construct`, `missing-import`, `recursion-limit` |
| 6 | toolchain unavailable | `toolchain-not-found`, `toolchain-incomplete` |
| 7 | bad selection | `invalid-configuration` |
| 9 | not implemented | `unimplemented` (temporary) |

`8` is absent rather than reassigned. It belonged to `path-escape`, which ADR 10 removed, and
reusing it would silently change what a shell script's `exit 8` means for failures that did not
change. It stays reserved.

`ErrorKind` intentionally derives no `serde` traits. The JSON error payload's shape belongs to the
subcommand output types in `cli.rs` — see the envelope's `error` object under "CLI Surface" above,
whose `kind` is `as_str()`. Rendering the `source()` chain as a human-readable message on stderr
belongs to `main.rs`.

## Neovim Plugin — Responsibilities

- Solution discovery: invoke the CLI to find `.sln` files under the project root, cache results,
  expose a manual rescan command.
- Active solution/config selection: interactive picker, persisted per workspace (project root,
  detected via standard LSP root markers like `.git`), with an explicit switch command.
- Compilation database generation: invoke the CLI to (re)generate `compile_commands.json` into
  `.msvclsp/<solution-name>/`.
- `clangd` lifecycle: start/restart `clangd` pointed explicitly at
  `.msvclsp/<solution-name>/compile_commands.json` (not relying on `clangd`'s own directory-walk
  discovery), re-launching when the database is regenerated or the active solution/config changes.

## Storage Layout

- `.msvclsp/` at the project root (project root = same root markers Neovim's LSP clients use).
- Namespaced by solution: `.msvclsp/<solution-name>/compile_commands.json`.
- A JSON state file (location within `.msvclsp/` TBD — likely a single file at the `.msvclsp/` root
  rather than one per solution, since it needs to track which solution is currently active) holds
  discovery cache, per-solution configuration/platform state, and file modification timestamps.

## VS/Project Schema Support

Initial target is **Visual Studio 2019 and 2022** project/solution schema conventions. Older
toolsets (VS2015/2017) and their schema quirks are explicitly out of scope for v1 to keep the
MSBuild-evaluation surface area bounded.

# Engineering Requirements

- **Correctness over completeness for v1 MSBuild evaluation** — it's acceptable to not support
  every possible MSBuild construct, but unsupported constructs encountered in a real solution
  should be surfaced (warning/log), not silently produce incorrect flags.
- **Reparse avoidance** — the tool must detect when a solution/its files haven't changed (via
  stored modification timestamps) and skip reparsing, per brainstorm.md.
- **Structured, parseable CLI output** — any CLI output consumed by the Neovim plugin must be
  structured (JSON) so the plugin doesn't need to scrape human-readable text.
- **Testability of toolchain discovery** — toolchain/SDK discovery logic should be isolated behind
  an interface that can be faked/mocked in tests that run on non-Windows CI runners.
- **All functionality unit tested**, per brainstorm.md.

# Testing Strategy

- **Unit tests** for each module (XML/MSBuild parsing, macro/condition evaluation, compile database
  generation, state-file persistence layer), run as standard `cargo test`, on any OS — toolchain
  discovery mocked out where needed.
- **Fixture-based integration tests**: a corpus of sample `.sln`/`.vcxproj`/`.props` files of
  varying complexity (simple single-project solution, multi-project solution, property-sheet
  inheritance, multiple configurations/platforms) checked into the repo, with expected
  `compile_commands.json` output asserted against as golden files. This is the primary correctness
  signal for the parsing pipeline, per brainstorm.md's call for "sample MSVC files of varying
  complexity."
- **Manual end-to-end verification against real `clangd`** for now: actually launching `clangd`
  against generated output and confirming completion/diagnostics behave correctly is a manual smoke
  test rather than an automated one, given the pivot away from an LSP-proxy architecture reduces the
  need for the plugin/binary itself to reason about LSP protocol correctness. Automating this
  (spinning up real `clangd` and asserting on LSP responses) is left as a future enhancement, not a
  v1 requirement.
- **Neovim plugin tests**: `mini.test`-based headless tests covering solution discovery/caching
  logic, picker/persistence behavior, and `clangd` lifecycle management — with the CLI binary
  invocation faked/stubbed rather than requiring a real Windows toolchain in plugin tests.

# CI Expectations

- **Rust:** `cargo build`, `cargo fmt --check`, `cargo clippy`, and `cargo test` (unit + fixture-based
  integration tests) on every push/PR. Since the codebase is cross-platform-buildable, this can run
  on a standard Linux CI runner; toolchain-discovery code stays mocked in this environment.
  A Windows runner job should additionally verify at least that the binary builds and runs on
  Windows, since that's the only real target runtime.
- **MSRV:** a job pinned to the declared `rust-version` running `cargo check --all-targets
  --locked`. Every other job uses `stable`, so without this the declared floor would drift upward
  the first time someone used a newer language feature. `--locked` also catches a stale lockfile.
- **Neovim plugin:** headless `mini.test` test run on every push/PR.
- Provider (e.g. GitHub Actions) and exact workflow files are not yet written — this section
  documents intent/expectations, not a finished pipeline.

# Open Questions / To Be Refined

- Exact scope of MSBuild evaluation needed (property sheet inheritance, conditions, macros) to
  produce accurate flags for VS2019/2022 solutions — carried over from project_spec.md.
- State file shape (keys for solutions, configurations/platforms, file timestamps) and how solution
  identity is keyed so it survives the working directory changing.
- Whether last-writer-wins state is acceptable long term, or whether two Neovim instances sharing a
  project root need real locking. Accepted for the MVP as a consequence of dropping SQLite.
- Whether the plugin should verify the envelope's `schema_version`, or whether the field exists only
  for a human reading a captured payload. Nothing enforces it today.
- Which channel warnings should actually reach the user through in Neovim: rendered from the
  envelope's `warnings` array, or folded in with stderr the way MVP-28 handles errors. V2-04 owns
  error surfacing; nothing yet says who owns warnings.
- Distribution's effect on MSRV. If the CLI ships as prebuilt release binaries, MSRV is a
  contributor-only concern; if the plugin builds from source or users `cargo install`, it gates
  users directly. Worth settling alongside the release/distribution question below.
- Whether wildcard `Include` attributes are ever *expanded*. MVP-12 settled that they are neither
  refused nor silently dropped: a wildcard warns naming the spec it skipped and contributes nothing,
  which closes the silent-no-sources hole without taking on MSBuild's globbing semantics. What is
  still open is whether the expansion itself is worth building — MVP-37 holds the work, and a project
  written this way gets an incomplete database until it lands. See
  [ADR 12](adr/12-clcompile-flag-extraction.md).
- Whether `AdditionalOptions` needs real shell-style argument splitting. MVP-12 splits on whitespace,
  which is correct for every switch a project is likely to carry there and wrong for a quoted path
  containing a space. MVP-13 declined to fix it here and made it *visible* instead — a value holding
  a `"` now warns that it will reach clangd as separate flags — because getting it right means
  choosing between MSBuild's quoting rules, `cl`'s response-file rules and the Windows command-line
  parser's, which disagree about backslashes before quotes. **MVP-38 holds the work.** See
  [ADR 13](adr/13-diagnostic-severity-and-wording.md).
- Whether `$(SolutionDir)` and the rest of the `Solution*` family should be seeded at all when a
  project is evaluated outside a solution. Not reachable today, since the CLI always takes a `.sln`,
  but standalone-project support and the multi-solution work both arrive at it. The honest answer may
  be that those properties should be *absent* — and so warn as undefined — rather than pointed at a
  directory no solution occupies. See [ADR 11](adr/11-reserved-property-seeding.md).
- Source and scope of the fixture corpus for integration tests — hand-authored minimal solutions,
  sanitized real-world examples, or both.
- Release/distribution strategy: how the CLI binary is distributed to the Neovim plugin (built from
  source by the plugin, prebuilt GitHub release binaries, `cargo install`) and how the plugin itself
  is distributed/versioned for plugin managers (e.g. `lazy.nvim` spec).
- CI provider and concrete workflow definitions.
