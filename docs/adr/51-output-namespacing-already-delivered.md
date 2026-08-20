# ADR 51 — Output Namespacing Was Already Delivered

- **Status:** Accepted
- **Date:** 2026-08-18
- **Issue:** [V1-02 — Namespace output per solution](../todos.md#v1) (`feat/output-namespacing`)
- **Affects:** `cli/tests/generate.rs`, [engineering_spec.md](../engineering_spec.md) ("JSON
  envelope"), [ADR 39](39-nvim-checkhealth.md), [ADR 19](19-hashed-storage-directory-write.md),
  [ADR 20](20-state-file-shape.md)

## Problem

`docs/todos.md` scoped V1-02 as: "Write to `.sln2cc/<solution-name>/` so a previously-used
solution's artifacts survive switching away and back." Read at face value, this describes new
work — output would need to move from some single, shared location into a per-solution one.

That move already happened. `compdb::resolve_output_dir`/`output_dir_name` (`cli/src/compdb.rs`)
have written every solution's `compile_commands.json` under `.sln2cc/<solution-stem>-<hash>/`
since MVP-17 (`feat/compdb-write`, ADR 19), with a `.source` collision marker that fails loudly
(`Error::StorageCollision`) rather than silently sharing or overwriting a different solution's
directory. `db::Store` (`cli/src/db.rs`) has kept `state.json` keyed per-solution path, root-
relative, since MVP-18 (ADR 20) — `two_solutions_hold_independent_selections` already proves one
solution's active configuration can't disturb another's. Both predate V1-02 as a todo item by
over a week. A previously-used solution's artifacts already survive switching away and back,
because that is exactly the property `resolve_output_dir`'s directory-per-solution scheme and
`.source` marker exist to provide.

This is the same shape V1-04 and V1-06 hit: a todo item describing behavior a prior issue had
already delivered as a matter of course. It's closest to V1-06's variant — V1-04's own **Tests:**
line could point at specs that already existed verbatim, but V1-02's could not. Its line reads:
"two solutions generating into distinct directories without clobbering each other." Before this
issue, that property had unit coverage on both halves in isolation
(`compdb.rs::output_dir_name_differs_for_different_source_directories`,
`compdb.rs::resolve_output_dir_rejects_a_genuine_collision`,
`db.rs::two_solutions_hold_independent_selections`) but nothing exercised it through the real
`generate` pipeline with two actual solutions sharing one `.sln2cc` root — the shape a genuine
multi-solution workspace has, and the only way to prove the collision-avoidance path is actually
wired into `generate` end to end rather than correct only in isolation.

Two existing cross-references also turned out to be stale. `engineering_spec.md`'s JSON-envelope
section and ADR 39 both said "namespacing is per-solution, not per-configuration (V1-02 owns the
latter)" — but neither the todo item's title ("Namespace output per solution") nor its body ever
described a per-configuration scheme; nothing anywhere else tracks per-configuration namespacing
as planned work. The phrasing was aspirational leftover from before "Storage Layout" settled on
the current scheme, not a second scope V1-02 was ever meant to cover.

## Decision

No `cli/src/**` or `nvim/lua/**` change. Close V1-02 by adding the one test that was genuinely
missing: `cli/tests/generate.rs` gains `two_solutions_generate_into_distinct_directories`, which
runs two independent `TempProject` trees through `generate` with a shared `--root`, and asserts
their resulting database paths differ, both live under the shared root, and each one's contents
name only its own project's directory — the failure mode a caller-must-share-one-namespace design
would have produced (one project's compile database silently overwritten by the other's run),
which is what makes the test meaningful despite pinning down behavior that already existed.

The two stale "V1-02 owns per-configuration namespacing" references in `engineering_spec.md` and
ADR 39 are corrected to state plainly that per-configuration namespacing was never scoped as
separate work and isn't tracked anywhere, rather than pointing a future reader at this issue for
something it was never going to deliver.

## Discussion

**Why MVP-17/MVP-18 already built this, rather than it being incidental.** "Storage Layout" in
`engineering_spec.md` designed the hashed-directory-per-solution scheme and the root-relative,
per-solution `state.json` keys before any code existed; MVP-17 and MVP-18 implemented that design
directly. Nothing in the codebase's history went through a single-shared-output phase that later
needed migrating away from — the per-solution property was there from the first working
`generate` pipeline.

**Why this counts as a distinct issue from V1-04/V1-06 rather than folding into either.** V1-04
closed with zero new tests. V1-06 needed one genuine integration-style addition. V1-02 also needed
exactly one addition, but at a different layer (CLI integration test, not plugin) and proving a
different property (cross-solution isolation through the real pipeline, not lazy discovery).
Recording it under its own ADR keeps the todo-to-ADR-to-test chain traceable to the issue that
actually motivated each test, per V1-04's own ADR precedent (37) and V1-06's (47).

**Why not treat "no code change needed" as reason to skip writing an ADR at all.** CLAUDE.md
requires an ADR when a design question is settled, not only when new code lands. This issue
settles two things on the record: that output namespacing was already correct before V1-02
existed, and that the "V1-02 owns per-configuration namespacing" phrasing elsewhere in the docs
was never an accurate scope statement — both are exactly the kind of relitigation-prevention an
ADR exists for.

**Cost.** None. There was no design trade-off to weigh; the only material changes are one new
integration test and two corrected cross-references.

## Consequences

- No behavior changes for users; no `docs/changelog.md` entry, matching V1-04's and V1-06's own
  precedent of skipping the changelog for a no-op close.
- `cli/tests/generate.rs` now has an end-to-end case guarding cross-solution output isolation
  directly, rather than relying on it being an emergent consequence of `compdb.rs`'s and
  `db.rs`'s isolated unit tests plus code reading.
- `engineering_spec.md` and ADR 39 no longer point a future reader at V1-02 for per-configuration
  namespacing; if that work is ever wanted, it needs its own todo item first; none is filed today.
