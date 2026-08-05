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
  themselves on stderr. The commands parse, validate, and report correctly — the generation
  pipeline behind them is not built yet, so both currently report that they are unimplemented.

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

  Properties that come from your Visual Studio installation — `$(VCTargetsPath)` above all — are
  still empty, so imports depending on them continue to be skipped with a warning until toolchain
  discovery lands.
