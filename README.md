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
| `docs/`     | Project spec, engineering spec, milestones, glossary.             |

## Status

Early scaffolding — nothing is implemented yet. See [docs/milestones.md](docs/milestones.md)
for what's planned, and [docs/engineering_spec.md](docs/engineering_spec.md) for how it's
being built.

If MSBuild's vocabulary is unfamiliar — items, property sheets, item definition groups, the
difference between a solution configuration and a project configuration — start with
[docs/glossary.md](docs/glossary.md). msvclsp's warnings use those terms, and the terms are
the tricky part of working with MSVC project files.

The binary builds on any platform, but real use requires Windows with Visual Studio
installed, since toolchain and Windows SDK discovery depend on it.

## Building and testing

### Prerequisites

| Tool                     | Needed for                                              |
| ------------------------ | -------------------------------------------------------- |
| Rust 1.85+ + Cargo       | Building and testing the CLI.                             |
| Neovim 0.9+              | Running the plugin and its tests.                         |
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
{ dir = "D:/code/msvclsp/nvim", main = "msvclsp", opts = {} }
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
