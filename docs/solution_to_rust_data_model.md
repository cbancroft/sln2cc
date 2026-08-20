# From `.sln` to Rust: How msvclsp Models a Solution

This document walks through how msvclsp turns a Visual Studio solution on disk into the Rust
data structures it uses internally, and ultimately into `compile_commands.json`. It follows the
CLI's own pipeline stage by stage, showing what each stage reads, what struct it produces, and
why that struct is shaped the way it is. See `docs/engineering_spec.md` for the authoritative
design and the ADRs under `docs/adr/` for the reasoning behind individual decisions — this
document is a guided tour, not a replacement for either.

## A worked example

We'll follow a small, two-file example through the whole pipeline: a solution called `Sprocket`
containing one C++ project, `Widget`, which pulls its warning and include settings from a shared
property sheet.

**`Sprocket.sln`**

```
Microsoft Visual Studio Solution File, Format Version 12.00
# Visual Studio Version 17
Project("{8BC9CEB8-8B4A-11D0-8D11-00A0C91BC942}") = "Widget", "Widget\Widget.vcxproj", "{6C204F3E-9CF3-4A61-9E17-9A6E6A6A0A01}"
EndProject
Global
    GlobalSection(SolutionConfigurationPlatforms) = preSolution
        Debug|x64 = Debug|x64
    EndGlobalSection
    GlobalSection(ProjectConfigurationPlatforms) = postSolution
        {6C204F3E-9CF3-4A61-9E17-9A6E6A6A0A01}.Debug|x64.ActiveCfg = Debug|x64
        {6C204F3E-9CF3-4A61-9E17-9A6E6A6A0A01}.Debug|x64.Build.0 = Debug|x64
    EndGlobalSection
EndGlobal
```

**`Widget\Widget.vcxproj`**

```xml
<Project DefaultTargets="Build" ToolsVersion="17.0"
         xmlns="http://schemas.microsoft.com/developer/msbuild/2003">
  <PropertyGroup Condition="'$(Configuration)|$(Platform)'=='Debug|x64'" Label="Configuration">
    <ConfigurationType>StaticLibrary</ConfigurationType>
  </PropertyGroup>
  <Import Project="..\common.props" />
  <ItemDefinitionGroup Condition="'$(Configuration)|$(Platform)'=='Debug|x64'">
    <ClCompile>
      <PreprocessorDefinitions>WIDGET_DEBUG;%(PreprocessorDefinitions)</PreprocessorDefinitions>
    </ClCompile>
  </ItemDefinitionGroup>
  <ItemGroup>
    <ClCompile Include="src\widget.cpp" />
  </ItemGroup>
</Project>
```

**`common.props`** (shared by every project in the solution)

```xml
<Project xmlns="http://schemas.microsoft.com/developer/msbuild/2003">
  <ItemDefinitionGroup>
    <ClCompile>
      <WarningLevel>Level3</WarningLevel>
      <AdditionalIncludeDirectories>$(ProjectDir)include;%(AdditionalIncludeDirectories)</AdditionalIncludeDirectories>
    </ClCompile>
  </ItemDefinitionGroup>
</Project>
```

We'll refer back to these three files throughout.

## Stage 1: the `.sln` — `Solution` and `SolutionProject`

`sln.rs` reads the `.sln` first, because it's the only file that lists which projects exist and
which configuration/platform pairs the solution as a whole declares. `.sln` is line-oriented
text, not XML — Visual Studio invented its own ad hoc format for it — so this is the one parser
in the crate that isn't built on `roxmltree`.

The output is:

```rust
pub struct Solution {
    pub path: PathBuf,
    pub projects: Vec<SolutionProject>,
    pub configurations: Vec<ConfigPlatform>,
}

pub struct SolutionProject {
    pub name: String,
    pub path: PathBuf,       // absolute path to the .vcxproj
    pub guid: String,
    pub mappings: Vec<ProjectConfigMapping>,
}

pub struct ProjectConfigMapping {
    pub solution: ConfigPlatform,  // what the user picks, e.g. Debug|x64
    pub project: ConfigPlatform,   // what MSBuild evaluation actually uses
    pub build: bool,
}
```

For our example, this becomes one `SolutionProject` named `Widget`, with one `ProjectConfigMapping`
mapping the solution's `Debug|x64` onto the project's own `Debug|x64`, `build: true`.

A few mapping decisions worth calling out because they aren't obvious from the XML-like syntax:

- **Membership is filtered by project type GUID**, not file extension. The line
  `Project("{8BC9CEB8-...}") = ...` carries a GUID that Visual Studio itself uses to distinguish a
  C++ project from a solution folder, a C# project, or a shared-items project. Only the C++ GUID
  becomes a `SolutionProject`; everything else is ordinary solution content and is silently
  skipped rather than reported as an error.
- **Solution pair and project pair are two separate fields, not one.** A solution can present a
  single `Debug|x64` to the user while quietly building one member as `Debug|Win32` — this
  actually happens with mixed-architecture solutions. Collapsing the two into one `ConfigPlatform`
  would lose the mapping msvclsp needs to hand each project's evaluator the pair *it* understands.
- **`build: false` matters.** A project can have a row in `ProjectConfigurationPlatforms` without a
  `Build.0` entry — that's what "unchecked in Configuration Manager" looks like on disk. msvclsp
  still evaluates such a project, because its source files still need indexing for `clangd` even
  though the real build skips them; only the `build` flag would differ if that policy changed.

At this point msvclsp knows *that* `Widget.vcxproj` exists and *which* configuration pair to
evaluate it under. It knows nothing about the project's contents yet — that's every remaining
stage.

## Stage 2: reading one project file — `ProjectFile` and `Element`

Each `.vcxproj` (and, as we'll see, each `.props`/`.targets` it imports) is read by
`msbuild/project.rs` into an intermediate representation that does no evaluation at all: no macro
is expanded, no `Condition` is checked, no `<Import>` is followed. It only answers "what elements
does this file contain, and in what order":

```rust
pub struct ProjectFile {
    pub path: PathBuf,
    pub elements: Vec<Element>,   // document order
}

pub enum Element {
    Properties(PropertyGroup),
    ItemDefinitions(ItemDefinitionGroup),
    Items(ItemGroup),
    Import(Import),
    ImportGroup(ImportGroup),
    Unsupported { name: String, line: usize },
}

// One <PropertyGroup>, e.g. Widget.vcxproj's ConfigurationType assignment.
pub struct PropertyGroup {
    pub condition: Option<String>,
    pub properties: Vec<Setting>,
    pub line: usize,
}

// <ItemDefinitionGroup><ClCompile>...</ClCompile></ItemDefinitionGroup> — per-item-type defaults.
pub struct ItemDefinitionGroup {
    pub condition: Option<String>,
    pub definitions: Vec<ItemDefinition>,   // one per <ClCompile>, <Link>, etc.
    pub line: usize,
}

// <ItemGroup><ClCompile Include="..." /></ItemGroup> — actual items, e.g. source files.
pub struct ItemGroup {
    pub condition: Option<String>,
    pub items: Vec<Item>,
    pub line: usize,
}

pub struct Item {
    pub item_type: String,               // "ClCompile", "ClInclude", ...
    pub include: Option<String>,         // raw spec: may be `;`-separated or a wildcard
    pub condition: Option<String>,
    pub metadata: Vec<Setting>,          // per-item overrides, e.g. %(PreprocessorDefinitions)
    pub line: usize,
}

// Shared by PropertyGroup's properties, ItemDefinition's metadata, and Item's metadata —
// same XML shape (<Name Condition="...">value</Name>), same downstream handling.
pub struct Setting {
    pub name: String,
    pub value: String,       // raw, unexpanded
    pub condition: Option<String>,
    pub line: usize,
}
```

The single most consequential decision here is that this is **a vector, not a map**. It would be
tempting to fold `<PropertyGroup>` contents directly into a `HashMap<String, String>` while
reading — after all, that's what everyone downstream eventually wants. But whether
`Widget.vcxproj`'s `ConfigurationType` PropertyGroup applies depends on its `Condition`, which
depends on the configuration/platform being evaluated, which this stage deliberately doesn't know
yet. A `HashMap` built here would have already thrown away the information needed to decide that —
and it would look complete while doing it, which is worse than an obvious gap. So the IR stays
document-ordered and faithful to the file; evaluation happens in later, dedicated passes.

A second, smaller decision: `PropertyGroup` settings and `ItemDefinitionGroup`/`ItemGroup` items
both reuse the same `Setting` struct. In the XML they're indistinguishable
(`<Name Condition="...">value</Name>`), and every later pass treats them identically — expand the
value, check the condition — so two structurally identical types would just be duplication with
extra steps.

Also worth noting: `Element::Unsupported` isn't a failure. `<Choose>`/`<When>`, or any construct
this reader doesn't model, is recorded as this variant *and* emitted as a warning, rather than
silently dropped. A `<Choose>` block can gate which flags apply to a file — dropping it in silence
would produce a compilation database that looks fine and is wrong, which is exactly the outcome
CLAUDE.md calls out as worse than a loud failure.

### Every XML tag msvclsp processes

The table below is every element `.vcxproj`/`.props`/`.targets` parsing actually branches on, what
it becomes in the `Element`/`Item` IR, and what happens to it from there. `docs/glossary.md`
explains what each of these *means* in MSBuild's own terms; this is about what msvclsp's reader
specifically does with it.

| Tag | Becomes | What happens to it |
| --- | --- | --- |
| `<Project>` | — (the root; not itself an `Element`) | Its immediate children are read as the `elements: Vec<Element>` list. A `<Project>` root is what tells `project.rs` this file is something it can parse at all — anything else fails as `project-schema`. |
| `<PropertyGroup>` | `Element::Properties(PropertyGroup)` | Its `<Name>value</Name>` children become `Setting`s. Evaluated in stage 4/5: each assignment expands `$(...)` eagerly and, if its `Condition` holds, is written into `Properties`. |
| `<ItemDefinitionGroup>` | `Element::ItemDefinitions(ItemDefinitionGroup)` | Deferred by stage 3 (condition not yet evaluated), then settled in stage 5 into per-item-type defaults — e.g. its `<ClCompile>` child becomes the project-wide `ClCompile` defaults every source file starts from. |
| `<ItemGroup>` | `Element::Items(ItemGroup)` | Also deferred, then settled in stage 5. Its children (`<ClCompile Include="...">`, etc.) become `Item`s; only `ClCompile` items go on to become `SourceItem`s and, eventually, `compile_commands.json` entries. |
| `<Import>` | `Element::Import(Import)` (inside the *importing* file) | Resolved in stage 3: its `Project` attribute is macro-expanded, its `Condition` checked, and — if both allow it — the target file is parsed with this same reader and spliced into the stream at that point. |
| `<ImportGroup>` | `Element::ImportGroup(ImportGroup)` | A `Condition`-gated wrapper around one or more `<Import>`s; each contained import is resolved exactly as a top-level one, gated by both the group's condition and its own. |
| `<ClCompile Include="...">` (inside an `ItemGroup`) | `Item { item_type: "ClCompile", .. }` | The one item type msvclsp actually turns into output: stage 5 walks these into `SourceItem`s, stage 6 turns each into a `FileCompilation`, stage 7 into a `compile_commands.json` entry. |
| `<ClCompile>` (inside an `ItemDefinitionGroup`) | `ItemDefinition { item_type: "ClCompile", .. }` | Composed into the project's default `ClCompile` metadata in stage 5, before any item is evaluated against it. |
| `<ProjectConfiguration Include="Debug|x64">` | `Item { item_type: "ProjectConfiguration", .. }` | Read by `items::declared_configurations` to know which configuration/platform pairs the project itself declares, purely so msvclsp can warn if you ask for one it doesn't. |
| Any other item type (`<ClInclude>`, `<ResourceCompile>`, `<Link>`, `<None>`, ...) | `Item { item_type: "ClInclude", .. }` etc. | Parsed into the generic `Item` shape like anything else inside an `<ItemGroup>`, but never type-matched again downstream — msvclsp has no use for a header list or a resource list, so these items are read and then ignored. |
| `<Target>`, `<UsingTask>`, `<ProjectExtensions>` | nothing — skipped during lowering | These describe the *build* half of MSBuild (tasks that run), not evaluation, and can't change how a file parses. Skipped silently rather than warned about, since every ordinary project file contains them. |
| `<Choose>` / `<When>` / `<Otherwise>` | `Element::Unsupported { name: "Choose", .. }` | Not modeled. Recorded as `Unsupported` and warned about, because unlike `<Target>` a `<Choose>` block can gate real settings — dropping it in silence risks a database that looks right and isn't. |
| Anything else unrecognized at the `<Project>` level | `Element::Unsupported { name, .. }` | Same treatment as `<Choose>`: recorded and warned about rather than silently dropped. |
| `Condition="..."` (attribute, on almost any element above) | `Option<String>` field on the owning `Element`/`Item`/`Setting` | Carried as raw text through stages 1–3, evaluated for real in stage 3 (for `<Import>`) or stage 5 (for everything else) against whatever property bag is in scope at that point. |
| `Include=`, `Exclude=`, `Remove=`, `Update=` (attributes on an item) | fields on `Item` | Resolved in stage 5: `Include` (and wildcards within it) adds items, `Exclude` subtracts specs from the same `Include`, `Remove` drops previously-declared items, `Update` attaches metadata to an item declared elsewhere without adding or removing anything. |
| `Label="..."` (attribute) | not modeled | Read by nothing — it's a Visual Studio IDE grouping hint (`Label="ProjectConfigurations"`, `Label="Globals"`) with no evaluation meaning, so msvclsp's reader ignores it entirely. |

Everything *inside* a `ClCompile` item or `ItemDefinitionGroup` entry — `<WarningLevel>`,
`<PreprocessorDefinitions>`, `<AdditionalIncludeDirectories>`, `<LanguageStandard>`, and several
dozen more — is metadata, not a distinct tag type; all of it flows through the generic `Setting`
shape above. Which of those *names* stage 6 actually turns into a flag (and which it deliberately
skips, like `TreatWarningAsError` or `PrecompiledHeader`) is its own table, in
[ADR 12](adr/12-clcompile-flag-extraction.md).

## Stage 3: following imports — `ProjectTree`

`Widget.vcxproj` imports `common.props`, and that's where its `WarningLevel` and include path
actually come from. Resolving that import can't be a simple "collect referenced files" pre-pass,
because the import's path can itself contain a macro (`<Import Project="$(SomeDir)\shared.props" />`
is common in real solutions) and its `Condition` can gate whether it's followed at all. So import
resolution *is* the first real evaluation pass, living in `msbuild/imports.rs`, and it's the point
where macro expansion (stage 4) and condition evaluation first get used.

It walks `Widget.vcxproj`'s elements in order, and each time it meets an `<Import>`, it resolves
the target path, parses that file with stage 2's reader, and splices its elements into the stream
*at that point* — not appended at the end. The result:

```rust
pub struct ProjectTree {
    pub files: Vec<PathBuf>,       // evaluation order, root first
    pub deferred: Vec<Deferred>,   // ItemGroup/ItemDefinitionGroup elements, spliced in order
    pub properties: Properties,    // property bag as the whole graph left it
}
```

For our example, `files` is `[Widget.vcxproj, common.props]`, and `deferred` holds `common.props`'s
`ItemDefinitionGroup` (with its `WarningLevel`/`AdditionalIncludeDirectories`) followed by
`Widget.vcxproj`'s own `ItemDefinitionGroup` and its `ItemGroup` of source files — in the order
they'd actually apply during a real MSBuild build.

Splicing rather than appending matters concretely here: if imports were parsed separately and
tacked onto the end of the list, `common.props`'s defaults would appear to be declared *after*
`Widget.vcxproj`'s own settings, when in a real build they take effect where the `<Import>` line
sits — which for a typical project is *before* the project's own item definitions, so the
project's own settings can override the shared ones. Getting this backwards would silently flip
which value wins whenever a project overrides something the shared sheet also sets.

Every `Condition` on these deferred elements is still unevaluated at this point — MSBuild doesn't
decide which `ItemDefinitionGroup` applies against the property values in scope at the point it
was written, but against whatever the property bag looks like once the *entire* graph has been
walked. That's answered in stage 5. This pass also enforces MSBuild's own "import a file once per
project" rule (its warning MSB4011): a `.props` sheet reachable by two different paths — a common
situation once several projects share sheets — is only spliced in on the first visit, both to
match real build semantics and to stop a cyclic import from recursing forever.

## Stage 4: expanding macros — `Properties` and `Expansion`

`$(Configuration)`, `$(ProjectDir)`, `$(SomeSharedProperty)` — none of these can be resolved until
something tracks what's actually been assigned. That's `msbuild/eval.rs`'s `Properties`, a
case-insensitive bag threaded through the whole import walk:

```rust
pub struct Properties {
    values: HashMap<String, String>,
    locked: HashSet<String>,   // reserved names an ordinary assignment can't overwrite
    // ... tracks which stored values still contain something it couldn't expand
}
```

Expansion is **eager**: a value is expanded the moment it's assigned, against the bag as it stands
*right then*, and what gets stored afterward contains no `$(` at all. This is MSBuild's own rule,
and it's why `<PreprocessorDefinitions>WIDGET_DEBUG;%(PreprocessorDefinitions)</PreprocessorDefinitions>`
appends onto whatever came before rather than exploding into infinite self-reference — the
right-hand `%(PreprocessorDefinitions)` (item metadata, resolved slightly differently — see stage
5) is expanded against the bag as it was *before* this assignment took effect.

Before `common.props` or `Widget.vcxproj` assign anything, a handful of properties are pre-seeded
by `msbuild/reserved.rs` — `$(ProjectDir)`, `$(SolutionDir)`, `$(Configuration)`, `$(Platform)`,
and about twenty others MSBuild guarantees exist without any file defining them. Without this
seeding, `common.props`'s `$(ProjectDir)include` would expand to the literal string `"include"`
with the directory prefix simply missing — a database that looks plausible and points nowhere.
These names are locked: an ordinary `<ProjectDir>...</ProjectDir>` in a project file can't
silently redefine them.

For our example, by the time `common.props` is evaluated, `$(ProjectDir)` has already been seeded
to `Widget`'s absolute directory, so `$(ProjectDir)include` resolves to a real, absolute path.

Not everything can be expanded — property functions, registry lookups, and a handful of other
MSBuild constructs are out of scope. Rather than erroring or silently emptying those references,
they're copied through **verbatim** and recorded as unresolved. That choice threads through the
rest of the pipeline: a flag that ends up containing a literal `$(SomeFunction(...))` is a visible,
self-explaining bug in the output database, which is strictly better than a flag that's silently
missing something.

## Stage 5: settling items — `SourceItem`

Once the whole import graph has been walked and `Properties` holds its final values, `msbuild/items.rs`
makes a second pass over `ProjectTree::deferred` — this time evaluating every `Condition` for real,
against the *final* bag, and building up the per-file compiler settings:

```rust
pub struct SourceItem {
    pub path: PathBuf,          // absolute, normalized
    pub metadata: Metadata,     // %(Name) values that apply to this specific file
    pub declared_in: PathBuf,   // for diagnostics only
    pub line: usize,
}
```

This is a genuine two-pass process within the pass: `ItemDefinitionGroup`s (the *defaults* —
`common.props`'s `WarningLevel`, `Widget.vcxproj`'s `PreprocessorDefinitions`) are fully resolved
first, across the whole deferred stream, and only then are the individual `ClCompile` items
(`src\widget.cpp`) evaluated against those finished defaults. Doing it in one combined sweep would
make an item's settings depend on its position in the file relative to the `ItemDefinitionGroup`s
that apply to it — which turned out to be wrong: a real-world project was found whose `ClCompile`
items are declared *before* the `ItemDefinitionGroup` that sets their flags, and Visual Studio
still applies those flags. Splitting the settling into two passes fixed that without changing
anything about how groups compose among themselves.

For `Widget`'s one source file, `metadata` ends up holding `WarningLevel=Level3` (from
`common.props`), `AdditionalIncludeDirectories` pointing at `Widget\include` (also from
`common.props`, but only resolvable once `Widget.vcxproj`'s own `$(ProjectDir)` was seeded), and
`PreprocessorDefinitions=WIDGET_DEBUG` (from `Widget.vcxproj` itself).

One rule earns particular mention: an unset `%(Name)` reference — the trailing
`%(PreprocessorDefinitions)` in our `common.props` when nothing has defined it yet — expands to
nothing *silently*, without a warning. Every real project file is full of these
(`WIN32;%(PreprocessorDefinitions)` is idiomatic MSBuild), so flagging each one would drown the
one warning channel this tool has in noise on files that are completely correct.

## Stage 6: translating to flags — `FileCompilation`

`msbuild/flags.rs` takes each `SourceItem`'s settled `Metadata` and translates it into an actual
MSVC (`cl`) command-line vocabulary:

```rust
pub struct FileCompilation {
    pub file: PathBuf,
    pub include_paths: Vec<PathBuf>,
    pub defines: Vec<String>,
    pub flags: Vec<String>,     // e.g. "/W3", "/std:c++20"
}
```

For `Widget`'s one file this becomes something like `include_paths: [Widget\include]`,
`defines: [WIDGET_DEBUG]`, `flags: ["/W3"]` (from `WarningLevel=Level3`).

Only settings that change *how a file parses* are translated here — include paths, defines,
language standard, warning level. Anything that only affects codegen or linking (optimization
level, `/WX`, precompiled headers) is deliberately skipped: clangd never builds or links anything,
so translating those settings would be work spent producing flags nobody consumes, and in a few
cases (`/WX`, in particular) would actively make things worse — MSVC's warning set and clang-cl's
don't line up closely enough for "treat warnings as errors" to mean the same thing in both, so
honoring it would turn a clean file into one the editor reports as broken.

The full project, gathering every source file's `FileCompilation`, becomes:

```rust
pub struct EvaluatedProject {
    pub path: PathBuf,               // the .vcxproj itself
    pub files: Vec<FileCompilation>,
    pub dependencies: Vec<PathBuf>,  // every file read: the .vcxproj plus every import
}
```

`dependencies` isn't used to build the database at all — it exists so the CLI's staleness cache
(`db.rs`) can later ask "has any file this evaluation touched changed since last time" without
re-running the whole pipeline.

## Stage 7: assembling the database — `CompileCommand`

Finally, `compdb.rs` combines every project's `EvaluatedProject` with the toolchain's own system
include paths (found separately, via `vswhere` and the installed MSVC/Windows SDK layout — not
part of the `.sln`/`.vcxproj` model at all) into the shape `compile_commands.json` actually needs:

```rust
pub struct CompileCommand {
    pub directory: PathBuf,
    pub file: PathBuf,
    pub arguments: Vec<String>,
}
```

`arguments` is built to be a genuinely runnable command line, not just a bag of flags:
`["clang-cl", "--driver-mode=cl", "/I<project includes>", "/I<toolchain includes>", "/D<defines>",
<flags>, "<absolute file path>"]`. Project include paths are listed before the toolchain's system
ones, matching `cl`'s own search order, so a project header shadowing a system one of the same name
resolves the same way it would in a real build. Flags stay in MSVC spelling throughout — msvclsp
never translates them into native clang flags — and clangd is told to interpret them as such via
`--driver-mode=cl`.

For `Widget\src\widget.cpp`, the final entry looks like:

```json
{
  "directory": "C:\\Sprocket\\Widget",
  "file": "C:\\Sprocket\\Widget\\src\\widget.cpp",
  "arguments": [
    "clang-cl", "--driver-mode=cl",
    "/IC:\\Sprocket\\Widget\\include",
    "/I<toolchain ucrt/um/shared includes>",
    "/DWIDGET_DEBUG",
    "/W3",
    "C:\\Sprocket\\Widget\\src\\widget.cpp"
  ]
}
```

## The shape of the whole pipeline

```
Sprocket.sln          ──sln.rs──►  Solution { projects: [SolutionProject], configurations }
                                          │
                                          ▼  (per project, per selected config/platform)
Widget.vcxproj  ──project.rs──►  ProjectFile { elements: Vec<Element> }   (no evaluation)
      │
      ▼ (imports.rs walks & splices, following <Import>, resolving via eval.rs's Properties)
   ProjectTree { files, deferred: Vec<Deferred>, properties: Properties }
      │
      ▼ (items.rs: settle deferred groups against the final Properties)
   Vec<SourceItem> { path, metadata: Metadata }
      │
      ▼ (flags.rs: translate settled metadata into cl switches)
   EvaluatedProject { files: Vec<FileCompilation> { include_paths, defines, flags } }
      │
      ▼ (compdb.rs: fold in toolchain include paths, build a runnable command line)
   Vec<CompileCommand> { directory, file, arguments }  ──►  compile_commands.json
```

Each stage's struct is shaped around what that stage does and does not yet know. The `.sln` stage
knows membership and configuration mapping but nothing about a project's contents. The reader
(`ProjectFile`) knows a file's contents but nothing about what any of it evaluates to. `ProjectTree`
knows the fully-resolved property bag but hasn't yet decided which conditional groups actually
apply. `SourceItem` knows exactly which settings apply to exactly which file, but still in MSBuild's
own vocabulary. `FileCompilation` has crossed into `cl`'s vocabulary. Only at the very end,
`CompileCommand`, does the model become something an external tool — clangd — can actually run.
That gradual narrowing is deliberate: collapsing two of these stages into one map or one struct
earlier would mean making an evaluation decision before the information needed to make it
correctly is available, and MSBuild's evaluation order is exactly the kind of thing that goes wrong
silently when you do that.
