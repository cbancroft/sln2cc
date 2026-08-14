# ADR 38 — A Whitelisted Subset of Property Functions

- **Status:** Accepted
- **Date:** 2026-08-14
- **Issue:** [MVP-43 — Support a subset of property functions in property references](../todos.md)
  (`feat/property-function-string-methods`)
- **Affects:** `cli/src/msbuild/eval.rs`, [engineering_spec.md](../engineering_spec.md)
  ("CLI Binary — Internal Modules"), [glossary.md](../glossary.md), [ADR 09](09-condition-evaluation-subset.md),
  [ADR 31](31-has-trailing-slash-condition-function.md), [ADR 32](32-toolchain-unsupported-construct-policy.md)

## Problem

Found testing against a real solution: `Condition="$(SolutionName.StartsWith('ContosoUnitTests'))"`
gating a `PropertyGroup`, whose assigned property then gated an `ItemDefinitionGroup`'s preprocessor
definitions. Generation failed with `Error::UnsupportedConstruct`.

This is deliberate, not a bug. `$(Property.Method(...))` is a **property function** — a method call
on an already-expanded property value — and ADR 09 scoped `Condition` evaluation to `Exists()`
only, naming property functions explicitly as out of scope: an expression that cannot reduce to a
boolean has no truth value to hand back, so it errors rather than guessing one (CLAUDE.md §3). ADR
32 reaffirmed the same boundary when MVP-36 needed to relax it for the Visual Studio installation's
own `.props`/`.targets` — but only for files that come from the toolchain. A construct in the
*user's own* project or solution tree, which is what the report hit, stays fatal by design.

The question this ADR answers is the same one ADR 31 answered for `HasTrailingSlash()`: not "how do
we support property functions" (ADR 09 and ADR 32 both rightly declined that as a whole grammar),
but "is there a small, named, real-world subset worth carving out." `StartsWith`/`EndsWith`/
`Contains`/`Equals` are the shape a project file writes to brand a subset of projects — a test
project, a special SKU — the same idiom the bug report's `ContosoUnitTests` gate uses, and they
share a grammar shape simple enough not to invite `ToLocaleUpper`/`Substring`/registry reads/static
`$([Class]::Method(...))` calls along for the ride.

## Decision

`msbuild/eval.rs` gains `property_function(body, props, out) -> Option<String>`, tried inside
`expand()`'s existing "not `is_property_name`" branch — the single place every `$(...)` reference
that isn't a plain property name currently falls through to "left verbatim, unsupported" — before
that fallback:

```rust
fn property_function(body: &str, props: &Properties, out: &mut Expansion) -> Option<String>
```

It recognizes exactly `PropertyName.Method('literal')` for four methods — `StartsWith`, `EndsWith`,
`Contains`, `Equals`, each the single-string-argument overload — and returns `None` for anything
else, which falls through to the existing unsupported/verbatim policy unchanged.

1. **No new `Condition` grammar.** Because every `$(...)` reference — inside a `Condition`
   operand, an ordinary property value, or metadata — already funnels through `expand()`, adding
   the recognition there covers all three call sites for free. `Condition`'s own `boolean()`/
   `comparison()` already treat an expanded `"True"`/`"False"` string correctly; `Parser` needed
   zero changes. This is the opposite of ADR 31's `HasTrailingSlash`, which *is* a new grammar
   production, because a bare condition function has no property to be a method call on.

2. **Method names matched case-sensitively.** Unlike `Exists`/`HasTrailingSlash`, which are this
   evaluator's own keyword grammar and match case-insensitively, a property function's method name
   is real .NET reflection binding — case-sensitive in actual MSBuild, where `startswith` would be
   a `MissingMethodException`. Matching case-insensitively here would silently accept a spelling
   real MSBuild rejects.

3. **Single string-literal argument only.** `Equals(string)`, not the `StringComparison`-taking
   overload; the argument is a plain quoted literal, not itself expanded (`'ContosoUnitTests'`,
   not `'$(Nested)'`). A multi-argument call or a non-literal argument returns `None` — outside the
   recognized shape, exactly like an unrecognized method name — rather than being partially
   evaluated or erroring.

4. **Comparison is ordinal**, via Rust's `str::starts_with`/`ends_with`/`contains`/`==`, not .NET's
   culture-aware default.

5. **An undefined receiver behaves as `""`**, matching ordinary `$(Undefined)` expansion, and is
   recorded through the same `Expansion::record_undefined` every other undefined reference uses —
   not a second, function-specific undefined path.

6. **A tainted receiver (ADR 32) still evaluates, best-effort, but the reference is also recorded
   unsupported** — mirroring `expand()`'s existing behavior for a plain tainted reference.

## Discussion

**Why this needed no `Parser` changes, unlike `HasTrailingSlash`.** ADR 31's addition was a new
`primary` production because `Exists`/`HasTrailingSlash` are condition *functions* — keywords the
tokenizer and parser have to recognize as such. A property function is different: it already
arrives at `Parser::comparison` as an ordinary (unexpanded) operand token, and only becomes
meaningful once `expand()` runs. Hooking `expand()` itself means `Condition="$(Prop.StartsWith('x'))"`,
`Condition="$(Prop.StartsWith('x')) == 'true'"`, and `<Foo>$(Prop.StartsWith('x'))</Foo>` all work
identically, for free, without three separate call sites to keep in sync.

**Why case-sensitive method names but case-insensitive `StartsWith`/etc. comparisons stay possible
if a project wants them.** These are two different case questions and easy to conflate. The method
*name* (`StartsWith` vs `startswith`) is which .NET method gets bound — case-sensitive in real
MSBuild, so this evaluator matches that. The *comparison itself* (whether `"Anti"` matches
`"anti"`) is ordinary `string.StartsWith(string)`, which is case-sensitive by .NET's own default
overload — there is no case-insensitive variant reachable without the `StringComparison` argument
this issue deliberately doesn't support. Both landed case-sensitive, for different, unrelated
reasons, which is worth a test each (`the_method_name_is_case_sensitive`,
`the_match_is_case_sensitive`) so the two do not get merged into one assumption later.

**Why the argument isn't expanded.** MSBuild does expand property-function arguments in general —
`$(Foo.StartsWith($(Bar)))` is legal there. Supporting it here would mean either a recursive call
into `expand()` (which ADR 08 deliberately keeps to one non-recursive pass, precisely to keep cycles
structurally impossible) or a special-cased one-level lookahead that only handles this one nesting
depth and no other. Neither is worth it for a case the bug report doesn't show: a literal argument
covers `StartsWith('ContosoUnitTests')` and every case like it. Left as a documented gap — falls
through to unsupported/verbatim exactly as an unrecognized method would, not silently guessed at.

**Why `Equals`, not just the three "contains-shaped" methods.** `StartsWith`/`EndsWith`/`Contains`
share an obvious family resemblance; `Equals` is a slightly different shape (exact match, not a
substring test) but the same "is this the special-case project" idiom in practice — a project
testing `$(Configuration.Equals('ContosoUnitTests'))` rather than a substring check. Confirmed
with the user rather than assumed: `StartsWith` alone would have covered the reported case, but the
other three share the grammar hook this issue already has to build, so the marginal cost of
including them is one `match` arm each, not new machinery.

**Why not the broader `$([Class]::Method(...))` grammar ADR 32 also declined.** That family is
static-method calls (`$([MSBuild]::Unescape(...))`, `$([System.IO.Path]::Combine(...))`) reached
through a bracketed class name, a materially different and larger surface — ADR 32's own reasoning
("building even a narrow slice... risks looking like an invitation to keep extending it") applies
here without change. This issue's methods are *instance* methods on an ordinary property, a smaller
and more clearly-bounded shape; nothing here reopens that question.

## Consequences

- `$(Property.StartsWith('...'))`/`EndsWith`/`Contains`/`Equals` now work as a `Condition` operand,
  an ordinary property value, or metadata, in the user's own project tree — not just under the
  toolchain-file downgrade ADR 32 added.
- The existing `a_construct_outside_the_supported_subset_is_recorded_and_left_verbatim` test's
  `$(Foo.Substring(0))` case, and `an_unsupported_function_is_an_error`'s
  `$([MSBuild]::VersionGreaterThanOrEquals(...))` case, are both unaffected — neither matches the
  new whitelist, so both still fall through to the pre-existing unsupported/verbatim and
  fatal-condition paths respectively.
- **New open question:** whether property-function arguments should ever be expanded (a nested
  `$(...)` inside the literal). Deferred rather than guessed at — see Discussion above — and left
  for a future issue if a real file needs it.
- **New open question:** whether the method whitelist should grow (`ToLower`/`ToUpper`/`Trim` are
  the next most likely candidates from real-world `.vcxproj` files). Deliberately not designed for
  ahead of a real case, per CLAUDE.md's "don't design for hypothetical future requirements" — add
  the next one when a real file needs it, the same way this issue itself started.
