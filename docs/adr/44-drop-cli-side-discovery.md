# ADR 44 — Discovery Stays a Plugin Responsibility

- **Status:** Accepted
- **Date:** 2026-08-15
- **Issue:** [V1-01 — `discover` subcommand](../todos.md#v1) (rejected)
- **Affects:** `cli/src/cli.rs`, [ADR 03](03-cli-surface-and-json-contract.md) (§"discover
  deliberately absent", not edited — see Discussion), [ADR 34](34-nvim-cli-invocation.md)
  (Consequences, not edited — see Discussion)

## Problem

ADR 03 left `discover` out of the CLI surface for MVP but treated its eventual existence as
settled — reserving an unused `DiscoveredSolution { path, name }` type in `cli.rs` and pointing
at "V1-01" as where it would be built. `docs/todos.md` carried that forward as a scheduled V1
item: move `.sln` scanning behind the CLI so the ignore-list/depth-cap rule lives in one place
instead of being duplicated between `discovery.lua` and a future `discover.rs`.

Revisiting the premise: the CLI's job is compiling a `compile_commands.json` for a solution it is
handed a path to. Locating that solution in the first place — walking a project tree, deciding
what counts as noise, remembering what was found — is a concern about the *caller's* environment
(a Neovim session, its project root, its own notion of what to ignore), not about MSBuild
evaluation. `discovery.lua` (MVP-27, [ADR 33](33-nvim-solution-discovery.md)) already does this
scan and does it entirely client-side, with no CLI round-trip needed. Moving it into the CLI
would not remove a duplicated rule so much as relocate ownership of a plugin concern into the
binary that has no other reason to touch the filesystem outside a solution it was already told
about.

## Decision

**The CLI will not gain a `discover` subcommand, ever, not just for MVP.** Solution discovery —
finding `.sln` files, choosing among them, remembering a choice — is entirely
`nvim/lua/msvclsp/discovery.lua`'s and later V1 picker work's responsibility. The CLI's contract
stays exactly `list-configs <solution>` / `generate <solution> ...`: it is always handed a
solution path, never asked to go find one.

`cli.rs`'s unused `DiscoveredSolution` type is removed, along with the code comment on `Command`
that named V1-01 as where `discover` would land.

`docs/todos.md`'s V1-01 entry is marked rejected rather than deleted, so the reasoning has
somewhere to live rather than silently vanishing from the issue list.

## Discussion

**Why this wasn't caught at ADR 03's time.** ADR 03 reserved the type and the issue number
because *something* — `cli.lua`'s already-stubbed `M.discover`, the reserved `DiscoveredSolution`
— suggested CLI-side discovery was the plan, and nothing at that point had examined whether it
should be. ADR 33, three weeks later, built the actual scan entirely in Lua and noted `cli.lua`'s
`M.discover` stub as dead code "predates ADR 03's decision that the CLI's discover subcommand is
deliberately absent for MVP" — i.e., even ADR 33 read V1-01 as still eventually happening, just
not yet. Nothing forced a second look at whether it should happen at all until this issue was
about to be started.

**Why this ADR doesn't edit ADR 03 or ADR 34.** Both contain sentences that describe a plan this
ADR reverses (ADR 03: "[`discover`] is V1-01... adding a subcommand later is purely additive";
ADR 34: "`DiscoveredSolution` itself stays in `cli.rs` as a V1-01 placeholder"). Editing them to
match the new decision would erase what was actually believed and decided at the time, which is
exactly what an ADR exists to preserve — ADR 33 set the precedent for this when it closed ADR
28's open question without touching ADR 28's own text. This ADR is the correction; the two it
corrects stay as accurate records of what was true when they were written.

**Was this actually additive-and-harmless to keep as "maybe later," rather than rejecting it
outright?** Considered, since ADR 03 explicitly praised deferring `discover` as low-cost
("purely additive and breaks no existing invocation"). Rejected: that argument was about *timing*
(build it now vs. later), not about *whether the CLI should own this at all*. Leaving V1-01 open
"in case" keeps a wrong architectural boundary on the books indefinitely and lets the next person
implement it on the strength of the old issue text alone, which is exactly what almost happened
here.

## Consequences

- `docs/todos.md`'s V1-01 is struck through and marked rejected rather than removed, so the
  decision has a place to attach to per CLAUDE.md §9's "always reference an item in todos.md."
- `cli.rs` loses `DiscoveredSolution` (unused — zero call sites beyond its own definition) and
  the `Command` doc comment naming V1-01.
- `docs/engineering_spec.md`'s "CLI Surface" section needed no change: it never documented
  `discover`, since MVP-03 never spelled it out beyond the one dismissive line this ADR now
  supersedes.
- No `docs/changelog.md` entry: nothing shipped, and removing unused code is purely internal per
  CLAUDE.md §9.
- Any future work on solution discovery (a picker, persistence, rescanning) is exclusively
  `nvim/lua/msvclsp/discovery.lua` and its neighbors' territory — V1-03 through V1-06 in
  `docs/todos.md` already assumed this and need no change.
