# msvclsp

clangd support for C/C++ projects built with Visual Studio solutions.

`clangd` needs a `compile_commands.json` compilation database to know how each file is
compiled. CMake projects generate one automatically; Visual Studio solutions don't.
msvclsp parses a `.sln` and its `.vcxproj`/`.props` files for a chosen
configuration/platform and produces that database — so you can use Neovim on MSBuild
projects without hand-converting them to CMake.

## Layout

| Path        | What                                                            |
| ----------- | --------------------------------------------------------------- |
| `cli/`      | Rust binary: parses solutions, emits `compile_commands.json`.     |
| `nvim/`     | Neovim plugin: drives the binary, manages clangd.                 |
| `fixtures/` | Sample MSVC solutions for tests and manual verification.          |
| `docs/`     | Project spec, engineering spec, milestones.                       |

## Status

Early scaffolding — nothing is implemented yet. See [docs/milestones.md](docs/milestones.md)
for what's planned, and [docs/engineering_spec.md](docs/engineering_spec.md) for how it's
being built.

The binary builds on any platform, but real use requires Windows with Visual Studio
installed, since toolchain and Windows SDK discovery depend on it.
