# sln2cc

Generates a `compile_commands.json` compilation database from a Visual Studio `.sln` and its
`.vcxproj`/`.props` files, so `clangd` gets real language support on MSBuild-built C/C++
projects without hand-converting them to CMake.

```sh
sln2cc list-configs path/to/Solution.sln
sln2cc generate path/to/Solution.sln --configuration Debug --platform x64
```

This crate is the CLI half of the [sln2cc](https://github.com/cbancroft/sln2cc) project, which
also ships a Neovim plugin that drives this binary and points `clangd` at its output — see the
main repository for the plugin, full documentation, and design docs.

The binary builds on any platform but only does useful work on Windows with Visual Studio
installed, since toolchain and Windows SDK discovery depend on it.
