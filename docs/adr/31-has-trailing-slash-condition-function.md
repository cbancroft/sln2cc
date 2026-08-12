# ADR 31 — Adding `HasTrailingSlash()` to the `Condition` Subset

- **Status:** Accepted
- **Date:** 2026-08-12
- **Issue:** [MVP-42 — Support `HasTrailingSlash()` in `Condition` evaluation](../todos.md) (`feat/condition-has-trailing-slash`)
- **Affects:** `cli/src/msbuild/eval.rs`, [engineering_spec.md](../engineering_spec.md)
  ("CLI Binary — Internal Modules"), [ADR 09](09-condition-evaluation-subset.md)

## Problem

ADR 09 scoped `Condition` evaluation to `Exists()` and named `HasTrailingSlash(...)` as one of its
own examples of a condition function that has no truth value here and must therefore be a fatal
`Error::UnsupportedConstruct`. That is a real cost: `HasTrailingSlash` is an ordinary MSBuild
condition function, not an exotic one, and a project file gating a `PropertyGroup` on it currently
fails generation outright rather than producing a compilation database.

The question is narrower than ADR 09's original one. That ADR weighed the *whole* excluded
grammar — numeric comparison, a dozen condition functions, property functions, item transforms,
registry reads — and rightly declined to build all of it speculatively. `HasTrailingSlash` is not
that scope. It takes one operand, like `Exists`, and answers a question about a string already in
hand rather than reaching into the filesystem, the registry, or a wider property-function grammar.

## Decision

`msbuild/eval.rs`'s `primary` production gains a second single-operand condition function:

```
primary := '(' condition ')'
         | 'Exists' '(' operand ')'
         | 'HasTrailingSlash' '(' operand ')'
         | operand ( ('==' | '!=') operand )?
```

`HasTrailingSlash(operand)` expands its operand and returns whether the result ends in `/` or `\`.
Matched case-insensitively, like `Exists` and `And`/`Or`. The two functions now share a
`single_operand` parser (`'(' operand ')'`) that `exists`'s parsing half already needed; only the
per-function check at the end differs.

Everything else ADR 09 named — the `$([MSBuild]::Version*)` family and numeric comparison — stays
out of scope. Those are property functions reached through `$(...)`, not bare condition functions:
supporting them means teaching `expand()` to recognize `$([Class]::Method(args))` generally, which
is the larger addition MVP-36 already inherits as an open question. `HasTrailingSlash` fits in the
existing grammar slot with no new machinery; the version family does not.

## Discussion

**Why extend ADR 09 with a new ADR rather than editing it.** ADR 09's decision — unsupported syntax
is fatal, an unsupported `$(...)` operand is merely recorded — is unchanged; this narrows the
*subset*, not the *policy*. CLAUDE.md's convention is that a superseded ADR stays put with its
status changed rather than being rewritten, and this isn't superseding anything: ADR 09's reasoning
for why the split exists is exactly as true as it was. Editing it in place would also destroy the
"here is a specific thing that used to be fatal and named as an example" record, which is worth
keeping for whoever next asks whether some other function is in scope.

**Why this doesn't touch MVP-36.** MVP-36 asks whether an unsupported condition should stay fatal
once the Visual Studio toolchain's own `.props`/`.targets` are imported — a provenance question,
not a grammar one. `HasTrailingSlash` moving into the supported subset shrinks the set of
conditions MVP-36 will ever see arrive as `UnsupportedConstruct`, but doesn't answer its question:
the toolchain's targets still use the version-comparison family, which is still out of scope.

**Why not implement the version functions at the same time.** They were the more surprising half of
ADR 09's example (`$([MSBuild]::VersionGreaterThanOrEquals(...))`), and it would have been tempting
to do both together since a comparator is small. They were deferred anyway: they are not bare
condition functions but static-method property functions, which means the change belongs in
`expand()`'s handling of `$(...)`, not in `condition_holds`'s grammar — a different code path with
its own design questions (how much of `$([Class]::Method(...))` to recognize, what an unparseable
version string should do given ADR 09's "no truth value, don't guess" rule). Bundling it here would
have made this change about two unrelated things instead of one.

**No trimming.** `Exists` trims its operand before checking emptiness, because `Exists('  ')` and
`Exists('')` mean the same thing — "no path was given." `HasTrailingSlash` does not trim: a value
ending in whitespace after a slash (`'C:\proj\ '`) is a different, stranger case MSBuild does not
special-case either, and inventing a trim policy here that `Exists` didn't need would be exactly
the kind of guess ADR 08 and 09 both avoid elsewhere.

## Consequences

- `msbuild/eval.rs::has_trailing_slash` is a free function, not a method, since it needs neither the
  parser's state nor the file/line `exists` needs for path resolution.
- The `an_unsupported_function_is_an_error` test in `eval.rs` no longer asserts `HasTrailingSlash`
  is fatal; it now asserts a wrong-arity call (`HasTrailingSlash('a', 'b')`) and the still-excluded
  version family instead, so the fatal path still has a producer to test against.
- `docs/todos.md` MVP-42 and this ADR are the record that the excluded-subset list in ADR 09's
  discussion is now stale for `HasTrailingSlash` specifically; ADR 09 itself is left unedited per
  the "status changed, not rewritten" convention.
