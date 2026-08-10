# ADR 19 — Hashed Storage Directory and Atomic `compile_commands.json` Write

- **Status:** Accepted
- **Date:** 2026-08-10
- **Issue:** [MVP-17 — Serialize and write `compile_commands.json`](../todos.md#compilation-database) (`feat/compdb-write`)
- **Affects:** `cli/src/compdb.rs`, `cli/src/error.rs`, [engineering_spec.md](../engineering_spec.md)
  ("Storage Layout"), [ADR 18](18-compdb-entry-assembly.md), [ADR 10](10-import-resolution.md), MVP-22

## Problem

`compdb::build` (MVP-16, ADR 18) produces `Vec<CompileCommand>`; `compdb::write` was still
`todo!()`. Two things needed settling that "Storage Layout" in `engineering_spec.md` had
already designed but nothing had implemented: which `.msvclsp/<solution-name>-<hash>`
directory a given `.sln` writes into (and how a hash collision between two different
solutions is caught instead of silently sharing output), and how the JSON itself gets to disk
without a crash or a concurrent run leaving clangd reading a truncated file.

## Decision

```rust
fn output_dir_name(sln_path: &Path) -> String;
pub fn resolve_output_dir(root: &Path, sln_path: &Path) -> Result<PathBuf>;
pub fn write(commands: &[CompileCommand], output_dir: &Path) -> Result<()>;
```

- **Directory naming and collision detection are a separate step (`resolve_output_dir`) from
  serialization (`write`).** `write` keeps taking an already-resolved output directory —
  unchanged from the signature `todo!()` already carried — so ADR 18's `build`→`write` handoff
  isn't disturbed, and MVP-22 (wiring `generate` to this pipeline) calls both in sequence.
- **`output_dir_name` is pure**, hashing the lowercased, lexically-normalized absolute
  directory containing the `.sln` via a hand-rolled FNV-1a (not `DefaultHasher`, whose
  algorithm is unspecified across releases — see "Storage Layout"), formatted as
  `<solution-stem>-<8 lowercase hex digits>`. Normalization reuses
  `msbuild::paths::normalize`, already used for include-path resolution, rather than a second
  lexical-path implementation.
- **A `.source` marker file inside each output directory** records the same normalized,
  lowercased source-directory string used to compute the hash. `resolve_output_dir` creates it
  (and the directory, and `.msvclsp` itself if absent) on first use, and on every later call
  checks it against the current solution's own value. A mismatch is
  `Error::StorageCollision`, naming both the claimed directory and the two conflicting source
  directories — never a silent share or overwrite (CLAUDE.md §3).
- **`Error::StorageCollision` gets exit code `10`**, the next unused code. `8` stays reserved
  (ADR 10 retired it from `path-escape`); `9` is `Unimplemented`.
- **`write` serializes through a local `Entry` struct** (`directory`/`file` as
  `.display().to_string()`, `arguments` borrowed as `&[String]`) rather than deriving
  `Serialize` on `CompileCommand` directly. `PathBuf`'s own `Serialize` impl errors on
  non-UTF-8 paths; `.display()` is the convention `compdb::arguments` already uses for the same
  fields.
- **Atomic write is temp-file-then-rename**, matching the idiom `db.rs`'s own module doc
  already commits to (for its own, still-unimplemented, state file) — this issue is the first
  to actually build it.
- **`write` creates its own output directory too**, even though `resolve_output_dir` will
  already have created it in the intended call order — keeps `write` safe to call on its own
  (as the tests do) without depending on that order.

## Discussion

**Why a marker file at all, rather than trusting the hash and never checking.** FNV-1a over an
8-hex-digit (32-bit) output has a real, if small, birthday-bound collision probability across
enough distinct solution directories on one machine. The spec's own requirement is that a
collision fails loudly rather than silently sharing a directory between two solutions'
generated output — plausible-but-wrong `compile_commands.json` is exactly what CLAUDE.md §3
warns against, and a wrong compilation database here means clangd confidently serving another
project's flags. The marker is the cheapest way to make that check possible: one string,
checked before every write.

**Why plain text, not a small JSON envelope.** There is exactly one field. `serde_json` for a
single string is ceremony this crate's own dependency policy (`Cargo.toml`'s comments; "prefer
the standard library") argues against.

**Why `resolve_output_dir` and `write` are not one function.** The issue's own test list asks
for the hash/naming behavior (same directory hashes the same way regardless of casing; two
directories never collide) to be verifiable without touching disk, and for the atomic-write
behavior (stable ordering, an existing file replaced rather than corrupted) to be verifiable
without caring about hashing. Splitting the two gives each its own narrow, fast test surface;
combining them would force every naming test through a temp directory for no benefit.

**Why `Entry` instead of deriving `Serialize` on `CompileCommand`.** `std::path::PathBuf`'s
`Serialize` implementation is via `OsStr`, which is fallible for non-UTF-8 paths on some
platforms. `compdb::arguments` (ADR 18) already renders every path through `.display()` into
the argument strings; `Entry` keeps that the one path-to-string convention in this module
instead of introducing a second, subtly different one that only fails at serialization time.
It also means `serde_json::to_string_pretty` is provably infallible for this crate's own data —
only `String`/`Vec<String>` fields, no floats, no non-string map keys — which is what justifies
an `.expect()` on that call under CLAUDE.md §5's "reserve for genuine invariants, with a
comment" rule, rather than plumbing a new `Error` variant for a case that cannot occur.

**One option considered and rejected:** hashing the full `.sln` path (including the filename)
rather than just its directory. Rejected because the directory is what disambiguates two
solutions sharing a filename — the stated purpose of this scheme — and because a solution
renamed in place (same directory, new `.sln` filename) would otherwise silently orphan its old
`.msvclsp` directory for a reason unrelated to the "solution moved" cost the spec already
accepts.

## Consequences

- **`compdb::write` now has a real implementation**, but nothing in the CLI calls it yet —
  `generate` isn't wired to this pipeline until MVP-22. No changelog entry, matching MVP-16's
  own precedent (ADR 18) for the same reason.
- **A hash collision can't be exercised end-to-end without a contrived test setup.** Tests seed
  a `.source` marker by hand at the exact path `output_dir_name` computes, rather than
  constructing a genuine FNV-1a collision, since one can't be constructed by hand. The
  collision-handling code path is real; only the *trigger* is synthetic in tests.
- **A moved solution directory still orphans its old `.msvclsp/<name>-<hash>/`**, unchanged
  from what "Storage Layout" already accepted — this issue implements the scheme, not a new
  tradeoff.
