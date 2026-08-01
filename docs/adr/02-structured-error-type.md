# ADR 02 — A Structured Error Type With Machine-Readable Kinds

- **Status:** Accepted
- **Date:** 2026-07-31
- **Issue:** [MVP-02 — Replace the placeholder error type](../todos.md#mvp) (`refactor/error-type`)
- **Affects:** `cli/src/error.rs` (new), `cli/src/lib.rs`,
  [engineering_spec.md](../engineering_spec.md) ("Error Handling"), MVP-03, MVP-04, MVP-21, V2-04

## Problem

`lib.rs` defined `pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>` as an
explicit placeholder. That type cannot express the one thing the plugin contract requires of it.
[ADR 01](01-address-dependencies.md) rejected `anyhow` precisely because the Neovim plugin has to
tell "solution not found" from "unsupported MSBuild construct" from "no Visual Studio installation"
in order to map a failure onto an exit code and a JSON error kind, and a boxed trait object forces
downcasting to recover that distinction. `thiserror` was chosen for this issue to spend.

The sequencing matters. MVP-02 sits ahead of MVP-03 (subcommand dispatch and exit-code plumbing)
and MVP-21 (the first end-to-end path) because both dispatch on the kind, and it sits ahead of the
parsing issues because every one of them returns errors. Every module already imported
`crate::Result` while every function body was still `todo!()`, so the swap was nearly free at this
point and would only have grown more expensive.

The open design questions were the enum's shape, how many variants to define before the modules
that raise them exist, and how much of the exit-code and JSON contract belongs here rather than in
MVP-03.

## Decision

A flat crate-level `Error` enum in a new `cli/src/error.rs`, with fourteen leaf variants plus a
`Context` wrapper, and a separate `ErrorKind` carrying the wire-facing classification.

`Error` variants carry the offending path, line, or construct — `SolutionParse { path, line,
detail }`, `MissingImport { import, referenced_by }`, `InvalidConfiguration { requested, available }`
— so the human-readable message can name what actually went wrong. `ErrorKind` is a `Copy` enum
whose `as_str()` returns explicitly-written kebab-case strings (`"solution-not-found"`,
`"unsupported-construct"`), and `ErrorKind::exit_code()` maps kinds onto seven coarse exit classes.
`Error::kind()` is total, and `Error::Context` delegates its kind to what it wraps.

Supporting decisions: no `From<std::io::Error>` impl — callers go through `Error::io(path, source)`;
a `ResultExt::with_context` extension trait building its message lazily; `Display for ConfigPlatform`
added to `lib.rs` so a pair renders as `Debug|x64`. Serialization of the JSON error payload is
deliberately *not* here — `ErrorKind` derives no `serde` traits, and MVP-03 will serialize
`as_str()` into the output types it owns.

The resulting kind and exit-code tables are recorded in "Error Handling" in
[engineering_spec.md](../engineering_spec.md).

## Discussion

**Flat enum vs. per-module error types.** The obvious alternative was `SlnError`, `MsbuildError`,
`ToolchainError`, and `DbError`, each wrapped by a top-level `Error` via `#[from]`. That is the
right shape for a library whose modules are independently consumable, which these are not: they
form one pipeline behind one binary, and nothing outside the crate will ever hold a `SlnError`. The
costs were concrete — four extra types, `#[from]` conversions at every module boundary, and a
`kind()` delegated down through a layer of wrappers for no gain, since the top-level enum would
have to enumerate every kind anyway to produce the wire string. The flat enum's real cost is that
`error.rs` now knows about every module's failure modes in one place; that centralization is what
makes the kind table reviewable as a contract, so it was accepted as a feature rather than tolerated.

**A separate `ErrorKind` rather than serializing variant names.** `thiserror` could have been paired
with a `serde` derive to emit the variant name directly, saving a second enum and a `match`. That
ties a published wire contract to Rust identifiers: renaming `SolutionNotFound` or splitting a
variant during a later refactor would silently change what the plugin receives, and nothing in the
Rust build would notice. Writing the strings out by hand costs one `match` arm per variant and makes
breaking the contract require editing a string literal — which a test then asserts against a literal
list, so it cannot happen as a side effect. The kinds are also deliberately coarser than the
variants where the plugin cannot act on the difference: `ToolchainNotFound` covers both "`vswhere`
is absent" and "no instance carries the C++ workload", because the user does the same thing in
either case. `ToolchainIncomplete` stayed separate for the opposite reason — "install Visual Studio"
and "install a Windows SDK" are different actions.

**Context wrapping that preserves the kind.** MSBuild evaluation nests deeply, and "evaluating
`a.props`, imported by `b.vcxproj`" is exactly the context ADR 01 said belongs in the error payload
rather than in log lines. The trap is that a naive wrapper variant gets its own kind, so adding
context would silently reclassify a failure — an `Io` error becomes an `Import` error somewhere up
the stack, and the plugin's dispatch breaks depending on how deep the call chain happened to be.
`Error::Context` therefore delegates `kind()` to its source, making context free to add at any
depth. Its `Display` prints only the context string, with the cause reachable through `source()`,
which keeps the chain composable rather than producing the run-on "A: B: C: D" messages that come
from concatenating at each level. Rendering that chain for humans is MVP-03's problem.

**Exit-code granularity.** One code per kind was rejected: fourteen codes is a second contract to
keep stable, and the plugin never needs it because it reads the precise kind from JSON. Collapsing
everything to `1` was also rejected, since the tool is run by hand constantly while debugging the
evaluator and `$?` distinguishing "your solution is missing" from "your Visual Studio install is
broken" is worth something in a shell. Seven classes is the compromise. `2` is reserved and never
returned — `clap` uses it for usage errors, and overloading it would make "you typed the command
wrong" indistinguishable from a real failure.

**How many variants to define now.** Defining fourteen variants against modules that are still
`todo!()` is speculative design, and the alternative — five or six broad variants that each
downstream issue refines — is the more disciplined instinct. It was rejected because the speculation
is already done and written down: every variant here corresponds to a failure mode `todos.md`
names explicitly (MVP-05's line-numbered `.sln` errors, MVP-10's missing imports and cycles, MVP-14's
absent installation, MVP-18's malformed state file, MVP-22's invalid configuration listing the valid
ones). The genuine cost is that the wire strings are published before anything raises them, so a
kind that turns out to be wrong has to be changed rather than never introduced. That is cheap while
the plugin is stubs and expensive after MVP-21, which is an argument for doing it now, not later.

**`roxmltree::Error` and `serde_json::Error` as `#[source]` fields.** Storing these directly leaks
the XML and JSON crate choices into the error type's public surface, which ADR 01 treated as
implementation detail. The alternative is flattening them into `String` at the raise site. Keeping
them buys a real `source()` chain and, for `roxmltree`, position information in the message for
free. Both crates are already direct dependencies, so nothing new enters the tree, and the coupling
is confined to two variants — replacing the XML parser would mean changing one field type and the
call sites that construct it.

**No `From<std::io::Error>`.** The blanket impl is what makes `?` pleasant on file reads, and it was
rejected anyway: `std::io::Error` does not carry the path, so a bare `?` produces "permission
denied" with no indication of which of the dozens of imported `.props` files was at fault.
`Error::io(path, source)` forces the call site to supply it. This is deliberate friction on the one
operation most likely to fail in a way the user must diagnose from a message alone.

## Consequences

- `Result<T>` now resolves to a concrete `Error`. No call site changed, because every module already
  used the alias and every body is still `todo!()` — the swap cost nothing today and would have cost
  progressively more from MVP-05 onward.
- **MVP-03 narrows** to wiring `ErrorKind::exit_code()` into `main.rs` and serializing
  `ErrorKind::as_str()` into a JSON payload, rather than designing that contract itself.
- **MVP-04** gains a fixed rule to respect: rendering the `source()` chain for humans goes to
  stderr, and the kind goes to stdout as JSON.
- Downstream parsing issues (MVP-05, MVP-07 through MVP-10, MVP-14, MVP-15, MVP-18, MVP-22) now have
  their error variants waiting rather than each inventing one, at the price of variants sitting
  unconstructed until then. `cargo clippy` does not flag them, since they are `pub`.
- **V2-04** ("Surface CLI errors in the plugin UI") has a fixed kind list to switch on.
- No changelog entry: this is internal plumbing with no user-visible behavior change.
- **New open question:** whether `Error` should be `#[non_exhaustive]`. It is a no-op inside the
  defining crate and there are no external consumers today, so it was left off; it becomes a real
  question if the crate is ever published.
