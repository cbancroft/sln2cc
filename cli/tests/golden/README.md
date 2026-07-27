# Golden Files

Expected `compile_commands.json` output for each fixture in `../../../fixtures/`, one file
per fixture (e.g. `simple.json`).

These are only stable because integration tests inject a `FakeToolchain` instead of
performing real discovery. Real VS/SDK paths differ per machine and would make these files
unreproducible — never regenerate them against a live toolchain.
