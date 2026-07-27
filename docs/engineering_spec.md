# Engineering Spec

This document defines the technical requirements for *how* MSVC LSP is built: tech stack,
architecture and system design, engineering requirements, and testing strategy. It implements the
product intent described in [project_spec.md](project_spec.md) and [brainstorm.md](brainstorm.md).
Several details remain open and are tracked at the end of this document rather than guessed at.

# Tech Stack

## CLI Binary

- **Language:** Rust.
- **Candidate crates** (per brainstorm.md, library selection is still to be investigated/validated,
  not finalized):
  - XML parsing (`.sln`/`.vcxproj`/`.props`/`.targets`) — e.g. `quick-xml` or `roxmltree`.
  - SQLite integration — e.g. `rusqlite`.
  - CLI argument/subcommand parsing — e.g. `clap`.
  - Serialization (JSON output for the plugin contract, `compile_commands.json` itself) — `serde` /
    `serde_json`.
  - Error handling — `anyhow` and/or `thiserror`.
  - Logging/diagnostics — `tracing` or `log` + a subscriber.
- **Toolchain/SDK discovery** will need to shell out to or otherwise query `vswhere` (or equivalent)
  to locate installed Visual Studio instances, MSVC toolset versions, and Windows SDK versions —
  this is not derivable from the `.vcxproj` files themselves.

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

- **`sln` parsing** — reads a `.sln` file to enumerate member projects.
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
- **Persistence (SQLite)** — stores, per discovered solution: known configuration/platform values,
  the last-active configuration/platform, and file modification timestamps used to detect when a
  reparse is needed. Schema design is not yet finalized (see Open Questions).
- **CLI/subcommand layer** — the contract the Neovim plugin drives. At minimum needs subcommands to:
  discover solutions under a root, list a solution's configuration/platform options, and generate/
  regenerate `compile_commands.json` for a given solution + configuration/platform. Exact
  subcommand names and output shape are not yet finalized (see Open Questions), but output intended
  for the plugin to consume programmatically should be structured (JSON) rather than
  human-formatted text.

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
- A SQLite database (location within `.msvclsp/` TBD — likely a single db at the `.msvclsp/` root
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
  generation, SQLite persistence layer), run as standard `cargo test`, on any OS — toolchain
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
- **Neovim plugin:** headless `plenary.nvim` test run on every push/PR.
- Provider (e.g. GitHub Actions) and exact workflow files are not yet written — this section
  documents intent/expectations, not a finished pipeline.

# Open Questions / To Be Refined

- Exact scope of MSBuild evaluation needed (property sheet inheritance, conditions, macros) to
  produce accurate flags for VS2019/2022 solutions — carried over from project_spec.md.
- Final crate choices (XML parser, SQLite crate, CLI parser, logging) — brainstorm.md flags these as
  needing investigation rather than being decided.
- SQLite schema design (tables/keys for solutions, configurations/platforms, file timestamps).
- CLI subcommand surface and JSON output contract with the Neovim plugin — carried over from
  project_spec.md.
- Minimum Supported Rust Version (MSRV) and Rust edition.
- Source and scope of the fixture corpus for integration tests — hand-authored minimal solutions,
  sanitized real-world examples, or both.
- Release/distribution strategy: how the CLI binary is distributed to the Neovim plugin (built from
  source by the plugin, prebuilt GitHub release binaries, `cargo install`) and how the plugin itself
  is distributed/versioned for plugin managers (e.g. `lazy.nvim` spec).
- CI provider and concrete workflow definitions.
