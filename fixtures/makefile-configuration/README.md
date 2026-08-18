# makefile-configuration

One project, `ConfigurationType=Makefile` — the shape of a vendored NMake dependency (e.g.
Detours) rather than an ordinary C++ project. `App/App.vcxproj` declares its compile settings as
`NMakePreprocessorDefinitions`/`NMakeIncludeSearchPath` on its own `PropertyGroup`, not through an
`ItemDefinitionGroup`, and imports none of the real Visual Studio VC targets (see
[ADR 25](../../docs/adr/25-fixture-simple-and-golden-harness.md) — this fixture has to evaluate
under `FakeToolchain` on CI, which has no Visual Studio installed). Its one `ClCompile` item
(`src/detours.cpp`) carries a deliberately misleading per-item `PreprocessorDefinitions` override,
so the golden output only matches if `generate` reads the `NMake*` properties instead of it.
`NMakeBuildCommandLine` is present but must not influence the generated flags — it drives a build
step clangd never performs.

`cli/tests/generate.rs`'s `makefile_configuration_matches_golden` compares its generated
`compile_commands.json` against `cli/tests/golden/makefile-configuration.json`. See
[ADR 52](../../docs/adr/52-makefile-configuration-nmake-settings.md) and V2-06 in
`docs/todos.md`.
