# simple

One project, one configuration/platform. The baseline fixture: if this doesn't produce a
correct `compile_commands.json`, nothing else will.

`App/App.vcxproj` compiles `src/main.cpp` and `src/greet.cpp` against `include/greet.h`, one
`Debug|x64` configuration, no imports of the real Visual Studio VC targets (see
[ADR 25](../../docs/adr/25-fixture-simple-and-golden-harness.md) — this fixture has to evaluate
under `FakeToolchain` on CI, which has no Visual Studio installed). `cli/tests/generate.rs`'s
`simple_solution_matches_golden` compares its generated `compile_commands.json` against
`cli/tests/golden/simple.json`.
