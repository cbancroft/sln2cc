# Changelog

Features and bugfixes as they land, following [Keep a Changelog](https://keepachangelog.com/en/1.1.0/)
and [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

Entries are added in the same change that implements them, written for someone *using*
sln2cc rather than someone reading the diff. Purely internal work — refactors, test
scaffolding, documentation — is omitted unless it changes behavior. At release, the
`[Unreleased]` section becomes the new version's notes.

Use these categories, omitting any that are empty: `Added`, `Changed`, `Deprecated`,
`Removed`, `Fixed`, `Security`.

## [Unreleased]

### Added

- The Neovim plugin can now be installed with Neovim's built-in `vim.pack.add()`, via a
  `nvim-plugin` branch CI keeps mirrored to the `nvim/` directory's contents. See
  "Using the Neovim plugin" in the README.

- A command line: `sln2cc list-configs <solution>` and `sln2cc generate <solution>
  --configuration <name> --platform <name>`, both accepting `--root` to say where `.sln2cc/`
  should live. `--json` switches stdout to a machine-readable envelope for the Neovim plugin;
  without it you get readable text. Failures exit with a distinct code per class and explain
  themselves on stderr. Both subcommands now actually work.

- `sln2cc generate <solution> --configuration <name> --platform <name>` writes a real
  `compile_commands.json` for the requested configuration/platform under `.sln2cc/`, discovering
  your Visual Studio installation, evaluating every project the solution maps to that pair — including
  one unchecked in Configuration Manager, since its files still deserve completions when you open
  them — and reporting the absolute path it wrote. A second run against unchanged files is a no-op;
  `--force` skips that check. Asking for a configuration/platform pair the solution does not declare
  fails with the list of valid ones instead of guessing.

- `sln2cc list-configs <solution>` lists the configuration/platform pairs (`Debug|x64`,
  `Release|x64`, and so on) the solution actually declares, in the order the solution lists them,
  and marks which one you last generated for it. Nothing to guess or open Visual Studio's
  Configuration Manager for.

- Warnings about things sln2cc noticed but could work around — an unfamiliar solution format
  version, say — are now reported rather than dropped. They appear on stderr whichever output mode
  you use, and in the `warnings` array under `--json`, including when the run then goes on to fail.
  File paths in them are shown relative to the project root so pasting the output somewhere does not
  leak your directory layout.

- `-v`/`--verbose` reports what sln2cc is working on as it goes, on stderr. Parsing a large solution
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

  Things sln2cc cannot work out are said out loud rather than guessed at: a wildcard such as
  `Include="src\*.cpp"` is not expanded yet, and now tells you which files are missing from the
  database instead of quietly producing none; asking for a configuration the project does not declare
  warns instead of handing you files with no flags on them; a setting whose value sln2cc does not
  recognize is named and skipped; and a value that still contains an unresolved `$(...)` says so
  before it reaches clangd.

  Toolchain-derived properties and the toolset's own system include directories now reach the
  database too, once `generate` discovers a Visual Studio installation to evaluate against.

- sln2cc can now find the actual compiler and Windows SDK headers on your machine, not just the
  ones your project files mention. Given a located Visual Studio installation, it reads which MSVC
  toolset version it defaults to, finds that toolset's `include` directory, and finds the newest
  complete Windows SDK installed (skipping a newer one that's missing a piece, in favor of an older
  one that works) — so `#include <windows.h>` and the C runtime headers will resolve once these
  paths reach the generated database.

- Warnings now come at two levels, so a long list is worth reading. A **warning** means sln2cc could
  not make sense of something and your flags may be wrong because of it — an unresolved `$(...)`, a
  `<Choose>` block, a wildcard whose source files are therefore missing, a setting whose value it does
  not recognize. A **note** means it is unusual but harmless: a solution saved by a newer Visual
  Studio that read fine anyway, an `<ItemGroup>` entry that configures nothing, a property sheet
  reached twice through a diamond and imported once, exactly as MSBuild would. Both appear on stderr,
  labelled `warning:` or `note:`, and both ride in the `warnings` array under `--json` with a
  `severity` of `"warning"` or `"note"`.

  Everything sln2cc reports now says what it means for you rather than just what it saw, and there
  is a full list of what it can report in the engineering spec.

- sln2cc can now find your Visual Studio installation on its own, the same way the VS installers
  register themselves — by asking `vswhere` which installed instances have the C++ compiler toolset
  and picking the newest one. Toolset and Windows SDK include paths, which is what actually makes
  `#include <windows.h>` resolve, are the next piece.

- The Neovim plugin's `setup()` now validates the options you pass it: an unrecognized key or a
  value of the wrong type is reported once, in full, via `vim.notify`, and nothing is applied — so
  a typo in your config surfaces immediately instead of causing confusing behavior later. Calling
  `setup()` more than once is safe.

- The Neovim plugin can now find your project root: it walks up from the buffer you're editing
  looking for a configured marker (`.git` by default), the same way Neovim's built-in LSP clients
  do, and falls back to your current working directory if nothing matches. `.sln2cc/` will be
  created there.

- The Neovim plugin can now find `.sln` files under your project root on its own, skipping
  directories that are never worth descending into (`.git`, `node_modules`, build output) and
  bounded by a configurable scan depth (`scan_ignore_dirs`, `scan_max_depth`). Results are
  cached rather than rescanned on every command.

- Wildcards in a `ClCompile` `Include`, `Update`, `Remove`, or `Exclude` — `src\*.cpp`, a single
  `?`, or MSBuild's recursive `src\**\*.cpp` — now actually expand to the real files they match,
  instead of being skipped with a warning. A directory a wildcard names that does not exist
  simply contributes nothing, matching what MSBuild itself does.

- `Condition` attributes can now use `HasTrailingSlash(...)`, alongside `Exists(...)`, instead of
  failing generation outright.

- The Neovim plugin now has real commands: `:Sln2ccGenerate` regenerates
  `compile_commands.json` for whichever configuration/platform you last picked (and tells you to
  run `:Sln2ccSelectConfig` first if you haven't picked one yet), `:Sln2ccSelectConfig` opens the
  configuration/platform picker, `:Sln2ccRescan` re-scans your project tree for `.sln` files, and
  `:Sln2ccRestart` restarts `clangd` against the current database. `:Sln2ccGenerate` and
  `:Sln2ccSelectConfig` restart `clangd` automatically afterward unless you've turned that off
  (`auto_restart_clangd = false`). Running any of these before `require("sln2cc").setup()` tells
  you so instead of Neovim's generic "not an editor command."

- A project file can now use `$(Property.StartsWith('...'))`, `EndsWith`, `Contains`, or `Equals`
  — the common way a solution brands a subset of projects for special handling — as a `Condition`
  or in an ordinary property value, instead of generation failing outright.

- `:checkhealth sln2cc` now does real checks instead of always erroring: the `sln2cc` binary is
  present and runnable (its version is reported), `clangd` is on `PATH`, your project root was
  actually found via a root marker rather than silently falling back to the current directory, a
  `.sln` was found, and — once a configuration/platform is selected — its `compile_commands.json`
  really exists on disk. Each failure names what to run next (`:Sln2ccSelectConfig`,
  `:Sln2ccRescan`, and so on) instead of leaving you to guess.

- The Neovim plugin now has real user documentation: `:help sln2cc` covers installation, a
  minimal `setup()` example, a first-use walkthrough, every command and configuration option,
  suggested (not bound) keymaps, and a troubleshooting section built around `:checkhealth`. The
  README's "Using the Neovim plugin" section covers the same install steps for anyone not yet
  inside Neovim, pointing at `:help sln2cc` for everything past getting started.

- `:Sln2ccGenerate!` (the bang form) forces regeneration past the CLI's own staleness cache,
  reaching the `--force` flag `generate` has always accepted from the command line. Plain
  `:Sln2ccGenerate` is unchanged.

- `:Sln2ccSelectSolution` lets you switch which `.sln` sln2cc targets when more than one is
  found under your project root, and remembers your choice across restarts so you're not asked
  again next session. Switching now also restores that solution's own remembered
  configuration/platform, regenerates `compile_commands.json` if needed, and restarts clangd
  — each solution keeps its own configuration rather than inheriting whatever was active for
  the one you switched away from.

- A solution can now be pinned via the new `pinned_solution` config option — an absolute path to
  a `.sln` file that always wins as the active solution, bypassing both your remembered choice
  and the interactive picker entirely. Meant for CI or scripting, where nothing is around to
  answer a prompt. A pinned path that doesn't exist is reported as an error rather than falling
  back to the picker.

- The MVP is now verified working end-to-end, not just implemented: a real `clangd` attaches,
  completes, resolves go-to-definition across translation units, and reports no missing-include
  diagnostics against a generated database, both for a fixture and for a real, multi-project
  Visual Studio solution with cross-directory project references — and switching
  configuration/platform and regenerating visibly changes the flags clangd receives.

- CLI failures reported by the Neovim plugin now include a one-line, kind-specific suggestion —
  "run `:Sln2ccSelectConfig`", "install/repair Visual Studio", "delete `.sln2cc/state.json`
  and regenerate", and so on — instead of just the bare underlying error message. Evaluation
  warnings (an unsupported MSBuild construct, an unfamiliar solution format, and the like) now
  also reach you as a notification after `:Sln2ccGenerate`/`:Sln2ccSelectConfig` instead of
  disappearing once the run succeeds.

### Changed

- The project is renamed from `msvclsp` to `sln2cc` — `msvclsp` implied an LSP the tool has never
  been; `sln2cc` names what it actually does (`.sln` in, `compile_commands.json` out). The CLI
  binary, the Neovim plugin's Lua module (`require("sln2cc")`), its user commands
  (`:Sln2ccGenerate` and siblings, previously `:Msvclsp*`), and the generated artifact directory
  (`.sln2cc/`, previously `.msvclsp/`) all changed names accordingly. See
  [ADR 53](adr/53-rename-msvclsp-to-sln2cc.md).

- The Neovim plugin now requires Neovim 0.12+ (previously 0.9+), since project root detection
  uses `vim.fs.root()`.

- `:Sln2ccSelectConfig` now always regenerates `compile_commands.json` for the configuration/
  platform you pick, even if no project file changed since the last run. The CLI's staleness
  cache only tracks file modification times, not which configuration/platform it was last
  built for, so picking a different one without touching any files used to silently keep
  serving the old database.

### Fixed

- A wildcarded `<Import Project="...">`, such as `$(VCTargetsPath)\ImportBefore\Default\*.props` —
  the way Visual Studio's own extension-hook directories are consumed — no longer fails generation
  outright when nothing is registered there. It now expands to every matching file, same as a
  wildcarded `ClCompile` item already does; a wildcard that matches nothing warns instead of
  erroring.

- The same problem is no longer reported over and over. One mistake in a property sheet shared by
  several projects used to produce one identical message per project that imported it; now it is
  reported once.

- `AdditionalOptions` containing a quoted argument, such as `/DBANNER="hello world"` or
  `/I"C:\Program Files\x\include"`, now reaches clangd as the single flag the project wrote instead
  of being split apart on the space inside the quotes.

- A property value with XML elements inside it no longer loses them without saying so. It is not
  valid MSBuild and sln2cc still keeps just the text, but it now names the value and the line.

- Generating against a real Visual Studio solution no longer fails outright the moment evaluation
  reaches the VS installation's own `.props`/`.targets`. Those files use MSBuild constructs well
  outside what sln2cc evaluates; a `Condition` or `<Import>` it cannot resolve there is now
  skipped with a warning instead of stopping generation, while the same construct in your own
  project files is still reported as a hard failure.

- A project whose source-file list is declared before the `ItemDefinitionGroup`s that configure it
  — a real-world project shape, not an edge case — no longer generates a `compile_commands.json`
  with every entry silently missing its defines, include directories, and warning settings. The
  affected files used to compile cleanly according to sln2cc and then have clangd flag every
  project-specific symbol as undefined, with nothing on stderr to explain why.

- `generate` no longer fails outright when a solution has a `Makefile`/NMake-configuration member
  project — common for vendored dependencies built outside the normal C++ project system, such as
  Detours. That project now just contributes nothing to the generated database, with a warning
  explaining why, instead of aborting generation for the whole solution.

- If your active solution's `.sln` has been deleted or moved since you last picked it,
  `:Sln2ccGenerate` and `:Sln2ccSelectConfig` no longer fail with a raw CLI file-not-found
  error. They now detect it, rescan, and reopen the solution picker so you can choose a
  replacement, then continue with whatever you picked.

- A `Makefile`/NMake-configuration project — a vendored dependency like Detours, built outside the
  normal C++ project system — that declares its own source files now gets real include-path and
  preprocessor-definition information for them, read from its `NMakeIncludeSearchPath`/
  `NMakePreprocessorDefinitions` properties. Previously such a project's files, once it stopped
  aborting generation outright, still got no completions or diagnostics of their own.

### Security

- A project file with pathologically nested XML no longer takes sln2cc down with it. Around 1.5 KB
  of deeply nested elements used to crash the process outright — no message, no exit code, nothing
  for the editor to report, just a tool that vanished. Such a file is now refused with an error that
  names it and an exit code your shell can act on. The limit is 32 levels of nesting; real project
  files use four, and nothing Visual Studio writes comes close.
