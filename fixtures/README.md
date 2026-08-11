# Fixtures

Sample Visual Studio solutions of increasing complexity. These serve two purposes:

1. **Automated** — input for the Rust integration tests in `cli/tests/`, which compare
   generated output against the golden files in `cli/tests/golden/`.
2. **Manual** — something to open in Neovim to verify clangd actually behaves correctly
   against generated output, which is a manual smoke test for now (see
   `docs/engineering_spec.md`).

Each fixture is a self-contained solution with real (if trivial) source files, so clangd
has something to index.

| Fixture              | Exercises                                                    |
| -------------------- | ------------------------------------------------------------ |
| `simple/`            | One project, one configuration/platform. Baseline.            |
| `multi-project/`     | Several projects in one solution, with differing settings.    |
| `props-inheritance/` | Shared `.props` sheets and property override precedence.      |
| `multi-config/`      | Multiple configuration/platform pairs, including custom names.|

Running the tool against these will create `.msvclsp/` directories inside them; those are
gitignored.

All four are hand-authored, schema-realistic solutions rather than sanitized real-world examples —
none import the real Visual Studio VC targets, so they evaluate under `FakeToolchain` on CI, which
has no Visual Studio installed (see
[ADR 25](../docs/adr/25-fixture-simple-and-golden-harness.md)).
