# ADR 05 — Solution Configurations and the Per-Project Mapping

- **Status:** Accepted
- **Date:** 2026-08-03
- **Issue:** [MVP-06 — Enumerate solution configurations and platforms](../todos.md#solution-parsing)
  (`feat/sln-configurations`)
- **Affects:** `cli/src/sln.rs`, [engineering_spec.md](../engineering_spec.md) ("CLI Binary —
  Internal Modules"), [ADR 04](04-sln-project-enumeration.md), MVP-04, MVP-21, MVP-22

## Problem

ADR 04 landed `Solution::projects` and deliberately returned `configurations: Vec::new()`, naming
this issue as the one that fills it. Until it does, `list-configs` has nothing to list and `generate`
has no way to know which pair to hand `msbuild::evaluate` for a given member.

The work is not "collect a list of strings". A solution declares its own configuration/platform pairs
in `SolutionConfigurationPlatforms`, then maps each one onto a *possibly different* pair per project
in `ProjectConfigurationPlatforms` — a solution-level `Debug|x64` legitimately means `Debug|Win32`
for one member. The same section records which projects are unchecked in Configuration Manager for a
given configuration, which generation has to be able to skip. Getting the mapping wrong is the worst
class of bug this tool has: the database is present, it parses, and every flag in it belongs to a
configuration the user did not ask for.

Three narrower questions came with it. Row keys are `{GUID}.<pair>.<Property>`, and configuration
names may contain a dot, so how is a key taken apart? ADR 04 explicitly deferred the format-version
header to this issue. And ADR 04's closing open question — whether duplicate project GUIDs need
detecting — named this issue's GUID-keyed mapping as the first thing a duplicate would confuse.

## Decision

`parse_text` gains `GlobalSection` tracking in the same single line-oriented scan, as ADR 04
anticipated. `Solution::configurations` is filled from the **key** side of each
`SolutionConfigurationPlatforms` row, deduplicated, in declaration order. `SolutionProject` gains
`configurations: Vec<ProjectConfigMapping>`, where a mapping carries the `solution` pair, the
`project` pair it resolves to, and a `build` flag. `SolutionProject::config_for` looks a mapping up
by solution pair; `None` means the project takes no part in that configuration at all.

Row keys are taken apart by stripping a known property (`.ActiveCfg`, `.Build.0`) off the **right**,
never by splitting on `.`. `Build.0`'s *presence* sets `build`; its value is not read. Rows are
collected into a temporary vector during the scan and distributed onto projects afterwards.

A duplicate project GUID — across all entry types, not just C++ ones — is
`Error::SolutionParse` naming both lines. The format-version header is **warned** about, not
enforced: `Solution` gains a `warnings: Vec<Diagnostic>` field, which is also new territory, since
MVP-06 is the first issue to produce a diagnostic at all.

## Discussion

**Properties are stripped as a suffix, not split on `.`.** A row key is
`{A1}.Debug|x64.ActiveCfg`, and the obvious parse — split the key on `.` and take three parts — is
wrong, because MSBuild configuration names may contain a dot. A solution with a `Debug With
Asserts.2` configuration would have every row silently misread, which is exactly the failure this
issue must not have. Stripping a known property off the right leaves the pair intact whatever it
contains. The blind spot is a configuration name that itself ends in `.ActiveCfg` or `.Build.0`,
which is not a name anyone writes, and the alternative — using the already-parsed
`SolutionConfigurationPlatforms` list to match the pair by prefix — would make the parse depend on
sections appearing in `preSolution`-before-`postSolution` order for no additional coverage.

**`.Deploy.0` and unrecognized properties are skipped, not reported.** Deploy is a ClickOnce/smart
device concept with no meaning for a native project, and the mapping is *fully* described by
`ActiveCfg` plus `Build.0`. A property this parser cannot name is one it could not act on either, so
there is nothing for a warning to tell the user to do. This is a different axis from a malformed
row: an unknown property on a well-formed row is skipped, while a row that is not `key = value`, or
whose pair has no `|`, or whose key does not start with a GUID, is an error naming the line. The
split matters because dropping a *configuration* silently is how a wrong database starts, whereas
dropping a property that carries no configuration information costs nothing.

**Rows are collected, then distributed.** Attaching each row to its project as it is read would make
the parse depend on `Global` following every `Project(...)` entry. Visual Studio always writes it
that way, but the cost of not depending on it is one small vector and one pass, and the failure mode
if the assumption ever broke is silent: a row that finds no project is indistinguishable from a row
belonging to a C# member, which is dropped by design. Distributing afterwards removes the ambiguity
rather than documenting it.

**The mapping lives on `SolutionProject`, not in a solution-level table.** MVP-22's loop is "for each
member project, get the pair to evaluate it with", so grouping at parse time removes a GUID join from
every use site, and rows belonging to entries the type-GUID filter already excluded fall away instead
of being carried. The alternative — a flat `Vec<ProjectConfigRow>` on `Solution` — would have left
`SolutionProject` and every existing MVP-05 test literal untouched, which was its only real
advantage. The cost accepted here is a name collision at two levels: `solution.configurations` is a
list of declared pairs, `project.configurations` is a list of mappings. The types differ, and
`config_for` is what call sites actually use, so the collision is tolerable.

**Exclusion is two distinct states, and both are preserved.** A project with `ActiveCfg` but no
`Build.0` is unchecked in Configuration Manager; a project with no rows at all takes no part in the
configuration. Collapsing them — dropping unbuilt projects at parse time — was rejected: the mapping
is real information, and whether an unbuilt project's files are still worth indexing for clangd is
MVP-22's call to make, not the parser's. `config_for` returning `None` versus returning a mapping
with `build: false` keeps the two answerable separately.

**The format version warns rather than being enforced or ignored.** Refusing anything that is not
`12.00` would reject solutions Visual Studio itself opens: `12.00` spans VS2013 through VS2022, and
the `GlobalSection` layout read here has been stable since VS2010, so an unfamiliar version is far
more likely to still parse correctly than not. Ignoring it entirely was the other candidate, and it
was rejected because ADR 04 left a real gap — with no header check, an arbitrary text file parses as
a solution with zero projects and reports nothing, which reads to the user as "the tool found no
projects" rather than "that is not a solution". So an absent header warns too, with no line to name.
The cost is that MVP-06 has to put the diagnostic *somewhere* before MVP-04 exists: the shape was
already settled by MVP-03 (`cli::Diagnostic`, `Envelope::with_warnings`), so `Solution::warnings`
carries it and MVP-04 drains it. Deferring the check to MVP-04 instead was considered and rejected —
the header question was assigned here, and answering it in a field MVP-04 has to consume anyway is
cheaper than reopening `sln.rs` later.

**A duplicate project GUID is an error, which closes ADR 04's open question.** With rows keyed by
GUID, two members sharing one means the second gets no mapping — so it looks like a project excluded
from every configuration and is silently skipped during generation. That is the "plausible nonsense"
CLAUDE.md §3 exists to prevent, and it is invisible: the user sees a database that is missing a
project's files with no diagnostic pointing anywhere. Visual Studio treats a duplicate GUID as
repair-worthy corruption, so refusing to parse matches what the ecosystem already considers broken.
GUIDs are tracked for *every* entry rather than just C++ ones, because a C# project sharing a C++
project's GUID would attach its rows to the C++ member — a wrong mapping, which is worse than a
missing one. Two lighter options were rejected: ignoring it (the silent-skip failure above), and
applying rows to every match (which papers over corruption and assumes the two members want the
same configuration).

**`Global` and `EndGlobal` are recognized explicitly.** They carry nothing, but without an arm for
them a section missing its `EndGlobalSection` gets reported as a malformed row at the `EndGlobal`
line, several lines past where the file actually broke. Recognizing them lets the error name the line
the section opened at, matching what ADR 04 established for `Project`/`EndProject` pairing. Section
pairing is tracked for the same reason.

**Names stay opaque and lookup is exact.** Solutions define custom configuration names and platforms
contain spaces (`Any CPU`), so nothing is normalized past trimming. Lookup compares exactly, so a
case-mismatched request from the user surfaces as `Error::InvalidConfiguration`, which lists the
valid pairs — a better answer than resolving to something they did not name. Section names are
likewise matched exactly; an unrecognized one becomes an ignored section, so a hand-mangled
`SolutionConfigurationPlatforms` yields an empty list and a visible "(none declared)" rather than a
wrong one.

## Consequences

- **MVP-04 has its first producer.** `Solution::warnings` must be drained into the envelope's
  `warnings` array. If MVP-04 introduces a shared accumulator threaded through evaluation, this
  field is what it replaces.
- **MVP-21 emits `Solution::configurations` directly** as `SolutionConfigs::available`, and gets the
  `InvalidConfiguration` list for free.
- **MVP-22 iterates `SolutionProject::config_for`**, evaluates `mapping.project` rather than the pair
  the user selected, and decides what to do with `build: false` members.
- **ADR 04's duplicate-GUID open question is resolved** — as an error. ADR 04 stays as written.
- **No changelog entry.** Both subcommands still return `Error::Unimplemented`, so nothing changed
  for someone using the tool; the entry lands with MVP-21.
- **MVP-24's `multi-config` fixture must include** a project whose pair differs from the solution's
  and a project excluded from one configuration, since both are otherwise covered only by inline
  unit snippets.
- **New open question:** whether a solution configuration that no project takes part in — declared,
  but with no `ActiveCfg` row anywhere — should be hidden from the picker or offered and then fail
  during generation. Nothing filters today.
- **New open question:** whether an empty `configurations` list deserves a warning of its own. It is
  currently indistinguishable from a solution whose `SolutionConfigurationPlatforms` section this
  parser failed to recognize.
