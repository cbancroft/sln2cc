# ADR 55 — A Whitelist of Static Property Functions for the Toolset Location Chain

- **Status:** Accepted
- **Date:** 2026-10-13
- **Issue:** [MVP-49 — Evaluate the static property functions the toolset location chain uses](../todos.md)
  (`fix/static-property-functions`)
- **Affects:** `cli/src/msbuild/eval.rs`, [engineering_spec.md](../engineering_spec.md)
  ("CLI Binary — Internal Modules"), [ADR 08](08-eager-macro-expansion.md),
  [ADR 32](32-toolchain-unsupported-construct-policy.md), [ADR 38](38-property-function-string-methods.md)

## Problem

Reported against a real solution: Debug entries in the generated database are missing `_DEBUG`.
Nothing in a Visual Studio–generated project writes it. The project sets
`<UseDebugLibraries>true</UseDebugLibraries>`; Microsoft's toolset `.props` turn that into
`RuntimeLibrary=MultiThreadedDebugDLL`; `flags.rs` turns that into `/MDd`; and clang-cl defines
`_DEBUG` when it sees `/MDd`. sln2cc never got past the first step.

Traced against a real VS 17 `v170` toolset, the chain that carries those defaults is:

```
Microsoft.Cpp.props
  _VCTargetsPathForToolset = $([MSBuild]::Unescape($([Microsoft.Build.Utilities.ToolLocationHelper]
      ::FindRootFolderWhereAllFilesExist($(_VCTargetsPathFolders), $(_RelativeToolsetFiles)))))
  → _ToolsetFound → Import Microsoft.Cpp.ToolsetLocation.props
      _PlatformProps = $(_VCTargetsPathForToolset)$(_RelativePlatformProps) → Import Platform.props
        → Microsoft.Cpp.Platform.props → Toolset.props → Microsoft.Cpp.MSVC.Toolset.x64.props
          → Microsoft.Cpp.MSVC.Toolset.Common.props → Microsoft.Cpp.Common.props
            → Microsoft.Cl.Common.props    ← every ClCompile default
```

The whole chain hangs off one property assigned from a nested static property function.
`expand()` recognized only MVP-43's instance-method form (`$(Prop.Method('arg'))`), so that value
was left verbatim and recorded unsupported, ADR 32's taint tracking marked
`_VCTargetsPathForToolset`, and every import built from it was skipped with a "could not be fully
resolved" warning. Generation succeeded and looked plausible, which is the outcome CLAUDE.md §3
ranks worst.

ADR 32 considered evaluating exactly these functions and declined, on the grounds that
`ToolsetLocation.props` "computes nothing sln2cc needs": ADR 17 already resolves the toolset's
include paths directly from the filesystem. That was true of include paths and false of everything
else. The skipped `.props` are where Microsoft sets the compiler defaults a project never writes
down — the runtime library, and through it `_DEBUG`; `UNICODE`/`_UNICODE` from `CharacterSet`;
exception handling, conformance mode, `wchar_t` handling. ADR 17 has no substitute for any of
these, and hard-coding copies of them is what `flags.rs`'s own notes already reject.

## Decision

`expand()` recognizes three static property functions, tried right after MVP-43's
`property_function` and before the existing unsupported/verbatim fallback:

| Call | Result |
| --- | --- |
| `$([MSBuild]::Unescape(s))` | `s` with `%XX` hex escapes decoded. A `%` not followed by two hex digits is left as written. |
| `$([Microsoft.Build.Utilities.ToolLocationHelper]::FindRootFolderWhereAllFilesExist(roots, files))` | The first `;`-separated root under which every `;`-separated relative file exists, **as written**. `""` when none does, or when `files` is empty. |
| `$([MSBuild]::GetDirectoryNameOfFileAbove(start, file))` | The first of `start` and its ancestors that contains `file`, or `""`. |

1. **A fixed list, not a grammar.** Class and method names match exactly and case-sensitively, as
   ADR 38's instance methods do — this is .NET binding, not this evaluator's keyword grammar. The
   argument count must match. Anything else — `NormalizePath`, the `Version*` family,
   `GetLatestSDKTargetPlatformVersion`, any other class — falls through unchanged, so ADR 32's
   toolchain downgrade still covers it.
2. **Static-call arguments are expanded, recursively.** Each argument is trimmed, has one matching
   pair of `'` or `"` stripped, and is expanded one level deeper. This is what lets the nested
   `Unescape(FindRootFolderWhereAllFilesExist($(…), $(…)))` resolve inside out. Nesting is capped at
   32 (`MAX_FUNCTION_DEPTH`, matching `MAX_CONDITION_DEPTH`), and exceeding it fails as
   `Error::RecursionLimit`.
3. **An argument that cannot be computed makes the whole call unsupported.** If any argument's
   expansion records something unsupported — an unlisted construct, or a reference to a tainted
   property — the call is not evaluated. The outer reference is recorded and left verbatim, as
   `a_reference_inside_an_unsupported_construct_is_part_of_it` already established. Undefined
   property names inside arguments are carried up when the call succeeds.
4. **Taint needs no change.** A property assigned from a fully resolved call records nothing
   unsupported, so `Properties::assign` leaves it clean, and MVP-36 stops skipping the imports
   built from it.
5. **`$(Registry:…)` stays unsupported.** `Microsoft.Cpp.WindowsSDK.props` has dozens of them, so
   warnings from it remain.

## Discussion

**Why revisit ADR 32 at all.** ADR 32's worry was that "building even a narrow slice of
`$([Class]::Method(...))` risks looking like an invitation to keep extending it", and its
justification for declining was that the location chain's output was irrelevant. The user's bug
showed the second premise was wrong, and once it is, the cost-benefit flips: these three functions
gate every compiler default a real project inherits. The first premise is answered by keeping the
list closed and naming the rule for growing it, the same one ADR 38 set — a function joins when a
real file on a real path needs it, not before.

**Why these three and not `NormalizePath`.** `ToolsetLocation.props` also calls
`[MSBuild]::NormalizePath`, but only inside `Condition="'$(VcpkgInstalledVCTargets)' == 'true'"`,
a vcpkg-installed-toolset branch a stock installation never takes. `GetDirectoryNameOfFileAbove` is
the weakest of the three: `Microsoft.Cpp.props` only calls it when a project opts into
`EnableVcpkgArtifactsIntegration` (the toolset never sets it), and even left unsupported the next
group clears `VcpkgManifestDirectory` again once `Exists()` fails on the garbled path. It is in
anyway because it is the only other static call in `Microsoft.Cpp.props` itself, an opted-in
project would otherwise get an unsupported-construct warning on every generation, and it is a pure
existence walk with no design questions of its own. With it, these three are every static call on
the path from `Microsoft.Cpp.props` to `Microsoft.Cl.Common.props`.

**Why recursion doesn't undo ADR 08.** ADR 08 kept expansion a single, non-recursive pass so a
cyclic property cannot be constructed: values are expanded on assignment, and a stored value is
never rescanned. That still holds. The new recursion walks the *literal input text* — an argument
that is itself a substring of what the file wrote — and never re-expands a stored value. Its
depth is bounded by how deeply the file nests calls, which the cap limits. The cap matters: the
1 MiB value cap would otherwise allow tens of thousands of nested `Unescape(` and a stack overflow,
and CLAUDE.md §5 treats project files as hostile.

**Why instance-method arguments stay literal.** ADR 38 left "should property-function arguments
ever be expanded?" open. This decision answers it for static calls only, because the real chain
needs it there. `$(Foo.StartsWith($(Bar)))` has no real case yet, and extending the same mechanism
to it would be the speculative widening ADR 38 declined.

**Why `FindRootFolderWhereAllFilesExist` returns the root as written.** The chain writes
`$(_VCTargetsPathForToolset)$(_RelativePlatformProps)` with no separator, relying on the root's
trailing `\`. Normalizing the result (as `paths::resolve` does) would drop that separator and fuse
`v170` with `Platforms`. MSBuild itself returns the candidate string unchanged.

**Why Registry reads stay unsupported.** A Python reimplementation of this pipeline expanded them
to `""`. That would be a silent guess about a real machine's registry, which CLAUDE.md §3 rules
out, and the values they feed — Windows SDK locations — are already resolved by ADR 17. Confirmed
with the user rather than assumed.

**Why the metadata guards were split out.** The real `Microsoft.Cl.Common.props` sets
`<RuntimeLibrary Condition="'%(ClCompile.RuntimeLibrary)' == ''">`. `Condition` evaluation does not
expand `%(...)`, so even with the chain loaded, that default does not apply. Fixing it is a
separate change to `Condition` evaluation and `expand_metadata`, with its own design question
(how strictly to match the `ClCompile.` qualifier). The user chose to split it into MVP-50, so this
ADR's end-to-end tests use a stub whose default is gated at `ItemDefinitionGroup` level only.

## Consequences

- **The toolset's own `.props` load on a real installation.** Every `ClCompile` default in
  `Microsoft.Cl.Common.props` that is *not* guarded on its own metadata now reaches the database.
  Real output changes for every project, which is the point, but users will see new flags.
- **The `_DEBUG` symptom itself is not fixed until MVP-50 lands.** This ADR fixes the import
  chain; MVP-50 makes the metadata-guarded defaults inside it apply.
  **Update:** fixed by [ADR 56](56-metadata-aware-metadata-conditions.md) (MVP-50) — metadata
  setting conditions now read the bag being built, so the guarded defaults apply.
- **`expand()` can read the filesystem**, but only for existence checks inside the two lookup
  functions, the same kind of check `Exists()` already makes during `Condition` evaluation.
  Nothing is executed.
- **More toolchain files are evaluated, so more ADR 32 warnings come from deeper in the chain**
  (`WindowsSDK.props`' registry reads above all). ADR 32's open question about warning noise gets
  more pressing.
- **ADR 32 is marked in place** where it declined the grammar, pointing here. **ADR 38's**
  argument-expansion open question is marked as answered for static calls only.
- **New open question:** whether `[MSBuild]::NormalizePath` and the vcpkg-installed-toolset branch
  should be supported. Deferred until a real installation uses it.
