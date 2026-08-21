# sln2cc

clangd support for C/C++ projects built with Visual Studio solutions.

## Introduction

This tool started as something fun to make while I was laid off from Bethesda. It served
as an easy way to learn more about how to apply AI (mainly Claude Code) to a project while
also teaching myself Rust and getting into Neovim plugin development. It grew out of my
absolute love/hate relationship with Visual Studio, and its utter lack of good Vim plugins.
Every time I tried to use it, I wished I had my Neovim setup integrated with it. I managed
to cobble together a halfway decent CMake definition of our project that kind of did the job,
but I wanted something that could integrate more easily with any solution. With that idea, 
`sln2cc` was born. It's not perfect, but it does pretty close to exactly what I want it to.
It is still very much a toy, but it has made my life easier when dealing with arbitrary 
Windows projects that don't already have a CMake integration that other tools can 
generate completion from.

I am trying to reduce the AI-Slop feel of the project as I take a bigger hands-on role in 
its development, so bear with me as I eliminate all the 'load-bearing's from the code base
as I continue development.

## Overview

`clangd` needs a `compile_commands.json` compilation database to know how each file is
compiled. CMake projects generate one automatically; Visual Studio solutions don't.
sln2cc parses a `.sln` and its `.vcxproj`/`.props` files for a chosen
configuration/platform and produces that database — so you can use Neovim on MSBuild
projects without hand-converting them to CMake.

## Layout

| Path        | What                                                            |
| ----------- | --------------------------------------------------------------- |
| `cli/`      | Rust binary: parses solutions, emits `compile_commands.json`.     |
| `nvim/`     | Neovim plugin: drives the binary, manages clangd.                 |
| `fixtures/` | Sample MSVC solutions for tests and manual verification.          |
| `docs/`     | Project spec, engineering spec, milestones, glossary.             |

## Status

**MVP complete.** The CLI parses a real solution and generates a working
`compile_commands.json` (`generate`, `list-configs`), and the Neovim plugin drives it with real
commands, config validation, and `:checkhealth`. MVP-34's end-to-end verification passed against
both a fixture and a real external multi-project solution: real `clangd` attaches, completes,
resolves go-to-definition across translation units, and reports zero missing-include diagnostics
in either case, and switching configuration/platform and regenerating visibly changes the emitted
flags (`/MTd` → `/MT`, `/DNDEBUG` appearing in the non-debug configuration) while producing the
same file count. See [docs/todos.md](docs/todos.md) for the exact list,
[docs/milestones.md](docs/milestones.md) for the full MVP/V1/V2 breakdown, and
[docs/engineering_spec.md](docs/engineering_spec.md) for how it's built.

If MSBuild's vocabulary is unfamiliar — items, property sheets, item definition groups, the
difference between a solution configuration and a project configuration — start with
[docs/glossary.md](docs/glossary.md). sln2cc's warnings use those terms, and the terms are
the tricky part of working with MSVC project files.

The binary builds on any platform, but real use requires Windows with Visual Studio
installed, since toolchain and Windows SDK discovery depend on it.

## Using the Neovim plugin

There's no tagged/versioned release yet, but the repository is on GitHub and installable
directly. The plugin lives at `nvim/`, alongside `cli/`, not at the repository root — plain
plugin-manager specs that clone the whole repo (`"cbancroft/sln2cc"`) won't find `plugin/`,
`lua/sln2cc/`, or `doc/`, since those end up nested under `nvim/` instead of at the clone's
root. Point your plugin manager at the `nvim-plugin` branch instead, which CI keeps mirrored
to `nvim/`'s current contents on every push to `main` (see
[ADR 54](docs/adr/54-nvim-pack-subtree-branch.md)):

With `lazy.nvim`:

```lua
{
  "cbancroft/sln2cc",
  branch = "nvim-plugin",
  main = "sln2cc",
  opts = {},
}
```

With Neovim's built-in `vim.pack.add()`:

```lua
vim.pack.add({
  { src = "https://github.com/cbancroft/sln2cc", version = "nvim-plugin" },
})
require("sln2cc").setup({})
```

If you're working on the plugin itself rather than just using it, point `lazy.nvim` at a local
checkout instead so edits take effect without pushing:

```lua
{
  dir = "/path/to/sln2cc/nvim",
  main = "sln2cc",
  opts = {},
}
```

Either way, you'll also need the `sln2cc` binary itself on `PATH` (`cargo install --path cli`
from a checkout, or point the plugin's `bin` option at a `cargo build` output) and `clangd` on
`PATH`. Once both are in place and `setup()` has run, open a file inside a solution's tree,
run `:Sln2ccSelectConfig` to pick a configuration/platform, and `clangd` attaches
automatically.

Everything else — every command, every configuration option, suggested keymaps, and a
troubleshooting section built around `:checkhealth sln2cc` — is in `:help sln2cc`
(`nvim/doc/sln2cc.txt`), which is the source of truth for using the plugin; this README
only covers building and testing the repository itself.

## Building and testing

### Prerequisites

| Tool                     | Needed for                                              |
| ------------------------ | -------------------------------------------------------- |
| Rust 1.85+ + Cargo       | Building and testing the CLI.                             |
| Neovim 0.12+             | Running the plugin and its tests.                         |
| [mini.test]               | The plugin test harness.                                  |
| clangd                   | Manual end-to-end verification only.                      |
| Visual Studio 2019/2022  | Manual end-to-end verification only (Windows).            |

[mini.test]: https://github.com/nvim-mini/mini.test

Rust builds, lints, and unit/integration tests run on any OS — toolchain discovery is
mocked out. Only manual verification against a real solution requires Windows with Visual
Studio.

1.85 is the declared minimum supported Rust version, set by `clap` and recorded as
`rust-version` in `cli/Cargo.toml`. An older toolchain fails with a clear cargo message rather
than a confusing compile error; `rustup update` is the fix.

### CLI

All Cargo commands run from `cli/`:

```sh
cd cli

cargo build                      # build the binary
cargo test                       # unit + integration tests
cargo fmt --check                # formatting (enforced in CI)
cargo clippy -- -D warnings      # lints (enforced in CI)
```

Integration tests in `cli/tests/` compare generated output against golden files in
`cli/tests/golden/`. They run against the sample solutions in `fixtures/` with a
`FakeToolchain` injected, which is what keeps the goldens stable across machines with
different Visual Studio installations — never regenerate them against a live toolchain.

Many tests are currently `#[ignore]`d placeholders; run `cargo test -- --ignored` to see
what's still unimplemented.

### Neovim plugin

Tests use [mini.test] and run headless:

```sh
cd nvim
nvim --headless --noplugin -u tests/minimal_init.lua \
  -c "lua require('mini.test').setup()" \
  -c "lua MiniTest.run()"
```

mini.test is located automatically at `stdpath("data")/site/pack/core/opt/mini.test` if
you already have it installed via `vim.pack` (Neovim's built-in plugin manager, 0.12+).
Otherwise clone it and point `MINI_TEST_DIR` at the checkout:

```sh
git clone --depth 1 https://github.com/nvim-mini/mini.test /tmp/mini.test
MINI_TEST_DIR=/tmp/mini.test nvim --headless --noplugin -u tests/minimal_init.lua \
  -c "lua require('mini.test').setup()" \
  -c "lua MiniTest.run()"
```

`--noplugin` matters: without it your own config's plugin scripts load into the test
session and can throw unrelated errors.

Plugin tests stub out the CLI binary rather than invoking it, so they need neither a Rust
build nor a Windows toolchain — they cover plugin logic only (discovery caching,
persistence, clangd lifecycle).

To load the plugin from this checkout while developing, add `nvim/` to your runtimepath —
e.g. with `lazy.nvim`:

```lua
{ dir = "D:/code/sln2cc/nvim", main = "sln2cc", opts = {} }
```

Note that plugin managers generally expect `lua/`, `plugin/`, and `doc/` at the *repository*
root rather than in a subdirectory; how this is packaged for real installs is still an open
question (see [docs/engineering_spec.md](docs/engineering_spec.md)).

### Manual end-to-end verification

Automated tests stop at "is the generated `compile_commands.json` correct." Confirming
clangd actually behaves correctly against it is a manual step for now:

1. Generate a database for a fixture (or a real solution) with the CLI.
2. Open one of its source files in Neovim with the plugin active.
3. Check that completion, diagnostics, and go-to-definition work, and that clangd isn't
   reporting missing includes.
