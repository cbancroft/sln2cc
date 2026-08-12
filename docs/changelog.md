# Changelog

Features and bugfixes as they land, following [Keep a Changelog](https://keepachangelog.com/en/1.1.0/)
and [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

Entries are added in the same change that implements them, written for someone *using*
msvclsp rather than someone reading the diff. Purely internal work — refactors, test
scaffolding, documentation — is omitted unless it changes behavior. At release, the
`[Unreleased]` section becomes the new version's notes.

Use these categories, omitting any that are empty: `Added`, `Changed`, `Deprecated`,
`Removed`, `Fixed`, `Security`.

## [Unreleased]

### Added

- A command line: `msvclsp list-configs <solution>` and `msvclsp generate <solution>
  --configuration <name> --platform <name>`, both accepting `--root` to say where `.msvclsp/`
  should live. `--json` switches stdout to a machine-readable envelope for the Neovim plugin;
  without it you get readable text. Failures exit with a distinct code per class and explain
  themselves on stderr. Both subcommands now actually work.

- `msvclsp generate <solution> --configuration <name> --platform <name>` writes a real
  `compile_commands.json` for the requested configuration/platform under `.msvclsp/`, discovering
  your Visual Studio installation, evaluating every project the solution maps to that pair — including
  one unchecked in Configuration Manager, since its files still deserve completions when you open
  them — and reporting the absolute path it wrote. A second run against unchanged files is a no-op;
  `--force` skips that check. Asking for a configuration/platform pair the solution does not declare
  fails with the list of valid ones instead of guessing.

- `msvclsp list-configs <solution>` lists the configuration/platform pairs (`Debug|x64`,
  `Release|x64`, and so on) the solution actually declares, in the order the solution lists them,
  and marks which one you last generated for it. Nothing to guess or open Visual Studio's
  Configuration Manager for.

- Warnings about things msvclsp noticed but could work around — an unfamiliar solution format
  version, say — are now reported rather than dropped. They appear on stderr whichever output mode
  you use, and in the `warnings` array under `--json`, including when the run then goes on to fail.
  File paths in them are shown relative to the project root so pasting the output somewhere does not
  leak your directory layout.

- `-v`/`--verbose` reports what msvclsp is working on as it goes, on stderr. Parsing a large solution
  takes real time, and this is how you tell it is doing something. It also switches warning paths to
  their full absolute form for when the shortened version is the confusing part. stdout is untouched
  either way, so `-v` is safe to add to a piped or scripted invocation.

- The built-in properties your project files assume exist now do exist. `$(SolutionDir)`,
  `$(ProjectDir)`, `$(ProjectName)`, `$(Configuration)`, `$(Platform)`,
  `$(MSBuildThisFileDirectory)` and the rest of that family are filled in from the solution, the
  project, and the configuration you asked for — so an include directory written as
  `$(ProjectDir)include` resolves to a real absolute path instead of losing its prefix, a property
  sheet pulled in through `$(SolutionDir)shared\common.props` is actually found, and the
  `PropertyGroup` guarded by `'$(Configuration)|$(Platform)'=='Debug|x64'` applies to the
  configuration you picked rather than to none of them. `$(MSBuildThisFileDirectory)` tracks the file
  being read, so a shared property sheet still finds paths relative to itself rather than to the
  project that imported it. Properties MSBuild reserves for itself are protected: a project file
  trying to redefine one gets a warning naming the file and line, and the real value is kept.

  Properties that come from your Visual Studio installation — `$(VCTargetsPath)`,
  `$(VSInstallDir)`, `$(MSBuildExtensionsPath)` — are filled in once a Visual Studio installation
  is discovered, so the imports that reach Microsoft's own property sheets through them are no
  longer skipped for that reason.

- Your project's compiler settings now turn into actual flags. Evaluating a `.vcxproj` produces the
  list of source files it compiles and, for each one, its include directories as absolute paths, its
  preprocessor defines, and the `cl` switches that change how the file is parsed — the language
  standard, the runtime library, exception handling, RTTI, conformance mode, the instruction set, and
  the warnings the project silences. Settings that only affect code generation or linking are left
  out on purpose: clangd never links, and a flag it cannot act on is one more thing to get wrong.

  Inheritance works the way the Visual Studio property pages imply. A property sheet's settings, the
  project's own, and a per-file override compose in that order, so a single file can add one define
  without losing the project's — and the "inherit from parent or project defaults" checkbox does what
  it says. A configuration/platform-gated section applies only to the pair you asked for.

  Things msvclsp cannot work out are said out loud rather than guessed at: a wildcard such as
  `Include="src\*.cpp"` is not expanded yet, and now tells you which files are missing from the
  database instead of quietly producing none; asking for a configuration the project does not declare
  warns instead of handing you files with no flags on them; a setting whose value msvclsp does not
  recognize is named and skipped; and a value that still contains an unresolved `$(...)` says so
  before it reaches clangd.

  Toolchain-derived properties and the toolset's own system include directories now reach the
  database too, once `generate` discovers a Visual Studio installation to evaluate against.

- msvclsp can now find the actual compiler and Windows SDK headers on your machine, not just the
  ones your project files mention. Given a located Visual Studio installation, it reads which MSVC
  toolset version it defaults to, finds that toolset's `include` directory, and finds the newest
  complete Windows SDK installed (skipping a newer one that's missing a piece, in favor of an older
  one that works) — so `#include <windows.h>` and the C runtime headers will resolve once these
  paths reach the generated database.

- Warnings now come at two levels, so a long list is worth reading. A **warning** means msvclsp could
  not make sense of something and your flags may be wrong because of it — an unresolved `$(...)`, a
  `<Choose>` block, a wildcard whose source files are therefore missing, a setting whose value it does
  not recognize. A **note** means it is unusual but harmless: a solution saved by a newer Visual
  Studio that read fine anyway, an `<ItemGroup>` entry that configures nothing, a property sheet
  reached twice through a diamond and imported once, exactly as MSBuild would. Both appear on stderr,
  labelled `warning:` or `note:`, and both ride in the `warnings` array under `--json` with a
  `severity` of `"warning"` or `"note"`.

  Everything msvclsp reports now says what it means for you rather than just what it saw, and there
  is a full list of what it can report in the engineering spec.

- msvclsp can now find your Visual Studio installation on its own, the same way the VS installers
  register themselves — by asking `vswhere` which installed instances have the C++ compiler toolset
  and picking the newest one. Toolset and Windows SDK include paths, which is what actually makes
  `#include <windows.h>` resolve, are the next piece.

- The Neovim plugin's `setup()` now validates the options you pass it: an unrecognized key or a
  value of the wrong type is reported once, in full, via `vim.notify`, and nothing is applied — so
  a typo in your config surfaces immediately instead of causing confusing behavior later. Calling
  `setup()` more than once is safe.

- The Neovim plugin can now find your project root: it walks up from the buffer you're editing
  looking for a configured marker (`.git` by default), the same way Neovim's built-in LSP clients
  do, and falls back to your current working directory if nothing matches. `.msvclsp/` will be
  created there.

### Changed

- The Neovim plugin now requires Neovim 0.12+ (previously 0.9+), since project root detection
  uses `vim.fs.root()`.

### Fixed

- The same problem is no longer reported over and over. One mistake in a property sheet shared by
  several projects used to produce one identical message per project that imported it; now it is
  reported once.

- A wildcard in an `Exclude` no longer warns when it could not have cost you anything. Paired with a
  wildcard `Include` — the usual way it is written — there was nothing for it to remove in the first
  place, so it is a note now. Paired with a *literal* `Include` it still warns, because there the
  files it names really do stay in the database when the build would have dropped them, and the
  message now says so rather than claiming they are missing. A wildcard in an `Include` or `Update`
  still warns; that one does cost you files or settings.

- `AdditionalOptions` containing a quoted argument, such as `/DBANNER="hello world"`, now warns. It
  is still split on spaces and still reaches clangd as two flags, which is wrong — but it no longer
  does so silently.

- A property value with XML elements inside it no longer loses them without saying so. It is not
  valid MSBuild and msvclsp still keeps just the text, but it now names the value and the line.

### Security

- A project file with pathologically nested XML no longer takes msvclsp down with it. Around 1.5 KB
  of deeply nested elements used to crash the process outright — no message, no exit code, nothing
  for the editor to report, just a tool that vanished. Such a file is now refused with an error that
  names it and an exit code your shell can act on. The limit is 32 levels of nesting; real project
  files use four, and nothing Visual Studio writes comes close.
