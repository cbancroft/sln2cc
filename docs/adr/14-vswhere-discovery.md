# ADR 14 — Visual Studio Discovery via `vswhere`

- **Status:** Accepted
- **Date:** 2026-08-08
- **Issue:** [MVP-14 — Locate Visual Studio via `vswhere`](../todos.md#toolchain-discovery) (`feat/vswhere-discovery`)
- **Affects:** `cli/src/toolchain/vswhere.rs`,
  [engineering_spec.md](../engineering_spec.md) ("CLI Binary — Internal Modules"),
  [ADR 02](02-structured-error-type.md), [ADR 11](11-reserved-property-seeding.md), MVP-15, MVP-36

## Problem

Every include path sln2cc can derive from `.vcxproj`/`.props` files runs out the moment a project
references `#include <windows.h>` or links against anything in the Windows SDK: those directories
live under a Visual Studio installation, and there is no `$(...)` in the project that names where
that installation is until sln2cc finds it first. `ToolchainProvider::discover()` has been a bare
`todo!()` since `toolchain.rs` was scaffolded, and `msbuild/reserved.rs` (ADR 11) has left
`$(VCTargetsPath)`/`$(VSInstallDir)` deliberately unseeded for the same reason — inventing plausible
values would be exactly the silent-wrong-answer failure CLAUDE.md §3 refuses.

`vswhere` is Microsoft's own answer: every Visual Studio and standalone Build Tools installer since
2017 drops a copy at a fixed Program Files path, and it can enumerate installed instances, filtered
by component, as JSON. Two questions had no single obviously-right answer and needed a decision
before implementation could start:

1. Which component ID actually proves an instance can compile C++, given that VS ships in shapes
   from a bare Build Tools install to the full IDE with every workload.
2. Whether a Preview-channel instance should ever be selected over none at all.

A third question was mechanical rather than open: MVP-15 needs an install root to walk into for the
toolset and Windows SDK, but nothing calls `discover()` from the CLI yet — no subcommand reaches it,
the same position `reserved::seed()` was in until MVP-12 wired it up — so this issue only has to
produce that install root correctly, not a complete `Toolchain`.

## Decision

`vswhere.rs` gains a small pipeline, split at the impure/pure boundary:

```rust
pub struct VsInstance { pub install_path: PathBuf, pub version: String }

fn vswhere_path() -> PathBuf;                          // %ProgramFiles(x86)%\...\vswhere.exe
fn run_vswhere(vswhere: &Path) -> Result<String>;       // shells out, returns raw stdout
fn parse_instances(json: &str) -> Result<Vec<VsInstance>>;
fn newest(instances: Vec<VsInstance>) -> Option<VsInstance>;
fn select_instance(json: &str) -> Result<VsInstance>;   // parse_instances + newest + error

impl VsWhereToolchain {
    fn locate(&self) -> Result<VsInstance> { select_instance(&run_vswhere(&vswhere_path())?) }
}
```

`vswhere` is invoked as `-products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64
-format json -utf8`, an argument vector per CLAUDE.md §5, with no `-prerelease` flag. `discover()`
calls `locate()` and leaves the rest — reading the toolset and Windows SDK layout under
`install_path` — as `todo!()` for MVP-15.

Both "no `vswhere.exe`" and "`vswhere` ran but returned no matching instance" raise the existing
`Error::ToolchainNotFound`; no new `ErrorKind` is added.

## Discussion

**Component ID: `VC.Tools.x86.x64`, not the `NativeDesktop` workload.** Two candidates were on the
table, both real vswhere `-requires` arguments:

- `Microsoft.VisualStudio.Workload.NativeDesktop` — the "Desktop development with C++" workload
  shown in the VS Installer UI. What most users who chose C++ support in the graphical installer
  actually have.
- `Microsoft.VisualStudio.Component.VC.Tools.x86.x64` — the core MSVC compiler component, one of
  many the workload pulls in.

The workload ID was rejected because a standalone "Build Tools for Visual Studio" install — common
on CI runners and for contributors who never open the IDE — carries the component without ever
installing the workload that bundles it. Filtering on the workload would report "no usable Visual
Studio installation" on a machine that can, in fact, run `cl.exe`. The component is the narrower,
more accurate signal for "can this compile C++", which is the only thing sln2cc needs; ATL/MFC and
the rest of what the workload adds are irrelevant to a compilation database. This was confirmed with
the user rather than decided silently, since it changes which real installations are found.

**Preview/prerelease instances are excluded**, which costs nothing extra — it is `vswhere`'s own
default when `-prerelease` is omitted. Rejected the alternative of including them: a Preview-channel
compiler is not what CI or a typical contributor builds against, and picking one over "no
installation found" would trade a clear, actionable error for a toolset the project's own build
never uses. Confirmed with the user for the same reason as the component choice.

**No new `ErrorKind` for malformed vswhere output.** The issue's test list asks for "multiple
instances, empty result, malformed output" as three cases, which invites three different failure
shapes. ADR 02 scoped the fourteen existing variants to failure modes `todos.md` names explicitly;
"vswhere emitted something that isn't JSON" isn't one of them, and `Error::ToolchainNotFound`'s own
doc comment already says its two causes share a remediation — reinstall or repair Visual Studio.
Malformed output is the same story: something about the install is broken, and there's nothing more
specific for the plugin to act on today. The `detail` string still names the parse failure for a
human reading it. If the plugin ever needs to tell "no installation" apart from "vswhere is broken"
programmatically, that's a cheap variant to add against a genuine caller, not against a hypothetical
one.

**The shell-out is split from the parsing, and the parsing is split from selection.**
`run_vswhere` does exactly one impure thing — run a process and hand back its stdout as a `String`
— parameterized on the `vswhere` path rather than resolving it internally. That last detail is what
makes "no `vswhere.exe` present" unit-testable on any OS: a test can point `run_vswhere` at a path
that exists nowhere and assert a clean `Err`, without needing Windows or a real installation, which
CLAUDE.md §8 requires. `parse_instances` and `newest` are pure, so `select_instance("[]")` and
`select_instance("{not valid")` exercise the "empty result" and "malformed output" cases directly
against recorded-shaped strings — the same pattern `msbuild/eval.rs`'s pure `expand` and
`condition_holds` established for testing evaluation logic without a filesystem.

**Version comparison is numeric per dotted component.** `installationVersion` looks like
`17.8.34309.116`. Comparing the strings directly was considered and rejected: `"9.0.0.0" >
"17.0.0.0"` lexicographically, which would make a hypothetical VS2015-and-VS2022 side-by-side
install pick the older one — wrong in the one situation "pick the newest" exists to handle.
`parse_version` splits on `.` and parses each segment as `u64`, comparing the resulting vectors
lexicographically (which `Vec<u64>`'s `Ord` already does correctly component-by-component). An
unparseable segment falls back to `0` rather than erroring: `vswhere`'s version string is
Microsoft's own tooling output, not untrusted project-file content, so this is a display-quality
tiebreak rather than a security boundary, and refusing to select an instance over a cosmetic version
quirk would be a worse failure than picking imperfectly.

**One well-known path, no search.** The issue text says "its well-known Program Files location" —
singular — so `vswhere_path()` builds
`%ProgramFiles(x86)%\Microsoft Visual Studio\Installer\vswhere.exe` and does not fall back to
scanning `PATH` or `Program Files` (non-x86, relevant on ARM64 Windows). An installation vswhere
itself can't find is exactly as unusable to sln2cc as no installation at all, so this collapses
into the same `ToolchainNotFound` case rather than needing a distinct one. Widening the search is
cheap to add later against a real report of it mattering.

## Consequences

- **MVP-15 consumes `VsInstance::install_path`** to locate `VC/Tools/MSVC/<version>/include` and the
  Windows SDK layout, and removes `discover()`'s remaining `todo!()`. **Landed**: see
  [ADR 17](17-toolchain-include-resolution.md).
- **MVP-36** (unsupported-condition policy for toolchain files) still needs `$(VCTargetsPath)` seeded
  from a real installation before it can be worked, which is unchanged — this issue only produces an
  install root, not the seeded properties themselves.
- No changes to `error.rs`: `Error::ToolchainNotFound` already covered every case this issue raises.
- No new dependencies: `serde`/`serde_json` already parse the toolchain-independent JSON everywhere
  else in the crate.
