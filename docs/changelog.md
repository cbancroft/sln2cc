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
