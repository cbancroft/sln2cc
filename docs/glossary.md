# Glossary

MSBuild and Visual Studio vocabulary, as the rest of these documents use it — and as msvclsp's own
warnings use it, since a warning that names an `ItemDefinitionGroup` is only actionable if you know
what one is.

Two conventions throughout:

- Definitions describe **MSBuild**, which is the thing your project files are written against.
  Where msvclsp does something narrower, a trailing note says so — those gaps are the difference
  between a compilation database that is right and one that merely looks right.
- The last two sections are msvclsp's own vocabulary and a short list of terms that sound like
  MSBuild and are not.

## Files

**Solution (`.sln`)** — the outer container: a list of member projects, the configuration/platform
pairs the solution offers, and how each pair maps onto each project. It is a *line-oriented text
format, not XML*, which is why msvclsp parses it with a separate module from everything else.

**Solution folder** — a grouping node in a solution's tree with no build meaning. It appears in the
`.sln` as a project with its own type GUID and no file behind it. Skipped in silence.

**Project (`.vcxproj`)** — one buildable unit: a library or an executable, its source files, and its
compiler settings. XML with a `<Project>` root. Visual Studio identifies the C/C++ kind by the type
GUID `{8BC9CEB8-8B4A-11D0-8D11-00A0C91BC942}` stamped in the solution rather than by the file
extension, and so does msvclsp.

**Property sheet (`.props`)** — a fragment of project XML meant to be `<Import>`ed, holding settings
shared by several projects. By convention imported *early*, so a project can override what it
defines. Nothing in the format distinguishes a `.props` from a `.vcxproj`; the extension is a
convention and msvclsp parses all three with one reader.

**Targets file (`.targets`)** — the same format again, by convention imported *late* and holding
build logic rather than settings. Microsoft's own `Microsoft.Cpp.targets` is what turns evaluated
properties into actual compiler invocations.

**`.vcxproj.filters`** — the IDE's folder tree for the Solution Explorer. Pure presentation; it has
no effect on a build and msvclsp never reads one.

**`Microsoft.Cpp.$(Platform).user.props`** — a per-user, per-machine property sheet every Visual
Studio project imports, usually holding local include paths. It lives in your user profile, not in
the repository, which is a common reason a project builds on one machine and not another.

**Compilation database (`compile_commands.json`)** — the file this whole tool exists to produce: a
JSON array with one entry per source file, giving the file, the directory to interpret it relative
to, and the exact compiler arguments. CMake emits one natively; Visual Studio does not. clangd reads
it to learn how each file is compiled.

## Properties

**Property** — a single named string. MSBuild's scalar; the counterpart to an item, which is a list.

**`PropertyGroup`** — the element that assigns properties. Assignments take effect in document
order and the last one wins, which is why the order of a project's elements is load-bearing and why
msvclsp's reader preserves it rather than collecting into a map.

**Property reference (`$(Name)`)** — how a property's value is used. An undefined property expands
to the *empty string* rather than erroring, which is the single most common cause of a database that
generates cleanly and is wrong: `$(TypoDir)include` silently becomes `include`. msvclsp warns about
every undefined reference for exactly this reason.

**Property bag** — not an MSBuild term; msvclsp's name for the accumulated set of property values at
a point in evaluation. See the msvclsp section below.

**Reserved property** — a property MSBuild computes itself and forbids a project file from
assigning: `$(MSBuildProjectDirectory)`, `$(MSBuildThisFileDirectory)`, `$(MSBuildProjectName)` and
the rest of the `MSBuild*` family. msvclsp seeds these and warns if a project tries to redefine one.

**Global property** — a property supplied from *outside* the project, by the command line or by the
solution build, which a project file also cannot override. `$(Configuration)`, `$(Platform)` and the
`$(Solution*)` family arrive this way. Their absence is why evaluating a `.vcxproj` against an empty
property bag matches no configuration-specific settings at all.

**Well-known property** — one that no project file defines and every project file uses, because
Microsoft's own imported `.props` define it: `$(ProjectDir)`, `$(ProjectName)`, `$(OutDir)`,
`$(IntDir)`. Note the trailing-separator convention, which is inconsistent *on purpose*:
`$(ProjectDir)` ends with a `\` and `$(MSBuildProjectDirectory)` does not, because files concatenate
them directly — `$(ProjectDir)inc` has to produce `...\app\inc`.

**Property function** — a method call inside a reference: `$(Version.Substring(0,2))`,
`$([System.IO.Path]::Combine(...))`, `$([MSBuild]::VersionGreaterThan(...))`. Real MSBuild, and
outside msvclsp's supported subset — such a reference is copied through *as written* and warned
about, so a flag containing it names its own cause instead of silently going missing.

**Eager expansion** — MSBuild's rule that a property's value is expanded at the moment it is
*assigned*, against the bag as it stood then. It is why `<Defs>$(Defs);_DEBUG</Defs>` appends
instead of looping forever, and why a self-referential property cannot be constructed.

## Items

**Item** — one entry in a named, typed list. Items are MSBuild's only plural container, and in a
C++ project they are almost always files. In XML, an item is an element inside an `<ItemGroup>`
whose *element name* is the item type:

```xml
<ItemGroup>
  <ClCompile Include="src\main.cpp" />
  <ClCompile Include="src\util.cpp">
    <PreprocessorDefinitions>UTIL;%(PreprocessorDefinitions)</PreprocessorDefinitions>
  </ClCompile>
  <ClInclude Include="inc\app.h" />
</ItemGroup>
```

That declares two `ClCompile` items and one `ClInclude` item. For msvclsp the `ClCompile` items
*are* the compilation database: one item becomes one `compile_commands.json` entry, and the metadata
that ends up attached to it is where every include path, define and flag on that entry comes from.

**Item type** — the element name, and the name of the list the item joins: `ClCompile`, `ClInclude`,
`Link`, `ResourceCompile`, `ProjectConfiguration`, `None`. Matched case-insensitively, so
`<Clcompile>` and `<ClCompile>` are the same list.

**Item spec** — the value of `Include`. Usually one file path, but it may be a semicolon-separated
list (`Include="a.cpp;b.cpp"`) or a wildcard (`Include="src\*.cpp"`, `Include="src\**\*.cpp"`).
Relative paths resolve against the **project's** directory, not the directory of the file the item
was written in — which is why a shared property sheet that means its own directory has to spell
`$(MSBuildThisFileDirectory)`.

**`ItemGroup`** — the element items are declared inside. It may carry a `Condition` gating every
item within it.

**Item metadata** — name/value pairs attached to an individual item, written as child elements. For
a `ClCompile` item this is the entire compiler configuration for that one file:
`AdditionalIncludeDirectories`, `PreprocessorDefinitions`, `LanguageStandard`, `RuntimeLibrary` and
several dozen more.

**Metadata reference (`%(Name)`)** — the metadata equivalent of `$(Name)`, referring to the current
value of that metadata on the item being defined. Its overwhelmingly common use is
self-reference — see *inheritance* below.

**Well-known metadata** — metadata MSBuild derives from an item's spec rather than from anything
written down: `%(Identity)`, `%(Filename)`, `%(Extension)`, `%(FullPath)`, `%(RelativeDir)`,
`%(RecursiveDir)`. msvclsp does **not** model these; a reference to one is copied through verbatim
and warned about.

**`ItemDefinitionGroup`** — where an item type's *default* metadata is declared, and where nearly
every compiler setting in a real `.vcxproj` actually lives. `<ClCompile>` inside an
`ItemDefinitionGroup` sets the project-wide defaults; `<ClCompile Include="...">` inside an
`ItemGroup` declares a file. The same element name means two different things depending on which
group encloses it, which is worth reading twice.

A definition group applies only to items declared **after** it. Visual Studio always writes the
definitions above the items, so this rarely bites — but it means order matters, not just presence.

**Inheritance** — how a per-file or per-sheet setting *adds to* a shared one instead of replacing
it, expressed as a self-reference:

```xml
<PreprocessorDefinitions>UTIL;%(PreprocessorDefinitions)</PreprocessorDefinitions>
```

This is what the "Inherit from parent or project defaults" checkbox in the Visual Studio property
pages writes. Delete the `%(...)` and the file no longer sees the project's defines. Where the
reference sits decides where the inherited list lands, so the semicolon order is significant.

**`Exclude`** — subtracts specs from the `Include` being added by the same element. Only does real
work against a wildcard.

**`Remove`** — drops items from a list declared earlier. `<ClCompile Remove="legacy.cpp" />`.

**`Update`** — attaches or changes metadata on items declared *elsewhere*, without adding or
removing any. This is how a `Directory.Build.props` or a shared sheet configures one specific file
it does not itself declare.

**Item list reference (`@(ItemType)`)** — the whole list, as a semicolon-joined string. **Item
transform** (`@(ClCompile->'%(Filename).obj')`) maps over it. Both are real MSBuild and both are
outside msvclsp's subset; they are copied through verbatim and warned about.

**`ClCompile`** — the item type holding C and C++ source files. The one msvclsp cares about.

**`ClInclude`** — header files. Recorded by the project, listed in the IDE, and not a translation
unit, so they produce no compilation database entry. clangd handles headers by inferring flags from
whichever source file includes them.

**`ProjectConfiguration`** — items in the `Label="ProjectConfigurations"` group at the top of every
`.vcxproj`, declaring which configuration/platform pairs the project supports. Ordinary items with
metadata, not a special element — msvclsp reads them to warn when you ask for a pair the project
never declared.

## Conditions

**`Condition`** — an attribute allowed on nearly every element, gating whether it applies. Evaluated
against the property values in scope. The idiom you will see most:

```xml
<ItemDefinitionGroup Condition="'$(Configuration)|$(Platform)'=='Debug|x64'">
```

The quotes matter: they make an undefined property compare as an empty string rather than producing
a syntax error. msvclsp supports `==`, `!=`, `And`, `Or`, `!`, parentheses, `Exists()` and a bare
operand as a boolean. Anything else — numeric comparison, `HasTrailingSlash`, a property function —
is a hard error rather than a guess, because inventing a truth value either drops a group carrying
every include path or applies one the project never asked for.

**`Exists('path')`** — a condition function testing for a file or directory, resolved relative to
the directory of the file the condition is written in. How an optional import guards itself.

## Imports and evaluation

**`Import`** — pulls another file's contents in at that point in the document, as though they had
been written there. The mechanism by which every real project acquires its settings: a `.vcxproj`
is mostly imports.

**Import graph** — the tree of files reached by following imports from a project. Order is
everything, because a later definition overrides an earlier one. An import's path can itself contain
property references and its `Condition` decides whether the path is even resolved, so resolving
imports *is* property evaluation — it cannot be done as a separate pre-pass.

**Duplicate import (MSB4011)** — MSBuild imports a given file at most once per project and warns on
the second attempt. One rule covering both diamond imports (two sheets importing a common third)
and cycles.

**Evaluation pass** — MSBuild evaluates a project in ordered phases before running anything:
properties and imports first, then item definitions, then items, and only then targets. The phasing
is not bureaucracy — it is why an item's `Condition` is judged against the property values
evaluation *ended* with, rather than the ones in scope where the item was written.

**Target** — a named, ordered set of tasks: the *build* half of MSBuild, as opposed to the
evaluation half. msvclsp never runs a target, which is why `<Target>`, `<UsingTask>` and
`<ProjectExtensions>` are skipped silently rather than warned about; none of them can change how a
source file parses.

**`Choose`/`When`/`Otherwise`** — MSBuild's if/else-if for whole blocks of properties or items. Not
supported by msvclsp: it is recorded and warned about rather than dropped in silence, because a
`<When>` this tool cannot evaluate could be carrying every setting a file needs.

## Configurations and platforms

**Configuration** — a named set of build settings: `Debug`, `Release`, or anything a project invents
(`Debug-Instrumented`, `Profile`). Just a string; there is nothing special about `Debug` beyond
convention.

**Platform** — the target architecture: `Win32` (which means x86, confusingly), `x64`, `ARM64`. Also
just a string.

**Configuration/platform pair** — the two together, spelled `Debug|x64`. What you actually select
when you build, and what msvclsp requires before it can generate anything, since the flags for one
pair are not the flags for another.

**Solution configuration vs project configuration** — *these are not the same pair.* A solution
defines its own list of pairs and maps each one onto a pair for every member project, and the two
sides may differ: solution `Debug|x64` may mean `Debug|Win32` for one project and `Release|x64` for
another. The user picks a solution pair; MSBuild evaluates the project pair. A project may also be
mapped to *nothing* for a given solution pair, meaning it simply is not built. Getting this wrong
produces a database describing a build nobody asked for.

## Toolchain

**Toolset / platform toolset** — the compiler version a project builds with, named in the
`<PlatformToolset>` property: `v142` is Visual Studio 2019, `v143` is 2022. It determines which
compiler binary and which default include directories apply.

**`$(VCTargetsPath)`** — the property pointing at the C++ build system's own `.props` and `.targets`
inside the Visual Studio installation. Every `.vcxproj` imports through it, and its value depends on
which Visual Studio is installed where — so it cannot be guessed, only discovered. Until msvclsp
discovers it, the imports depending on it resolve to nothing and are skipped with a warning, which
is why flags generated today are missing everything Microsoft's own sheets would contribute.

**`vswhere`** — the small Microsoft-supplied executable that reports where Visual Studio is
installed. The supported way to locate a toolchain, as opposed to guessing at registry keys or
`C:\Program Files` paths.

**Windows SDK** — the system headers and libraries (`windows.h` and friends), versioned and
installed separately from the compiler. Its include directories are not written in your project;
they come from the toolchain, which is why clangd cannot resolve a system header without them.

**`cl`** — the MSVC compiler driver, and the flag dialect this project works in: `/I`, `/D`,
`/std:c++20`, `/EHsc`. Note that MSVC accepts both `/` and `-` prefixes.

**clang-cl** — clang wearing a `cl`-compatible command line. Passing `--driver-mode=cl` tells clangd
to interpret MSVC-form flags directly, which is why msvclsp emits `cl` flags rather than translating
them to native clang spellings: translation is one more place to introduce an error nobody can see.

**Translation unit** — one source file plus everything it includes: the unit a compiler is actually
invoked on, and the unit a compilation database entry describes. Headers are not translation units,
which is why they get no entry of their own.

**Precompiled header** — a header compiled once and reused, configured by `PrecompiledHeader` and
`/Yc`/`/Yu`. Worth knowing that `/Yu` does *not* inject the header: a source file using a
precompiled header still `#include`s it explicitly, which is why msvclsp can skip these flags
without losing any parse fidelity.

## msvclsp's own vocabulary

Terms in these documents that are not MSBuild's.

**Pass 1 / pass 2 / pass 3** — msvclsp's three evaluation stages. Pass 1 walks the import graph and
builds the property values; pass 2 settles the item groups those files deferred; pass 3 translates
the resulting metadata into `cl` flags. The first two reproduce MSBuild's own phases; the third is
purely msvclsp's, since MSBuild would hand the settings to a compiler instead.

**Property bag** — the accumulated property values at a point in evaluation, keyed
case-insensitively as MSBuild keys them.

**Deferred element** — an `ItemGroup` or `ItemDefinitionGroup` that pass 1 collected without
evaluating, in the order evaluation reached it, for pass 2 to settle against the final property
values. Deferral is not laziness — it is what MSBuild's phase ordering requires.

**Settle** — to finally evaluate a deferred element's conditions and expand its values, producing
the source files and metadata that apply.

**Finding vs warning** — a *finding* is something an evaluation step noticed and recorded without
being able to phrase it, because it knows the construct but not which file or line it came from. A
*warning* is a finding once a caller has attached that context and worded it. The split is why
expansion and condition evaluation take no reporter.

**Golden file** — a recorded expected output for a fixture, compared byte for byte by the
integration tests. Never regenerated against a real Visual Studio installation, since that would
make the tests unreproducible on any other machine.

**`FakeToolchain`** — the test implementation of the toolchain-discovery trait, supplying fixed
fabricated paths. What makes golden files reproducible and lets the whole suite run on Linux CI.

## Not what it sounds like

**`%(Inherited)`** — does not exist. There is no such token in MSBuild. The Visual Studio property
pages show an "Inherit from parent or project defaults" checkbox, and what it writes into the file
is the ordinary self-reference `%(PreprocessorDefinitions)`. If you are looking for the literal
token, you will not find it.

**`Win32`** — means x86 / 32-bit, not "Windows". The 64-bit platform is spelled `x64`.

**A `.sln` is not XML.** It looks structured and it is not; it is a line-oriented format with
`Project(...)` entries and `GlobalSection` blocks.

**A `.props` file is not a distinct format.** It is the same `<Project>` XML as a `.vcxproj`. The
extension says how it is meant to be used, not what it is.

**`$(ProjectDir)` and `$(MSBuildProjectDirectory)` are not interchangeable.** The first ends with a
separator and the second does not, deliberately, and both conventions are relied on by files that
concatenate them.
