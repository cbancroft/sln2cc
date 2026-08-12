# ADR 17 — Resolving Toolset and Windows SDK Include Paths

- **Status:** Accepted
- **Date:** 2026-08-09
- **Issue:** [MVP-15 — Resolve toolset and Windows SDK include paths](../todos.md#toolchain-discovery) (`feat/toolchain-includes`)
- **Affects:** `cli/src/toolchain.rs`, `cli/src/toolchain/vswhere.rs`, `cli/src/msbuild/reserved.rs`,
  [engineering_spec.md](../engineering_spec.md) ("CLI Binary — Internal Modules"),
  [ADR 11](11-reserved-property-seeding.md), [ADR 14](14-vswhere-discovery.md), MVP-16, MVP-36

## Problem

MVP-14 stops at a located `VsInstance { install_path, version }`; `ToolchainProvider::discover()`
still ends in `todo!()`. Two things are still missing before a `.vcxproj` referencing
`#include <windows.h>` or linking the CRT can produce a usable `compile_commands.json`:

1. The actual system include directories — the MSVC toolset's own `include` folder and the
   Windows SDK's `ucrt`/`um`/`shared` — which live under the installation MVP-14 found but are
   not named anywhere in a project file.
2. The three properties `msbuild/reserved.rs` deliberately left unseeded since ADR 11:
   `$(VCTargetsPath)`, `$(VSInstallDir)`, `$(MSBuildExtensionsPath)`. Every `<Import>` depending
   on them — which is most real `.vcxproj` files, since they all reach `Microsoft.Cpp.Default.props`
   through one of these — resolves to an empty path and is skipped with a warning until this
   lands.

Four questions needed a decision before implementation could start:

- Where the Windows SDK lives, given nothing in a project file names it and this project's
  dependency policy rules out a registry-access crate.
- What decides which MSVC toolset version is "the" one when several are installed side by side.
- What decides which Windows SDK version is "the" one when several are installed.
- How `$(VCTargetsPath)` is derived, since it depends on the VS *product* version rather than
  anything under `install_path` alone.

## Decision

`Toolchain` gains two fields:

```rust
pub struct Toolchain {
    pub install_path: PathBuf,       // seeds $(VSInstallDir)
    pub vc_targets_path: PathBuf,    // seeds $(VCTargetsPath)
    pub include_paths: Vec<PathBuf>, // msvc include, then ucrt, um, shared
    pub toolset_version: String,
    pub sdk_version: String,
}
```

`vswhere.rs` resolves the rest of `discover()` as a small pipeline, split the same way MVP-14
split `locate()` from `select_instance()`:

```rust
fn resolve(instance: &VsInstance, sdk_root: &Path) -> Result<Toolchain>;
fn read_toolset_version(install_path: &Path) -> Result<String>;   // VC/Auxiliary/Build/Microsoft.VCToolsVersion.default.txt
fn msvc_include_dir(install_path: &Path, version: &str) -> Result<PathBuf>;
fn sdk_root() -> PathBuf;                                          // %ProgramFiles(x86)%\Windows Kits\10
fn resolve_sdk(sdk_root: &Path) -> Result<WindowsSdk>;
fn vc_targets_path(instance: &VsInstance) -> PathBuf;
```

`msbuild/reserved.rs`'s `ProjectScope` gains `toolchain: Option<Toolchain>`, and `seed()` writes
`VCTargetsPath`, `VSInstallDir`, `MSBuildExtensionsPath` (each with a trailing separator, matching
every other directory-valued property this module seeds) when it is `Some`, and leaves all three
undefined when it is `None`.

Both new failure shapes — no toolset version file, no matching `VC/Tools/MSVC/<version>` include
directory, no complete Windows SDK — raise the existing `Error::ToolchainIncomplete`, which
already covers exactly this case end to end (`error.rs`, `ErrorKind::as_str`, `exit_code`). No new
`ErrorKind`.

## Discussion

**The Windows SDK is found via a well-known path, not the registry.** `vswhere_path()` already
established the pattern MVP-14 used for `vswhere.exe` itself: one fixed Program Files location, no
`PATH` search, no registry read. `%ProgramFiles(x86)%\Windows Kits\10` is that same shape for the
SDK. The alternative — reading `HKLM\SOFTWARE\Microsoft\Windows Kits\Installed Roots` — is how
Microsoft's own tooling actually finds a relocated SDK, and was rejected: CLAUDE.md §5 keeps
dependencies to what's settled, a registry crate is a new one, and the glossary already frames
`vswhere` as "the supported way to locate a toolchain, as opposed to guessing at registry keys or
`C:\Program Files` paths" for the VS side of this problem. A relocated SDK is a real but rare
setup; when it happens, `resolve_sdk` will find nothing at the well-known root and fail loud with
`ToolchainIncomplete`, which is the correct outcome for a configuration this project doesn't yet
support, not a silent wrong answer. Widening the search is cheap to add later against a real
report of it mattering — the same reasoning ADR 14 used for `vswhere_path()` itself.

**The MSVC toolset version comes from the version file, not from picking the newest installed
folder.** The issue text is explicit: "read the toolset version file, locate the *matching*
`VC/Tools/MSVC/<version>/include` directory" — singular. Real MSBuild reads
`VC\Auxiliary\Build\Microsoft.VCToolsVersion.default.txt` when a project doesn't override
`$(VCToolsVersion)`, and using anything else would silently diverge from what an actual `cl.exe`
invocation resolves to — exactly the guessing CLAUDE.md §3 refuses. The alternative considered —
treat multiple `VC\Tools\MSVC\*` directories the way `newest()` treats multiple VS instances — was
rejected because "newest installed" and "what the default file names" are different questions with
different real-world answers: an install can carry an older toolset pinned by the version file
specifically because a project depends on it, and picking the newest would produce flags a real
build never uses. The "several toolsets installed" test case is satisfied by proving the version
file's answer wins over both the first and the last directory on disk, not by proving "newest"
wins.

**Windows SDK version selection *does* pick the newest, and specifically the newest *complete*
one.** Unlike the MSVC toolset, there is no per-installation default-version file for the SDK
family this issue resolves, and no project-level `$(WindowsTargetPlatformVersion)` is threaded
through yet — that arrives with a real subcommand pipeline, out of scope here. Reusing MVP-14's
numeric version comparison (`newest`'s underlying `parse_version`) rather than duplicating it means
one tested comparator, not two. The one addition specific to the SDK: a version directory missing
`ucrt`, `um`, or `shared` is skipped in favor of an older complete one rather than failing the
whole lookup outright. This is a deliberate softening of "never silently guess" — the surviving
choice must still be the newest **usable** version, so it is guessing between two knowable facts
(a directory exists or it doesn't), not inventing a fact. The cost: an SDK installation that is
missing one family for every version present still produces `ToolchainIncomplete`, correctly, and
a newest-but-broken SDK never gets silently promoted over the older one that actually works. A
half-installed newest SDK is an ordinary disk state — installer updates and workload changes leave
partial installs behind routinely — and is not the same failure category as a project file lying
about a path.

**`$(VCTargetsPath)` is precomputed in `vswhere.rs`, not derived in `reserved.rs` from a bare
`install_path`.** The MSVC toolset version (`14.38.33130`) and the VS product version
(`17.8.34309.116`) are unrelated numbers, and only the second determines the `v170`/`v160` folder
under `MSBuild\Microsoft\VC\`. `VsInstance.version` is in scope in `vswhere.rs` and nowhere else by
design — `ProjectScope` never carries a `VsInstance`, only a `Toolchain` — so computing the path
where the version already lives avoids threading a second version string through `reserved.rs` for
one derived value. The major-version-to-folder mapping (`v{major}0`) is a real but simple pattern
across VS2019/2022, the only two schemas this project targets (CLAUDE.md §8).

**`ProjectScope.toolchain` is `Option`, not required.** `msbuild::evaluate` has no subcommand
calling it yet — the same position `reserved::seed` itself was in before MVP-12 wired it up — so
every test today builds a `ProjectScope` by hand, and a required `Toolchain` would be unrelated
churn to every one of them for this issue. `None` preserves the exact behavior ADR 11 documented:
the three properties stay unseeded, and every import depending on them keeps warning about an
empty path, rather than seeding an empty string that `Condition="'$(X)' != ''"` would read as
*defined*.

## Consequences

- **MVP-16** consumes `Toolchain::include_paths` directly when assembling `compile_commands.json`
  entries — ordered project includes first, these appended after, per its own scope.
- **MVP-36** (unsupported-condition policy for toolchain-derived imports) can now be worked: the
  three properties it needs seeded are seeded whenever a `Toolchain` is discovered. **Update:**
  landed, using `Toolchain::install_path` as the provenance signal. See
  [ADR 32](32-toolchain-unsupported-construct-policy.md).
- **No changes to `error.rs`.** `Error::ToolchainIncomplete` already covered every failure shape
  this issue raises.
- **No new dependencies.** The SDK's well-known path reuses the exact pattern `vswhere_path()`
  established; no registry crate was added.
- **ADR 11 and ADR 14's own "Consequences" sections are updated** to note this issue landed and
  point here, per CLAUDE.md §9 — neither is rewritten.
- **New open question:** a Windows SDK relocated away from its well-known Program Files root is
  invisible to `resolve_sdk` today, exactly as a relocated `vswhere.exe` is invisible to MVP-14's
  `vswhere_path()`. Neither has a reported case motivating the extra complexity of a registry read
  or a search; worth revisiting if one surfaces.
