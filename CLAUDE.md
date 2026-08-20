# CLAUDE.md

Guidance for working in this repository.

## 1. Project Goals

`clangd` needs a `compile_commands.json` compilation database to know how each file is
compiled. CMake generates one; Visual Studio solutions don't — so C/C++ codebases built with
MSBuild can't get real language support outside Visual Studio without hand-converting the
project to CMake.

sln2cc closes that gap with two artifacts: a **Rust CLI** that parses a `.sln` and its
`.vcxproj`/`.props` files for a chosen configuration/platform and emits a
`compile_commands.json`, and a **Neovim plugin** that drives the CLI and points `clangd` at
the result. Generated artifacts live in `.sln2cc/` at the project root.

Secondary (but real) goals: learn Rust on a non-trivial project, and learn how LSP tooling
fits together.

**Current milestone:** MVP — a single solution, manually selected configuration/platform,
generating a `compile_commands.json` that clangd can actually use.

## 2. Architecture Overview

Two independent artifacts communicating through the filesystem and a subprocess boundary —
no daemon, no persistent IPC, no LSP proxy. The CLI writes a compilation database; the
plugin invokes the CLI and manages clangd's lifecycle. clangd itself does all real language
work.

Key decisions worth internalizing before changing anything:

- **The CLI is not an LSP server.** An earlier design proxied LSP traffic to a child clangd;
  that was dropped. Don't reintroduce protocol handling.
- **Toolchain discovery is a trait, not free functions.** Real VS/SDK paths only exist on
  Windows and differ per machine. `ToolchainProvider` lets tests inject `FakeToolchain`,
  which is what makes golden files reproducible.
- **State is scoped per-solution** even though only one solution is active at a time. This is
  deliberate headroom for multi-solution support later.
- **Flags stay in MSVC (`cl`) form** and are consumed by clangd in clang-cl mode, rather than
  being translated to native clang flags.

See [docs/engineering_spec.md](docs/engineering_spec.md) for the full design, and
[docs/project_spec.md](docs/project_spec.md) for product intent.

```
sln2cc/
├── CLAUDE.md                     This file.
├── README.md                     User-facing intro, build and test instructions.
├── LICENSE                       MIT. Mirrored into cli/LICENSE so `cargo package` includes it.
├── .gitignore                    Ignores cli/target/, generated .sln2cc/ dirs, and .plans/.
├── .github/workflows/ci.yml      CI: Rust lint+test (Linux), build (Windows), MSRV check, plugin tests.
├── .claude/skills/               Claude Code skills for this repo's own workflow.
│   ├── start-issue/              Branch, plan, and write tests+docs for a docs/todos.md issue.
│   └── review-issue/             Review the implementation, reconcile the docs, commit.
│
├── cli/                          Rust CLI binary. All cargo commands run from here.
│   ├── Cargo.toml                Four deps (see section 5), MSRV 1.85. Cargo.lock is committed.
│   ├── README.md                 crates.io-facing readme (`readme` must stay inside the package root).
│   ├── LICENSE                   Mirror of the root LICENSE; `cargo package` needs it in-tree.
│   ├── src/
│   │   ├── main.rs               Thin entry point: arg parsing and dispatch only.
│   │   ├── lib.rs                Public API surface; Result alias, ConfigPlatform.
│   │   ├── error.rs              Error enum + ErrorKind: wire-facing kinds and exit codes.
│   │   ├── diagnostics.rs        Diagnostic/Severity + Reporter: warning accumulation and -v progress.
│   │   ├── cli.rs                Subcommand definitions and JSON output types (plugin contract).
│   │   ├── sln.rs                .sln parsing: member projects and declared configurations.
│   │   ├── msbuild.rs            .vcxproj evaluation — the hardest part of the system.
│   │   │   ├── project.rs        Lowers one .vcxproj/.props/.targets into a document-ordered IR.
│   │   │   ├── imports.rs        .props/.targets import resolution, in evaluation order.
│   │   │   ├── eval.rs           $(Macro)/%(Metadata) expansion and Condition evaluation.
│   │   │   ├── reserved.rs       Seeds the built-in properties project files never define.
│   │   │   ├── items.rs          Settles deferred item groups into the ClCompile sources.
│   │   │   ├── flags.rs          Translates settled metadata into MSVC (cl) switches.
│   │   │   └── paths.rs          Lexical resolution of MSBuild-spelled paths.
│   │   ├── toolchain.rs          ToolchainProvider trait + FakeToolchain for tests.
│   │   │   └── vswhere.rs        Real discovery via vswhere. Windows-only at runtime.
│   │   ├── compdb.rs             compile_commands.json construction and serialization.
│   │   ├── db.rs                 JSON state file: per-solution config/platform and mtimes.
│   │   └── testing.rs            Test-only: throwaway file trees for the tests that need real files.
│   └── tests/
│       ├── cli_surface.rs        Argument surface, exit codes, stdout/stderr discipline.
│       ├── generate.rs           Fixture-driven integration tests.
│       ├── support/              Shared golden-comparison harness (mod.rs), pulled in via `mod support;`.
│       └── golden/               Expected output per fixture. Never regenerate from a live toolchain.
│
├── nvim/                         Neovim plugin.
│   ├── lua/sln2cc/
│   │   ├── init.lua              setup() entry point.
│   │   ├── config.lua            Defaults and user option merging.
│   │   ├── cli.lua               Binary invocation and JSON decoding.
│   │   ├── discovery.lua         Project root detection, solution discovery, cache.
│   │   ├── picker.lua            vim.ui.select wrappers for solution and config choice.
│   │   ├── lsp.lua               clangd start/restart against an explicit database path.
│   │   ├── commands.lua          User command registration.
│   │   └── health.lua            :checkhealth sln2cc.
│   ├── plugin/sln2cc.lua         Load guard. Keep startup cost at zero.
│   ├── doc/sln2cc.txt            Vimdoc.
│   └── tests/
│       ├── minimal_init.lua      Headless test bootstrap; locates mini.test.
│       └── test_*.lua            mini.test specs. CLI is stubbed, never really invoked.
│
├── fixtures/                     Sample MSVC solutions: simple, multi-project,
│                                 props-inheritance, multi-config. Used by both Rust
│                                 integration tests and manual clangd verification.
└── docs/                         See section 9.
    └── adr/                      Architecture decision records, numbered. One file per decision.
```

## 3. CLI Guidelines

- **Output consumed by the plugin is JSON on stdout, behind `--json`.** Human-readable text is
  the stdout default; the plugin always passes the flag, and under it stdout carries the JSON
  envelope and nothing else. Never make the plugin scrape human-readable text. Human-facing
  progress goes to stderr, which also carries the error message in both modes. See "CLI Surface"
  in `docs/engineering_spec.md`.
- **Exit codes are meaningful.** Non-zero on failure, with a structured error the plugin can
  surface. Don't exit 0 with an error message.
- **Never silently guess.** If MSBuild evaluation hits an unsupported construct, warn about
  it. A subtly wrong compilation database is worse than a loud failure, because clangd will
  produce plausible nonsense.
- **The binary is stateless per-invocation.** All persistence goes through `db.rs`. No
  long-running process, no in-memory cache spanning runs.
- **Keep `main.rs` thin.** Logic lives in the library so integration tests can reach it.
- **Paths are absolute in generated output.** `compile_commands.json` consumers expect this.
- **Windows is the only real runtime target, but the crate must build everywhere.** Guard
  Windows-specific behavior behind the toolchain trait rather than `#[cfg(windows)]`
  scattered through the codebase.

## 4. Nvim Plugin Guidelines

- **Zero startup cost.** `plugin/sln2cc.lua` sets a load guard and nothing else. Real work
  happens on `setup()` or first command invocation. Never scan the filesystem at startup.
- **Never block the UI.** CLI invocations are async; parsing a large solution takes real
  time. Use callbacks, and give the user feedback while work is in flight.
- **Don't claim keymaps.** Expose commands; let users bind them. Document suggested
  bindings rather than setting them.
- **Use `vim.ui.select`**, not a bespoke picker, so users' existing picker UI applies.
- **Fail loudly and legibly.** Surface CLI errors via `vim.notify` with real detail. A
  silent no-op is the worst outcome — the user just sees clangd not working.
- **Everything configurable has a default in `config.lua`.** No magic values inline.
- **`:checkhealth` earns its keep.** Binary present and runnable, clangd on PATH, project
  root detectable, solution selected — these are the things users get wrong.
- **Plugin logic must be testable without the binary.** Stub CLI invocation at the
  `cli.lua` boundary.

## 5. Constraints and Policies

**Dependencies**

- Prefer the standard library. Every crate and Lua dependency is a maintenance liability.
- CLI dependencies are settled: `roxmltree`, `clap`, `serde`/`serde_json`, `thiserror`. Persistence
  is a JSON file, not SQLite, and there is deliberately no logging crate. The reasoning — including
  why `quick-xml`, `rusqlite`, `anyhow`, and `tracing` were rejected — is in `Cargo.toml` comments
  and "Dependency Decisions" in the engineering spec. Read it before proposing an addition; don't
  add dependencies casually, and if one is genuinely needed, say why.
- The plugin depends on Neovim itself and mini.test (tests only). Avoid adding runtime plugin
  dependencies.

**Security**

- Never execute paths or flags read out of a project file. Solution and project files are
  untrusted input — parse them, don't evaluate them as commands.
- Shelling out (`vswhere`) uses argument vectors, never a shell string, so nothing in a path
  can be interpreted as shell syntax.
- Treat `.sln`/`.vcxproj` input as hostile: malformed XML, cyclic imports, absolute paths
  escaping the project root, and deep recursion must all fail gracefully rather than hang or
  panic.
- Don't log full environment contents or absolute user paths in anything shareable.

**Code quality**

- `cargo fmt` and `cargo clippy -- -D warnings` must both be clean. CI enforces this.
- No `unwrap()`/`expect()` on anything that can fail in normal operation. Reserve them for
  genuine invariants, with a comment saying why it can't fail.
- No panics in library code paths reachable from the CLI. Return errors.
- Public items get doc comments explaining *why*, not restating the signature.
- New behavior comes with tests. Match the surrounding style rather than importing new
  idioms.

## 6. Repository Etiquette

- **Branch from `main`.** Never commit directly to `main`.
- Branch names: `feat/<short-desc>`, `fix/<short-desc>`, `docs/<short-desc>`,
  `refactor/<short-desc>`, `chore/<short-desc>` (dependencies, tooling, CI, housekeeping).
- **Always run the full test suite before pushing.** Both the CLI and the plugin — see
  section 7. A green local run is the minimum bar for a push; don't rely on CI to catch it.
- Also run `cargo fmt` and `cargo clippy -- -D warnings` before pushing; CI fails on either.
- Commits are focused and imperative-mood ("Add sln project enumeration"). Keep unrelated
  changes out.
- A change that adds a feature or fixes a bug updates `docs/changelog.md` in the same
  commit — see section 9.
- A change that settles an architecture or design decision adds an ADR under `docs/adr/` in
  the same commit — see section 9.
- Never commit generated artifacts: `cli/target/`, `.sln2cc/` (including inside
  `fixtures/`). Both are gitignored — don't force-add them.
- Never commit golden files regenerated against a real Visual Studio installation; they must
  come from `FakeToolchain` output.

## 7. Commands

**CLI** — run from `cli/`:

```sh
cargo build                      # build
cargo test                       # unit + integration tests
cargo test -- --ignored          # see unimplemented placeholder tests
cargo fmt                        # format
cargo fmt --check                # verify formatting (CI)
cargo clippy -- -D warnings      # lint (CI)
cargo check --all-targets        # fast type check
```

**Neovim plugin** — run from `nvim/`:

```sh
nvim --headless --noplugin -u tests/minimal_init.lua \
  -c "lua require('mini.test').setup()" \
  -c "lua MiniTest.run()"
```

`--noplugin` matters — without it the developer's own config loads into the test session and
throws unrelated errors. mini.test is found automatically at
`stdpath("data")/lazy/mini.test`, or set `MINI_TEST_DIR`.

**Full pre-push check:**

```sh
cd cli && cargo fmt --check && cargo clippy -- -D warnings && cargo test && cd ..
cd nvim && nvim --headless --noplugin -u tests/minimal_init.lua \
  -c "lua require('mini.test').setup()" \
  -c "lua MiniTest.run()"
```

## 8. Testing

**Layers**

| Layer | Where | Covers |
| ----- | ----- | ------ |
| Rust unit | `cli/src/**` (`#[cfg(test)]`) | Parsing, macro/condition evaluation, database logic. |
| Rust integration | `cli/tests/` | Full pipeline against `fixtures/`, compared to `tests/golden/`. |
| Plugin | `nvim/tests/` | Discovery caching, persistence, clangd lifecycle. CLI stubbed. |
| Manual E2E | by hand | Whether clangd actually behaves correctly against generated output. |

**Rules**

- Integration tests always inject `FakeToolchain`. A test that depends on a real Visual
  Studio installation is a broken test — it won't run in CI and won't reproduce across
  machines.
- Plugin tests never invoke the real binary and never require a Windows toolchain.
- Unimplemented tests are `#[ignore]`d (Rust) or `MiniTest.skip()` (Lua) placeholders rather than
  deleted, so the gap stays visible.

**MVP testing constraints**

The product is not functional yet, so calibrate expectations:

- Automated coverage stops at "is the generated `compile_commands.json` correct." Verifying
  clangd's actual responses is **manual** for the MVP; automating it is explicitly deferred.
- Only VS2019/VS2022 schemas are in scope. Don't write tests against older toolsets.
- The fixture corpus is incomplete. Growing it is part of the work, not a prerequisite.
- Expect most tests to be placeholders right now. When implementing a module, replace its
  placeholders rather than adding tests alongside them.

**Manual verification checklist**

1. Generate a database for a fixture (or a real solution) with the CLI.
2. Open one of its source files in Neovim with the plugin active.
3. Confirm: completion works, go-to-definition resolves, diagnostics appear, and clangd
   reports no missing includes.
4. Switch configuration/platform, regenerate, restart clangd, confirm flags actually changed.

## 9. Documentation

| Doc | What it covers |
| --- | --- |
| [docs/project_spec.md](docs/project_spec.md) | Purpose, audience, problem, product behavior, multi-solution handling. **The source of truth for *what* and *why*.** |
| [docs/engineering_spec.md](docs/engineering_spec.md) | Tech stack, architecture, module design, engineering requirements, testing strategy, CI. **The source of truth for *how*.** |
| [docs/adr/](docs/adr/) | Architecture decision records — one file per decision, capturing the problem, the decision, and the discussion that got there. **The source of truth for *why it isn't otherwise*.** |
| [docs/milestones.md](docs/milestones.md) | Feature groupings for MVP / V1 / V2 / future. No timelines. |
| [docs/todos.md](docs/todos.md) | Issue tracker: per-release checklist of scoped work items, each with a branch name and required tests. **Stands in for GitHub issues** until the project has a repo host. |
| [docs/changelog.md](docs/changelog.md) | Running log of features and bugfixes. **Updated as work lands, not at release time** — it's the source material for release notes. |
| [docs/glossary.md](docs/glossary.md) | MSBuild and Visual Studio vocabulary as every other doc uses it, plus sln2cc's own terms. **Reference, not a decision record** — if a definition and a spec disagree, the spec wins and the glossary is wrong. |
| [docs/brainstorm.md](docs/brainstorm.md) | Original thinking. **Historical — partly superseded.** It describes an LSP-proxy architecture that was abandoned. Read for context, don't treat as current. |
| [README.md](README.md) | User-facing intro, prerequisites, build and test instructions. |

**Consult these before making design decisions.** Each spec ends with an "Open Questions"
section; if a task touches one of those, raise it rather than silently picking an answer.

**Update the changelog as you go.** Any user-visible feature or bugfix gets an entry under
`## [Unreleased]` in [docs/changelog.md](docs/changelog.md) *in the same change that
implements it* — not reconstructed from git log at release time. Write entries for someone
using the tool, not someone reading the diff: what changed for them, not which functions
moved. Skip purely internal work (refactors, test scaffolding, doc edits) unless it changes
behavior. At release, the `[Unreleased]` section becomes the new version's notes.

**Write an ADR when a decision gets made.** Anything that settles an architecture or design
question — a dependency chosen or rejected, a module boundary drawn, an approach abandoned —
gets a record in [docs/adr/](docs/adr/) *in the same change that implements it*. The specs say
what the design is; ADRs say why it is that and not the alternative, so the same debate isn't
had again a year later. Conventions:

- **Filename:** a zero-padded number, then a 3–6 word hyphenated descriptor — e.g.
  `01-address-dependencies.md`. Numbers are assigned in order and never reused, and a
  superseded ADR stays put with its status changed rather than being deleted or rewritten.
- **Always reference an item in [docs/todos.md](docs/todos.md).** An ADR records the reasoning
  behind a piece of scoped work, so it names the issue it came out of. If a decision doesn't
  correspond to a todo item, add the item first — a decision with no work behind it is a spec
  change, not an ADR.
- **Cover three things:** the problem being addressed, the decision reached, and the back and
  forth that produced it — including options rejected and why, and what the decision costs.
  The discussion is the part that has value later; a bare verdict doesn't prevent relitigation.
- **Link both directions.** Note the decision's outcome on the todo item and record it in the
  relevant spec, with the spec's section pointing at the ADR for the full argument. Specs stay
  the readable current-state description; ADRs hold the depth.

**Keep the rest current.** After completing a milestone, resolving an open question, or
making a structural change:

- Record resolved open questions as decisions in the relevant spec, and remove them from the
  Open Questions list.
- Add any *new* open questions that surfaced — an unanswered question in writing beats an
  undocumented assumption.
- Update `docs/milestones.md` when a milestone's scope changes or completes.
- Update the tree in section 2 of this file when directories or modules are added or moved.
- Update `README.md` when build/test commands change.
