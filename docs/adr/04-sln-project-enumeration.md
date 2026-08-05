# ADR 04 — Enumerating Solution Members From a `.sln`

- **Status:** Accepted
- **Date:** 2026-08-03
- **Issue:** [MVP-05 — Enumerate projects from a `.sln`](../todos.md#mvp) (`feat/sln-projects`)
- **Affects:** `cli/src/sln.rs`, [engineering_spec.md](../engineering_spec.md) ("CLI Binary —
  Internal Modules"), MVP-06, MVP-10, MVP-21, MVP-23

## Problem

`sln.rs` held the right types and `todo!("parse .sln")`. Every later item in the pipeline starts
from a list of member `.vcxproj` paths, so this is the first real input the tool has — and four
questions had to be answered before a line of it could be written.

What makes a `Project(...)` entry a C/C++ project? What happens to the entries that aren't — a
solution folder, a C# project, a `.vcxitems` shared-item project? How does a relative,
backslash-spelled location become an absolute path that `clangd` will match against the file
Neovim has open? And can the `projects` half of `Solution` land without the `configurations` half,
which is MVP-06?

The path question is the load-bearing one. `compile_commands.json` entries are matched by path, so
a spelling that disagrees with what the editor sends produces a database that is present, parses,
and silently does nothing.

## Decision

Classification is by project type GUID: `{8BC9CEB8-8B4A-11D0-8D11-00A0C91BC942}`, compared
case-insensitively. Everything else is skipped and reported nowhere — not an error, not a warning.

`parse()` is implemented now as a single line-oriented scan that fills `projects` and returns
`configurations: Vec::new()`, with a comment naming MVP-06 as the issue that fills it.
`SolutionProject` gains a `guid` field, which nothing in MVP-05 reads.

An entry line is read by scanning its `"..."` runs in order and requiring exactly four, rather than
matching the `Project(...) = ..., ..., ...` grammar. Too few fields is
`Error::SolutionParse { line, .. }`. `Project`/`EndProject` pairing is tracked so a truncated file
names the line where the unclosed entry began. A missing file is `Error::SolutionNotFound`; a
non-UTF-8 file is `Error::Io` with `InvalidData`; a UTF-8 BOM is stripped.

Locations are split by hand on `\` and `/`, joined onto the solution's directory, made absolute
with `std::path::absolute`, and normalized lexically. `..` is resolved, not rejected.

## Discussion

**Type GUID rather than the `.vcxproj` extension.** The GUID is what Visual Studio itself
dispatches on, and it is the only signal present for entries that name no file at all — a solution
folder's third field is a display name, not a path. Requiring *both* the GUID and a `.vcxproj`
extension was considered and rejected: the mismatch it would catch is a hand-edited solution, and
there is no defined remediation for it. Reporting the mismatch would need MVP-04's warning channel,
which does not exist yet, and erroring on it would refuse to parse a solution that Visual Studio
itself opens fine. The extension check can be added later if a real solution ever motivates it.

**Skipped entries are silent.** CLAUDE.md §3 says never silently guess, and this is deliberately
not a guess: a solution folder or a C# project alongside a native one is *correct* solution
content, and warning about it would train the user to ignore warnings — which matters, because
MVP-13's unsupported-construct warnings are the ones that must be read. The cost is real and
accepted: a C++ project whose entry carries the wrong type GUID disappears with no explanation. It
surfaces as a missing project in `list-configs`/`generate` output rather than as a diagnostic
pointing at the line, and if that turns out to bite, the fix is a warning once MVP-04 lands.

**Quoted-field scan rather than a strict grammar.** The `(`, `=`, and `,` separators carry no
information a stricter parse would use, while project names legitimately contain commas and
parentheses — splitting on commas would corrupt them. A regex would mean a new dependency for one
call site, which ADR 01's posture rules out. Scanning quotes has one blind spot: a stray quote
inside a field would shift every subsequent field. Visual Studio has no escape for a quote in a
project name, so the malformed input cannot be spelled in the first place, and a field count that
is off then produces an error naming the line rather than a wrong path.

**One pass with `configurations` empty, rather than a separate `parse_projects`.** A `.sln` is
sequential and its `GlobalSection`s are order-dependent, so MVP-06 needs the same scan this issue
writes; giving it its own entry point would mean reading the file twice or deleting this one. The
cost is that `Solution` is momentarily half-true — `configurations` is empty because nobody parsed
it, not because the solution declares none. Nothing reads the field before MVP-06, and MVP-21 is
the first thing that would, so the window is closed before it can mislead.

**`SolutionProject::guid` is captured for MVP-06.** `ProjectConfigurationPlatforms` keys its rows
by project GUID, so the alternative is a second pass over the entry lines later. Carrying an unused
field for one issue is cheaper than parsing the same lines twice, and it is free here — the field
is already parsed to validate the entry's shape.

**`std::path::absolute` rather than `fs::canonicalize`.** `canonicalize` requires the file to
exist, which would make a solution that references a deleted project unparseable rather than
reportable, and on Windows it returns a `\\?\` verbatim path — not something to put in a
compilation database. Lexical normalization has a known limit: two spellings of the same file (a
symlink, a junction, an 8.3 short name) resolve to two different absolute paths, so they would
produce two entries. No solution seen so far does that, and MVP-22's staleness cache keys off the
same paths, so both halves stay consistent even if the spelling is not the shortest one.

**`..` is resolved, not rejected.** Visual Studio writes `..\` whenever a project lives outside the
solution folder, which is ordinary in a repository that keeps `.sln` files in a `build/` or `sln/`
directory. `Error::PathEscape` exists for `<Import>` resolution (MVP-10), where a property sheet
climbing out of the project tree is genuinely suspect; applying it here would reject working
solutions. Resolving rather than passing `..` through is still worth doing — a database full of
`C:\proj\sln\..\app\main.cpp` is legal and unreadable, and path-matched by nobody.

**Splitting the location by hand.** `.sln` always spells separators as `\`, and CI runs the unit
tests on Linux, where `PathBuf::from(r"app\app.vcxproj")` is a single component with a backslash in
its name. Splitting on both separators is what lets the same test assert the same resolved path on
both platforms.

**UTF-8 only.** Visual Studio writes `.sln` as UTF-8 with a BOM, which is stripped. A UTF-16 file
is an error rather than something to sniff and decode: guessing an encoding is exactly the kind of
silent guess that produces a plausible, wrong database. If one ever turns up, the failure names the
file rather than misparsing it.

**Structural validation stops at `Project`/`EndProject` pairing.** The header line
(`Microsoft Visual Studio Solution File, Format Version 12.00`) is not checked, so an arbitrary
text file parses as a solution with zero projects instead of being refused. Pairing is tracked
because it is what makes a truncated file name the line it broke at, which is the case MVP-05's
test list calls out; version validation belongs with MVP-06, which is the issue that actually cares
what schema the `GlobalSection`s follow.

## Consequences

- **MVP-06 adds match arms** to the scan this issue writes, rather than a second parser, and keys
  its per-project mapping off `SolutionProject::guid`. It also inherits the question of whether to
  validate the format-version header.
- **MVP-10 owns path-escape validation.** `Error::PathEscape` and `ErrorKind::PathEscape` stay
  unused until then; that is deliberate, not an oversight.

  **Superseded by [ADR 10](10-import-resolution.md).** MVP-10 found there is no root to escape
  from — a legitimate import is either absolute, into the Visual Studio installation, or
  `..`-relative, into a shared sheets directory — so the validation was never written and both
  types were removed. This bullet is left in place as the record of what was expected.
- **MVP-21 and MVP-22 get absolute, normalized `.vcxproj` paths** to walk, and the same path
  spelling MVP-22's staleness cache will store.
- **MVP-23's fixtures must include** a solution folder and a non-C++ member, since the filter is
  otherwise only covered by inline unit snippets.
- **No changelog entry.** Both subcommands still return `Error::Unimplemented`, so nothing changed
  for someone using the tool; the entry lands with MVP-21.
- **New open question:** whether duplicate project GUIDs or duplicate names within one solution
  need detection. Visual Studio treats a duplicate GUID as a repair-worthy corruption, and MVP-06's
  GUID-keyed mapping is the first thing that would be confused by one. Nothing checks today.
