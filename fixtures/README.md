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

Where these come from — hand-authored minimal solutions, sanitized real-world examples, or
both — is still an open question.
