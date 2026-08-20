# ADR 53 — Renaming the Project from `msvclsp` to `sln2cc`

- **Status:** Accepted
- **Date:** 2026-08-20
- **Issue:** [MVP-47 — Rename the project from msvclsp to sln2cc](../todos.md#mvp-47)
  (`chore/rename-to-sln2cc`)
- **Affects:** `cli/Cargo.toml`, `cli/Cargo.lock`, `cli/tests/cli_surface.rs`,
  `cli/tests/support/mod.rs`, every prose reference under `docs/**`, `CLAUDE.md`, `README.md`,
  `.gitignore`, and the whole `nvim/` plugin tree (`nvim/lua/msvclsp/` → `nvim/lua/sln2cc/`,
  `nvim/plugin/msvclsp.lua` → `nvim/plugin/sln2cc.lua`, `nvim/doc/msvclsp.txt` →
  `nvim/doc/sln2cc.txt`, and every `:Msvclsp*` user command)

## Problem

`msvclsp` reads as "MSVC LSP" — an implementation the project deliberately rejected. CLAUDE.md's
own architecture section says so directly: "The CLI is not an LSP server. An earlier design
proxied LSP traffic to a child clangd; that was dropped." The CLI only ever produces a
`compile_commands.json`; clangd, run and managed by the Neovim plugin, does the actual language
work. Nothing in the current design speaks LSP at all — the name was accurate to a design that
was abandoned before MVP work began, and has been quietly wrong ever since.

## Decision

Rename the project to `sln2cc`: `.sln` in, `compile_commands.json` (`cc`) out — a name that
describes the one thing the tool actually does, rather than a role it explicitly doesn't fill.
Applied mechanically and exhaustively rather than partially:

- `cli/Cargo.toml`'s package name, and `cli/Cargo.lock` regenerated to match (not hand-edited).
- The Neovim plugin's Lua module namespace (`require("msvclsp...")` → `require("sln2cc...")`),
  its file layout (`nvim/lua/msvclsp/` → `nvim/lua/sln2cc/`, `nvim/plugin/msvclsp.lua` →
  `nvim/plugin/sln2cc.lua`, `nvim/doc/msvclsp.txt` → `nvim/doc/sln2cc.txt`), and every user
  command (`:MsvclspGenerate` → `:Sln2ccGenerate`, and the same for `SelectConfig`, `Rescan`,
  `Restart`, `SelectSolution`).
- The generated artifact directory, `.msvclsp/` → `.sln2cc/`, in `.gitignore` and in every doc
  that names it.
- Test-harness constants tied to the old name: `CARGO_BIN_EXE_msvclsp` (which Cargo derives
  automatically from the package name, so this followed from the `Cargo.toml` change) and the
  `MSVCLSP_UPDATE_GOLDEN` golden-regeneration env var, now `SLN2CC_UPDATE_GOLDEN`.
- Every prose mention across `CLAUDE.md`, `README.md`, and `docs/**`, including the spelled-out
  "MSVC LSP" phrasing in `project_spec.md`, `engineering_spec.md`, `milestones.md`, and
  `brainstorm.md`.

Not touched: leftover `.msvclsp/` directories from previous local test runs (e.g. under
`fixtures/simple/`). These are gitignored scratch output, not tracked, and regenerate under the
new name the next time `generate` runs against those fixtures — there was nothing to rename.

## Discussion

**Whole-project rename, not a soft alias.** A transition period where both `msvclsp` and `sln2cc`
resolve to the same binary/commands was considered and rejected. The project is pre-1.0 with no
external users to break, so the cost of a compatibility shim (two sets of Neovim commands, two
possible artifact directory names to check for staleness, a doc set that has to explain both) buys
nothing here and would itself become the kind of inconsistency this rename exists to remove.

**Verification is the existing test suite, unchanged.** This is a rename, not a behavior change,
so no new tests were written. What matters is that the full suite — `cargo test` (431 unit tests,
18 CLI-surface tests, 14 fixture/golden-comparison integration tests), `cargo clippy -- -D
warnings`, `cargo fmt --check`, and the Neovim `mini.test` suite (139 cases) — passes unchanged
under the new names. The golden-file comparisons are the most load-bearing check here: they prove
the renamed `.sln2cc/` output directory still produces byte-identical `compile_commands.json`
content, i.e. that the rename touched naming and nothing else.

**`sln2cc` over the alternatives considered** (`vcxcc`, `vcdb`, `msvcdb`): shortest names that
still self-explain the tool's job to someone who has never seen the project. `sln2cc` reads
unambiguously as "sln to compile_commands," which is exactly what the CLI does — `vcdb` reads too
easily as "version control db," and `msvcdb`/`vcxcc` still lean on the MSVC/vcxproj half of the
picture rather than the transformation itself.

## Consequences

- **`docs/glossary.md`'s own definitions are unaffected** — they describe MSBuild and Visual
  Studio vocabulary, which didn't change. Only the two sections describing msvclsp's *own*
  vocabulary needed the name swapped in, which the mechanical rename already covered.
- **Every ADR before this one keeps its original prose intent** even though the tool name inside
  quoted code and prose now reads `sln2cc` — ADRs are a historical record of *decisions*, and the
  rename doesn't change what was decided in any of them, only what the thing they describe is
  called.
- **A contributor pulling an old branch or stashed diff pre-dating this rename** will see merge
  conflicts on nearly every doc file and on `nvim/lua/msvclsp/**`'s deleted paths. There is no way
  around this for a rename this wide; it is why the change lands as one commit rather than
  spread across several.
