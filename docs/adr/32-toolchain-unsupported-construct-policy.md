# ADR 32 — Downgrading Unsupported Constructs in Toolchain Files

- **Status:** Accepted
- **Date:** 2026-08-12
- **Issue:** [MVP-36 — Unsupported-construct policy for toolchain files](../todos.md) (`fix/unsupported-condition-policy`)
- **Affects:** `cli/src/msbuild/imports.rs`, `cli/src/msbuild.rs`, [engineering_spec.md](../engineering_spec.md)
  ("CLI Binary — Internal Modules"), [ADR 09](09-condition-evaluation-subset.md),
  [ADR 08](08-eager-macro-expansion.md), [ADR 10](10-import-resolution.md),
  [ADR 17](17-toolchain-include-resolution.md)

## Problem

ADR 09 made an unsupported `Condition` a fatal `Error::UnsupportedConstruct`, and named its own
cost explicitly: Microsoft's own `.props`/`.targets` use constructs well outside the supported
subset, so "taken literally, this decision makes the tool fail on every real solution." It handed
the fix to MVP-10, which could not act on it — the signal needed (is this file the user's, or the
toolchain's?) did not exist until MVP-15 supplied a discovered `Toolchain`. MVP-15 has landed.

A real solution surfaced the problem in a shape ADR 09 didn't anticipate. `Microsoft.Cpp.Default.
props` unconditionally imports `Microsoft.Cpp.ToolsetLocation.props`, which builds an
`<Import Project="...">` *path* — not a `Condition` — from
`$([MSBuild]::Unescape($([Microsoft.Build.Utilities.ToolLocationHelper]::FindRootFolderWhereAllFilesExist(...))))`.
`expand()` already handles this gracefully per ADR 08: the construct is left verbatim and recorded
as a warning, not an error. The garbled string is then joined onto a path and checked against the
filesystem by `imports.rs::import`, fails `is_file()`, and surfaces as fatal `Error::MissingImport`
— a second, distinct failure mode from the `Condition` case ADR 09 discussed, caused by the same
underlying gap.

The real file's shape made this harder than it first looked: the unsupported construct is not
written directly inside the `<Import>`'s `Project` attribute. It is assigned to an intermediate
property first (`<_PlatformFolder>$(VCTargetsPath)$([MSBuild]::Unescape(...))</_PlatformFolder>`),
and the `<Import>` references *that* property. Read in isolation, `$(_PlatformFolder)` in the
`Project` attribute is a perfectly ordinary, well-formed reference — `expand()` had no way to know
the value it resolves to was itself built from something unsupported, because ADR 08's eager
expansion stores only the resulting text, not where it came from. A first version of this fix,
tested only against the construct written directly in an `<Import>` path, missed this entirely.

Actually implementing `$([MSBuild]::Unescape(...))` and, worse,
`ToolLocationHelper.FindRootFolderWhereAllFilesExist` — real MSBuild engine logic that probes
candidate roots on disk for a matching file set — was considered and rejected. It's the open-ended
property-function grammar ADR 08 and 09 both deliberately declined, for one call site that
computes nothing sln2cc needs: ADR 17 already resolves toolset include paths and
`$(VCTargetsPath)` directly from the filesystem rather than by evaluating MSBuild's own resolution
logic. `Microsoft.Cpp.ToolsetLocation.props` is on the import graph only because a real `.vcxproj`
pulls it in unconditionally, not because anything downstream consumes what it would have computed.

## Decision

`imports::resolve` gains `toolchain_root: Option<&Path>` — `scope.toolchain.map(|t|
&t.install_path)` at the one call site in `msbuild.rs::evaluate`, `None` wherever no toolchain was
discovered (matching ADR 17's existing `Option<Toolchain>` precedent: `None` preserves prior
behavior exactly). `Walk` carries it and gains:

```rust
fn in_toolchain(&self, file: &Path) -> bool {
    self.toolchain_root.as_deref().is_some_and(|root| file.starts_with(root))
}
```

Two downgrades, both gated on `in_toolchain(file)` where `file` is whichever file *wrote* the
`Condition` or `<Import>` — the parameter every caller in `imports.rs` already threads through:

1. **`Walk::holds`** matches on the `Result` from `eval::condition_holds` instead of using `?`.
   `Err(e)` where `e.kind() == ErrorKind::UnsupportedConstruct` and the site is in-toolchain
   becomes a warning and `Ok(false)`. Every other kind — `RecursionLimit` above all, a
   resource-exhaustion guard rather than a grammar gap — still propagates unconditionally.

2. **`Walk::import`** inspects `expansion.unsupported` (from `eval::expand`) before consuming it
   with `.report(...)`. If non-empty and the referrer is in-toolchain, the import is warned about
   and skipped outright, without ever reaching the `path.is_file()` check. This is keyed on "the
   expansion recorded something unsupported," not on "the resulting path doesn't exist" — a
   toolchain file whose import path resolves *cleanly* and is genuinely missing stays exactly as
   fatal as `MissingImport` already is.

No new `ErrorKind`, no new `Error` variant. Both downgrades intercept before an error would be
constructed, rather than catching and reclassifying one that already carries wire meaning
elsewhere.

**Taint propagation closes the intermediate-property gap.** `Properties` gains a `tainted:
HashSet<String>` of keys whose *stored* value still carries an unsupported construct copied
through verbatim. `assign()` marks a key tainted whenever the `Expansion` it produces has a
non-empty `unsupported` list, and clears it on a later clean reassignment. `expand()`'s property-
substitution branch checks `props.is_tainted(body)` alongside the existing `props.get(body)`
lookup: if the property is tainted, the *reference* (`$(_PlatformFolder)`, not the garbled text
it resolves to) is recorded as unsupported in the new `Expansion`, exactly as if the construct had
been written at this site directly. This makes taint transitive for free — assigning `$(Outer)`
from `$(Inner)\sub` where `Inner` is already tainted taints `Outer` too, because `expand()` records
the reference as unsupported and `assign()` reads that back off the same `Expansion` it always
did. No separate propagation pass, no new field on `Expansion`.

## Discussion

**`false`, not `true`, for a downgraded condition.** ADR 09 refused to guess for a user's project
file because both directions are bad — `false` silently drops a group, `true` applies flags the
file never asked for. Toolchain files change the calculus, not the shape of the trade-off: this
codebase already picks the conservative branch whenever an unknown must resolve to a boolean —
`Exists('$(Undefined)')` is `false`, a zero-match wildcard import is skipped rather than assumed
present. `false` keeps that precedent. The failure mode it accepts is IntelliSense missing a
default Microsoft's own `.props` would have set; the failure mode `true` would accept is clangd
being handed a flag or macro the project never wrote, which is the more actively misleading of the
two per CLAUDE.md §3.

**`install_path`, not `vc_targets_path`, is the boundary.** `Toolchain` carries both. `install_path`
is the broader signal — every file this problem touches (`MSBuild\Microsoft\VC\...`, anything
under `MSBuildExtensionsPath`) already sits beneath it — and "is this under the whole VS
installation" is a simpler, harder-to-get-wrong question than "is this under one specific
subdirectory of it." No real solution's project tree legitimately lives inside `C:\Program
Files\Microsoft Visual Studio\...`, so the coarser boundary costs nothing in precision.

**The import-path downgrade is keyed on "unsupported was recorded," not on "the file is under the
toolchain root."** The alternative — treat every `MissingImport` from a toolchain-rooted file as
non-fatal — was rejected. A cleanly-resolved import path that simply doesn't exist under
`toolchain_root` is a different problem: a broken, relocated, or unexpectedly-shaped VS
installation. ADR 17 already treats that class of problem (`ToolchainIncomplete`) as something
that should fail loudly rather than guess, and a silently-skipped import in a real installation is
exactly the "database that looks right and isn't" CLAUDE.md §3 warns against. Keying the downgrade
on `expansion.unsupported` rather than on provenance alone keeps both intact: a garbled,
untrustworthy path is skipped; a trustworthy path that's simply wrong still fails.

**Warning, not Note.** ADR 13 draws the split as "a Note means nothing was lost." Here something
is: a `PropertyGroup` or an import that a real MSBuild build would have applied is skipped. That
the loss is expected and routine for a toolchain file doesn't change what it is — consistent with
how an unsupported `$(...)` inside an ordinary operand is already a Warning, not a Note.

**Taint tracking stays on `Properties`, not `Metadata`.** `%(Metadata)` inheritance (`eval.rs`'s
other bag) never feeds `<Import>` path resolution — only properties do — so extending taint there
would be speculative scope with no case exercising it, which CLAUDE.md's "don't design for
hypothetical future requirements" already rules out.

**Two failure shapes, one provenance mechanism, one plan.** The `Condition` case is what ADR 09 and
MVP-36's original text anticipated; the import-path case is what the bug report actually hit
first. They share a root cause (toolchain files using constructs outside the subset) and the exact
same signal (`in_toolchain`), so implementing them together kept the change to one coherent policy
rather than two unrelated patches landing months apart under the same issue number.

**No property-function grammar was added.** `$([MSBuild]::Unescape(...))` alone would have been
small. `ToolLocationHelper.FindRootFolderWhereAllFilesExist` would not have been — it's real
MSBuild engine behavior (candidate-root filesystem probing), and building even a narrow slice of
`$([Class]::Method(...))` risks looking like an invitation to keep extending it. Since ADR 17
already makes this file's actual output irrelevant to sln2cc, skipping it entirely is strictly
simpler than partially evaluating it.

**Superseded by [ADR 55](55-static-property-function-whitelist.md).** The premise was wrong: ADR 17
makes the chain's *include paths* irrelevant, but the `.props` it gates are also where every
`ClCompile` default lives (`RuntimeLibrary` from `UseDebugLibraries`, and through it `_DEBUG`).
MVP-49 found a real solution missing `_DEBUG` for exactly this reason and added a closed whitelist
of the three static functions the chain uses. The downgrade policy above is unchanged and still
covers every static call off that list.

## Consequences

- **ADR 09's Consequences section is updated** to note this landed and point here, per CLAUDE.md
  §9 — ADR 09 itself is left otherwise unedited, matching the convention MVP-42/ADR 31 also
  followed.
- **The two open questions ADR 09 left** ("whether a downgraded condition should be treated as
  true or false") is now closed: false. The `Condition`-numeric-comparison open question is
  unaffected and stays open.
- Generation against a real Visual Studio solution no longer fails outright the moment evaluation
  reaches the toolchain's own import chain.
- **New open question:** noise. A solution with many projects imports `Microsoft.Cpp.props` once
  per project, and `Reporter`'s existing dedup only collapses *identical* findings (same file,
  line, message, severity) — distinct construct text at distinct lines across many projects won't
  collapse. Worth revisiting if a real multi-project solution's warning output turns out to be
  dominated by this rather than by genuine findings in the user's own tree.
- **[ADR 42](42-makefile-configuration-missing-import.md) adds a third, narrower downgrade** for a
  case this ADR's mechanism doesn't cover: a toolchain import that resolves cleanly but is genuinely
  absent because sln2cc deliberately doesn't seed `$(MSBuildToolsPath)`, gated on
  `ConfigurationType == Makefile` rather than on `in_toolchain` alone — the "genuinely absent stays
  fatal" guarantee this ADR's Discussion section argues for is preserved, not widened.
