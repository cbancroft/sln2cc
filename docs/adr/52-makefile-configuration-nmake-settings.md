# ADR 52 — Extracting Compile Settings for Makefile-Configuration Projects from `NMake*` Properties

- **Status:** Accepted
- **Date:** 2026-08-18
- **Issue:** [V2-06 — Extract compile settings from Makefile-configuration
  projects](../todos.md#v2-06) (`feat/makefile-configuration-nmake-settings`)
- **Affects:** `cli/src/msbuild.rs`, `cli/src/msbuild/items.rs`, `cli/src/msbuild/imports.rs`,
  [engineering_spec.md](../engineering_spec.md) ("CLI Binary — Internal Modules"),
  [ADR 42](42-makefile-configuration-missing-import.md)

## Problem

[ADR 42](42-makefile-configuration-missing-import.md) (MVP-44) stopped `generate` from hard-failing
on a `ConfigurationType=Makefile` project — a real NMake dependency vendored into a solution (e.g.
Detours), built outside the normal C++ project system — by downgrading its missing
`$(MSBuildToolsPath)`-rooted toolchain import to a warning. That fix was deliberately narrow: it
left the project contributing **zero files with zero flags** to the generated database, since the
`ItemDefinitionGroup`/`ClCompile` pipeline that would normally populate a source's compile settings
depends on exactly the toolchain import chain ADR 42 skips.

Real Makefile-configuration projects still declare their sources under an ordinary
`<ItemGroup><ClCompile Include=... /></ItemGroup>`, so Visual Studio's Solution Explorer and
IntelliSense have something to show — Detours' own `.vcxproj` does this. What differs is where
their *compile settings* live: not in `ItemDefinitionGroup`, but as plain properties on the
project's own `PropertyGroup` — `NMakePreprocessorDefinitions`, `NMakeIncludeSearchPath`,
`NMakeForcedIncludes`, and a handful of build/link-only properties
(`NMakeBuildCommandLine`, `NMakeOutput`, `NMakeCleanCommandLine`, `NMakeReBuildCommandLine`,
`NMakeAssemblySearchPath`, `NMakeForcedUsingAssemblies`) that drive the actual `nmake` invocation.
So a source file that exists, and would compile perfectly well under `flags::compile` if it only had
the right `Metadata`, gets none — a vendored dependency's own files get no completions or
diagnostics when a user opens them, exactly the gap ADR 42's Consequences section filed as V2-06
rather than solved.

## Decision

`msbuild::evaluate`'s Pass 3 branches on whether the project is a Makefile configuration. For such a
project, each source's settled `Metadata` (from `items::settle`, Pass 2) is discarded and replaced
with one built from the project's own `NMake*` properties instead:

```rust
pub fn nmake_metadata(properties: &Properties) -> Metadata {
    let mut metadata = Metadata::new();
    for (nmake, clcompile) in [
        ("NMakePreprocessorDefinitions", "PreprocessorDefinitions"),
        ("NMakeIncludeSearchPath", "AdditionalIncludeDirectories"),
        ("NMakeForcedIncludes", "ForcedIncludeFiles"),
    ] {
        if let Some(value) = properties.get(nmake) {
            metadata.set(clcompile, value);
        }
    }
    metadata
}
```

Only the three `NMake*` properties that change how a file *parses* are mapped, onto the exact
`ClCompile`-metadata names `flags::compile` already translates. This is a rename, not a new
evaluation pass: `Properties::assign` expands eagerly ([ADR 08](08-eager-macro-expansion.md)), so a
`NMake*` property is already a fully-resolved string in `tree.properties` by the time `Pass 1`
(`imports::resolve`) returns — the same bag `ConfigurationType` itself lives in. `flags::compile` is
reused unchanged, which buys its existing unresolved-`$(`/`%(`-reference warning, include-path
resolution and dedup, and define splitting for free.

`Walk::is_makefile_configuration` (private, checking `self.properties` during the import walk)
becomes a `pub(crate)` free function, `imports::is_makefile_configuration(&Properties) -> bool`,
with `Walk`'s method delegating to it. `msbuild::evaluate` needs to ask the same
`ConfigurationType == Makefile` (case-insensitive) question of the *final* bag `resolve()` hands
back that `Walk` already asks of its own running one, and two independently-typed string-literal
comparisons is how that check quietly drifts apart under a future edit.

## Discussion

**Full replacement, not merge.** A Makefile-configuration source's `Metadata` from `items::settle`
is entirely discarded, not layered under or over the `NMake*` values. The two settings mechanisms
describe mutually exclusive build systems for the same project: MSBuild's own compile step is not
what actually runs for a Makefile project, so there is no meaningful "inherit and extend" the way
there is between two ordinary `ItemDefinitionGroup`s. In practice a Makefile-configuration project's
`ItemDefinitionGroup` is typically empty anyway (nothing populates it, per ADR 42), so this mostly
matters for the case of a per-item override — `<ClCompile Include="..."><PreprocessorDefinitions>
...` — which a hand-edited or IDE-massaged project file can still carry. That value describes a
build that will never run under `nmake`, so it must not silently win over the `NMake*` value that
actually does.

**Why only three `NMake*` properties.** The rest of the real family —
`NMakeBuildCommandLine`/`NMakeCleanCommandLine`/`NMakeReBuildCommandLine`/`NMakeOutput`/
`NMakeAssemblySearchPath`/`NMakeForcedUsingAssemblies` — drives the build and link step, which
clangd never performs. This mirrors the line `flags.rs`'s own skip list already draws for ordinary
`ClCompile` codegen/link settings (`Optimization`, `WholeProgramOptimization`, and the rest,
documented at the bottom of `flags.rs`): only what changes how a file parses is worth translating.

**Why no new evaluation pass, and why this needs no fixture toolchain import.** ADR 42 already
anticipated this: "does not require walking `Microsoft.Common.(CurrentVersion.)targets`... those
properties live on the project's own `PropertyGroup`s, not anything that subtree computes." The new
fixture (`fixtures/makefile-configuration/`) accordingly imports none of the real Visual Studio VC
targets at all, following the precedent `fixtures/property-group-import-order` set for MVP-44 itself
(per [ADR 25](25-fixture-simple-and-golden-harness.md)) — the import-skip mechanism ADR 42 built is
already covered by `imports.rs`'s and `msbuild.rs`'s own unit tests and does not need re-proving
here.

**Why `is_makefile_configuration` moves to a free function instead of `msbuild::evaluate`
re-deriving the check.** The alternative — matching `"ConfigurationType"`/`"Makefile"` a second time
directly in `msbuild.rs` — costs nothing today but is exactly the kind of duplication ADR 42's own
gate (`Walk::is_makefile_configuration`) was written to avoid re-deriving a second time elsewhere.
Exposing the existing predicate keeps the two call sites — mid-walk in `imports.rs`, and
post-walk in `msbuild.rs` — provably asking the same question.

## Consequences

- **V2-06 closed.** A Makefile-configuration project's own source files, when declared under an
  ordinary `ClCompile` `ItemGroup`, now get real `include_paths`/`defines` from its `NMake*`
  properties instead of empty `Metadata`.
- **The build/link-only `NMake*` properties remain permanently out of scope**, for the same reason
  `flags.rs` already skips their `ClCompile` codegen/link counterparts: clangd never builds or
  links, only parses.
- **`docs/engineering_spec.md`'s `ConfigurationType=Makefile` paragraph is extended**, not replaced,
  with this settings-extraction behavior alongside ADR 42's import-downgrade one.
- **No change to `eval.rs`, `flags.rs`, `reserved.rs`, or `imports.rs`'s import-resolution logic**
  beyond exposing the existing predicate. The fix is contained to `msbuild.rs::evaluate`'s Pass 3
  dispatch and one new function in `items.rs`, following ADR 42's own precedent for scoping
  toolchain-policy fixes tightly.
