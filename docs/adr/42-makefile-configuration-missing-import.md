# ADR 42 — Downgrading a Missing Toolchain Import for Makefile-Configuration Projects

- **Status:** Accepted
- **Date:** 2026-08-15
- **Issue:** [MVP-44 — Don't hard-fail `generate` on Makefile-configuration-type
  projects](../todos.md#mvp-milestone) (`fix/makefile-configuration-type`)
- **Affects:** `cli/src/msbuild/imports.rs`, [engineering_spec.md](../engineering_spec.md) ("CLI
  Binary — Internal Modules"), [ADR 32](32-toolchain-unsupported-construct-policy.md), MVP-34,
  V2-06

## Problem

MVP-44 was originally filed as "`ConfigurationType` resolves to the wrong branch across an
`Import` boundary" — found running MVP-34's real-solution pass against a real external Contoso3
solution, which failed `generate` outright with `Error::MissingImport` naming the literal path
`C:\Microsoft.Common.Targets`. The filer's working theory was a `PropertyGroup`/`Import`
document-order bug in `imports.rs`, since the failing project sets `ConfigurationType` in a
per-configuration `PropertyGroup` before importing `Microsoft.Cpp.props`, and the import graph
then behaves as if `ConfigurationType` were `Makefile` — a value nothing in the project or
toolchain appeared to assign.

An earlier pass on this branch tested that theory directly: a synthetic `FakeToolchain` fixture
reproducing the exact shape (a self-defaulting property, a local unconditional override between
two imports, and a second import's `Condition` reading the result) resolved *correctly* under
`imports.rs::Walk::evaluate()`, which already processes `PropertyGroup` and `Import` strictly in
document order. `<Choose>`/`<When>`/`<Otherwise>` was checked too and ruled out — `project.rs`
records it as `Element::Unsupported` and never evaluates it, so it can only ever be inert, never
wrongly take a branch. The real trigger stayed unconfirmed; the regression test and fixture landed
anyway as a guard for the (already-correct) mechanism the issue's title had blamed.

With the real Contoso3 solution available locally, `generate -v` was finally run against it
directly, and the actual mechanism turned out to be nothing the title described:

- `vendorlib_2022.vcxproj` — the project MVP-44 originally suspected — evaluates cleanly. Its
  `ConfigurationType` resolves correctly to `StaticLibrary`, in exactly the
  self-default/override/import shape the issue described.
- The failure is `external\Detours\vc\Detours.vcxproj` — a genuine member project of the `.sln`
  (`Project("{...}") = "Detours", "..\..\external\Detours\vc\Detours.vcxproj", ...`), which
  legitimately declares `<ConfigurationType>Makefile</ConfigurationType>` for every configuration,
  because it really is built via `nmake`
  (`<NMakeBuildCommandLine>...nmake</NMakeBuildCommandLine>`), not the normal C++ project system.
  Real Visual Studio solutions vendor dependencies like this routinely.
- `Microsoft.Cpp.props` correctly takes the `Makefile` branch — msvclsp's own `Condition`
  evaluation is not at fault, mirroring what the earlier investigation already found — and imports
  `Microsoft.MakeFile.Targets`, which imports `$(MSBuildToolsPath)\Microsoft.Common.targets`.
  `$(MSBuildToolsPath)` is not among the properties `reserved.rs` seeds (deliberately — see ADR
  11's note that toolchain-derived properties are supplied incrementally as something actually
  needs them), so it expands to nothing, the import resolves to the literal path
  `C:\Microsoft.Common.Targets`, and `Walk::evaluate` raises `Error::MissingImport` — fatal,
  aborting `generate` for the entire solution over one dependency project.

This is the same *shape* [ADR 32](32-toolchain-unsupported-construct-policy.md) (MVP-36) already
solved — a toolchain file using something this evaluator can't fully resolve — but not the same
*case*. ADR 32's downgrade is keyed on `expansion.unsupported`: the import path itself carries an
unsupported construct (a property function, a `ToolLocationHelper` call) left verbatim by
`expand()`. Here `$(MSBuildToolsPath)` is simply an *undefined* property — `expand()` reports it
as "not defined and expanded to nothing," a distinct, unremarkable finding every real solution
already produces dozens of. `expansion.unsupported` is empty, so ADR 32's existing gate does not
fire, and the import fails exactly as loud as a genuinely broken installation would.

Seeding `$(MSBuildToolsPath)` for real — pointing it at the actual MSBuild `Bin` directory next to
the discovered Visual Studio installation — was tried by inspection before writing this ADR.
`Microsoft.Common.targets` on a real VS2022 install is 64 lines whose entire job is
`<Import Project="$(MSBuildToolsPath)\Microsoft.Common.CurrentVersion.targets" />` — thousands of
lines of generic .NET/MSBuild build machinery (package restore, `Directory.Build.targets`
discovery, project-extension imports) with no C++ relevance whatsoever, and well outside "Only
VS2019/2022 [C++] schemas are in scope" (CLAUDE.md §8). Even with ADR 32's `in_toolchain` downgrade
absorbing whatever unsupported constructs it contains, walking into it buys nothing: a
`ConfigurationType=Makefile` project has no `<ClCompile>` items for `items::settle()` to find
either way, so every property that subtree would resolve is dead weight `msvclsp` would pay
parsing and evaluation cost for, for zero effect on the generated database.

## Decision

`Walk::evaluate`'s `MissingImport` raise gains one more downgrade, alongside ADR 32's two, gated on
a new `Walk::is_makefile_configuration()`:

```rust
fn is_makefile_configuration(&self) -> bool {
    self.properties
        .get("ConfigurationType")
        .is_some_and(|value| value.eq_ignore_ascii_case("Makefile"))
}
```

When an import that would otherwise be `Error::MissingImport` is referenced by a file under
`in_toolchain` *and* the project currently being walked has `ConfigurationType == Makefile`
(case-insensitive, matching `eval::condition_holds`'s treatment of `Condition` operands and the
exact test `Microsoft.Cpp.props` itself performs to route a project down this branch), it is
downgraded to a warning and the import is skipped rather than resolved. Everything else about the
project's evaluation proceeds normally; since a Makefile project has no `ClCompile` items, `Pass
2`/`Pass 3` produce an empty file list, which `msbuild.rs::evaluate`'s existing "no `ClCompile`
source files were found" warning already covers — no new empty-result handling needed.

Makefile-configuration projects' own compilation settings (`NMakePreprocessorDefinitions`,
`NMakeIncludeSearchPath`, and friends) are not extracted. That is out of scope here — tracked as
**V2-06** in `docs/todos.md`, not solved by this ADR.

## Discussion

**Gated on `ConfigurationType`, not on `in_toolchain` alone.** ADR 32's Discussion section already
considered and rejected the broader rule — "every `MissingImport` from a toolchain-rooted file is
non-fatal" — because a cleanly-resolved, genuinely-absent toolchain file is a different problem
(a broken or relocated VS installation) that should keep failing loud, matching ADR 17's
`ToolchainIncomplete` philosophy. `imports.rs`'s existing regression test,
`a_genuinely_missing_import_in_a_toolchain_file_still_fails`, guards exactly this, and this ADR
does not loosen it — it adds a second, narrower condition (`ConfigurationType == Makefile`) rather
than widening ADR 32's `in_toolchain` gate to cover this case. A non-Makefile project hitting a
genuinely missing toolchain import — the actual "broken installation" signature — still aborts
generation exactly as before.

**Why not seed `$(MSBuildToolsPath)` and let it resolve for real.** Covered above: the file it
points at is realistically irrelevant to a `compile_commands.json` and expensive/risky to walk
into for no payoff. Filed as V2-06 instead of attempted here, in case a future version wants
Makefile-project compilation settings badly enough to justify reading `NMake*` properties directly
— which does not require walking `Microsoft.Common.(CurrentVersion.)targets` at all, since those
properties live on the project's own `PropertyGroup`s, not anything that subtree computes.

**Why the gate lives in `Walk::evaluate`, not earlier.** `ConfigurationType` is only known after
the project's own `PropertyGroup`s run — which is itself part of Pass 1 (see
[ADR 40](40-item-definitions-apply-regardless-of-item-order.md)/MVP-45 on why definitions and items
are already split into phases for a related ordering reason). There is no earlier hook without
partially duplicating Pass 1 just to look ahead for one property before starting it for real.
Catching it at the one call site that actually needs it — the missing-import raise itself — is
smaller than restructuring the pass boundary for a single-project case.

**The misdiagnosis is recorded here on purpose.** MVP-44's original title and branch name
(`fix/property-group-import-order`) described a bug that does not exist; the regression test that
theory produced stays, because it correctly guards a real (if previously unconfirmed) mechanism,
just not the one that was failing. A future reader hitting a similar-looking `MissingImport` from
a real solution should find this ADR before re-walking the `PropertyGroup`/`Import`-ordering path
a second time.

## Consequences

- **MVP-34's real-solution pass loses its second and last blocker.** MVP-45 (item-definition
  ordering) closed first; this closes MVP-44. The real-solution checklist pass can now proceed
  past `generate` for the Contoso3 solution.
- **`docs/engineering_spec.md`'s ADR 32 paragraph is extended**, not replaced, with this one
  narrower exception.
- **`docs/todos.md`'s MVP-44 entry is retitled** from "Fix `ConfigurationType` resolving to the
  wrong branch across an `Import` boundary" to "Don't hard-fail `generate` on
  Makefile-configuration-type projects," and its branch name corrected from
  `fix/property-group-import-order` to `fix/makefile-configuration-type` — the recorded name
  described the disproven diagnosis.
- **V2-06 filed**, not solved: Makefile-configuration projects still contribute nothing to the
  database. Acceptable for the MVP, since such projects are typically vendored dependencies rather
  than code the user is editing, but a real gap if one is.
- **No change to `eval.rs`, `items.rs`, `flags.rs`, `reserved.rs`, or `msbuild.rs` beyond the
  existing "no source files" warning path already covering the empty result.** The fix is entirely
  contained to `imports.rs::Walk::evaluate`'s control flow, following ADR 32's own precedent for
  scoping toolchain-policy fixes tightly.
