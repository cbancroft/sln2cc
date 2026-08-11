# multi-config

Two solution configuration/platform pairs, `Debug|x64` and `Release|x64` — a non-default name
paired with the platform, not just the bare `Debug`/`Release` × `x86`/`x64` default set. Two
projects: `App` maps each solution pair straight onto the same project pair, while `Tool` maps
solution `Debug|x64` onto project `Debug|Win32` (a differing pair — see
[ADR 05](../../docs/adr/05-sln-configuration-mapping.md)'s consequence that this fixture needed
one) and declares no `Build.0` row for `Release|x64` at all, so it's excluded from that
configuration entirely. `cli/tests/generate.rs`'s `multi_config_matches_golden` generates for
`Debug|x64` and compares against `cli/tests/golden/multi-config.json`, exercising `App`'s and
`Tool`'s divergent per-project pairs. No imports of the real Visual Studio VC targets (see
[ADR 25](../../docs/adr/25-fixture-simple-and-golden-harness.md)).
