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
- Testing via `plenary.nvim`'s test harness (busted-style), run headless.

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
  below. A malformed entry is an error naming the line, never a panic. See
  [ADR 04](adr/04-sln-project-enumeration.md).
- **`vcxproj`/MSBuild evaluation** — reads each project's `.vcxproj`, follows `<Import>`s of
  `.props`/`.targets` files (including property sheets), evaluates `Condition` attributes and
  macros (e.g. `$(Configuration)`, `$(Platform)`, `$(SolutionDir)`) for the selected
  configuration/platform. This is the most complex piece of the system — see Open Questions.
- **Toolchain discovery** — locates the MSVC toolset and Windows SDK include paths/versions for the
  resolved project (via `vswhere` or equivalent), since these aren't fully specified inside the
  project files.
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
| `-v`, `--verbose` | global | Progress on stderr. Inert until MVP-04. |

`discover` is deliberately absent — it is V1-01, and the MVP plugin scans in Lua (MVP-27). Adding a
subcommand later is additive.

### Stream discipline

- **stdout** is human-readable text by default, and exactly one JSON object under `--json`. Never a
  mix, and never partially written.
- **stderr** carries the human-readable message and any progress output, in *both* modes. MVP-28
  folds its contents into the plugin's own structured error whenever the exit code is non-zero.
- **`clap` usage errors are a carve-out.** `clap` writes its own text to stderr and exits `2`;
  stdout stays empty and no envelope is produced, regardless of `--json`. This is why no
  `ErrorKind` may return `2`.

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
| `warnings` | Non-fatal diagnostics (`severity`, `message`, `file`, `line`). Always present, empty when there are none. MVP-04 owns accumulating into it. |

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
| `import-cycle` | Imports form a cycle. |
| `recursion-limit` | A macro-expansion or import depth cap was hit. |
| `toolchain-not-found` | No `vswhere`, or no VS instance with the C++ workload. |
| `toolchain-incomplete` | VS is present but the toolset/Windows SDK layout is not. |
| `invalid-configuration` | The requested configuration/platform isn't one the solution declares. |
| `state-parse` | The state file is malformed or truncated. |
| `state-version` | The state file's schema version is unsupported. |
| `path-escape` | A path from an untrusted project file resolved outside its permitted root. |
| `unimplemented` | A subcommand's argument surface exists but its pipeline does not. **Temporary** — removed by MVP-22. |

`ErrorKind::exit_code()` maps kinds onto coarse classes rather than one code per kind — the plugin
reads the exact kind from JSON, so exit codes only need to be useful from a shell. `2` is never
returned, because that is `clap`'s usage-error code and overloading it would make a mistyped command
indistinguishable from a real failure.

| Code | Class | Kinds |
| ---- | ----- | ----- |
| 1 | generic failure | `io` |
| 3 | input not found | `solution-not-found` |
| 4 | malformed input | `solution-parse`, `project-parse`, `state-parse`, `state-version` |
| 5 | unsupported construct | `unsupported-construct`, `missing-import`, `import-cycle`, `recursion-limit` |
| 6 | toolchain unavailable | `toolchain-not-found`, `toolchain-incomplete` |
| 7 | bad selection | `invalid-configuration` |
| 8 | rejected path | `path-escape` |
| 9 | not implemented | `unimplemented` (temporary) |

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
- **Neovim plugin tests**: `plenary.nvim`-based headless tests covering solution discovery/caching
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
- **Neovim plugin:** headless `plenary.nvim` test run on every push/PR.
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
- Distribution's effect on MSRV. If the CLI ships as prebuilt release binaries, MSRV is a
  contributor-only concern; if the plugin builds from source or users `cargo install`, it gates
  users directly. Worth settling alongside the release/distribution question below.
- Source and scope of the fixture corpus for integration tests — hand-authored minimal solutions,
  sanitized real-world examples, or both.
- Release/distribution strategy: how the CLI binary is distributed to the Neovim plugin (built from
  source by the plugin, prebuilt GitHub release binaries, `cargo install`) and how the plugin itself
  is distributed/versioned for plugin managers (e.g. `lazy.nvim` spec).
- CI provider and concrete workflow definitions.
