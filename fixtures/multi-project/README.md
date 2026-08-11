# multi-project

Two projects in one solution — `App` (an `Application`) and `Lib` (a `StaticLibrary`) — each with
its own `WarningLevel`, preprocessor definition, and include directory, to verify per-project
settings don't leak across project boundaries: `App`'s entry must carry `APP_DEBUG`/`/W3` and
never `LIB_DEBUG`/`/W4`, and vice versa. Both projects declare only `Debug|x64`, no imports of the
real Visual Studio VC targets (see [ADR 25](../../docs/adr/25-fixture-simple-and-golden-harness.md)
— fixtures evaluate under `FakeToolchain` on CI, which has no Visual Studio installed).
`cli/tests/generate.rs`'s `multi_project_matches_golden` compares the generated
`compile_commands.json` against `cli/tests/golden/multi-project.json`.
