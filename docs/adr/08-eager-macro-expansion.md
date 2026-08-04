# ADR 08 — Eager `$(Macro)` Expansion and the Property Bag

- **Status:** Accepted
- **Date:** 2026-08-04
- **Issue:** [MVP-08 — Property bag and `$(Macro)` expansion](../todos.md#msbuild-evaluation) (`feat/msbuild-macros`)
- **Affects:** `cli/src/msbuild/eval.rs`, `cli/src/error.rs`,
  [engineering_spec.md](../engineering_spec.md) ("CLI Binary — Internal Modules"),
  [ADR 02](02-structured-error-type.md), [ADR 06](06-diagnostic-collection.md),
  [ADR 07](07-vcxproj-reader-ir.md), MVP-09, MVP-10, MVP-11, MVP-12, MVP-13

## Problem

ADR 07 left every value and every `Condition` in the IR raw, by design. Nothing in the crate could
turn `$(ProjectDir)inc;%(AdditionalIncludeDirectories)` into a path, and four separate pieces of
work need that same primitive: MVP-09 expands both sides of a condition before comparing them,
MVP-10 expands an `Import`'s `Project` attribute before resolving it, MVP-11 seeds the bag, and
MVP-12 expands `ClCompile` metadata.

The question underneath it is **when** a value gets expanded. The obvious reading of "properties
are assigned in order with later definitions shadowing earlier ones, and their values may themselves
reference other properties" is that the bag stores raw text and expansion happens on lookup. That
reading is what MVP-08's own description assumes when it asks for "a recursion depth limit so a
self-referential or cyclic property fails cleanly instead of hanging."

It is also wrong about MSBuild, and the difference is not academic — it changes what a project file
means.

## Decision

**Expansion is eager: a value is expanded at the point of assignment, against the bag as it stood at
that moment, and what is stored contains no `$(` at all.**

`cli/src/msbuild/eval.rs` gains:

- `Properties`, an opaque bag keyed by ASCII-lowercased name. `get` returns `Option<&str>`, where
  `Some("")` and `None` are different answers. `set` stores verbatim; `assign` expands then stores.
- `expand(input, props) -> Result<Expansion>`, a single non-recursive pass.
- `Expansion { value, undefined, unsupported }` — the expanded text plus two deduped, first-
  reference-ordered lists of what could not be resolved. No `Reporter`, no file, no line.

A `$(...)` resolves three ways: a defined property name becomes its value; an undefined one becomes
nothing and lands in `undefined`; anything else — a property function, a registry read, a nested
reference, `$()`, an unterminated `$(` — is copied through **verbatim** and lands in `unsupported`.
`%(Metadata)` and `@(Item)` are untouched; `$` is the only sigil this pass knows.

Runaway growth is bounded by `MAX_VALUE_BYTES` (1 MiB) on the expanded value, raised as the existing
`Error::RecursionLimit`.

## Discussion

**Eager, because that is what MSBuild does — and lazy quietly changes what a file means.** Three
things follow from expanding at assignment, and they are the whole design:

1. `<PreprocessorDefinitions>$(PreprocessorDefinitions);_DEBUG</PreprocessorDefinitions>` — the
   standard append idiom, in every real project — works because `$(PreprocessorDefinitions)` reads
   the *previous* value. It terminates by construction.
2. Redefining `$(A)` never reaches back into a value already built from it.
3. **A cycle cannot be constructed.** Stored values contain no references, so a substituted value
   can never itself need expanding, and `expand` never re-scans what it just wrote.

Lazy expansion was rejected on all three counts. It answers `"2x"` where MSBuild answers `"1x"`, and
it turns the append idiom into an infinite regress that a depth limit would then reject as a cycle —
failing a construct that is not merely legal but ubiquitous. Its one advantage, that a property can
reference something defined later, is not a real one: MSBuild's own pass is sequential, and a
forward reference genuinely does expand to empty there too.

The cost is that `set` and `assign` are not interchangeable and the difference is invisible at the
call site. `assign` exists precisely so the correct order is the shorter thing to write; `set` is
documented as being for MVP-11's literal reserved-property values.

**The depth limit MVP-08 asked for is replaced by a size cap.** With cycles structurally impossible,
there is no depth for a depth limit to count. The hostile-input risk is real but differently shaped:
`<A>$(A)$(A)</A>` doubles the value on every assignment, so forty lines of ordinary-looking XML ask
for roughly a terabyte. That is the CLAUDE.md §5 failure the guard has to stop, and it is a size
problem, not a depth problem.

So MVP-08's "a cyclic reference erroring at the depth limit" test is replaced by two: one asserting
that a self-reference is legal and terminates, and one asserting that runaway doubling is refused.
This is a deliberate departure from the issue text, recorded here because the issue text is the
thing a future reader will reach for first.

**The cap reuses `Error::RecursionLimit` rather than adding a kind.** A new
`ErrorKind::ValueTooLarge` would describe the failure exactly. It was rejected because ADR 02 keeps
kinds coarse and every kind is a permanent string in the plugin contract, and this one buys a
distinction the plugin cannot act on differently: both mean "refuse this project file." Exit code 5
already classifies it. The cost is that the rendered message reads `recursion limit of 1048576
exceeded`, where the number is a byte count rather than a depth — which is why the `context` string
spells out "into a value of more than N bytes" rather than leaving the reader to guess.

**Unsupported constructs are copied through, not emptied.** `$([System.IO.Path]::Combine(...))`,
`$(Registry:...)` and `$($(Inner))` are all outside the supported subset. Treating them like an
undefined property — expanding to nothing — was rejected: it silently drops an include directory,
and the user meets the consequence as a missing header with nothing connecting it back to a
construct this tool declined to evaluate. Left verbatim, the bogus entry in `compile_commands.json`
names its own cause. It is the same argument ADR 07 made for `Element::Unsupported`: record the
fact, do not decide.

A reference *inside* an unsupported construct is part of that construct and is not expanded either.
Expanding it would produce a half-processed expression that is neither what the file says nor
anything MSBuild would produce.

The closing parenthesis is found by a depth-tracking scan rather than `find(')')`, so a property
function is recorded as one whole construct. A naive scan ends
`$([System.IO.Path]::Combine('a','b'))` at the parenthesis closing `Combine(`, which leaves a stray
`)` in the output and quotes half an expression in the diagnostic.

**An unterminated `$(` is recorded, not fatal.** MSBuild fails the build outright. Matching that was
rejected as the wrong trade: sinking an entire generation over one stray `$(` in a property nothing
reads is disproportionate when ADR 06 already built a warning channel that reaches the user. The
text before it still expands.

**`expand` records rather than warns.** It has no `Reporter`, no file, and no line, and it is not
given them. Three reasons: it does not know where the value came from — the caller does; MVP-13 owns
what a warning says, and burying the wording here would scatter it; and a warning per *occurrence*
rather than per *name* would bury a run in noise, since a single include-path value can reference
the same undefined property repeatedly. The two lists are deduped and ordered by first reference so
a caller can emit one finding per distinct thing.

**`Some("")` and `None` are different answers.** Collapsing them would be simpler, and "undefined and
empty both expand to nothing" is true. It was rejected because `Condition="'$(X)' != ''"` is how
every real project tests for a property it expects to be absent, so an undefined-reference warning
that fired on it would fire constantly on correct files — the same "train the user to ignore
warnings" failure ADR 07 avoided by skipping `<Target>` silently. `<PropertyGroup Label="UserMacros"
/>` and `<Empty></Empty>` are ordinary content, and a property assigned nothing is *defined*.

**Names are matched case-insensitively.** MSBuild property names are, and `$(configuration)` landing
in a different slot than `$(Configuration)` surfaces as an empty include path — a database that
looks fine and is wrong. Lowercasing the key is the same choice `items_of_type` already made with
`eq_ignore_ascii_case`. The cost is that the bag cannot report a property's name as the file spelled
it; nothing needs that yet, and the reference site has the spelling when a diagnostic wants it.

**The bag stays minimal.** No `iter`, no `len`, and no record of which file and line defined each
property. The last is tempting — "`$(X)` was last set in `a.props:12`" is a good diagnostic — but
nothing needs it yet, and ADR 07's restraint about speculative structure applies equally here.

## Consequences

- **MVP-09 expands both sides of a comparison through `expand`** before evaluating it, and inherits
  `Expansion::unsupported` for the condition-function cases it does not support.
- **MVP-10 expands an `Import`'s `Project` attribute** against the bag as it stands at that point in
  the file, which is what makes import resolution interleave with expansion rather than run as a
  pre-pass. It uses `set` for nothing and `assign` for every `PropertyGroup` child it walks.
- **MVP-11 seeds reserved properties with `set`**, not `assign`: their values are literal paths with
  nothing to expand.
- **MVP-12 expands `ClCompile` metadata** and must leave `%(...)` intact through this pass, since
  inheritance is resolved there and not here.
- **MVP-13 consumes `Expansion::undefined` and `Expansion::unsupported`** and owns every word of the
  resulting warnings, plus the decision of whether an undefined reference inside a condition is
  worth reporting at all.
- **`Error::RecursionLimit` now covers a size cap as well as a depth cap**, and its `limit` field is
  no longer always a depth.
- **New open question:** the size cap is per-value and there is no cap on the *number* of
  properties. A project file with a million distinct `PropertyGroup` children is bounded only by
  MVP-35's document-level guard, which counts nesting rather than breadth.
- **New open question:** MSBuild's `%XX` escapes (`%3B` for a literal `;`, `%24` for a literal `$`)
  are not decoded anywhere. Visual Studio writes them when a value contains a character that would
  otherwise be a list separator, so a semicolon-bearing include path is currently mis-split
  downstream. Whether MVP-12 decodes them or this module does is unsettled.
