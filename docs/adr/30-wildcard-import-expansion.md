# ADR 30 — Wildcard `<Import>` Expansion

- **Status:** Accepted
- **Date:** 2026-08-12
- **Issue:** [MVP-41 — Expand wildcard `<Import>` `Project` specs](../todos.md#msbuild-evaluation)
  (`feat/import-globbing`)
- **Affects:** `cli/src/msbuild/imports.rs`, [engineering_spec.md](../engineering_spec.md)
  ("MSBuild Evaluation", "Diagnostics"), [ADR 10](10-import-resolution.md),
  [ADR 29](29-wildcard-item-expansion.md)

## Problem

`<Import>` accepts the same `*`/`?`/`**` wildcards MSBuild items do, and ADR 29 built a real
globber for items without wiring it into import resolution — `imports.rs::Walk::import` still
resolves a `Project` spec as one literal path. That is not a hypothetical gap: the toolset's own
`ImportBefore`/`ImportAfter` extension-hook directories are conventionally consumed as
`$(VCTargetsPath)\ImportBefore\Default\*.props`, gated behind `Condition="Exists(...)"`. Today
that spec resolves to a literal path named `*.props`, which essentially never exists — and if the
guarding `Exists()` happens to be true, evaluation fails outright with `Error::MissingImport` over
a construct that is not user error, the exact "subtly wrong or loudly wrong" tension CLAUDE.md §3
asks this crate to come down on the loud side of, except here the loud failure is itself wrong.

## Decision

`Walk::import` detects `*`/`?` in the macro-expanded `Project` value the same way
`items.rs::specs` already does, and on a match calls `msbuild/glob.rs::expand` — the module ADR 29
built — instead of resolving one literal path. Every file `expand` returns, in its sorted order, is
imported through the existing `Walk::enter`, one call per match, rather than through new traversal
logic.

**A wildcard resolving to zero matches warns, rather than staying silent.** This is a deliberate
departure from ADR 29's item-list precedent (an unmatched item wildcard is silent, matching how a
literal miss was always silent). The precedent does not transfer: for imports, a literal target
that does not exist is already `Error::MissingImport` — fatal — so "wildcard matched nothing" is
not the wildcard equivalent of "silent literal miss", it is the wildcard equivalent of a case this
module has never treated as silent. A `Condition="Exists(...)"`-gated hook directory that resolved
to nothing is a real, actionable signal, and staying quiet about it would trade one class of
surprising failure (the hard error this issue removes) for another (a hook silently not running).

## Discussion

**Reusing `Walk::enter` per match, rather than writing a wildcard-specific import loop, was the
central design choice.** `enter`/`evaluate` already implement MSB4011 dedup (ADR 10), the
recursion-depth cap, and `MSBuildThisFile*` scoping correctly for a sequence of files entered one
after another — which is exactly what N wildcard matches are. A bespoke loop would have to
reimplement all three or risk drifting from them; instead, a wildcard import behaves precisely like
N literal `<Import>` elements written out by hand, evaluated in the same sorted order `glob.rs`
already guarantees. The corollary, exercised in the test suite, is that a cycle reached only
through a wildcard match is caught by the ordinary duplicate rule: the second occurrence of a file
is already in `self.files` regardless of whether it arrived through a literal path or a glob match.

**Warning on zero matches, rather than staying silent for consistency with MVP-37, was chosen after
raising it explicitly** — the issue text called this out as unresolved, since the two obvious
answers (match ADR 29's precedent, or match this module's own missing-import strictness) point in
opposite directions. Matching this module's own strictness won: `imports.rs` has never treated a
target that fails to resolve as unremarkable, and inventing a new silent case here — precisely
where the toolchain extension-hook idiom lives — would undermine the value of fixing the hard
failure this issue exists to remove. The cost is a new warning surface a hook directory with no
files in it will now trigger on every run; accepted, since `Exists()` already gates the import and
an empty directory behind that gate is unusual enough to be worth a line on stderr.

**Semicolon-delimited multi-target `Project` specs were left out of scope.** MSBuild's grammar
permits `<Import Project="a.props;b.props" />` in principle, but `imports.rs` has never split on
`;` for a single `<Import>` element — the existing code always treated the whole attribute as one
path — and the issue does not ask for it. Building that here would be answering a question nobody
raised while fixing a narrower, concretely-motivated one; a spec containing both `;` and a wildcard
is left exactly as unsupported as it was before this change.

## Consequences

- **The `ImportBefore`/`ImportAfter` idiom no longer fails evaluation.** A solution with nothing
  registered in a toolchain hook directory used to hit `Error::MissingImport` whenever the guarding
  `Exists()` happened to be true; now it resolves the wildcard, imports whatever is actually there
  (typically nothing), and warns instead of aborting.
- **Diagnostics table gains a row**: "An `<Import>` wildcard matching no files, skipped" —
  Warning — `msbuild/imports.rs`. See the updated table in engineering_spec.md.
- **Likely intersects MVP-36** (the toolchain unsupported-condition policy): a real
  `ImportBefore`/`ImportAfter` file matched this way is exactly the kind of file whose `Condition`s
  that issue's leniency policy needs to cover. Left for MVP-36 to pick up; nothing here depends on
  it, since a condition still fails the same way regardless of whether the import it guards is a
  literal path or a wildcard.
