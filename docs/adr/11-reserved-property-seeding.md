# ADR 11 — Seeding Reserved and Well-Known MSBuild Properties

- **Status:** Accepted
- **Date:** 2026-08-05
- **Issue:** [MVP-11 — Reserved and well-known MSBuild properties](../todos.md#msbuild-evaluation) (`feat/msbuild-reserved-properties`)
- **Affects:** `cli/src/msbuild/reserved.rs`, `cli/src/msbuild/eval.rs`,
  `cli/src/msbuild/imports.rs`,
  [engineering_spec.md](../engineering_spec.md) ("CLI Binary — Internal Modules"),
  [ADR 05](05-sln-configuration-mapping.md), [ADR 08](08-eager-macro-expansion.md),
  [ADR 09](09-condition-evaluation-subset.md), [ADR 10](10-import-resolution.md),
  MVP-12, MVP-13, MVP-15

## Problem

MVP-10 produced a working first evaluation pass over an **empty** property bag, and that is the
binding constraint on everything downstream. Project files reference `$(SolutionDir)`,
`$(ProjectDir)`, `$(Configuration)`, `$(MSBuildThisFileDirectory)` and a dozen more without ever
defining them, because MSBuild supplies them. Against an empty bag every one of those expands to
nothing:

- `<AdditionalIncludeDirectories>$(ProjectDir)inc</...>` yields `inc` — a relative path clangd
  resolves against the wrong directory, or not at all.
- `<Import Project="$(SolutionDir)shared\common.props" />` resolves to a bare relative path.
- `Condition="'$(Configuration)|$(Platform)'=='Debug|x64'"` compares `'|'` against `'Debug|x64'`, so
  **no** configuration-specific `PropertyGroup` applies and the flags come from nowhere.

Each of those fails in the shape CLAUDE.md §3 singles out: generation succeeds, the database looks
plausible, and clangd reports a missing header with nothing to trace it to.

ADR 10 anticipated this and left the door open deliberately — `imports::resolve` takes its
`Properties` as a parameter so the seed can be supplied without the resolver knowing what is in it.
This ADR settles what goes in, and three questions that are not obvious:

1. Where the values live and who owns the list.
2. What happens when a project file assigns to one of them.
3. How `MSBuildThisFile*`, which is a property of the *file being read* rather than of the project,
   coexists with a bag that is otherwise built once.

## Decision

A new `msbuild/reserved.rs` owns the **policy** — which names exist and what they hold. `eval.rs`
owns the **mechanism** that keeps a project file from overwriting them. `imports.rs` learns neither.

```rust
pub struct ProjectScope { solution: PathBuf, project: PathBuf, config: ConfigPlatform }
pub fn seed(scope: &ProjectScope) -> Properties;

pub struct FileScope(PathBuf);                                  // opaque
pub fn enter_file(props: &mut Properties, file: &Path) -> FileScope;
pub fn restore(props: &mut Properties, scope: FileScope);
```

Twenty-four properties are seeded, in five families: `Solution*` (5), `Configuration`/`Platform`,
`MSBuildProject*` (6), `MSBuildThisFile*` (6), and `Project*` (5). All are derived from three
inputs — the `.sln` path, the `.vcxproj` path, and the selected pair — and nothing else.

`Properties` gains a locked-key set. `set_reserved` writes and locks; `assign` refuses a locked
target, expanding the value anyway so its findings are still reported but storing nothing and
setting a new `Expansion::refused`. `imports.rs` turns that flag into a warning naming the file and
line. `set` remains able to overwrite a locked key, which is what lets the walk rewrite
`MSBuildThisFile*` on every file it enters.

`Walk::enter` brackets `evaluate` with `enter_file`/`restore`, two lines beside the existing
`depth += 1` / `depth -= 1` pair.

## Discussion

**The property list is the whole point, so it is complete rather than minimal.** The issue text named
six properties and "and similar". Seeding only those leaves the rest expanding to nothing, and the
symptom — an include path that silently lost a directory — is indistinguishable from the symptom of
not having done this work at all. Since every value is derivable from three inputs with string
operations, the cost of completeness is a few dozen lines. A test enumerates all twenty-four names
and asserts each holds something, so a typo in one cannot hide.

What is *not* seeded is the toolchain: `$(VCTargetsPath)`, `$(VSInstallDir)`,
`$(MSBuildExtensionsPath)`, `$(WindowsSDKVersion)`. Those need a discovered Visual Studio
installation, which is MVP-14/MVP-15's, and inventing plausible values for them would be exactly the
silent guessing this project refuses.

**The two trailing-separator conventions are MSBuild's, and both are load-bearing.**
`$(MSBuildProjectDirectory)` carries no trailing separator; `$(SolutionDir)`, `$(ProjectDir)` and
`$(MSBuildThisFileDirectory)` do. It reads like an inconsistency to be normalised away, and
normalising it either way breaks real files, because they concatenate directly:
`$(ProjectDir)inc` must produce `...\app\inc`, while `Microsoft.Common.props` itself writes
`<ProjectDir>$(MSBuildProjectDirectory)\</ProjectDir>` — adding the separator that is deliberately
absent. Getting this wrong yields `...\appinc`, a path that does not exist, in a database that looks
right. It has its own test for that reason.

**`MAIN_SEPARATOR` rather than a literal `\`.** MSBuild spells `\` and Windows is the only real
runtime target, which argues for the literal. Against it: the crate must build and test everywhere
(CLAUDE.md §3), every other path in a run comes out of `PathBuf`, and a seeded `/tmp/x\` on the
Linux CI runner mixes separators inside a single path — which breaks `Exists()`, reads as corruption
in a warning, and would have the golden files disagree per platform. The literal buys fidelity to
MSBuild's spelling on the one platform where `PathBuf` already produces that spelling anyway, so it
buys nothing.

**Immutability is enforced in `Properties`, not by a check at the call site.** Three options were on
the table:

- *Let assignment win.* Simplest, no new API — and a project file that reassigns `$(ProjectDir)`
  then silently relocates every include path in the project. Rejected outright.
- *A `reserved::is_reserved(name)` predicate consulted by `imports.rs::property_group`.* Four lines,
  zero churn to MVP-08's settled API, and there is exactly one caller of `assign` today. Rejected
  because it is a convention rather than a guarantee: the next caller — MVP-12 evaluating deferred
  item groups, MVP-15 extending the seed — has to remember to ask, and forgetting produces the quiet
  wrong answer rather than a compile error.
- *Locked keys in `Properties`.* Chosen. It costs a `HashSet<String>` and one `bool` on `Expansion`,
  and it fits a boundary MVP-08 had **already drawn**: `set` is for trusted seeding, `assign` is for
  anything read out of a project file. The doc comment on `set` said so a release before this issue
  existed. Making `assign` respect the lock is that comment becoming enforceable.

**A refused assignment is still expanded.** The value is thrown away, so expanding it looks like
waste. It is the same reasoning ADR 09 used to keep `And`/`Or` from short-circuiting: a property
function this tool does not understand is worth reporting whether or not the value it produced is
ever used, and a user tracing a bad include path needs every occurrence, not the surviving ones.

**Two tiers of immutability, not one.** MSBuild distinguishes three things this code has to
reproduce as two:

| MSBuild | Examples | On assignment | Here |
| --- | --- | --- | --- |
| Reserved | `MSBuild*` | Error MSB4004 | Locked |
| Global (solution build) | `Configuration`, `Platform`, `Solution*` | Silently ignored | Locked |
| Ordinary, conditionally defaulted | `ProjectDir`, `ProjectName`, `ProjectExt` | Takes effect | Seeded, overridable |

Locking everything would be simpler and is wrong: `Microsoft.Cpp.Default.props` defines the third
group conditionally — `<ProjectName Condition="'$(ProjectName)'==''">$(MSBuildProjectName)</ProjectName>`
— and a project that renames itself with `<ProjectName>` is doing an ordinary supported thing.
Warning about it would be a false positive on ordinary files, which is how users learn to ignore
warnings. This split is also why both `set` and `set_reserved` exist rather than one function.

Collapsing the first two rows loses MSBuild's error/silence distinction. That is deliberate: erroring
would fail generation over a construct that cannot make the *database* wrong once refused, and going
silent would drop a finding. A warning is the one option that keeps the database generatable and
still names the file.

The idiomatic `<Configuration Condition="'$(Configuration)'==''">Debug</Configuration>` does not
trip any of this, because `property_group` evaluates the condition before attempting the assignment
and the condition is false once `Configuration` is seeded. Only an *unconditional* reassignment
warns, and those are rare. Both cases have tests.

**`MSBuildThisFile*` follows the walk, so seeding cannot be a single up-front call.** These six are
computed from the file being *read*, not the project being evaluated, and they are how a shared
property sheet locates its own siblings. A sheet one directory down that saw the root project's
directory would send every relative path it builds somewhere with none of its headers in it.

The bookkeeping goes in `Walk::enter`, beside `depth`, rather than in an RAII guard: a guard would
have to hold `&mut self.properties` across the call to `evaluate`, which needs `&mut self`. Two
explicit lines mirroring an increment/decrement pair that is already there read better than fighting
that.

`FileScope` carries only the displaced **path**, not six saved strings, because all six values are a
pure function of it — so `restore` is a second call to the same private helper. That also means
`Properties` needs no removal API: `seed` sets `MSBuildThisFile*` from the project path, so there is
always a previous value to put back, and the root file needs no special case.

**`ProjectScope` takes paths, not `sln::Solution`.** Seeding needs three facts, and taking the whole
`Solution` would make every unit test build one. It also keeps a decision where it belongs: per
ADR 05 evaluation is handed the **project** pair from `ProjectConfigMapping::project`, not the
solution-level pair the user picked, since a solution's `Debug|x64` legitimately means `Debug|Win32`
for one member. Choosing between them is the caller's job, and a `Solution`-shaped argument would
invite this module to guess.

**`seed` is infallible.** Deriving a file name from a path that has none would seed empty strings,
but such a path could not have been parsed as a project to begin with. ADR 10 removed two error
variants for being unraisable; adding a third would be the same mistake.

## Consequences

- **MVP-12 must call `seed`** before `imports::resolve`, and `msbuild::evaluate`'s signature —
  `(&Path, &ConfigPlatform)` — has no solution path to build a `ProjectScope` from. It needs one.
  Nothing calls `seed` today, since `evaluate` is still `todo!()`.
- **MVP-15 extends the seed** rather than adding a second mechanism: the toolchain families
  (`VCTargetsPath`, `VSInstallDir`, `MSBuildExtensionsPath`) belong in `reserved.rs` behind the
  `ToolchainProvider` trait, and `ProjectScope` is where they arrive.
- **MVP-13 inherits one more warning wording** — the reserved-assignment refusal — on the same terms
  as ADR 10's: the fact is settled here, the phrasing is MVP-13's.
- **MVP-10's undefined-property warning noise drops sharply** once a caller seeds, which is the
  relief ADR 10 said MVP-13 would otherwise have to engineer. It does not disappear:
  `$(VCTargetsPath)` and friends stay undefined until MVP-15.
- **`Expansion` grows a field**, so any future consumer of `assign` has a `refused` case to handle.
  It is `#[must_use]`, which makes ignoring the whole value hard, but not ignoring the flag.
- **An empty bag remains valid.** Every MVP-10 test still resolves from one, and `enter_file` sets
  `MSBuildThisFile*` whether or not anything was seeded. A test pins that.
- **New open question:** whether `$(SolutionDir)` should be seeded at all when a project is evaluated
  outside a solution. Not reachable today — the CLI always takes a `.sln` — but standalone-project
  support and the multi-solution work both arrive at it, and the honest answer may be that those
  properties should be *absent* rather than guessed.
