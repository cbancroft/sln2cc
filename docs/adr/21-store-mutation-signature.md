# ADR 21 — `Store::set_active_config` Takes `&mut self`

- **Status:** Accepted
- **Date:** 2026-08-10
- **Issue:** [MVP-19 — Persist active configuration/platform per solution](../todos.md#persistence)
  (`feat/db-active-config`)
- **Affects:** `cli/src/db.rs`, [ADR 20](20-state-file-shape.md), MVP-20

## Problem

MVP-18 stubbed `Store::set_active_config` as `pub fn set_active_config(&self, ...)`. Implementing
it for real means mutating `self.state.solutions` and rewriting `state.json`, neither of which is
possible through a shared reference. Something has to change: the signature, or the storage
strategy behind it.

## Decision

`set_active_config` becomes `pub fn set_active_config(&mut self, solution: &Path, config:
&ConfigPlatform) -> Result<()>`. It looks up (or inserts) the solution's `SolutionRecord` in
`self.state.solutions` via `solution_key`, sets `active_config`, and calls the existing
`write_state` helper to rewrite the file atomically — the same path `open()` already uses to write
a fresh state file. `active_config` stays `&self`; it only reads the in-memory copy `open()`
already loaded.

Every `set_active_config` call writes through immediately. There is no batching or dirty-flag
deferral.

## Discussion

**Why `&mut self` rather than interior mutability** (`RefCell<State>`, `Mutex<State>`). Nothing in
the codebase needs `Store` reachable through a shared reference — it's created once per CLI
invocation and used by a single owner. Interior mutability would hide a real mutation behind a type
that claims to be read-only, for no capability anything currently needs. ADR 20 already committed
to `Store` owning `state` for the object's lifetime rather than re-reading per call; `&mut self` is
the direct expression of "this call changes what's held," and matches the crate's general posture
against unnecessary indirection (CLAUDE.md §1).

**Why this doesn't conflict with ADR 20 calling the stub signatures "already-published".** That
language was about the *parameter list* — ADR 20 confirms `set_active_config`/`active_config`/
`is_up_to_date` don't need a `root` parameter added, because `Store` derives and stores its own
root at `open()` time. It says nothing about `&self` vs `&mut self`, and nothing in the CLI calls
`Store::set_active_config` yet (`Store::open` itself isn't reachable from a subcommand until
MVP-21/22), so there is no external caller this change breaks.

**Why write-through on every call, not batched.** `write_state` is already a full atomic
temp-file-then-rename per call — that's what `open()`'s fresh-state path does, and there is no
existing "dirty" tracking to hook a deferred write into. The state file holds a handful of KB;
batching would trade a real simplicity cost for a performance win nothing here needs. This matches
the module doc's own accepted tradeoff (last-writer-wins across *processes*, not a transactional
log) — deferring within a process would only widen the window where an in-memory selection isn't
yet reflected on disk, for a workload where that window buys nothing.

**Why not have `active_config` also go through `&mut self`, for symmetry.** It doesn't mutate
anything — it reads `self.state`, which is already loaded. Requiring `&mut self` for a pure read
would force every caller to hold an exclusive borrow just to check the last selection, which is a
real ergonomic cost (e.g. reading before deciding whether to prompt) for no benefit.

## Consequences

- `Store` instances must be bound `let mut store = Store::open(...)?` wherever
  `set_active_config` is called — this reaches into MVP-21/22, which wire `Store::open` into the
  CLI for the first time.
- **MVP-20's `is_up_to_date` may face the same question** if it ever updates `SolutionRecord.files`
  as a byproduct of checking staleness (e.g. refreshing a cached mtime after a reparse) rather than
  purely reading. That issue should revisit whether it needs `&mut self` too, rather than assuming
  this ADR settled it — recording mtimes and checking staleness are different operations, and
  `is_up_to_date`'s own scope may turn out to be read-only.
- No new `Error`/`ErrorKind` variants: `write_state`'s existing `Error::io` path already covers a
  failed rewrite.
