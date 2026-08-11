# ADR 22 — Staleness Tracking Compares Recorded File Mtimes, Not Re-Evaluated Import Sets

- **Status:** Accepted
- **Date:** 2026-08-10
- **Issue:** [MVP-20 — Modification-time staleness tracking](../todos.md#persistence)
  (`feat/db-staleness`)
- **Affects:** `cli/src/db.rs`, [ADR 20](20-state-file-shape.md), [ADR 21](21-store-mutation-signature.md), MVP-22

## Problem

`Store::is_up_to_date` has been `todo!("MVP-20")` since ADR 20 settled the state file shape;
`SolutionRecord.files` exists in the schema but nothing populates or reads it. ADR 21 flagged
that this issue "may face the same `&mut self` question" `set_active_config` did, without
deciding it, since recording a dependency set and checking one against the filesystem might
turn out to be the same operation or two different ones. Separately, the issue's own test list
names a case — "adding a new import invalidates the cache" — that isn't obviously answerable by
a pure mtime comparison: an import that's *new* wasn't in the previously recorded set at all, so
naively it seems like detecting it requires re-discovering the current import graph, which is
the expensive operation staleness tracking exists to avoid.

## Decision

Two methods, not one:

```rust
impl Store {
    pub fn is_up_to_date(&self, solution: &Path) -> Result<bool>;
    pub fn record_dependencies(&mut self, solution: &Path, files: &[PathBuf]) -> Result<()>;
}
```

- **`record_dependencies` takes `&mut self`** (settling the question ADR 21 left open) and
  **fully replaces** `SolutionRecord.files` on every call, rather than merging. A call represents
  one complete evaluation's dependency set; a file that dropped out of the import graph (e.g. a
  conditional import no longer satisfied) must stop being tracked, or it would linger as an entry
  that can never go stale and never gets cleaned up.
- **`is_up_to_date` stays `&self`** and only ever compares the *already-recorded* set against the
  current filesystem — it never re-runs evaluation to discover files that joined the dependency
  graph without touching a file already being tracked. A missing recorded file, or any other
  failure reading its current mtime, both count as stale (`Ok(false)`), matching the issue's own
  wording ("up to date only if... anything else triggers a full reparse"). `record_dependencies`,
  by contrast, propagates a metadata failure as `Error::io` — it runs immediately after
  evaluation successfully read every one of those files, so failing to stat one moments later is
  a real race or bug, not an expected outcome.
- **No new `Error`/`ErrorKind` variants.** `Error::io` already covers everything
  `record_dependencies` can fail on, matching ADR 19/20/21's own precedent of not adding a
  variant until something actually needs it.

## Discussion

**Why "adding a new import invalidates the cache" doesn't need a full dependency-set diff.**
Editing a project to add an `<Import>` means editing the file that names it — the `.vcxproj`
itself, or a `.props` it already includes — and that file's own mtime is already part of the
recorded set from the prior evaluation. The existing mtime comparison catches the edit without
needing to know anything about *what* changed inside the file. The only scenario this doesn't
catch is evaluation *conditions* changing which files get imported without editing any file
already on record (e.g. an environment variable flips which conditional `<Import>` is
satisfied) — a limitation every mtime-based build cache shares (`make` has the identical gap),
and out of scope for this issue.

**Why not have `is_up_to_date` re-evaluate and diff the resulting file set instead.** That would
mean doing the expensive work (parsing the `.sln`, every `.vcxproj`, resolving every import) on
every staleness check, which defeats the entire point of caching — `generate` would pay the full
evaluation cost whether or not anything changed. A pure mtime comparison over the previously
recorded set is the cheap check the issue asks for; catching import-graph changes precisely would
require the expensive one.

**Why `record_dependencies` replaces rather than merges.** The alternative — accumulating files
across calls — would mean a file that's no longer part of a solution's dependency graph keeps
being checked forever, and worse, keeps being able to mark the solution stale even after it
stopped mattering. Since each call already represents a *complete* dependency set (per the
issue's own description — "the dependency set is everything the evaluation touched... must be
captured during evaluation"), replacing is the direct expression of that: the new set is
authoritative, not additive.

**Why a metadata failure means "stale" in `is_up_to_date` but "error" in `record_dependencies`.**
These functions run at different points relative to the filesystem state they're reading.
`is_up_to_date` checks files that may have been deleted, moved, or become inaccessible in the
time since the last evaluation — that's exactly the condition it exists to detect, so folding it
into "stale" rather than a hard error keeps the caller's control flow simple (one boolean, no
special-casing IO errors from genuinely corrupt state). `record_dependencies` checks files that
were just read successfully as part of the evaluation that's calling it; a failure there points
at a real problem (a race with another process, a permissions change mid-run) worth surfacing
rather than silently swallowing.

**Test infrastructure: `TempTree::set_mtime`.** Asserting "touching a dependency reports stale"
needs precise control over a file's mtime, because two `std::fs::write` calls in quick succession
can land within the same filesystem timestamp tick and produce a flaky test. `std::fs::File::
set_modified` (stable since Rust 1.75, under this crate's 1.85 MSRV) does this without a
`filetime`-style dependency, and is added to `testing.rs`'s `TempTree` rather than kept local to
`db.rs`, matching how `write`/`project` already centralize "make files exist for tests that need
real files on disk" there.

## Consequences

- **The "new import" case's coverage is only as good as the container file's own mtime.**
  Editing a project file and re-checking within the same filesystem timestamp tick (finer than
  the whole-second resolution `SolutionRecord.files` stores) is a known, accepted gap shared by
  the general mtime-comparison approach — the test suite uses `TempTree::set_mtime` to sidestep
  it rather than pretending it doesn't exist.
- **`known_configs` is still unaddressed.** ADR 20 reserved the field with no owning accessor;
  this issue doesn't claim it either, since it's scoped to `files`/`is_up_to_date` only. Still
  open, as ADR 20 already recorded.
- **`Store::open` is still not reachable from the CLI.** `record_dependencies`/`is_up_to_date`
  have no caller until MVP-22 wires the `generate` pipeline through `Store`, matching MVP-16/17/
  18/19's own precedent (ADR 18/19/20/21) of skipping the changelog for work not yet reachable
  from a real invocation.
