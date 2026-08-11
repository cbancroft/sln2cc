# props-inheritance

`common.props` (sets `WarningLevel=Level3` and a base `AdditionalIncludeDirectories`) is imported
by both projects. `App.vcxproj` imports it, then imports `overrides.props` (redefines
`WarningLevel=Level4`), then contributes its own `ItemDefinitionGroup` (adds
`ExceptionHandling=Sync`, which neither sheet sets). `Widget.vcxproj` imports only `common.props`.
The golden output must show `App` at `/W4` — the later import winning, per import order
(`imports.rs`: "later definitions override earlier ones") — with the include directory
`common.props` contributed and the `/EHsc` `App` itself added, proving properties merge across
files rather than the last one replacing the whole bag. `Widget` must stay at `/W3` with no
`/EHsc` at all, proving `overrides.props` and `App`'s own settings apply only to the project that
imported them, not to every project that shares `common.props`. This is the fixture ADR 25 called
out as mattering most: import ordering is where subtly wrong flags are most likely to originate.
No imports of the real Visual Studio VC targets (see
[ADR 25](../../docs/adr/25-fixture-simple-and-golden-harness.md)). `cli/tests/generate.rs`'s
`props_inheritance_matches_golden` compares the generated `compile_commands.json` against
`cli/tests/golden/props-inheritance.json`.
