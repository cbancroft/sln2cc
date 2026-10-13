# ADR 56 — Metadata Conditions Read the Bag Being Built

- **Status:** Accepted
- **Date:** 2026-10-13
- **Issue:** [MVP-50 — Evaluate `%(Metadata)` references in metadata `Condition`s](../todos.md)
  (`fix/metadata-conditions`)
- **Affects:** `cli/src/msbuild/eval.rs`, `cli/src/msbuild/items.rs`,
  [engineering_spec.md](../engineering_spec.md) ("CLI Binary — Internal Modules"),
  [ADR 09](09-condition-evaluation-subset.md), [ADR 12](12-clcompile-flag-extraction.md),
  [ADR 55](55-static-property-function-whitelist.md)

## Problem

ADR 55 made Microsoft's toolset `.props` load on a real installation. The defaults inside them still
did not apply, because `Microsoft.Cl.Common.props` writes every one of them behind a guard on the
item's own metadata:

```xml
<ItemDefinitionGroup Condition="'$(UseDebugLibraries)' == 'true'">
  <ClCompile>
    <RuntimeLibrary Condition="'%(ClCompile.RuntimeLibrary)' == ''">MultiThreadedDebugDll</RuntimeLibrary>
```

"Default only if nothing earlier chose one." Two gaps stopped it:

1. **`Condition` evaluation did not read metadata.** `Parser::operand` ran `expand()`, which handles
   `$(…)` only and leaves `%(…)` untouched without recording it. The operand stayed the literal
   `%(ClCompile.RuntimeLibrary)`, never equalled `''`, and the default was skipped — silently.
   No warning, no error, a plausible database with `/MDd` (and so `_DEBUG`) missing from every Debug
   entry. That is the outcome CLAUDE.md §3 ranks worst.
2. **The qualified form was rejected.** ADR 12 recorded `%(ClCompile.X)` as unsupported, using
   `%(ClCompile.Identity)` as its example. The toolset uses the qualified form throughout — 143
   `ClCompile.` and 121 `Link.` references in the v170 `.props` — always naming the enclosing item
   type.

## Decision

1. **A metadata setting's own `Condition` reads the bag being built, live.** `eval.rs` gains
   `metadata_condition_holds(condition, props, metadata, file, line)` beside `condition_holds`; the
   parser carries an optional `&Metadata`, and each operand expands `$(…)` then `%(…)` — the order
   `Metadata::assign` already uses, for ADR 12's reason. `items.rs::apply` uses it for every
   metadata child: under an `ItemDefinitionGroup`'s `<ClCompile>` and under a
   `<ClCompile Include=…>`. The bag is the one being written, so a setting sees earlier siblings and
   everything earlier definition groups assigned.
2. **Group- and item-level conditions stay property-only.** The `Condition` on an
   `ItemDefinitionGroup`, an `ItemGroup`, a `<ClCompile>` definition or a `<ClCompile Include=…>`
   item goes through `condition_holds` unchanged.
3. **`Metadata` knows its item type.** `Metadata::for_item("ClCompile")` replaces `Metadata::new()`.
   `expand_metadata` resolves `%(Type.Name)` only when `Type` matches the bag's type
   case-insensitively, as MSBuild matches item types; any other qualifier stays unsupported and is
   left verbatim. `items.rs` names `ClCompile` once.
4. **Qualified well-known metadata stays unsupported.** `%(ClCompile.Identity)`, `FullPath`,
   `Filename` and the rest of MSBuild's fourteen well-known names are never seeded, so they are
   recorded and copied through rather than resolving to `""`.
5. **Unsupported metadata in an operand is a finding, not an error** — recorded, left verbatim, and
   the comparison still made, as ADR 09 already treats an unsupported `$(…)`.

## Discussion

**Live, not a snapshot.** MSBuild evaluates a definition's children in order against the
accumulating definition, so a guard sees what a sibling above it just assigned — and
`Cl.Common.props` depends on that (`DebugInformationFormatDefined` is set from a guard on
`DebugInformationFormat`, and `Platform.Common.props` keys a later default off the result). It is
also what `items.rs::apply`'s own comment already promised for values: "a later sibling sees what an
earlier assigned." A snapshot taken when each definition starts — what the Python reimplementation
did for target item groups — would make a value and a condition on adjacent lines see different
bags.

**Why only metadata settings.** Outside a target, MSBuild gives group- and item-level conditions no
item to read metadata from. Threading a bag into them would mean choosing one to read — the empty
defaults, or some item's — and either choice would be invented. Leaving them on `condition_holds`
keeps them exactly as they behaved before, including the existing silent treatment of a stray `%(`
there (a construct real MSBuild rejects anyway).

**Why the item type lives on `Metadata`.** Three options were weighed with the user. Accepting any
qualifier, as the Python version does, would resolve `%(Link.TargetMachine)` against `ClCompile`
metadata and hand clangd a linker value as a compiler one. Hard-coding `ClCompile.` in
`expand_metadata` would work today but put `items.rs`'s policy — only `ClCompile` is settled —
inside `eval.rs`'s mechanism, and quietly become wrong the day another item type is evaluated.
Storing the type on the bag keeps `eval.rs` generic at the cost of about ten call-site changes,
nearly all in tests. There is deliberately no `Default`: an untyped bag would have to accept or
reject every qualifier, and which would be a guess.

**Why qualified well-known names stay unsupported, and the asymmetry that leaves.** ADR 12's whole
reason for recording `%(ClCompile.Identity)` was that a flag containing it names its own cause,
where an empty one leaves a missing include path with nothing to trace it to. Resolving the
qualified form generally would have undone that for exactly the names nothing seeds. The unqualified
`%(Filename)` already expanded to `""` silently before this change, by the "unset is silent" rule.
Making it unsupported too would be consistent, but it would add warnings to any project writing
`$(IntDir)%(Filename).obj`, which is a behaviour change outside this issue. It is recorded as an
open question instead. No well-known name appears in a `ClCompile` definition on the real v170
chain, so neither choice affects the defaults this issue exists to apply.

**Injection.** Operands are still expanded after tokenizing (ADR 09), so a metadata value containing
`' or '` is one operand's text, not syntax. Tested directly, since metadata values come from the
same untrusted files properties do.

## Consequences

- **Debug configurations on a real installation now get `/MDd`, and clangd sees `_DEBUG`** — the
  bug reported against ADR 55 is fixed end to end, along with every other guarded `ClCompile`
  default in `Microsoft.Cl.Common.props` that `flags.rs` translates.
- **A project or sheet that sets a value explicitly keeps it.** One set *after* `Microsoft.Cpp.props`
  overrides the default as before; one set *before* it (a sheet imported early) is now kept by the
  guard instead of being lost.
- **ADR 12's "qualified references are recorded"** is marked in place: true now only for another
  item type's qualifier and for well-known names.
- **`Metadata::new()` is gone**; every bag is created for a named item type.
- **New open question:** whether unqualified well-known metadata (`%(Filename)`, `%(Identity)`)
  should be recorded unsupported rather than silently empty, now that its qualified spelling is.
