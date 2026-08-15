# ADR 40 — Item Definitions Apply Regardless of Item Declaration Order

- **Status:** Accepted
- **Date:** 2026-08-14
- **Issue:** [MVP-45 — `ClCompile` items declared before their `ItemDefinitionGroup` settle with no
  metadata](../todos.md#mvp-milestone) (`fix/item-definition-group-siblings`)
- **Affects:** `cli/src/msbuild/items.rs`, `fixtures/item-definition-order/`,
  `cli/tests/golden/item-definition-order.json`,
  [engineering_spec.md](../engineering_spec.md) ("CLI Binary — Internal Modules"),
  [ADR 12](12-clcompile-flag-extraction.md), MVP-34

## Problem

MVP-45 was filed against a real external solution as: a `ClCompile` `ItemDefinitionGroup` that
shares its element with sibling `<Link>`/`<Lib>` metadata blocks settles into no metadata at all —
`generate` reports success and writes a `compile_commands.json`, but every entry carries only the
toolchain's own system include paths.

That framing turned out to be wrong. Removing the `Link`/`Lib` siblings from the real project's
`ItemDefinitionGroup` changed nothing; the group's condition being false wasn't it either. Binary
search against the real `.vcxproj` isolated the actual shape: its `ClCompile` source-file
`ItemGroup` is declared **before** every `ItemDefinitionGroup` in the file — the reverse of where
Visual Studio conventionally places them, and of every fixture in this repo's corpus. Moving that
one `ItemGroup` to after the `ItemDefinitionGroup`s fixed the output completely, with or without the
`Link`/`Lib` siblings.

`items.rs`'s `settle()` implements a documented, deliberate rule from
[ADR 12](12-clcompile-flag-extraction.md): "an `ItemDefinitionGroup` applies only to items declared
after it," snapshotting the running defaults into each item as it is declared. The doc comment on
that rule called a project relying on the opposite "unusual" and reasoned that gathering definitions
up front would "silently apply settings to files that MSBuild itself compiles without them." A test,
`an_item_definition_group_after_an_item_does_not_affect_it`, asserted exactly that.

The premise was checked directly: opening the real project in Visual Studio and inspecting
`UserApi.cpp`'s effective `Preprocessor Definitions` property for `MT-Retail|x64` shows the
`ItemDefinitionGroup`'s values applied, despite the item preceding the group in the file. Real
MSBuild does not gate item defaults on document order relative to the item that declares them. This
project's own glossary already says as much, just not in `items.rs`'s design: "MSBuild evaluates a
project in ordered phases before running anything: properties and imports first, then item
definitions, then items" (`docs/glossary.md`, "Evaluation pass"). Item definitions are a **complete
pass**, finished before any item is evaluated — not interleaved with items in one left-to-right walk.

## Decision

`items::settle()` becomes two passes over the same `ProjectTree::deferred` stream instead of one:

1. `definitions()` walks the stream, processing only `Element::ItemDefinitions`, evaluating each
   group's and each `ClCompile` definition's `Condition`, and composing them into one final
   `Metadata` — exactly the accumulation `settle()` did before, just isolated into its own pass and
   run to completion before any item is looked at.
2. The existing item-processing loop runs second, reading that finished `Metadata` for every
   `Include`/`Update` rather than a running accumulator that depended on how far through the stream
   the walk had gotten.

Order among `ItemDefinitionGroup`s is unchanged and still matters: a later group's `%(Inherited)`
self-reference still resolves against an earlier group's value, and `holds()`/`apply()` are reused
unchanged. What stops mattering is a group's position **relative to items** — which is the part of
ADR 12's decision #1 this ADR supersedes.

## Discussion

**What stays true from ADR 12, and what doesn't.** ADR 12 bundled two claims into "definition groups
apply forward only": that group-to-group order matters (true, and load-bearing for `%(Inherited)`
composition — [ADR 10](10-import-resolution.md)'s spliced-stream design exists partly to make this
recoverable), and that an item's position relative to a group gates whether the group applies to it
(false). Splitting `settle()` into two passes keeps the first claim and drops the second, without
touching `imports.rs`'s splice, `eval.rs`'s `%()` mechanism, or `flags.rs`'s translation table — none
of which assumed anything about item/definition ordering.

**Why the original assumption held for so long.** Every fixture in this repo's corpus, and every
Visual Studio-authored project either of the ADR's authors had seen at the time, writes item
definitions before the items they configure — VS's project system always generates them that way.
The single-pass model was invisibly wrong rather than loudly wrong: it produced *plausible* output
(a database that generates cleanly, with the right file list) for any file laid out the conventional
way, and only failed silently — no warning, exit code 0 — on the minority shape. That is exactly the
failure mode CLAUDE.md §3 calls worse than a loud one, and exactly why MVP-45's original filer
couldn't tell from the symptom alone which part of the shape was responsible.

**Why two passes over one, rather than reordering the single pass.** A single pass that looked ahead
for definitions before processing an item was considered and rejected: it would require either
buffering the whole stream before processing anything (no simpler than a real second pass) or a
lookahead that reprocesses definitions per item, which is worse than walking the stream twice. Two
explicit passes is the direct expression of "definitions is a phase, then items is a phase," matches
`docs/glossary.md`'s existing (correct) description of MSBuild's evaluation phases, and reuses every
existing per-element helper (`holds`, `apply`, `specs`, `include_item`, `update_item`) unchanged.

**Cost.** `tree.deferred` is walked twice and `tree.properties` is cloned twice instead of once.
Both are cheap next to the XML parsing and toolchain discovery elsewhere in the pipeline, and the
project's fixture corpus and every real solution tried against it are small enough that this was not
measured as a regression worth trading correctness for.

**Test coverage.** `an_item_definition_group_after_an_item_does_not_affect_it` is inverted into
`an_item_definition_group_after_an_item_still_affects_it`, and a new test,
`group_order_still_governs_composition_when_an_item_sits_between_them`, guards the part of the old
behavior that must survive: `%(Inherited)`-style composition between two conflicting groups, with an
item declared between them. A new fixture, `fixtures/item-definition-order/`, reproduces the real
project's shape end to end (source `ItemGroup` before two `ItemDefinitionGroup`s, the second
carrying `ClCompile` alongside `Link`/`Lib` siblings) through the full CLI pipeline, since the unit
tests alone don't exercise `imports.rs`'s splice or `flags.rs`'s translation.

Implementing the two-pass split surfaced a third stale test, missed during planning:
`a_duplicate_declaration_warns_and_keeps_the_first` used two `ItemDefinitionGroup`s bracketing a
duplicate `main.cpp` declaration to prove which value the surviving entry kept — a shape that, under
the two-pass model, composes both groups into one shared default before either declaration is
evaluated, so it no longer distinguishes anything. Rewritten to use per-file metadata overrides
instead, which stayed unique to each `<ClCompile>` declaration regardless of this ADR.

## Consequences

- **MVP-34's real-solution pass has one fewer blocker.** This was found running that pass against a
  real external solution; fixing it is a prerequisite for that checklist to run against output worth
  trusting, alongside MVP-44.
- **`docs/engineering_spec.md`'s description of pass 2 is corrected** to describe the two-pass model
  instead of "an `ItemDefinitionGroup` applies only to items declared after it."
- **ADR 12's decision #1 and its "Definition groups apply forward only" discussion paragraph are
  marked superseded by this ADR**, in place rather than rewritten, per this project's ADR
  conventions. Decisions #2–4 in that ADR (unset `%(Name)` silence, `$` before `%`, item paths
  resolving against the project directory) are untouched and still correct.
- **No change to `imports.rs`, `eval.rs`, `flags.rs`, or `reserved.rs`.** The fix is entirely
  contained to `items.rs::settle()`'s control flow.
- **`Reporter`'s findings no longer accumulate in strict document order across the two element
  kinds.** [ADR 06](06-diagnostic-collection.md) built the append-only collector specifically so
  diagnostics "accumulate in order." That still holds *within* each pass — every
  `ItemDefinitionGroup` finding is in document order relative to other `ItemDefinitionGroup`
  findings, and likewise for `ItemGroup` findings — but a finding from an `ItemGroup` that precedes
  an `ItemDefinitionGroup` in the file now appears *after* that group's findings in the reporter's
  list, since `definitions()` finishes its whole pass before `settle()`'s loop starts. Accepted
  because nothing consumes finding order across element kinds today — `-v` progress and warning
  text both read fine either way — but worth naming here rather than leaving the next reader to
  rediscover it against ADR 06.
